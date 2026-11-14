//! Acceptance tests for the `RetryPolicy` builder and synchronous execution path.
//!
//! Each test exercises a single observable contract: builder type-state transitions,
//! execution loop invariants through a policy (attempt counting, sleep timing),
//! reset across `.retry()` invocations, and the `RetryPolicy::default()`
//! safe-defaults guarantee. Classifier semantics live in `when_until.rs`, the
//! default clock in `defaults.rs`. The test harness avoids real
//! sleeps by injecting a `VirtualClock`, whose waits advance virtual time
//! instead of blocking.

mod common;

use common::{ARBITRARY_ATTEMPTS, ARBITRARY_DELAY, ARBITRARY_VALUE};

use common::RecordingClock;
use core::cell::Cell;
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryPolicy, Verdict};
use relentless::{stop, wait};

const DEFAULT_POLICY_MAX_ATTEMPTS: u32 = 3;
const DEFAULT_POLICY_INITIAL_WAIT: Duration = Duration::from_millis(100);
const STORAGE_POLICY_WAIT: Duration = Duration::from_millis(1);

#[test]
fn new_policy_retries_on_any_error_by_default() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn new_policy_accepts_ok_immediately() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let result = policy
        .retry(|_| Ok::<_, &str>(ARBITRARY_VALUE))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

#[test]
fn new_policy_has_exponential_wait_by_default() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));
    let clock = RecordingClock::new();

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    let recorded = clock.waits();
    assert_eq!(recorded.len(), (ARBITRARY_ATTEMPTS - 1) as usize);
    assert_eq!(recorded[0], Duration::from_millis(100));
    assert_eq!(recorded[1], Duration::from_millis(200));
}

#[test]
fn stop_builder_configures_stop_strategy() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
}

#[test]
fn wait_builder_configures_wait_strategy() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(ARBITRARY_DELAY));
    let clock = RecordingClock::new();

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    assert_eq!(clock.waits(), vec![ARBITRARY_DELAY]);
}

#[test]
fn when_builder_configures_classifier() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .when(|e: &&str| *e == "retryable");

    // Retryable error: should retry until exhausted.
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("retryable")
        })
        .clock(VirtualClock::new())
        .call();
    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));

    // Non-retryable error: should NOT retry, returns immediately.
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fatal")
        })
        .clock(VirtualClock::new())
        .call();
    assert_eq!(call_count.get(), 1);
    match result {
        Err(RetryError::Aborted { last }) => {
            assert_eq!(last, "fatal");
        }
        other => panic!("expected Aborted with last=\"fatal\", got {other:?}"),
    }
}

#[test]
fn policy_is_easy_to_store_via_three_type_params() {
    type DbPolicy =
        RetryPolicy<stop::StopAfterAttempts, wait::WaitFixed, relentless::DefaultClassifier>;

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
        .retry(|_| Ok::<_, &str>(ARBITRARY_VALUE))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
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
        .clock(VirtualClock::new())
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count.get(), 2);
}

#[test]
fn retry_succeeds_after_transient_failures() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            if n < ARBITRARY_ATTEMPTS {
                Err("transient")
            } else {
                Ok(ARBITRARY_VALUE)
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
}

#[test]
fn retry_returns_exhausted_when_all_attempts_fail() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let result = policy
        .retry(|_| Err::<i32, _>("always fails"))
        .clock(VirtualClock::new())
        .call();

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Err("always fails"));
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

#[test]
fn retry_with_never_stop_still_returns_on_ok() {
    // stop::never() will never fire, so the only exit is a successful outcome
    // returned by the classifier. Verifies the loop doesn't require a stop condition
    // to terminate when Ok is returned.
    let policy = RetryPolicy::new().stop(stop::never());

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            if n < 3 {
                Err("transient")
            } else {
                Ok(ARBITRARY_VALUE)
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(call_count.get(), 3);
}

#[test]
fn default_policy_uses_exponential_backoff() {
    let policy = RetryPolicy::default();
    let clock = RecordingClock::new();

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    let durations = clock.waits();
    assert_eq!(durations.len(), (DEFAULT_POLICY_MAX_ATTEMPTS - 1) as usize);
    assert_eq!(durations[0], DEFAULT_POLICY_INITIAL_WAIT);
    assert_eq!(durations[1], DEFAULT_POLICY_INITIAL_WAIT.saturating_mul(2),);
}

#[test]
fn exponential_wait_increases_sleep_durations() {
    let clock = RecordingClock::new();
    let initial = Duration::from_millis(10);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(wait::exponential(initial));

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    let durations = clock.waits();
    assert_eq!(durations.len(), 3);
    assert_eq!(durations[0], Duration::from_millis(10));
    assert_eq!(durations[1], Duration::from_millis(20));
    assert_eq!(durations[2], Duration::from_millis(40));
}

#[cfg(feature = "alloc")]
#[test]
fn boxed_local_erases_policy_types() {
    // `.boxed_local()` erases stop+wait only; the classifier keeps its concrete
    // type (here the default `DefaultClassifier`, via the third defaulted param).
    let policy: RetryPolicy<
        Box<dyn relentless::stop::Stop + 'static>,
        Box<dyn relentless::wait::Wait + 'static>,
    > = RetryPolicy::new().boxed_local();
    let result = policy
        .retry(|_| Err::<(), _>("fail"))
        .clock(VirtualClock::new())
        .call();
    assert!(result.is_err());
}

/// A boxed default-classifier policy is reusable across operations with
/// different `Ok` types, because only stop and wait are boxed: the
/// classifier stays generic, not pinned to one outcome type.
#[cfg(feature = "alloc")]
#[test]
fn boxed_policy_reuses_across_different_ok_types() {
    let policy: RetryPolicy<Box<dyn relentless::Stop + Send>, Box<dyn relentless::Wait + Send>> =
        RetryPolicy::new()
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .boxed();

    let s = policy
        .retry(|_| Ok::<String, &str>("v".to_string()))
        .clock(VirtualClock::new())
        .call();
    let u = policy
        .retry(|_| Ok::<(), &str>(()))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(s.unwrap(), "v");
    assert_eq!(u.unwrap(), ());
}

/// `boxed_local` erases types but preserves retry behavior: the loop still
/// runs the configured number of attempts and returns `Exhausted` on failure.
#[cfg(feature = "alloc")]
#[test]
fn boxed_local_runs_full_retry_cycle() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed_local();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(
        call_count.get(),
        ARBITRARY_ATTEMPTS,
        "should run ARBITRARY_ATTEMPTS times"
    );
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// `boxed_local` policy is not `Send` — the trait objects it contains lack the
/// `Send` bound. Verify this at compile time via a `compile_fail` doc test on
/// the method itself; here we verify the positive case: it IS usable on a
/// single thread without any `Send` requirement.
#[cfg(feature = "alloc")]
#[test]
fn boxed_local_is_usable_without_send_bound() {
    // Rc is !Send — if boxed_local required Send, this would fail to compile.
    let counter = std::rc::Rc::new(Cell::new(0_u32));
    let counter_clone = counter.clone();

    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed_local();

    let result = policy
        .retry(move |_| {
            counter_clone.set(counter_clone.get().saturating_add(1));
            Err::<(), _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(counter.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[cfg(feature = "alloc")]
#[test]
fn boxed_policy_erases_strategy_types() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn policy_resets_between_retry_invocations() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    let result = policy
        .retry(|_| Err::<i32, _>("fail"))
        .clock(VirtualClock::new())
        .call();
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));

    // A second `.retry()` call on the same policy must start from attempt 1.
    // If internal state leaked across calls, the counter would already be at
    // ARBITRARY_ATTEMPTS and op would never be invoked.
    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail again")
        })
        .clock(VirtualClock::new())
        .call();
    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn retry_with_single_attempt_calls_op_once() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), 1);
}

#[test]
fn retry_succeeds_on_first_attempt() {
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Ok::<_, &str>(ARBITRARY_VALUE)
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(call_count.get(), 1);
}

#[test]
fn composed_polling_classifier_handles_transient_errors_and_not_ready_values() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .decide(|o: Result<i32, &'static str>| match o {
            Err("transient") => Verdict::Retry(Err("transient")),
            Ok(v) if v < ARBITRARY_VALUE => Verdict::Retry(Ok(v)),
            Ok(v) => Verdict::Return(v),
            Err(e) => Verdict::Abort(e),
        });

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let next_call = call_count.get().saturating_add(1);
            call_count.set(next_call);
            match next_call {
                1 => Err("transient"),
                2 => Ok(0),
                _ => Ok(ARBITRARY_VALUE),
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(call_count.get(), 3);
}

/// §6
#[test]
fn policy_retry_borrows_self_immutably() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(Duration::ZERO));

    let r1 = policy
        .retry(|_| Ok::<i32, &str>(1))
        .clock(VirtualClock::new())
        .call();
    let r2 = policy
        .retry(|_| Ok::<i32, &str>(2))
        .clock(VirtualClock::new())
        .call();
    assert_eq!(r1, Ok(1));
    assert_eq!(r2, Ok(2));
}

/// §6
#[test]
fn shared_policy_reference_across_multiple_threads() {
    use std::sync::Arc;
    use std::thread;

    let policy = Arc::new(
        RetryPolicy::new()
            .stop(stop::attempts(2))
            .wait(wait::fixed(Duration::ZERO)),
    );

    let p1 = Arc::clone(&policy);
    let p2 = Arc::clone(&policy);

    let t1 = thread::spawn(move || {
        p1.retry(|_| Ok::<i32, &str>(1))
            .clock(VirtualClock::new())
            .call()
    });
    let t2 = thread::spawn(move || {
        p2.retry(|_| Ok::<i32, &str>(2))
            .clock(VirtualClock::new())
            .call()
    });

    assert_eq!(t1.join().unwrap(), Ok(1));
    assert_eq!(t2.join().unwrap(), Ok(2));
}

/// 6.4.2
#[test]
fn call_returns_retry_result_type() {
    let result: relentless::RetryResult<i32, &str> = RetryPolicy::new()
        .stop(stop::attempts(1))
        .retry(|_| Err::<i32, &str>("fail"))
        .clock(VirtualClock::new())
        .call();
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// 5.1, 5.2
#[test]
fn new_and_default_produce_same_policy() {
    // Both should retry 3 times on persistent errors. The unparameterized
    // `RetryPolicy` annotation also pins the default type parameters.
    let call_count_new = Cell::new(0_u32);
    let r_new = RetryPolicy::new()
        .retry(|_| {
            call_count_new.set(call_count_new.get().saturating_add(1));
            Err::<i32, &str>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    let call_count_def = Cell::new(0_u32);
    let default_policy: RetryPolicy = RetryPolicy::default();
    let r_def = default_policy
        .retry(|_| {
            call_count_def.set(call_count_def.get().saturating_add(1));
            Err::<i32, &str>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert!(matches!(r_new, Err(RetryError::Exhausted { .. })));
    assert!(matches!(r_def, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count_new.get(), DEFAULT_POLICY_MAX_ATTEMPTS);
    assert_eq!(call_count_def.get(), DEFAULT_POLICY_MAX_ATTEMPTS);
}

/// 5.3
#[test]
fn builder_methods_return_typed_policy() {
    // If builder methods consumed/mutated rather than returning new types, the
    // chained call would fail to compile; executing it pins that the fully
    // chained policy actually runs.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO))
        .when(|_: &&str| true)
        .until(|_: &i32| true);

    let result = policy
        .retry(|_| Ok::<i32, &str>(ARBITRARY_VALUE))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

#[test]
fn default_retry_policy_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<relentless::RetryPolicy>();
}

/// 5.7
#[test]
fn retry_policy_is_clone_when_components_are_clone() {
    use core::time::Duration;
    use relentless::{RetryPolicy, stop, wait};

    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::from_millis(10)));

    let cloned = policy.clone();

    // Both clones produce the same results when used independently.
    let result1 = policy
        .retry(|_| Ok::<i32, &str>(1))
        .clock(VirtualClock::new())
        .call();
    let result2 = cloned
        .retry(|_| Ok::<i32, &str>(1))
        .clock(VirtualClock::new())
        .call();
    assert_eq!(result1, result2);
}
