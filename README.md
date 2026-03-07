# tenacious

`tenacious` is a reusable retry and polling toolkit for Rust.

It models retry behavior as three composable parts: when to stop retrying
(`Stop`), how long to wait between retries (`Wait`), and what outcomes are
retryable (`Predicate`). Compared with simpler retry helpers, it gives you
policy reuse, polling-aware predicates like `on::any_error`, hooks for
dynamically interacting with retries, cancellation, and stats, all under one API
that works in sync and async code across `std`, `no_std`, and `wasm` targets.

It is inspired by Python's [`tenacity`](https://github.com/jd/tenacity),
especially composable strategy algebra, and Rust's
[`backon`](https://crates.io/crates/backon), especially ergonomic retry
builders.

## Features

- Start simple: `my_fn.retry().call()` with safe defaults.
- Compose policies: combine `Stop`, `Wait`, and `Predicate` with operators.
- Reuse policies across call sites instead of duplicating retry loops.
- Handle polling workflows, not just retry-on-error workflows.
- Add hooks, cancellation, and stats without changing your core retry model.

## Install

```bash
cargo add tenacious
```

Optional runtime adapters and feature flags are in [`Cargo.toml`](./Cargo.toml).

If you want to contribute, see [`CONTRIBUTING.md`](./CONTRIBUTING.md).

---

## Quick start

For full docs and additional examples, see <https://docs.rs/tenacious>.

Behavior spec: [docs/SPEC.md](./docs/SPEC.md)

The HTTP-focused examples below use `reqwest`. The async example uses `tokio`.

### 1) Basic Retry on closures/functions with defaults (`RetryExt`)

Use this for one-off operations. Defaults are: 3 attempts, exponential backoff
from 100ms, retry on any error.

```rust
use tenacious::RetryExt;

fn fetch_health(client: &reqwest::blocking::Client) -> Result<String, reqwest::Error> {
    client
        .get("https://api.example.com/health")
        .send()?
        .error_for_status()?
        .text()
}

let client = reqwest::blocking::Client::new();
let health = (|| fetch_health(&client)).retry().call().unwrap();
```

### 2) Reuse a policy for API calls

Use `RetryPolicy` when multiple operations share retry rules.

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, on, stop, wait};
use reqwest::{Error, StatusCode};

fn fetch_invoice(
    client: &reqwest::blocking::Client,
    invoice_id: &str,
) -> Result<String, Error> {
    client
        .get(format!("https://api.example.com/invoices/{invoice_id}"))
        .send()?
        .error_for_status()?
        .text()
}

fn is_retryable(err: &Error) -> bool {
    err.is_timeout()
        || err.is_connect()
        || err.status().is_some_and(|status| {
            status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
        })
}

let mut api_policy = RetryPolicy::new()
    .stop(stop::attempts(5) | stop::elapsed(Duration::from_secs(2)))
    .wait(wait::fixed(Duration::from_millis(50))
        + wait::exponential(Duration::from_millis(100)))
    .when(on::error(is_retryable));

let client = reqwest::blocking::Client::new();
let invoice_id = api_policy
    .retry(|| fetch_invoice(&client, "inv_123"))
    .call()
    .unwrap();
```

### 3) Poll conditions until ready (`on::until_ready`)

Use this when both transient `Err` values and `Ok("pending")` results should
retry.

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, on, stop, wait};
use reqwest::Error;

fn fetch_export_status(client: &reqwest::blocking::Client) -> Result<String, Error> {
    let body = client
        .get("https://api.example.com/exports/exp_42")
        .send()?
        .error_for_status()?
        .text()?;

    // Example HTTP 200 payloads:
    // { "status": "pending" } or { "status": "success" }
    if body.contains("\"status\":\"success\"") {
        Ok("success".to_string())
    } else {
        Ok("pending".to_string())
    }
}

let mut policy = RetryPolicy::new()
    .stop(stop::attempts(8))
    .wait(wait::fixed(Duration::from_millis(250)))
    .when(on::until_ready(|status: &String| status == "success"));

let client = reqwest::blocking::Client::new();
let final_status = policy.retry(|| fetch_export_status(&client)).call().unwrap();
```

### 4) Use the same model in async code

`retry_async` uses the same stop/wait/predicate model as sync retries.

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, on, stop, wait};

async fn fetch_profile(client: &reqwest::Client) -> Result<String, reqwest::Error> {
    client
        .get("https://api.example.com/profile")
        .send()
        .await?
        .text()
        .await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(wait::exponential(Duration::from_millis(100)))
        .when(on::any_error());

    let profile_json = policy
        .retry_async(|| fetch_profile(&client))
        .sleep(tokio::time::sleep)
        .await
        .unwrap();

    let _ = profile_json;
    Ok(())
}
```

### 5) Add hooks, cancellation, and stats

```rust
use core::sync::atomic::{AtomicBool, Ordering};
use tenacious::{RetryExt, RetryError, on};

fn fetch_control_plane(client: &reqwest::blocking::Client) -> Result<String, reqwest::Error> {
    client
        .get("https://api.example.com/control-plane/health")
        .send()?
        .error_for_status()?
        .text()
}

let cancelled = AtomicBool::new(false);
let client = reqwest::blocking::Client::new();

let (result, stats) = (|| fetch_control_plane(&client))
    .retry()
    .when(on::any_error())
    .after_attempt(|state| {
        if let Err(err) = state.outcome {
            log::warn!(
                "control-plane health check failed on attempt {}: {err}",
                state.attempt
            );
        }
    })
    .sleep(|_dur| {
        // Real world: shutdown signal arrives while waiting for retry.
        cancelled.store(true, Ordering::Relaxed);
    })
    .cancel_on(&cancelled)
    .with_stats()
    .call();

assert!(matches!(result, Err(RetryError::Cancelled { .. })));
assert_eq!(stats.attempts, 1);
```

---

## API surface at a glance

- `RetryPolicy<S, W, P>`: reusable retry configuration.
- `RetryExt` / `AsyncRetryExt`: start from closures and function pointers.
- `stop`: `attempts`, `elapsed`, `before_elapsed`, `never`.
- `wait`: `fixed`, `linear`, `exponential`, plus composition.
- `on`: `any_error`, `error`, `ok`, `result`, `until_ready`.
- `SyncRetry` / `AsyncRetry`: execution builders with lifecycle hooks.
- `RetryError<E, T>` / `RetryResult<T, E>`: terminal outcomes and alias.
- `RetryStats`: aggregate execution stats via `.with_stats()`.

If you prefer concise imports:

```rust
use tenacious::prelude::*;
```

## MSRV

Minimum supported Rust version: `1.85`.

## License

Licensed under either:

- MIT ([LICENSE-MIT.txt](./LICENSE-MIT.txt))
- Apache-2.0 ([LICENSE-APACHE.txt](./LICENSE-APACHE.txt))
