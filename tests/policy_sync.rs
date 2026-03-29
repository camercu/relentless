//! Acceptance tests for policy builder and sync execution.
//!
//! These tests verify:
//! - RetryPolicy::new() type-state and RetryPolicy::default() safe defaults
//! - Builder methods: .stop(), .wait(), .when()
//! - SyncRetry via .retry(op).call()
//! - RetryPolicy is Clone when constituents are Clone
//! - Reset on each .retry() invocation
//! - Hook callbacks: before_attempt, after_attempt, on_exit
//! - Sleep function requirement

use core::cell::Cell;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::cell::RefCell;
use tenacious::{RetryError, RetryPolicy, StopReason};
use tenacious::{predicate, stop, wait};

// ---------------------------------------------------------------------------
// Test constants
// ---------------------------------------------------------------------------

/// Maximum attempts for most retry tests.
const MAX_ATTEMPTS: u32 = 3;

/// Safe default maximum attempts.
const DEFAULT_POLICY_MAX_ATTEMPTS: u32 = 3;

/// Fixed wait duration used in tests requiring sleep.
const WAIT_DURATION: Duration = Duration::from_millis(10);

/// Safe default initial backoff duration.
const DEFAULT_POLICY_INITIAL_WAIT: Duration = Duration::from_millis(100);

/// Arbitrary success value.
const SUCCESS_VALUE: i32 = 42;

/// Deadline used for deterministic custom-clock elapsed stop tests.
const CUSTOM_CLOCK_DEADLINE: Duration = Duration::from_millis(5);

/// Simulated operation runtime increment for custom-clock tests.
const CUSTOM_CLOCK_STEP_MILLIS: u64 = 10;

/// Short wait used by policy-storage ergonomics tests.
const STORAGE_POLICY_WAIT: Duration = Duration::from_millis(1);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A no-op sleep function that doesn't actually sleep (for fast tests).
fn instant_sleep(_dur: Duration) {}

/// Records each requested sleep duration.
fn recording_sleep(recorder: &RefCell<Vec<Duration>>) -> impl FnMut(Duration) + '_ {
    move |dur| recorder.borrow_mut().push(dur)
}

static ELAPSED_CLOCK_MILLIS: AtomicU64 = AtomicU64::new(0);

fn elapsed_clock_millis() -> Duration {
    Duration::from_millis(ELAPSED_CLOCK_MILLIS.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// RetryPolicy::new() type-state + RetryPolicy::default() safe policy
// ---------------------------------------------------------------------------

#[test]
fn new_policy_retries_on_any_error_by_default() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(call_count.get(), MAX_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn new_policy_accepts_ok_immediately() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn new_policy_has_exponential_wait_by_default() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let sleeps: RefCell<Vec<Duration>> = RefCell::new(Vec::new());

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .sleep(recording_sleep(&sleeps))
        .call();

    let recorded = sleeps.borrow();
    assert_eq!(recorded.len(), (MAX_ATTEMPTS - 1) as usize);
    // Default wait is exponential starting at 100ms.
    assert_eq!(recorded[0], Duration::from_millis(100));
    assert_eq!(recorded[1], Duration::from_millis(200));
}

// ---------------------------------------------------------------------------
// Builder methods replace type params
// ---------------------------------------------------------------------------

#[test]
fn stop_builder_configures_stop_strategy() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(call_count.get(), MAX_ATTEMPTS);
}

#[test]
fn wait_builder_configures_wait_strategy() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(WAIT_DURATION));
    let sleeps: RefCell<Vec<Duration>> = RefCell::new(Vec::new());

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .sleep(recording_sleep(&sleeps))
        .call();

    assert_eq!(*sleeps.borrow(), vec![WAIT_DURATION]);
}

#[test]
fn when_builder_configures_predicate() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::error(|e: &&str| *e == "retryable"));

    // Retryable error: should retry until exhausted.
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("retryable")
        })
        .sleep(instant_sleep)
        .call();
    assert_eq!(call_count.get(), MAX_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));

    // Non-retryable error: should NOT retry, returns immediately.
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fatal")
        })
        .sleep(instant_sleep)
        .call();
    assert_eq!(call_count.get(), 1);
    match result {
        Err(RetryError::Rejected { last }) => {
            assert_eq!(last, "fatal");
        }
        other => panic!("expected Rejected with last=\"fatal\", got {:?}", other),
    }
}

#[test]
fn policy_is_easy_to_store_via_three_type_params() {
    type DbPolicy =
        RetryPolicy<stop::StopAfterAttempts, wait::WaitFixed, predicate::PredicateAnyError>;

    struct Service {
        retry: DbPolicy,
    }

    let service = Service {
        retry: RetryPolicy::new()
            .stop(stop::attempts(2))
            .wait(wait::fixed(STORAGE_POLICY_WAIT)),
    };

    let result = service
        .retry
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn retry_borrows_policy_without_mut() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(STORAGE_POLICY_WAIT));
    let call_count = Cell::new(0_u32);

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count.get(), 2);
}

// ---------------------------------------------------------------------------
// SyncRetry execution
// ---------------------------------------------------------------------------

#[test]
fn retry_succeeds_after_transient_failures() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            if n < MAX_ATTEMPTS {
                Err("transient")
            } else {
                Ok(SUCCESS_VALUE)
            }
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), MAX_ATTEMPTS);
}

#[test]
fn sync_retry_type_is_nameable_from_crate_root() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let retry = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep);
    let typed: tenacious::SyncRetry<'_, _, _, _, _, _, _, _, _, i32, &str> = retry;
    assert_eq!(typed.call(), Ok(SUCCESS_VALUE));
}

#[test]
fn retry_returns_exhausted_when_all_attempts_fail() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result = policy
        .retry(|_| Err::<i32, _>("always fails"))
        .sleep(instant_sleep)
        .call();

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Err("always fails"));
        }
        other => panic!("expected Exhausted, got {:?}", other),
    }
}

#[test]
fn retry_predicate_evaluated_before_stop() {
    // Predicate is checked before stop.
    // If predicate says "accept this outcome", return immediately even if
    // stop hasn't fired yet.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|v: &i32| *v < 0)); // retry on negative Ok values

    let result = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE)) // positive => predicate accepts
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn retry_with_never_stop_still_returns_on_ok() {
    // stop::never() means never stop, but predicate still accepts Ok.
    let policy = RetryPolicy::new().stop(stop::never());

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            if n < 3 {
                Err("transient")
            } else {
                Ok(SUCCESS_VALUE)
            }
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), 3);
}

#[test]
fn default_policy_retries_three_times() {
    let policy = RetryPolicy::default();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count.get(), DEFAULT_POLICY_MAX_ATTEMPTS);
}

#[test]
fn unparameterized_retry_policy_default_is_safe_policy() {
    let policy: RetryPolicy = RetryPolicy::default();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count.get(), DEFAULT_POLICY_MAX_ATTEMPTS);
}

#[test]
fn default_policy_uses_exponential_backoff() {
    let policy = RetryPolicy::default();
    let sleeps: RefCell<Vec<Duration>> = RefCell::new(Vec::new());

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .sleep(recording_sleep(&sleeps))
        .call();

    let durations = sleeps.borrow();
    assert_eq!(durations.len(), (DEFAULT_POLICY_MAX_ATTEMPTS - 1) as usize);
    assert_eq!(durations[0], DEFAULT_POLICY_INITIAL_WAIT);
    assert_eq!(durations[1], DEFAULT_POLICY_INITIAL_WAIT.saturating_mul(2),);
}

// ---------------------------------------------------------------------------
// Execution loop details — sleep is called with computed delay
// ---------------------------------------------------------------------------

#[test]
fn sleep_function_receives_computed_delay() {
    let sleep_durations: RefCell<Vec<Duration>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .sleep(|dur| sleep_durations.borrow_mut().push(dur))
        .call();

    let durations = sleep_durations.borrow();
    // 3 attempts = 2 sleeps (sleep happens between attempts, not after last).
    assert_eq!(durations.len(), (MAX_ATTEMPTS - 1) as usize);
    for d in durations.iter() {
        assert_eq!(*d, WAIT_DURATION);
    }
}

#[test]
fn exponential_wait_increases_sleep_durations() {
    let sleep_durations: RefCell<Vec<Duration>> = RefCell::new(Vec::new());
    let initial = Duration::from_millis(10);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(wait::exponential(initial));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .sleep(|dur| sleep_durations.borrow_mut().push(dur))
        .call();

    let durations = sleep_durations.borrow();
    assert_eq!(durations.len(), 3); // 4 attempts = 3 sleeps
    assert_eq!(durations[0], Duration::from_millis(10)); // 10ms * 2^0
    assert_eq!(durations[1], Duration::from_millis(20)); // 10ms * 2^1
    assert_eq!(durations[2], Duration::from_millis(40)); // 10ms * 2^2
}

// ---------------------------------------------------------------------------
// RetryPolicy is Clone
// ---------------------------------------------------------------------------

#[test]
fn policy_is_clone() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let policy2 = policy.clone();

    let result = policy2
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[cfg(feature = "alloc")]
#[test]
fn boxed_policy_erases_strategy_types() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed::<i32, &str>();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(call_count.get(), MAX_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

// ---------------------------------------------------------------------------
// Reset on each .retry() invocation
// ---------------------------------------------------------------------------

#[test]
fn policy_resets_between_retry_invocations() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    // First use: exhaust all attempts.
    let result = policy
        .retry(|_| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));

    // Second use: should work again with fresh state (reset happened).
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail again")
        })
        .sleep(instant_sleep)
        .call();
    assert_eq!(call_count.get(), MAX_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn hooks_are_per_call_and_do_not_persist_across_retries() {
    let before_calls = Cell::new(0_u32);
    let policy = RetryPolicy::new().stop(stop::attempts(2));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .before_attempt(|_state| {
            before_calls.set(before_calls.get().saturating_add(1));
        })
        .sleep(instant_sleep)
        .call();
    assert_eq!(before_calls.get(), 2);

    let _ = policy
        .retry(|_| Err::<i32, _>("fail again"))
        .sleep(instant_sleep)
        .call();
    assert_eq!(
        before_calls.get(),
        2,
        "hook should not carry over to later retry invocations"
    );
}

// ---------------------------------------------------------------------------
// Hook callbacks
// ---------------------------------------------------------------------------

#[test]
fn before_attempt_hook_fires_before_each_attempt() {
    let hook_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .before_attempt(|state| {
            hook_calls.borrow_mut().push(state.attempt);
        })
        .sleep(instant_sleep)
        .call();

    let calls = hook_calls.borrow();
    assert_eq!(*calls, vec![1, 2, 3]);
}

#[test]
fn after_attempt_hook_fires_after_each_attempt() {
    let hook_results: RefCell<Vec<(u32, bool)>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            if n < MAX_ATTEMPTS {
                Err("fail")
            } else {
                Ok(SUCCESS_VALUE)
            }
        })
        .after_attempt(|state: &tenacious::AttemptState<i32, &str>| {
            let is_ok = state.outcome.is_ok();
            hook_results.borrow_mut().push((state.attempt, is_ok));
        })
        .sleep(instant_sleep)
        .call();

    let results = hook_results.borrow();
    assert_eq!(results.len(), MAX_ATTEMPTS as usize);
    // First two are errors, last is success.
    assert_eq!(results[0], (1, false));
    assert_eq!(results[1], (2, false));
    assert_eq!(results[2], (3, true));
}

#[test]
fn after_attempt_receives_next_delay_some_for_retryable_none_for_terminal() {
    let hook_calls: RefCell<Vec<(u32, Option<Duration>)>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .after_attempt(|state: &tenacious::AttemptState<i32, &str>| {
            hook_calls
                .borrow_mut()
                .push((state.attempt, state.next_delay));
        })
        .sleep(instant_sleep)
        .call();

    let calls = hook_calls.borrow();
    // Attempts 1,2 retry (Some), attempt 3 terminal (None).
    assert_eq!(
        *calls,
        vec![
            (1, Some(WAIT_DURATION)),
            (2, Some(WAIT_DURATION)),
            (3, None),
        ]
    );
}

#[test]
fn on_exit_hook_fires_when_stop_triggers() {
    let exit_reason = Cell::new(None);
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .on_exit(|state: &tenacious::ExitState<i32, &str>| {
            exit_reason.set(Some(state.stop_reason));
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(exit_reason.get(), Some(StopReason::Exhausted));
}

#[test]
fn on_exit_hook_fires_on_success() {
    let exit_reason = Cell::new(None);
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .on_exit(|state: &tenacious::ExitState<i32, &str>| {
            exit_reason.set(Some(state.stop_reason));
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(exit_reason.get(), Some(StopReason::Accepted));
}

// ---------------------------------------------------------------------------
// std feature provides default sleep
// ---------------------------------------------------------------------------

#[test]
#[cfg(feature = "std")]
fn std_feature_provides_default_sleep() {
    // When std is active, .call() should work without explicit .sleep().
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(Duration::from_millis(1)));

    let result = policy.retry(|_| Ok::<_, &str>(SUCCESS_VALUE)).call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn retry_with_single_attempt_calls_op_once() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(call_count.get(), 1);
}

#[test]
fn retry_succeeds_on_first_attempt() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Ok::<_, &str>(SUCCESS_VALUE)
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), 1);
}

#[test]
fn exhausted_returned_for_ok_predicate_exhaustion() {
    // When using predicate::ok() and stop fires while Ok values keep failing predicate.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|v: &i32| *v < 0)); // retry while Ok value is negative

    let result = policy
        .retry(|_| Ok::<_, &str>(-1_i32)) // always returns Ok(-1), predicate says retry
        .sleep(instant_sleep)
        .call();

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Ok(-1));
        }
        other => panic!("expected Exhausted, got {:?}", other),
    }
}

#[test]
fn composed_polling_predicate_handles_transient_errors_and_not_ready_values() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .when(
            predicate::error(|error: &&str| *error == "transient")
                | predicate::ok(|value: &i32| *value < SUCCESS_VALUE),
        );

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let next_call = call_count.get().saturating_add(1);
            call_count.set(next_call);
            match next_call {
                1 => Err("transient"),
                2 => Ok(0),
                _ => Ok(SUCCESS_VALUE),
            }
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), 3);
}

#[test]
fn predicate_rejects_err_means_immediate_return() {
    // If predicate says "don't retry this error", return Rejected.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::error(|e: &&str| *e == "retryable"));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fatal")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(call_count.get(), 1);
    match result {
        Err(RetryError::Rejected { last }) => {
            assert_eq!(last, "fatal");
        }
        other => panic!("expected Rejected with last=\"fatal\", got {:?}", other),
    }
}

#[test]
fn custom_elapsed_clock_drives_elapsed_stop_without_std_clock() {
    ELAPSED_CLOCK_MILLIS.store(0, Ordering::Relaxed);

    let policy = RetryPolicy::new().stop(stop::elapsed(CUSTOM_CLOCK_DEADLINE));
    let call_count = Cell::new(0_u32);

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            ELAPSED_CLOCK_MILLIS.fetch_add(CUSTOM_CLOCK_STEP_MILLIS, Ordering::Relaxed);
            Err::<i32, _>("clocked failure")
        })
        .elapsed_clock(elapsed_clock_millis)
        .sleep(instant_sleep)
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}
