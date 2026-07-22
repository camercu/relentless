//! Tests for state types passed to Stop strategies and hooks.
//!
//! Verifies the public layout of `RetryState` and `AttemptState`, the 1-indexed
//! attempt numbers, and the `Exit` view delivered to `on_exit`.

use core::time::Duration;
use relentless::clock::VirtualClock;

#[test]
fn retry_state_attempt_is_one_indexed() {
    let state = relentless::RetryState::for_attempt(1);
    assert_eq!(state.attempt, 1, "first attempt should be 1, not 0");
}

#[test]
fn attempt_state_exposes_attempt_elapsed_and_outcome() {
    let outcome: Result<(), String> = Err("network timeout".to_string());
    let state = relentless::AttemptState::new(1, Duration::ZERO, &outcome);

    assert_eq!(state.attempt, 1);
    assert_eq!(state.elapsed, Duration::ZERO);
    assert_eq!(state.outcome.as_ref().unwrap_err(), "network timeout");
}

#[test]
fn duration_is_core_time_duration() {
    let d: core::time::Duration = Duration::from_millis(10);
    let state = relentless::RetryState::for_attempt(1).with_elapsed(d);
    assert_eq!(state.elapsed, Duration::from_millis(10));
}

/// 3.6.2
#[test]
fn retry_state_is_copy() {
    let state = relentless::RetryState::for_attempt(3).with_elapsed(Duration::from_secs(1));
    let a = state; // copy
    let b = state; // copy again — would fail if state were moved
    assert_eq!(a, b); // whole-struct compare catches any future field, too
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
        .clock(VirtualClock::new())
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
        .clock(VirtualClock::new())
        .call();

    // before_attempt and op both see 1, 2, 3 in order.
    assert_eq!(*before_attempts.borrow(), vec![1, 2, 3]);
    assert_eq!(*op_attempts.borrow(), vec![1, 2, 3]);
}

/// 3.6.6
#[test]
fn exit_attempt_is_exactly_one_for_single_attempt_stop() {
    use relentless::{Exit, RetryPolicy, stop};
    use std::cell::Cell;

    let exit_attempt = Cell::new(0_u32);
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .on_exit(|exit: &Exit<i32, &str, Result<i32, &str>>| {
            exit_attempt.set(exit.attempt());
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(exit_attempt.get(), 1);
}

/// 3.6.6
#[test]
fn exit_attempt_matches_completed_attempts() {
    use relentless::{Exit, RetryPolicy, stop, wait};
    use std::cell::Cell;

    let exit_attempt = Cell::new(0_u32);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .on_exit(|exit: &Exit<i32, &str, Result<i32, &str>>| {
            exit_attempt.set(exit.attempt());
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(exit_attempt.get(), 3);
}

/// 3.6.7
#[test]
fn exit_exposes_the_final_exhausted_outcome() {
    use relentless::{Exit, RetryPolicy, stop};
    use std::cell::RefCell;

    let final_outcome: RefCell<Option<bool>> = RefCell::new(None); // true=ok, false=err

    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let _ = policy
        .retry(|_| Err::<i32, &str>("final error"))
        .on_exit(|exit: &Exit<i32, &str, Result<i32, &str>>| {
            if let Exit::Exhausted { last, .. } = exit {
                *final_outcome.borrow_mut() = Some(last.is_ok());
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(*final_outcome.borrow(), Some(false));
}

/// 3.6.1
#[test]
fn retry_state_for_attempt_defaults_elapsed_zero_and_delay_none() {
    let state = relentless::RetryState::for_attempt(3);

    assert_eq!(state.attempt, 3);
    assert_eq!(state.elapsed, Duration::ZERO);
    assert_eq!(state.previous_delay, None);
}

#[test]
fn retry_state_with_elapsed_sets_elapsed() {
    let elapsed = Duration::from_secs(5);
    let state = relentless::RetryState::for_attempt(1).with_elapsed(elapsed);

    assert_eq!(state.elapsed, elapsed);
}

#[test]
fn retry_state_with_previous_delay_sets_previous_delay() {
    let prev = Duration::from_millis(40);
    let state = relentless::RetryState::for_attempt(2).with_previous_delay(Some(prev));

    assert_eq!(state.previous_delay, Some(prev));
}

#[test]
fn retry_state_setters_chain_without_clobbering() {
    let elapsed = Duration::from_secs(5);
    let prev = Duration::from_millis(40);
    let state = relentless::RetryState::for_attempt(2)
        .with_elapsed(elapsed)
        .with_previous_delay(Some(prev));

    assert_eq!(state.elapsed, elapsed);
    assert_eq!(state.previous_delay, Some(prev));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "attempt is 1-indexed")]
fn retry_state_for_attempt_zero_panics_in_debug() {
    let _ = relentless::RetryState::for_attempt(0);
}

/// 3.6.1
#[test]
fn attempt_state_new_zero_panics_in_debug() {
    // Guarded separately: the panic path only exists in debug builds.
    #[cfg(debug_assertions)]
    {
        let outcome: Result<i32, &str> = Ok(1);
        let result = std::panic::catch_unwind(|| {
            let _ = relentless::AttemptState::new(0, Duration::ZERO, &outcome);
        });
        assert!(result.is_err(), "attempt 0 should panic in debug");
    }
}
