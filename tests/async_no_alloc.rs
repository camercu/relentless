//! Async API coverage that must remain available without the crate's `alloc` feature.

mod common;

use common::block_on;
use core::cell::{Cell, RefCell};
use core::future::ready;

use relentless::clock::VirtualClock;
use relentless::{AsyncRetryExt, RetryError, RetryPolicy, stop};

const SUCCESS_VALUE: i32 = 42;
const ERROR_VALUE: &str = "fail";
const MAX_ATTEMPTS: u32 = 3;

#[test]
fn policy_async_retry_remains_available_without_alloc() {
    let attempts = Cell::new(0_u32);
    let events: RefCell<Vec<char>> = RefCell::new(Vec::new());
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
            .before_attempt(|_state| events.borrow_mut().push('b'))
            .before_attempt(|_state| events.borrow_mut().push('B'))
            .after_attempt(|_state| events.borrow_mut().push('a'))
            .on_exit(|_state| events.borrow_mut().push('x'))
            .clock(&clock)
            .call(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
    // Hooks must actually fire — including multiple hooks of the same kind,
    // which no longer require `alloc`.
    assert_eq!(
        *events.borrow(),
        vec!['b', 'B', 'a', 'b', 'B', 'a', 'b', 'B', 'a', 'x']
    );
}

#[test]
fn async_retry_ext_remains_available_without_alloc() {
    let attempts = Cell::new(0_u32);
    let events: RefCell<Vec<char>> = RefCell::new(Vec::new());

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
        .before_attempt(|_state| events.borrow_mut().push('b'))
        .after_attempt(|_state| events.borrow_mut().push('a'))
        .on_exit(|_state| events.borrow_mut().push('x'))
        .clock(&clock)
        .call(),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
    // The no-alloc single-hook slots must actually fire, not just typecheck.
    assert_eq!(*events.borrow(), vec!['b', 'a', 'b', 'a', 'b', 'a', 'x']);
}
