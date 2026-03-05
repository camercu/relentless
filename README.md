# tenacious

`tenacious` is a Rust retry and polling library for building resilient clients
without ad hoc retry loops.

It is inspired by Python's [`tenacity`](https://github.com/jd/tenacity) library,
especially its operator-based strategy composition and callback ergonomics, and
by Rust's [`backon`](https://crates.io/crates/backon) crate, especially its
lightweight retry-builder workflow.

Compared with `backon`, `tenacious` centers on reusable policy objects that
compose `Stop`, `Wait`, and `Predicate` strategies directly (`|`, `&`, `+`,
and `.chain(...)`), support full-result polling predicates like
`on::until_ready`, and expose lifecycle hooks, cancellation, and optional
execution stats in the same API surface.

It supports sync and async execution across `std`, `alloc`, `no_std`, and
`wasm` targets, with runtime-specific sleep adapters behind feature flags.

## Examples

For most integrations, start with one of these patterns. They are ordered from
the simplest default path to more specialized usage.

### Retry directly on a closure

Use `RetryExt` for one-off retry runs. Uses sane defaults:

- stop: 3 attempts
- wait: exponential backoff starting at 100ms
- predicate: retry on any error

```rust
use std::fs;
use tenacious::RetryExt;

let contents = (|| fs::read_to_string("config.toml")).retry().call().unwrap();
```

Or you can change the default strategy:

```rust
use core::time::Duration;
use std::fs;
use tenacious::RetryExt;

let contents = (|| fs::read_to_string("config.toml"))
  .retry()
  .stop(stop::elapsed(Duration::from_secs(5)))
  .wait(wait::fixed(Duration::from_millis(200)));
  .call()
  .unwrap();

```

### Retry transient errors with defaults (sync)

Use `RetryPolicy::default()` for a safe, ready-to-run, and reusable policy:

- stop: 3 attempts
- wait: exponential backoff starting at 100ms
- predicate: retry on any error

```rust
use std::fs;
use tenacious::RetryPolicy;

let mut policy = RetryPolicy::default();
let result = policy
    .retry(|| fs::read_to_string("service-config.toml"))
    .call();

let _ = result;
```

### Retry with explicit policy settings (sync)

Use `RetryPolicy::new()` when you want full control over stop and wait behavior:

```rust
use core::time::Duration;
use std::fs;
use tenacious::{RetryPolicy, stop, wait};

let mut policy = RetryPolicy::new()
    .stop(stop::attempts(5))
    .wait(wait::fixed(Duration::from_millis(20)));

let result = policy
    .retry(|| fs::read_to_string("orders-cache.json"))
    .call();

let _ = result;
```

### Poll until a value is ready

Use `on::until_ready` when transient errors and "not ready yet" values should
both keep polling:

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, on, stop, wait};

#[derive(Debug)]
struct HttpError;

#[derive(Debug)]
struct JobStatus {
    status: &'static str, // "pending" | "success"
}

fn fetch_job_status() -> Result<JobStatus, HttpError> {
    // Example: HTTP 200 { "status": "pending" } -> Ok(JobStatus { status: "pending" })
    // Example: HTTP 200 { "status": "success" } -> Ok(JobStatus { status: "success" })
    // Transport/5xx failures would return Err(HttpError).
    Err(HttpError)
}

let mut policy = RetryPolicy::new()
    .stop(stop::attempts(4))
    .wait(wait::fixed(Duration::from_millis(250)))
    .when(on::until_ready(|response: &JobStatus| response.status == "success"));

let result = policy
    .retry(fetch_job_status)
    .call();
```

`on::ok` and `on::until_ready` look similar but differ on `Err` handling:

- `on::ok(...)`: retries selected `Ok` values and returns immediately on any `Err`
- `on::until_ready(...)`: retries on `Err` and retries on `Ok` values until ready criteria met

Side-by-side:

```rust
use tenacious::on;

let retry_only_ok = on::ok(|value: &u32| *value < 3);
let retry_until_ready = on::until_ready(|value: &u32| *value >= 3);
```

`on::until_ready(is_ready)` is equivalent to:
`on::any_error() | on::ok(|value| !is_ready(value))`.

### Retry async operations

`retry_async` is runtime-agnostic. Provide any `Sleeper` implementation (or a
compatible closure):

```rust
use core::time::Duration;
use std::fs;
use tenacious::{RetryPolicy, stop};

let mut policy = RetryPolicy::new().stop(stop::attempts(3));
let retry = policy
    .retry_async(|| async { fs::read_to_string("profile.json") })
    .sleep(|_dur: Duration| async {});

let _ = retry;
```

With `tokio-sleep` enabled, you can pass `tenacious::sleep::tokio()`.

## Public API overview

- `stop`: stop strategies (`attempts`, `elapsed`, `before_elapsed`, `never`)
- `wait`: wait strategies (`fixed`, `linear`, `exponential`, composition)
- `on`: retry predicates (`any_error`, `error`, `ok`, `result`,
  `until_ready`)
- `RetryPolicy`: reusable retry configuration
- `SyncRetry` / `AsyncRetry`: execution builders
- `RetryError`: terminal retry outcomes
- `RetryStats`: optional aggregate execution statistics via `.with_stats()`

For ergonomic imports, use:

```rust
use tenacious::prelude::*;
```

## Hooks and stats

`SyncRetry` and `AsyncRetry` support lifecycle hooks, and both support optional
aggregate stats:

```rust
use std::fs;
use tenacious::{RetryPolicy, stop};

let mut policy = RetryPolicy::new().stop(stop::attempts(3));

let (_result, stats) = policy
    .retry(|| fs::File::open("service.lock"))
    .before_attempt(|state| {
        let _ = state.attempt;
    })
    .after_attempt(|state| {
        let _ = state.attempt;
    })
    .on_exit(|state| {
        let _ = state.reason;
    })
    .with_stats()
    .call();

assert!(stats.attempts >= 1);
```

## Cancellation

All execution builders support `.cancel_on(...)`, including extension-trait
builders from `RetryExt` and `AsyncRetryExt`.

With `tokio-cancel` enabled, passing a
`tokio_util::sync::CancellationToken` enables wake-driven async cancellation
while sleeping between attempts.

```rust
use core::sync::atomic::{AtomicBool, Ordering};
use std::fs;
use tenacious::{RetryExt, RetryError};

let cancelled = AtomicBool::new(false);

let result = (|| fs::read_to_string("/tmp/service.sock"))
    .retry()
    .sleep(|_dur| {
        cancelled.store(true, Ordering::Relaxed);
    })
    .cancel_on(&cancelled)
    .call();

assert!(matches!(result, Err(RetryError::Cancelled { .. })));
```

## Constructor behavior

- `RetryPolicy::new()` returns an unconfigured policy whose stop type is
  `NeedsStop`; you must call `.stop(...)` before `retry`/`retry_async`.
- `RetryPolicy::default()` returns a safe configured policy (3 attempts,
  100ms exponential backoff).
- `RetryExt::retry()` and `AsyncRetryExt::retry_async()` start from the same
  safe defaults as `RetryPolicy::default()`.
- `stop::attempts(n)` is the ergonomic constructor for hardcoded, known-valid
  literals.
- `stop::attempts_checked(n)` is the control-path constructor for runtime or
  untrusted values.
- `RetryPolicy::elapsed_clock(fn() -> Duration)` lets you provide a monotonic
  elapsed-time source (useful for `no_std` targets where wall-clock time is not
  available from `std::time::Instant`).

## Error behavior

`RetryError<E, T>` has four variants:

- `Exhausted`: stop condition fired while operation kept returning `Err`
- `PredicateRejected`: predicate rejected an `Err` as non-retryable
- `ConditionNotMet`: stop condition fired while retrying `Ok` values
- `Cancelled`: cancellation signal interrupted retries

## Feature flags

- `std` (default): blocking sleep default, wall-clock elapsed timing, and
  `std::error::Error` support
- `alloc`: boxed policy storage and async execution support
- `tokio-sleep`: tokio sleep adapter
- `futures-timer-sleep`: `futures-timer` adapter
- `gloo-timers-sleep`: wasm/gloo timers adapter
- `embassy-sleep`: embassy-time adapter (requires an embassy time driver on
  your target)
- `tokio-cancel`: `CancellationToken`-based async cancellation signaling
- `jitter`: randomized jitter for wait strategies
- `serde`: serialization for strategy/stat types
- `strict-futures`: panics on `AsyncRetry` repoll-after-completion in release
  builds (debug already panics)

With `jitter` enabled, use `.with_seed([u8; 32])` and `.with_nonce(u64)` on
`WaitJitter` when you need reproducible jitter sequences.

## Production notes

**Scope.** `tenacious` is a per-call retry library. It does not provide circuit
breaking, global rate limiting, or retry budgets. In distributed systems where
many callers may retry simultaneously against a degraded backend, pair this
library with a circuit breaker or concurrency limiter to avoid thundering-herd
amplification. Because `Stop` is an open trait, you can integrate an external
breaker directly:

```rust
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tenacious::{Stop, RetryState, RetryPolicy, stop, wait};
use core::time::Duration;

/// Stops retrying when a shared circuit breaker trips open.
#[derive(Clone)]
struct CircuitBreakerStop<S> {
    inner: S,
    open: Arc<AtomicBool>,
}

impl<S: Stop> Stop for CircuitBreakerStop<S> {
    fn should_stop(&mut self, state: &RetryState) -> bool {
        self.open.load(Ordering::Relaxed) || self.inner.should_stop(state)
    }
    fn reset(&mut self) { self.inner.reset(); }
}

let breaker = Arc::new(AtomicBool::new(false));

let mut policy = RetryPolicy::new()
    .stop(CircuitBreakerStop {
        inner: stop::attempts(5),
        open: breaker.clone(),
    })
    .wait(wait::exponential(Duration::from_millis(100)));

// When the breaker trips, all in-flight retries stop at the next attempt.
// breaker.store(true, Ordering::Relaxed);
let _ = policy
    .retry(|| fs::read_to_string("/tmp/downstream-health"))
    .sleep(|_| {})
    .call();
```

**Hook panics.** Panics in user-supplied hook callbacks (`before_attempt`,
`after_attempt`, `before_sleep`, `on_exit`) propagate through the retry
loop and will unwind the calling thread. If hooks run fallible or
user-provided logic, consider catching panics at the call site.

**Thread safety.** `RetryPolicy` is `Send + Sync` when all its constituent
strategy types are `Send + Sync` (all built-in strategies satisfy this).
Policies can be shared across threads via `Arc<Mutex<RetryPolicy>>` or cloned
per-thread since `RetryPolicy` is `Clone`.

**Configuration validation.** `stop::attempts(n)` panics when `n == 0`.
Use `stop::attempts(n)` for hardcoded constants. For runtime or untrusted
input, use `stop::attempts_checked(n)` and handle `StopConfigError`
explicitly.

```rust
use tenacious::stop;

fn parse_attempts(
    raw: u32,
) -> Result<tenacious::stop::StopAfterAttempts, tenacious::stop::StopConfigError> {
    stop::attempts_checked(raw)
}
```

## Safety-critical usage

**Saturation, not failure.** Arithmetic overflow in wait durations and attempt
counters saturates silently (`Duration::MAX`, `u32::MAX`) rather than
panicking or returning an error. If your system depends on precise delay
values at extreme scales, add assertions in a `before_sleep` hook.

**Floating-point backoff.** `wait::exponential` computes delays using `f64`
internally. Delays are not bit-for-bit reproducible across CPU architectures.
Calling `.base()` with a value below `1.0` silently clamps to `1.0`
(constant delay). If deterministic delays matter, use `wait::fixed` or
`wait::linear`, which use only integer `Duration` arithmetic.

**Elapsed time on no_std.** Without `std` or a custom `elapsed_clock`,
`elapsed` is always `None`. This means `stop::elapsed()` and
`stop::before_elapsed()` silently never fire — the retry loop relies
entirely on attempt-count stops. Always pair an elapsed stop with an
attempt stop on `no_std`: `stop::attempts(n) | stop::elapsed(deadline)`.

**Hook state across retries.** Hooks are configured on each execution builder
(`.retry(...)` / `.retry_async(...)`), not stored on `RetryPolicy`. Captured
state persists only if you reuse the same closure value across calls.

## no_std support

The crate supports `no_std` operation. Build with:

```bash
cargo build --no-default-features
```

For wasm `no_std` compatibility:

```bash
cargo check --target wasm32-unknown-unknown --no-default-features --features alloc,gloo-timers-sleep
```

When checking `embassy-sleep` on desktop hosts, use compile-only verification:

```bash
cargo check --features embassy-sleep
```

In this repository, `cargo test --all-features` works on host targets because
dev-dependencies enable `embassy-time`'s `mock-driver` feature for local test
and lint runs.

For downstream binaries using `embassy-sleep`, you still need exactly one
Embassy time driver in the final crate graph (for example, a HAL-provided
driver on embedded targets). Without a driver, linking fails.

## MSRV

Minimum supported Rust version: `1.85`.

## License

Licensed under either of:

- MIT license ([LICENSE-MIT.txt](./LICENSE-MIT.txt))
- Apache License, Version 2.0 ([LICENSE-APACHE.txt](./LICENSE-APACHE.txt))
