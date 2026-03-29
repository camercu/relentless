//! Demonstrates cancellation of an async retry loop.
//!
//! Because `retry_async` returns a plain `Future`, it is cancel-safe:
//! dropping or racing away the future stops the loop immediately, even
//! mid-sleep between attempts. No in-flight state is left dangling.
//!
//! Two common patterns are shown:
//!
//! 1. `tokio::time::timeout` — simplest external deadline.
//! 2. `tokio::select!` — general form, works with any async signal
//!    (shutdown channel, `ctrl_c`, etc.).
//!
//! Run with:
//!   cargo run --example async-cancel --features tokio-sleep

use core::time::Duration;
use tenacious::{retry_async, sleep, stop, wait};

/// Simulates a flaky operation that never succeeds.
async fn flaky_call() -> Result<String, &'static str> {
    Err("transient error")
}

#[tokio::main]
async fn main() {
    // Pattern 1: tokio::time::timeout wraps the future with an external deadline.
    // The retry loop is dropped when the timeout fires, no cleanup required.
    let retry_future = retry_async(|state| {
        println!("pattern 1 — attempt {}", state.attempt);
        flaky_call()
    })
    .stop(stop::attempts(10))
    .wait(wait::fixed(Duration::from_millis(50)))
    .sleep(sleep::tokio());

    match tokio::time::timeout(Duration::from_millis(120), retry_future).await {
        Ok(Ok(val)) => println!("success: {val}"),
        Ok(Err(e)) => println!("retries exhausted: {e:?}"),
        Err(_elapsed) => println!("pattern 1: cancelled — deadline exceeded"),
    }

    // Pattern 2: tokio::select! races the retry future against any async signal —
    // a oneshot channel here, but the same pattern works for ctrl_c or broadcast channels.
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        let _ = shutdown_tx.send(());
    });

    let retry_future = retry_async(|state| {
        println!("pattern 2 — attempt {}", state.attempt);
        flaky_call()
    })
    .stop(stop::attempts(10))
    .wait(wait::fixed(Duration::from_millis(50)))
    .sleep(sleep::tokio());

    tokio::select! {
        result = retry_future => {
            match result {
                Ok(val) => println!("success: {val}"),
                Err(e) => println!("retries exhausted: {e:?}"),
            }
        }
        _ = &mut shutdown_rx => {
            println!("pattern 2: cancelled — shutdown signal received");
        }
    }
}
