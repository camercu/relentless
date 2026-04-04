//! Acceptance tests for the `RetryPolicy` builder and synchronous execution path.
//!
//! Each test exercises a single observable contract: builder type-state transitions,
//! execution loop invariants (attempt counting, sleep timing, early exit on predicate
//! rejection), hook firing order, reset across `.retry()` invocations, and the
//! `RetryPolicy::default()` safe-defaults guarantee. The test harness avoids real
//! sleeps by supplying no-op or recording sleep functions.

use core::cell::Cell;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use relentless::{RetryError, RetryPolicy, StopReason};
use relentless::{predicate, stop, wait};
use std::cell::RefCell;

const MAX_ATTEMPTS: u32 = 3;
const DEFAULT_POLICY_MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(10);
const DEFAULT_POLICY_INITIAL_WAIT: Duration = Duration::from_millis(100);
const SUCCESS_VALUE: i32 = 42;
/// Deadline shorter than `CUSTOM_CLOCK_STEP_MILLIS` so the first attempt always exhausts it.
const CUSTOM_CLOCK_DEADLINE: Duration = Duration::from_millis(5);
const CUSTOM_CLOCK_STEP_MILLIS: u64 = 10;
const STORAGE_POLICY_WAIT: Duration = Duration::from_millis(1);

// Helpers

fn instant_sleep(_dur: Duration) {}

/// Returns a closure that appends each sleep duration to `recorder`.
/// Used to assert that the wait strategy produces the expected delay sequence.
fn recording_sleep(recorder: &RefCell<Vec<Duration>>) -> impl FnMut(Duration) + '_ {
    move |dur| recorder.borrow_mut().push(dur)
}

static ELAPSED_CLOCK_MILLIS: AtomicU64 = AtomicU64::new(0);

fn elapsed_clock_millis() -> Duration {
    Duration::from_millis(ELAPSED_CLOCK_MILLIS.load(Ordering::Relaxed))
}

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
    assert_eq!(recorded[0], Duration::from_millis(100));
    assert_eq!(recorded[1], Duration::from_millis(200));
}

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
    let typed: relentless::SyncRetry<'_, _, _, _, _, _, _, _, _, i32, &str> = retry;
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
    // The predicate is consulted before the stop strategy. A non-negative Ok value
    // satisfies the predicate immediately, so the loop exits even though attempts
    // remain — stop::attempts is never reached.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|v: &i32| *v < 0));

    let result = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn retry_with_never_stop_still_returns_on_ok() {
    // stop::never() will never fire, so the only exit is a successful outcome
    // accepted by the predicate. Verifies the loop doesn't require a stop condition
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
    // Sleep is injected between attempts, not after the last one.
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
    assert_eq!(durations.len(), 3);
    assert_eq!(durations[0], Duration::from_millis(10));
    assert_eq!(durations[1], Duration::from_millis(20));
    assert_eq!(durations[2], Duration::from_millis(40));
}

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
fn boxed_local_erases_policy_types() {
    #[allow(clippy::type_complexity)]
    let policy: RetryPolicy<
        Box<dyn relentless::stop::Stop + 'static>,
        Box<dyn relentless::wait::Wait + 'static>,
        Box<dyn relentless::predicate::Predicate<(), &str> + 'static>,
    > = RetryPolicy::new().boxed_local::<(), &str>();
    let result = policy.retry(|_| Err::<(), _>("fail")).sleep(|_| {}).call();
    assert!(result.is_err());
}

/// `boxed_local` erases types but preserves retry behavior: the loop still
/// runs the configured number of attempts and returns `Exhausted` on failure.
#[cfg(feature = "alloc")]
#[test]
fn boxed_local_runs_full_retry_cycle() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed_local::<i32, &str>();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(
        call_count.get(),
        MAX_ATTEMPTS,
        "should run MAX_ATTEMPTS times"
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
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed_local::<(), &str>();

    let result = policy
        .retry(move |_| {
            counter_clone.set(counter_clone.get().saturating_add(1));
            Err::<(), _>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(counter.get(), MAX_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
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

#[test]
fn policy_resets_between_retry_invocations() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    let result = policy
        .retry(|_| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));

    // A second `.retry()` call on the same policy must start from attempt 1.
    // If internal state leaked across calls, the counter would already be at
    // MAX_ATTEMPTS and op would never be invoked.
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
        .after_attempt(|state: &relentless::AttemptState<i32, &str>| {
            let is_ok = state.outcome.is_ok();
            hook_results.borrow_mut().push((state.attempt, is_ok));
        })
        .sleep(instant_sleep)
        .call();

    let results = hook_results.borrow();
    assert_eq!(results.len(), MAX_ATTEMPTS as usize);
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
        .after_attempt(|state: &relentless::AttemptState<i32, &str>| {
            hook_calls
                .borrow_mut()
                .push((state.attempt, state.next_delay));
        })
        .sleep(instant_sleep)
        .call();

    let calls = hook_calls.borrow();
    // `next_delay` is Some while the loop will continue, None on the terminal attempt.
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
        .on_exit(|state: &relentless::ExitState<i32, &str>| {
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
        .on_exit(|state: &relentless::ExitState<i32, &str>| {
            exit_reason.set(Some(state.stop_reason));
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(exit_reason.get(), Some(StopReason::Accepted));
}

#[test]
#[cfg(feature = "std")]
fn std_feature_provides_default_sleep() {
    // Without the `std` feature there is no default sleep provider, so `.call()` requires
    // an explicit `.sleep(fn)`. With `std` active the implicit provider uses
    // `std::thread::sleep`, so the builder chain must compile without `.sleep()`.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(Duration::from_millis(1)));

    let result = policy.retry(|_| Ok::<_, &str>(SUCCESS_VALUE)).call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
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
    // predicate::ok retries while the Ok value satisfies the condition. When stop
    // fires with an Ok value still being rejected, the error variant must be
    // Exhausted with last=Ok(_), not Rejected (which is reserved for Err outcomes).
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|v: &i32| *v < 0));

    let result = policy
        .retry(|_| Ok::<_, &str>(-1_i32))
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
    // When the predicate does not match an Err, the loop exits immediately with
    // RetryError::Rejected rather than waiting for the stop strategy to fire.
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

/// `.timeout()` stops the loop when elapsed time meets or exceeds the budget,
/// even if the stop strategy would allow more attempts.
#[test]
fn timeout_stops_loop_when_budget_exceeded() {
    ELAPSED_CLOCK_MILLIS.store(0, Ordering::Relaxed);

    // Allow up to MAX_ATTEMPTS+10 attempts but set a tight timeout so the
    // loop exits after the first attempt advances the clock past the deadline.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO));
    let call_count = Cell::new(0_u32);

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            ELAPSED_CLOCK_MILLIS.fetch_add(CUSTOM_CLOCK_STEP_MILLIS, Ordering::Relaxed);
            Err::<i32, _>("fail")
        })
        .elapsed_clock(elapsed_clock_millis)
        .timeout(CUSTOM_CLOCK_DEADLINE)
        .sleep(instant_sleep)
        .call();

    // The timeout is tighter than MAX_ATTEMPTS would allow, so only 1 attempt runs.
    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
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
    .sleep(instant_sleep)
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
    .sleep(instant_sleep)
    .call();

    assert_eq!(*states_seen.borrow(), vec![1, 2, 3]);
}

/// 6.2.2
#[test]
fn retry_ext_closure_takes_no_retry_state() {
    use relentless::RetryExt;
    use std::rc::Rc;

    // The closure passed to .retry() via RetryExt must have zero args.
    let call_count = Rc::new(Cell::new(0_u32));
    let call_count_clone = call_count.clone();
    let result = (move || {
        call_count_clone.set(call_count_clone.get().saturating_add(1));
        if call_count_clone.get() < 3 {
            Err("transient")
        } else {
            Ok(SUCCESS_VALUE)
        }
    })
    .retry()
    .stop(stop::attempts(3))
    .wait(wait::fixed(Duration::ZERO))
    .sleep(instant_sleep)
    .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), 3);
}

/// 16.2, 16.3
#[test]
fn compat_duration_is_core_time_duration() {
    let _: relentless::RetryState =
        relentless::RetryState::new(1, Some(core::time::Duration::from_millis(5)));
}

/// §6
#[test]
fn policy_retry_borrows_self_immutably() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(Duration::ZERO));

    let r1 = policy
        .retry(|_| Ok::<i32, &str>(1))
        .sleep(instant_sleep)
        .call();
    let r2 = policy
        .retry(|_| Ok::<i32, &str>(2))
        .sleep(instant_sleep)
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

    let t1 = thread::spawn(move || p1.retry(|_| Ok::<i32, &str>(1)).sleep(|_| {}).call());
    let t2 = thread::spawn(move || p2.retry(|_| Ok::<i32, &str>(2)).sleep(|_| {}).call());

    assert_eq!(t1.join().unwrap(), Ok(1));
    assert_eq!(t2.join().unwrap(), Ok(2));
}

/// 6.4.2
#[test]
fn call_returns_retry_result_type() {
    let result: relentless::RetryResult<i32, &str> = RetryPolicy::new()
        .stop(stop::attempts(1))
        .retry(|_| Err::<i32, &str>("fail"))
        .sleep(instant_sleep)
        .call();
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// 7.2.4
#[test]
fn predicate_accepted_before_stop_fires() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(1))
        .when(predicate::ok(|v: &i32| *v < 0));

    let result = policy
        .retry(|_| Ok::<i32, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

/// 7.2.1
#[test]
fn after_attempt_fires_including_final_attempt() {
    let attempt_nums: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .after_attempt(|state: &relentless::AttemptState<i32, &str>| {
            attempt_nums.borrow_mut().push(state.attempt);
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(*attempt_nums.borrow(), vec![1, 2, 3]);
}

/// 7.2.2, 7.2.3
#[test]
fn after_attempt_next_delay_some_then_none() {
    let next_delays: RefCell<Vec<Option<Duration>>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .after_attempt(|state: &relentless::AttemptState<i32, &str>| {
            next_delays.borrow_mut().push(state.next_delay);
        })
        .sleep(instant_sleep)
        .call();

    let delays = next_delays.borrow();
    assert_eq!(delays[0], Some(WAIT_DURATION));
    assert_eq!(delays[1], Some(WAIT_DURATION));
    assert_eq!(delays[2], None);
}

/// 8.3
#[test]
fn on_exit_fires_exactly_once_per_execution() {
    let exit_count = Cell::new(0_u32);
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .on_exit(|_: &relentless::ExitState<i32, &str>| {
            exit_count.set(exit_count.get().saturating_add(1));
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(exit_count.get(), 1);
}

/// §6.4
#[test]
fn sleep_occurs_after_after_attempt_hook_fires() {
    let events: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(WAIT_DURATION));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .after_attempt(|_: &relentless::AttemptState<i32, &str>| {
            events.borrow_mut().push("after_attempt");
        })
        .sleep(|_| {
            events.borrow_mut().push("sleep");
        })
        .call();

    // after_attempt fires, then sleep fires, then final after_attempt (no sleep after terminal).
    assert_eq!(
        *events.borrow(),
        vec!["after_attempt", "sleep", "after_attempt"]
    );
}

/// 11.4.4
#[test]
fn timeout_stop_reason_is_exhausted() {
    ELAPSED_CLOCK_MILLIS.store(0, Ordering::Relaxed);

    let policy = RetryPolicy::new()
        .stop(stop::attempts(100))
        .wait(wait::fixed(Duration::ZERO));

    let (result, stats) = policy
        .retry(|_| {
            ELAPSED_CLOCK_MILLIS.fetch_add(10, Ordering::Relaxed);
            Err::<i32, &str>("fail")
        })
        .elapsed_clock(elapsed_clock_millis)
        .timeout(Duration::from_millis(5))
        .sleep(instant_sleep)
        .with_stats()
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
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

/// 5.4
#[test]
fn when_and_until_last_call_wins() {
    // Set .when(any_error()) then override with .until(ok(always_true)).
    // until(ok(f)) retries while ok is false and stops when ok is true.
    // Any Err stops immediately (ok returns false, until negates to true, then
    // we wait — actually with until the logic is: until(ok(f)) retries errors automatically.
    // The last predicate wins: .until() after .when() replaces it.
    let call_count = Cell::new(0_u32);
    let result = RetryPolicy::new()
        .stop(stop::attempts(5))
        .wait(wait::fixed(Duration::ZERO))
        .when(predicate::error(|_e: &&str| false)) // would reject everything (no retries)
        .until(predicate::ok(|v: &i32| *v >= 3)) // last call wins: retry until Ok >= 3
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            Ok::<i32, &str>(n as i32)
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(result, Ok(3));
    assert_eq!(call_count.get(), 3);
}

/// 5.1, 5.2
#[test]
fn new_and_default_produce_same_policy() {
    // Both should retry 3 times on persistent errors.
    let call_count_new = Cell::new(0_u32);
    let r_new = RetryPolicy::new()
        .retry(|_| {
            call_count_new.set(call_count_new.get().saturating_add(1));
            Err::<i32, &str>("fail")
        })
        .sleep(instant_sleep)
        .call();

    let call_count_def = Cell::new(0_u32);
    let r_def = RetryPolicy::default()
        .retry(|_| {
            call_count_def.set(call_count_def.get().saturating_add(1));
            Err::<i32, &str>("fail")
        })
        .sleep(instant_sleep)
        .call();

    assert!(matches!(r_new, Err(RetryError::Exhausted { .. })));
    assert!(matches!(r_def, Err(RetryError::Exhausted { .. })));
    assert_eq!(call_count_new.get(), 3);
    assert_eq!(call_count_def.get(), 3);
}

/// 5.3
#[test]
fn builder_methods_return_typed_policy() {
    let _p = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO))
        .when(predicate::any_error())
        .until(predicate::ok(|_: &i32| true));
    // If builder methods consumed/mutated rather than returning new types, the
    // chained call would fail to compile. The fact that it compiles is the test.
}

/// 3.5.2
#[test]
#[cfg(feature = "std")]
fn std_sync_retry_uses_thread_sleep_by_default() {
    let result = RetryPolicy::new()
        .stop(stop::attempts(1))
        .wait(wait::fixed(Duration::ZERO))
        .retry(|_| Ok::<i32, &str>(SUCCESS_VALUE))
        .call();
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

/// 11.4.1, 11.4.2, 11.4.3
#[test]
fn timeout_clamps_delay_to_remaining_budget() {
    ELAPSED_CLOCK_MILLIS.store(0, Ordering::Relaxed);

    let sleep_calls = Cell::new(0_u32);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(10))
        .wait(wait::fixed(Duration::from_millis(100)));

    let result = policy
        .retry(|_| {
            // Advance clock past timeout on first attempt.
            ELAPSED_CLOCK_MILLIS.fetch_add(10, Ordering::Relaxed);
            Err::<i32, &str>("fail")
        })
        .elapsed_clock(elapsed_clock_millis)
        .timeout(Duration::from_millis(5))
        .sleep(|_dur| {
            sleep_calls.set(sleep_calls.get().saturating_add(1));
        })
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    // Sleep should be skipped since the clamped delay is zero.
    assert_eq!(sleep_calls.get(), 0);
}

/// 11.1.2
#[test]
#[cfg(feature = "std")]
fn timeout_with_std_uses_instant_clock() {
    // Just verify it compiles and runs correctly without an explicit elapsed_clock.
    let result = RetryPolicy::new()
        .stop(stop::attempts(1))
        .wait(wait::fixed(Duration::ZERO))
        .retry(|_| Ok::<i32, &str>(SUCCESS_VALUE))
        .timeout(Duration::from_secs(60))
        .call();
    assert_eq!(result, Ok(SUCCESS_VALUE));
}
