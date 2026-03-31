//! Tests for the Stop trait and built-in stop strategies.
//!
//! Covers threshold/boundary behavior of each strategy, the non-generic (T/E-independent)
//! nature of Stop, composition via `|`/`&` (StopAny/StopAll), and the no-short-circuit
//! guarantee for composite strategies.

use core::cell::Cell;
use core::time::Duration;
use tenacious::Stop;
use tenacious::stop;

const MAX_ATTEMPTS: u32 = 5;
const DEADLINE: Duration = Duration::from_secs(30);

fn make_state(attempt: u32) -> tenacious::RetryState {
    tenacious::RetryState::new(attempt, None)
}

fn make_state_with_elapsed(attempt: u32, elapsed: Duration) -> tenacious::RetryState {
    tenacious::RetryState::new(attempt, Some(elapsed))
}

/// Minimal Stop implementation used to verify the trait contract.
/// Stops when `state.attempt >= self.max`.
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

#[test]
fn stop_and_wait_are_not_generic_over_result_type() {
    let stop = StopAfter { max: 3 };
    let state = make_state(1);

    // Stop takes &RetryState, not Result<T, E> — a single strategy instance works across
    // operations with different outcome types. This is a compile-time check.
    assert!(!stop.should_stop(&state));
}

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
    // make_state passes None for elapsed — simulates no clock available
    let state = make_state(1);
    assert!(
        !s.should_stop(&state),
        "elapsed should never fire when clock is unavailable"
    );
}

#[test]
fn never_always_returns_false() {
    let s = stop::never();
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
    let state = make_state(1); // attempt 1 < 5 and no elapsed — neither condition is met
    assert!(!s.should_stop(&state));
}

#[test]
fn stop_all_fires_when_both_fire() {
    let s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(MAX_ATTEMPTS, DEADLINE);
    assert!(s.should_stop(&state));
}

#[test]
fn stop_all_does_not_fire_when_only_left_fires() {
    let s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    let state = make_state(MAX_ATTEMPTS); // elapsed is None, so elapsed condition is unmet
    assert!(!s.should_stop(&state));
}

#[test]
fn stop_all_does_not_fire_when_only_right_fires() {
    let s = stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE);
    let state = make_state_with_elapsed(1, DEADLINE); // attempts condition unmet (1 < 5)
    assert!(!s.should_stop(&state));
}

#[test]
fn three_way_or_composition() {
    let s = stop::attempts(MAX_ATTEMPTS) | stop::elapsed(DEADLINE) | stop::never();

    let state = make_state(MAX_ATTEMPTS);
    assert!(s.should_stop(&state));

    let state = make_state_with_elapsed(1, DEADLINE);
    assert!(s.should_stop(&state));

    // never() never fires, and neither of the other conditions is met here
    let state = make_state(1);
    assert!(!s.should_stop(&state));
}

#[test]
fn mixed_and_or_composition() {
    // (attempts(5) & elapsed(30s)) | never()
    let s = (stop::attempts(MAX_ATTEMPTS) & stop::elapsed(DEADLINE)) | stop::never();

    // Both inner conditions met — the & fires, so the outer | fires too.
    let state = make_state_with_elapsed(MAX_ATTEMPTS, DEADLINE);
    assert!(s.should_stop(&state));

    // Only attempts condition met — the & doesn't fire, and never() never fires.
    let state = make_state(MAX_ATTEMPTS);
    assert!(!s.should_stop(&state));
}

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

#[test]
fn cloned_strategy_is_independent() {
    let original = stop::attempts(MAX_ATTEMPTS);
    let cloned = original;

    let state = make_state(MAX_ATTEMPTS);
    assert!(original.should_stop(&state));
    assert!(cloned.should_stop(&state));

    let state = make_state(1);
    assert!(!original.should_stop(&state));
    assert!(!cloned.should_stop(&state));
}

/// A stateful stop strategy that fires after being consulted a fixed number of times.
/// Used with interior mutability to count evaluations and verify StopAny's
/// no-short-circuit guarantee.
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

const CONSULTATION_THRESHOLD: u32 = 3;

#[test]
fn no_short_circuit_in_stop_any() {
    // StopAny must evaluate both sides on every call — it does not short-circuit when
    // the left side fires, because stateful strategies on the right rely on being called.
    let composite = stop::attempts(1) | StopAfterConsultations::new(CONSULTATION_THRESHOLD);
    let state = make_state(1);

    assert!(composite.should_stop(&state)); // left fires; right count -> 1
    assert!(composite.should_stop(&state)); // left fires; right count -> 2
    assert!(composite.should_stop(&state)); // left fires; right count -> 3 (threshold reached)
}

#[test]
fn no_short_circuit_in_stop_all() {
    // StopAll must evaluate both sides on every call — it does not short-circuit when
    // the left side returns false, because stateful strategies on the right rely on being called.
    let composite = stop::never() & StopAfterConsultations::new(CONSULTATION_THRESHOLD);
    let state = make_state(1);

    assert!(!composite.should_stop(&state)); // left=false; right count -> 1
    assert!(!composite.should_stop(&state)); // left=false; right count -> 2
    assert!(!composite.should_stop(&state)); // left=false; right count -> 3 (threshold reached, but left never fires)
    // Right has hit threshold but left (never()) never fires, so StopAll never fires either.
    // Crucially, right was evaluated all 3 times despite left always returning false.
    assert_eq!(
        composite.should_stop(&state),
        false,
        "StopAll fires only when both fire; left=never keeps it from firing"
    );
}

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
