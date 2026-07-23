//! Tests for the Predicate trait and built-in predicate factories.
//!
//! Covers the type-parameterization of Predicate<T, E>, the behavior of each factory
//! (error, `any_error`, result, ok), short-circuit semantics of `|`/`&` composition,
//! the `until` inversion wrapper, and the closure blanket impl.

use core::cell::Cell;
use relentless::Predicate;
use relentless::clock::VirtualClock;
use relentless::predicate;

// READY_VALUE is the threshold at which a polling result is "ready".
// ARBITRARY_NOT_READY_VALUE is any value below that threshold.
const READY_VALUE: u32 = 10;
const ARBITRARY_OK_VALUE: u32 = 7;
const ARBITRARY_NOT_READY_VALUE: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestError {
    Retryable,
    Fatal,
}

type TestResult = Result<u32, TestError>;

fn assert_predicate_impl<T, E, P: Predicate<T, E>>(_predicate: &P) {}

#[allow(clippy::unnecessary_wraps)]
fn ok(value: u32) -> TestResult {
    Ok(value)
}

fn err(error: TestError) -> TestResult {
    Err(error)
}

/// Minimal Predicate implementation used to verify the trait contract.
struct RetryOnAnyError;

impl Predicate<String, std::io::Error> for RetryOnAnyError {
    fn should_retry(&self, outcome: &Result<String, std::io::Error>) -> bool {
        outcome.is_err()
    }
}

#[test]
fn predicate_should_retry_returns_true_for_retryable_error() {
    let pred = RetryOnAnyError;
    let err_result: Result<String, std::io::Error> = Err(std::io::Error::other("boom"));
    assert!(pred.should_retry(&err_result));
}

#[test]
fn predicate_should_retry_returns_false_for_ok() {
    let pred = RetryOnAnyError;
    let ok_result: Result<String, std::io::Error> = Ok("success".to_string());
    assert!(!pred.should_retry(&ok_result));
}

/// Implements Predicate for two distinct (T, E) pairs to verify T and E are
/// trait-level type parameters, not method-level.
struct AlwaysRetry;

impl Predicate<u32, String> for AlwaysRetry {
    fn should_retry(&self, _outcome: &Result<u32, String>) -> bool {
        true
    }
}

impl Predicate<bool, i32> for AlwaysRetry {
    fn should_retry(&self, _outcome: &Result<bool, i32>) -> bool {
        true
    }
}

#[test]
fn predicate_type_params_are_on_trait_not_method() {
    let pred = AlwaysRetry;
    let r1: Result<u32, String> = Ok(42);
    let r2: Result<bool, i32> = Err(-1);

    assert!(<AlwaysRetry as Predicate<u32, String>>::should_retry(
        &pred, &r1
    ));
    assert!(<AlwaysRetry as Predicate<bool, i32>>::should_retry(
        &pred, &r2
    ));
}

#[test]
fn predicate_is_callable_multiple_times_through_shared_ref() {
    let pred = RetryOnAnyError;

    let err: Result<String, std::io::Error> = Err(std::io::Error::other("boom"));
    let ok: Result<String, std::io::Error> = Ok("ok".to_string());

    assert!(pred.should_retry(&err));
    assert!(pred.should_retry(&Err(std::io::Error::other("boom2"))));
    assert!(!pred.should_retry(&ok));
}

#[test]
fn predicate_blanket_impl_for_closure() {
    let pred = |outcome: &Result<i32, &str>| outcome.is_err();

    let err: Result<i32, &str> = Err("fail");
    let ok: Result<i32, &str> = Ok(42);

    assert!(Predicate::should_retry(&pred, &err));
    assert!(!Predicate::should_retry(&pred, &ok));
}

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

    let ok_predicate = predicate::ok(|value: &u32| *value < READY_VALUE);
    assert_predicate_impl::<u32, TestError, _>(&ok_predicate);
    assert!(ok_predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!ok_predicate.should_retry(&ok(READY_VALUE)));
}

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

#[test]
fn any_error_retries_on_any_error() {
    let predicate = predicate::any_error();

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(ARBITRARY_OK_VALUE)));
}

#[test]
fn result_can_decide_using_full_outcome() {
    let predicate = predicate::result(|outcome: &TestResult| match outcome {
        Ok(value) => *value < READY_VALUE,
        Err(TestError::Retryable) => true,
        Err(TestError::Fatal) => false,
    });

    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
}

#[test]
fn ok_retries_only_matching_ok_values() {
    let predicate = predicate::ok(|value: &u32| *value < READY_VALUE);

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
    // Composition lives inside a `result` closure now (native `||`).
    let predicate = predicate::result(|o: &TestResult| {
        matches!(o, Err(TestError::Retryable)) || matches!(o, Ok(v) if *v < READY_VALUE)
    });

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
}

#[test]
fn result_can_express_polling_rules_in_one_predicate() {
    let predicate = predicate::result(|outcome: &TestResult| match outcome {
        Ok(value) => *value < READY_VALUE,
        Err(TestError::Retryable) => true,
        Err(TestError::Fatal) => false,
    });

    assert!(predicate.should_retry(&err(TestError::Retryable)));
    assert!(predicate.should_retry(&ok(ARBITRARY_NOT_READY_VALUE)));
    assert!(!predicate.should_retry(&err(TestError::Fatal)));
    assert!(!predicate.should_retry(&ok(READY_VALUE)));
}

#[test]
fn policy_until_sets_predicate() {
    use core::time::Duration;
    use relentless::{RetryPolicy, stop, wait};

    let policy = RetryPolicy::new()
        .stop(stop::attempts(15))
        .wait(wait::fixed(Duration::from_millis(1)))
        .until(predicate::ok(|value: &u32| *value >= READY_VALUE));

    let counter = Cell::new(0u32);
    let result = policy
        .retry(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            Ok::<u32, &str>(n)
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result.unwrap(), READY_VALUE);
    assert_eq!(counter.get(), READY_VALUE);
}

#[test]
fn builder_until_sets_predicate() {
    use core::time::Duration;
    use relentless::{RetryExt, stop, wait};

    let counter = Cell::new(0u32);
    let result = (|| {
        let n = counter.get() + 1;
        counter.set(n);
        Ok::<u32, &str>(n)
    })
    .retry()
    .stop(stop::attempts(15))
    .wait(wait::fixed(Duration::from_millis(1)))
    .until(predicate::ok(|value: &u32| *value >= READY_VALUE))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result.unwrap(), READY_VALUE);
    assert_eq!(counter.get(), READY_VALUE);
}

#[test]
fn until_ok_retries_errors_by_default() {
    // .until(ok(f)) retries errors automatically: ok(f) returns false for Err variants,
    // and until inverts to true (keep retrying).
    use core::time::Duration;
    use relentless::{RetryPolicy, stop, wait};

    let counter = Cell::new(0u32);
    let result = RetryPolicy::new()
        .stop(stop::attempts(5))
        .wait(wait::fixed(Duration::from_millis(1)))
        .until(predicate::ok(|value: &u32| *value >= 3))
        .retry(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            if n < 2 {
                Err::<u32, &str>("transient")
            } else {
                Ok(n)
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result.unwrap(), 3);
    assert_eq!(counter.get(), 3);
}

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
    fn evaluate<P: Predicate<u32, TestError>>(predicate: &P, outcome: TestResult) -> bool {
        predicate.should_retry(&outcome)
    }

    let closure = |outcome: &TestResult| matches!(outcome, Err(TestError::Retryable));

    assert!(evaluate(&closure, Err(TestError::Retryable)));
    assert!(!evaluate(&closure, Err(TestError::Fatal)));
    assert!(!evaluate(&closure, Ok(ARBITRARY_OK_VALUE)));
}

/// Boxed predicates delegate `should_retry` to the boxed impl — all three
/// `dyn` variants (plain, `+ Send`, `+ Send + Sync`).
#[cfg(feature = "alloc")]
#[test]
fn boxed_dyn_predicate_delegates_should_retry() {
    let plain: Box<dyn Predicate<u32, TestError>> = Box::new(predicate::any_error());
    let send: Box<dyn Predicate<u32, TestError> + Send> = Box::new(predicate::any_error());
    let send_sync: Box<dyn Predicate<u32, TestError> + Send + Sync> =
        Box::new(predicate::any_error());

    assert!(plain.should_retry(&err(TestError::Retryable)));
    assert!(!plain.should_retry(&ok(ARBITRARY_OK_VALUE)));
    assert!(send.should_retry(&err(TestError::Retryable)));
    assert!(!send.should_retry(&ok(ARBITRARY_OK_VALUE)));
    assert!(send_sync.should_retry(&err(TestError::Retryable)));
    assert!(!send_sync.should_retry(&ok(ARBITRARY_OK_VALUE)));
}
