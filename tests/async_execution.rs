//! Acceptance tests for Async Execution (Spec items 6.1–6.8).
#![cfg(all(feature = "alloc", feature = "std"))]
//!
//! These tests verify:
//! - `RetryPolicy::retry_async(op)` configures async retry (6.1)
//! - `AsyncRetry::sleep(...)` sets required sleeper and enables execution (6.2)
//! - `AsyncRetry` is directly awaitable (6.3)
//! - Async execution loop behavior matches sync semantics (6.4)
//! - Async execution is executor-agnostic and deterministic in-process (6.5)
//! - `tokio_sleep` re-export is available behind feature gate (6.6)
//! - `embassy_sleep` is available behind feature gate (6.7)
//! - Async hook callbacks are synchronous and fire at the right points (6.8)
//!
//! This file also closes deferred retry-predicate execution behavior checks:
//! - Predicate evaluated before stop (4.9)
//! - Default predicate behaves like `on::any_error()` (4.10)

use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tenacious::{RetryError, RetryPolicy};
use tenacious::{on, sleep::Sleeper, stop, wait};

// ---------------------------------------------------------------------------
// Test constants
// ---------------------------------------------------------------------------

/// Maximum attempts for most async retry tests.
const MAX_ATTEMPTS: u32 = 3;

/// Wait duration used in async sleep verification tests.
const WAIT_DURATION: Duration = Duration::from_millis(10);

/// Sleep duration used to simulate operation runtime.
const OPERATION_RUNTIME: Duration = Duration::from_millis(5);

/// Tight elapsed deadline used to verify operation runtime is counted.
const ELAPSED_DEADLINE: Duration = Duration::from_millis(1);

/// Deadline for conservative before-elapsed stop tests.
const BEFORE_ELAPSED_DEADLINE: Duration = Duration::from_millis(30);

/// Wait used in conservative before-elapsed stop tests.
const BEFORE_ELAPSED_WAIT: Duration = Duration::from_millis(50);

/// Arbitrary success value used across tests.
const SUCCESS_VALUE: i32 = 42;

/// Arbitrary error value for tests.
const ERROR_VALUE: &str = "fail";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Creates a no-op waker for polling futures in unit tests.
fn noop_waker() -> Waker {
    struct NoopWake;
    impl std::task::Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    Waker::from(Arc::new(NoopWake))
}

/// Minimal `block_on` for immediately-ready unit-test futures.
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

#[derive(Clone)]
struct RecordingSleeper {
    calls: Rc<RefCell<Vec<Duration>>>,
}

impl RecordingSleeper {
    fn new() -> Self {
        Self {
            calls: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl Sleeper for RecordingSleeper {
    type Sleep = core::future::Ready<()>;

    fn sleep(&self, dur: Duration) -> Self::Sleep {
        self.calls.borrow_mut().push(dur);
        core::future::ready(())
    }
}

// ---------------------------------------------------------------------------
// 6.1, 6.2, 6.3: Async retry setup and awaitability
// ---------------------------------------------------------------------------

#[test]
fn retry_async_executes_when_sleeper_is_set() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let sleeper = RecordingSleeper::new();

    let call_count = Rc::new(Cell::new(0_u32));
    let future = policy.retry_async(|| {
        let call_count = Rc::clone(&call_count);
        call_count.set(call_count.get().saturating_add(1));
        async move {
            if call_count.get() < MAX_ATTEMPTS {
                Err(ERROR_VALUE)
            } else {
                Ok(SUCCESS_VALUE)
            }
        }
    });

    let result: Result<i32, RetryError<&str, i32>> = block_on(future.sleep(sleeper.clone()));
    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), MAX_ATTEMPTS);
}

#[test]
fn async_retry_type_is_nameable_from_crate_root() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    let retry = policy
        .retry_async(|| async { Ok::<i32, &str>(SUCCESS_VALUE) })
        .sleep(|_dur: Duration| async {});
    let _typed: tenacious::AsyncRetry<'_, _, _, _, _, _, _, _, _, _, _, i32, &str> = retry;
}

#[test]
fn async_retry_is_directly_awaitable() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    let sleeper = RecordingSleeper::new();
    let async_retry = policy
        .retry_async(|| async { Ok::<i32, &str>(SUCCESS_VALUE) })
        .sleep(sleeper);

    let result: Result<i32, RetryError<&str, i32>> = block_on(async_retry);
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[cfg(debug_assertions)]
#[test]
fn async_retry_repoll_after_completion_panics_in_debug() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    let mut retry = Box::pin(
        policy
            .retry_async(|| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(|_dur: Duration| async {}),
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
fn async_retry_repoll_after_completion_is_pending_in_release() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    let mut retry = Box::pin(
        policy
            .retry_async(|| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(|_dur: Duration| async {}),
    );
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    let first_poll = Future::poll(Pin::as_mut(&mut retry), &mut cx);
    assert_eq!(first_poll, Poll::Ready(Ok(SUCCESS_VALUE)));

    let second_poll = Future::poll(Pin::as_mut(&mut retry), &mut cx);
    assert_eq!(second_poll, Poll::Pending);
}

// ---------------------------------------------------------------------------
// 6.4: Async loop behavior matches sync semantics
// ---------------------------------------------------------------------------

#[test]
fn async_retry_returns_exhausted_on_persistent_errors() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let sleeper = RecordingSleeper::new();

    let result: Result<i32, RetryError<&str, i32>> = block_on(
        policy
            .retry_async(|| async { Err::<i32, &str>(ERROR_VALUE) })
            .sleep(sleeper),
    );

    match result {
        Err(RetryError::Exhausted {
            error, attempts, ..
        }) => {
            assert_eq!(error, ERROR_VALUE);
            assert_eq!(attempts, MAX_ATTEMPTS);
        }
        other => panic!("expected Exhausted, got {:?}", other),
    }
}

#[test]
fn async_retry_returns_condition_not_met_for_ok_exhaustion() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::ok(|value: &i32| *value < 0));
    let sleeper = RecordingSleeper::new();

    let result = block_on(
        policy
            .retry_async(|| async { Ok::<i32, &str>(-1) })
            .sleep(sleeper),
    );

    match result {
        Err(RetryError::ConditionNotMet { last, attempts, .. }) => {
            assert_eq!(last, -1);
            assert_eq!(attempts, MAX_ATTEMPTS);
        }
        other => panic!("expected ConditionNotMet, got {:?}", other),
    }
}

#[test]
fn async_sleep_receives_wait_strategy_delays() {
    let sleeper = RecordingSleeper::new();
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _result: Result<i32, RetryError<&str, i32>> = block_on(
        policy
            .retry_async(|| async { Err::<i32, &str>(ERROR_VALUE) })
            .sleep(sleeper.clone()),
    );

    let calls = sleeper.calls.borrow();
    let expected_sleep_calls = (MAX_ATTEMPTS - 1) as usize;
    assert_eq!(calls.len(), expected_sleep_calls);
    for duration in calls.iter() {
        assert_eq!(*duration, WAIT_DURATION);
    }
}

// ---------------------------------------------------------------------------
// Deferred retry-predicate checks in execution engine (4.9, 4.10)
// ---------------------------------------------------------------------------

#[test]
fn async_predicate_is_evaluated_before_stop() {
    // attempts(1) would stop immediately if checked first.
    // Predicate accepts this Ok, so result must be returned.
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(1))
        .when(on::ok(|value: &i32| *value < 0));
    let sleeper = RecordingSleeper::new();

    let result = block_on(
        policy
            .retry_async(|| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(sleeper),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn async_default_predicate_behaves_like_any_error() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let sleeper = RecordingSleeper::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|| {
                call_count.set(call_count.get().saturating_add(1));
                async { Err::<i32, &str>(ERROR_VALUE) }
            })
            .sleep(sleeper),
    );

    assert_eq!(call_count.get(), MAX_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn async_elapsed_stop_counts_operation_runtime() {
    let mut policy = RetryPolicy::new().stop(stop::elapsed(ELAPSED_DEADLINE));
    let sleeper = RecordingSleeper::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|| {
                call_count.set(call_count.get().saturating_add(1));
                async {
                    std::thread::sleep(OPERATION_RUNTIME);
                    Err::<i32, &str>("slow failure")
                }
            })
            .sleep(sleeper.clone()),
    );

    assert_eq!(call_count.get(), 1);
    assert!(sleeper.calls.borrow().is_empty());
    assert!(matches!(
        result,
        Err(RetryError::Exhausted { attempts: 1, .. })
    ));
}

#[test]
fn async_before_elapsed_uses_computed_next_delay_before_sleeping() {
    let mut policy = RetryPolicy::new()
        .stop(stop::before_elapsed(BEFORE_ELAPSED_DEADLINE))
        .wait(wait::fixed(BEFORE_ELAPSED_WAIT));
    let sleeper = RecordingSleeper::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|| {
                call_count.set(call_count.get().saturating_add(1));
                async { Err::<i32, &str>("would exceed budget") }
            })
            .sleep(sleeper.clone()),
    );

    assert_eq!(call_count.get(), 1);
    assert!(sleeper.calls.borrow().is_empty());
    assert!(matches!(
        result,
        Err(RetryError::Exhausted { attempts: 1, .. })
    ));
}

// ---------------------------------------------------------------------------
// 6.8: Async hooks are synchronous
// ---------------------------------------------------------------------------

#[test]
fn async_hooks_fire_in_expected_places() {
    let before_attempt_calls = Rc::new(RefCell::new(Vec::new()));
    let after_attempt_calls = Rc::new(RefCell::new(Vec::new()));
    let before_sleep_calls = Rc::new(RefCell::new(Vec::new()));
    let exhausted_called = Rc::new(Cell::new(false));
    let sleeper = RecordingSleeper::new();

    let before_attempt_ref = Rc::clone(&before_attempt_calls);
    let after_attempt_ref = Rc::clone(&after_attempt_calls);
    let before_sleep_ref = Rc::clone(&before_sleep_calls);
    let exhausted_ref = Rc::clone(&exhausted_called);

    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .before_attempt(move |state| {
            before_attempt_ref.borrow_mut().push(state.attempt);
        })
        .after_attempt(move |state: &tenacious::AttemptState<'_, i32, &str>| {
            after_attempt_ref.borrow_mut().push(state.attempt);
        })
        .before_sleep(move |state: &tenacious::AttemptState<'_, i32, &str>| {
            before_sleep_ref.borrow_mut().push(state.attempt);
        })
        .on_exhausted(move |_state: &tenacious::AttemptState<'_, i32, &str>| {
            exhausted_ref.set(true);
        });

    let _result: Result<i32, RetryError<&str, i32>> = block_on(
        policy
            .retry_async(|| async { Err::<i32, &str>(ERROR_VALUE) })
            .sleep(sleeper),
    );

    let before_attempt = before_attempt_calls.borrow();
    let after_attempt = after_attempt_calls.borrow();
    let before_sleep = before_sleep_calls.borrow();

    assert_eq!(*before_attempt, vec![1, 2, 3]);
    assert_eq!(*after_attempt, vec![1, 2, 3]);
    assert_eq!(*before_sleep, vec![1, 2]);
    assert!(exhausted_called.get());
}

// ---------------------------------------------------------------------------
// 6.6: tokio sleep re-export (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "tokio-sleep")]
#[test]
fn tokio_sleep_reexport_is_available() {
    let _sleep_fn: fn(Duration) -> tokio::time::Sleep = tenacious::sleep::tokio_sleep;
}

// ---------------------------------------------------------------------------
// 6.7: embassy sleep adapter (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "embassy-sleep")]
#[test]
fn embassy_sleep_adapter_is_available() {
    let _future = Sleeper::sleep(&tenacious::sleep::embassy_sleep, Duration::ZERO);
}

// ---------------------------------------------------------------------------
// Additional runtime sleep adapters (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(all(feature = "gloo-timers-sleep", target_arch = "wasm32"))]
#[test]
fn gloo_sleep_reexport_is_available() {
    let _future = tenacious::sleep::gloo_sleep(Duration::ZERO);
}

#[cfg(feature = "futures-timer-sleep")]
#[test]
fn futures_timer_sleep_adapter_is_available() {
    let _future = tenacious::sleep::futures_timer_sleep(Duration::ZERO);
}
