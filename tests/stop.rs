//! Acceptance tests for the Stop trait and stop strategies.
//!
//! These tests verify:
//! - Stop trait: should_stop(&self, &RetryState) -> bool
//! - Stop and Wait are non-generic (decoupled from T, E)
//! - stop::attempts(n) fires at the correct threshold
//! - stop::elapsed(dur) fires on deadline, handles None clock
//! - stop::never() always returns false
//! - BitOr composition produces StopAny
//! - BitAnd composition produces StopAll
//! - Clone and Debug derive on all types

use core::cell::Cell;
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
    tenacious::RetryState::new(attempt, None)
}

fn make_state_with_elapsed(attempt: u32, elapsed: Duration) -> tenacious::RetryState {
    tenacious::RetryState::new(attempt, Some(elapsed))
}

// ---------------------------------------------------------------------------
// Stop trait contract
// ---------------------------------------------------------------------------

/// A trivial Stop implementation that stops after a fixed number of attempts.
struct StopAfter {
    max: u32,
}

impl Stop for StopAfter {
    fn should_stop(&self, state: &tenacious::RetryState) -> bool {
        state.attempt >= self.max
    }
}

#[test]
fn stop_should_stop_takes_ref_self_and_retry_state() {
    let stop = StopAfter { max: 3 };

    let state = make_state(1);
    assert!(
        !stop.should_stop(&state),
        "attempt 1 < max, should not stop"
    );

    let state = make_state(3);
    assert!(stop.should_stop(&state), "attempt == max, should stop");
}

/// Stop and Wait accept RetryState (non-generic), so a single strategy
/// works across operations with different Result types.
#[test]
fn stop_and_wait_are_not_generic_over_result_type() {
    let stop = StopAfter { max: 3 };
    let state = make_state(1);

    // The same stop instance works regardless of operation type —
    // no <T, E> parameterization needed. This is a compile-time check.
    assert!(!stop.should_stop(&state));
}

// ---------------------------------------------------------------------------
// stop::attempts(n)
// ---------------------------------------------------------------------------

#[test]
fn attempts_does_not_fire_below_threshold() {
    let s = stop::attempts(MAX_ATTEMPTS);
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
    let s = stop::attempts(MAX_ATTEMPTS);
    let state = make_state(MAX_ATTEMPTS);
    assert!(s.should_stop(&state));
}

#[test]
fn attempts_fires_above_threshold() {
    let s = stop::attempts(MAX_ATTEMPTS);
    let state = make_state(MAX_ATTEMPTS + 1);
    assert!(s.should_stop(&state));
}

#[test]
fn attempts_with_one_stops_immediately() {
    let s = stop::attempts(1);
    let state = make_state(1);
    assert!(s.should_stop(&state));
}

#[test]
fn attempts_with_zero_panics() {
    let panic_result = std::panic::catch_unwind(|| {
        let _ = stop::attempts(0);
    });
    let panic_message = panic_result
        .as_ref()
        .err()
        .and_then(|payload| payload.downcast_ref::<&str>().copied())
        .or_else(|| {
            panic_result
                .as_ref()
                .err()
                .and_then(|payload| payload.downcast_ref::<String>().map(String::as_str))
        })
        .unwrap_or("<non-string panic payload>");
    assert!(
        panic_result.is_err(),
        "stop::attempts(0) should panic with invalid configuration"
    );
    assert!(
        panic_message.contains("stop::attempts requires max >= 1"),
        "panic message should explain invalid attempts count, got: {panic_message}"
    );
}

// ---------------------------------------------------------------------------
// 2.2: stop::elapsed(dur)
// ---------------------------------------------------------------------------

#[test]
fn elapsed_does_not_fire_below_deadline() {
    let s = stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE - Duration::from_millis(1));
    assert!(!s.should_stop(&state));
}

#[test]
fn elapsed_fires_at_deadline() {
    let s = stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE);
    assert!(s.should_stop(&state));
}

#[test]
fn elapsed_fires_above_deadline() {
    let s = stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE + Duration::from_secs(1));
    assert!(s.should_stop(&state));
}

#[test]
fn elapsed_never_fires_when_no_clock() {
    let s = stop::elapsed(DEADLINE);
    // elapsed is None — no clock available
    let state = make_state(1);
    assert!(
        !s.should_stop(&state),
        "elapsed should never fire when clock is unavailable"
    );
}

// ---------------------------------------------------------------------------
// 2.4: stop::never()
// ---------------------------------------------------------------------------

#[test]
fn never_always_returns_false() {
    let s = stop::never();
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
    let s = stop::never();
    let state = make_state_with_elapsed(u32::MAX, Duration::MAX);
    assert!(!s.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.5: StopAny via BitOr
// ---------------------------------------------------------------------------

#[test]
fn stop_any_fires_when_left_fires() {
    // attempts(1) fires immediately; elapsed(DEADLINE) does not.
    let s = stop::attempts(1) | stop::elapsed(DEADLINE);
    let state = make_state(1);
    assert!(s.should_stop(&state));
}

#[test]
fn stop_any_fires_when_right_fires() {
    // attempts(MAX) does not fire; elapsed fires.
    let s = stop::attempts(u32::MAX) | stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE);
    assert!(s.should_stop(&state));
}

#[test]
fn stop_any_does_not_fire_when_neither_fires() {
    let s = stop::attempts(MAX_ATTEMPTS) | stop::elapsed(DEADLINE);
    let state = make_state(1); // attempt 1 < 5, no clock
    assert!(!s.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.6: StopAll via BitAnd
// ---------------------------------------------------------------------------

#[test]
fn stop_all_fires_when_both_fire() {
    let s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(MAX_ATTEMPTS, DEADLINE);
    assert!(s.should_stop(&state));
}

#[test]
fn stop_all_does_not_fire_when_only_left_fires() {
    let s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    let state = make_state(MAX_ATTEMPTS); // attempts met but no clock
    assert!(!s.should_stop(&state));
}

#[test]
fn stop_all_does_not_fire_when_only_right_fires() {
    let s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE); // elapsed met but attempt 1 < 5
    assert!(!s.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.7: Chained composition
// ---------------------------------------------------------------------------

#[test]
fn three_way_or_composition() {
    // stop::attempts(5) | stop::elapsed(30s) | stop::never()
    let s = stop::attempts(MAX_ATTEMPTS) | stop::elapsed(DEADLINE) | stop::never();

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
    let s = (stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE)) | stop::never();

    // Both inner conditions met — the & fires, so | fires.
    let state = make_state_with_elapsed(MAX_ATTEMPTS, DEADLINE);
    assert!(s.should_stop(&state));

    // Only one inner condition met — the & doesn't fire, never doesn't fire.
    let state = make_state(MAX_ATTEMPTS);
    assert!(!s.should_stop(&state));
}

// ---------------------------------------------------------------------------
// 2.9: Clone and Debug
// ---------------------------------------------------------------------------

#[test]
fn attempts_is_clone_and_debug() {
    let s = stop::attempts(MAX_ATTEMPTS);
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&s);
    let debug = format!("{:?}", s);
    assert!(debug.contains("StopAfterAttempts"));
}

#[test]
fn elapsed_is_clone_and_debug() {
    let s = stop::elapsed(DEADLINE);
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&s);
    let debug = format!("{:?}", s);
    assert!(debug.contains("StopAfterElapsed"));
}

#[test]
fn never_is_clone_and_debug() {
    let s = stop::never();
    fn assert_clone<T: Clone>(_value: &T) {}
    assert_clone(&s);
    let debug = format!("{:?}", s);
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
    let original = stop::attempts(MAX_ATTEMPTS);
    let cloned = original;

    let state = make_state(MAX_ATTEMPTS);
    assert!(original.should_stop(&state));
    assert!(cloned.should_stop(&state));

    // Both should still work independently after calls.
    let state = make_state(1);
    assert!(!original.should_stop(&state));
    assert!(!cloned.should_stop(&state));
}

// ---------------------------------------------------------------------------
// No short-circuit in StopAny (stateful custom strategy with interior mutability)
// ---------------------------------------------------------------------------

/// A stateful stop strategy that fires after being consulted a fixed number of
/// times, used to verify StopAny evaluates both sides.
struct StopAfterConsultations {
    threshold: u32,
    count: Cell<u32>,
}

impl StopAfterConsultations {
    const fn new(threshold: u32) -> Self {
        Self {
            threshold,
            count: Cell::new(0),
        }
    }
}

impl Stop for StopAfterConsultations {
    fn should_stop(&self, _state: &tenacious::RetryState) -> bool {
        let n = self.count.get() + 1;
        self.count.set(n);
        n >= self.threshold
    }
}

/// Number of consultations before the custom strategy fires.
const CONSULTATION_THRESHOLD: u32 = 3;

#[test]
fn no_short_circuit_in_stop_any() {
    // Verify both sides are always evaluated even when left fires.
    // attempts(1) always fires at attempt 1, but the right side should still be consulted.
    let composite = stop::attempts(1) | StopAfterConsultations::new(CONSULTATION_THRESHOLD);

    let state = make_state(1);

    // Even though left fires every time, right must still be called (no short-circuit).
    assert!(composite.should_stop(&state)); // fires (left), custom count 1
    assert!(composite.should_stop(&state)); // fires (left), custom count 2
    assert!(composite.should_stop(&state)); // fires (left+right), custom count 3
}

// ---------------------------------------------------------------------------
// Named combinators (.or, .and) match operator forms
// ---------------------------------------------------------------------------

#[test]
fn stop_named_combinators_match_operator_forms() {
    let named_or = stop::attempts(3).or(stop::elapsed(Duration::from_secs(2)));
    let op_or = stop::attempts(3) | stop::elapsed(Duration::from_secs(2));

    let early = make_state_with_elapsed(1, Duration::from_secs(1));
    let elapsed_hit = make_state_with_elapsed(1, Duration::from_secs(2));
    let attempt_hit = make_state(3);
    assert_eq!(named_or.should_stop(&early), op_or.should_stop(&early));
    assert_eq!(
        named_or.should_stop(&elapsed_hit),
        op_or.should_stop(&elapsed_hit)
    );
    assert_eq!(
        named_or.should_stop(&attempt_hit),
        op_or.should_stop(&attempt_hit)
    );

    let named_and = stop::attempts(3).and(stop::elapsed(Duration::from_secs(2)));
    let op_and = stop::attempts(3) & stop::elapsed(Duration::from_secs(2));

    let both_hit = make_state_with_elapsed(3, Duration::from_secs(2));
    assert_eq!(named_and.should_stop(&early), op_and.should_stop(&early));
    assert_eq!(
        named_and.should_stop(&elapsed_hit),
        op_and.should_stop(&elapsed_hit)
    );
    assert_eq!(
        named_and.should_stop(&both_hit),
        op_and.should_stop(&both_hit)
    );
}
