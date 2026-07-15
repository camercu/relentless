//! Tests for `before_attempt`, `after_attempt`, and `on_exit` hooks.
//!
//! Verifies call ordering relative to the operation and predicate evaluation,
//! the `next_delay` field in `AttemptState` (Some for retried attempts, None for terminal),
//! and that multiple hooks of the same kind fire in registration order.

use core::cell::Cell;
use core::time::Duration;
use relentless::RetryPolicy;
use relentless::{StopReason, predicate, stop, wait};
use std::cell::RefCell;

const MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(10);

fn instant_sleep(_dur: Duration) {}

#[test]
fn before_attempt_and_after_attempt_fire_at_defined_points() {
    let events: RefCell<Vec<(char, u32)>> = RefCell::new(Vec::new());
    let op_attempt = Cell::new(0_u32);

    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|_| {
            let attempt = op_attempt.get().saturating_add(1);
            op_attempt.set(attempt);
            events.borrow_mut().push(('o', attempt));
            Err::<i32, _>("fail")
        })
        .before_attempt(|state| events.borrow_mut().push(('b', state.attempt)))
        .after_attempt(|state: &relentless::AttemptState<i32, &str>| {
            events.borrow_mut().push(('a', state.attempt));
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(
        *events.borrow(),
        vec![
            ('b', 1),
            ('o', 1),
            ('a', 1),
            ('b', 2),
            ('o', 2),
            ('a', 2),
            ('b', 3),
            ('o', 3),
            ('a', 3),
        ]
    );
}

/// SPEC 3.6: `RetryState.previous_delay` is "the delay applied before this
/// attempt", shared with the `before_attempt` hook — `None` only on the first
/// attempt.
#[test]
fn before_attempt_sees_previous_delay() {
    let delays: RefCell<Vec<Option<Duration>>> = RefCell::new(Vec::new());

    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .before_attempt(|state| delays.borrow_mut().push(state.previous_delay))
        .sleep(instant_sleep)
        .call();

    assert_eq!(
        *delays.borrow(),
        vec![None, Some(WAIT_DURATION), Some(WAIT_DURATION)]
    );
}

#[test]
fn after_attempt_runs_after_predicate_evaluation() {
    let predicate_calls = Cell::new(0_u32);
    let after_observed: RefCell<Vec<u32>> = RefCell::new(Vec::new());

    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .when(predicate::result(|_outcome: &Result<i32, &str>| {
            predicate_calls.set(predicate_calls.get().saturating_add(1));
            true
        }));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .after_attempt(|_state: &relentless::AttemptState<i32, &str>| {
            after_observed.borrow_mut().push(predicate_calls.get());
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(*after_observed.borrow(), vec![1, 2]);
}

#[test]
fn after_attempt_receives_next_delay_for_retryable_attempts() {
    let seen: RefCell<Vec<(u32, Option<Duration>)>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .after_attempt(|state: &relentless::AttemptState<i32, &str>| {
            seen.borrow_mut().push((state.attempt, state.next_delay));
        })
        .sleep(instant_sleep)
        .call();

    // next_delay is Some for attempts that will be retried, None for the terminal attempt
    assert_eq!(
        *seen.borrow(),
        vec![
            (1, Some(WAIT_DURATION)),
            (2, Some(WAIT_DURATION)),
            (3, None),
        ]
    );
}

/// SPEC 11.4.2: the delay passed to `after_attempt` is the *clamped* value —
/// the hook observes the truthful sleep, never the raw wait-strategy output.
#[cfg(feature = "alloc")]
#[test]
fn after_attempt_sees_next_delay_clamped_to_timeout_budget() {
    const TIMEOUT: Duration = Duration::from_millis(100);
    const FULL_WAIT: Duration = Duration::from_millis(50);
    const OP_RUNTIME: Duration = Duration::from_millis(40);
    // Remaining budget when attempt 2 finishes: 100ms - 2 * 40ms.
    const CLAMPED_WAIT: Duration = Duration::from_millis(20);

    let clock_millis = std::rc::Rc::new(Cell::new(0_u64));
    let op_clock = std::rc::Rc::clone(&clock_millis);
    let seen: RefCell<Vec<(u32, Option<Duration>)>> = RefCell::new(Vec::new());

    let policy = RetryPolicy::new()
        .stop(stop::attempts(10))
        .wait(wait::fixed(FULL_WAIT));

    let _ = policy
        .retry(|_| {
            op_clock.set(op_clock.get() + OP_RUNTIME.as_millis() as u64);
            Err::<i32, _>("fail")
        })
        .after_attempt(|state: &relentless::AttemptState<i32, &str>| {
            seen.borrow_mut().push((state.attempt, state.next_delay));
        })
        .sleep(instant_sleep)
        .timeout(TIMEOUT)
        .elapsed_clock_fn(move || Duration::from_millis(clock_millis.get()))
        .call();

    // Attempt 1: 60ms budget left, full wait fits. Attempt 2: only 20ms left —
    // the hook must see the clamp. Attempt 3: budget exhausted, terminal.
    assert_eq!(
        *seen.borrow(),
        vec![(1, Some(FULL_WAIT)), (2, Some(CLAMPED_WAIT)), (3, None),]
    );
}

#[test]
fn on_exit_fires_once_with_final_state() {
    let exits: RefCell<Vec<(u32, bool, StopReason)>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .on_exit(|state: &relentless::ExitState<i32, &str>| {
            exits
                .borrow_mut()
                .push((state.attempt, state.outcome.is_err(), state.stop_reason));
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(
        *exits.borrow(),
        vec![(MAX_ATTEMPTS, true, StopReason::Exhausted)]
    );
}

#[test]
fn on_exit_reports_non_retryable_error_reason() {
    let reasons: RefCell<Vec<StopReason>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(predicate::error(|err: &&str| *err == "retryable"));

    let _ = policy
        .retry(|_| Err::<i32, _>("fatal"))
        .on_exit(|state: &relentless::ExitState<i32, &str>| {
            reasons.borrow_mut().push(state.stop_reason);
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(*reasons.borrow(), vec![StopReason::Rejected]);
}

#[cfg(feature = "alloc")]
#[test]
fn multiple_hooks_of_same_kind_fire_in_registration_order() {
    let calls: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());

    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .before_attempt(|_state| calls.borrow_mut().push("before_1"))
        .before_attempt(|_state| calls.borrow_mut().push("before_2"))
        .after_attempt(|_state: &relentless::AttemptState<i32, &str>| {
            calls.borrow_mut().push("after_1");
        })
        .after_attempt(|_state: &relentless::AttemptState<i32, &str>| {
            calls.borrow_mut().push("after_2");
        })
        .on_exit(|_state: &relentless::ExitState<i32, &str>| calls.borrow_mut().push("exit_1"))
        .on_exit(|_state: &relentless::ExitState<i32, &str>| calls.borrow_mut().push("exit_2"))
        .sleep(instant_sleep)
        .call();

    assert_eq!(
        *calls.borrow(),
        vec![
            "before_1", "before_2", "after_1", "after_2", "exit_1", "exit_2",
        ]
    );
}
