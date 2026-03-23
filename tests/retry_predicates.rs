//! Acceptance tests for Retry Predicates (Spec items 4.1–4.8).
//!
//! These tests verify:
//! - `predicate` module factory functions exist and produce predicates (4.1)
//! - `predicate::error` retries only matching `Err` values (4.2)
//! - `predicate::any_error` retries on any `Err` (4.3)
//! - `predicate::result` sees the full outcome (4.4)
//! - `predicate::ok` retries only matching `Ok` values (4.5)
//! - Predicate composition with `|` retries when either side retries (4.6)
//! - Predicate composition with `&` retries only when both sides retry (4.7)
//! - Closures satisfy `Predicate<T, E>` via blanket impl (4.8)
//!
//! Polling behavior that combines `error`, `ok`, and `result` is validated here
//! and in execution tests where the retry loop exists.

use core::cell::Cell;
use tenacious::Predicate;
use tenacious::predicate;

// ---------------------------------------------------------------------------
// Test constants
// ---------------------------------------------------------------------------

/// Value used to represent a "ready" result.
const READY_VALUE: u32 = 10;

/// Values below this threshold are considered "not ready yet".
const READY_THRESHOLD: u32 = READY_VALUE;

/// Arbitrary success value for tests that only need a valid `Ok` payload.
const ARBITRARY_OK_VALUE: u32 = 7;

/// Arbitrary value that does not satisfy readiness conditions.
const ARBITRARY_NOT_READY_VALUE: u32 = 3;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestError {
    Retryable,
    Fatal,
}

type TestResult = Result<u32, TestError>;

fn assert_predicate_impl<T, E, P: Predicate<T, E>>(_predicate: &P) {}

fn ok(value: u32) -> TestResult {
    Ok(value)
}

fn err(error: TestError) -> TestResult {
    Err(error)
}

// ---------------------------------------------------------------------------
// 4.1: `predicate` module factory functions
// ---------------------------------------------------------------------------

#[test]
fn factory_functions_return_predicates() {
    let any_error = predicate::any_error();
    assert_predicate_impl::<u32, TestError, _>(&any_error);
    assert!(any_error.should_retry(&err(TestError::Fatal)));
    assert!(!any_error.should_retry(&ok(ARBITRARY_OK_VALUE)));

    let error = predicate::error(|error: &TestError| matches!(error, TestError::Retryable));
    assert_predicate_impl::<u32, TestError, _>(&error);
    assert!(error.should_retry(&err(TestError::Retryable)));
    assert!(!error.should_retry(&err(TestError::Fatal)));

    let result = predicate::result(|outcome: &TestResult| outcome.is_err());
    assert_predicate_impl::<u32, TestError, _>(&result);
    assert!(result.should_retry(&err(TestError::Retryable)));
    assert!(!result.should_retry(&ok(ARBITRARY_OK_VALUE)));

    let ok_predicate = predicate::ok(|value: &u32| *value < READY_THRESHOLD);
    assert_predicate_impl::<u32, TestError, _>(&ok_predicate);
    assert!(ok_predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!ok_predicate.should_retry(&ok(READY_VALUE)));
}

// ---------------------------------------------------------------------------
// 4.2: `predicate::error`
// ---------------------------------------------------------------------------

#[test]
fn error_retries_only_matching_errors() {
    let predicate = predicate::error(|error: &TestError| matches!(error, TestError::Retryable));

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
}

#[test]
fn error_does_not_call_matcher_for_ok_values() {
    let calls = Cell::new(0_u32);
    let predicate = predicate::error(|_error: &TestError| {
        calls.set(calls.get().saturating_add(1));
        true
    });

    assert!(!predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
    assert_eq!(calls.get(), 0);
}

// ---------------------------------------------------------------------------
// 4.3: `predicate::any_error`
// ---------------------------------------------------------------------------

#[test]
fn any_error_retries_on_any_error() {
    let predicate = predicate::any_error();

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
}

// ---------------------------------------------------------------------------
// 4.4: `predicate::result`
// ---------------------------------------------------------------------------

#[test]
fn result_can_decide_using_full_outcome() {
    let predicate = predicate::result(|outcome: &TestResult| match outcome {
        Ok(value) => *value < READY_THRESHOLD,
        Err(TestError::Retryable) => true,
        Err(TestError::Fatal) => false,
    });

    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
}

// ---------------------------------------------------------------------------
// 4.5: `predicate::ok`
// ---------------------------------------------------------------------------

#[test]
fn ok_retries_only_matching_ok_values() {
    let predicate = predicate::ok(|value: &u32| *value < READY_THRESHOLD);

    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Retryable)));
}

#[test]
fn ok_does_not_call_matcher_for_error_values() {
    let calls = Cell::new(0_u32);
    let predicate = predicate::ok(|_value: &u32| {
        calls.set(calls.get().saturating_add(1));
        true
    });

    assert!(!predicate.should_retry(&err(TestError::Retryable)));
    assert_eq!(calls.get(), 0);
}

#[test]
fn polling_can_retry_selected_errors_and_not_ready_values() {
    let predicate = predicate::error(|error: &TestError| matches!(error, TestError::Retryable))
        | predicate::ok(|value: &u32| *value < READY_THRESHOLD);

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
}

#[test]
fn result_can_express_polling_rules_in_one_predicate() {
    let predicate = predicate::result(|outcome: &TestResult| match outcome {
        Ok(value) => *value < READY_THRESHOLD,
        Err(TestError::Retryable) => true,
        Err(TestError::Fatal) => false,
    });

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
}

// ---------------------------------------------------------------------------
// 4.6: Predicate composition with `|`
// ---------------------------------------------------------------------------

#[test]
fn predicate_or_retries_when_either_side_retries() {
    let predicate = predicate::error(|error: &TestError| matches!(error, TestError::Retryable))
        | predicate::ok(|value: &u32| *value < READY_THRESHOLD);

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
}

#[test]
fn predicate_or_supports_chained_composition() {
    let predicate = predicate::error(|error: &TestError| matches!(error, TestError::Retryable))
        | predicate::ok(|value: &u32| *value < READY_THRESHOLD)
        | predicate::result(
            |outcome: &TestResult| matches!(outcome, Ok(value) if *value == READY_VALUE),
        );

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(predicate.should_retry(&ok(READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
}

#[test]
fn predicate_or_short_circuits_when_left_retries() {
    let right_calls = Cell::new(0_u32);
    let predicate = predicate::result(|_outcome: &TestResult| true)
        | predicate::result(|_outcome: &TestResult| {
            right_calls.set(right_calls.get().saturating_add(1));
            false
        });

    assert!(predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
    assert_eq!(right_calls.get(), 0);
}

// ---------------------------------------------------------------------------
// 4.7: Predicate composition with `&`
// ---------------------------------------------------------------------------

#[test]
fn predicate_and_retries_only_when_both_sides_retry() {
    let predicate = predicate::any_error()
        & predicate::error(|error: &TestError| matches!(error, TestError::Retryable));

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
}

#[test]
fn predicate_and_supports_chained_composition() {
    let predicate = predicate::result(|outcome: &TestResult| outcome.is_ok())
        & predicate::ok(|value: &u32| *value < READY_THRESHOLD)
        & predicate::result(|outcome: &TestResult| matches!(outcome, Ok(value) if *value > 0));

    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Retryable)));
}

#[test]
fn predicate_and_short_circuits_when_left_rejects() {
    let right_calls = Cell::new(0_u32);
    let predicate = predicate::result(|_outcome: &TestResult| false)
        & predicate::result(|_outcome: &TestResult| {
            right_calls.set(right_calls.get().saturating_add(1));
            true
        });

    assert!(!predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
    assert_eq!(right_calls.get(), 0);
}

// ---------------------------------------------------------------------------
// 4.8: Blanket impl for `Fn(&Result<T, E>) -> bool`
// ---------------------------------------------------------------------------

#[test]
fn closure_implements_predicate_trait() {
    let predicate = |outcome: &TestResult| outcome.is_err();

    assert!(Predicate::should_retry(
        &predicate,
        &err(TestError::Retryable)
    ));
    assert!(!Predicate::should_retry(
        &predicate,
        &ok(ARBITRARY_OK_VALUE)
    ));
}

#[test]
fn closure_predicate_can_be_used_in_generic_context() {
    fn evaluate<P: Predicate<u32, TestError>>(predicate: P, outcome: TestResult) -> bool {
        predicate.should_retry(&outcome)
    }

    let closure = |outcome: &TestResult| matches!(outcome, Err(TestError::Retryable));

    assert!(evaluate(closure, Err(TestError::Retryable)));
    assert!(!evaluate(closure, Err(TestError::Fatal)));
    assert!(!evaluate(closure, Ok(ARBITRARY_OK_VALUE)));
}
