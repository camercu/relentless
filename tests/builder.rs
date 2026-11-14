//! Acceptance tests for the builder: `retry(..)` at top level, `retry_async(..)`
//! in `async_tests`.
//!
//! Each test drives one builder surface end to end under a virtual clock:
//! the default classifier path, `.decide`/`.when`/`.until`, self-classifying
//! outcomes, `.call_with_stats()` and `.timeout`.
mod common;

use common::ARBITRARY_ATTEMPTS;

use core::cell::{Cell, RefCell};
use core::time::Duration;

use relentless::clock::VirtualClock;
use relentless::{Decision, Outcome, RetryError, StopReason, Verdict, retry, stop, wait};

/// A non-`Result` poll outcome that classifies itself, so the default path
/// drives it with no `.decide` at the call site.
#[derive(Debug, PartialEq)]
enum Poll {
    Pending,
    Ready(i32),
    Failed(&'static str),
}

impl Outcome for Poll {
    type Return = i32;
    type Abort = &'static str;

    fn classify(self) -> Verdict<i32, &'static str, Poll> {
        match self {
            Poll::Ready(value) => Verdict::Return(value),
            Poll::Failed(error) => Verdict::Abort(error),
            Poll::Pending => Verdict::Retry(self),
        }
    }
}

#[test]
fn retries_on_err_then_returns_the_ok_value() {
    let counter = Cell::new(0);
    let result = retry(|_| {
        let n = counter.get() + 1;
        counter.set(n);
        if n >= 2 {
            Ok::<i32, &str>(7)
        } else {
            Err("not yet")
        }
    })
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(7));
    assert_eq!(counter.get(), 2, "should stop retrying once Ok is returned");
}

#[test]
fn exhausts_with_the_final_outcome_when_stop_fires() {
    let counter = Cell::new(0);
    let result = retry(|_| {
        counter.set(counter.get() + 1);
        Err::<i32, &str>("boom")
    })
    .stop(stop::attempts(3))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Err(RetryError::Exhausted { last: Err("boom") }));
    assert_eq!(counter.get(), 3, "should attempt exactly the stop budget");
}

#[test]
fn decide_can_abort_on_a_projected_payload() {
    // Fuzzing shape: keep retrying transient errors, but abort fatally on a
    // specific one — the abort arm pins the payload type inline.
    let result = retry(|_| Err::<i32, &str>("fatal"))
        .decide(|o| match o {
            Ok(v) => Verdict::Return(v),
            Err("fatal") => Verdict::Abort("boom"),
            Err(_) => Verdict::Retry(o),
        })
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Err(RetryError::Aborted { last: "boom" }));
}

#[test]
fn decide_returns_the_sought_error_through_ok() {
    // The inverted-polling wart, gone: probe until a failure appears and
    // deliver that failure as the success value via `Ok` — no `RetryError`.
    let counter = Cell::new(0);
    let result = retry(|_| {
        let n = counter.get() + 1;
        counter.set(n);
        if n >= 3 { Err("crash") } else { Ok(()) }
    })
    .decide(|o| match o {
        Err(e) => Decision::Return(e),
        Ok(()) => Decision::Retry(o),
    })
    .stop(stop::attempts(10))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok("crash"));
    assert_eq!(counter.get(), 3);
}

#[test]
fn until_polls_a_result_until_ready() {
    #[derive(Debug, PartialEq)]
    enum Status {
        Pending,
        Done,
    }

    let counter = Cell::new(0);
    let result = retry(|_| {
        let n = counter.get() + 1;
        counter.set(n);
        Ok::<Status, &str>(if n >= 2 {
            Status::Done
        } else {
            Status::Pending
        })
    })
    .until(|s: &Status| *s == Status::Done)
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(Status::Done));
    assert_eq!(counter.get(), 2);
}

#[test]
fn when_aborts_on_a_rejected_error() {
    // Retry only "transient"; any other error is non-retryable and aborts
    // with the bare error payload.
    let result = retry(|_| Err::<i32, &str>("fatal"))
        .when(|e: &&str| *e == "transient")
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Err(RetryError::Aborted { last: "fatal" }));
}

#[test]
fn owned_outcome_type_returns_via_the_default_path() {
    let counter = Cell::new(0);
    let result = retry(|_| {
        let n = counter.get() + 1;
        counter.set(n);
        if n >= 3 {
            Poll::Ready(9)
        } else {
            Poll::Pending
        }
    })
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(9));
    assert_eq!(counter.get(), 3);
}

#[test]
fn owned_outcome_type_aborts_via_the_default_path() {
    let result = retry(|_| Poll::Failed("io"))
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Err(RetryError::Aborted { last: "io" }));
}

#[test]
fn retry_starts_from_a_closure_ignoring_state() {
    let counter = Cell::new(0);
    let result = retry(|_| {
        let n = counter.get() + 1;
        counter.set(n);
        if n >= 2 { Ok::<i32, &str>(5) } else { Err("x") }
    })
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(5));
    assert_eq!(counter.get(), 2);
}

#[test]
fn call_with_stats_reports_attempts_wait_elapsed_and_reason() {
    let (result, stats) = retry(|_| Err::<i32, &str>("boom"))
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::from_millis(5)))
        .clock(VirtualClock::new())
        .call_with_stats();

    assert_eq!(result, Err(RetryError::Exhausted { last: Err("boom") }));
    assert_eq!(stats.attempts, 3);
    // Two inter-attempt waits of 5ms; the terminal attempt does not wait.
    assert_eq!(stats.total_wait, Duration::from_millis(10));
    assert_eq!(stats.total_elapsed, Duration::from_millis(10));
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
}

/// 6.1.1
#[test]
fn free_function_retry_uses_default_policy() {
    use relentless::retry;

    let call_count = Cell::new(0_u32);
    let result = retry(|_| {
        call_count.set(call_count.get().saturating_add(1));
        Err::<i32, &str>("always fails")
    })
    .clock(VirtualClock::new())
    .call();

    // Default is attempts(3), so 3 calls should be made.
    assert_eq!(call_count.get(), 3);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// 6.1.2
#[test]
fn free_function_retry_provides_retry_state_to_op() {
    use relentless::{RetryState, retry};

    let states_seen: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let _ = retry(|state: RetryState| {
        states_seen.borrow_mut().push(state.attempt);
        Err::<i32, &str>("fail")
    })
    .stop(stop::attempts(3))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(*states_seen.borrow(), vec![1, 2, 3]);
}

mod async_tests {
    //! Acceptance tests for the async builder (`retry_async(..)`).
    //!
    //! Mirrors the sync builder scenarios above, driven under a virtual
    //! clock and a no-op waker so no executor is needed.

    use super::common;

    use common::{ARBITRARY_ATTEMPTS, ARBITRARY_VALUE, ReadyClock, block_on};
    use core::cell::Cell;
    use core::time::Duration;
    use std::rc::Rc;

    use relentless::clock::VirtualClock;
    use relentless::{Decision, RetryError, StopReason, Verdict, retry_async, stop, wait};

    #[test]
    fn retry_async_starts_from_a_closure_ignoring_state() {
        let counter = Cell::new(0);
        let clock = VirtualClock::new();
        let result = block_on(
            retry_async(|_| {
                let n = counter.get() + 1;
                counter.set(n);
                async move { if n >= 2 { Ok::<i32, &str>(5) } else { Err("x") } }
            })
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(&clock)
            .call(),
        );

        assert_eq!(result, Ok(5));
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn async_default_path_retries_then_returns_ok() {
        let counter = Cell::new(0);
        let clock = VirtualClock::new();
        let result = block_on(
            retry_async(|_| {
                let n = counter.get() + 1;
                counter.set(n);
                async move { if n >= 2 { Ok::<i32, &str>(7) } else { Err("x") } }
            })
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(&clock)
            .call(),
        );

        assert_eq!(result, Ok(7));
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn async_decide_infers_through_the_operation_output() {
        // Inline classifier with no annotations: the parameter is typed through
        // `Operation::Output`.
        let clock = VirtualClock::new();
        let result = block_on(
            retry_async(|_| async { Err::<i32, &str>("fatal") })
                .decide(|o| match o {
                    Ok(v) => Verdict::Return(v),
                    Err("fatal") => Verdict::Abort("boom"),
                    Err(_) => Verdict::Retry(o),
                })
                .stop(stop::attempts(ARBITRARY_ATTEMPTS))
                .wait(wait::fixed(Duration::ZERO))
                .clock(&clock)
                .call(),
        );

        assert_eq!(result, Err(RetryError::Aborted { last: "boom" }));
    }

    #[test]
    fn async_decide_returns_sought_error_through_ok() {
        let counter = Cell::new(0);
        let clock = VirtualClock::new();
        let result = block_on(
            retry_async(|_| {
                let n = counter.get() + 1;
                counter.set(n);
                async move { if n >= 3 { Err("crash") } else { Ok(()) } }
            })
            .decide(|o| match o {
                Err(e) => Decision::Return(e),
                Ok(()) => Decision::Retry(o),
            })
            .stop(stop::attempts(10))
            .wait(wait::fixed(Duration::ZERO))
            .clock(&clock)
            .call(),
        );

        assert_eq!(result, Ok("crash"));
        assert_eq!(counter.get(), 3);
    }

    #[test]
    fn async_until_polls_a_result() {
        let counter = Cell::new(0);
        let clock = VirtualClock::new();
        let result = block_on(
            retry_async(|_| {
                let n = counter.get() + 1;
                counter.set(n);
                async move { Ok::<i32, &str>(n) }
            })
            .until(|v: &i32| *v >= 3)
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(&clock)
            .call(),
        );

        assert_eq!(result, Ok(3));
        assert_eq!(counter.get(), 3);
    }

    #[test]
    fn async_call_with_stats_matches_the_sync_engine() {
        let clock = VirtualClock::new();
        let (result, stats) = block_on(
            retry_async(|_| async { Err::<i32, &str>("boom") })
                .stop(stop::attempts(3))
                .wait(wait::fixed(Duration::from_millis(5)))
                .clock(&clock)
                .call_with_stats(),
        );

        assert_eq!(result, Err(RetryError::Exhausted { last: Err("boom") }));
        assert_eq!(stats.attempts, 3);
        assert_eq!(stats.total_wait, Duration::from_millis(10));
        assert_eq!(stats.stop_reason, StopReason::Exhausted);
    }

    /// Returns `Pending` on its first poll (waking the task) and `Ready` after.
    struct YieldOnce {
        yielded: bool,
    }

    impl core::future::Future for YieldOnce {
        type Output = ();

        fn poll(
            mut self: core::pin::Pin<&mut Self>,
            cx: &mut core::task::Context<'_>,
        ) -> core::task::Poll<()> {
            if self.yielded {
                return core::task::Poll::Ready(());
            }
            self.yielded = true;
            cx.waker().wake_by_ref();
            core::task::Poll::Pending
        }
    }

    /// GIVEN an operation whose future is pending before it resolves
    /// WHEN the retry future is driven to completion
    /// THEN the same future is polled again: the operation runs once and its
    /// value is returned after one attempt.
    #[test]
    fn a_pending_operation_future_is_repolled_not_restarted() {
        let calls = Cell::new(0);
        let clock = VirtualClock::new();
        let (result, stats) = block_on(
            retry_async(|_| {
                calls.set(calls.get() + 1);
                // Only the first call's future yields, so a driver that restarted
                // the operation on `Pending` would finish with two calls, not hang.
                let first_call = calls.get() == 1;
                async move {
                    if first_call {
                        YieldOnce { yielded: false }.await;
                    }
                    Ok::<i32, &str>(ARBITRARY_VALUE)
                }
            })
            .clock(&clock)
            .call_with_stats(),
        );

        assert_eq!(
            (result, stats.attempts, calls.get()),
            (Ok(ARBITRARY_VALUE), 1, 1)
        );
    }

    /// §10.1 — the retry future never holds an outcome across a wait, so a `!Send`
    /// outcome still yields a `Send` future when the operation's future is `Send`.
    #[test]
    fn a_non_send_outcome_still_gives_a_send_retry_future() {
        fn assert_send<T: Send>(_: &T) {}

        let future = retry_async(|_| async { Ok::<Rc<i32>, &str>(Rc::new(ARBITRARY_VALUE)) })
            .clock(ReadyClock)
            .call();
        assert_send(&future);
        assert_eq!(block_on(future).as_deref(), Ok(&ARBITRARY_VALUE));
    }
}
