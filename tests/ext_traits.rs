//! Acceptance tests for extension-trait owned retry builders (Spec iteration 14).
#![cfg(feature = "std")]

use core::cell::Cell;
use core::time::Duration;
use std::cell::RefCell;
use std::rc::Rc;

use tenacious::{AsyncRetryExt, RetryError, RetryExt, RetryPolicy, StopReason, on, stop, wait};

const MAX_ATTEMPTS: u32 = 3;
const SUCCESS_VALUE: i32 = 42;
const ERROR_VALUE: &str = "fail";
const WAIT_DURATION: Duration = Duration::from_millis(1);

fn instant_sleep(_dur: Duration) {}

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
    let result = do_work.retry().stop(stop::attempts(1)).call();
    assert_eq!(result, Ok(SUCCESS_VALUE));
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
fn retry_ext_hooks_match_policy_hook_points() {
    let before_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let after_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let before_sleep_calls: RefCell<Vec<(u32, Duration)>> = RefCell::new(Vec::new());
    let exit_calls: RefCell<Vec<(u32, bool, StopReason)>> = RefCell::new(Vec::new());

    let _ = (|| Err::<i32, _>(ERROR_VALUE))
        .retry()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION))
        .before_attempt(|state| before_calls.borrow_mut().push(state.attempt))
        .after_attempt(|state: &tenacious::AttemptState<i32, &str>| {
            after_calls.borrow_mut().push(state.attempt);
        })
        .before_sleep(|state: &tenacious::AttemptState<i32, &str>| {
            before_sleep_calls
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
    assert_eq!(*after_calls.borrow(), vec![1, 2, 3]);
    assert_eq!(
        *before_sleep_calls.borrow(),
        vec![(1, WAIT_DURATION), (2, WAIT_DURATION)]
    );
    assert_eq!(
        *exit_calls.borrow(),
        vec![(MAX_ATTEMPTS, true, StopReason::StopCondition)]
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
            last: -1,
            attempts: 2,
            ..
        })
    ));
}

#[test]
fn retry_ext_predicate_rejected_returns_error_immediately() {
    let result = (|| Err::<i32, &str>(ERROR_VALUE))
        .retry()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::error(|err: &&str| *err == "retryable"))
        .sleep(instant_sleep)
        .call();

    assert!(matches!(
        result,
        Err(RetryError::PredicateRejected {
            error: ERROR_VALUE,
            attempts: 1,
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
    fn async_retry_ext_hooks_match_policy_hook_points() {
        let before_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
        let after_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
        let before_sleep_calls: RefCell<Vec<(u32, Duration)>> = RefCell::new(Vec::new());
        let exit_calls: RefCell<Vec<(u32, bool, StopReason)>> = RefCell::new(Vec::new());

        let future = (|| ready::<Result<i32, &str>>(Err(ERROR_VALUE)))
            .retry_async()
            .stop(stop::attempts(MAX_ATTEMPTS))
            .wait(wait::fixed(WAIT_DURATION))
            .before_attempt(|state| before_calls.borrow_mut().push(state.attempt))
            .after_attempt(|state: &tenacious::AttemptState<i32, &str>| {
                after_calls.borrow_mut().push(state.attempt);
            })
            .before_sleep(|state: &tenacious::AttemptState<i32, &str>| {
                before_sleep_calls
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
        assert_eq!(*after_calls.borrow(), vec![1, 2, 3]);
        assert_eq!(
            *before_sleep_calls.borrow(),
            vec![(1, WAIT_DURATION), (2, WAIT_DURATION)]
        );
        assert_eq!(
            *exit_calls.borrow(),
            vec![(MAX_ATTEMPTS, true, StopReason::StopCondition)]
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

    #[cfg(debug_assertions)]
    #[test]
    fn async_retry_ext_repoll_after_completion_panics_in_debug() {
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

    #[cfg(not(debug_assertions))]
    #[test]
    #[cfg(not(feature = "strict-futures"))]
    fn async_retry_ext_repoll_after_completion_is_pending_in_release() {
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

        let second_poll = Future::poll(Pin::as_mut(&mut retry), &mut cx);
        assert_eq!(second_poll, Poll::Pending);
    }

    #[cfg(not(debug_assertions))]
    #[test]
    #[cfg(feature = "strict-futures")]
    fn async_retry_ext_repoll_after_completion_panics_with_strict_feature() {
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
    let from_ext = (|| Ok::<i32, &str>(SUCCESS_VALUE))
        .retry()
        .stop(stop::attempts(1))
        .call();

    let from_policy = RetryPolicy::new()
        .stop(stop::attempts(1))
        .retry(|| Ok::<i32, &str>(SUCCESS_VALUE))
        .call();

    assert_eq!(from_ext, from_policy);
}
