# tenacious

`tenacious` is a retry and polling toolkit for Rust.

It models retry behavior as three composable parts: what outcomes are
retryable (`Predicate`), how long to wait between retries (`Wait`), and when
to stop retrying (`Stop`). Compared with simpler retry helpers, it gives you
policy reuse, polling-oriented workflows, hooks for observing retry lifecycle and stats, all under one API that works in sync and async code
across `std`, `no_std`, and `wasm` targets.

It is inspired by Python's [`tenacity`](https://github.com/jd/tenacity),
especially composable strategy algebra, and Rust's
[`backon`](https://crates.io/crates/backon), especially ergonomic retry
builders.

## Features

- Start simple: `retry(|_| my_fn()).call()` with safe defaults.
- Compose policies: combine `Stop`, `Wait`, and `Predicate` with operators.
- Reuse policies across call sites instead of duplicating retry loops.
- Handle polling workflows, not just retry-on-error workflows.
- Add hooks and stats without changing your core retry model.

## Install

```bash
cargo add tenacious
```

Feature flags are listed in [`Cargo.toml`](./Cargo.toml). Key flags:

| Flag | Purpose |
|------|---------|
| `std` (default) | `std::thread::sleep` fallback, `Instant` elapsed clock, `std::error::Error` on `RetryError` |
| `alloc` | Boxed policies, closure elapsed clocks, multiple hooks per point |
| `tokio-sleep` | `sleep::tokio()` async sleep adapter |
| `embassy-sleep` | `sleep::embassy()` async sleep adapter |
| `gloo-timers-sleep` | `sleep::gloo()` async sleep adapter (wasm32) |
| `futures-timer-sleep` | `sleep::futures_timer()` async sleep adapter |
| `jitter` | Jitter strategies and `Wait` jitter decorator methods |
| `serde` | Serialize/deserialize for `RetryPolicy` |

Async retry does not require `alloc`.

If you want to contribute, see [`CONTRIBUTING.md`](./CONTRIBUTING.md).

---

## Quick start

For full docs and additional examples, see <https://docs.rs/tenacious>.

Behavior spec: [docs/SPEC.md](./docs/SPEC.md)

Runnable examples live in [`examples/`](./examples).

The HTTP-focused examples below use `reqwest`. The async example uses `tokio`.
Sync examples omit `.sleep(...)` because `std` builds use
`std::thread::sleep` by default. Without `std`, pass an explicit blocking
sleeper before `.call()`.

### 1) Retry with defaults

Use the `retry` free function for one-off operations. Defaults are: 3
attempts, exponential backoff from 100ms, retry on any error.

```rust
use tenacious::retry;

fn fetch_health(client: &reqwest::blocking::Client) -> Result<String, reqwest::Error> {
    client
        .get("https://api.example.com/health")
        .send()?
        .error_for_status()?
        .text()
}

fn run() -> Result<String, tenacious::RetryError<String, reqwest::Error>> {
    let client = reqwest::blocking::Client::new();
    let health = retry(|_| fetch_health(&client)).call()?;
    Ok(health)
}
```

### 2) Reuse a policy for API calls

Use `RetryPolicy` when multiple operations share retry rules.

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, predicate, stop, wait};
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

fn run() -> Result<String, tenacious::RetryError<String, Error>> {
    let api_policy = RetryPolicy::new()
        .when(predicate::error(is_retryable))
        .wait(wait::fixed(Duration::from_millis(50))
            + wait::exponential(Duration::from_millis(100)))
        .stop(stop::attempts(5) | stop::elapsed(Duration::from_secs(2)));
    let client = reqwest::blocking::Client::new();
    let invoice = api_policy
        .retry(|_| fetch_invoice(&client, "inv_123"))
        .call()?;
    Ok(invoice)
}
```

### 3) Poll for a condition

Use `predicate::ok(...)` when the operation returns `Ok` for both "not ready"
and "ready" states. Retry while the predicate returns `true`; any `Err` stops
immediately.

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, predicate, stop, wait};

fn fetch_export_status(
    client: &reqwest::blocking::Client,
) -> Result<String, reqwest::Error> {
    client
        .get("https://api.example.com/exports/exp_42")
        .send()?
        .error_for_status()?
        .text()
}

fn run() -> Result<String, tenacious::RetryError<String, reqwest::Error>> {
    let client = reqwest::blocking::Client::new();
    let final_status = RetryPolicy::new()
        .until(predicate::ok(|status: &String| status.contains("\"status\":\"success\"")))
        .wait(wait::fixed(Duration::from_millis(250)))
        .stop(stop::attempts(8))
        .retry(|_| fetch_export_status(&client))
        .call()?;
    Ok(final_status)
}
```

When stop fires during polling, the error is `RetryError::Exhausted` carrying
the last `Ok` value. If you also need to retry selected errors during polling,
compose predicates with `predicate::result(...)`:

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, predicate, stop, wait};

#[derive(Debug)]
enum ExportState { Pending, Success }

#[derive(Debug)]
enum ExportError { RetryableTransport, Fatal }

fn fetch_export_status() -> Result<ExportState, ExportError> {
    unimplemented!()
}

let policy = RetryPolicy::new()
    .when(predicate::result(|outcome: &Result<ExportState, ExportError>| {
        matches!(
            outcome,
            Ok(ExportState::Pending) | Err(ExportError::RetryableTransport)
        )
    }))
    .wait(wait::fixed(Duration::from_millis(250)))
    .stop(stop::attempts(8));

let _ = policy.retry(fetch_export_status).sleep(|_| {}).call();
```

### 4) Async retry

`retry_async` uses the same stop/wait/predicate model. Async execution always
requires an explicit sleeper. The example below uses Tokio (`tokio-sleep`
feature).

```rust
use core::time::Duration;
use tenacious::{predicate, retry_async, stop, wait};

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
    let profile_json = retry_async(|_| fetch_profile(&client))
        .when(predicate::any_error())
        .wait(wait::exponential(Duration::from_millis(100)))
        .stop(stop::attempts(4))
        .sleep(tokio::time::sleep)
        .await?;
    let _ = profile_json;
    Ok(())
}
```

### 5) Hooks and stats

```rust
use tenacious::{retry, predicate};

fn fetch_control_plane(client: &reqwest::blocking::Client) -> Result<String, reqwest::Error> {
    client
        .get("https://api.example.com/control-plane/health")
        .send()?
        .error_for_status()?
        .text()
}

let client = reqwest::blocking::Client::new();

let (result, stats) = retry(|_| fetch_control_plane(&client))
    .when(predicate::any_error())
    .after_attempt(|state| {
        if let Err(err) = state.outcome {
            log::warn!(
                "control-plane health check failed on attempt {}: {err}",
                state.attempt
            );
        }
    })
    .with_stats()
    .call();

let _ = result;
assert!(stats.attempts >= 1);
```

---

## API surface at a glance

| Area | Items |
|------|-------|
| Entry points | `retry`, `retry_async` (free functions); `RetryExt`, `AsyncRetryExt` (extension traits) |
| Policy | `RetryPolicy<S, W, P>` with `.retry()`, `.retry_async()` |
| Stop strategies | `stop::attempts`, `stop::elapsed`, `stop::never` |
| Wait strategies | `wait::fixed`, `wait::linear`, `wait::exponential`, `wait::decorrelated_jitter` (jitter feature) |
| Predicates | `predicate::any_error`, `predicate::error`, `predicate::ok`, `predicate::result` |
| Execution builders | `SyncRetryBuilder` / `AsyncRetryBuilder` with hooks, stats, timeout |
| Terminal types | `RetryError<T, E>` (`Exhausted`, `Rejected`), `RetryResult<T, E>`, `RetryStats`, `StopReason` |

Builder methods follow the order: **when/until** -> **wait** -> **stop** ->
sleep -> hooks -> stats -> call.

If you need builder types in signatures, use `tenacious::builders::*`.

For concise imports:

```rust
use tenacious::prelude::*;
```

The prelude exports core traits, entry traits, terminal types, and the most
common constructors (`attempts`, `elapsed`, `never`, `fixed`, `linear`,
`exponential`, `any_error`, `error`, `ok`, `result`). It leaves modules and
runtime-specific helpers on their original paths so wildcard imports stay
predictable.

## MSRV

Minimum supported Rust version: **1.85**.

## Release notes

For user-facing changes, see the [changelog](./CHANGELOG.md).

## License

Licensed under either:

- MIT ([LICENSE-MIT.txt](./LICENSE-MIT.txt))
- Apache-2.0 ([LICENSE-APACHE.txt](./LICENSE-APACHE.txt))
