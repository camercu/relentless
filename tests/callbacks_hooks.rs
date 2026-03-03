//! Acceptance tests for Callbacks and Hooks (Spec items 7.1–7.7, 12.2–12.4).

use core::cell::Cell;
use core::time::Duration;
use std::cell::RefCell;
use tenacious::RetryPolicy;
use tenacious::{StopReason, on, stop, wait};

const MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(10);

fn instant_sleep(_dur: Duration) {}

#[test]
fn before_attempt_and_after_attempt_fire_at_defined_points() {
    let before_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let after_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());

    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .before_attempt(|state| before_calls.borrow_mut().push(state.attempt))
        .after_attempt(|state: &tenacious::AttemptState<i32, &str>| {
            after_calls.borrow_mut().push(state.attempt);
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(*before_calls.borrow(), vec![1, 2, 3]);
    assert_eq!(*after_calls.borrow(), vec![1, 2, 3]);
}

#[test]
fn after_attempt_runs_after_predicate_evaluation() {
    let predicate_calls = Cell::new(0_u32);
    let after_observed: RefCell<Vec<u32>> = RefCell::new(Vec::new());

    let mut policy = RetryPolicy::new().stop(stop::attempts(2)).when(on::result(
        |_outcome: &Result<i32, &str>| {
            predicate_calls.set(predicate_calls.get().saturating_add(1));
            true
        },
    ));

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .after_attempt(|_state: &tenacious::AttemptState<i32, &str>| {
            after_observed.borrow_mut().push(predicate_calls.get());
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(*after_observed.borrow(), vec![1, 2]);
}

#[test]
fn before_sleep_fires_after_stop_check_with_next_delay_populated() {
    let seen: RefCell<Vec<(u32, Duration)>> = RefCell::new(Vec::new());
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION));

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .before_sleep(|state: &tenacious::AttemptState<i32, &str>| {
            seen.borrow_mut().push((state.attempt, state.next_delay));
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(*seen.borrow(), vec![(1, WAIT_DURATION), (2, WAIT_DURATION)]);
}

#[test]
fn on_exit_fires_once_with_final_state() {
    let exits: RefCell<Vec<(u32, bool, StopReason)>> = RefCell::new(Vec::new());
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .on_exit(|state: &tenacious::ExitState<i32, &str>| {
            exits
                .borrow_mut()
                .push((state.attempt, state.outcome.is_err(), state.reason));
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(
        *exits.borrow(),
        vec![(MAX_ATTEMPTS, true, StopReason::StopCondition)]
    );
}

#[test]
fn on_exit_reports_predicate_accepted_reason() {
    let reasons: RefCell<Vec<StopReason>> = RefCell::new(Vec::new());
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .when(on::error(|err: &&str| *err == "retryable"));

    let _ = policy
        .retry(|| Err::<i32, _>("fatal"))
        .on_exit(|state: &tenacious::ExitState<i32, &str>| {
            reasons.borrow_mut().push(state.reason);
        })
        .sleep(instant_sleep)
        .call();

    assert_eq!(*reasons.borrow(), vec![StopReason::PredicateAccepted]);
}

#[cfg(feature = "alloc")]
#[test]
fn multiple_hooks_of_same_kind_fire_in_registration_order() {
    let calls: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());

    let mut policy = RetryPolicy::new().stop(stop::attempts(1));

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .before_attempt(|_state| calls.borrow_mut().push("before_1"))
        .before_attempt(|_state| calls.borrow_mut().push("before_2"))
        .after_attempt(|_state: &tenacious::AttemptState<i32, &str>| {
            calls.borrow_mut().push("after_1")
        })
        .after_attempt(|_state: &tenacious::AttemptState<i32, &str>| {
            calls.borrow_mut().push("after_2")
        })
        .on_exit(|_state: &tenacious::ExitState<i32, &str>| calls.borrow_mut().push("exit_1"))
        .on_exit(|_state: &tenacious::ExitState<i32, &str>| calls.borrow_mut().push("exit_2"))
        .sleep(instant_sleep)
        .call();

    assert_eq!(
        *calls.borrow(),
        vec![
            "before_1", "before_2", "after_1", "after_2", "exit_1", "exit_2",
        ]
    );
}
