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

The `.retry()` extension trait is the fastest way to add retries. Defaults: 3
attempts, exponential backoff from 100 ms, retry on any `Err`.

```rust,no_run
use tenacious::RetryExt;

fn fetch_config() -> Result<String, std::io::Error> {
    std::fs::read_to_string("/etc/app/config.json")
}

let config = fetch_config.retry().call();
```

### 2) Customized retry

The `retry` free function is equivalent to the extension trait, with the added
ability to capture retry loop state. Both the free function and extension trait
give full control over which errors to retry, how long to wait, and when to
stop.

```rust,no_run
use core::time::Duration;
use tenacious::{Wait, retry, predicate, stop, wait};

let body = retry(|state| {
    println!("attempt {}", state.attempt);
    reqwest::blocking::get("https://api.example.com/data")?.text()
})
.when(predicate::error(|e: &reqwest::Error| e.is_timeout()))
.wait(
    wait::exponential(Duration::from_millis(200))
        .full_jitter()
        .cap(Duration::from_secs(5)),
)
.stop(stop::attempts(10))
.timeout(Duration::from_secs(30))
.call();
```

### 3) Reuse a policy across call sites

`RetryPolicy` captures retry rules once. Compose wait strategies with `+` and
stop strategies with `|` or `&`.

```rust,no_run
use core::time::Duration;
use tenacious::{RetryPolicy, stop, wait};

fn check_health() -> Result<String, std::io::Error> { todo!() }
fn fetch_invoice(id: &str) -> Result<String, std::io::Error> { todo!() }

let policy = RetryPolicy::new()
    .wait(
        wait::fixed(Duration::from_millis(50))
            + wait::exponential(Duration::from_millis(100)),
    )
    .stop(stop::attempts(5) | stop::elapsed(Duration::from_secs(30)));

// Same policy, different operations.
let health = policy.retry(|_| check_health()).call();
let invoice = policy.retry(|_| fetch_invoice("inv_123")).call();
```

### 4) Poll for a condition

Use `.until(...)` when `Ok` doesn't mean "done." Retry continues until the
predicate is satisfied.

```rust,no_run
use tenacious::{RetryPolicy, predicate};

#[derive(Debug, PartialEq)]
enum Status { Pending, Done }

fn poll_status() -> Result<Status, std::io::Error> { todo!() }

let result = RetryPolicy::new()
    .until(predicate::ok(|s: &Status| *s == Status::Done))
    .retry(|_| poll_status())
    .call();
```

To also retry selected errors during polling, use `predicate::result`:

```rust,no_run
use tenacious::{RetryPolicy, predicate};

#[derive(Debug)]
enum Status { Pending, Done }
#[derive(Debug)]
enum Error { Retryable, Fatal }

fn poll_job() -> Result<Status, Error> { todo!() }

// Retry until Done or Fatal; keep going on Pending or Retryable.
let result = RetryPolicy::new()
    .until(predicate::result(|outcome: &Result<Status, Error>| {
        matches!(outcome, Ok(Status::Done) | Err(Error::Fatal))
    }))
    .retry(|_| poll_job())
    .call();
```

### 5) Async retry

Pass an async sleep adapter — here via the `tokio-sleep` feature.

```rust,no_run
use tenacious::retry_async;

async fn fetch(url: &str) -> Result<String, reqwest::Error> {
    reqwest::get(url).await?.text().await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = retry_async(|_| fetch("https://api.example.com/data"))
        .sleep(tenacious::sleep::tokio())
        .await?;
    Ok(())
}
```

### 6) Hooks & stats

```rust
use tenacious::retry;

let (result, stats) = retry(|_| Ok::<_, &str>("done"))
    .before_attempt(|state| {
        if state.attempt > 1 {
            println!("retrying (attempt {})", state.attempt);
        }
    })
    .after_attempt(|state| {
        if let Err(e) = state.outcome {
            eprintln!("attempt {} failed: {e}", state.attempt);
        }
    })
    .with_stats()
    .call();

println!("attempts: {}, total wait: {:?}", stats.attempts, stats.total_wait);
```

### 7) Error handling

```rust,no_run
use tenacious::{retry, RetryError};

match retry(|_| Err::<(), &str>("boom")).call() {
    Ok(val) => println!("success: {val:?}"),
    Err(RetryError::Exhausted { last }) => {
        // Stop strategy fired; last is the final attempt's Result.
        println!("gave up: {last:?}");
    }
    Err(RetryError::Rejected { last }) => {
        // Predicate decided this error is non-retryable.
        println!("non-retryable: {last}");
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
