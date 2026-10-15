//! Tests for `Outcome` classification and the default classifier.
//!
//! `Result`, `Option` and `ControlFlow` classify themselves; the default
//! classifier delegates to `Outcome::classify` and hands a retried outcome
//! back whole.

use core::ops::ControlFlow;

use relentless::{Decide, DefaultClassifier, Outcome, Verdict};

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
