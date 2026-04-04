//! Demonstrates cancellation of a synchronous retry loop.
//!
//! Sync execution cannot be interrupted mid-operation or mid-sleep — the
//! operating system does not provide a way to preempt a blocking call.
//! The retry loop can only observe a cancel condition at attempt boundaries.
//!
//! Two cancellation patterns are shown:
//!
//! 1. `.timeout(dur)` — crate-native deadline that fires at attempt
//!    boundaries and clamps inter-attempt sleep to the remaining budget.
//!    Works without a runtime; internally uses `std::time::Instant`.
//!
//! 2. `AtomicBool` flag — cooperative cancellation from another thread
//!    (e.g., a signal handler or a watchdog). The flag is checked inside
//!    the operation on each attempt.
//!
//!    **Predicate footgun:** with the default `any_error()` predicate, the
//!    cancel error returned by the operation is treated as a transient error
//!    and retried — the loop keeps running even after the flag is set.
//!    Use `.when(error(|e| e.is_transient()))` (or equivalent) so the cancel
//!    error is non-retryable and terminates the loop immediately as
//!    `RetryError::Rejected`.
//!
//! Run with:
//!   cargo run --example sync-cancel

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use relentless::{RetryError, RetryExt, predicate, retry, stop, wait};
use std::sync::Arc;

fn main() {
    // ── Pattern 1: .timeout(dur) ───────────────────────────────────────────
    //
    // The crate's built-in deadline for sync execution. It combines two
    // behaviours:
    //   a) After each completed attempt, checks whether elapsed >= deadline
    //      and stops if so (fires at attempt boundaries, not mid-sleep).
    //   b) Before each inter-attempt sleep, clamps the delay to the remaining
    //      budget so the loop does not overshoot the deadline by a full sleep
    //      duration.
    //
    // This is the right pattern for sync contexts where you cannot race
    // futures. It requires no extra threads or signals.
    //
    // Total budget: 250 ms. Each attempt takes ~0 ms (instant error) +
    // 100 ms sleep = ~100 ms per cycle, so the deadline fires at the
    // attempt boundary after the third attempt, with the final sleep clamped.

    let result = (|| -> Result<&str, &str> { Err("service unavailable") })
        .retry()
        .stop(stop::attempts(100))
        .wait(wait::fixed(Duration::from_millis(100)))
        .timeout(Duration::from_millis(250))
        .call();

    match result {
        Ok(val) => println!("pattern 1 — success: {val}"),
        Err(RetryError::Exhausted { last }) => {
            println!("pattern 1 — deadline exceeded, last: {last:?}")
        }
        Err(e) => println!("pattern 1 — rejected: {e:?}"),
    }

    // ── Pattern 2: AtomicBool flag ─────────────────────────────────────────
    //
    // Cooperative cancellation driven by a flag set from another thread.
    // The operation checks the flag on every attempt and returns a
    // distinguishable cancel error when it is set.
    //
    // ⚠ Predicate footgun: if the predicate retries all errors (the default),
    // the cancel error is retried like any other transient failure — the flag
    // being set does not stop the loop. The fix is a selective predicate that
    // treats the cancel error as non-retryable. The example below uses an
    // enum error type to make the distinction explicit.

    #[derive(Debug, PartialEq)]
    enum ServiceError {
        Transient(&'static str),
        Cancelled,
    }

    let cancelled = Arc::new(AtomicBool::new(false));

    // Simulate a watchdog thread that sets the flag after 3 attempts.
    let flag = Arc::clone(&cancelled);
    let attempts_before_cancel = Arc::new(std::sync::Mutex::new(0_u32));
    let counter = Arc::clone(&attempts_before_cancel);

    let result = retry(|_state| -> Result<(), ServiceError> {
        // Check the cancel flag at the start of every attempt.
        if cancelled.load(Ordering::Relaxed) {
            return Err(ServiceError::Cancelled);
        }

        // Simulate a transient error. After 3 attempts, set the flag to
        // trigger cancellation on the next attempt.
        let mut n = counter.lock().unwrap();
        *n += 1;
        if *n >= 3 {
            flag.store(true, Ordering::Relaxed);
        }
        drop(n);

        Err(ServiceError::Transient("connection refused"))
    })
    .stop(stop::attempts(100))
    .wait(wait::fixed(Duration::ZERO)) // no sleep so the example runs instantly
    // ↓ Only retry transient errors; treat Cancelled as terminal.
    //   Without this, the cancel error would be retried indefinitely.
    .when(predicate::error(|e: &ServiceError| {
        matches!(e, ServiceError::Transient(_))
    }))
    .call();

    match result {
        Ok(val) => println!("pattern 2 — success: {val:?}"),
        Err(RetryError::Rejected { last }) => {
            println!("pattern 2 — cancelled after flag set, error: {last:?}")
        }
        Err(RetryError::Exhausted { last }) => {
            println!("pattern 2 — exhausted: {last:?}")
        }
    }
}
