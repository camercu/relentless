//! Tests for state types passed to Stop strategies and hooks.
//!
//! Verifies the public field layout of `RetryState`, `AttemptState`, and `ExitState`,
//! including 1-indexed attempt numbers and Option<Duration> for elapsed time.

use core::time::Duration;

#[test]
fn retry_state_has_required_fields() {
    let elapsed = Duration::from_secs(5);
    let state = relentless::RetryState::for_attempt(1).with_elapsed(Some(elapsed));

    assert_eq!(state.attempt, 1);
    assert_eq!(state.elapsed, Some(elapsed));
}

#[test]
fn retry_state_attempt_is_one_indexed() {
    let state = relentless::RetryState::for_attempt(1);
    assert_eq!(state.attempt, 1, "first attempt should be 1, not 0");
}

#[test]
fn retry_state_elapsed_can_be_none() {
    let state = relentless::RetryState::for_attempt(1);
    assert_eq!(state.elapsed, None);
}

#[test]
fn attempt_state_has_flat_fields_and_outcome() {
    let outcome: Result<i32, String> = Ok(42);

    let state = relentless::AttemptState::for_attempt(1, &outcome)
        .with_elapsed(Some(Duration::ZERO))
        .with_next_delay(Some(Duration::from_millis(100)));

    assert_eq!(state.attempt, 1);
    assert_eq!(*state.outcome, Ok(42));
    assert_eq!(state.next_delay, Some(Duration::from_millis(100)));
}

#[test]
fn attempt_state_with_err_outcome() {
    let outcome: Result<(), String> = Err("network timeout".to_string());

    let state = relentless::AttemptState::for_attempt(1, &outcome)
        .with_elapsed(Some(Duration::ZERO))
        .with_next_delay(Some(Duration::ZERO));

    assert!(state.outcome.is_err());
    assert_eq!(state.outcome.as_ref().unwrap_err(), "network timeout");
}

#[test]
fn exit_state_has_required_fields() {
    let outcome = Err::<i32, &str>("fatal");
    let state = relentless::ExitState::for_attempt(2, &outcome, relentless::StopReason::Exhausted);

    assert_eq!(state.attempt, 2);
    assert!(state.outcome.is_err());
    assert_eq!(state.elapsed, None);
    assert_eq!(state.stop_reason, relentless::StopReason::Exhausted);
}

#[test]
fn duration_is_core_time_duration() {
    let d: core::time::Duration = Duration::from_millis(10);
    let state = relentless::RetryState::for_attempt(1).with_elapsed(Some(d));
    assert_eq!(state.elapsed, Some(Duration::from_millis(10)));
}

/// 3.6.2
#[test]
fn retry_state_is_copy() {
    let state = relentless::RetryState::for_attempt(3).with_elapsed(Some(Duration::from_secs(1)));
    let a = state; // copy
    let b = state; // copy again — would fail if state were moved
    assert_eq!(a.attempt, b.attempt);
    assert_eq!(a.elapsed, b.elapsed);
}

/// 3.6.3
#[test]
fn retry_state_attempts_are_one_indexed_in_execution() {
    use relentless::{RetryPolicy, stop, wait};
    use std::cell::RefCell;

    let attempts_seen: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO));

    let _ = policy
        .retry(|state| {
            attempts_seen.borrow_mut().push(state.attempt);
            Err::<i32, &str>("fail")
        })
        .sleep(|_| {})
        .call();

    assert_eq!(*attempts_seen.borrow(), vec![1, 2, 3]);
}

/// 3.6.3
#[test]
fn before_attempt_and_op_see_same_attempt_as_stop_wait() {
    use relentless::{RetryPolicy, stop};
    use std::cell::RefCell;

    let before_attempts: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let op_attempts: RefCell<Vec<u32>> = RefCell::new(Vec::new());

    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(relentless::wait::fixed(Duration::ZERO));

    let _ = policy
        .retry(|state| {
            op_attempts.borrow_mut().push(state.attempt);
            Err::<i32, &str>("fail")
        })
        .before_attempt(|state| {
            before_attempts.borrow_mut().push(state.attempt);
        })
        .sleep(|_| {})
        .call();

    // before_attempt and op both see 1, 2, 3 in order.
    assert_eq!(*before_attempts.borrow(), vec![1, 2, 3]);
    assert_eq!(*op_attempts.borrow(), vec![1, 2, 3]);
}

/// 3.6.5
#[test]
fn attempt_state_next_delay_none_on_final() {
    use relentless::{RetryPolicy, stop, wait};
    use std::cell::RefCell;

    let next_delays: RefCell<Vec<Option<Duration>>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::from_millis(10)));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .after_attempt(|state: &relentless::AttemptState<i32, &str>| {
            next_delays.borrow_mut().push(state.next_delay);
        })
        .sleep(|_| {})
        .call();

    let delays = next_delays.borrow();
    // First two attempts have a next delay, the final one does not.
    assert_eq!(delays[0], Some(Duration::from_millis(10)));
    assert_eq!(delays[1], Some(Duration::from_millis(10)));
    assert_eq!(delays[2], None, "final attempt should have next_delay=None");
}

/// 3.6.6
#[test]
fn exit_state_attempt_is_at_least_one() {
    use relentless::{RetryPolicy, stop};
    use std::cell::Cell;

    let exit_attempt = Cell::new(0_u32);
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .on_exit(|state: &relentless::ExitState<i32, &str>| {
            exit_attempt.set(state.attempt);
        })
        .sleep(|_| {})
        .call();

    assert!(exit_attempt.get() >= 1);
}

/// 3.6.7
#[test]
fn exit_state_outcome_is_final_attempt_result() {
    use relentless::{RetryPolicy, stop};
    use std::cell::RefCell;

    let final_outcome: RefCell<Option<bool>> = RefCell::new(None); // true=ok, false=err

    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let _ = policy
        .retry(|_| Err::<i32, &str>("final error"))
        .on_exit(|state: &relentless::ExitState<i32, &str>| {
            *final_outcome.borrow_mut() = Some(state.outcome.is_ok());
        })
        .sleep(|_| {})
        .call();

    assert_eq!(*final_outcome.borrow(), Some(false));
}

/// 3.6.1
#[test]
fn state_types_are_constructed_via_for_attempt() {
    // If these compile, the public constructors exist and work.
    let _ = relentless::RetryState::for_attempt(1);
    let outcome: Result<i32, &str> = Ok(1);
    let _ = relentless::AttemptState::for_attempt(1, &outcome);
    let _ = relentless::ExitState::for_attempt(1, &outcome, relentless::StopReason::Succeeded);
}

#[test]
fn retry_state_for_attempt_defaults_optional_fields_to_none() {
    let state = relentless::RetryState::for_attempt(3);

    assert_eq!(state.attempt, 3);
    assert_eq!(state.elapsed, None);
    assert_eq!(state.previous_delay, None);
}

#[test]
fn retry_state_with_elapsed_sets_elapsed() {
    let elapsed = Duration::from_secs(5);
    let state = relentless::RetryState::for_attempt(1).with_elapsed(Some(elapsed));

    assert_eq!(state.elapsed, Some(elapsed));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "attempt is 1-indexed")]
fn retry_state_for_attempt_zero_panics_in_debug() {
    let _ = relentless::RetryState::for_attempt(0);
}

#[test]
fn attempt_state_for_attempt_defaults_optional_fields_to_none() {
    let outcome: Result<i32, &str> = Ok(42);
    let state = relentless::AttemptState::for_attempt(2, &outcome);

    assert_eq!(state.attempt, 2);
    assert_eq!(*state.outcome, Ok(42));
    assert_eq!(state.elapsed, None);
    assert_eq!(state.next_delay, None);
}

#[test]
fn attempt_state_with_elapsed_and_next_delay_set_fields() {
    let outcome: Result<i32, &str> = Ok(42);
    let state = relentless::AttemptState::for_attempt(1, &outcome)
        .with_elapsed(Some(Duration::ZERO))
        .with_next_delay(Some(Duration::from_millis(100)));

    assert_eq!(state.elapsed, Some(Duration::ZERO));
    assert_eq!(state.next_delay, Some(Duration::from_millis(100)));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "attempt is 1-indexed")]
fn attempt_state_for_attempt_zero_panics_in_debug() {
    let outcome: Result<i32, &str> = Ok(1);
    let _ = relentless::AttemptState::for_attempt(0, &outcome);
}

#[test]
fn exit_state_for_attempt_defaults_elapsed_to_none() {
    let outcome = Err::<i32, &str>("fatal");
    let state = relentless::ExitState::for_attempt(2, &outcome, relentless::StopReason::Exhausted);

    assert_eq!(state.attempt, 2);
    assert!(state.outcome.is_err());
    assert_eq!(state.elapsed, None);
    assert_eq!(state.stop_reason, relentless::StopReason::Exhausted);
}

#[test]
fn exit_state_with_elapsed_sets_elapsed() {
    let outcome: Result<i32, &str> = Ok(1);
    let state = relentless::ExitState::for_attempt(1, &outcome, relentless::StopReason::Succeeded)
        .with_elapsed(Some(Duration::from_secs(2)));

    assert_eq!(state.elapsed, Some(Duration::from_secs(2)));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "attempt is 1-indexed")]
fn exit_state_for_attempt_zero_panics_in_debug() {
    let outcome: Result<i32, &str> = Ok(1);
    let _ = relentless::ExitState::for_attempt(0, &outcome, relentless::StopReason::Succeeded);
}
