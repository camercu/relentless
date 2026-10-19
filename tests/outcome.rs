//! Tests for `Outcome` classification and the default classifier.
//!
//! `Result`, `Option` and `ControlFlow` classify themselves; the default
//! classifier delegates to `Outcome::classify` and hands a retried outcome
//! back whole. End-to-end sync runs cover `Option` and `ControlFlow` ops with
//! and without a `.decide` override.
mod common;

use common::{ARBITRARY_ATTEMPTS, ARBITRARY_DELAY, ARBITRARY_VALUE, CONTINUE_PAYLOAD};

use core::cell::Cell;
use core::ops::ControlFlow;
use std::rc::Rc;

use relentless::clock::VirtualClock;
use relentless::{Decide, DefaultClassifier, Outcome, RetryError, Verdict, retry, stop, wait};

#[test]
fn result_ok_classifies_as_return() {
    let outcome: Result<i32, &str> = Ok(7);
    assert_eq!(outcome.classify(), Verdict::Return(7));
}

#[test]
fn result_err_classifies_as_retry_carrying_the_whole_outcome() {
    let outcome: Result<i32, &str> = Err("boom");
    assert_eq!(outcome.classify(), Verdict::Retry(Err("boom")));
}

#[test]
fn default_classifier_delegates_to_outcome() {
    let classifier = DefaultClassifier;
    assert_eq!(classifier.decide(Ok::<i32, &str>(1)), Verdict::Return(1));
    assert_eq!(
        classifier.decide(Err::<i32, &str>("x")),
        Verdict::Retry(Err("x"))
    );
}

#[test]
fn option_some_classifies_as_return() {
    assert_eq!(Some(7).classify(), Verdict::Return(7));
}

#[test]
fn option_none_classifies_as_retry_carrying_the_whole_outcome() {
    assert_eq!(None::<i32>.classify(), Verdict::Retry(None));
}

#[test]
fn default_classifier_delegates_to_option() {
    let classifier = DefaultClassifier;
    assert_eq!(classifier.decide(Some(1)), Verdict::Return(1));
    assert_eq!(classifier.decide(None::<i32>), Verdict::Retry(None));
}

#[test]
fn control_flow_break_classifies_as_return() {
    let outcome: ControlFlow<&str, i32> = ControlFlow::Break("done");
    assert_eq!(outcome.classify(), Verdict::Return("done"));
}

#[test]
fn control_flow_continue_classifies_as_retry_carrying_the_whole_outcome() {
    let outcome: ControlFlow<&str, i32> = ControlFlow::Continue(5);
    assert_eq!(outcome.classify(), Verdict::Retry(ControlFlow::Continue(5)));
}

#[test]
fn default_classifier_delegates_to_control_flow() {
    let classifier = DefaultClassifier;
    let brk: ControlFlow<&str, i32> = ControlFlow::Break("x");
    let cont: ControlFlow<&str, i32> = ControlFlow::Continue(1);
    assert_eq!(classifier.decide(brk), Verdict::Return("x"));
    assert_eq!(
        classifier.decide(cont),
        Verdict::Retry(ControlFlow::Continue(1))
    );
}

#[test]
fn option_outcome_retries_through_none_then_returns_some() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);

    // An `Option`-returning op drives the default classifier with no `.decide`:
    // `None` retries, `Some(v)` delivers `v`.
    let result = retry(move |_| {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        (attempts_ref.get() >= ARBITRARY_ATTEMPTS).then_some(ARBITRARY_VALUE)
    })
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(ARBITRARY_DELAY))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(attempts.get(), ARBITRARY_ATTEMPTS);
}

#[test]
fn option_outcome_all_none_exhausts_carrying_none() {
    // No `Some` ever arrives, so the loop exhausts its stop strategy; the last
    // outcome carried out is `None` (Option's abort type is `Infallible`).
    let result = retry(|_| None::<i32>)
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(ARBITRARY_DELAY))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Err(RetryError::Exhausted { last: None }));
}

#[test]
fn option_op_decide_override_projects_a_custom_verdict() {
    // The default classifier is not the only path for an `Option` op: `.decide`
    // composes with it, and a non-`Infallible` abort type flows through. Here a
    // sentinel `Some(0)` is treated as fatal rather than as success.
    let result = retry(|_| Some(0_i32))
        .decide(|outcome: Option<i32>| match outcome {
            Some(0) => Verdict::Abort("sentinel"),
            Some(v) => Verdict::Return(v),
            None => Verdict::Retry(None),
        })
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(ARBITRARY_DELAY))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Err(RetryError::Aborted { last: "sentinel" }));
}

#[test]
fn control_flow_outcome_retries_through_continue_then_breaks() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);

    // A `ControlFlow` op drives the default classifier with no `.decide`:
    // `Continue(_)` retries, `Break(v)` delivers `v`.
    let result = retry(move |_| {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        if attempts_ref.get() >= ARBITRARY_ATTEMPTS {
            ControlFlow::Break(ARBITRARY_VALUE)
        } else {
            ControlFlow::Continue(())
        }
    })
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(ARBITRARY_DELAY))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(attempts.get(), ARBITRARY_ATTEMPTS);
}

#[test]
fn control_flow_outcome_all_continue_exhausts_carrying_continue() {
    let result = retry(|_| ControlFlow::<i32, i32>::Continue(CONTINUE_PAYLOAD))
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(ARBITRARY_DELAY))
        .clock(VirtualClock::new())
        .call();

    let err = result.unwrap_err();
    // The non-unit `Continue` payload survives into the exhausted error.
    assert_eq!(
        err,
        RetryError::Exhausted {
            last: ControlFlow::Continue(CONTINUE_PAYLOAD)
        }
    );
    // The engine-produced ControlFlow error is usable end to end (Display/last).
    assert_eq!(err.to_string(), "retries exhausted");
    assert_eq!(
        err.into_last(),
        Some(ControlFlow::Continue(CONTINUE_PAYLOAD))
    );
}
