//! Tests for cancellation support (Spec iteration 13).

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::cell::Cell;

use tenacious::{Canceler, RetryError, RetryPolicy, RetryStats, StopReason, cancel, stop, wait};

fn instant_sleep(_dur: Duration) {}

#[derive(Default)]
struct CancelOnCheck {
    calls: Cell<u32>,
    cancel_at: u32,
}

impl CancelOnCheck {
    fn new(cancel_at: u32) -> Self {
        Self {
            calls: Cell::new(0),
            cancel_at,
        }
    }
}

impl Canceler for CancelOnCheck {
    type Cancel = core::future::Pending<()>;

    fn is_cancelled(&self) -> bool {
        let calls = self.calls.get().saturating_add(1);
        self.calls.set(calls);
        calls >= self.cancel_at
    }

    fn cancel(&self) -> Self::Cancel {
        core::future::pending()
    }
}

// -- Sync tests --

#[test]
fn sync_cancel_before_first_attempt() {
    let flag = AtomicBool::new(true); // pre-set
    let mut policy = RetryPolicy::new().stop(stop::attempts(5));
    let result = policy
        .retry(|| Err::<(), _>("fail"))
        .sleep(instant_sleep)
        .cancel_on(&flag)
        .call();

    match result {
        Err(RetryError::Cancelled { last, attempts, .. }) => {
            assert_eq!(attempts, 0);
            assert_eq!(last, None);
        }
        other => panic!("expected Cancelled, got {:?}", other),
    }
}

#[test]
fn sync_cancel_after_first_attempt() {
    let flag = AtomicBool::new(false);
    let call_count = Cell::new(0u32);
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(5))
        .wait(wait::fixed(Duration::ZERO));

    let result = policy
        .retry(|| {
            call_count.set(call_count.get() + 1);
            Err::<(), _>("fail")
        })
        .sleep(|_dur| {
            // Cancel during the first sleep
            flag.store(true, Ordering::Relaxed);
        })
        .cancel_on(&flag)
        .call();

    match result {
        Err(RetryError::Cancelled { last, attempts, .. }) => {
            assert_eq!(attempts, 1);
            assert_eq!(last, Some(Err("fail")));
        }
        other => panic!("expected Cancelled, got {:?}", other),
    }
    assert_eq!(call_count.get(), 1);
}

#[test]
fn sync_cancellation_does_not_interrupt_running_operation() {
    let flag = AtomicBool::new(false);
    let op_completed = AtomicBool::new(false);
    let op_started = std::sync::Barrier::new(2);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(0);

    std::thread::scope(|scope| {
        let flag_ref = &flag;
        let started_ref = &op_started;
        let release_tx = release_tx;
        scope.spawn(move || {
            started_ref.wait();
            flag_ref.store(true, Ordering::Relaxed);
            release_tx
                .send(())
                .expect("cancellation coordinator should release operation");
        });

        let mut policy = RetryPolicy::new()
            .stop(stop::attempts(5))
            .wait(wait::fixed(Duration::ZERO));

        let result = policy
            .retry(|| {
                op_started.wait();
                release_rx
                    .recv()
                    .expect("operation should be released after cancel flag is set");
                op_completed.store(true, Ordering::Relaxed);
                Err::<(), _>("in-flight")
            })
            .sleep(instant_sleep)
            .cancel_on(&flag)
            .call();

        assert!(op_completed.load(Ordering::Relaxed));
        assert!(matches!(
            result,
            Err(RetryError::Cancelled {
                attempts: 1,
                last: Some(Err("in-flight")),
                ..
            })
        ));
    });
}

#[test]
fn sync_no_cancel_normal_success() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(3));
    let result = policy
        .retry(|| Ok::<_, &str>(42))
        .sleep(instant_sleep)
        .cancel_on(cancel::never())
        .call();

    assert_eq!(result.unwrap(), 42);
}

#[test]
fn stats_reports_cancelled_reason() {
    let flag = AtomicBool::new(true);
    let mut policy = RetryPolicy::new().stop(stop::attempts(5));
    let (result, stats): (Result<(), _>, RetryStats) = policy
        .retry(|| Err::<(), _>("fail"))
        .sleep(instant_sleep)
        .cancel_on(&flag)
        .with_stats()
        .call();

    assert!(matches!(result, Err(RetryError::Cancelled { .. })));
    assert_eq!(stats.stop_reason, StopReason::Cancelled);
    assert_eq!(stats.attempts, 0);
}

#[test]
fn on_exit_fires_with_cancelled_reason() {
    let flag = AtomicBool::new(false);
    let exit_calls = Cell::new(0_u32);
    let exit_reason = Cell::new(None);
    let exit_attempt = Cell::new(0_u32);
    let exit_has_outcome = Cell::new(false);
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(5))
        .wait(wait::fixed(Duration::ZERO));

    let result = policy
        .retry(|| Err::<(), _>("fail"))
        .on_exit(|state| {
            exit_calls.set(exit_calls.get().saturating_add(1));
            exit_reason.set(Some(state.reason));
            exit_attempt.set(state.attempt);
            exit_has_outcome.set(state.outcome.is_some());
        })
        .sleep(|_dur| {
            flag.store(true, Ordering::Relaxed);
        })
        .cancel_on(&flag)
        .call();

    assert!(matches!(result, Err(RetryError::Cancelled { .. })));
    assert_eq!(exit_calls.get(), 1);
    assert_eq!(exit_reason.get(), Some(StopReason::Cancelled));
    assert_eq!(exit_attempt.get(), 1);
    assert!(exit_has_outcome.get());
}

#[test]
fn on_exit_fires_when_cancelled_before_first_attempt() {
    let flag = AtomicBool::new(true);
    let exit_calls = Cell::new(0_u32);
    let exit_attempt = Cell::new(99_u32);
    let exit_has_outcome = Cell::new(true);
    let mut policy = RetryPolicy::new().stop(stop::attempts(5));

    let result = policy
        .retry(|| Err::<(), _>("fail"))
        .on_exit(|state| {
            exit_calls.set(exit_calls.get().saturating_add(1));
            exit_attempt.set(state.attempt);
            exit_has_outcome.set(state.outcome.is_some());
        })
        .sleep(instant_sleep)
        .cancel_on(&flag)
        .call();

    assert!(matches!(
        result,
        Err(RetryError::Cancelled { attempts: 0, .. })
    ));
    assert_eq!(exit_calls.get(), 1);
    assert_eq!(exit_attempt.get(), 0);
    assert!(!exit_has_outcome.get());
}

#[test]
fn on_exit_fires_when_cancelled_between_attempts() {
    let canceler = CancelOnCheck::new(3);
    let exit_calls = Cell::new(0_u32);
    let exit_attempt = Cell::new(0_u32);
    let exit_has_err_outcome = Cell::new(false);
    let call_count = Cell::new(0_u32);
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(5))
        .wait(wait::fixed(Duration::ZERO));

    let result = policy
        .retry(|| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<(), _>("fail")
        })
        .on_exit(|state| {
            exit_calls.set(exit_calls.get().saturating_add(1));
            exit_attempt.set(state.attempt);
            exit_has_err_outcome.set(matches!(state.outcome, Some(Err("fail"))));
        })
        .sleep(instant_sleep)
        .cancel_on(canceler)
        .call();

    assert!(matches!(
        result,
        Err(RetryError::Cancelled {
            attempts: 1,
            last: Some(Err("fail")),
            ..
        })
    ));
    assert_eq!(call_count.get(), 1);
    assert_eq!(exit_calls.get(), 1);
    assert_eq!(exit_attempt.get(), 1);
    assert!(exit_has_err_outcome.get());
}

#[test]
fn cancelled_last_result_preserves_ok_value_in_polling_mode() {
    let flag = AtomicBool::new(false);
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(5))
        .wait(wait::fixed(Duration::ZERO))
        .when(tenacious::on::ok(|_value: &i32| true));

    let result = policy
        .retry(|| Ok::<_, &str>(-1))
        .sleep(|_dur| {
            flag.store(true, Ordering::Relaxed);
        })
        .cancel_on(&flag)
        .call();

    assert!(matches!(
        result,
        Err(RetryError::Cancelled {
            attempts: 1,
            last: Some(Ok(-1)),
            ..
        })
    ));
}

#[test]
fn never_cancel_is_zero_cost_default() {
    // Ensure NeverCancel always returns false
    assert!(!cancel::never().is_cancelled());
    assert!(!cancel::NeverCancel.is_cancelled());
}

#[test]
fn atomic_bool_canceler() {
    let flag = AtomicBool::new(false);
    let canceler: &AtomicBool = &flag;
    assert!(!canceler.is_cancelled());
    flag.store(true, Ordering::Relaxed);
    assert!(canceler.is_cancelled());
}

#[cfg(feature = "alloc")]
mod alloc_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn arc_atomic_bool_canceler() {
        let flag = Arc::new(AtomicBool::new(false));
        assert!(!flag.is_cancelled());
        flag.store(true, Ordering::Relaxed);
        assert!(flag.is_cancelled());
    }
}

// -- Async tests --

#[cfg(feature = "alloc")]
mod async_tests {
    use super::*;
    use core::future::Future;
    use core::pin::Pin;
    use core::task::Poll;
    use std::rc::Rc;

    /// Number of cancellation-future polls before cancellation is reported.
    const CANCEL_READY_AFTER_POLLS: u32 = 2;

    struct CancelAfterPollsFuture {
        poll_count: Rc<Cell<u32>>,
        ready_after: u32,
    }

    impl Future for CancelAfterPollsFuture {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut core::task::Context<'_>) -> Poll<Self::Output> {
            let next = self.poll_count.get().saturating_add(1);
            self.poll_count.set(next);
            if next >= self.ready_after {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }

    struct CompleteAfterSecondPoll {
        poll_count: Rc<Cell<u32>>,
        cancel_flag: std::sync::Arc<AtomicBool>,
        completed: Rc<Cell<bool>>,
    }

    impl Future for CompleteAfterSecondPoll {
        type Output = Result<(), &'static str>;

        fn poll(self: Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> Poll<Self::Output> {
            let next = self.poll_count.get().saturating_add(1);
            self.poll_count.set(next);
            if next == 1 {
                self.cancel_flag.store(true, Ordering::Relaxed);
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                self.completed.set(true);
                Poll::Ready(Err("op-finished"))
            }
        }
    }

    #[derive(Clone)]
    struct CancelViaFuture {
        poll_count: Rc<Cell<u32>>,
        ready_after: u32,
    }

    impl CancelViaFuture {
        fn new(ready_after: u32) -> Self {
            Self {
                poll_count: Rc::new(Cell::new(0)),
                ready_after,
            }
        }

        fn poll_count(&self) -> u32 {
            self.poll_count.get()
        }
    }

    impl Canceler for CancelViaFuture {
        type Cancel = CancelAfterPollsFuture;

        fn is_cancelled(&self) -> bool {
            false
        }

        fn cancel(&self) -> Self::Cancel {
            CancelAfterPollsFuture {
                poll_count: Rc::clone(&self.poll_count),
                ready_after: self.ready_after,
            }
        }
    }

    fn block_on<F: core::future::Future>(f: F) -> F::Output {
        // Minimal single-threaded executor for testing
        use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        use std::pin::pin;

        fn dummy_raw_waker() -> RawWaker {
            fn no_op(_: *const ()) {}
            fn clone(p: *const ()) -> RawWaker {
                RawWaker::new(p, &VTABLE)
            }
            const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
            RawWaker::new(core::ptr::null(), &VTABLE)
        }

        let waker = unsafe { Waker::from_raw(dummy_raw_waker()) };
        let mut cx = Context::from_waker(&waker);
        let mut f = pin!(f);
        loop {
            match f.as_mut().poll(&mut cx) {
                Poll::Ready(val) => return val,
                Poll::Pending => {} // spin
            }
        }
    }

    #[test]
    fn async_cancel_before_first_attempt() {
        let flag = AtomicBool::new(true);
        let mut policy = RetryPolicy::new().stop(stop::attempts(5));
        let result = block_on(
            policy
                .retry_async(|| async { Err::<(), _>("fail") })
                .sleep(|_dur: Duration| async {})
                .cancel_on(&flag),
        );

        match result {
            Err(RetryError::Cancelled { last, attempts, .. }) => {
                assert_eq!(attempts, 0);
                assert_eq!(last, None);
            }
            other => panic!("expected Cancelled, got {:?}", other),
        }
    }

    #[test]
    fn async_cancel_after_attempt_when_sleep_sets_flag() {
        let flag = AtomicBool::new(false);
        let exit_calls = Cell::new(0_u32);
        let exit_reason = Cell::new(None);
        let exit_attempt = Cell::new(0_u32);
        let exit_has_outcome = Cell::new(false);
        let mut policy = RetryPolicy::new()
            .stop(stop::attempts(5))
            .wait(wait::fixed(Duration::ZERO));

        let call_count = Cell::new(0u32);
        let result = block_on(
            policy
                .retry_async(|| {
                    call_count.set(call_count.get() + 1);
                    async { Err::<(), _>("async fail") }
                })
                .on_exit(|state| {
                    exit_calls.set(exit_calls.get().saturating_add(1));
                    exit_reason.set(Some(state.reason));
                    exit_attempt.set(state.attempt);
                    exit_has_outcome.set(state.outcome.is_some());
                })
                .sleep(|_dur: Duration| {
                    flag.store(true, Ordering::Relaxed);
                    async {}
                })
                .cancel_on(&flag),
        );

        match result {
            Err(RetryError::Cancelled { last, attempts, .. }) => {
                assert_eq!(attempts, 1);
                assert_eq!(last, Some(Err("async fail")));
            }
            other => panic!("expected Cancelled, got {:?}", other),
        }
        assert_eq!(exit_calls.get(), 1);
        assert_eq!(exit_reason.get(), Some(StopReason::Cancelled));
        assert_eq!(exit_attempt.get(), 1);
        assert!(exit_has_outcome.get());
    }

    #[test]
    fn async_cancellation_does_not_interrupt_running_operation() {
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let op_poll_count = Rc::new(Cell::new(0_u32));
        let op_completed = Rc::new(Cell::new(false));
        let flag_for_op = std::sync::Arc::clone(&flag);
        let poll_count_for_op = Rc::clone(&op_poll_count);
        let completed_for_op = Rc::clone(&op_completed);

        let mut policy = RetryPolicy::new()
            .stop(stop::attempts(5))
            .wait(wait::fixed(Duration::ZERO));

        let result = block_on(
            policy
                .retry_async(move || CompleteAfterSecondPoll {
                    poll_count: Rc::clone(&poll_count_for_op),
                    cancel_flag: std::sync::Arc::clone(&flag_for_op),
                    completed: Rc::clone(&completed_for_op),
                })
                .sleep(|_dur: Duration| async {})
                .cancel_on(std::sync::Arc::clone(&flag)),
        );

        assert_eq!(op_poll_count.get(), 2);
        assert!(op_completed.get());
        assert!(matches!(
            result,
            Err(RetryError::Cancelled {
                attempts: 1,
                last: Some(Err("op-finished")),
                ..
            })
        ));
    }

    #[test]
    fn async_stats_reports_cancelled() {
        let flag = AtomicBool::new(true);
        let mut policy = RetryPolicy::new().stop(stop::attempts(5));
        let (result, stats) = block_on(
            policy
                .retry_async(|| async { Err::<(), _>("fail") })
                .sleep(|_dur: Duration| async {})
                .cancel_on(&flag)
                .with_stats(),
        );

        assert!(matches!(result, Err(RetryError::Cancelled { .. })));
        assert_eq!(stats.stop_reason, StopReason::Cancelled);
        assert_eq!(stats.attempts, 0);
    }

    #[test]
    fn async_on_exit_fires_when_cancelled_before_first_attempt() {
        let flag = AtomicBool::new(true);
        let exit_calls = Cell::new(0_u32);
        let exit_attempt = Cell::new(99_u32);
        let exit_has_outcome = Cell::new(true);
        let mut policy = RetryPolicy::new().stop(stop::attempts(5));

        let result = block_on(
            policy
                .retry_async(|| async { Err::<(), _>("fail") })
                .on_exit(|state| {
                    exit_calls.set(exit_calls.get().saturating_add(1));
                    exit_attempt.set(state.attempt);
                    exit_has_outcome.set(state.outcome.is_some());
                })
                .sleep(|_dur: Duration| async {})
                .cancel_on(&flag),
        );

        assert!(matches!(
            result,
            Err(RetryError::Cancelled { attempts: 0, .. })
        ));
        assert_eq!(exit_calls.get(), 1);
        assert_eq!(exit_attempt.get(), 0);
        assert!(!exit_has_outcome.get());
    }

    #[test]
    fn async_on_exit_fires_when_cancelled_between_attempts() {
        let canceler = CancelOnCheck::new(3);
        let exit_calls = Cell::new(0_u32);
        let exit_attempt = Cell::new(0_u32);
        let exit_has_err_outcome = Cell::new(false);
        let call_count = Cell::new(0_u32);
        let mut policy = RetryPolicy::new()
            .stop(stop::attempts(5))
            .wait(wait::fixed(Duration::ZERO));

        let result = block_on(
            policy
                .retry_async(|| {
                    call_count.set(call_count.get().saturating_add(1));
                    async { Err::<(), _>("fail") }
                })
                .on_exit(|state| {
                    exit_calls.set(exit_calls.get().saturating_add(1));
                    exit_attempt.set(state.attempt);
                    exit_has_err_outcome.set(matches!(state.outcome, Some(Err("fail"))));
                })
                .sleep(|_dur: Duration| async {})
                .cancel_on(canceler),
        );

        assert!(matches!(
            result,
            Err(RetryError::Cancelled {
                attempts: 1,
                last: Some(Err("fail")),
                ..
            })
        ));
        assert_eq!(call_count.get(), 1);
        assert_eq!(exit_calls.get(), 1);
        assert_eq!(exit_attempt.get(), 1);
        assert!(exit_has_err_outcome.get());
    }

    #[test]
    fn async_cancelled_last_result_preserves_ok_value_in_polling_mode() {
        let flag = AtomicBool::new(false);
        let mut policy = RetryPolicy::new()
            .stop(stop::attempts(5))
            .wait(wait::fixed(Duration::ZERO))
            .when(tenacious::on::ok(|_value: &i32| true));

        let result = block_on(
            policy
                .retry_async(|| async { Ok::<_, &str>(-1) })
                .sleep(|_dur: Duration| {
                    flag.store(true, Ordering::Relaxed);
                    async {}
                })
                .cancel_on(&flag),
        );

        assert!(matches!(
            result,
            Err(RetryError::Cancelled {
                attempts: 1,
                last: Some(Ok(-1)),
                ..
            })
        ));
    }

    #[test]
    fn async_cancel_future_interrupts_sleep_when_poll_signal_stays_false() {
        let canceler = CancelViaFuture::new(CANCEL_READY_AFTER_POLLS);
        let canceler_for_assert = canceler.clone();
        let call_count = Cell::new(0_u32);
        let mut policy = RetryPolicy::new()
            .stop(stop::attempts(5))
            .wait(wait::fixed(Duration::ZERO));

        let result = block_on(
            policy
                .retry_async(|| {
                    call_count.set(call_count.get().saturating_add(1));
                    async { Err::<(), _>("future-cancel") }
                })
                .sleep(|_dur: Duration| core::future::pending())
                .cancel_on(canceler),
        );

        assert_eq!(call_count.get(), 1);
        assert!(matches!(
            result,
            Err(RetryError::Cancelled {
                attempts: 1,
                last: Some(Err("future-cancel")),
                ..
            })
        ));
        assert!(canceler_for_assert.poll_count() >= CANCEL_READY_AFTER_POLLS);
    }

    #[cfg(feature = "tokio-cancel")]
    #[test]
    fn async_tokio_cancellation_token_interrupts_sleep() {
        let token = tokio_util::sync::CancellationToken::new();
        let token_for_sleep = token.clone();
        let call_count = Cell::new(0_u32);
        let mut policy = RetryPolicy::new()
            .stop(stop::attempts(5))
            .wait(wait::fixed(Duration::ZERO));

        let result = block_on(
            policy
                .retry_async(|| {
                    call_count.set(call_count.get().saturating_add(1));
                    async { Err::<(), _>("tokio-cancel") }
                })
                .sleep(move |_dur: Duration| {
                    token_for_sleep.cancel();
                    core::future::pending()
                })
                .cancel_on(token),
        );

        assert_eq!(call_count.get(), 1);
        assert!(matches!(
            result,
            Err(RetryError::Cancelled {
                attempts: 1,
                last: Some(Err("tokio-cancel")),
                ..
            })
        ));
    }
}
