//! Tests for the hook points on the asynchronous path.
//!
//! Mirrors `hooks.rs`: hook ordering, the `Exit` view, per-call hook scope and
//! cancellation behavior of `on_exit`.

mod common;

use common::RecordingAsyncClock as RecordingClock;
use common::block_on;
use core::cell::Cell;
use core::future::Future;
use core::task::{Context, Waker};
use core::time::Duration;
use relentless::{RetryError, RetryPolicy, predicate, stop, wait};
use std::cell::RefCell;
use std::rc::Rc;

const MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(10);
const SUCCESS_VALUE: i32 = 42;
const ERROR_VALUE: &str = "fail";

#[test]
fn async_hooks_fire_in_expected_places() {
    let before_attempt_calls = Rc::new(RefCell::new(Vec::new()));
    let after_attempt_calls = Rc::new(RefCell::new(Vec::new()));
    let exit_reason = Rc::new(Cell::new(None));
    let clock = RecordingClock::new();

    let before_attempt_ref = Rc::clone(&before_attempt_calls);
    let after_attempt_ref = Rc::clone(&after_attempt_calls);
    let exit_reason_ref = Rc::clone(&exit_reason);

    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .before_attempt(move |state| {
                before_attempt_ref.borrow_mut().push(state.attempt);
            })
            .after_attempt(
                move |state: &relentless::AttemptState<'_, Result<i32, &str>>| {
                    after_attempt_ref.borrow_mut().push(state.attempt);
                },
            )
            .on_exit(
                move |exit: &relentless::Exit<'_, i32, &str, Result<i32, &str>>| {
                    exit_reason_ref.set(Some(exit.stop_reason()));
                },
            )
            .clock(clock)
            .call(),
    );

    let before_attempt = before_attempt_calls.borrow();
    let after_attempt = after_attempt_calls.borrow();

    assert_eq!(*before_attempt, vec![1, 2, 3]);
    assert_eq!(*after_attempt, vec![1, 2, 3]);
    assert_eq!(exit_reason.get(), Some(relentless::StopReason::Exhausted));
}

/// GIVEN a retry future parked in an inter-attempt wait
/// WHEN it is dropped instead of polled to completion (`select!`, `timeout`,
///      an aborted task — the normal lifecycle of an async future)
/// THEN `on_exit` never runs, so a hook that releases a permit or decrements
///      an in-flight gauge leaks. SPEC 9.1 says so; this pins it, because the
///      `on_exit` rustdoc now promises it and nothing else would catch a
///      change.
#[test]
fn async_on_exit_does_not_run_when_the_retry_future_is_dropped() {
    /// A clock whose wait never resolves, so the retry future parks in the
    /// first inter-attempt wait and can be dropped there.
    #[derive(Clone, Default)]
    struct StalledClock;

    impl relentless::Clock for StalledClock {
        fn now(&self) -> Duration {
            Duration::ZERO
        }
    }

    impl relentless::AsyncClock for StalledClock {
        type Wait = core::future::Pending<()>;
        fn wait_async(&self, _dur: Duration) -> Self::Wait {
            core::future::pending()
        }
    }

    let exit_ran = Rc::new(Cell::new(false));
    let exit_ran_ref = Rc::clone(&exit_ran);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let mut cx = Context::from_waker(Waker::noop());
    let mut future = Box::pin(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .on_exit(
                move |_: &relentless::Exit<'_, i32, &str, Result<i32, &str>>| {
                    exit_ran_ref.set(true);
                },
            )
            .clock(StalledClock)
            .call(),
    );

    assert!(
        future.as_mut().poll(&mut cx).is_pending(),
        "the first attempt must fail and park in the wait"
    );
    drop(future);

    assert!(!exit_ran.get(), "on_exit must not run on drop-cancellation");
}

#[test]
fn async_on_exit_reports_success_reason() {
    let exit_reason = Rc::new(Cell::new(None));
    let exit_reason_ref = Rc::clone(&exit_reason);
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .on_exit(
                move |exit: &relentless::Exit<'_, i32, &str, Result<i32, &str>>| {
                    exit_reason_ref.set(Some(exit.stop_reason()));
                },
            )
            .clock(RecordingClock::new())
            .call(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(exit_reason.get(), Some(relentless::StopReason::Returned));
}

#[test]
fn async_on_exit_reports_non_retryable_error_reason() {
    let exit_reason = Rc::new(Cell::new(None));
    let exit_reason_ref = Rc::clone(&exit_reason);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::error(|err: &&str| *err == "retryable"));

    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>("fatal") })
            .on_exit(
                move |exit: &relentless::Exit<'_, i32, &str, Result<i32, &str>>| {
                    exit_reason_ref.set(Some(exit.stop_reason()));
                },
            )
            .clock(RecordingClock::new())
            .call(),
    );

    assert!(matches!(result, Err(RetryError::Aborted { .. })));
    assert_eq!(exit_reason.get(), Some(relentless::StopReason::Aborted));
}

#[test]
fn async_hooks_are_per_call_and_do_not_persist() {
    let exit_calls = Rc::new(Cell::new(0_u32));
    let exit_calls_ref = Rc::clone(&exit_calls);
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let _ = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .on_exit(
                move |_e: &relentless::Exit<'_, i32, &str, Result<i32, &str>>| {
                    exit_calls_ref.set(exit_calls_ref.get().saturating_add(1));
                },
            )
            .clock(RecordingClock::new())
            .call(),
    );
    assert_eq!(exit_calls.get(), 1);

    let _ = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .clock(RecordingClock::new())
            .call(),
    );
    assert_eq!(exit_calls.get(), 1);
}
