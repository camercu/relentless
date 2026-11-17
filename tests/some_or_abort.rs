//! Tests for the `some_or_abort` classifier over `Result<Option<T>, E>`.
//!
//! `Ok(Some(v))` returns `v`, `Ok(None)` retries, and `Err(e)` aborts with
//! `e` (SPEC 3.4.15).

mod common;

use common::{ARBITRARY_ATTEMPTS, ARBITRARY_ERROR, ARBITRARY_VALUE};
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryPolicy, retry, some_or_abort, stop};

const READY_ON_ATTEMPT: u32 = 2;

/// GIVEN an operation that is not ready on its first attempt
/// WHEN it runs with `.decide(some_or_abort)`
/// THEN `Ok(None)` retries and the later `Ok(Some(v))` returns `v`
#[test]
fn some_or_abort_retries_none_until_some() {
    let result = retry(|state| {
        if state.attempt < READY_ON_ATTEMPT {
            Ok::<_, &str>(None)
        } else {
            Ok(Some(ARBITRARY_VALUE))
        }
    })
    .decide(some_or_abort)
    .clock(VirtualClock::new())
    .call();
    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

/// GIVEN an operation that fails with a hard error
/// WHEN it runs with `.decide(some_or_abort)`
/// THEN the loop aborts on the first attempt with that error
#[test]
fn some_or_abort_aborts_on_err() {
    let mut attempts = 0;
    let result = retry(|_| {
        attempts += 1;
        Err::<Option<i32>, _>(ARBITRARY_ERROR)
    })
    .decide(some_or_abort)
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .clock(VirtualClock::new())
    .call();
    assert_eq!(
        (result, attempts),
        (
            Err(RetryError::Aborted {
                last: ARBITRARY_ERROR
            }),
            1
        )
    );
}

/// GIVEN a reusable policy configured with `.decide(some_or_abort)`
/// WHEN it runs an operation that is ready on its first attempt
/// THEN the value returns, with no annotation on the policy-first path
#[test]
fn some_or_abort_works_policy_first() {
    let policy = RetryPolicy::new().decide(some_or_abort);
    let result = policy
        .retry(|_| Ok::<_, &str>(Some(ARBITRARY_VALUE)))
        .clock(VirtualClock::new())
        .call();
    assert_eq!(result, Ok(ARBITRARY_VALUE));
}
