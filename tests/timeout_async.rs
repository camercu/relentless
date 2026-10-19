//! Tests for `.timeout` on the asynchronous path.
//!
//! Mirrors `timeout.rs`: the deadline ends the execution in exhaustion and the
//! recording clock keeps its read and wait seams coherent.

mod common;

use common::{ARBITRARY_ATTEMPTS, CUSTOM_CLOCK_DEADLINE, CUSTOM_CLOCK_STEP_MILLIS};

use common::RecordingAsyncClock as RecordingClock;
use common::block_on;
use core::cell::Cell;
use core::time::Duration;
use relentless::{RetryError, RetryPolicy, stop, wait};

/// `.timeout()` stops the async loop when elapsed time meets or exceeds the budget,
/// even if the stop strategy would allow more attempts.
#[test]
fn async_timeout_stops_loop_when_budget_exceeded() {
    // Allow up to ARBITRARY_ATTEMPTS+10 attempts but set a tight timeout so the
    // loop exits after the first attempt advances the clock past the deadline.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO));
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async {
                    clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
                    Err::<i32, &str>("fail")
                }
            })
            .timeout(CUSTOM_CLOCK_DEADLINE)
            .clock(clock.clone())
            .call(),
    );

    // The timeout is tighter than ARBITRARY_ATTEMPTS+10 would allow, so only 1 attempt runs.
    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// A timeout set on the policy seeds the async builder, so it applies without
/// being repeated at the call site.
#[test]
fn async_policy_timeout_seeds_builder() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO))
        .timeout(CUSTOM_CLOCK_DEADLINE);
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async {
                    clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
                    Err::<i32, &str>("fail")
                }
            })
            .clock(clock.clone())
            .call(),
    );

    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// A builder `.timeout()` replaces the policy's timeout for that async call (it
/// does not take the tighter of the two).
#[test]
fn async_policy_timeout_replaced_by_builder() {
    const LOOSE_BUDGET: Duration = Duration::from_secs(60);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO))
        .timeout(CUSTOM_CLOCK_DEADLINE);
    let clock = RecordingClock::new();
    let call_count = Cell::new(0_u32);

    let result = block_on(
        policy
            .retry_async(|_| {
                call_count.set(call_count.get().saturating_add(1));
                async {
                    clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
                    Err::<i32, &str>("fail")
                }
            })
            .clock(clock.clone())
            .timeout(LOOSE_BUDGET)
            .call(),
    );

    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS + 10);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}
