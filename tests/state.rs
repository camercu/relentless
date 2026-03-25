//! Acceptance tests for state types (Spec §Core abstractions → State types).
//!
//! These tests verify:
//! - RetryState has `attempt` (1-indexed) and `elapsed` (Option<Duration>) fields
//! - AttemptState has flat fields including `outcome` and `next_delay`
//! - ExitState has `stop_reason` field
//! - Duration is core::time::Duration

use core::time::Duration;

// ---------------------------------------------------------------------------
// RetryState
// ---------------------------------------------------------------------------

#[test]
fn retry_state_has_required_fields() {
    let elapsed = Duration::from_secs(5);
    let state = tenacious::RetryState::new(1, Some(elapsed));

    assert_eq!(state.attempt, 1);
    assert_eq!(state.elapsed, Some(elapsed));
}

#[test]
fn retry_state_attempt_is_one_indexed() {
    let state = tenacious::RetryState::new(1, None);
    assert_eq!(state.attempt, 1, "first attempt should be 1, not 0");
}

#[test]
fn retry_state_elapsed_can_be_none() {
    let state = tenacious::RetryState::new(1, None);
    assert_eq!(state.elapsed, None);
}

// ---------------------------------------------------------------------------
// AttemptState
// ---------------------------------------------------------------------------

#[test]
fn attempt_state_has_flat_fields_and_outcome() {
    let outcome: Result<i32, String> = Ok(42);

    let state = tenacious::AttemptState::new(
        1,
        Some(Duration::ZERO),
        &outcome,
        Some(Duration::from_millis(100)),
    );

    assert_eq!(state.attempt, 1);
    assert_eq!(*state.outcome, Ok(42));
    assert_eq!(state.next_delay, Some(Duration::from_millis(100)));
}

#[test]
fn attempt_state_with_err_outcome() {
    let outcome: Result<(), String> = Err("network timeout".to_string());

    let state =
        tenacious::AttemptState::new(1, Some(Duration::ZERO), &outcome, Some(Duration::ZERO));

    assert!(state.outcome.is_err());
    assert_eq!(state.outcome.as_ref().unwrap_err(), "network timeout");
}

// ---------------------------------------------------------------------------
// ExitState
// ---------------------------------------------------------------------------

#[test]
fn exit_state_has_required_fields() {
    let outcome = Err::<i32, &str>("fatal");
    let state = tenacious::ExitState::new(2, None, &outcome, tenacious::StopReason::Exhausted);

    assert_eq!(state.attempt, 2);
    assert!(state.outcome.is_err());
    assert_eq!(state.elapsed, None);
    assert_eq!(state.stop_reason, tenacious::StopReason::Exhausted);
}

// ---------------------------------------------------------------------------
// Duration is core::time::Duration
// ---------------------------------------------------------------------------

#[test]
fn duration_is_core_time_duration() {
    let d: core::time::Duration = Duration::from_millis(10);
    let state = tenacious::RetryState::new(1, Some(d));
    assert_eq!(state.elapsed, Some(Duration::from_millis(10)));
}
