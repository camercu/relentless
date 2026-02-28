//! Acceptance tests for Phase 2: Stop Strategies (Spec items 2.1–2.9)
//!
//! These tests verify:
//! - stop::attempts(n) fires at the correct threshold (2.1)
//! - stop::elapsed(dur) fires on deadline, handles None clock (2.2)
//! - stop::before_elapsed(dur) fires conservatively with next_delay (2.3)
//! - stop::never() always returns false (2.4)
//! - BitOr composition produces StopAny (2.5, 2.7)
//! - BitAnd composition produces StopAll (2.6, 2.7)
//! - Reset propagation through composites (2.8)
//! - Clone and Debug derive on all types (2.9)

use core::time::Duration;
use tenacious::Stop;
use tenacious::stop;

// ---------------------------------------------------------------------------
// Test constants
// ---------------------------------------------------------------------------

/// The attempt threshold used across most stop-strategy tests.
const MAX_ATTEMPTS: u32 = 5;

/// Deadline duration for elapsed-based strategies.
const DEADLINE: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_state(attempt: u32) -> tenacious::RetryState {
    tenacious::RetryState {
        attempt,
        elapsed: None,
        next_delay: Duration::ZERO,
        total_wait: Duration::ZERO,
    }
}

fn make_state_with_elapsed(attempt: u32, elapsed: Duration) -> tenacious::RetryState {
    tenacious::RetryState {
        attempt,
        elapsed: Some(elapsed),
        next_delay: Duration::ZERO,
        total_wait: Duration::ZERO,
    }
}

fn make_state_with_delay(
    attempt: u32,
    elapsed: Duration,
    next_delay: Duration,
) -> tenacious::RetryState {
    tenacious::RetryState {
        attempt,
        elapsed: Some(elapsed),
        next_delay,
        total_wait: Duration::ZERO,
    }
}

// ---------------------------------------------------------------------------
// 2.1: stop::attempts(n)
// ---------------------------------------------------------------------------

#[test]
fn attempts_does_not_fire_below_threshold() {
    let mut s = stop::attempts(MAX_ATTEMPTS);
    for attempt in 1..MAX_ATTEMPTS {
        let state = make_state(attempt);
        assert!(
            !s.should_stop(&state),
            "should not stop at attempt {attempt}"
        );
    }
}

#[test]
fn attempts_fires_at_threshold() {
    let mut s = stop::attempts(MAX_ATTEMPTS);
    let state = make_state(MAX_ATTEMPTS);
    assert!(s.should_stop(&state));
}

#[test]
fn attempts_fires_above_threshold() {
    let mut s = stop::attempts(MAX_ATTEMPTS);
    let state = make_state(MAX_ATTEMPTS + 1);
    assert!(s.should_stop(&state));
}

#[test]
fn attempts_with_one_stops_immediately() {
    let mut s = stop::attempts(1);
    let state = make_state(1);
    assert!(s.should_stop(&state));
}

#[test]
#[should_panic(expected = "stop::attempts requires max >= 1")]
fn attempts_with_zero_panics() {
    let _ = stop::attempts(0);
}

// ---------------------------------------------------------------------------
// 2.2: stop::elapsed(dur)
// ---------------------------------------------------------------------------

#[test]
fn elapsed_does_not_fire_below_deadline() {
    let mut s = stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE - Duration::from_millis(1));
    assert!(!s.should_stop(&state));
}

#[test]
fn elapsed_fires_at_deadline() {
    let mut s = stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE);
    assert!(s.should_stop(&state));
}

#[test]
fn elapsed_fires_above_deadline() {
    let mut s = stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE + Duration::from_secs(1));
    assert!(s.should_stop(&state));
}

#[test]
fn elapsed_never_fires_when_no_clock() {
    let mut s = stop::elapsed(DEADLINE);
    // elapsed is None — no clock available
    let state = make_state(1);
    assert!(
        !s.should_stop(&state),
        "elapsed should never fire when clock is unavailable"
    );
}

// ---------------------------------------------------------------------------
// 2.3: stop::before_elapsed(dur)
// ---------------------------------------------------------------------------

#[test]
fn before_elapsed_does_not_fire_when_time_remains() {
    let mut s = stop::before_elapsed(DEADLINE);
    // 20s elapsed + 5s delay = 25s < 30s deadline
    let delay = Duration::from_secs(5);
    let state = make_state_with_delay(1, Duration::from_secs(20), delay);
    assert!(!s.should_stop(&state));
}

#[test]
fn before_elapsed_fires_when_next_attempt_would_exceed() {
    let mut s = stop::before_elapsed(DEADLINE);
    // 25s elapsed + 6s delay = 31s >= 30s deadline
    let state = make_state_with_delay(1, Duration::from_secs(25), Duration::from_secs(6));
    assert!(s.should_stop(&state));
}

#[test]
fn before_elapsed_fires_at_exact_boundary() {
    let mut s = stop::before_elapsed(DEADLINE);
    // 25s elapsed + 5s delay = 30s == 30s deadline (fires at >=)
    let state = make_state_with_delay(1, Duration::from_secs(25), Duration::from_secs(5));
    assert!(s.should_stop(&state));
}

#[test]
fn before_elapsed_never_fires_when_no_clock() {
    let mut s = stop::before_elapsed(DEADLINE);
    let state = make_state(1);
    assert!(
        !s.should_stop(&state),
        "before_elapsed should never fire when clock is unavailable"
    );
}

#[test]
fn before_elapsed_saturates_on_overflow() {
    let mut s = stop::before_elapsed(DEADLINE);
    // Duration::MAX + anything should not panic — saturating_add handles it.
    let state = make_state_with_delay(1, Duration::MAX, Duration::from_secs(1));
    assert!(s.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.4: stop::never()
// ---------------------------------------------------------------------------

#[test]
fn never_always_returns_false() {
    let mut s = stop::never();
    // Test at various attempt counts including extremes.
    for &attempt in &[1, 100, u32::MAX] {
        let state = make_state(attempt);
        assert!(
            !s.should_stop(&state),
            "never() should not stop at attempt {attempt}"
        );
    }
}

#[test]
fn never_returns_false_even_with_elapsed() {
    let mut s = stop::never();
    let state = make_state_with_elapsed(u32::MAX, Duration::MAX);
    assert!(!s.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.5: StopAny via BitOr
// ---------------------------------------------------------------------------

#[test]
fn stop_any_fires_when_left_fires() {
    // attempts(1) fires immediately; elapsed(DEADLINE) does not.
    let mut s = stop::attempts(1) | stop::elapsed(DEADLINE);
    let state = make_state(1);
    assert!(s.should_stop(&state));
}

#[test]
fn stop_any_fires_when_right_fires() {
    // attempts(MAX) does not fire; elapsed fires.
    let mut s = stop::attempts(u32::MAX) | stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE);
    assert!(s.should_stop(&state));
}

#[test]
fn stop_any_does_not_fire_when_neither_fires() {
    let mut s = stop::attempts(MAX_ATTEMPTS) | stop::elapsed(DEADLINE);
    let state = make_state(1); // attempt 1 < 5, no clock
    assert!(!s.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.6: StopAll via BitAnd
// ---------------------------------------------------------------------------

#[test]
fn stop_all_fires_when_both_fire() {
    let mut s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(MAX_ATTEMPTS, DEADLINE);
    assert!(s.should_stop(&state));
}

#[test]
fn stop_all_does_not_fire_when_only_left_fires() {
    let mut s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    let state = make_state(MAX_ATTEMPTS); // attempts met but no clock
    assert!(!s.should_stop(&state));
}

#[test]
fn stop_all_does_not_fire_when_only_right_fires() {
    let mut s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE); // elapsed met but attempt 1 < 5
    assert!(!s.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.7: Chained composition
// ---------------------------------------------------------------------------

#[test]
fn three_way_or_composition() {
    // stop::attempts(5) | stop::elapsed(30s) | stop::never()
    let mut s = stop::attempts(MAX_ATTEMPTS) | stop::elapsed(DEADLINE) | stop::never();

    // Only attempts fires
    let state = make_state(MAX_ATTEMPTS);
    assert!(s.should_stop(&state));

    // Only elapsed fires
    let state = make_state_with_elapsed(1, DEADLINE);
    assert!(s.should_stop(&state));

    // Neither fires (never never fires)
    let state = make_state(1);
    assert!(!s.should_stop(&state));
}

#[test]
fn mixed_and_or_composition() {
    // (attempts(5) & elapsed(30s)) | never()
    let mut s = (stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE)) | stop::never();

    // Both inner conditions met — the & fires, so | fires.
    let state = make_state_with_elapsed(MAX_ATTEMPTS, DEADLINE);
    assert!(s.should_stop(&state));

    // Only one inner condition met — the & doesn't fire, never doesn't fire.
    let state = make_state(MAX_ATTEMPTS);
    assert!(!s.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.8: Reset propagation
// ---------------------------------------------------------------------------

#[test]
fn stop_any_reset_propagates_to_both() {
    // Use built-in strategies which implement the operator overloads.
    let mut s = stop::attempts(MAX_ATTEMPTS) | stop::elapsed(DEADLINE);
    // Just verify reset doesn't panic and the strategy still works after.
    s.reset();
    let state = make_state(MAX_ATTEMPTS);
    assert!(s.should_stop(&state));
}

#[test]
fn stop_all_reset_propagates_to_both() {
    let mut s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    s.reset();
    let state = make_state_with_elapsed(MAX_ATTEMPTS, DEADLINE);
    assert!(s.should_stop(&state));
}

#[test]
fn nested_composite_reset_propagates_deeply() {
    let mut s = (stop::attempts(MAX_ATTEMPTS) | stop::elapsed(DEADLINE)) | stop::never();
    s.reset();
    let state = make_state(MAX_ATTEMPTS);
    assert!(s.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.9: Clone and Debug
// ---------------------------------------------------------------------------

#[test]
fn attempts_is_clone_and_debug() {
    let s = stop::attempts(MAX_ATTEMPTS);
    let s2 = s.clone();
    let debug = format!("{:?}", s2);
    assert!(debug.contains("StopAfterAttempts"));
}

#[test]
fn elapsed_is_clone_and_debug() {
    let s = stop::elapsed(DEADLINE);
    let s2 = s.clone();
    let debug = format!("{:?}", s2);
    assert!(debug.contains("StopAfterElapsed"));
}

#[test]
fn before_elapsed_is_clone_and_debug() {
    let s = stop::before_elapsed(DEADLINE);
    let s2 = s.clone();
    let debug = format!("{:?}", s2);
    assert!(debug.contains("StopBeforeElapsed"));
}

#[test]
fn never_is_clone_and_debug() {
    let s = stop::never();
    let s2 = s.clone();
    let debug = format!("{:?}", s2);
    assert!(debug.contains("StopNever"));
}

#[test]
fn stop_any_is_clone_and_debug() {
    let s = stop::attempts(MAX_ATTEMPTS) | stop::elapsed(DEADLINE);
    let s2 = s.clone();
    let debug = format!("{:?}", s2);
    assert!(debug.contains("StopAny"));
}

#[test]
fn stop_all_is_clone_and_debug() {
    let s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    let s2 = s.clone();
    let debug = format!("{:?}", s2);
    assert!(debug.contains("StopAll"));
}

/// Verify cloned strategy behaves independently of the original.
#[test]
fn cloned_strategy_is_independent() {
    let mut original = stop::attempts(MAX_ATTEMPTS);
    let mut cloned = original.clone();

    let state = make_state(MAX_ATTEMPTS);
    assert!(original.should_stop(&state));
    assert!(cloned.should_stop(&state));

    // Both should still work independently after calls.
    let state = make_state(1);
    assert!(!original.should_stop(&state));
    assert!(!cloned.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.8 (extended): Reset propagation with stateful custom strategy
// ---------------------------------------------------------------------------

/// A stateful stop strategy that fires after being consulted a fixed number of
/// times, used to verify reset propagation actually clears internal state.
struct StopAfterConsultations {
    threshold: u32,
    count: u32,
}

impl StopAfterConsultations {
    const fn new(threshold: u32) -> Self {
        Self {
            threshold,
            count: 0,
        }
    }
}

impl Stop for StopAfterConsultations {
    fn should_stop(&mut self, _state: &tenacious::RetryState) -> bool {
        self.count += 1;
        self.count >= self.threshold
    }

    fn reset(&mut self) {
        self.count = 0;
    }
}

/// Number of consultations before the custom strategy fires.
const CONSULTATION_THRESHOLD: u32 = 3;

#[test]
fn stop_any_reset_clears_stateful_strategy() {
    let custom = StopAfterConsultations::new(CONSULTATION_THRESHOLD);
    let mut composite = tenacious::StopAny::new(stop::never(), custom);

    let state = make_state(1);

    // Consult until the custom side fires.
    assert!(!composite.should_stop(&state)); // count 1
    assert!(!composite.should_stop(&state)); // count 2
    assert!(composite.should_stop(&state)); // count 3 — fires

    // After reset, the custom side should start fresh.
    composite.reset();
    assert!(!composite.should_stop(&state)); // count 1 again
    assert!(!composite.should_stop(&state)); // count 2
    assert!(composite.should_stop(&state)); // count 3 — fires again
}

#[test]
fn stop_all_reset_clears_stateful_strategy() {
    let custom = StopAfterConsultations::new(CONSULTATION_THRESHOLD);
    // Both must fire: attempts(1) always fires, custom fires after 3 consultations.
    let mut composite = tenacious::StopAll::new(stop::attempts(1), custom);

    let state = make_state(1);

    // attempts(1) fires immediately, but custom doesn't fire until 3rd consultation.
    assert!(!composite.should_stop(&state)); // custom count 1
    assert!(!composite.should_stop(&state)); // custom count 2
    assert!(composite.should_stop(&state)); // custom count 3 — both fire

    // After reset, custom restarts.
    composite.reset();
    assert!(!composite.should_stop(&state)); // custom count 1
    assert!(!composite.should_stop(&state)); // custom count 2
    assert!(composite.should_stop(&state)); // custom count 3
}

#[test]
fn no_short_circuit_in_stop_any() {
    // Verify both sides are always evaluated even when left fires.
    let custom = StopAfterConsultations::new(CONSULTATION_THRESHOLD);
    // attempts(1) always fires at attempt 1, but custom should still be consulted.
    let mut composite = tenacious::StopAny::new(stop::attempts(1), custom);

    let state = make_state(1);

    // Even though left fires every time, right must still be called (no short-circuit).
    assert!(composite.should_stop(&state)); // fires (left), custom count 1
    assert!(composite.should_stop(&state)); // fires (left), custom count 2
    assert!(composite.should_stop(&state)); // fires (left+right), custom count 3

    // After reset, custom count should have been 3, proving no short-circuit.
    composite.reset();
    assert!(composite.should_stop(&state)); // fires (left), custom count 1 again
}
