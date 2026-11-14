//! Tests for the `.when` and `.until` classifier shortcuts.
//!
//! `.when(f)` tests the error: a matching `Err` retries, any other `Err`
//! aborts, and an `Ok` returns. `.until(f)` tests the `Ok` value: a matching
//! `Ok` returns, and everything else retries.

mod common;

use common::{ARBITRARY_ATTEMPTS, ARBITRARY_VALUE};
use core::cell::Cell;
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryPolicy, retry, stop, wait};

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

#[test]
fn retry_classifier_evaluated_before_stop() {
    // The classifier is consulted before the stop strategy. A non-negative Ok value
    // satisfies `.until` immediately, so the loop exits even though attempts
    // remain — stop::attempts is never reached.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .until(|v: &i32| *v >= 0);

    let result = policy
        .retry(|_| Ok::<_, &str>(ARBITRARY_VALUE))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

#[test]
fn exhausted_returned_when_until_never_holds() {
    // `.until` retries while the Ok value fails the condition. When stop
    // fires with an Ok value still being rejected, the error variant must be
    // Exhausted with last=Ok(_), not Aborted (which `.when` yields only for Err).
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .until(|v: &i32| *v >= 0);

    let result = policy
        .retry(|_| Ok::<_, &str>(-1_i32))
        .clock(VirtualClock::new())
        .call();

    match result {
        Err(RetryError::Exhausted { last }) => {
            assert_eq!(last, Ok(-1));
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

#[test]
fn until_ok_retries_through_transient_errors_while_polling() {
    // `.until(ok(is_ready))` retries on everything except a matching `Ok`, so a
    // poll that errors keeps polling rather than surfacing the error. This locks
    // the semantics documented on `.until` (retry on everything except the match)
    // and under `.until`, `Err` is retried, not terminal.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .until(|v: &i32| *v >= ARBITRARY_VALUE);

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            let next_call = call_count.get().saturating_add(1);
            call_count.set(next_call);
            match next_call {
                1 | 2 => Err("transient"),
                _ => Ok(ARBITRARY_VALUE),
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(
        call_count.get(),
        3,
        "Err retries; only a matching Ok returns"
    );
}

#[test]
fn when_rejecting_an_err_aborts_immediately() {
    // When `.when` rejects an Err, the loop exits immediately with
    // RetryError::Aborted rather than waiting for the stop strategy to fire.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .when(|e: &&str| *e == "retryable");

    let call_count = Cell::new(0_u32);
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get() + 1);
            Err::<i32, _>("fatal")
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(call_count.get(), 1);
    match result {
        Err(RetryError::Aborted { last }) => {
            assert_eq!(last, "fatal");
        }
        other => panic!("expected Aborted with last=\"fatal\", got {other:?}"),
    }
}

/// 7.2.4
#[test]
fn until_returns_before_stop_fires() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(1))
        .until(|v: &i32| *v >= 0);

    let result = policy
        .retry(|_| Ok::<i32, &str>(ARBITRARY_VALUE))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

/// 5.4
#[test]
fn when_and_until_last_call_wins() {
    // `.until` after `.when` replaces the classifier: the last call wins.
    // `.until` retries every Err and returns once the Ok value is done.
    let call_count = Cell::new(0_i32);
    let result = RetryPolicy::new()
        .stop(stop::attempts(5))
        .wait(wait::fixed(Duration::ZERO))
        .when(|_e: &&str| false) // would reject everything (no retries)
        .until(|v: &i32| *v >= 3) // last call wins: retry until Ok >= 3
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            Ok::<i32, &str>(n)
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Ok(3));
    assert_eq!(call_count.get(), 3);
}
