//! Acceptance tests for Retry Predicates (Spec items 4.1–4.8).
//!
//! These tests verify:
//! - `on` module factory functions exist and produce predicates (4.1)
//! - `on::error` retries only matching `Err` values (4.2)
//! - `on::any_error` retries on any `Err` (4.3)
//! - `on::result` sees the full outcome (4.4)
//! - `on::ok` retries only matching `Ok` values (4.5)
//! - Predicate composition with `|` retries when either side retries (4.6)
//! - Predicate composition with `&` retries only when both sides retry (4.7)
//! - Closures satisfy `Predicate<T, E>` via blanket impl (4.8)
//!
//! Spec items 4.9 and 4.10 concern execution-engine behavior and are validated
//! in execution tests where the retry loop exists.

use core::cell::Cell;
use tenacious::Predicate;
use tenacious::on;

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
// 4.1: `on` module factory functions
// ---------------------------------------------------------------------------

#[test]
fn factory_functions_return_predicates() {
    let mut any_error = on::any_error();
    assert_predicate_impl::<u32, TestError, _>(&any_error);
    assert!(any_error.should_retry(&err(TestError::Fatal)));
    assert!(!any_error.should_retry(&ok(ARBITRARY_OK_VALUE)));

    let mut error = on::error(|error: &TestError| matches!(error, TestError::Retryable));
    assert_predicate_impl::<u32, TestError, _>(&error);
    assert!(error.should_retry(&err(TestError::Retryable)));
    assert!(!error.should_retry(&err(TestError::Fatal)));

    let mut result = on::result(|outcome: &TestResult| outcome.is_err());
    assert_predicate_impl::<u32, TestError, _>(&result);
    assert!(result.should_retry(&err(TestError::Retryable)));
    assert!(!result.should_retry(&ok(ARBITRARY_OK_VALUE)));

    let mut ok_predicate = on::ok(|value: &u32| *value < READY_THRESHOLD);
    assert_predicate_impl::<u32, TestError, _>(&ok_predicate);
    assert!(ok_predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!ok_predicate.should_retry(&ok(READY_VALUE)));

    let mut until_ready = on::until_ready(|value: &u32| *value >= READY_THRESHOLD);
    assert_predicate_impl::<u32, TestError, _>(&until_ready);
    assert!(until_ready.should_retry(&err(TestError::Fatal)));
    assert!(!until_ready.should_retry(&ok(READY_VALUE)));
}

// ---------------------------------------------------------------------------
// 4.2: `on::error`
// ---------------------------------------------------------------------------

#[test]
fn error_retries_only_matching_errors() {
    let mut predicate = on::error(|error: &TestError| matches!(error, TestError::Retryable));

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
}

#[test]
fn error_does_not_call_matcher_for_ok_values() {
    let calls = Cell::new(0_u32);
    let mut predicate = on::error(|_error: &TestError| {
        calls.set(calls.get().saturating_add(1));
        true
    });

    assert!(!predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
    assert_eq!(calls.get(), 0);
}

// ---------------------------------------------------------------------------
// 4.3: `on::any_error`
// ---------------------------------------------------------------------------

#[test]
fn any_error_retries_on_any_error() {
    let mut predicate = on::any_error();

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
}

// ---------------------------------------------------------------------------
// 4.4: `on::result`
// ---------------------------------------------------------------------------

#[test]
fn result_can_decide_using_full_outcome() {
    let mut predicate = on::result(|outcome: &TestResult| match outcome {
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
// 4.5: `on::ok`
// ---------------------------------------------------------------------------

#[test]
fn ok_retries_only_matching_ok_values() {
    let mut predicate = on::ok(|value: &u32| *value < READY_THRESHOLD);

    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Retryable)));
}

#[test]
fn ok_does_not_call_matcher_for_error_values() {
    let calls = Cell::new(0_u32);
    let mut predicate = on::ok(|_value: &u32| {
        calls.set(calls.get().saturating_add(1));
        true
    });

    assert!(!predicate.should_retry(&err(TestError::Retryable)));
    assert_eq!(calls.get(), 0);
}

#[test]
fn until_ready_retries_until_ready_and_retries_errors() {
    let mut predicate = on::until_ready(|value: &u32| *value >= READY_THRESHOLD);

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&err(TestError::Fatal)));
    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
}

#[test]
fn until_ready_composes_with_error_matchers() {
    let mut predicate = on::until_ready(|value: &u32| *value >= READY_THRESHOLD)
        & on::error(|error: &TestError| matches!(error, TestError::Retryable))
        | on::ok(|value: &u32| *value < READY_THRESHOLD);

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
}

#[test]
fn until_ready_matches_any_error_or_inverse_ok_composition() {
    let mut until_ready = on::until_ready(|value: &u32| *value >= READY_THRESHOLD);
    let mut composed = on::any_error() | on::ok(|value: &u32| *value < READY_THRESHOLD);

    assert_eq!(
        until_ready.should_retry(&err(TestError::Retryable)),
        composed.should_retry(&err(TestError::Retryable))
    );
    assert_eq!(
        until_ready.should_retry(&err(TestError::Fatal)),
        composed.should_retry(&err(TestError::Fatal))
    );
    assert_eq!(
        until_ready.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)),
        composed.should_retry(&ok(ARBITRARY_NOT_READY_VALUE))
    );
    assert_eq!(
        until_ready.should_retry(&ok(READY_VALUE)),
        composed.should_retry(&ok(READY_VALUE))
    );
}

// ---------------------------------------------------------------------------
// 4.6: Predicate composition with `|`
// ---------------------------------------------------------------------------

#[test]
fn predicate_or_retries_when_either_side_retries() {
    let mut predicate = on::error(|error: &TestError| matches!(error, TestError::Retryable))
        | on::ok(|value: &u32| *value < READY_THRESHOLD);

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
}

#[test]
fn predicate_or_supports_chained_composition() {
    let mut predicate = on::error(|error: &TestError| matches!(error, TestError::Retryable))
        | on::ok(|value: &u32| *value < READY_THRESHOLD)
        | on::result(|outcome: &TestResult| matches!(outcome, Ok(value) if *value == READY_VALUE));

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(predicate.should_retry(&ok(READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
}

#[test]
fn predicate_or_short_circuits_when_left_retries() {
    let right_calls = Cell::new(0_u32);
    let mut predicate = on::result(|_outcome: &TestResult| true)
        | on::result(|_outcome: &TestResult| {
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
    let mut predicate =
        on::any_error() & on::error(|error: &TestError| matches!(error, TestError::Retryable));

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
}

#[test]
fn predicate_and_supports_chained_composition() {
    let mut predicate = on::result(|outcome: &TestResult| outcome.is_ok())
        & on::ok(|value: &u32| *value < READY_THRESHOLD)
        & on::result(|outcome: &TestResult| matches!(outcome, Ok(value) if *value > 0));

    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Retryable)));
}

#[test]
fn predicate_and_short_circuits_when_left_rejects() {
    let right_calls = Cell::new(0_u32);
    let mut predicate = on::result(|_outcome: &TestResult| false)
        & on::result(|_outcome: &TestResult| {
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
    let mut predicate = |outcome: &TestResult| outcome.is_err();

    assert!(Predicate::should_retry(
        &mut predicate,
        &err(TestError::Retryable)
    ));
    assert!(!Predicate::should_retry(
        &mut predicate,
        &ok(ARBITRARY_OK_VALUE)
    ));
}

#[test]
fn closure_predicate_can_be_used_in_generic_context() {
    fn evaluate<P: Predicate<u32, TestError>>(mut predicate: P, outcome: TestResult) -> bool {
        predicate.should_retry(&outcome)
    }

    let closure = |outcome: &TestResult| matches!(outcome, Err(TestError::Retryable));

    assert!(evaluate(closure, Err(TestError::Retryable)));
    assert!(!evaluate(closure, Err(TestError::Fatal)));
    assert!(!evaluate(closure, Ok(ARBITRARY_OK_VALUE)));
}
