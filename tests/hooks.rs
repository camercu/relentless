//! Tests for `before_attempt`, `after_attempt`, and `on_exit` hooks.
//!
//! Verifies call ordering relative to the operation and classification, the
//! `Exit` view delivered to `on_exit`, and that multiple hooks of the same kind
//! fire in registration order.

use core::cell::Cell;
use core::time::Duration;
use relentless::RetryPolicy;
use relentless::clock::VirtualClock;
use relentless::{StopReason, predicate, stop, wait};
use std::cell::RefCell;

const MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(10);

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
        .after_attempt(|state: &relentless::AttemptState<Result<i32, &str>>| {
            events.borrow_mut().push(('a', state.attempt));
        })
        .clock(VirtualClock::new())
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
        .clock(VirtualClock::new())
        .call();

    assert_eq!(
        *delays.borrow(),
        vec![None, Some(WAIT_DURATION), Some(WAIT_DURATION)]
    );
}

#[test]
fn after_attempt_runs_before_classification() {
    // ADR-6: after_attempt fires once per attempt *before* the classifier
    // consumes the outcome, so it observes the classifier not-yet-run for the
    // current attempt (counter is 0 then 1, never having advanced for this
    // attempt yet).
    let classifier_calls = Cell::new(0_u32);
    let after_observed: RefCell<Vec<u32>> = RefCell::new(Vec::new());

    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .when(predicate::result(|_outcome: &Result<i32, &str>| {
            classifier_calls.set(classifier_calls.get().saturating_add(1));
            true
        }));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .after_attempt(|_state: &relentless::AttemptState<Result<i32, &str>>| {
            after_observed.borrow_mut().push(classifier_calls.get());
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(*after_observed.borrow(), vec![0, 1]);
}

/// SPEC 8.3
#[test]
fn on_exit_fires_once_with_final_state() {
    let exits: RefCell<Vec<(u32, bool, StopReason)>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .on_exit(|exit: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            let outcome_is_err = match exit {
                relentless::Exit::Returned { .. } => false,
                relentless::Exit::Exhausted { last, .. } => last.is_err(),
                _ => true,
            };
            exits
                .borrow_mut()
                .push((exit.attempt(), outcome_is_err, exit.stop_reason()));
        })
        .clock(VirtualClock::new())
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
        .on_exit(|exit: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            reasons.borrow_mut().push(exit.stop_reason());
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(*reasons.borrow(), vec![StopReason::Aborted]);
}

#[test]
fn multiple_hooks_of_same_kind_fire_in_registration_order() {
    let calls: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());

    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .before_attempt(|_state| calls.borrow_mut().push("before_1"))
        .before_attempt(|_state| calls.borrow_mut().push("before_2"))
        .after_attempt(|_state: &relentless::AttemptState<Result<i32, &str>>| {
            calls.borrow_mut().push("after_1");
        })
        .after_attempt(|_state: &relentless::AttemptState<Result<i32, &str>>| {
            calls.borrow_mut().push("after_2");
        })
        .on_exit(|_e: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            calls.borrow_mut().push("exit_1");
        })
        .on_exit(|_e: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            calls.borrow_mut().push("exit_2");
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(
        *calls.borrow(),
        vec![
            "before_1", "before_2", "after_1", "after_2", "exit_1", "exit_2",
        ]
    );
}
