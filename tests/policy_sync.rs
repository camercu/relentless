//! Acceptance tests for the `RetryPolicy` builder and synchronous execution path.
//!
//! Each test exercises a single observable contract: builder type-state transitions,
//! execution loop invariants (attempt counting, sleep timing, early exit on predicate
//! rejection), hook firing order, reset across `.retry()` invocations, and the
//! `RetryPolicy::default()` safe-defaults guarantee. The test harness avoids real
//! sleeps by injecting a `VirtualClock`, whose waits advance virtual time
//! instead of blocking.

use core::cell::Cell;
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryPolicy, RetryState, StopReason};
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

/// Test-side clock that records each wait into a `Vec`, so wait-sequence
/// assertions also run under `--no-default-features`, where the library's
/// `VirtualClock` has no `alloc`-gated recorder. (The test binary always
/// links `std`.)
struct RecordingClock {
    now: Cell<Duration>,
    waits: RefCell<Vec<Duration>>,
}

impl RecordingClock {
    fn new() -> Self {
        Self {
            now: Cell::new(Duration::ZERO),
            waits: RefCell::new(Vec::new()),
        }
    }

    fn waits(&self) -> Vec<Duration> {
        self.waits.borrow().clone()
    }

    /// Advances virtual time without recording a wait (simulates op runtime).
    fn advance(&self, dur: Duration) {
        self.now.set(self.now.get().saturating_add(dur));
    }
}

impl relentless::Clock for RecordingClock {
    fn now(&self) -> Duration {
        self.now.get()
    }
}

impl relentless::SyncClock for RecordingClock {
    fn wait(&self, dur: Duration) {
        self.now.set(self.now.get().saturating_add(dur));
        self.waits.borrow_mut().push(dur);
    }
}

// Baseline tests (SPEC 11.1.1): three whole 20 ms attempt steps — 20, 40, 60 —
// against a 50 ms deadline, so the loop stops on the third attempt.
const IDLE_BEFORE_CALL_MILLIS: u64 = 1_000;
const PER_ATTEMPT_MILLIS: u64 = 20;
const BASELINE_DEADLINE: Duration = Duration::from_millis(50);

#[test]
fn new_policy_retries_on_any_error_by_default() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), MAX_ATTEMPTS);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn new_policy_accepts_ok_immediately() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[test]
fn new_policy_has_exponential_wait_by_default() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));
    let clock = RecordingClock::new();

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    let recorded = clock.waits();
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
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), MAX_ATTEMPTS);
}

#[test]
fn wait_builder_configures_wait_strategy() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(WAIT_DURATION));
    let clock = RecordingClock::new();

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    assert_eq!(clock.waits(), vec![WAIT_DURATION]);
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
        .clock(VirtualClock::new())
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
        .clock(VirtualClock::new())
        .call();
    assert_eq!(call_count.get(), 1);
    match result {
        Err(RetryError::Aborted { last }) => {
            assert_eq!(last, "fatal");
        }
        other => panic!("expected Rejected with last=\"fatal\", got {other:?}"),
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
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .clock(VirtualClock::new())
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
        .clock(VirtualClock::new())
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
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), MAX_ATTEMPTS);
}

#[test]
fn sync_retry_type_is_nameable_from_crate_root() {
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let retry = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .clock(VirtualClock::new());
    let typed: relentless::Retry<_, _, _, _, _, (), (), ()> = retry;
    assert_eq!(typed.call(), Ok(SUCCESS_VALUE));
}

#[test]
fn retry_returns_exhausted_when_all_attempts_fail() {
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

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
fn retry_predicate_evaluated_before_stop() {
    // The predicate is consulted before the stop strategy. A non-negative Ok value
    // satisfies the predicate immediately, so the loop exits even though attempts
    // remain — stop::attempts is never reached.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::ok(|v: &i32| *v < 0));

    let result = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .clock(VirtualClock::new())
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
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
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
fn clock_receives_computed_delay() {
    let clock = RecordingClock::new();
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _ = policy.retry(|_| Err::<i32, _>("fail")).clock(&clock).call();

    let durations = clock.waits();
    // The wait is injected between attempts, not after the last one.
    assert_eq!(durations.len(), (MAX_ATTEMPTS - 1) as usize);
    for d in &durations {
        assert_eq!(*d, WAIT_DURATION);
    }
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

#[test]
fn policy_is_clone() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let policy2 = policy.clone();

    let result = policy2
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .clock(VirtualClock::new())
        .call();
    assert_eq!(result, Ok(SUCCESS_VALUE));
}

#[cfg(feature = "alloc")]
#[test]
fn boxed_local_erases_policy_types() {
    // `.boxed_local()` erases stop+wait only; the predicate keeps its concrete
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

/// A boxed default-predicate policy is reusable across operations with
/// different `Ok` types, because the predicate is left generic rather than
/// pinned to one `(T, E)` via `Box<dyn Predicate<T, E>>`.
#[cfg(feature = "alloc")]
#[test]
fn boxed_policy_reuses_across_different_ok_types() {
    let policy: RetryPolicy<Box<dyn relentless::Stop + Send>, Box<dyn relentless::Wait + Send>> =
        RetryPolicy::new()
            .stop(stop::attempts(MAX_ATTEMPTS))
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
        .stop(stop::attempts(MAX_ATTEMPTS))
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
        .boxed_local();

    let result = policy
        .retry(move |_| {
            counter_clone.set(counter_clone.get().saturating_add(1));
            Err::<(), _>("fail")
        })
        .clock(VirtualClock::new())
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
        .boxed();

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .clock(VirtualClock::new())
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
        .clock(VirtualClock::new())
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
        .clock(VirtualClock::new())
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
        .clock(VirtualClock::new())
        .call();
    assert_eq!(before_calls.get(), 2);

    let _ = policy
        .retry(|_| Err::<i32, _>("fail again"))
        .clock(VirtualClock::new())
        .call();
    assert_eq!(
        before_calls.get(),
        2,
        "hook should not carry over to later retry invocations"
    );
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
        .after_attempt(|state: &relentless::AttemptState<Result<i32, &str>>| {
            let is_ok = state.outcome.is_ok();
            hook_results.borrow_mut().push((state.attempt, is_ok));
        })
        .clock(VirtualClock::new())
        .call();

    let results = hook_results.borrow();
    assert_eq!(results.len(), MAX_ATTEMPTS as usize);
    assert_eq!(results[0], (1, false));
    assert_eq!(results[1], (2, false));
    assert_eq!(results[2], (3, true));
}

#[test]
fn on_exit_hook_fires_when_stop_triggers() {
    let exit_reason = Cell::new(None);
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .on_exit(|exit: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            exit_reason.set(Some(exit.stop_reason()));
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(exit_reason.get(), Some(StopReason::Exhausted));
}

#[test]
fn on_exit_hook_fires_on_success() {
    let exit_reason = Cell::new(None);
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|_| Ok::<_, &str>(SUCCESS_VALUE))
        .on_exit(|exit: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            exit_reason.set(Some(exit.stop_reason()));
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(exit_reason.get(), Some(StopReason::Returned));
}

#[test]
#[cfg(feature = "std")]
fn std_feature_provides_default_clock() {
    // Without the `std` feature there is no default clock, so `.call()` requires
    // an explicit `.clock(...)`. With `std` active the default `SystemClock`
    // waits with `std::thread::sleep`, so the chain must compile without `.clock()`.
    const DEFAULT_SLEEP_WAIT: Duration = Duration::from_millis(1);

    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(DEFAULT_SLEEP_WAIT));

    let attempts = Cell::new(0_u32);
    let started = std::time::Instant::now();
    let result = policy
        .retry(|_| {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                Err("fail")
            } else {
                Ok(SUCCESS_VALUE)
            }
        })
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    // The default provider must actually block: thread::sleep guarantees at
    // least the requested duration, so this lower bound is deterministic.
    assert!(started.elapsed() >= DEFAULT_SLEEP_WAIT);
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
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Ok::<_, &str>(SUCCESS_VALUE)
        })
        .clock(VirtualClock::new())
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
        .clock(VirtualClock::new())
        .call();

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Ok(-1));
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

#[test]
fn composed_polling_predicate_handles_transient_errors_and_not_ready_values() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .when(predicate::result(|o: &Result<i32, &str>| {
            matches!(o, Err("transient")) || matches!(o, Ok(v) if *v < SUCCESS_VALUE)
        }));

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
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), 3);
}

#[test]
fn until_ok_retries_through_transient_errors_while_polling() {
    // `.until(ok(is_ready))` retries on everything except a matching `Ok`, so a
    // poll that errors keeps polling rather than surfacing the error. This locks
    // the semantics documented on `.until` (retry on everything except the match)
    // and on `predicate::ok`: under `.until`, `Err` is retried, not terminal.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .until(predicate::ok(|v: &i32| *v >= SUCCESS_VALUE));

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let next_call = call_count.get().saturating_add(1);
            call_count.set(next_call);
            match next_call {
                1 | 2 => Err("transient"),
                _ => Ok(SUCCESS_VALUE),
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(
        call_count.get(),
        3,
        "Err retries; only a matching Ok returns"
    );
}

#[test]
fn predicate_rejects_err_means_immediate_return() {
    // When the predicate does not match an Err, the loop exits immediately with
    // RetryError::Aborted rather than waiting for the stop strategy to fire.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::error(|e: &&str| *e == "retryable"));

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
        other => panic!("expected Rejected with last=\"fatal\", got {other:?}"),
    }
}

/// `.timeout()` stops the loop when elapsed time meets or exceeds the budget,
/// even if the stop strategy would allow more attempts.
#[test]
fn timeout_stops_loop_when_budget_exceeded() {
    // Allow up to MAX_ATTEMPTS+10 attempts but set a tight timeout so the
    // loop exits after the first attempt advances the clock past the deadline.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO));
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("fail")
        })
        .clock(&clock)
        .timeout(CUSTOM_CLOCK_DEADLINE)
        .call();

    // The timeout is tighter than MAX_ATTEMPTS would allow, so only 1 attempt runs.
    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// A timeout set on the policy seeds the builder, so it applies without being
/// repeated at the call site.
#[test]
fn policy_timeout_seeds_builder() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO))
        .timeout(CUSTOM_CLOCK_DEADLINE);
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("fail")
        })
        .clock(&clock)
        .call();

    // The policy timeout is tighter than the stop strategy, so only 1 attempt runs.
    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// The policy's timeout survives strategy combinators applied *after* it (SPEC
/// 5.10): setting `.timeout()` first, then `.stop()`/`.wait()`, must not drop it.
#[test]
fn policy_timeout_survives_later_combinators() {
    let policy = RetryPolicy::new()
        .timeout(CUSTOM_CLOCK_DEADLINE)
        .stop(stop::attempts(MAX_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO));
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("fail")
        })
        .clock(&clock)
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// A builder `.timeout()` replaces the policy's timeout for that call (it does
/// not take the tighter of the two): a loose builder timeout lets the loop run
/// past the policy's tight budget.
#[test]
fn policy_timeout_replaced_by_builder() {
    const LOOSE_BUDGET: Duration = Duration::from_secs(60);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO))
        .timeout(CUSTOM_CLOCK_DEADLINE);
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("fail")
        })
        .clock(&clock)
        .timeout(LOOSE_BUDGET)
        .call();

    // The loose builder timeout wins over the policy's tight one, so the stop
    // strategy bounds the loop instead.
    assert_eq!(call_count.get(), MAX_ATTEMPTS + 10);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// End-to-end: the engine feeds each attempt's post-clamp delay forward as the
/// next attempt's `RetryState::previous_delay`. The unit tests construct
/// `RetryState` by hand; this proves the sync loop actually wires it, without
/// which feedback strategies (decorrelated jitter) silently degrade.
#[test]
fn engine_feeds_previous_delay_forward_sync() {
    struct RecordingWait<'a> {
        seen: &'a RefCell<Vec<Option<Duration>>>,
        delay: Duration,
    }
    impl relentless::Wait for RecordingWait<'_> {
        fn next_wait(&self, state: &RetryState) -> Duration {
            self.seen.borrow_mut().push(state.previous_delay);
            self.delay
        }
    }

    const FEEDBACK_DELAY: Duration = Duration::from_millis(7);
    let seen: RefCell<Vec<Option<Duration>>> = RefCell::new(Vec::new());

    let _ = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(RecordingWait {
            seen: &seen,
            delay: FEEDBACK_DELAY,
        })
        .retry(|_| Err::<i32, &str>("fail"))
        .clock(VirtualClock::new())
        .call();

    // The wait strategy is consulted once per *retry* (not on the terminal
    // attempt, which stops before any wait). The first consultation sees no
    // previous delay; each later one sees the prior (constant) delay fed forward.
    assert_eq!(
        *seen.borrow(),
        vec![None, Some(FEEDBACK_DELAY), Some(FEEDBACK_DELAY)]
    );
}

/// The elapsed clock is read twice per attempt: once for the `RetryState` the
/// operation receives (attempt start) and again after the operation completes,
/// for the stop/wait evaluation. A clock that advances during the operation
/// must therefore show the operation an *earlier* elapsed than the wait
/// strategy sees for the same attempt. This guards the two-snapshot semantics
/// so engine refactors that thread `RetryState` as a unit cannot silently
/// collapse the pre-op and post-op snapshots into one.
#[cfg(feature = "alloc")]
#[test]
fn elapsed_is_snapshotted_separately_for_op_and_wait() {
    use std::rc::Rc;

    struct RecordingWait {
        seen: Rc<RefCell<Vec<Duration>>>,
    }
    impl relentless::Wait for RecordingWait {
        fn next_wait(&self, state: &relentless::RetryState) -> Duration {
            self.seen.borrow_mut().push(state.elapsed);
            Duration::ZERO
        }
    }

    let clock = VirtualClock::new();
    let op_elapsed: Cell<Option<Duration>> = Cell::new(None);
    let wait_elapsed: Rc<RefCell<Vec<Duration>>> = Rc::new(RefCell::new(Vec::new()));

    let _ = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(RecordingWait {
            seen: Rc::clone(&wait_elapsed),
        })
        .retry(|state: RetryState| {
            // Record the elapsed the op sees on the first attempt, then advance
            // the clock so the post-op read differs.
            if op_elapsed.get().is_none() {
                op_elapsed.set(Some(state.elapsed));
            }
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .call();

    let op_saw = op_elapsed.get().expect("op ran at least once");
    let wait_saw = wait_elapsed.borrow()[0];
    assert!(
        wait_saw > op_saw,
        "wait should see post-op elapsed ({wait_saw:?}), later than the op ({op_saw:?})"
    );
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
    .clock(VirtualClock::new())
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
    .clock(VirtualClock::new())
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
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(call_count.get(), 3);
}

/// 16.2, 16.3
#[test]
fn compat_duration_is_core_time_duration() {
    let _: relentless::RetryState =
        relentless::RetryState::for_attempt(1).with_elapsed(core::time::Duration::from_millis(5));
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

/// 7.2.4
#[test]
fn predicate_accepted_before_stop_fires() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(1))
        .when(predicate::ok(|v: &i32| *v < 0));

    let result = policy
        .retry(|_| Ok::<i32, &str>(SUCCESS_VALUE))
        .clock(VirtualClock::new())
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
        .after_attempt(|state: &relentless::AttemptState<Result<i32, &str>>| {
            attempt_nums.borrow_mut().push(state.attempt);
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(*attempt_nums.borrow(), vec![1, 2, 3]);
}

/// §6.4
#[test]
fn sleep_occurs_after_after_attempt_hook_fires() {
    /// Clock that logs each wait into a shared event list.
    struct EventClock<'a> {
        events: &'a RefCell<Vec<&'static str>>,
        now: Cell<Duration>,
    }
    impl relentless::Clock for EventClock<'_> {
        fn now(&self) -> Duration {
            self.now.get()
        }
    }
    impl relentless::SyncClock for EventClock<'_> {
        fn wait(&self, dur: Duration) {
            self.events.borrow_mut().push("sleep");
            self.now.set(self.now.get().saturating_add(dur));
        }
    }

    let events: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(WAIT_DURATION));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .after_attempt(|_: &relentless::AttemptState<Result<i32, &str>>| {
            events.borrow_mut().push("after_attempt");
        })
        .clock(EventClock {
            events: &events,
            now: Cell::new(Duration::ZERO),
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
    let policy = RetryPolicy::new()
        .stop(stop::attempts(100))
        .wait(wait::fixed(Duration::ZERO));
    let clock = VirtualClock::new();

    let (result, stats) = policy
        .retry(|_| {
            clock.advance(Duration::from_millis(10));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .timeout(Duration::from_millis(5))
        .with_stats()
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
}

#[test]
fn custom_elapsed_clock_drives_elapsed_stop_without_std_clock() {
    let policy = RetryPolicy::new().stop(stop::elapsed(CUSTOM_CLOCK_DEADLINE));
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("clocked failure")
        })
        .clock(&clock)
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// 11.1.1 — the elapsed baseline is captured when execution starts (`.call()`),
/// not when the builder is configured. Idle time between configuring the
/// builder and calling it must not consume the elapsed budget.
#[test]
fn elapsed_baseline_starts_at_call_not_builder_construction() {
    let clock = VirtualClock::new();
    let policy = RetryPolicy::new()
        .stop(stop::elapsed(BASELINE_DEADLINE))
        .wait(wait::fixed(Duration::ZERO));
    let execution = policy
        .retry(|_| {
            clock.advance(Duration::from_millis(PER_ATTEMPT_MILLIS));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .with_stats();

    clock.advance(Duration::from_millis(IDLE_BEFORE_CALL_MILLIS));
    let (result, stats) = execution.call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.attempts, 3);
}

/// 5.4
#[test]
fn when_and_until_last_call_wins() {
    // Set .when(any_error()) then override with .until(ok(always_true)).
    // until(ok(f)) retries while ok is false and stops when ok is true.
    // Any Err stops immediately (ok returns false, until negates to true, then
    // we wait — actually with until the logic is: until(ok(f)) retries errors automatically.
    // The last predicate wins: .until() after .when() replaces it.
    let call_count = Cell::new(0_i32);
    let result = RetryPolicy::new()
        .stop(stop::attempts(5))
        .wait(wait::fixed(Duration::ZERO))
        .when(predicate::error(|_e: &&str| false)) // would reject everything (no retries)
        .until(predicate::ok(|v: &i32| *v >= 3)) // last call wins: retry until Ok >= 3
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            Ok::<i32, &str>(n)
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(3));
    assert_eq!(call_count.get(), 3);
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
        .when(predicate::any_error())
        .until(predicate::ok(|_: &i32| true));

    let result = policy
        .retry(|_| Ok::<i32, &str>(SUCCESS_VALUE))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(SUCCESS_VALUE));
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
    let clock = RecordingClock::new();
    let policy = RetryPolicy::new()
        .stop(stop::attempts(10))
        .wait(wait::fixed(Duration::from_millis(100)));

    let result = policy
        .retry(|_| {
            // Advance clock past timeout on first attempt.
            clock.advance(Duration::from_millis(10));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .timeout(Duration::from_millis(5))
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    // The wait must be skipped entirely: the clamped delay is zero.
    assert_eq!(clock.waits(), Vec::new());
}

/// 11.4.2
///
/// Unlike the zero-budget case above, here each attempt leaves budget on the
/// table, so the clock must observe the *clamped* delays — not the wait
/// strategy's raw output.
#[test]
fn timeout_clamps_delay_to_partial_remaining_budget() {
    const TIMEOUT: Duration = Duration::from_millis(100);
    const RAW_DELAY: Duration = Duration::from_millis(80);
    const OP_RUNTIME_MILLIS: u64 = 30;

    let clock = RecordingClock::new();
    let policy = RetryPolicy::new()
        .stop(stop::attempts(10))
        .wait(wait::fixed(RAW_DELAY));

    let result = policy
        .retry(|_| {
            clock.advance(Duration::from_millis(OP_RUNTIME_MILLIS));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .timeout(TIMEOUT)
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    // Attempt 1 leaves elapsed at 30 ms → remaining budget 70 ms, below the
    // raw 80 ms delay, so the wait is clamped to 70 ms. The wait itself
    // consumes that budget (the clock is coherent: waiting advances elapsed),
    // so attempt 2 ends at 130 ms ≥ 100 ms and the loop stops at the deadline
    // with one final attempt (SPEC 11.4).
    assert_eq!(clock.waits(), vec![Duration::from_millis(70)]);
}

/// 11.1.2
#[test]
#[cfg(feature = "std")]
fn timeout_with_std_uses_instant_clock() {
    // Just verify it compiles and runs correctly with the default SystemClock.
    let result = RetryPolicy::new()
        .stop(stop::attempts(1))
        .wait(wait::fixed(Duration::ZERO))
        .retry(|_| Ok::<i32, &str>(SUCCESS_VALUE))
        .timeout(Duration::from_secs(60))
        .call();
    assert_eq!(result, Ok(SUCCESS_VALUE));
}
