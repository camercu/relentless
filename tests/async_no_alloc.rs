//! Async API coverage that must remain available without the crate's `alloc` feature.

mod common;

use common::block_on;
use core::cell::Cell;
use core::future::ready;

use relentless::clock::VirtualClock;
use relentless::{AsyncRetryExt, RetryError, RetryPolicy, stop};

const SUCCESS_VALUE: i32 = 42;
const ERROR_VALUE: &str = "fail";
const MAX_ATTEMPTS: u32 = 3;

#[test]
fn policy_async_retry_remains_available_without_alloc() {
    let attempts = Cell::new(0_u32);
    let policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let clock = VirtualClock::new();
    let result: Result<i32, RetryError<&str, Result<i32, &str>>> = block_on(
        policy
            .retry_async(|_| {
                attempts.set(attempts.get().saturating_add(1));
                if attempts.get() < MAX_ATTEMPTS {
                    ready(Err(ERROR_VALUE))
                } else {
                    ready(Ok(SUCCESS_VALUE))
                }
            })
            .clock(&clock)
            .call(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
}

#[test]
fn async_retry_ext_remains_available_without_alloc() {
    let attempts = Cell::new(0_u32);

    let clock = VirtualClock::new();
    let result: Result<i32, RetryError<&str, Result<i32, &str>>> = block_on(
        (|| {
            attempts.set(attempts.get().saturating_add(1));
            if attempts.get() < MAX_ATTEMPTS {
                ready(Err(ERROR_VALUE))
            } else {
                ready(Ok(SUCCESS_VALUE))
            }
        })
        .retry_async()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .clock(&clock)
        .call(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
}
