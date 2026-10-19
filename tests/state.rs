//! Tests for `RetryState`, the context passed to Stop and Wait strategies and
//! to `before_attempt`.
//!
//! Verifies the public layout, the 1-indexed attempt numbers and the setters.

use core::time::Duration;
use relentless::clock::VirtualClock;

#[test]
fn retry_state_attempt_is_one_indexed() {
    let state = relentless::RetryState::for_attempt(1);
    assert_eq!(state.attempt, 1, "first attempt should be 1, not 0");
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

/// §14
#[test]
fn retry_state_is_clone_copy_partial_eq() {
    use core::time::Duration;
    use relentless::RetryState;

    let s = RetryState::for_attempt(3).with_elapsed(Duration::from_secs(1));
    let cloned = s; // Copy
    let copied = s; // Copy again — would fail if s were moved
    assert_eq!(s, cloned);
    assert_eq!(s, copied);
    assert_ne!(s, RetryState::for_attempt(4));
}
