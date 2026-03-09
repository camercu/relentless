# tenacious specification

This document is the normative behavior and public-API contract for
`tenacious`. It defines what the crate guarantees at runtime and which items
are part of the supported surface. It does not prescribe internal file layout,
development workflow, or historical migration steps.

## Overview

`tenacious` is a Rust library for retrying fallible operations and polling for
conditions. It models retries with three composable parts:

- `Stop`: when to stop retrying
- `Wait`: how long to wait between attempts
- `Predicate`: which outcomes should retry

The same model applies to sync and async execution. Policies are reusable, hook
callbacks are configured per execution, and the crate supports `std`,
`no_std`, `wasm32`, and embedded-oriented environments.

## Support matrix

The crate is `#![no_std]` unconditionally. Feature flags add capabilities on
top of that base.

| Capability | `core` only | `alloc` | `std` |
| --- | --- | --- | --- |
| `Stop`, `Wait`, `Predicate`, state types | yes | yes | yes |
| Sync retry with explicit `.sleep(...)` | yes | yes | yes |
| Sync retry with implicit default sleeper | no | no | yes |
| Async retry with explicit `.sleep(...)` | yes | yes | yes |
| `AsyncRetryExt` and `AsyncRetryBuilder` | yes | yes | yes |
| Single hook per hook point | yes | yes | yes |
| Multiple appended hooks per hook point | no | yes | yes |
| `BoxedRetryPolicy` | no | yes | yes |
| `Arc<AtomicBool>` canceler | no | yes | yes |
| `std::error::Error` on `RetryError` | no | no | yes |

Runtime adapter helpers are feature-gated separately:

- `tokio-sleep`: `sleep::tokio()`
- `embassy-sleep`: `sleep::embassy()`
- `gloo-timers-sleep`: `sleep::gloo()` on `wasm32`
- `futures-timer-sleep`: `sleep::futures_timer()`
- `tokio-cancel`: `CancellationToken` canceler support
- `jitter`: wait jitter support
- `serde`: serde support for built-in policy configuration types

`alloc` is not required for async retry itself. It is required only for boxed
policies, `Arc<AtomicBool>`, and registering more than one hook of the same
kind on a single execution builder.

## Core abstractions

The public model centers on four traits plus a reusable policy type.

### Stop

`Stop` decides whether the retry loop should terminate after a completed
attempt.

```rust
pub trait Stop {
    fn should_stop(&mut self, state: &RetryState) -> bool;
    fn reset(&mut self) {}
}
```

Built-in strategies:

- `stop::attempts(n)`
- `stop::attempts_checked(n) -> Result<_, StopConfigError>`
- `stop::elapsed(dur)`
- `stop::before_elapsed(dur)`
- `stop::never()`

Composition is supported with:

- `a | b` or `a.or(b)` for `StopAny`
- `a & b` or `a.and(b)` for `StopAll`

### Wait

`Wait` returns the delay that should be applied before the next attempt.

```rust
pub trait Wait {
    fn next_wait(&mut self, state: &RetryState) -> Duration;
    fn reset(&mut self) {}
}
```

Built-in strategies:

- `wait::fixed(dur)`
- `wait::linear(initial, increment)`
- `wait::exponential(initial)`

Builder and composition methods are provided by `WaitExt`:

- `.cap(max)`
- `.jitter(max_jitter)` with the `jitter` feature
- `.chain(other, after)`
- `a + b` or `a.add(b)` for `WaitCombine`

Wait strategies only compute `Duration`. They do not sleep directly.

### Predicate

`Predicate<T, E>` decides whether a completed outcome should retry.

```rust
pub trait Predicate<T, E> {
    fn should_retry(&mut self, outcome: &Result<T, E>) -> bool;
}
```

Built-in predicate constructors:

- `on::any_error()`
- `on::error(f)`
- `on::result(f)`
- `on::ok(f)`

Polling is expressed with the existing predicate combinators:

- use `on::ok(|value| !is_ready(value))` when `Err` should terminate
- combine `on::error(is_retryable)` with `on::ok(...)` when selected errors
  should retry
- use `on::result(...)` when the retry decision needs the full `Result<T, E>`

Composition is supported with:

- `a | b` or `a.or(b)` for `PredicateAny`
- `a & b` or `a.and(b)` for `PredicateAll`

`Predicate` is blanket-implemented for `FnMut(&Result<T, E>) -> bool`.

### Sleeper

`Sleeper` abstracts async delay behavior.

```rust
pub trait Sleeper {
    type Sleep: Future<Output = ()>;
    fn sleep(&self, dur: Duration) -> Self::Sleep;
}
```

It is blanket-implemented for `Fn(Duration) -> Fut` where
`Fut: Future<Output = ()>`.

Sync execution does not use `Sleeper`. It uses a blocking sleep function via
`.sleep(...)`, or `std::thread::sleep` when the `std` feature is active and no
explicit sync sleeper is provided.

### State types

The crate exposes four read-only state types:

- `RetryState`
- `BeforeAttemptState`
- `AttemptState<'a, T, E>`
- `ExitState<'a, T, E>`

All are `#[non_exhaustive]` and provide `new(...)` constructors for tests and
custom strategy implementations.

Field meanings:

- `attempt` is 1-indexed for completed or about-to-start attempts
- `elapsed` is `None` when no elapsed clock is available
- `AttemptState.next_delay` is `Some(delay)` only when another attempt will run
- `ExitState.outcome` is `None` only when cancellation happens before the first
  attempt

## Policy model

`RetryPolicy<S, W, P>` stores owned stop, wait, and predicate values.

Construction forms:

- `RetryPolicy::new()` starts with `NeedsStop` and intentionally blocks retry
  execution until `.stop(...)` is configured
- `RetryPolicy::default()` creates a safe ready-to-run policy:
  `attempts(3)`, `exponential(100ms)`, and `any_error()`

Builder methods:

- `.stop(...)`
- `.wait(...)`
- `.when(...)`
- `.elapsed_clock(fn() -> Duration)`
- `.clear_elapsed_clock()`
- `.boxed()` with `alloc`

Policy lifecycle guarantees:

- `RetryPolicy::retry(...)` and `RetryPolicy::retry_async(...)` call
  `stop.reset()` and `wait.reset()` before execution begins
- owned extension builders created by `RetryExt` and `AsyncRetryExt` reset
  stateful stop and wait strategies when first polled or called
- `elapsed_clock` uses a bare function pointer so it works without allocation
  and can be cleared with `clear_elapsed_clock()`

## Execution semantics

Sync and async execution share the same transition rules. The difference is
only how sleeping happens.

### Sync execution

`RetryPolicy::retry(op)` returns `SyncRetry`. `RetryExt::retry()` returns an
owned `SyncRetryBuilder`.

Calling `.sleep(...)` is:

- optional with `std`
- required without `std`

The sync loop performs these steps:

1. Check cancellation before the attempt starts.
2. Fire `before_attempt`.
3. Call the user operation.
4. Evaluate the predicate.
5. If the predicate does not retry, terminate immediately.
6. Compute the next wait duration.
7. Evaluate the stop strategy with `RetryState.next_delay` populated.
8. If stop fires, terminate immediately.
9. Fire `after_attempt` with `next_delay = Some(delay)`.
10. Sleep for the computed delay.
11. Check cancellation again.
12. Increment the attempt counter and continue.

### Async execution

`RetryPolicy::retry_async(op)` returns `AsyncRetry`. `AsyncRetryExt::retry_async()`
returns an owned `AsyncRetryBuilder`.

Async execution always requires `.sleep(...)` before the future can run. The
crate never auto-selects an async runtime.

The async future is single-use:

- it is directly awaitable
- polling after completion always panics

The async loop uses the same transition order as sync execution. During the
sleep phase it polls both the sleep future and `Canceler::cancel()`, so
wake-driven cancelers can interrupt sleep promptly.

## Termination semantics

Retry termination is defined by the final accepted outcome, the predicate, the
stop strategy, and cancellation.

| Final condition | Return value | `StopReason` | `after_attempt.next_delay` on final attempt | `ExitState.outcome` |
| --- | --- | --- | --- | --- |
| Accepted `Ok(T)` | `Ok(T)` | `Success` | `None` | `Some(&Ok(T))` |
| Predicate accepts `Err(E)` as terminal | `Err(RetryError::NonRetryableError)` | `NonRetryableError` | `None` | `Some(&Err(E))` |
| Stop fires on retrying `Err(E)` | `Err(RetryError::Exhausted)` | `StopStrategyTriggered` | `None` | `Some(&Err(E))` |
| Stop fires on retrying `Ok(T)` | `Err(RetryError::ConditionNotMet)` | `StopStrategyTriggered` | `None` | `Some(&Ok(T))` |
| Cancelled before first attempt | `Err(RetryError::Cancelled)` | `Cancelled` | not fired | `None` |
| Cancelled after a completed attempt | `Err(RetryError::Cancelled)` | `Cancelled` | not fired for cancellation itself | `Some(&last_result)` |

Additional guarantees:

- predicate evaluation always happens before stop evaluation
- stop evaluation always happens after wait computation
- `ConditionNotMet` is the terminal error when the loop is retrying `Ok`
  values and the stop strategy fires first
- `RetryResult<T, E>` is `Result<T, RetryError<E, T>>`

## Hooks

Hooks are configured on execution builders, not on `RetryPolicy`.

Hook points:

- `before_attempt(FnMut(&BeforeAttemptState))`
- `after_attempt(FnMut(&AttemptState<'_, T, E>))`
- `on_exit(FnMut(&ExitState<'_, T, E>))`

Timing guarantees:

- `before_attempt` fires before the user operation starts
- `after_attempt` fires after each completed attempt
- `on_exit` fires exactly once for every terminal path

Ordering guarantees:

- hooks of the same kind fire in registration order
- without `alloc`, each hook point accepts at most one callback
- attempting to register a second callback without `alloc` is a compile-time
  error because the setter method disappears from the type state

Hook panics propagate normally. The crate does not catch them.

## Cancellation

Cancellation is provided by the `Canceler` trait:

```rust
pub trait Canceler {
    type Cancel: Future<Output = ()>;
    fn is_cancelled(&self) -> bool;
    fn cancel(&self) -> Self::Cancel;
}
```

Provided cancelers:

- `cancel::never()` / `NeverCancel`
- `&AtomicBool`
- `Arc<AtomicBool>` with `alloc`
- `tokio_util::sync::CancellationToken` with `tokio-cancel`

Cancellation guarantees:

- it is checked before every attempt
- sync retries check it again after sleeping
- async retries race sleep against `cancel()`
- the crate never interrupts a user operation that is already running

## Statistics and elapsed time

Calling `.with_stats()` changes the terminal output to
`(Result<T, RetryError<E, T>>, RetryStats)`.

`RetryStats` contains:

- `attempts`
- `total_elapsed`
- `total_wait`
- `stop_reason`

Elapsed-time behavior:

- `std` builds use `std::time::Instant` unless a custom elapsed clock is set
- without `std`, elapsed values remain `None` unless `elapsed_clock(...)` is
  configured
- statistics do not force additional timing work beyond what the chosen clock
  already provides

## Feature-gated APIs

The crate exposes feature-gated helpers and derives in these areas.

### Sleep adapters

The `sleep` module exports these helpers when their features are enabled:

- `sleep::tokio()`
- `sleep::embassy()`
- `sleep::gloo()` on `wasm32`
- `sleep::futures_timer()`

These helpers are conveniences only. Callers may always pass their own
compatible sleeper function or type.

### Jitter

With the `jitter` feature:

- `WaitExt::jitter(max_jitter)` is available
- `WaitJitter` is exported from the crate root
- `WaitJitter::with_seed([u8; 32])` and `.with_nonce(u64)` support
  reproducible sequences

Jitter adds a uniformly distributed duration in `[0, max_jitter]` before
`cap(...)` is applied.

### Serde

With the `serde` feature, built-in policy, stop, wait, predicate, stats, and
reason types support serde where implemented by the crate.

Serde guarantees:

- strategy configuration is serialized
- hook callbacks are never serialized
- elapsed clock function pointers are never serialized
- deserialized policies always restore `elapsed_clock` to `None`
- constructor invariants are re-validated during deserialization

## Public API surface

The following items are part of the supported crate-root surface.

Always exported:

- `RetryPolicy`
- `RetryError`
- `RetryResult`
- `RetryStats`
- `StopReason`
- `RetryState`
- `BeforeAttemptState`
- `AttemptState`
- `ExitState`
- `Stop`, `StopExt`, `StopAny`, `StopAll`, `NeedsStop`, `StopConfigError`
- `Wait`, `WaitExt`, `WaitCapped`, `WaitChain`, `WaitCombine`
- `Predicate`, `PredicateExt`
- `Sleeper`
- `Canceler`, `NeverCancel`
- `SyncRetry`, `SyncRetryWithStats`
- `AsyncRetry`, `AsyncRetryWithStats`
- `SyncRetryBuilder`, `SyncRetryBuilderWithStats`, `RetryExt`
- `AsyncRetryBuilder`, `AsyncRetryBuilderWithStats`, `AsyncRetryExt`
- modules: `cancel`, `on`, `sleep`, `stop`, `wait`

Conditionally exported:

- `BoxedRetryPolicy` with `alloc`
- `WaitJitter` with `jitter`

The crate also exports a `prelude` module containing the most common traits and
constructors, including `RetryExt`, `AsyncRetryExt`, `attempts`, `elapsed`,
`fixed`, `exponential`, `any_error`, `error`, and `ok`.

## Compatibility guarantees

The crate guarantees the following project-wide properties.

- MSRV is Rust `1.85.0`
- the crate forbids `unsafe` with `#![forbid(unsafe_code)]`
- `Duration` is always `core::time::Duration`
- `RetryError` implements `Display` unconditionally
- `RetryError` implements `std::error::Error` when `std` is active and its type
  parameters satisfy the required bounds
