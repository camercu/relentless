//! Acceptance tests for Phase 7: Callbacks and Hooks (Spec items 7.1–7.7).

use core::cell::Cell;
use core::time::Duration;
use std::cell::RefCell;
use tenacious::RetryPolicy;
use tenacious::{on, stop, wait};

const MAX_ATTEMPTS: u32 = 3;
const WAIT_DURATION: Duration = Duration::from_millis(10);

fn instant_sleep(_dur: Duration) {}

#[test]
fn before_attempt_and_after_attempt_fire_at_defined_points() {
    let before_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let after_calls: RefCell<Vec<u32>> = RefCell::new(Vec::new());

    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .before_attempt(|state| before_calls.borrow_mut().push(state.attempt))
        .after_attempt(|state: &tenacious::AttemptState<i32, &str>| {
            after_calls.borrow_mut().push(state.attempt);
        });

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();

    assert_eq!(*before_calls.borrow(), vec![1, 2, 3]);
    assert_eq!(*after_calls.borrow(), vec![1, 2, 3]);
}

#[test]
fn after_attempt_runs_after_predicate_evaluation() {
    let predicate_calls = Cell::new(0_u32);
    let after_observed: RefCell<Vec<u32>> = RefCell::new(Vec::new());

    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .when(on::result(|_outcome: &Result<i32, &str>| {
            predicate_calls.set(predicate_calls.get().saturating_add(1));
            true
        }))
        .after_attempt(|_state: &tenacious::AttemptState<i32, &str>| {
            after_observed.borrow_mut().push(predicate_calls.get());
        });

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();

    assert_eq!(*after_observed.borrow(), vec![1, 2]);
}

#[test]
fn before_sleep_fires_after_stop_check_with_next_delay_populated() {
    let seen: RefCell<Vec<(u32, Duration)>> = RefCell::new(Vec::new());
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION))
        .before_sleep(|state: &tenacious::AttemptState<i32, &str>| {
            seen.borrow_mut().push((state.attempt, state.next_delay));
        });

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();

    assert_eq!(*seen.borrow(), vec![(1, WAIT_DURATION), (2, WAIT_DURATION)]);
}

#[test]
fn on_exhausted_fires_once_with_final_state() {
    let exhausted: RefCell<Vec<(u32, bool)>> = RefCell::new(Vec::new());
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .on_exhausted(|state: &tenacious::AttemptState<i32, &str>| {
            exhausted
                .borrow_mut()
                .push((state.attempt, state.outcome.is_err()));
        });

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();

    assert_eq!(*exhausted.borrow(), vec![(MAX_ATTEMPTS, true)]);
}

#[cfg(feature = "alloc")]
#[test]
fn multiple_hooks_of_same_kind_fire_in_registration_order() {
    let calls: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());

    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(1))
        .before_attempt(|_state| calls.borrow_mut().push("before_1"))
        .before_attempt(|_state| calls.borrow_mut().push("before_2"))
        .after_attempt(|_state: &tenacious::AttemptState<i32, &str>| {
            calls.borrow_mut().push("after_1")
        })
        .after_attempt(|_state: &tenacious::AttemptState<i32, &str>| {
            calls.borrow_mut().push("after_2")
        })
        .on_exhausted(|_state: &tenacious::AttemptState<i32, &str>| {
            calls.borrow_mut().push("exhausted_1")
        })
        .on_exhausted(|_state: &tenacious::AttemptState<i32, &str>| {
            calls.borrow_mut().push("exhausted_2")
        });

    let _ = policy
        .retry(|| Err::<i32, _>("fail"))
        .sleep(instant_sleep)
        .call();

    assert_eq!(
        *calls.borrow(),
        vec![
            "before_1",
            "before_2",
            "after_1",
            "after_2",
            "exhausted_1",
            "exhausted_2",
        ]
    );
}
