//! Acceptance tests for extension-trait owned retry builders (Spec iteration 14).
#![cfg(feature = "std")]

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::cell::RefCell;
use std::rc::Rc;

use tenacious::{
    AsyncRetryExt, RetryError, RetryExt, RetryPolicy, Stop, StopReason, Wait, on, stop, wait,
};

const MAX_ATTEMPTS: u32 = 3;
const SUCCESS_VALUE: i32 = 42;
const ERROR_VALUE: &str = "fail";
const WAIT_DURATION: Duration = Duration::from_millis(1);
const DEFAULT_INITIAL_WAIT: Duration = Duration::from_millis(100);
const DEFAULT_SECOND_WAIT: Duration = Duration::from_millis(200);
const DEFAULT_WAIT_SEQUENCE: [Duration; 2] = [DEFAULT_INITIAL_WAIT, DEFAULT_SECOND_WAIT];
const STATEFUL_STOP_THRESHOLD: u32 = 2;
const DIRTY_STOP_COUNT: u32 = 1;
const DIRTY_WAIT_COUNT: u32 = 1;
const RESET_WAIT_DURATION: Duration = Duration::from_millis(1);

fn instant_sleep(_dur: Duration) {}

struct StatefulStop {
    consultations: u32,
    threshold: u32,
}

impl Stop for StatefulStop {
    fn should_stop(&mut self, _state: &tenacious::RetryState) -> bool {
        self.consultations = self.consultations.saturating_add(1);
        self.consultations >= self.threshold
    }

    fn reset(&mut self) {
        self.consultations = 0;
    }
}

struct StatefulWait {
    calls: u32,
}

impl Wait for StatefulWait {
    fn next_wait(&mut self, _state: &tenacious::RetryState) -> Duration {
        self.calls = self.calls.saturating_add(1);
        Duration::from_millis(u64::from(self.calls))
    }

    fn reset(&mut self) {
        self.calls = 0;
    }
}

#[test]
fn retry_ext_closure_form_retries_until_success() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);

    let result: Result<i32, RetryError<&str, i32>> = (move || {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        if attempts_ref.get() < MAX_ATTEMPTS {
            Err(ERROR_VALUE)
        } else {
            Ok(SUCCESS_VALUE)
        }
    })
    .retry()
    .stop(stop::attempts(MAX_ATTEMPTS))
    .wait(wait::fixed(WAIT_DURATION))
    .sleep(|_dur| {})
    .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
}

fn do_work() -> Result<i32, &'static str> {
    Ok(SUCCESS_VALUE)
}

#[test]
fn retry_ext_function_pointer_form_works() {
    let result = do_work.retry().call();
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn retry_ext_uses_default_policy_when_not_overridden() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);
    let sleeps = Rc::new(RefCell::new(Vec::new()));
    let sleeps_ref = Rc::clone(&sleeps);

    let result = (move || {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        Err::<i32, &str>(ERROR_VALUE)
    })
    .retry()
    .sleep(move |dur| sleeps_ref.borrow_mut().push(dur))
    .call();

    assert!(matches!(
        result,
        Err(RetryError::Exhausted {
            attempts: MAX_ATTEMPTS,
            ..
        })
    ));
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
    assert_eq!(*sleeps.borrow(), DEFAULT_WAIT_SEQUENCE);
}

#[test]
fn retry_ext_with_stats_reports_attempts() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);

    let (result, stats): (Result<i32, RetryError<&str, i32>>, tenacious::RetryStats) =
        (move || {
            attempts_ref.set(attempts_ref.get().saturating_add(1));
            Err(ERROR_VALUE)
        })
        .retry()
        .stop(stop::attempts(2))
        .sleep(|_dur| {})
        .with_stats()
        .call();

    assert!(matches!(
        result,
        Err(RetryError::Exhausted { attempts: 2, .. })
    ));
    assert_eq!(stats.attempts, 2);
    assert_eq!(attempts.get(), 2);
}

#[test]
fn retry_ext_resets_stateful_stop_and_wait_before_execution() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);
    let sleeps = Rc::new(RefCell::new(Vec::new()));
    let sleeps_ref = Rc::clone(&sleeps);

    let result = (move || {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        Err::<i32, &str>(ERROR_VALUE)
    })
    .retry()
    .stop(StatefulStop {
        consultations: DIRTY_STOP_COUNT,
        threshold: STATEFUL_STOP_THRESHOLD,
    })
    .wait(StatefulWait {
        calls: DIRTY_WAIT_COUNT,
    })
    .sleep(move |dur| sleeps_ref.borrow_mut().push(dur))
    .call();

    assert!(matches!(
        result,
        Err(RetryError::Exhausted {
            attempts: STATEFUL_STOP_THRESHOLD,
            ..
        })
    ));
    assert_eq!(attempts.get(), STATEFUL_STOP_THRESHOLD);
    assert_eq!(*sleeps.borrow(), vec![RESET_WAIT_DURATION]);
}

#[test]
fn retry_ext_hooks_match_policy_hook_points() {
    let before_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let after_calls: RefCell<Vec<(u32, Option<Duration>)>> = RefCell::new(Vec::new());
    let exit_calls: RefCell<Vec<(u32, bool, StopReason)>> = RefCell::new(Vec::new());

    let _ = (|| Err::<i32, _>(ERROR_VALUE))
        .retry()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION))
        .before_attempt(|state| before_calls.borrow_mut().push(state.attempt))
        .after_attempt(|state: &tenacious::AttemptState<i32, &str>| {
            after_calls
                .borrow_mut()
                .push((state.attempt, state.next_delay));
        })
        .on_exit(|state: &tenacious::ExitState<i32, &str>| {
            exit_calls.borrow_mut().push((
                state.attempt,
                state
                    .outcome
                    .expect("outcome should be present when stop triggers")
                    .is_err(),
                state.reason,
            ));
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(*before_calls.borrow(), vec![1, 2, 3]);
    assert_eq!(
        *after_calls.borrow(),
        vec![
            (1, Some(WAIT_DURATION)),
            (2, Some(WAIT_DURATION)),
            (3, None),
        ]
    );
    assert_eq!(
        *exit_calls.borrow(),
        vec![(MAX_ATTEMPTS, true, StopReason::StopStrategyTriggered)]
    );
}

#[test]
fn retry_ext_condition_not_met_for_ok_exhaustion() {
    let result = (|| Ok::<i32, &str>(-1))
        .retry()
        .stop(stop::attempts(2))
        .when(on::ok(|_value: &i32| true))
        .sleep(instant_sleep)
        .call();

    assert!(matches!(
        result,
        Err(RetryError::ConditionNotMet {
            last: Ok(-1),
            attempts: 2,
            ..
        })
    ));
}

#[test]
fn retry_ext_non_retryable_error_returns_immediately() {
    let result = (|| Err::<i32, &str>(ERROR_VALUE))
        .retry()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::error(|err: &&str| *err == "retryable"))
        .sleep(instant_sleep)
        .call();

    assert!(matches!(
        result,
        Err(RetryError::NonRetryableError {
            last: Err(ERROR_VALUE),
            attempts: 1,
            ..
        })
    ));
}

#[test]
fn retry_ext_cancel_before_first_attempt_returns_cancelled() {
    let flag = AtomicBool::new(true);
    let result = (|| Err::<i32, &str>(ERROR_VALUE))
        .retry()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .sleep(instant_sleep)
        .cancel_on(&flag)
        .call();

    assert!(matches!(
        result,
        Err(RetryError::Cancelled {
            attempts: 0,
            last: None,
            ..
        })
    ));
}

#[test]
fn retry_ext_cancel_after_attempt_preserves_last_result() {
    let flag = AtomicBool::new(false);
    let calls = Cell::new(0_u32);

    let result = (|| {
        calls.set(calls.get().saturating_add(1));
        Err::<i32, &str>(ERROR_VALUE)
    })
    .retry()
    .stop(stop::attempts(MAX_ATTEMPTS))
    .wait(wait::fixed(Duration::ZERO))
    .sleep(|_dur| {
        flag.store(true, Ordering::Relaxed);
    })
    .cancel_on(&flag)
    .call();

    assert_eq!(calls.get(), 1);
    assert!(matches!(
        result,
        Err(RetryError::Cancelled {
            attempts: 1,
            last: Some(Err(ERROR_VALUE)),
            ..
        })
    ));
}

#[cfg(feature = "alloc")]
mod async_tests {
    use core::future::{Future, ready};
    use core::pin::Pin;
    use core::task::{Context, Poll, Waker};
    use std::sync::Arc;

    use super::*;

    /// Number of cancellation-future polls before cancellation is reported.
    const CANCEL_READY_AFTER_POLLS: u32 = 2;

    struct CancelAfterPollsFuture {
        poll_count: Rc<Cell<u32>>,
        ready_after: u32,
    }

    impl Future for CancelAfterPollsFuture {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            let next = self.poll_count.get().saturating_add(1);
            self.poll_count.set(next);
            if next >= self.ready_after {
                Poll::Ready(())
            } else {
                Poll::Pending
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

    impl tenacious::Canceler for CancelViaFuture {
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

    fn noop_waker() -> Waker {
        struct NoopWake;
        impl std::task::Wake for NoopWake {
            fn wake(self: Arc<Self>) {}
        }
        Waker::from(Arc::new(NoopWake))
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        loop {
            match Future::poll(Pin::as_mut(&mut future), &mut cx) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn async_retry_ext_retries_until_success() {
        let attempts = Rc::new(Cell::new(0_u32));
        let attempts_ref = Rc::clone(&attempts);

        let future = (move || {
            attempts_ref.set(attempts_ref.get().saturating_add(1));
            if attempts_ref.get() < MAX_ATTEMPTS {
                ready(Err(ERROR_VALUE))
            } else {
                ready(Ok(SUCCESS_VALUE))
            }
        })
        .retry_async()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::from_millis(1)))
        .sleep(|_dur| ready(()));

        let result: Result<i32, RetryError<&str, i32>> = block_on(future);
        assert_eq!(result, Ok(SUCCESS_VALUE));
        assert_eq!(attempts.get(), MAX_ATTEMPTS);
    }

    #[test]
    fn async_retry_ext_uses_default_policy_when_not_overridden() {
        let attempts = Rc::new(Cell::new(0_u32));
        let attempts_ref = Rc::clone(&attempts);
        let sleeps = Rc::new(RefCell::new(Vec::new()));
        let sleeps_ref = Rc::clone(&sleeps);

        let result = block_on(
            (move || {
                attempts_ref.set(attempts_ref.get().saturating_add(1));
                ready::<Result<i32, &str>>(Err(ERROR_VALUE))
            })
            .retry_async()
            .sleep(move |dur| {
                sleeps_ref.borrow_mut().push(dur);
                ready(())
            }),
        );

        assert!(matches!(
            result,
            Err(RetryError::Exhausted {
                attempts: MAX_ATTEMPTS,
                ..
            })
        ));
        assert_eq!(attempts.get(), MAX_ATTEMPTS);
        assert_eq!(*sleeps.borrow(), DEFAULT_WAIT_SEQUENCE);
    }

    #[test]
    fn async_retry_ext_hooks_match_policy_hook_points() {
        let before_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
        let after_calls: RefCell<Vec<(u32, Option<Duration>)>> = RefCell::new(Vec::new());
        let exit_calls: RefCell<Vec<(u32, bool, StopReason)>> = RefCell::new(Vec::new());

        let future = (|| ready::<Result<i32, &str>>(Err(ERROR_VALUE)))
            .retry_async()
            .stop(stop::attempts(MAX_ATTEMPTS))
            .wait(wait::fixed(WAIT_DURATION))
            .before_attempt(|state| before_calls.borrow_mut().push(state.attempt))
            .after_attempt(|state: &tenacious::AttemptState<i32, &str>| {
                after_calls
                    .borrow_mut()
                    .push((state.attempt, state.next_delay));
            })
            .on_exit(|state: &tenacious::ExitState<i32, &str>| {
                exit_calls.borrow_mut().push((
                    state.attempt,
                    state
                        .outcome
                        .expect("outcome should be present when stop triggers")
                        .is_err(),
                    state.reason,
                ));
            })
            .sleep(|_dur| ready(()));

        let _ = block_on(future);

        assert_eq!(*before_calls.borrow(), vec![1, 2, 3]);
        assert_eq!(
            *after_calls.borrow(),
            vec![
                (1, Some(WAIT_DURATION)),
                (2, Some(WAIT_DURATION)),
                (3, None),
            ]
        );
        assert_eq!(
            *exit_calls.borrow(),
            vec![(MAX_ATTEMPTS, true, StopReason::StopStrategyTriggered)]
        );
    }

    #[test]
    fn async_retry_ext_with_stats_reports_attempts() {
        let future = (|| ready::<Result<i32, &str>>(Err(ERROR_VALUE)))
            .retry_async()
            .stop(stop::attempts(2))
            .when(on::any_error())
            .sleep(|_dur| ready(()))
            .with_stats();

        let (result, stats) = block_on(future);
        assert!(matches!(
            result,
            Err(RetryError::Exhausted { attempts: 2, .. })
        ));
        assert_eq!(stats.attempts, 2);
    }

    #[test]
    fn async_retry_ext_resets_stateful_stop_and_wait_before_execution() {
        let attempts = Rc::new(Cell::new(0_u32));
        let attempts_ref = Rc::clone(&attempts);
        let sleeps = Rc::new(RefCell::new(Vec::new()));
        let sleeps_ref = Rc::clone(&sleeps);

        let result = block_on(
            (move || {
                attempts_ref.set(attempts_ref.get().saturating_add(1));
                ready::<Result<i32, &str>>(Err(ERROR_VALUE))
            })
            .retry_async()
            .stop(StatefulStop {
                consultations: DIRTY_STOP_COUNT,
                threshold: STATEFUL_STOP_THRESHOLD,
            })
            .wait(StatefulWait {
                calls: DIRTY_WAIT_COUNT,
            })
            .sleep(move |dur| {
                sleeps_ref.borrow_mut().push(dur);
                ready(())
            }),
        );

        assert!(matches!(
            result,
            Err(RetryError::Exhausted {
                attempts: STATEFUL_STOP_THRESHOLD,
                ..
            })
        ));
        assert_eq!(attempts.get(), STATEFUL_STOP_THRESHOLD);
        assert_eq!(*sleeps.borrow(), vec![RESET_WAIT_DURATION]);
    }

    #[test]
    fn async_retry_ext_cancel_before_first_attempt_returns_cancelled() {
        let flag = AtomicBool::new(true);

        let result = block_on(
            (|| ready::<Result<i32, &str>>(Err(ERROR_VALUE)))
                .retry_async()
                .stop(stop::attempts(MAX_ATTEMPTS))
                .sleep(|_dur| ready(()))
                .cancel_on(&flag),
        );

        assert!(matches!(
            result,
            Err(RetryError::Cancelled {
                attempts: 0,
                last: None,
                ..
            })
        ));
    }

    #[test]
    fn async_retry_ext_cancel_after_attempt_preserves_last_result() {
        let flag = AtomicBool::new(false);
        let calls = Cell::new(0_u32);

        let result = block_on(
            (|| {
                calls.set(calls.get().saturating_add(1));
                ready::<Result<i32, &str>>(Err(ERROR_VALUE))
            })
            .retry_async()
            .stop(stop::attempts(MAX_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .sleep(|_dur| {
                flag.store(true, Ordering::Relaxed);
                ready(())
            })
            .cancel_on(&flag),
        );

        assert_eq!(calls.get(), 1);
        assert!(matches!(
            result,
            Err(RetryError::Cancelled {
                attempts: 1,
                last: Some(Err(ERROR_VALUE)),
                ..
            })
        ));
    }

    #[test]
    fn async_retry_ext_cancel_future_interrupts_sleep_when_poll_signal_stays_false() {
        let canceler = CancelViaFuture::new(CANCEL_READY_AFTER_POLLS);
        let canceler_for_assert = canceler.clone();
        let calls = Cell::new(0_u32);

        let result = block_on(
            (|| {
                calls.set(calls.get().saturating_add(1));
                ready::<Result<i32, &str>>(Err("future-cancel"))
            })
            .retry_async()
            .stop(stop::attempts(MAX_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .sleep(|_dur| core::future::pending())
            .cancel_on(canceler),
        );

        assert_eq!(calls.get(), 1);
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
    fn async_retry_ext_tokio_cancellation_token_interrupts_sleep() {
        let token = tokio_util::sync::CancellationToken::new();
        let token_for_sleep = token.clone();
        let calls = Cell::new(0_u32);

        let result = block_on(
            (|| {
                calls.set(calls.get().saturating_add(1));
                ready::<Result<i32, &str>>(Err("tokio-cancel"))
            })
            .retry_async()
            .stop(stop::attempts(MAX_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .sleep(move |_dur| {
                token_for_sleep.cancel();
                core::future::pending()
            })
            .cancel_on(token),
        );

        assert_eq!(calls.get(), 1);
        assert!(matches!(
            result,
            Err(RetryError::Cancelled {
                attempts: 1,
                last: Some(Err("tokio-cancel")),
                ..
            })
        ));
    }

    #[test]
    fn async_retry_ext_repoll_after_completion_panics() {
        let mut retry = Box::pin(
            (|| ready(Ok::<i32, &str>(SUCCESS_VALUE)))
                .retry_async()
                .stop(stop::attempts(1))
                .sleep(|_dur| ready(())),
        );
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let first_poll = Future::poll(Pin::as_mut(&mut retry), &mut cx);
        assert_eq!(first_poll, Poll::Ready(Ok(SUCCESS_VALUE)));

        let second_poll = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = Future::poll(Pin::as_mut(&mut retry), &mut cx);
        }));
        assert!(second_poll.is_err());
    }
}

#[test]
fn policy_and_extension_forms_are_equivalent_for_basic_case() {
    let from_ext = (|| Err::<i32, &str>(ERROR_VALUE))
        .retry()
        .sleep(instant_sleep)
        .call();

    let from_policy = RetryPolicy::default()
        .retry(|| Err::<i32, &str>(ERROR_VALUE))
        .sleep(instant_sleep)
        .call();

    assert!(matches!(
        from_ext,
        Err(RetryError::Exhausted {
            last: Err(ERROR_VALUE),
            attempts: MAX_ATTEMPTS,
            ..
        })
    ));
    assert!(matches!(
        from_policy,
        Err(RetryError::Exhausted {
            last: Err(ERROR_VALUE),
            attempts: MAX_ATTEMPTS,
            ..
        })
    ));
}
