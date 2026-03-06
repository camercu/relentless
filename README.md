# tenacious

`tenacious` seamless and reusable retry for Rust.

It models retry behavior as three composable parts: when to stop retrying (`Stop`), how
long to wait between retries (`Wait`), and what outcomes are retryable (`Predicate`). Compared
with simpler retry helpers, it gives you policy reuse, polling-aware predicates
like `on::any_error`, hooks for dynamically interacting with retries, cancellation, and stats, all under one API that
works in sync and async code across `std`, `no_std`, and `wasm` targets.

It is inspired by Python's
[`tenacity`](https://github.com/jd/tenacity), especially composable strategy
algebra, and Rust's [`backon`](https://crates.io/crates/backon), especially
ergonomic retry builders.

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

Optional runtime adapters and feature flags are in
[`Cargo.toml`](./Cargo.toml).

---

## Quick start

For full docs and additional examples, see <https://docs.rs/tenacious>.

Behavior spec: [docs/SPEC.md](./docs/SPEC.md)

### 1) Basic Retry on closures/functions with defaults (`RetryExt`)

Use this for one-off operations. Defaults are: 3 attempts, exponential backoff
from 100ms, retry on any error.

```rust
use tenacious::RetryExt;
use std::fs::read_to_string;

let contents = (|| read_to_string("myfile")).retry().call().unwrap();
```

### 2) Reuse a policy for API calls

Use `RetryPolicy` when multiple operations share retry rules.

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, on, stop, wait};

#[derive(Debug)]
enum ApiError {
    Transport,
    Server(u16),
    Unauthorized,
}

fn create_invoice() -> Result<String, ApiError> {
    Err(ApiError::Server(503))
}

let mut api_policy = RetryPolicy::new()
    .stop(stop::attempts(5) | stop::elapsed(Duration::from_secs(2)))
    .wait(wait::fixed(Duration::from_millis(50))
        + wait::exponential(Duration::from_millis(100)))
    .when(on::error(|err: &ApiError| {
        matches!(err, ApiError::Transport | ApiError::Server(429 | 503 | 504))
    }));

let invoice_id = api_policy.retry(create_invoice).call().unwrap();
```

### 3) Poll HTTP 200 payloads until ready (`on::until_ready`)

Use this when both transient `Err` values and `Ok("pending")` results should
retry.

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, on, stop, wait};

#[derive(Debug)]
struct HttpError;

#[derive(Debug)]
struct JobResponse {
    status: &'static str, // "pending" | "success"
}

fn get_export_status() -> Result<JobResponse, HttpError> {
    // Example HTTP 200 payloads:
    // { "status": "pending" }
    // { "status": "success" }
    Ok(JobResponse { status: "pending" })
}

let mut policy = RetryPolicy::new()
    .stop(stop::attempts(8))
    .wait(wait::fixed(Duration::from_millis(250)))
    .when(on::until_ready(|body: &JobResponse| body.status == "success"));

let final_status = policy.retry(get_export_status).call().unwrap();
```

### 4) Use the same model in async code

`retry_async` uses the same stop/wait/predicate model. Provide any compatible
sleeper.

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, stop};

#[derive(Debug)]
enum QueueError {
    BrokerUnavailable,
}

async fn publish_event() -> Result<(), QueueError> {
    Err(QueueError::BrokerUnavailable)
}

async fn runtime_sleep(_dur: Duration) {}

let mut policy = RetryPolicy::new().stop(stop::attempts(4));
let result = policy
    .retry_async(publish_event)
    .sleep(|dur| runtime_sleep(dur))
    .await;

let _ = result;
```

### 5) Add hooks, cancellation, and stats

```rust
use core::sync::atomic::{AtomicBool, Ordering};
use tenacious::{RetryExt, RetryError, StopReason};

let cancelled = AtomicBool::new(false);

let (result, stats) = (|| std::fs::read_to_string("/tmp/leader.lock"))
    .retry()
    .before_attempt(|state| {
        let _ = state.attempt;
    })
    .sleep(|_dur| {
        // Real world: shutdown signal arrives while waiting.
        cancelled.store(true, Ordering::Relaxed);
    })
    .cancel_on(&cancelled)
    .with_stats()
    .call();

assert!(matches!(result, Err(RetryError::Cancelled { .. })));
assert_eq!(stats.stop_reason, StopReason::Cancelled);
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
