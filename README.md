# tenacious

[![crates.io](https://img.shields.io/crates/v/tenacious.svg)](https://crates.io/crates/tenacious)
[![docs.rs](https://docs.rs/tenacious/badge.svg)](https://docs.rs/tenacious)
[![CI](https://github.com/camercu/tenacious/actions/workflows/ci.yml/badge.svg)](https://github.com/camercu/tenacious/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-blue.svg)](#msrv)

Retry and polling for Rust — with composable strategies, policy reuse, and
first-class support for polling workflows where `Ok(_)` doesn't always mean
"done."

Most retry libraries handle the simple case well: call a function, retry on
error, back off. `tenacious` handles that too, but it also handles the cases
those libraries make awkward:

- **Polling**, where `Ok("pending")` means "keep going" and you need
  `.until(predicate::ok(...))` rather than just retrying errors.
- **Policy reuse**, where a single `RetryPolicy` captures your retry rules and
  gets shared across multiple call sites — no duplicated builder chains.
- **Strategy composition**, where `wait::fixed(50ms) + wait::exponential(100ms)`
  and `stop::attempts(5) | stop::elapsed(2s)` express complex behavior in one
  line.
- **Hooks and stats**, where you observe the retry lifecycle (logging, metrics)
  without restructuring your retry logic.

All of this works in sync and async code, across `std`, `no_std`, and `wasm`
targets.

Inspired by Python's [`tenacity`](https://github.com/jd/tenacity) (composable
strategy algebra) and Rust's [`backon`](https://crates.io/crates/backon)
(ergonomic retry builders).

## Install

```bash
cargo add tenacious
```

### Feature flags

| Flag                  | Purpose                                                                                     |
| --------------------- | ------------------------------------------------------------------------------------------- |
| `std` (default)       | `std::thread::sleep` fallback, `Instant` elapsed clock, `std::error::Error` on `RetryError` |
| `alloc`               | Boxed policies, closure elapsed clocks, multiple hooks per point                            |
| `tokio-sleep`         | `sleep::tokio()` async sleep adapter                                                        |
| `embassy-sleep`       | `sleep::embassy()` async sleep adapter                                                      |
| `gloo-timers-sleep`   | `sleep::gloo()` async sleep adapter (wasm32)                                                |
| `futures-timer-sleep` | `sleep::futures_timer()` async sleep adapter                                                |

Async retry does not require `alloc`.

---

## Examples

For full docs, see <https://docs.rs/tenacious>. Behavior spec:
[docs/SPEC.md](./docs/SPEC.md). Runnable examples live in
[`examples/`](./examples).

Sync examples omit `.sleep(...)` because `std` builds fall back to
`std::thread::sleep` automatically. Without `std`, pass an explicit sleeper
before `.call()`.

### 1) Retry with defaults

The `.retry()` extension method is the fastest way to add retries. Defaults: 3
attempts, exponential backoff from 100 ms, retry on any `Err`.

```rust,no_run
use tenacious::RetryExt;

fn fetch_health(client: &reqwest::blocking::Client) -> Result<String, reqwest::Error> {
    client
        .get("https://api.example.com/health")
        .send()?
        .error_for_status()?
        .text()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::new();
    let health = (|| fetch_health(&client)).retry().call()?;
    println!("{health}");
    Ok(())
}
```

### 2) Customized retry

Use the `retry` free function for full control over predicate, wait, and stop
strategies. Here, exponential backoff with full jitter and a cap gives
production-grade behavior in three chained calls.

```rust,no_run
use core::time::Duration;
use tenacious::{Wait, retry, predicate, stop, wait};
use reqwest::{Error, StatusCode};

fn fetch_invoice(
    client: &reqwest::blocking::Client,
    id: &str,
) -> Result<String, Error> {
    client
        .get(format!("https://api.example.com/invoices/{id}"))
        .send()?
        .error_for_status()?
        .text()
}

fn is_retryable(err: &Error) -> bool {
    err.is_timeout()
        || err.is_connect()
        || err.status().is_some_and(|s| {
            s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error()
        })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::new();
    let invoice = retry(|_| fetch_invoice(&client, "inv_123"))
        .when(predicate::error(is_retryable))
        .wait(
            wait::exponential(Duration::from_millis(100))
                .full_jitter()
                .cap(Duration::from_secs(5)),
        )
        .stop(stop::attempts(5))
        .call()?;
    println!("{invoice}");
    Ok(())
}
```

### 3) Reuse a policy across call sites

Define retry rules once with `RetryPolicy`, then apply them wherever needed.
Compose strategies with operators: `+` sums wait durations, `|` stops when
either condition fires.

```rust,no_run
use core::time::Duration;
use tenacious::{RetryPolicy, predicate, stop, wait};
use reqwest::{Error, StatusCode};

fn is_retryable(err: &Error) -> bool {
    err.is_timeout()
        || err.is_connect()
        || err.status().is_some_and(|s| {
            s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error()
        })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_policy = RetryPolicy::new()
        .when(predicate::error(is_retryable))
        // fixed + exponential produces a steeper curve with a floor.
        .wait(
            wait::fixed(Duration::from_millis(50))
                + wait::exponential(Duration::from_millis(100)),
        )
        // Stop after 5 attempts OR 30 seconds, whichever comes first.
        .stop(stop::attempts(5) | stop::elapsed(Duration::from_secs(30)));

    let client = reqwest::blocking::Client::new();

    // Same policy, different operations.
    let health = api_policy
        .retry(|_| {
            client
                .get("https://api.example.com/health")
                .send()?
                .text()
        })
        .call()?;

    let invoice = api_policy
        .retry(|_| {
            client
                .get("https://api.example.com/invoices/inv_123")
                .send()?
                .error_for_status()?
                .text()
        })
        .call()?;

    println!("health: {health}, invoice: {invoice}");
    Ok(())
}
```

### 4) Poll for a condition

Not every retry is about errors. Use `.until(...)` when the operation returns
`Ok` for both "not ready" and "done" states — retry continues until the
predicate is satisfied.

```rust,no_run
use core::time::Duration;
use tenacious::{RetryPolicy, predicate, stop, wait};

#[derive(Debug, PartialEq)]
enum ExportStatus { Pending, Complete }

fn fetch_export_status(
    client: &reqwest::blocking::Client,
) -> Result<ExportStatus, reqwest::Error> {
    // In practice, parse the response body into ExportStatus.
    let _ = client
        .get("https://api.example.com/exports/exp_42")
        .send()?
        .error_for_status()?;
    Ok(ExportStatus::Pending) // placeholder
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::new();
    let status = RetryPolicy::new()
        .until(predicate::ok(|s: &ExportStatus| *s == ExportStatus::Complete))
        .wait(wait::fixed(Duration::from_millis(250)))
        .stop(stop::attempts(8))
        .retry(|_| fetch_export_status(&client))
        .call()?;
    println!("{status:?}");
    Ok(())
}
```

When stop fires during polling, the error is `RetryError::Exhausted` carrying
the last `Ok` value. If you also need to retry selected errors during polling,
use `.until(predicate::result(...))` to match on both:

```rust,no_run
use core::time::Duration;
use tenacious::{RetryPolicy, predicate, stop, wait};

#[derive(Debug)]
enum ExportState { Pending, Success }

#[derive(Debug)]
enum ExportError { Retryable, Fatal }

fn check_export() -> Result<ExportState, ExportError> {
    todo!()
}

fn main() {
    // Retry until a terminal state: Success or Fatal.
    // Pending results and Retryable errors both trigger another attempt.
    let result = RetryPolicy::new()
        .until(predicate::result(
            |outcome: &Result<ExportState, ExportError>| {
                matches!(
                    outcome,
                    Ok(ExportState::Success) | Err(ExportError::Fatal)
                )
            },
        ))
        .wait(wait::fixed(Duration::from_millis(250)))
        .stop(stop::attempts(8))
        .retry(|_| check_export())
        .call();
}
```

### 5) Async retry

Async retry uses the same stop/wait/predicate model. Pass an async sleeper —
here via the `tokio-sleep` feature.

```rust,no_run
use core::time::Duration;
use tenacious::{retry_async, stop, wait};

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
    let profile = retry_async(|_| fetch_profile(&client))
        .wait(wait::exponential(Duration::from_millis(100)))
        .stop(stop::attempts(4))
        .sleep(tenacious::sleep::tokio())
        .await?;
    println!("{profile}");
    Ok(())
}
```

### 6) Hooks, stats & error handling

Attach lifecycle hooks, collect retry statistics, and distinguish how the
retry loop terminated.

```rust,no_run
use tenacious::{retry, RetryError};

fn fetch_control_plane(
    client: &reqwest::blocking::Client,
) -> Result<String, reqwest::Error> {
    client
        .get("https://api.example.com/control-plane/health")
        .send()?
        .error_for_status()?
        .text()
}

fn main() {
    let client = reqwest::blocking::Client::new();

    let (result, stats) = retry(|_| fetch_control_plane(&client))
        .before_attempt(|state| {
            if state.attempt > 1 {
                log::info!("retry attempt {}", state.attempt);
            }
        })
        .after_attempt(|state| {
            if let Err(err) = state.outcome {
                log::warn!("attempt {} failed: {err}", state.attempt);
            }
        })
        .with_stats()
        .call();

    match result {
        Ok(body) => {
            println!("OK after {} attempts: {body}", stats.attempts);
        }
        Err(RetryError::Exhausted { last }) => {
            // Stop strategy fired while predicate still wanted to retry.
            // `last` is the Result from the final attempt.
            eprintln!("gave up after {} attempts: {last:?}", stats.attempts);
        }
        Err(RetryError::Rejected { last }) => {
            // Predicate decided this error is not retryable.
            eprintln!("non-retryable error: {last}");
        }
    }
}
```

---

## API surface at a glance

| Area               | Items                                                                                         |
| ------------------ | --------------------------------------------------------------------------------------------- |
| Entry points       | `retry`, `retry_async` (free functions); `RetryExt`, `AsyncRetryExt` (extension traits)       |
| Policy             | `RetryPolicy<S, W, P>` with `.retry()`, `.retry_async()`                                      |
| Stop strategies    | `stop::attempts`, `stop::elapsed`, `stop::never`                                              |
| Wait strategies    | `wait::fixed`, `wait::linear`, `wait::exponential`, `wait::decorrelated_jitter`               |
| Predicates         | `predicate::any_error`, `predicate::error`, `predicate::ok`, `predicate::result`              |
| Execution builders | `SyncRetryBuilder` / `AsyncRetryBuilder` with hooks, stats, timeout                           |
| Terminal types     | `RetryError<T, E>` (`Exhausted`, `Rejected`), `RetryResult<T, E>`, `RetryStats`, `StopReason` |

Builder methods follow the order: **when/until** -> **wait** -> **stop** ->
sleep -> hooks -> stats -> call.

If you need builder types in signatures, use `tenacious::builders::*`.

## MSRV

Minimum supported Rust version: **1.85**.

## Contributing

See [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## Release notes

For user-facing changes, see the [changelog](./CHANGELOG.md).

## License

Licensed under either:

- MIT ([LICENSE-MIT](./LICENSE-MIT))
- Apache-2.0 ([LICENSE-APACHE](./LICENSE-APACHE))
