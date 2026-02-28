//! Acceptance tests for Phase 8: Statistics (Spec items 8.1–8.6).
//!
//! These tests verify:
//! - `.with_stats()` on SyncRetry/AsyncRetry changes return type (8.1)
//! - `RetryStats` struct fields: attempts, total_elapsed, total_wait, stop_reason (8.2)
//! - `StopReason` variants: Success, StopCondition, PredicateAccepted (8.3)
//! - Stats accumulated inside the execution engine (8.4)
//! - `total_elapsed` is `Some` when std active (8.5, verified implicitly)
//! - `RetryStats` derives Debug, Clone; StopReason derives Debug, Clone, Copy, Eq (8.6)

use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use std::rc::Rc;
use std::sync::Arc;
use tenacious::error::RetryError;
use tenacious::policy::RetryPolicy;
use tenacious::{RetryStats, StopReason, on, stop, wait};

// ---------------------------------------------------------------------------
// Test constants
// ---------------------------------------------------------------------------

/// Maximum attempts for most stats tests.
const MAX_ATTEMPTS: u32 = 3;

/// Fixed wait duration used in tests.
const WAIT_DURATION: Duration = Duration::from_millis(5);

/// Arbitrary success value.
const SUCCESS_VALUE: i32 = 42;

/// Arbitrary error value.
const ERROR_VALUE: &str = "fail";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn instant_sleep(_dur: Duration) {}

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

#[derive(Clone, Copy)]
struct InstantSleeper;

impl tenacious::Sleeper for InstantSleeper {
    type Sleep = core::future::Ready<()>;

    fn sleep(&self, _dur: Duration) -> Self::Sleep {
        core::future::ready(())
    }
}

// ---------------------------------------------------------------------------
// 8.1: .with_stats() changes return type to (Result, RetryStats)
// ---------------------------------------------------------------------------

#[test]
fn sync_with_stats_returns_result_and_stats() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));
    let call_count = Cell::new(0_u32);

    let (result, stats) = policy
        .retry(|| {
            let n = call_count.get().saturating_add(1);
            call_count.set(n);
            if n < MAX_ATTEMPTS {
                Err::<i32, _>(ERROR_VALUE)
            } else {
                Ok(SUCCESS_VALUE)
            }
        })
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, MAX_ATTEMPTS);
    assert_eq!(
        stats.total_wait,
        WAIT_DURATION.saturating_mul(MAX_ATTEMPTS - 1)
    );
    assert!(stats.total_elapsed.is_some());
    assert_eq!(stats.stop_reason, StopReason::Success);
}

#[test]
fn async_with_stats_returns_result_and_stats() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));
    let call_count = Rc::new(Cell::new(0_u32));

    let (result, stats) = block_on(
        policy
            .retry_async(|| {
                let count = Rc::clone(&call_count);
                count.set(count.get().saturating_add(1));
                async move {
                    if count.get() < MAX_ATTEMPTS {
                        Err::<i32, _>(ERROR_VALUE)
                    } else {
                        Ok(SUCCESS_VALUE)
                    }
                }
            })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, MAX_ATTEMPTS);
    assert_eq!(
        stats.total_wait,
        WAIT_DURATION.saturating_mul(MAX_ATTEMPTS - 1)
    );
    assert!(stats.total_elapsed.is_some());
    assert_eq!(stats.stop_reason, StopReason::Success);
}

// ---------------------------------------------------------------------------
// 8.2: RetryStats fields
// ---------------------------------------------------------------------------

#[test]
fn sync_first_attempt_success_has_minimal_stats() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let (result, stats) = policy
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.total_wait, Duration::ZERO);
    assert!(stats.total_elapsed.is_some());
    assert_eq!(stats.stop_reason, StopReason::Success);
}

#[test]
fn sync_stats_total_wait_accumulates_with_exponential() {
    let initial = Duration::from_millis(10);
    let num_attempts: u32 = 4;
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(num_attempts))
        .wait(wait::exponential(initial));

    let (_result, stats) = policy
        .retry(|| Err::<i32, _>(ERROR_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    // 4 attempts = 3 sleeps: 10ms + 20ms + 40ms = 70ms
    let expected_wait =
        Duration::from_millis(10) + Duration::from_millis(20) + Duration::from_millis(40);
    assert_eq!(stats.attempts, num_attempts);
    assert_eq!(stats.total_wait, expected_wait);
    assert_eq!(stats.stop_reason, StopReason::StopCondition);
}

// ---------------------------------------------------------------------------
// 8.3: StopReason variants
// ---------------------------------------------------------------------------

#[test]
fn sync_stop_reason_success_with_default_predicate() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let (result, stats) = policy
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.stop_reason, StopReason::Success);
}

#[test]
fn sync_stop_reason_stop_condition_on_exhaustion() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let (_result, stats) = policy
        .retry(|| Err::<i32, _>(ERROR_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(stats.attempts, MAX_ATTEMPTS);
    assert_eq!(
        stats.total_wait,
        WAIT_DURATION.saturating_mul(MAX_ATTEMPTS - 1)
    );
    assert_eq!(stats.stop_reason, StopReason::StopCondition);
}

#[test]
fn sync_stop_reason_predicate_accepted_for_custom_predicate_on_ok() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::ok(|value: &i32| *value < 0));

    let (result, stats) = policy
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.total_wait, Duration::ZERO);
    assert_eq!(stats.stop_reason, StopReason::PredicateAccepted);
}

#[test]
fn sync_stop_reason_predicate_accepted_when_error_rejected() {
    // on::error() rejects "fatal" errors — predicate says "don't retry this".
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::error(|e: &&str| *e == "retryable"));

    let (result, stats) = policy
        .retry(|| Err::<i32, _>("fatal"))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    // Predicate rejected the error, so we get PredicateAccepted (not StopCondition).
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.stop_reason, StopReason::PredicateAccepted);
}

#[test]
fn sync_stop_reason_stop_condition_on_condition_not_met() {
    // Using on::ok() to retry while value is negative. Stop fires before we get a positive.
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::ok(|v: &i32| *v < 0));

    let (result, stats) = policy
        .retry(|| Ok::<_, &str>(-1_i32))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    match result {
        Err(RetryError::ConditionNotMet { last, attempts, .. }) => {
            assert_eq!(last, -1);
            assert_eq!(attempts, MAX_ATTEMPTS);
        }
        other => panic!("expected ConditionNotMet, got {:?}", other),
    }
    assert_eq!(stats.stop_reason, StopReason::StopCondition);
    assert_eq!(stats.attempts, MAX_ATTEMPTS);
}

// ---------------------------------------------------------------------------
// 8.3: StopReason async variants
// ---------------------------------------------------------------------------

#[test]
fn async_stop_reason_stop_condition_on_exhaustion() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let (_result, stats) = block_on(
        policy
            .retry_async(|| async { Err::<i32, &str>(ERROR_VALUE) })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert_eq!(stats.attempts, MAX_ATTEMPTS);
    assert_eq!(
        stats.total_wait,
        WAIT_DURATION.saturating_mul(MAX_ATTEMPTS - 1)
    );
    assert_eq!(stats.stop_reason, StopReason::StopCondition);
}

#[test]
fn async_first_attempt_success_has_minimal_stats() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let (result, stats) = block_on(
        policy
            .retry_async(|| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.total_wait, Duration::ZERO);
    assert_eq!(stats.stop_reason, StopReason::Success);
}

#[test]
fn async_stop_reason_condition_not_met() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::ok(|v: &i32| *v < 0));

    let (result, stats) = block_on(
        policy
            .retry_async(|| async { Ok::<i32, &str>(-1) })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert!(matches!(
        result,
        Err(RetryError::ConditionNotMet { last: -1, .. })
    ));
    assert_eq!(stats.stop_reason, StopReason::StopCondition);
    assert_eq!(stats.attempts, MAX_ATTEMPTS);
}

#[test]
fn async_stop_reason_predicate_accepted_for_custom_predicate_on_ok() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::ok(|value: &i32| *value < 0));

    let (result, stats) = block_on(
        policy
            .retry_async(|| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.total_wait, Duration::ZERO);
    assert_eq!(stats.stop_reason, StopReason::PredicateAccepted);
}

// ---------------------------------------------------------------------------
// 8.4: Stats only when with_stats is active (call() without stats still works)
// ---------------------------------------------------------------------------

#[test]
fn sync_call_without_stats_returns_plain_result() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result: Result<i32, RetryError<&str, i32>> = policy
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

// ---------------------------------------------------------------------------
// 8.5: total_elapsed is Some when std is active
// ---------------------------------------------------------------------------

#[test]
fn sync_total_elapsed_is_some_with_std() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(1));

    let (_result, stats) = policy
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert!(
        stats.total_elapsed.is_some(),
        "total_elapsed should be Some when std feature is active"
    );
}

#[test]
fn async_total_elapsed_is_some_with_std() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(1));

    let (_result, stats) = block_on(
        policy
            .retry_async(|| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert!(
        stats.total_elapsed.is_some(),
        "total_elapsed should be Some when std feature is active"
    );
}

// ---------------------------------------------------------------------------
// 8.6: RetryStats implements Debug, Clone; StopReason implements Debug, Clone, Copy, Eq
// ---------------------------------------------------------------------------

#[test]
fn retry_stats_implements_debug_and_clone() {
    let stats = RetryStats {
        attempts: MAX_ATTEMPTS,
        total_elapsed: Some(Duration::from_secs(1)),
        total_wait: WAIT_DURATION,
        stop_reason: StopReason::Success,
    };

    let cloned = stats.clone();
    assert_eq!(stats, cloned);

    let debug = format!("{:?}", stats);
    assert!(debug.contains("RetryStats"), "Debug output: {debug}");
}

#[test]
fn stop_reason_implements_debug_clone_copy_eq() {
    let reason = StopReason::Success;

    // Copy
    let copied = reason;
    assert_eq!(reason, copied);

    // Clone (compile-time trait bound check)
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&reason);

    // Debug
    let debug = format!("{:?}", reason);
    assert!(debug.contains("Success"), "Debug output: {debug}");

    // All three variants
    assert_ne!(StopReason::Success, StopReason::StopCondition);
    assert_ne!(StopReason::StopCondition, StopReason::PredicateAccepted);
    assert_ne!(StopReason::Success, StopReason::PredicateAccepted);
}

// ---------------------------------------------------------------------------
// Policy reuse: stats are fresh after reset
// ---------------------------------------------------------------------------

#[test]
fn sync_stats_are_fresh_after_policy_reuse() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    // First invocation: exhaust all attempts.
    let (_result1, stats1) = policy
        .retry(|| Err::<i32, _>(ERROR_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();
    assert_eq!(stats1.attempts, MAX_ATTEMPTS);
    assert_eq!(stats1.stop_reason, StopReason::StopCondition);

    // Second invocation: succeed on first attempt.
    let (result2, stats2) = policy
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();
    assert_eq!(result2, Ok(SUCCESS_VALUE));
    assert_eq!(stats2.attempts, 1);
    assert_eq!(stats2.total_wait, Duration::ZERO);
    assert_eq!(stats2.stop_reason, StopReason::Success);
}

// ---------------------------------------------------------------------------
// Stats with hooks active (no interference)
// ---------------------------------------------------------------------------

#[test]
fn sync_stats_work_alongside_hooks() {
    let hook_calls = Cell::new(0_u32);
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION))
        .before_attempt(|_state| {
            hook_calls.set(hook_calls.get().saturating_add(1));
        });

    let (result, stats) = policy
        .retry(|| {
            if hook_calls.get() < MAX_ATTEMPTS {
                Err::<i32, _>(ERROR_VALUE)
            } else {
                Ok(SUCCESS_VALUE)
            }
        })
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, MAX_ATTEMPTS);
    assert_eq!(hook_calls.get(), MAX_ATTEMPTS);
}
