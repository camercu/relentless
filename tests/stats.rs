//! Tests for retry statistics (`RetryStats`, `StopReason`, `.with_stats()`).
//!
//! Verifies that stats are accumulated fresh per invocation, that attempt counts and
//! total_wait match expected values, that StopReason correctly distinguishes Accepted from
//! Exhausted (including the Accepted-when-Rejected edge case), and that total_elapsed is
//! Some only when the `std` feature is active.

use core::cell::Cell;
#[cfg(all(feature = "alloc", feature = "std"))]
use core::future::Future;
#[cfg(all(feature = "alloc", feature = "std"))]
use core::pin::Pin;
#[cfg(all(feature = "alloc", feature = "std"))]
use core::task::{Context, Poll, Waker};
use core::time::Duration;
#[cfg(all(feature = "alloc", feature = "std"))]
use std::rc::Rc;
#[cfg(all(feature = "alloc", feature = "std"))]
use std::sync::Arc;
use tenacious::{RetryError, RetryPolicy};
use tenacious::{RetryStats, StopReason, predicate, stop, wait};

const MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(5);
const SUCCESS_VALUE: i32 = 42;
const ERROR_VALUE: &str = "fail";

fn instant_sleep(_dur: Duration) {}

#[cfg(all(feature = "alloc", feature = "std"))]
fn noop_waker() -> Waker {
    struct NoopWake;
    impl std::task::Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    Waker::from(Arc::new(NoopWake))
}

#[cfg(all(feature = "alloc", feature = "std"))]
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
#[cfg(all(feature = "alloc", feature = "std"))]
struct InstantSleeper;

#[cfg(all(feature = "alloc", feature = "std"))]
impl tenacious::Sleeper for InstantSleeper {
    type Sleep = core::future::Ready<()>;

    fn sleep(&self, _dur: Duration) -> Self::Sleep {
        core::future::ready(())
    }
}

#[test]
fn sync_with_stats_returns_result_and_stats() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));
    let call_count = Cell::new(0_u32);

    let (result, stats) = policy
        .retry(|_| {
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
    #[cfg(feature = "std")]
    assert!(stats.total_elapsed.is_some());
    #[cfg(not(feature = "std"))]
    assert!(stats.total_elapsed.is_none());
    assert_eq!(stats.stop_reason, StopReason::Accepted);
}

#[test]
#[cfg(all(feature = "alloc", feature = "std"))]
fn async_with_stats_returns_result_and_stats() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));
    let call_count = Rc::new(Cell::new(0_u32));

    let (result, stats) = block_on(
        policy
            .retry_async(|_| {
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
    assert_eq!(stats.stop_reason, StopReason::Accepted);
}

#[test]
fn sync_first_attempt_success_has_minimal_stats() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let (result, stats) = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.total_wait, Duration::ZERO);
    #[cfg(feature = "std")]
    assert!(stats.total_elapsed.is_some());
    #[cfg(not(feature = "std"))]
    assert!(stats.total_elapsed.is_none());
    assert_eq!(stats.stop_reason, StopReason::Accepted);
}

#[test]
fn sync_stats_total_wait_accumulates_with_exponential() {
    let initial = Duration::from_millis(10);
    let num_attempts: u32 = 4;
    let policy = RetryPolicy::new()
        .stop(stop::attempts(num_attempts))
        .wait(wait::exponential(initial));

    let (_result, stats) = policy
        .retry(|_| Err::<i32, _>(ERROR_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    // 4 attempts = 3 inter-attempt sleeps: 10ms + 20ms + 40ms = 70ms
    let expected_wait =
        Duration::from_millis(10) + Duration::from_millis(20) + Duration::from_millis(40);
    assert_eq!(stats.attempts, num_attempts);
    assert_eq!(stats.total_wait, expected_wait);
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
}

#[test]
fn sync_stop_reason_accepted_with_default_predicate() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let (result, stats) = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.stop_reason, StopReason::Accepted);
}

#[test]
fn sync_stop_reason_exhausted_on_exhaustion() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let (_result, stats) = policy
        .retry(|_| Err::<i32, _>(ERROR_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(stats.attempts, MAX_ATTEMPTS);
    assert_eq!(
        stats.total_wait,
        WAIT_DURATION.saturating_mul(MAX_ATTEMPTS - 1)
    );
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
}

#[test]
fn sync_stop_reason_accepted_for_custom_predicate_on_ok() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|value: &i32| *value < 0));

    let (result, stats) = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.total_wait, Duration::ZERO);
    assert_eq!(stats.stop_reason, StopReason::Accepted);
}

#[test]
fn sync_stop_reason_accepted_for_result_predicate_on_ok() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::result(|r: &Result<i32, &str>| r.is_err()));

    let (result, stats) = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.stop_reason, StopReason::Accepted);
}

#[test]
fn sync_stop_reason_accepted_for_error_predicate_on_ok() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::error(|e: &&str| *e == "retryable"));

    let (result, stats) = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.stop_reason, StopReason::Accepted);
}

#[test]
fn sync_stop_reason_accepted_when_error_rejected() {
    // A non-retryable error (rejected by the predicate) exits immediately with Accepted,
    // because the predicate "accepted" the decision to stop — it was not exhausted.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::error(|e: &&str| *e == "retryable"));

    let (result, stats) = policy
        .retry(|_| Err::<i32, _>("fatal"))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert!(matches!(result, Err(RetryError::Rejected { .. })));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.stop_reason, StopReason::Accepted);
}

#[test]
fn sync_stop_reason_exhausted_on_condition_not_met() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|v: &i32| *v < 0));

    let (result, stats) = policy
        .retry(|_| Ok::<_, &str>(-1_i32))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    match result {
        Err(RetryError::Exhausted { last, .. }) => {
            assert_eq!(last, Ok(-1));
        }
        other => panic!("expected Exhausted, got {:?}", other),
    }
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
    assert_eq!(stats.attempts, MAX_ATTEMPTS);
}

#[test]
#[cfg(all(feature = "alloc", feature = "std"))]
fn async_stop_reason_exhausted_on_exhaustion() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let (_result, stats) = block_on(
        policy
            .retry_async(|_| async { Err::<i32, &str>(ERROR_VALUE) })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert_eq!(stats.attempts, MAX_ATTEMPTS);
    assert_eq!(
        stats.total_wait,
        WAIT_DURATION.saturating_mul(MAX_ATTEMPTS - 1)
    );
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
}

#[test]
#[cfg(all(feature = "alloc", feature = "std"))]
fn async_first_attempt_success_has_minimal_stats() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let (result, stats) = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.total_wait, Duration::ZERO);
    assert_eq!(stats.stop_reason, StopReason::Accepted);
}

#[test]
#[cfg(all(feature = "alloc", feature = "std"))]
fn async_stop_reason_condition_not_met() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|v: &i32| *v < 0));

    let (result, stats) = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(-1) })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert!(matches!(
        result,
        Err(RetryError::Exhausted { last: Ok(-1), .. })
    ));
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
    assert_eq!(stats.attempts, MAX_ATTEMPTS);
}

#[test]
#[cfg(all(feature = "alloc", feature = "std"))]
fn async_stop_reason_accepted_for_custom_predicate_on_ok() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|value: &i32| *value < 0));

    let (result, stats) = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.total_wait, Duration::ZERO);
    assert_eq!(stats.stop_reason, StopReason::Accepted);
}

#[test]
fn sync_call_without_stats_returns_plain_result() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result: Result<i32, RetryError<i32, &str>> = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
#[cfg(feature = "std")]
fn sync_total_elapsed_is_some_with_std() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let (_result, stats) = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert!(
        stats.total_elapsed.is_some(),
        "total_elapsed should be Some when std feature is active"
    );
}

#[test]
#[cfg(all(feature = "alloc", feature = "std"))]
fn async_total_elapsed_is_some_with_std() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let (_result, stats) = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(SUCCESS_VALUE) })
            .sleep(InstantSleeper)
            .with_stats(),
    );

    assert!(
        stats.total_elapsed.is_some(),
        "total_elapsed should be Some when std feature is active"
    );
}

#[test]
fn retry_stats_implements_debug_and_clone() {
    let stats = RetryStats {
        attempts: MAX_ATTEMPTS,
        total_elapsed: Some(Duration::from_secs(1)),
        total_wait: WAIT_DURATION,
        stop_reason: StopReason::Accepted,
    };

    let cloned = stats;
    assert_eq!(stats, cloned);

    let debug = format!("{:?}", stats);
    assert!(debug.contains("RetryStats"), "Debug output: {debug}");
}

#[test]
fn stop_reason_implements_debug_clone_copy_eq() {
    let reason = StopReason::Accepted;

    let copied = reason; // Copy
    assert_eq!(reason, copied);

    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&reason); // Clone (compile-time check)

    let debug = format!("{:?}", reason);
    assert!(debug.contains("Accepted"), "Debug output: {debug}");

    assert_ne!(StopReason::Accepted, StopReason::Exhausted); // Eq, both variants distinct
}

#[test]
fn sync_stats_are_fresh_after_policy_reuse() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let (_result1, stats1) = policy
        .retry(|_| Err::<i32, _>(ERROR_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();
    assert_eq!(stats1.attempts, MAX_ATTEMPTS);
    assert_eq!(stats1.stop_reason, StopReason::Exhausted);

    let (result2, stats2) = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .with_stats()
        .call();
    assert_eq!(result2, Ok(SUCCESS_VALUE));
    assert_eq!(stats2.attempts, 1);
    assert_eq!(stats2.total_wait, Duration::ZERO);
    assert_eq!(stats2.stop_reason, StopReason::Accepted);
}

#[test]
fn sync_stats_work_alongside_hooks() {
    let hook_calls = Cell::new(0_u32);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let (result, stats) = policy
        .retry(|_| {
            if hook_calls.get() < MAX_ATTEMPTS {
                Err::<i32, _>(ERROR_VALUE)
            } else {
                Ok(SUCCESS_VALUE)
            }
        })
        .before_attempt(|_state| {
            hook_calls.set(hook_calls.get().saturating_add(1));
        })
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(stats.attempts, MAX_ATTEMPTS);
    assert_eq!(hook_calls.get(), MAX_ATTEMPTS);
}
