//! Acceptance tests for Async Execution (Spec items 6.1–6.8).
//!
//! These tests verify:
//! - `RetryPolicy::retry_async(op)` configures async retry (6.1)
//! - `AsyncRetry::sleep(...)` sets required sleeper and enables execution (6.2)
//! - `AsyncRetry` is directly awaitable (6.3)
//! - Async execution loop behavior matches sync semantics (6.4)
//! - Async execution is executor-agnostic and deterministic in-process (6.5)
//! - runtime sleep helper constructors are available behind feature gates
//! - Async hook callbacks are synchronous and fire at the right points (6.8)
//!
//! This file also closes deferred retry-predicate execution behavior checks:
//! - Predicate evaluated before stop (4.9)
//! - Default predicate behaves like `on::any_error()` (4.10)

use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tenacious::{RetryError, RetryPolicy};
use tenacious::{predicate, sleep::Sleeper, stop, wait};

// ---------------------------------------------------------------------------
// Test constants
// ---------------------------------------------------------------------------

/// Maximum attempts for most async retry tests.
const MAX_ATTEMPTS: u32 = 3;

/// Wait duration used in async sleep verification tests.
const WAIT_DURATION: Duration = Duration::from_millis(10);

/// Deadline used for deterministic custom-clock elapsed stop tests.
const ASYNC_CUSTOM_CLOCK_DEADLINE: Duration = Duration::from_millis(5);

/// Simulated operation runtime increment for custom-clock tests.
const ASYNC_CUSTOM_CLOCK_STEP_MILLIS: u64 = 10;

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

static ASYNC_ELAPSED_CLOCK_MILLIS: AtomicU64 = AtomicU64::new(0);

fn async_elapsed_clock_millis() -> Duration {
    Duration::from_millis(ASYNC_ELAPSED_CLOCK_MILLIS.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// 6.1, 6.2, 6.3: Async retry setup and awaitability
// ---------------------------------------------------------------------------

#[test]
fn retry_async_executes_when_sleeper_is_set() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let sleeper = RecordingSleeper::new();

    let call_count = Rc::new(Cell::new(0_u32));
    let future = policy.retry_async(|_| {
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

    let result: Result<i32, RetryError<i32, &str>> = block_on(future.sleep(sleeper.clone()));
    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), MAX_ATTEMPTS);
}

#[test]
fn async_retry_type_is_nameable_from_crate_root() {
    #[allow(clippy::type_complexity)]
    fn assert_nameable<S, W, P, BA, AA, OE, F, Fut, SleepImpl, T, E, SleepFut>(
        retry: tenacious::AsyncRetry<'_, S, W, P, BA, AA, OE, F, Fut, SleepImpl, T, E, SleepFut>,
    ) where
        F: FnMut(tenacious::RetryState) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let _ = retry;
    }

    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let retry = policy
        .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
        .sleep(|_dur: Duration| async {});
    assert_nameable(retry);
    let result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(|_dur: Duration| async {}),
    );
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn async_retry_is_directly_awaitable() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let sleeper = RecordingSleeper::new();
    let async_retry = policy
        .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
        .sleep(sleeper);

    let result: Result<i32, RetryError<i32, &str>> = block_on(async_retry);
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn async_retry_repoll_after_completion_panics() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let mut retry = Box::pin(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
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

#[test]
fn retry_async_borrows_policy_immutably() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(Duration::ZERO));
    let call_count = Rc::new(Cell::new(0_u32));

    let result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| {
                let call_count = Rc::clone(&call_count);
                call_count.set(call_count.get().saturating_add(1));
                async move { Err::<i32, &str>(ERROR_VALUE) }
            })
            .sleep(|_dur: Duration| async {}),
    );

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count.get(), 2);
}

// ---------------------------------------------------------------------------
// 6.4: Async loop behavior matches sync semantics
// ---------------------------------------------------------------------------

#[test]
fn async_retry_returns_exhausted_on_persistent_errors() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let sleeper = RecordingSleeper::new();

    let result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .sleep(sleeper),
    );

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Err(ERROR_VALUE));
        }
        other => panic!("expected Exhausted, got {:?}", other),
    }
}

#[test]
fn async_retry_returns_exhausted_for_ok_exhaustion() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|value: &i32| *value < 0));
    let sleeper = RecordingSleeper::new();

    let result = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(-1) })
            .sleep(sleeper),
    );

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Ok(-1));
        }
        other => panic!("expected Exhausted, got {:?}", other),
    }
}

#[test]
fn async_composed_polling_predicate_handles_transient_errors_and_not_ready_values() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .when(
            predicate::error(|error: &&str| *error == ERROR_VALUE)
                | predicate::ok(|value: &i32| *value < SUCCESS_VALUE),
        );
    let sleeper = RecordingSleeper::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                let current = call_count.get().saturating_add(1);
                call_count.set(current);
                async move {
                    match current {
                        1 => Err::<i32, &str>(ERROR_VALUE),
                        2 => Ok(0),
                        _ => Ok(SUCCESS_VALUE),
                    }
                }
            })
            .sleep(sleeper),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), 3);
}

#[test]
fn async_sleep_receives_wait_strategy_delays() {
    let sleeper = RecordingSleeper::new();
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
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
    let policy = RetryPolicy::new()
        .stop(stop::attempts(1))
        .when(predicate::ok(|value: &i32| *value < 0));
    let sleeper = RecordingSleeper::new();

    let result = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(sleeper),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn async_default_predicate_behaves_like_any_error() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let sleeper = RecordingSleeper::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async { Err::<i32, &str>(ERROR_VALUE) }
            })
            .sleep(sleeper),
    );

    assert_eq!(call_count.get(), MAX_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn async_custom_elapsed_clock_counts_operation_runtime() {
    ASYNC_ELAPSED_CLOCK_MILLIS.store(0, Ordering::Relaxed);
    let policy = RetryPolicy::new().stop(stop::elapsed(ASYNC_CUSTOM_CLOCK_DEADLINE));
    let sleeper = RecordingSleeper::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async {
                    ASYNC_ELAPSED_CLOCK_MILLIS
                        .fetch_add(ASYNC_CUSTOM_CLOCK_STEP_MILLIS, Ordering::Relaxed);
                    Err::<i32, &str>("slow failure")
                }
            })
            .elapsed_clock(async_elapsed_clock_millis)
            .sleep(sleeper.clone()),
    );

    assert_eq!(call_count.get(), 1);
    assert!(sleeper.calls.borrow().is_empty());
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn async_elapsed_stop_triggers_after_deadline() {
    ASYNC_ELAPSED_CLOCK_MILLIS.store(0, Ordering::Relaxed);
    let policy = RetryPolicy::new()
        .stop(stop::elapsed(Duration::from_millis(5)))
        .wait(wait::fixed(Duration::from_millis(1)));
    let sleeper = RecordingSleeper::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async {
                    // Advance clock past the deadline on the first attempt
                    ASYNC_ELAPSED_CLOCK_MILLIS.fetch_add(10, Ordering::Relaxed);
                    Err::<i32, &str>("would exceed budget")
                }
            })
            .elapsed_clock(async_elapsed_clock_millis)
            .sleep(sleeper.clone()),
    );

    assert_eq!(call_count.get(), 1);
    assert!(sleeper.calls.borrow().is_empty());
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

// ---------------------------------------------------------------------------
// 6.8: Async hooks are synchronous
// ---------------------------------------------------------------------------

#[test]
fn async_hooks_fire_in_expected_places() {
    let before_attempt_calls = Rc::new(RefCell::new(Vec::new()));
    let after_attempt_calls = Rc::new(RefCell::new(Vec::new()));
    let exit_reason = Rc::new(Cell::new(None));
    let sleeper = RecordingSleeper::new();

    let before_attempt_ref = Rc::clone(&before_attempt_calls);
    let after_attempt_ref = Rc::clone(&after_attempt_calls);
    let exit_reason_ref = Rc::clone(&exit_reason);

    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .before_attempt(move |state| {
                before_attempt_ref.borrow_mut().push(state.attempt);
            })
            .after_attempt(move |state: &tenacious::AttemptState<'_, i32, &str>| {
                after_attempt_ref.borrow_mut().push(state.attempt);
            })
            .on_exit(move |state: &tenacious::ExitState<'_, i32, &str>| {
                exit_reason_ref.set(Some(state.stop_reason));
            })
            .sleep(sleeper),
    );

    let before_attempt = before_attempt_calls.borrow();
    let after_attempt = after_attempt_calls.borrow();

    assert_eq!(*before_attempt, vec![1, 2, 3]);
    assert_eq!(*after_attempt, vec![1, 2, 3]);
    assert_eq!(exit_reason.get(), Some(tenacious::StopReason::Exhausted));
}

#[test]
fn async_on_exit_reports_success_reason() {
    let exit_reason = Rc::new(Cell::new(None));
    let exit_reason_ref = Rc::clone(&exit_reason);
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .on_exit(move |state: &tenacious::ExitState<'_, i32, &str>| {
                exit_reason_ref.set(Some(state.stop_reason));
            })
            .sleep(RecordingSleeper::new()),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(exit_reason.get(), Some(tenacious::StopReason::Accepted));
}

#[test]
fn async_on_exit_reports_non_retryable_error_reason() {
    let exit_reason = Rc::new(Cell::new(None));
    let exit_reason_ref = Rc::clone(&exit_reason);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::error(|err: &&str| *err == "retryable"));

    let result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>("fatal") })
            .on_exit(move |state: &tenacious::ExitState<'_, i32, &str>| {
                exit_reason_ref.set(Some(state.stop_reason));
            })
            .sleep(RecordingSleeper::new()),
    );

    assert!(matches!(result, Err(RetryError::Rejected { .. })));
    assert_eq!(exit_reason.get(), Some(tenacious::StopReason::Accepted));
}

#[test]
fn async_hooks_are_per_call_and_do_not_persist() {
    let exit_calls = Rc::new(Cell::new(0_u32));
    let exit_calls_ref = Rc::clone(&exit_calls);
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let _ = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .on_exit(move |_state: &tenacious::ExitState<'_, i32, &str>| {
                exit_calls_ref.set(exit_calls_ref.get().saturating_add(1));
            })
            .sleep(RecordingSleeper::new()),
    );
    assert_eq!(exit_calls.get(), 1);

    let _ = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .sleep(RecordingSleeper::new()),
    );
    assert_eq!(exit_calls.get(), 1);
}

// ---------------------------------------------------------------------------
// Runtime sleep helpers (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "tokio-sleep")]
#[test]
fn tokio_sleep_helper_is_available() {
    let sleep_fn: fn(Duration) -> tokio::time::Sleep = tenacious::sleep::tokio();
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(sleep_fn),
    );
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[cfg(all(feature = "embassy-sleep", target_os = "none"))]
#[test]
fn embassy_sleep_helper_is_available() {
    let sleep_fn: fn(Duration) -> embassy_time::Timer = tenacious::sleep::embassy();
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(sleep_fn),
    );
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[cfg(all(feature = "gloo-timers-sleep", target_arch = "wasm32"))]
#[test]
fn gloo_sleep_helper_is_available() {
    let sleep_fn: fn(Duration) -> gloo_timers::future::TimeoutFuture = tenacious::sleep::gloo();
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(sleep_fn),
    );
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[cfg(feature = "futures-timer-sleep")]
#[test]
fn futures_timer_sleep_helper_is_available() {
    let sleep_fn: fn(Duration) -> futures_timer::Delay = tenacious::sleep::futures_timer();
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let result: Result<i32, RetryError<i32, &str>> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(sleep_fn),
    );
    assert_eq!(result, Ok(SUCCESS_VALUE));
}
