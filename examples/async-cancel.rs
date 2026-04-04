//! Demonstrates cancellation of an async retry loop.
//!
//! Because `retry_async` returns a plain `Future`, it is cancel-safe:
//! dropping the future at any `.await` point stops the loop immediately,
//! even mid-sleep or mid-operation. No in-flight state is left dangling.
//! Note that `on_exit` does **not** fire when the future is dropped — only
//! when it runs to a terminal result. Use `Drop` impls on your own types
//! for guaranteed cleanup.
//!
//! ## Why `tokio::time::timeout` instead of the crate's `.timeout()` method?
//!
//! The crate's built-in `.timeout(dur)` is a **boundary-check**: it fires
//! only after each completed attempt and clamps the next inter-attempt sleep.
//! It cannot interrupt an operation or sleep already in progress.
//!
//! `tokio::time::timeout` wraps the entire retry future and races it against
//! a timer on every poll. It can preempt the loop **mid-sleep or
//! mid-operation**, giving a true wall-clock deadline. Use it when you need
//! hard cancellation; use `.timeout()` when you want runtime-agnostic
//! deadline bounding that works in sync contexts too.
//!
//! Two cancellation patterns are shown:
//!
//! 1. `tokio::time::timeout` — simplest hard deadline.
//! 2. `tokio::select!` — general form, works with any async signal:
//!    shutdown channels, `ctrl_c`, broadcast receivers, etc.
//!
//! Run with:
//!   cargo run --example async-cancel --features tokio-sleep

use core::time::Duration;
use relentless::{RetryError, retry_async, sleep, stop, wait};

/// Simulates a flaky network call: takes 40 ms per attempt and never succeeds.
///
/// The artificial delay makes mid-sleep cancellation visible: with a 120 ms
/// deadline and 50 ms inter-attempt waits, the loop is guaranteed to be
/// sleeping when the deadline fires rather than spinning through instant errors.
async fn flaky_network_call(attempt: u32) -> Result<String, String> {
    tokio::time::sleep(Duration::from_millis(40)).await; // simulate I/O latency
    Err(format!("attempt {attempt}: connection refused"))
}

#[tokio::main]
async fn main() {
    // ── Pattern 1: tokio::time::timeout ────────────────────────────────────
    //
    // Wraps the retry future with a hard wall-clock deadline. The future is
    // dropped — mid-sleep if necessary — the instant the timer fires.
    // Total budget: 120 ms. Each attempt takes ~40 ms + 50 ms sleep = ~90 ms,
    // so the deadline fires during the sleep after the first attempt.

    let result = tokio::time::timeout(
        Duration::from_millis(120),
        retry_async(|state| flaky_network_call(state.attempt))
            .stop(stop::attempts(10))
            .wait(wait::fixed(Duration::from_millis(50)))
            .sleep(sleep::tokio()),
    )
    .await;

    match result {
        Ok(Ok(val)) => println!("pattern 1 — success: {val}"),
        Ok(Err(RetryError::Exhausted { last })) => {
            println!("pattern 1 — retries exhausted, last: {last:?}")
        }
        Ok(Err(e)) => println!("pattern 1 — rejected: {e:?}"),
        Err(_elapsed) => println!("pattern 1 — deadline exceeded (cancelled mid-sleep)"),
    }

    // ── Pattern 2: tokio::select! ───────────────────────────────────────────
    //
    // Races the retry future against any async signal. A oneshot channel is
    // used here, but the same arms work with tokio::signal::ctrl_c(),
    // broadcast::Receiver::recv(), or any other async signal source.
    //
    // Unlike pattern 1, the cancellation trigger is not a simple timer —
    // it can fire from application logic (e.g., a user-initiated abort,
    // a peer closing a connection, or a coordinated shutdown sequence).

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    // Simulate an external shutdown signal arriving after 120 ms.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        let _ = shutdown_tx.send(());
        // The spawned task exits here; the channel is closed automatically.
    });

    tokio::select! {
        result = retry_async(|state| flaky_network_call(state.attempt))
            .stop(stop::attempts(10))
            .wait(wait::fixed(Duration::from_millis(50)))
            .sleep(sleep::tokio()) =>
        {
            match result {
                Ok(val) => println!("pattern 2 — success: {val}"),
                Err(e) => println!("pattern 2 — retries exhausted: {e:?}"),
            }
        }
        _ = shutdown_rx => {
            // The retry future is dropped here. on_exit does not fire.
            println!("pattern 2 — shutdown signal received (cancelled mid-sleep)");
        }
    }
}
