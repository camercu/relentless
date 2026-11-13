//! Tests for the `.when` and `.until` classifier shortcuts.
//!
//! `.when(f)` tests the error: a matching `Err` retries, any other `Err`
//! aborts, and an `Ok` returns. `.until(f)` tests the `Ok` value: a matching
//! `Ok` returns, and everything else retries.

mod common;

use common::ARBITRARY_ATTEMPTS;
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryPolicy, retry, stop};

const READY: u32 = 10;
const NOT_READY: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestError {
    Retryable,
    Fatal,
}

impl TestError {
    fn is_retryable(self) -> bool {
        self == TestError::Retryable
    }
}

/// GIVEN `.when` over an operation that succeeds
/// WHEN it runs
/// THEN the value returns without a retry
#[test]
fn when_returns_an_ok() {
    let result = retry(|_| Ok::<u32, TestError>(READY))
        .when(|e| e.is_retryable())
        .clock(VirtualClock::new())
        .call();
    assert_eq!(result, Ok(READY));
}

/// GIVEN `.when` over an operation that always fails with a matching error
/// WHEN it runs
/// THEN every attempt retries until the stop strategy exhausts the loop
#[test]
fn when_retries_a_matching_error() {
    let mut attempts = 0;
    let result = retry(|_| {
        attempts += 1;
        Err::<u32, TestError>(TestError::Retryable)
    })
    .when(|e| e.is_retryable())
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .clock(VirtualClock::new())
    .call();
    assert_eq!(
        (result, attempts),
        (
            Err(RetryError::Exhausted {
                last: Err(TestError::Retryable)
            }),
            ARBITRARY_ATTEMPTS
        )
    );
}

/// GIVEN `.when` over an operation that fails with a non-matching error
/// WHEN it runs
/// THEN the loop aborts at once with the bare error
#[test]
fn when_aborts_on_a_non_matching_error() {
    let result = retry(|_| Err::<u32, TestError>(TestError::Fatal))
        .when(|e| e.is_retryable())
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .clock(VirtualClock::new())
        .call();
    assert_eq!(
        result,
        Err(RetryError::Aborted {
            last: TestError::Fatal
        })
    );
}

/// GIVEN `.until` over a poll that is ready on the third attempt
/// WHEN it runs
/// THEN the ready value returns
#[test]
fn until_returns_the_first_matching_ok() {
    let result =
        retry(|state| Ok::<u32, TestError>(if state.attempt >= 3 { READY } else { NOT_READY }))
            .until(|v| *v == READY)
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .clock(VirtualClock::new())
            .call();
    assert_eq!(result, Ok(READY));
}

/// GIVEN `.until` over a poll that fails before it is ready
/// WHEN it runs
/// THEN errors are retried rather than ending the loop
#[test]
fn until_retries_errors() {
    let result = retry(|state| {
        if state.attempt == 1 {
            Err(TestError::Fatal)
        } else {
            Ok(READY)
        }
    })
    .until(|v| *v == READY)
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .clock(VirtualClock::new())
    .call();
    assert_eq!(result, Ok(READY));
}

/// GIVEN `.until` over a poll that is never ready
/// WHEN the stop strategy fires
/// THEN the loop is exhausted with the last outcome
#[test]
fn until_exhausts_with_the_last_unready_value() {
    let result = retry(|_| Ok::<u32, TestError>(NOT_READY))
        .until(|v| *v == READY)
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .clock(VirtualClock::new())
        .call();
    assert_eq!(
        result,
        Err(RetryError::Exhausted {
            last: Ok(NOT_READY)
        })
    );
}

/// GIVEN a policy configured with `.when` and `.until`
/// WHEN each retries an operation
/// THEN they behave as the builder shortcuts do
#[test]
fn policy_shortcuts_match_the_builder() {
    let when = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .when(|e: &TestError| e.is_retryable());
    let until = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .until(|v: &u32| *v == READY);

    let aborted = when
        .retry(|_| Err::<u32, TestError>(TestError::Fatal))
        .clock(VirtualClock::new())
        .call();
    let ready = until
        .retry(|_| Ok::<u32, TestError>(READY))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(
        (aborted, ready),
        (
            Err(RetryError::Aborted {
                last: TestError::Fatal
            }),
            Ok(READY)
        )
    );
}
