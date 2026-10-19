//! Tests for `.timeout` on the synchronous path.
//!
//! A timeout is a deadline on elapsed time for the whole execution: it acts as
//! an extra stop condition (ending in exhaustion) and clamps the final delay so
//! the last attempt starts at the deadline.

mod common;

use common::{ARBITRARY_ATTEMPTS, CUSTOM_CLOCK_DEADLINE, CUSTOM_CLOCK_STEP_MILLIS};
#[cfg(feature = "std")]
use common::{
    ARBITRARY_DELAY, ARBITRARY_ERROR, ARBITRARY_VALUE, CLOCK_STEP_MILLIS, TIMEOUT_DEADLINE,
};

use common::RecordingClock;
use core::cell::Cell;
use core::time::Duration;
#[cfg(feature = "std")]
use relentless::RetryExt;
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryPolicy, StopReason, Verdict, retry, stop, wait};

/// `.timeout()` stops the loop when elapsed time meets or exceeds the budget,
/// even if the stop strategy would allow more attempts.
#[test]
fn timeout_stops_loop_when_budget_exceeded() {
    // Allow up to ARBITRARY_ATTEMPTS+10 attempts but set a tight timeout so the
    // loop exits after the first attempt advances the clock past the deadline.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO));
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("fail")
        })
        .clock(&clock)
        .timeout(CUSTOM_CLOCK_DEADLINE)
        .call();

    // The timeout is tighter than ARBITRARY_ATTEMPTS would allow, so only 1 attempt runs.
    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// A timeout set on the policy seeds the builder, so it applies without being
/// repeated at the call site.
#[test]
fn policy_timeout_seeds_builder() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO))
        .timeout(CUSTOM_CLOCK_DEADLINE);
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("fail")
        })
        .clock(&clock)
        .call();

    // The policy timeout is tighter than the stop strategy, so only 1 attempt runs.
    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// The policy's timeout survives *every* strategy combinator applied after it
/// (SPEC 5.10). Set `.timeout()` first, then run it through a classifier
/// combinator (`.decide()`) as well as `.stop()`/`.wait()`; each rebuilds the
/// policy and must copy the seeded budget forward. The `.decide()` link covers
/// the classifier field-copy path (`RetryPolicy::decide`), not just stop/wait.
#[test]
fn policy_timeout_survives_later_combinators() {
    let policy = RetryPolicy::new()
        .timeout(CUSTOM_CLOCK_DEADLINE)
        .decide(|o: Result<i32, &'static str>| match o {
            Ok(v) => Verdict::Return(v),
            // Inert here (the op only yields "fail"); the arm pins the abort
            // type so the classifier's `Verdict` is fully inferred.
            Err("fatal") => Verdict::Abort("boom"),
            Err(_) => Verdict::Retry(o),
        })
        .stop(stop::attempts(ARBITRARY_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO));
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("fail")
        })
        .clock(&clock)
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// A policy's seeded timeout survives `.boxed()` type-erasure (SPEC 5.10): the
/// timeout is a plain field, not an erased strategy, so boxing stop and wait
/// keeps it. `boxed_local` shares the identical field-copy path.
#[cfg(feature = "alloc")]
#[test]
fn policy_timeout_survives_boxed() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO))
        .timeout(CUSTOM_CLOCK_DEADLINE)
        .boxed();
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("fail")
        })
        .clock(&clock)
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// A builder `.timeout()` replaces the policy's timeout for that call (it does
/// not take the tighter of the two): a loose builder timeout lets the loop run
/// past the policy's tight budget.
#[test]
fn policy_timeout_replaced_by_builder() {
    const LOOSE_BUDGET: Duration = Duration::from_secs(60);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO))
        .timeout(CUSTOM_CLOCK_DEADLINE);
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("fail")
        })
        .clock(&clock)
        .timeout(LOOSE_BUDGET)
        .call();

    // The loose builder timeout wins over the policy's tight one, so the stop
    // strategy bounds the loop instead.
    assert_eq!(call_count.get(), ARBITRARY_ATTEMPTS + 10);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// `.timeout(Duration::ZERO)` permits exactly one attempt: the elapsed boundary
/// (`elapsed >= budget`) is met (`0 >= 0`) the instant the first attempt
/// completes, even without any virtual time passing. There is no "disabled" or
/// "infinite" timeout value. Locks the rustdoc note on `Retry::timeout`.
#[test]
fn timeout_zero_permits_exactly_one_attempt() {
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS + 10))
        .wait(wait::fixed(Duration::ZERO))
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            Err::<i32, _>("fail")
        })
        .clock(&clock)
        .timeout(Duration::ZERO)
        .call();

    // Zero budget stops after the first attempt without any clock advance, so
    // the much larger attempt cap never applies.
    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// 11.4.4
#[test]
fn timeout_stop_reason_is_exhausted() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(100))
        .wait(wait::fixed(Duration::ZERO));
    let clock = VirtualClock::new();

    let (result, stats) = policy
        .retry(|_| {
            clock.advance(Duration::from_millis(10));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .timeout(Duration::from_millis(5))
        .with_stats()
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
}

/// 11.4.1, 11.4.2, 11.4.3
#[test]
fn timeout_clamps_delay_to_remaining_budget() {
    let clock = RecordingClock::new();
    let policy = RetryPolicy::new()
        .stop(stop::attempts(10))
        .wait(wait::fixed(Duration::from_millis(100)));

    let result = policy
        .retry(|_| {
            // Advance clock past timeout on first attempt.
            clock.advance(Duration::from_millis(10));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .timeout(Duration::from_millis(5))
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    // The wait must be skipped entirely: the clamped delay is zero.
    assert_eq!(clock.waits(), Vec::new());
}

/// 11.4.2
///
/// Unlike the zero-budget case above, here each attempt leaves budget on the
/// table, so the clock must observe the *clamped* delays — not the wait
/// strategy's raw output.
#[test]
fn timeout_clamps_delay_to_partial_remaining_budget() {
    const TIMEOUT: Duration = Duration::from_millis(100);
    const RAW_DELAY: Duration = Duration::from_millis(80);
    const OP_RUNTIME_MILLIS: u64 = 30;

    let clock = RecordingClock::new();
    let policy = RetryPolicy::new()
        .stop(stop::attempts(10))
        .wait(wait::fixed(RAW_DELAY));

    let result = policy
        .retry(|_| {
            clock.advance(Duration::from_millis(OP_RUNTIME_MILLIS));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .timeout(TIMEOUT)
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    // Attempt 1 leaves elapsed at 30 ms → remaining budget 70 ms, below the
    // raw 80 ms delay, so the wait is clamped to 70 ms. The wait itself
    // consumes that budget (the clock is coherent: waiting advances elapsed),
    // so attempt 2 ends at 130 ms ≥ 100 ms and the loop stops at the deadline
    // with one final attempt (SPEC 11.4).
    assert_eq!(clock.waits(), vec![Duration::from_millis(70)]);
}

/// 11.1.2
#[test]
#[cfg(feature = "std")]
fn timeout_with_std_uses_instant_clock() {
    // Just verify it compiles and runs correctly with the default SystemClock.
    let result = RetryPolicy::new()
        .stop(stop::attempts(1))
        .wait(wait::fixed(Duration::ZERO))
        .retry(|_| Ok::<i32, &str>(ARBITRARY_VALUE))
        .timeout(Duration::from_secs(60))
        .call();
    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

#[cfg(feature = "std")]
#[test]
fn retry_ext_timeout_stops_on_deadline() {
    let clock = VirtualClock::new();

    let result = (|| {
        clock.advance(Duration::from_millis(CLOCK_STEP_MILLIS));
        Err::<i32, &str>(ARBITRARY_ERROR)
    })
    .retry()
    .stop(stop::attempts(100))
    .wait(wait::fixed(ARBITRARY_DELAY))
    .clock(&clock)
    .timeout(TIMEOUT_DEADLINE)
    .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

#[test]
fn timeout_stops_the_loop_once_the_budget_is_exceeded() {
    // 10ms waits under a 25ms budget: attempts run at t=0,10,20,25; the last
    // wait is clamped to 5ms to land exactly on the deadline, where the next
    // boundary check exhausts the loop.
    let (result, stats) = retry(|_| Err::<i32, &str>("x"))
        .stop(stop::attempts(100))
        .wait(wait::fixed(Duration::from_millis(10)))
        .timeout(Duration::from_millis(25))
        .clock(VirtualClock::new())
        .with_stats()
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.attempts, 4);
    assert_eq!(stats.total_elapsed, Duration::from_millis(25));
    assert_eq!(stats.total_wait, Duration::from_millis(25));
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
}
