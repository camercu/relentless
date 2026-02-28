//! Acceptance tests for Phase 5: Policy Builder and Sync Execution (Spec items 5.1–5.10)
//!
//! These tests verify:
//! - RetryPolicy::new() type-state and RetryPolicy::default() safe defaults (5.1)
//! - Builder methods: .stop(), .wait(), .when() (5.2, 5.3)
//! - SyncRetry via .retry(op).call() (5.5, 5.6)
//! - RetryPolicy is Clone when constituents are Clone (5.7)
//! - Reset on each .retry() invocation (5.8)
//! - Hook callbacks: before_attempt, after_attempt, before_sleep, on_exhausted (5.9)
//! - Sleep function requirement (5.10)

use core::cell::Cell;
use core::time::Duration;
use std::cell::RefCell;
use tenacious::{RetryError, RetryPolicy};
use tenacious::{on, stop, wait};

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

/// Sleep duration used to simulate operation runtime.
const OPERATION_RUNTIME: Duration = Duration::from_millis(5);

/// Tight elapsed deadline used to verify operation runtime is counted.
const ELAPSED_DEADLINE: Duration = Duration::from_millis(1);

/// Deadline for conservative before-elapsed stop tests.
const BEFORE_ELAPSED_DEADLINE: Duration = Duration::from_millis(30);

/// Wait used in conservative before-elapsed stop tests.
const BEFORE_ELAPSED_WAIT: Duration = Duration::from_millis(50);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A no-op sleep function that doesn't actually sleep (for fast tests).
fn instant_sleep(_dur: Duration) {}

/// Records each requested sleep duration.
fn recording_sleep(recorder: &RefCell<Vec<Duration>>) -> impl FnMut(Duration) + '_ {
    move |dur| recorder.borrow_mut().push(dur)
}

// ---------------------------------------------------------------------------
// 5.1: RetryPolicy::new() type-state + RetryPolicy::default() safe policy
// ---------------------------------------------------------------------------

#[test]
fn new_policy_retries_on_any_error_by_default() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|| {
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
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result = policy
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn new_policy_has_zero_wait_by_default() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let sleeps: RefCell<Vec<Duration>> = RefCell::new(Vec::new());

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(recording_sleep(&sleeps))
        .call();

    let recorded = sleeps.borrow();
    assert_eq!(recorded.len(), (MAX_ATTEMPTS - 1) as usize);
    for dur in recorded.iter() {
        assert_eq!(*dur, Duration::ZERO);
    }
}

// ---------------------------------------------------------------------------
// 5.2, 5.3: Builder methods replace type params
// ---------------------------------------------------------------------------

#[test]
fn stop_builder_configures_stop_strategy() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(call_count.get(), MAX_ATTEMPTS);
}

#[test]
fn wait_builder_configures_wait_strategy() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(WAIT_DURATION));
    let sleeps: RefCell<Vec<Duration>> = RefCell::new(Vec::new());

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(recording_sleep(&sleeps))
        .call();

    assert_eq!(*sleeps.borrow(), vec![WAIT_DURATION]);
}

#[test]
fn when_builder_configures_predicate() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::error(|e: &&str| *e == "retryable"));

    // Retryable error: should retry until exhausted.
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|| {
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
        .retry(|| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fatal")
        })
        .sleep(instant_sleep)
        .call();
    assert_eq!(call_count.get(), 1);
    match result {
        Err(RetryError::PredicateRejected {
            error, attempts, ..
        }) => {
            assert_eq!(error, "fatal");
            assert_eq!(attempts, 1);
        }
        other => panic!(
            "expected PredicateRejected with attempts=1, got {:?}",
            other
        ),
    }
}

// ---------------------------------------------------------------------------
// 5.5, 5.6: SyncRetry execution
// ---------------------------------------------------------------------------

#[test]
fn retry_succeeds_after_transient_failures() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|| {
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
    let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    let retry = policy
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep);
    let _typed: tenacious::SyncRetry<'_, _, _, _, _, _, _, _, _, _, i32, &str> = retry;
}

#[test]
fn retry_returns_exhausted_when_all_attempts_fail() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result = policy
        .retry(|| Err::<i32, _>("always fails"))
        .sleep(instant_sleep)
        .call();

    match result {
        Err(RetryError::Exhausted {
            error, attempts, ..
        }) => {
            assert_eq!(error, "always fails");
            assert_eq!(attempts, MAX_ATTEMPTS);
        }
        other => panic!("expected Exhausted, got {:?}", other),
    }
}

#[test]
fn retry_predicate_evaluated_before_stop() {
    // Spec 5.6 step 2: predicate is checked before stop.
    // If predicate says "accept this outcome", return immediately even if
    // stop hasn't fired yet.
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::ok(|v: &i32| *v < 0)); // retry on negative Ok values

    let result = policy
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE)) // positive => predicate accepts
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn retry_with_never_stop_still_returns_on_ok() {
    // stop::never() means never stop, but predicate still accepts Ok.
    let mut policy = RetryPolicy::new().stop(stop::never());

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|| {
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
    let mut policy = RetryPolicy::default();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|| {
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
    let mut policy: RetryPolicy = RetryPolicy::default();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|| {
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
    let mut policy = RetryPolicy::default();
    let sleeps: RefCell<Vec<Duration>> = RefCell::new(Vec::new());

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(recording_sleep(&sleeps))
        .call();

    let durations = sleeps.borrow();
    assert_eq!(durations.len(), (DEFAULT_POLICY_MAX_ATTEMPTS - 1) as usize);
    assert_eq!(durations[0], DEFAULT_POLICY_INITIAL_WAIT);
    assert_eq!(durations[1], DEFAULT_POLICY_INITIAL_WAIT.saturating_mul(2),);
}

// ---------------------------------------------------------------------------
// 5.6: Execution loop details — sleep is called with computed delay
// ---------------------------------------------------------------------------

#[test]
fn sleep_function_receives_computed_delay() {
    let sleep_durations: RefCell<Vec<Duration>> = RefCell::new(Vec::new());
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
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
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(wait::exponential(initial));

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(|dur| sleep_durations.borrow_mut().push(dur))
        .call();

    let durations = sleep_durations.borrow();
    assert_eq!(durations.len(), 3); // 4 attempts = 3 sleeps
    assert_eq!(durations[0], Duration::from_millis(10)); // 10ms * 2^0
    assert_eq!(durations[1], Duration::from_millis(20)); // 10ms * 2^1
    assert_eq!(durations[2], Duration::from_millis(40)); // 10ms * 2^2
}

// ---------------------------------------------------------------------------
// 5.7: RetryPolicy is Clone
// ---------------------------------------------------------------------------

#[test]
fn policy_is_clone() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let mut policy2 = policy.clone();

    let result = policy2
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[cfg(feature = "alloc")]
#[test]
fn boxed_policy_erases_strategy_types() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed::<i32, &str>();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(call_count.get(), MAX_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

// ---------------------------------------------------------------------------
// 5.8: Reset on each .retry() invocation
// ---------------------------------------------------------------------------

#[test]
fn policy_resets_between_retry_invocations() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    // First use: exhaust all attempts.
    let result = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();
    assert!(matches!(
        result,
        Err(RetryError::Exhausted { attempts: 3, .. })
    ));

    // Second use: should work again with fresh state (reset happened).
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail again")
        })
        .sleep(instant_sleep)
        .call();
    assert_eq!(call_count.get(), MAX_ATTEMPTS);
    assert!(matches!(
        result,
        Err(RetryError::Exhausted { attempts: 3, .. })
    ));
}

// ---------------------------------------------------------------------------
// 5.9: Hook callbacks
// ---------------------------------------------------------------------------

#[test]
fn before_attempt_hook_fires_before_each_attempt() {
    let hook_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .before_attempt(|state| {
            hook_calls.borrow_mut().push(state.attempt);
        });

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();

    let calls = hook_calls.borrow();
    assert_eq!(*calls, vec![1, 2, 3]);
}

#[test]
fn after_attempt_hook_fires_after_each_attempt() {
    let hook_results: RefCell<Vec<(u32, bool)>> = RefCell::new(Vec::new());
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .after_attempt(|state: &tenacious::AttemptState<i32, &str>| {
            let is_ok = state.outcome.is_ok();
            hook_results.borrow_mut().push((state.attempt, is_ok));
        });

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|| {
            let n = call_count.get() + 1;
            call_count.set(n);
            if n < MAX_ATTEMPTS {
                Err("fail")
            } else {
                Ok(SUCCESS_VALUE)
            }
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
fn before_sleep_hook_fires_before_each_sleep() {
    let hook_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION))
        .before_sleep(|state: &tenacious::AttemptState<i32, &str>| {
            hook_calls.borrow_mut().push(state.attempt);
        });

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();

    let calls = hook_calls.borrow();
    // 3 attempts = 2 sleeps = 2 before_sleep calls.
    assert_eq!(*calls, vec![1, 2]);
}

#[test]
fn on_exhausted_hook_fires_when_stop_triggers() {
    let exhausted_called = Cell::new(false);
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .on_exhausted(|_state: &tenacious::AttemptState<i32, &str>| {
            exhausted_called.set(true);
        });

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();

    assert!(exhausted_called.get());
}

#[test]
fn on_exhausted_hook_does_not_fire_on_success() {
    let exhausted_called = Cell::new(false);
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .on_exhausted(|_state: &tenacious::AttemptState<i32, &str>| {
            exhausted_called.set(true);
        });

    let _ = policy
        .retry(|| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();

    assert!(!exhausted_called.get());
}

// ---------------------------------------------------------------------------
// 5.10: std feature provides default sleep
// ---------------------------------------------------------------------------

#[test]
fn std_feature_provides_default_sleep() {
    // When std is active, .call() should work without explicit .sleep().
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(Duration::from_millis(1)));

    let result = policy.retry(|| Ok::<_, &str>(SUCCESS_VALUE)).call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn retry_with_single_attempt_calls_op_once() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(1));

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(call_count.get(), 1);
}

#[test]
fn retry_succeeds_on_first_attempt() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|| {
            call_count.set(call_count.get() + 1);
            Ok::<_, &str>(SUCCESS_VALUE)
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), 1);
}

#[test]
fn condition_not_met_returned_for_ok_predicate_exhaustion() {
    // When using on::ok() and stop fires while Ok values keep failing predicate.
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::ok(|v: &i32| *v < 0)); // retry while Ok value is negative

    let result = policy
        .retry(|| Ok::<_, &str>(-1_i32)) // always returns Ok(-1), predicate says retry
        .sleep(instant_sleep)
        .call();

    match result {
        Err(RetryError::ConditionNotMet { last, attempts, .. }) => {
            assert_eq!(last, -1);
            assert_eq!(attempts, MAX_ATTEMPTS);
        }
        other => panic!("expected ConditionNotMet, got {:?}", other),
    }
}

#[test]
fn predicate_rejects_err_means_immediate_return() {
    // If predicate says "don't retry this error", return PredicateRejected with attempts=1.
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::error(|e: &&str| *e == "retryable"));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fatal")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(call_count.get(), 1);
    match result {
        Err(RetryError::PredicateRejected {
            error, attempts, ..
        }) => {
            assert_eq!(error, "fatal");
            assert_eq!(attempts, 1);
        }
        other => panic!(
            "expected PredicateRejected with attempts=1, got {:?}",
            other
        ),
    }
}

#[test]
fn elapsed_stop_counts_operation_runtime() {
    let mut policy = RetryPolicy::new().stop(stop::elapsed(ELAPSED_DEADLINE));
    let sleeps: RefCell<Vec<Duration>> = RefCell::new(Vec::new());
    let call_count = Cell::new(0_u32);

    let result = policy
        .retry(|| {
            call_count.set(call_count.get().saturating_add(1));
            std::thread::sleep(OPERATION_RUNTIME);
            Err::<i32, _>("slow failure")
        })
        .sleep(recording_sleep(&sleeps))
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(sleeps.borrow().is_empty());
    assert!(matches!(
        result,
        Err(RetryError::Exhausted { attempts: 1, .. })
    ));
}

#[test]
fn before_elapsed_uses_computed_next_delay_before_sleeping() {
    let mut policy = RetryPolicy::new()
        .stop(stop::before_elapsed(BEFORE_ELAPSED_DEADLINE))
        .wait(wait::fixed(BEFORE_ELAPSED_WAIT));
    let sleeps: RefCell<Vec<Duration>> = RefCell::new(Vec::new());
    let call_count = Cell::new(0_u32);

    let result = policy
        .retry(|| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("would exceed budget")
        })
        .sleep(recording_sleep(&sleeps))
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(sleeps.borrow().is_empty());
    assert!(matches!(
        result,
        Err(RetryError::Exhausted { attempts: 1, .. })
    ));
}
