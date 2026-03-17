# tenacious

`tenacious` is a reusable retry and polling toolkit for Rust.

It models retry behavior as three composable parts: when to stop retrying
(`Stop`), how long to wait between retries (`Wait`), and what outcomes are
retryable (`Predicate`). Compared with simpler retry helpers, it gives you
policy reuse, polling-oriented predicate composition, hooks for dynamically
interacting with retries, cancellation, and stats, all under one API that
works in sync and async code across `std`, `no_std`, and `wasm` targets.

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
Async retry does not require the crate's `alloc` feature. `alloc` is only
needed for boxed policies, `Arc<AtomicBool>` cancellation, and registering
multiple hooks of the same kind on one execution builder.

If you want to contribute, see [`CONTRIBUTING.md`](./CONTRIBUTING.md).

---

## Quick start

For full docs and additional examples, see <https://docs.rs/tenacious>.

Behavior spec: [docs/SPEC.md](./docs/SPEC.md)

Runnable examples live in [`examples/`](./examples).

The HTTP-focused examples below use `reqwest`. The async example uses `tokio`.
Sync examples omit `.sleep(...)` because `std` builds use
`std::thread::sleep` by default. If you build without `std`, pass an explicit
blocking sleeper before `.call()`.

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

fn run() -> Result<(), tenacious::RetryError<reqwest::Error, String>> {
    let client = reqwest::blocking::Client::new();
    let health = (|| fetch_health(&client)).retry().call()?;
    let _ = health;
    Ok(())
}
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

fn run() -> Result<(), tenacious::RetryError<Error, String>> {
    let api_policy = RetryPolicy::new()
        .stop(stop::attempts(5) | stop::elapsed(Duration::from_secs(2)))
        .wait(wait::fixed(Duration::from_millis(50))
            + wait::exponential(Duration::from_millis(100)))
        .when(on::error(is_retryable));
    let client = reqwest::blocking::Client::new();
    let invoice_id = api_policy
        .retry_clone(|| fetch_invoice(&client, "inv_123"))
        .call()?;
    let _ = invoice_id;
    Ok(())
}
```

### 3) Poll conditions with `on::ok`

Use `on::ok` when the operation returns successful values for both "not ready"
and "ready" states, and any `Err` must stop immediately.

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, on, stop, wait};

fn fetch_export_status(
    client: &reqwest::blocking::Client,
) -> Result<String, reqwest::Error> {
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

fn run() -> Result<(), tenacious::RetryError<reqwest::Error, String>> {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(8))
        .wait(wait::fixed(Duration::from_millis(250)))
        .when(on::ok(|status: &String| status != "success"));
    let client = reqwest::blocking::Client::new();
    let final_status = policy
        .retry_clone(|| fetch_export_status(&client))
        .call()?;
    let _ = final_status;
    Ok(())
}
```

When you use `on::ok(...)`, the final stop-triggered error is
`RetryError::ConditionNotMet` because the loop was still retrying `Ok` values.
With `on::any_error()` or `on::error(...)`, the same stop trigger on `Err`
produces `RetryError::Exhausted`.

If selected errors are also retryable, compose the predicates directly or use
`on::result(...)`:

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, on, stop, wait};

#[derive(Debug)]
enum ExportState {
    Pending,
    Success,
}

#[derive(Debug)]
enum ExportError {
    RetryableTransport,
    Fatal,
}

fn fetch_export_status() -> Result<ExportState, ExportError> {
    unimplemented!()
}

let policy = RetryPolicy::new()
    .stop(stop::attempts(8))
    .wait(wait::fixed(Duration::from_millis(250)))
    .when(on::result(|outcome: &Result<ExportState, ExportError>| {
        matches!(
            outcome,
            Ok(ExportState::Pending) | Err(ExportError::RetryableTransport)
        )
    }));

let _ = policy.retry_clone(fetch_export_status).call();
```

### 4) Use the same model in async code

`retry_async` uses the same stop/wait/predicate model as sync retries. Async
execution always requires an explicit sleeper. The example below uses Tokio, so
it requires the `tokio-sleep` feature.

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

    let policy = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(wait::exponential(Duration::from_millis(100)))
        .when(on::any_error());

    let profile_json = policy
        .retry_async_clone(|| fetch_profile(&client))
        .sleep(tokio::time::sleep)
        .await
        ?;

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
- `on`: `any_error`, `error`, `ok`, `result`.
- `SyncRetry` / `AsyncRetry`: execution builders with lifecycle hooks.
- `RetryError<E, T>` / `RetryResult<T, E>`: terminal outcomes and alias.
- `RetryStats`: aggregate execution stats via `.with_stats()`.

If you need explicit builder types in signatures, prefer
`tenacious::builders::*` with the standard `retry()` / `retry_async()` entry
points.

If you prefer concise imports:

```rust
use tenacious::prelude::*;
```

The prelude exports the common builder entry traits, terminal types, and the
most common constructors: `attempts`, `before_elapsed`, `elapsed`, `never`,
`fixed`, `linear`, `exponential`, `any_error`, `error`, `ok`, and `result`.

It intentionally leaves modules and runtime-specific helpers such as
`sleep::tokio()` on their original paths so wildcard imports stay predictable.

## MSRV

Minimum supported Rust version: `1.85`.

## Release notes

For user-facing changes, see the [changelog](./CHANGELOG.md).

## License

Licensed under either:

- MIT ([LICENSE-MIT.txt](./LICENSE-MIT.txt))
- Apache-2.0 ([LICENSE-APACHE.txt](./LICENSE-APACHE.txt))
