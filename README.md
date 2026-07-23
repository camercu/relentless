# relentless

[![crates.io](https://img.shields.io/crates/v/relentless.svg)](https://crates.io/crates/relentless)
[![docs.rs](https://docs.rs/relentless/badge.svg)](https://docs.rs/relentless)
[![CI](https://github.com/camercu/relentless/actions/workflows/ci.yml/badge.svg)](https://github.com/camercu/relentless/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-blue.svg)](#msrv)
[![License](https://img.shields.io/crates/l/relentless.svg)](LICENSE-MIT)


Retry and polling for Rust — with composable strategies, policy reuse, and
first-class support for polling workflows where `Ok(_)` doesn't always mean
"done."

Most retry libraries handle the simple case well: call a function, retry on
error, back off. `relentless` handles that too, but it also handles the cases
those libraries make awkward:

- **Polling**, where `Ok("pending")` means "keep going" and you need
  `.until(predicate::ok(...))` rather than just retrying errors.
- **Outcome classification**, where `.decide(...)` sorts each outcome into
  return / retry / abort — so a sought-after `Err`, a non-`Result` poll enum, or
  a search state can drive the loop directly, independent of `Result` semantics.
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
(ergonomic retry builders and the `.retry()` extension trait — starting a retry
directly from a function or closure).

## Install

```bash
cargo add relentless
```

### Feature flags

| Flag                  | Purpose                                                                              |
| --------------------- | ------------------------------------------------------------------------------------ |
| `std` (default)       | `clock::SystemClock` default for sync retries, `std::error::Error` on `RetryError`   |
| `alloc`               | Boxed policies, `clock::VirtualClock` wait recording                                 |
| `tokio-clock`         | `clock::TokioClock` async clock adapter                                              |
| `embassy-clock`       | `clock::EmbassyClock` async clock adapter                                            |
| `gloo-timers-clock`   | `clock::GlooClock` async clock adapter (wasm32; caller supplies the now-source)      |
| `futures-timer-clock` | `clock::FuturesTimerClock` async clock adapter                                       |

Async retry does not require `alloc`.

---

## Examples

For full docs, see <https://docs.rs/relentless>. Behavior spec:
[docs/SPEC.md](./docs/SPEC.md). Runnable examples live in
[`examples/`](./examples).

Sync examples omit `.clock(...)` because `std` builds default to
`clock::SystemClock` (wall time + `std::thread::sleep`). Without `std`, inject
an explicit clock before `.call()`.

### 1) Retry with defaults

The `.retry()` extension trait is the fastest way to add retries. Defaults: 3
attempts, exponential backoff from 100 ms, retry on any `Err`.

```rust,no_run
use relentless::RetryExt;

fn fetch_job_output() -> Result<String, std::io::Error> {
    std::fs::read_to_string("/var/run/background_job.output")
}

let results = fetch_job_output.retry().call();
```

### 2) Customized retry

The `retry` free function is equivalent to the extension trait, with the added
ability to capture retry loop state. Both the free function and extension trait
give full control over which errors to retry, how long to wait, and when to
stop.

```rust,no_run
use core::time::Duration;
use relentless::{Wait, retry, predicate, stop, wait};

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
use relentless::{RetryPolicy, stop, wait};

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

Use `.until(predicate)` to keep retrying until a success condition is met.
Unlike `.when()`, which retries on matching outcomes, `.until()` retries on
everything *except* the matching outcome.

```rust,no_run
use relentless::{RetryPolicy, predicate};

#[derive(Debug, PartialEq)]
enum Status { Pending, Done }

fn poll_status() -> Result<Status, std::io::Error> { todo!() }

let result = RetryPolicy::new()
    .until(predicate::ok(|s: &Status| *s == Status::Done))
    .retry(|_| poll_status())
    .call();
```

> **Note:** `predicate::ok` constrains only the success type, so an operation
> that never returns a concrete `Err` leaves the error type unpinned (`E0282`).
> Give the op a signature — as `poll_status` does above — or annotate it inline,
> e.g. `.retry(|_| Ok::<_, std::io::Error>(Status::Done))`.

To also retry selected errors during polling, use `predicate::result`:

```rust,no_run
use relentless::{RetryPolicy, predicate};

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

Pass an async clock adapter — here via the `tokio-clock` feature.

```rust,no_run
use relentless::clock::TokioClock;
use relentless::retry_async;

async fn fetch(url: &str) -> Result<String, reqwest::Error> {
    reqwest::get(url).await?.text().await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = retry_async(|_| fetch("https://api.example.com/data"))
        .clock(TokioClock::new())
        .call()
        .await?;
    Ok(())
}
```

### More

Full inline code for these lives in the [API docs](https://docs.rs/relentless),
with runnable versions in [`examples/`](./examples):

- **Hooks & stats** — observe the retry lifecycle for logging or metrics with
  `.before_attempt` / `.after_attempt`, and collect a `RetryStats` summary via
  `.with_stats()`. ([`hooks-and-stats.rs`](./examples/hooks-and-stats.rs))
- **Self-classifying outcomes** — implement `Outcome` for a domain type (a poll
  enum, a search state) so it sorts itself into return / retry / abort, and the
  default engine drives it with no `.decide` at the call site.
  ([`custom-outcome.rs`](./examples/custom-outcome.rs))
- **Error handling** — on failure you get a `RetryError`: `Exhausted { last }`
  when the stop strategy fired (`last` is the final attempt's full
  `Result<T, E>` — polling can exhaust while the last outcome was still `Ok`),
  or `Aborted { last }` when the classifier rejected the outcome as fatal
  (`last` is the bare error). A classifier (`.decide`) can also make any outcome
  the success value, so a probe can return its found `Err` through `Ok`.
- **Deterministic testing** — `clock::VirtualClock` asserts the exact backoff
  schedule with zero wall-clock time spent, so timeout and backoff tests stay
  fast and non-flaky; one injected value drives both waits and elapsed time,
  so the two can never disagree.
  ([`testing-with-virtual-clock.rs`](./examples/testing-with-virtual-clock.rs))
- **Cancellation** — there is no built-in cancel primitive; the loop observes
  the cancellation your environment already provides (a dropped future, an
  `AtomicBool`, `.timeout(...)`) at attempt boundaries.
  ([`sync-cancel.rs`](./examples/sync-cancel.rs),
  [`async-cancel.rs`](./examples/async-cancel.rs))

---

## How the builders fit together

The full API surface — every strategy, predicate, and type — lives on
[docs.rs](https://docs.rs/relentless). Two things worth knowing up front:

Builder chains read best in this order: **when/until** -> **wait** -> **stop**
-> clock -> hooks -> stats -> call. That order is a reading convention, not a
compiler contract — the types enforce only four rules: strategy overrides
(`when`/`until`/`wait`/`stop`) exist only on builders that own their policy
(below), everything is configured before `.with_stats()`, an async chain needs
`.clock(...)` before `.call()`, and `.clock(...)` can be called at most once
(it exists only while the builder still carries the default clock).

Where you start decides what you can override. The free-function and
extension-trait builders own their policy, so they accept the strategy overrides
`when`/`until`/`wait`/`stop`. An execution started from a shared `RetryPolicy`
(`policy.retry(...)`) keeps that policy's strategies fixed and accepts only
clock, hooks, timing, and stats methods.

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
