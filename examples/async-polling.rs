//! Polling an async status endpoint until it reports "ready", using `.until(...)`
//! to keep retrying while the value is not yet the one you want.
//!
//! Run: `cargo run --example async-polling --features tokio-clock`
use core::cell::Cell;
use core::time::Duration;
use relentless::clock::TokioClock;
use relentless::{RetryPolicy, stop, wait};

#[tokio::main]
async fn main() {
    // Represents a remote status endpoint that returns "deploying" until the service
    // is up. Using a counter keeps the example self-contained.
    let checks_remaining = Cell::new(3_u32);
    let check_deploy_status = |_| {
        let status = if checks_remaining.get() > 0 {
            checks_remaining.set(checks_remaining.get() - 1);
            "deploying"
        } else {
            "ready"
        };
        async move { Ok::<_, &str>(status) }
    };

    // `.until()` tests the Ok value: retry until it matches. A policy has no
    // operation to infer from, so the closure names its parameter type.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(10))
        .wait(wait::fixed(Duration::from_millis(25)))
        .until(|status: &&str| *status == "ready");

    let result = policy
        .retry_async(check_deploy_status)
        .clock(TokioClock::new())
        .call()
        .await;

    assert_eq!(result, Ok("ready"));
    println!("polled until the deploy reported: {result:?}");
    println!("status checks that returned \"deploying\" first: 3");
}
