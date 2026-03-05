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

With `tokio-sleep` enabled, you can pass `tenacious::sleep::tokio()`.

## Polling for conditions

Use `on::wait_for_ok` for the common polling flow where transient errors and
"not ready yet" values should both retry:

```rust
use tenacious::{RetryPolicy, on, stop};

let mut policy = RetryPolicy::new()
    .stop(stop::attempts(4))
    .when(on::wait_for_ok(|v: &i32| *v >= 0));

let mut poll = -2;
let result = policy
    .retry(|| {
        poll += 1;
        if poll == -1 {
            Err::<i32, &str>("transient")
        } else {
            Ok(poll)
        }
    })
    .sleep(|_dur| {})
    .call();
assert_eq!(result, Ok(0));
```

Use `on::ok` when you only want to retry selected `Ok` values and immediately
return on any `Err`.

## Public API overview

- `stop`: stop strategies (`attempts`, `elapsed`, `before_elapsed`, `never`)
- `wait`: wait strategies (`fixed`, `linear`, `exponential`, composition)
- `on`: retry predicates (`any_error`, `error`, `ok`, `result`,
  `wait_for_ok`)
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
use tenacious::{RetryPolicy, stop};

let mut policy = RetryPolicy::new().stop(stop::attempts(3));

let (_result, stats) = policy
    .retry(|| Err::<(), _>("fail"))
    .before_attempt(|state| {
        let _ = state.attempt;
    })
    .after_attempt(|state: &tenacious::AttemptState<(), &str>| {
        let _ = state.attempt;
    })
    .on_exit(|state: &tenacious::ExitState<(), &str>| {
        let _ = state.reason;
    })
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
let _ = policy.retry(|| Err::<(), _>("fail")).sleep(|_| {}).call();
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

Running `cargo test --all-features` on a host target can fail to link because
`embassy-time` expects a platform time driver symbol provided by embedded
targets.

## MSRV

Minimum supported Rust version: `1.85`.

## License

Licensed under either of:

- MIT license ([LICENSE-MIT.txt](./LICENSE-MIT.txt))
- Apache License, Version 2.0 ([LICENSE-APACHE.txt](./LICENSE-APACHE.txt))
