# tenacious

`tenacious` is a Rust library for retrying fallible operations and polling for
conditions with composable stop, wait, and predicate strategies.

It supports `std`, `alloc`, and `no_std` targets, including async execution and
runtime-specific sleep adapters behind feature flags.

## Installation

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
tenacious = "0.1"
```

If you need async runtime adapters or optional integrations, enable features:

```toml
[dependencies]
tenacious = { version = "0.1", features = ["tokio-sleep", "jitter", "serde"] }
```

## Quick start (sync)

Use `RetryPolicy::default()` for a safe ready-to-run policy:

- stop: 3 attempts
- wait: exponential backoff starting at 100ms
- predicate: retry on any error

```rust
use tenacious::RetryPolicy;

let mut policy = RetryPolicy::default();
let result = policy
    .retry(|| Err::<(), _>("transient"))
    .sleep(|_dur| {})
    .call();

assert!(result.is_err());
```

If you want full control, start from `RetryPolicy::new()` and configure `.stop`
explicitly before executing retries:

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, stop, wait};

let mut policy = RetryPolicy::new()
    .stop(stop::attempts(5))
    .wait(wait::fixed(Duration::from_millis(20)));

let result = policy.retry(|| Ok::<u32, &str>(42)).sleep(|_dur| {}).call();
assert_eq!(result, Ok(42));
```

## Quick start (async)

`retry_async` is runtime-agnostic. Provide any `Sleeper` implementation (or a
compatible closure):

```rust
use core::time::Duration;
use tenacious::{RetryPolicy, stop};

let mut policy = RetryPolicy::new().stop(stop::attempts(3));
let retry = policy
    .retry_async(|| async { Ok::<u32, &str>(1) })
    .sleep(|_dur: Duration| async {});

# let _ = retry;
```

With `tokio-sleep` enabled, you can pass `tenacious::sleep::tokio_sleep`.

## Polling for conditions

Use `on::ok` when the operation succeeds but the condition is not met yet:

```rust
use tenacious::{RetryPolicy, on, stop};

let mut policy = RetryPolicy::new()
    .stop(stop::attempts(4))
    .when(on::ok(|v: &i32| *v < 0));

let result = policy.retry(|| Ok::<_, &str>(-1)).sleep(|_dur| {}).call();
assert!(result.is_err());
```

## Public API overview

- `stop`: stop strategies (`attempts`, `elapsed`, `before_elapsed`, `never`)
- `wait`: wait strategies (`fixed`, `linear`, `exponential`, composition)
- `on`: retry predicates (`any_error`, `error`, `ok`, `result`)
- `RetryPolicy`: reusable retry configuration
- `SyncRetry` / `AsyncRetry`: execution builders
- `RetryError`: terminal retry outcomes
- `RetryStats`: optional aggregate execution statistics via `.with_stats()`

For ergonomic imports, use:

```rust
use tenacious::prelude::*;
```

## Hooks and stats

`RetryPolicy` supports lifecycle hooks and optional aggregate stats:

```rust
use tenacious::{RetryPolicy, stop};

let mut policy = RetryPolicy::new()
    .stop(stop::attempts(3))
    .before_attempt(|state| {
        let _ = state.attempt;
    })
    .after_attempt(|state: &tenacious::AttemptState<(), &str>| {
        let _ = state.attempt;
    });

let (_result, stats) = policy
    .retry(|| Err::<(), _>("fail"))
    .sleep(|_dur| {})
    .with_stats()
    .call();

assert_eq!(stats.attempts, 3);
```

## Constructor behavior

- `RetryPolicy::new()` returns an unconfigured policy whose stop type is
  `NeedsStop`; you must call `.stop(...)` before `retry`/`retry_async`.
- `RetryPolicy::default()` returns a safe configured policy (3 attempts,
  100ms exponential backoff).
- `stop::attempts(n)` is the ergonomic constructor for hardcoded, known-valid
  literals.
- `stop::attempts_checked(n)` is the control-path constructor for runtime or
  untrusted values.
- `RetryPolicy::elapsed_clock(fn() -> Duration)` lets you provide a monotonic
  elapsed-time source (useful for `no_std` targets where wall-clock time is not
  available from `std::time::Instant`).

## Error behavior

`RetryError<E, T>` has three variants:

- `Exhausted`: stop condition fired while operation kept returning `Err`
- `PredicateRejected`: predicate rejected an `Err` as non-retryable
- `ConditionNotMet`: stop condition fired while retrying `Ok` values

## Feature flags

- `std` (default): blocking sleep default, wall-clock elapsed timing, and
  `std::error::Error` support
- `alloc`: boxed policy storage and async execution support
- `tokio-sleep`: tokio sleep adapter
- `futures-timer-sleep`: `futures-timer` adapter
- `gloo-timers-sleep`: wasm/gloo timers adapter
- `embassy-sleep`: embassy-time adapter (requires an embassy time driver on
  your target)
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
amplification.

**Hook panics.** Panics in user-supplied hook callbacks (`before_attempt`,
`after_attempt`, `before_sleep`, `on_exhausted`) propagate through the retry
loop and will unwind the calling thread. If hooks run fallible or
user-provided logic, consider catching panics at the call site.

**Thread safety.** `RetryPolicy` is `Send + Sync` when all its constituent
strategy and hook types are `Send + Sync` (all built-in strategies satisfy
this). Policies can be shared across threads via `Arc<Mutex<RetryPolicy>>` or
cloned per-thread since `RetryPolicy` is `Clone`.

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

Running `cargo test --all-features` on a host target can fail to link because
`embassy-time` expects a platform time driver symbol provided by embedded
targets.

## MSRV

Minimum supported Rust version: `1.85`.

## License

Licensed under either of:

- MIT license ([LICENSE-MIT.txt](./LICENSE-MIT.txt))
- Apache License, Version 2.0 ([LICENSE-APACHE.txt](./LICENSE-APACHE.txt))
