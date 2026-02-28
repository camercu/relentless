//! Acceptance tests for Phase 1: Core Types and Traits (Spec items 1.1–1.11)
//!
//! These tests verify:
//! - Stop trait definition and semantics (1.2)
//! - Wait trait definition and semantics (1.3)
//! - Predicate trait definition and semantics (1.4)
//! - Sleeper trait and blanket impl (1.5, 1.6)
//! - RetryState, AttemptState, and BeforeAttemptState structs (1.7, 1.8)
//! - RetryError enum and Display/Error impls (1.9, 1.10)
//! - Duration is core::time::Duration (1.11)

use core::time::Duration;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tenacious::Predicate;
use tenacious::Sleeper;
use tenacious::Stop;
use tenacious::Wait;

// ---------------------------------------------------------------------------
// Test constants
// ---------------------------------------------------------------------------

/// The maximum attempts threshold used in stop-strategy tests.
const STOP_AFTER_MAX_ATTEMPTS: u32 = 3;

/// Per-call increment for the counting wait strategy.
const WAIT_INCREMENT_MILLIS: u64 = 100;

/// Values that are genuinely arbitrary — any valid value would work.
/// These signal "the specific value doesn't matter" to the reader.
const ARBITRARY_DURATION: Duration = Duration::from_millis(10);
const ARBITRARY_ATTEMPT_COUNT: u32 = 5;

// ---------------------------------------------------------------------------
// 1.2: Stop trait — should_stop and reset methods
// ---------------------------------------------------------------------------

/// A trivial Stop implementation that stops after a fixed number of attempts.
struct StopAfter {
    max: u32,
}

impl Stop for StopAfter {
    fn should_stop(&mut self, state: &tenacious::RetryState) -> bool {
        state.attempt >= self.max
    }
}

#[test]
fn stop_trait_should_stop_returns_bool() {
    let mut stop = StopAfter {
        max: STOP_AFTER_MAX_ATTEMPTS,
    };

    let state = make_retry_state(1);
    assert!(
        !stop.should_stop(&state),
        "attempt 1 < max, should not stop"
    );

    let state = make_retry_state(STOP_AFTER_MAX_ATTEMPTS);
    assert!(stop.should_stop(&state), "attempt == max, should stop");
}

/// Verify that the default reset() implementation exists and can be called.
#[test]
fn stop_trait_reset_has_default_impl() {
    let mut stop = StopAfter {
        max: STOP_AFTER_MAX_ATTEMPTS,
    };
    stop.reset();
}

/// A Stop impl that overrides reset to verify custom reset works.
struct ResettableStop {
    fired: bool,
}

impl Stop for ResettableStop {
    fn should_stop(&mut self, _state: &tenacious::RetryState) -> bool {
        self.fired = true;
        true
    }

    fn reset(&mut self) {
        self.fired = false;
    }
}

#[test]
fn stop_trait_custom_reset() {
    let mut stop = ResettableStop { fired: false };
    let state = make_retry_state(1);
    stop.should_stop(&state);
    assert!(stop.fired);
    stop.reset();
    assert!(!stop.fired);
}

/// Verify Stop::should_stop takes &mut self — a stateful stop strategy that
/// counts calls internally and stops after a threshold.
struct CountingStop {
    calls: u32,
    threshold: u32,
}

impl Stop for CountingStop {
    fn should_stop(&mut self, _state: &tenacious::RetryState) -> bool {
        self.calls = self.calls.saturating_add(1);
        self.calls >= self.threshold
    }

    fn reset(&mut self) {
        self.calls = 0;
    }
}

#[test]
fn stop_trait_mutates_state_across_calls() {
    let mut stop = CountingStop {
        calls: 0,
        threshold: STOP_AFTER_MAX_ATTEMPTS,
    };
    let state = make_retry_state(1);

    assert!(!stop.should_stop(&state));
    assert!(!stop.should_stop(&state));
    assert!(
        stop.should_stop(&state),
        "should stop after threshold calls"
    );

    stop.reset();
    assert!(
        !stop.should_stop(&state),
        "after reset, count restarts from zero"
    );
}

// ---------------------------------------------------------------------------
// 1.3: Wait trait — next_wait and reset methods
// ---------------------------------------------------------------------------

/// A trivial Wait implementation that returns a fixed duration.
struct FixedWait {
    dur: Duration,
}

impl Wait for FixedWait {
    fn next_wait(&mut self, _state: &tenacious::RetryState) -> Duration {
        self.dur
    }
}

#[test]
fn wait_trait_next_wait_returns_duration() {
    let mut wait = FixedWait {
        dur: ARBITRARY_DURATION,
    };
    let state = make_retry_state(1);
    assert_eq!(wait.next_wait(&state), ARBITRARY_DURATION);
}

#[test]
fn wait_trait_reset_has_default_impl() {
    let mut wait = FixedWait {
        dur: ARBITRARY_DURATION,
    };
    wait.reset();
}

/// A Wait impl that overrides reset — counts calls and produces increasing waits.
struct ResettableWait {
    call_count: u32,
}

impl Wait for ResettableWait {
    fn next_wait(&mut self, _state: &tenacious::RetryState) -> Duration {
        self.call_count += 1;
        Duration::from_millis(self.call_count as u64 * WAIT_INCREMENT_MILLIS)
    }

    fn reset(&mut self) {
        self.call_count = 0;
    }
}

#[test]
fn wait_trait_custom_reset() {
    let first_call_wait = Duration::from_millis(WAIT_INCREMENT_MILLIS);
    let second_call_wait = Duration::from_millis(WAIT_INCREMENT_MILLIS * 2);

    let mut wait = ResettableWait { call_count: 0 };
    let state = make_retry_state(1);

    assert_eq!(wait.next_wait(&state), first_call_wait);
    assert_eq!(wait.next_wait(&state), second_call_wait);

    wait.reset();
    assert_eq!(
        wait.next_wait(&state),
        first_call_wait,
        "reset should restart the sequence"
    );
}

// ---------------------------------------------------------------------------
// 1.2/1.3: Stop and Wait are non-generic (decoupled from T, E)
// ---------------------------------------------------------------------------

/// Stop and Wait accept RetryState (non-generic), so a single strategy
/// works across operations with different Result types.
#[test]
fn stop_and_wait_are_not_generic_over_result_type() {
    let mut stop = StopAfter {
        max: STOP_AFTER_MAX_ATTEMPTS,
    };
    let mut wait = FixedWait {
        dur: ARBITRARY_DURATION,
    };
    let state = make_retry_state(1);

    // The same stop and wait instances work regardless of operation type —
    // no <T, E> parameterization needed. This is a compile-time check.
    let _ = stop.should_stop(&state);
    let _ = wait.next_wait(&state);
}

// ---------------------------------------------------------------------------
// 1.4: Predicate<T, E> trait — should_retry method
// ---------------------------------------------------------------------------

/// A predicate that retries on any error.
struct RetryOnAnyError;

impl Predicate<String, std::io::Error> for RetryOnAnyError {
    fn should_retry(&self, outcome: &Result<String, std::io::Error>) -> bool {
        outcome.is_err()
    }
}

#[test]
fn predicate_trait_should_retry_on_error() {
    let pred = RetryOnAnyError;
    let err_result: Result<String, std::io::Error> = Err(std::io::Error::other("boom"));
    assert!(pred.should_retry(&err_result));
}

#[test]
fn predicate_trait_should_not_retry_on_ok() {
    let pred = RetryOnAnyError;
    let ok_result: Result<String, std::io::Error> = Ok("success".to_string());
    assert!(!pred.should_retry(&ok_result));
}

/// Verify that T and E are type parameters on the trait (not the method).
/// Two different Predicate impls for different (T, E) pairs on the same struct.
struct AlwaysRetry;

impl Predicate<u32, String> for AlwaysRetry {
    fn should_retry(&self, _outcome: &Result<u32, String>) -> bool {
        true
    }
}

impl Predicate<bool, i32> for AlwaysRetry {
    fn should_retry(&self, _outcome: &Result<bool, i32>) -> bool {
        true
    }
}

#[test]
fn predicate_trait_type_params_on_trait() {
    let pred = AlwaysRetry;
    let r1: Result<u32, String> = Ok(42);
    let r2: Result<bool, i32> = Err(-1);

    // Both should compile and work — T, E are on the trait, not the method.
    assert!(<AlwaysRetry as Predicate<u32, String>>::should_retry(
        &pred, &r1
    ));
    assert!(<AlwaysRetry as Predicate<bool, i32>>::should_retry(
        &pred, &r2
    ));
}

/// 4.8: Predicate is blanket-implemented for Fn(&Result<T, E>) -> bool.
#[test]
fn predicate_blanket_impl_for_closure() {
    let pred = |outcome: &Result<i32, &str>| outcome.is_err();

    let err: Result<i32, &str> = Err("fail");
    let ok: Result<i32, &str> = Ok(42);

    assert!(Predicate::should_retry(&pred, &err));
    assert!(!Predicate::should_retry(&pred, &ok));
}

/// Predicate::should_retry takes &self (immutable). Verify it can be called
/// multiple times through a shared reference.
#[test]
fn predicate_is_immutable() {
    let pred = RetryOnAnyError;
    let shared: &dyn Predicate<String, std::io::Error> = &pred;

    let err: Result<String, std::io::Error> = Err(std::io::Error::other("boom"));
    let ok: Result<String, std::io::Error> = Ok("ok".to_string());

    // Multiple calls through a shared reference must work.
    assert!(shared.should_retry(&err));
    assert!(shared.should_retry(&err));
    assert!(!shared.should_retry(&ok));
}

// ---------------------------------------------------------------------------
// 1.5, 1.6: Sleeper trait and blanket impl for Fn(Duration) -> Future
// ---------------------------------------------------------------------------

/// A minimal future that resolves immediately.
struct Immediate;

impl Future for Immediate {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }
}

/// Verify Sleeper trait has the right shape: associated type Sleep and sleep method.
struct ImmediateSleeper;

impl Sleeper for ImmediateSleeper {
    type Sleep = Immediate;
    fn sleep(&self, _dur: Duration) -> Self::Sleep {
        Immediate
    }
}

#[test]
fn sleeper_trait_direct_impl() {
    let sleeper = ImmediateSleeper;
    let _fut = sleeper.sleep(ARBITRARY_DURATION);
}

/// 1.6: Blanket impl — a closure `Fn(Duration) -> Fut` satisfies Sleeper.
#[test]
fn sleeper_blanket_impl_for_closure() {
    let sleeper_fn = |_dur: Duration| Immediate;
    let _fut = Sleeper::sleep(&sleeper_fn, ARBITRARY_DURATION);
}

/// Verify the blanket impl works with a closure returning a different future type.
struct DelayedReady(bool);

impl Future for DelayedReady {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[test]
fn sleeper_blanket_impl_different_future_type() {
    let sleeper_fn = |_dur: Duration| DelayedReady(false);
    let _fut = Sleeper::sleep(&sleeper_fn, Duration::ZERO);
}

// ---------------------------------------------------------------------------
// 1.7, 1.8: RetryState, AttemptState, and BeforeAttemptState structs
// ---------------------------------------------------------------------------

#[test]
fn retry_state_has_required_fields() {
    let elapsed = Duration::from_secs(5);
    let next_delay = Duration::from_millis(200);
    let total_wait = Duration::from_millis(100);

    let state = tenacious::RetryState {
        attempt: 1,
        elapsed: Some(elapsed),
        next_delay,
        total_wait,
    };

    assert_eq!(state.attempt, 1);
    assert_eq!(state.elapsed, Some(elapsed));
    assert_eq!(state.next_delay, next_delay);
    assert_eq!(state.total_wait, total_wait);
}

#[test]
fn retry_state_attempt_is_one_indexed() {
    let state = make_retry_state(1);
    assert_eq!(state.attempt, 1, "first attempt should be 1, not 0");
}

#[test]
fn retry_state_elapsed_can_be_none() {
    let state = tenacious::RetryState {
        attempt: 1,
        elapsed: None,
        next_delay: Duration::ZERO,
        total_wait: Duration::ZERO,
    };
    assert_eq!(state.elapsed, None);
}

#[test]
fn attempt_state_has_flat_fields_and_outcome() {
    let retry_state = make_retry_state(1);
    let outcome: Result<i32, String> = Ok(42);

    let state = tenacious::AttemptState {
        attempt: retry_state.attempt,
        outcome: &outcome,
        elapsed: retry_state.elapsed,
        next_delay: retry_state.next_delay,
        total_wait: retry_state.total_wait,
    };

    assert_eq!(state.attempt, 1);
    assert_eq!(*state.outcome, Ok(42));
    assert_eq!(state.next_delay, Duration::ZERO);
}

#[test]
fn attempt_state_with_err_outcome() {
    let retry_state = make_retry_state(1);
    let outcome: Result<(), String> = Err("network timeout".to_string());

    let state = tenacious::AttemptState {
        attempt: retry_state.attempt,
        outcome: &outcome,
        elapsed: retry_state.elapsed,
        next_delay: retry_state.next_delay,
        total_wait: retry_state.total_wait,
    };

    assert!(state.outcome.is_err());
    assert_eq!(state.outcome.as_ref().unwrap_err(), "network timeout");
}

#[test]
fn before_attempt_state_has_required_fields() {
    let elapsed = Duration::from_secs(1);
    let total_wait = Duration::from_millis(500);

    let state = tenacious::BeforeAttemptState {
        attempt: 2,
        elapsed: Some(elapsed),
        total_wait,
    };

    assert_eq!(state.attempt, 2);
    assert_eq!(state.elapsed, Some(elapsed));
    assert_eq!(state.total_wait, total_wait);
}

#[test]
fn before_attempt_state_does_not_have_outcome() {
    // Compile-time structural check: BeforeAttemptState has only these three fields.
    // If an `outcome` or `next_delay` field were added, this exhaustive destructure
    // would fail to compile.
    let state = tenacious::BeforeAttemptState {
        attempt: 1,
        elapsed: None,
        total_wait: Duration::ZERO,
    };
    let tenacious::BeforeAttemptState {
        attempt: _,
        elapsed: _,
        total_wait: _,
    } = state;
}

// ---------------------------------------------------------------------------
// 1.9, 1.10: RetryError enum
// ---------------------------------------------------------------------------

#[test]
fn retry_error_exhausted_variant() {
    let err: tenacious::RetryError<String> = tenacious::RetryError::Exhausted {
        error: "connection refused".to_string(),
        attempts: ARBITRARY_ATTEMPT_COUNT,
        total_elapsed: Some(ARBITRARY_DURATION),
    };

    match err {
        tenacious::RetryError::Exhausted {
            ref error,
            attempts,
            total_elapsed,
        } => {
            assert_eq!(error, "connection refused");
            assert_eq!(attempts, ARBITRARY_ATTEMPT_COUNT);
            assert_eq!(total_elapsed, Some(ARBITRARY_DURATION));
        }
        _ => panic!("expected Exhausted variant"),
    }
}

#[test]
fn retry_error_predicate_rejected_variant() {
    let err: tenacious::RetryError<String> = tenacious::RetryError::PredicateRejected {
        error: "fatal".to_string(),
        attempts: ARBITRARY_ATTEMPT_COUNT,
        total_elapsed: Some(ARBITRARY_DURATION),
    };

    match err {
        tenacious::RetryError::PredicateRejected {
            ref error,
            attempts,
            total_elapsed,
        } => {
            assert_eq!(error, "fatal");
            assert_eq!(attempts, ARBITRARY_ATTEMPT_COUNT);
            assert_eq!(total_elapsed, Some(ARBITRARY_DURATION));
        }
        _ => panic!("expected PredicateRejected variant"),
    }
}

#[test]
fn retry_error_condition_not_met_variant_carries_last_value() {
    let err: tenacious::RetryError<String, i32> = tenacious::RetryError::ConditionNotMet {
        last: 42,
        attempts: ARBITRARY_ATTEMPT_COUNT,
        total_elapsed: None,
    };

    match err {
        tenacious::RetryError::ConditionNotMet {
            last,
            attempts,
            total_elapsed,
        } => {
            assert_eq!(last, 42);
            assert_eq!(attempts, ARBITRARY_ATTEMPT_COUNT);
            assert_eq!(total_elapsed, None);
        }
        _ => panic!("expected ConditionNotMet variant"),
    }
}

#[test]
fn retry_error_condition_not_met_default_t_is_unit() {
    // When T defaults to (), ConditionNotMet still compiles.
    let err: tenacious::RetryError<String> = tenacious::RetryError::ConditionNotMet {
        last: (),
        attempts: ARBITRARY_ATTEMPT_COUNT,
        total_elapsed: None,
    };
    assert!(matches!(err, tenacious::RetryError::ConditionNotMet { .. }));
}

#[test]
fn retry_error_exhausted_elapsed_can_be_none() {
    let err: tenacious::RetryError<&str> = tenacious::RetryError::Exhausted {
        error: "fail",
        attempts: 1,
        total_elapsed: None,
    };

    if let tenacious::RetryError::Exhausted { total_elapsed, .. } = err {
        assert_eq!(total_elapsed, None);
    }
}

/// 1.10: RetryError implements Display unconditionally.
#[test]
fn retry_error_display_includes_meaningful_content() {
    let err: tenacious::RetryError<String> = tenacious::RetryError::Exhausted {
        error: "timeout".to_string(),
        attempts: ARBITRARY_ATTEMPT_COUNT,
        total_elapsed: Some(ARBITRARY_DURATION),
    };

    let msg = format!("{}", err);
    assert!(
        msg.contains(&ARBITRARY_ATTEMPT_COUNT.to_string()),
        "Display should include the attempt count: {msg}"
    );
    assert!(
        msg.contains("timeout"),
        "Display should include the error message: {msg}"
    );

    let err2: tenacious::RetryError<String, i32> = tenacious::RetryError::ConditionNotMet {
        last: 99,
        attempts: ARBITRARY_ATTEMPT_COUNT,
        total_elapsed: None,
    };
    let msg2 = format!("{}", err2);
    assert!(
        msg2.contains(&ARBITRARY_ATTEMPT_COUNT.to_string()),
        "Display should include the attempt count: {msg2}"
    );
}

/// 1.10: RetryError implements std::error::Error when std is active and E: Error + 'static.
#[test]
fn retry_error_implements_std_error() {
    let inner = std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out");
    let err: tenacious::RetryError<std::io::Error> = tenacious::RetryError::Exhausted {
        error: inner,
        attempts: ARBITRARY_ATTEMPT_COUNT,
        total_elapsed: Some(ARBITRARY_DURATION),
    };

    // Verify it can be used as a dyn Error.
    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_some(),
        "Exhausted should chain to the inner error via source()"
    );
}

/// source() returns None for ConditionNotMet.
#[test]
fn retry_error_condition_not_met_source_is_none() {
    let err: tenacious::RetryError<std::io::Error> = tenacious::RetryError::ConditionNotMet {
        last: (),
        attempts: ARBITRARY_ATTEMPT_COUNT,
        total_elapsed: None,
    };

    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_none(),
        "ConditionNotMet has no source error"
    );
}

/// source() returns Some(inner) for PredicateRejected.
#[test]
fn retry_error_predicate_rejected_source_is_inner_error() {
    let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "fatal");
    let err: tenacious::RetryError<std::io::Error> = tenacious::RetryError::PredicateRejected {
        error: inner,
        attempts: ARBITRARY_ATTEMPT_COUNT,
        total_elapsed: None,
    };

    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_some(),
        "PredicateRejected should chain to the inner error via source()"
    );
}

/// RetryError derives Clone and PartialEq for ergonomic test assertions.
#[test]
fn retry_error_derives_clone_and_partial_eq() {
    let err: tenacious::RetryError<String> = tenacious::RetryError::Exhausted {
        error: "fail".to_string(),
        attempts: 1,
        total_elapsed: None,
    };

    let cloned = err.clone();
    assert_eq!(err, cloned);
}

/// Verify RetryError can be used in a Result context (ergonomics).
#[test]
fn retry_error_in_result_context() {
    fn fallible() -> Result<i32, tenacious::RetryError<String>> {
        Err(tenacious::RetryError::Exhausted {
            error: "fail".to_string(),
            attempts: 1,
            total_elapsed: None,
        })
    }

    assert!(fallible().is_err());
}

// ---------------------------------------------------------------------------
// 1.11: Duration is core::time::Duration
// ---------------------------------------------------------------------------

#[test]
fn duration_is_core_time_duration() {
    // AttemptState uses Duration — verify it's the standard core::time::Duration.
    let d: core::time::Duration = ARBITRARY_DURATION;
    let state = tenacious::RetryState {
        attempt: 1,
        elapsed: None,
        next_delay: d,
        total_wait: Duration::ZERO,
    };
    assert_eq!(state.next_delay, ARBITRARY_DURATION);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Helper to construct a RetryState with default zero/none fields.
///
/// NOTE: The spec says state types are "never constructed by user code" (1.8).
/// These helpers simulate what the execution engine would do.
fn make_retry_state(attempt: u32) -> tenacious::RetryState {
    tenacious::RetryState {
        attempt,
        elapsed: None,
        next_delay: Duration::ZERO,
        total_wait: Duration::ZERO,
    }
}
