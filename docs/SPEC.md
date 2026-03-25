# tenacious specification

This document is the normative behavior and public-API contract for
`tenacious`. It defines what the crate guarantees at runtime and which items
are part of the supported surface. It does not prescribe internal file layout,
development workflow, or historical migration steps.

## Overview

`tenacious` is a Rust library for retrying fallible operations and polling for
conditions. It models retries with three composable parts:

- `Predicate`: which outcomes should retry
- `Wait`: how long to wait between attempts
- `Stop`: when to stop retrying

The same model applies to sync and async execution. Policies are reusable, hook
callbacks are configured per execution, and the crate supports `std`,
`no_std`, `wasm32`, and embedded-oriented environments.

Polling for a condition is expressed with `.until()`, which accepts a predicate
using natural "done when true" logic. `.when()` provides the direct form:
"retry when this predicate is true." Both set the same predicate slot;
`.until(p)` wraps `p` in a `PredicateUntil` inverter. See Predicate for details.

## Support matrix

The crate is `#![no_std]` unconditionally. Feature flags add capabilities on
top of that base.

|Capability                              |`core` only|`alloc`|`std`|
|----------------------------------------|-----------|-------|-----|
|`Stop`, `Wait`, `Predicate`, state types|yes        |yes    |yes  |
|Sync retry with explicit `.sleep(...)`  |yes        |yes    |yes  |
|Sync retry with implicit default sleeper|no         |no     |yes  |
|Async retry with explicit `.sleep(...)` |yes        |yes    |yes  |
|Free functions and extension traits     |yes        |yes    |yes  |
|Single hook per hook point              |yes        |yes    |yes  |
|Multiple appended hooks per hook point  |no         |yes    |yes  |
|Closure-based elapsed clock             |no         |yes    |yes  |
|`std::error::Error` on `RetryError`     |no         |no     |yes  |

Runtime adapter helpers are feature-gated separately:

- `tokio`: `sleep::tokio()`
- `embassy`: `sleep::embassy()`
- `gloo-timers`: `sleep::gloo()` on `wasm32`
- `futures-timer`: `sleep::futures_timer()`
- `jitter`: jitter strategies and `Wait` jitter decorator methods

`alloc` is not required for async retry itself. It is required only for
closure-based elapsed clocks and registering more than one hook of the same
kind on a single execution builder.

## Core abstractions

The public model centers on four traits plus a reusable policy type. All four
core traits (`Stop`, `Wait`, `Predicate`, `Sleeper`) use `&self` receivers.
Strategies that need internal mutation use interior mutability (`Cell`,
`AtomicUsize`).

> The uniform `&self` model means strategies are trivially shareable,
> cloneable, and object-safe without requiring wrapper types or `Arc`.

### Stop

`Stop` decides whether the retry loop should terminate after a completed
attempt. Composition methods are provided directly on the trait with
`where Self: Sized` bounds, following the `Iterator` pattern.

```rust
pub trait Stop {
    fn should_stop(&self, state: &RetryState) -> bool;

    fn or<S: Stop>(self, other: S) -> stop::StopAny<Self, S>
    where Self: Sized { ... }

    fn and<S: Stop>(self, other: S) -> stop::StopAll<Self, S>
    where Self: Sized { ... }
}
```

Built-in strategies:

- `stop::attempts(n: u32) -> StopAfterAttempts`
- `stop::elapsed(dur: Duration) -> StopAfterElapsed`
- `stop::never() -> StopNever`

Stop semantics:

- `stop::attempts(n)` treats `n` as the maximum number of completed attempts
- `attempts` fires when `state.attempt >= n`; `n = 1` means "run at most one
  attempt"
- `attempts(0)` panics unconditionally
- `stop::elapsed(dur)` fires when `state.elapsed >= Some(dur)` and never fires
  when `state.elapsed` is `None`

### Wait

`Wait` returns the delay that should be applied before the next attempt.
Composition and builder methods are provided directly on the trait with
`where Self: Sized` bounds.

```rust
pub trait Wait {
    fn next_wait(&self, state: &RetryState) -> Duration;

    fn cap(self, max: Duration) -> wait::WaitCapped<Self>
    where Self: Sized { ... }

    fn jitter(self, max_jitter: Duration) -> wait::WaitJitter<Self>
    where Self: Sized { ... } // jitter feature

    fn full_jitter(self) -> wait::WaitFullJitter<Self>
    where Self: Sized { ... } // jitter feature

    fn equal_jitter(self) -> wait::WaitEqualJitter<Self>
    where Self: Sized { ... } // jitter feature

    fn chain<W: Wait>(self, other: W, after: u32) -> wait::WaitChain<Self, W>
    where Self: Sized { ... }

    fn add<W: Wait>(self, other: W) -> wait::WaitCombine<Self, W>
    where Self: Sized { ... }
}
```

Built-in strategies:

- `wait::fixed(dur: Duration) -> WaitFixed`
- `wait::linear(initial: Duration, increment: Duration) -> WaitLinear`
- `wait::exponential(initial: Duration) -> WaitExponential`
- `wait::decorrelated_jitter(base: Duration) -> WaitDecorrelatedJitter`
  (with `jitter`)

Built-in wait semantics:

- `fixed(dur)` always returns `dur`
- `linear(initial, increment)` returns
  `initial + (attempt - 1) * increment` with saturating arithmetic
- `exponential(initial)` returns `initial * 2^(attempt - 1)` with saturating
  arithmetic
- `.chain(other, after)` uses the first strategy when `attempt <= after`, then
  uses `other` when `attempt > after`; when `after` is `0`, the first strategy
  is never consulted
- the second strategy in `.chain(...)` receives the original global
  `RetryState` unchanged; attempt counting is not rebased at the switch point
- `.add(other)` combines two strategies by summing their outputs. Equivalent to
  the `+` operator. `(a + b).next_wait(state)` returns
  `a.next_wait(state) + b.next_wait(state)` with saturating arithmetic.

Wait strategies only compute `Duration`. They do not sleep directly.

**Zero-duration sleep rule.** When a wait strategy returns `Duration::ZERO`
(or any other mechanism reduces the delay to zero), sleep is skipped entirely
— no sleep call is made and no async yield occurs. This makes
`wait::fixed(Duration::ZERO)` a valid "no delay" strategy for tight polling
loops. All other loop behavior (hooks, stop checks) proceeds normally. This
rule is referenced by the Timeout section and the loop pseudocode (step 10).

### Jitter strategies

All jitter strategies require the `jitter` feature. Three are decorator methods
on the `Wait` trait that transform the inner strategy's output. One is a
standalone constructor that computes delays independently.

**Additive jitter** (`.jitter(max_jitter)`): adds a uniformly distributed
duration in `[0, max_jitter]` to the inner strategy's output.

```
output = base + random(0, max_jitter)
```

**Full jitter** (`.full_jitter()`): replaces the inner strategy's output with
a random value between zero and the computed base. This is the "Full Jitter"
strategy from the AWS Architecture Blog. It produces the lowest total client
work under contention.

```
output = random(0, base)
```

**Equal jitter** (`.equal_jitter()`): keeps half the computed base and jitters
the other half. This is the "Equal Jitter" strategy from the AWS Architecture
Blog. It guarantees a minimum delay of `base / 2` while still spreading
requests.

```
output = base / 2 + random(0, base / 2)
```

**Decorrelated jitter** (`wait::decorrelated_jitter(base)`): a standalone
strategy where each delay is random between `base` and three times the
previous delay. This is the "Decorrelated Jitter" strategy from the AWS
Architecture Blog. It is stateful, tracking the previous delay via interior
mutability (`Cell<Duration>`), consistent with the `&self` model.

```
output = random(base, last_sleep * 3)
```

On the first attempt, `last_sleep` is `base`. Decorrelated jitter composes
with `.cap(max)` to bound the maximum delay.

Because decorrelated jitter is stateful via `Cell<Duration>`, each concurrent
or sequential retry loop should use its own clone of a decorrelated jitter
strategy to ensure independent sequences. Cloning snapshots the current
`last_sleep` value; the two copies then diverge independently.

All jitter strategy types support `.with_seed([u8; 32])` and
`.with_nonce(u64)` for reproducible sequences.

Jitter decorators (`.jitter()`, `.full_jitter()`, `.equal_jitter()`) apply
before `.cap(...)`:

```rust
// Jitter applied to base, then capped:
wait::exponential(Duration::from_millis(100))
    .full_jitter()
    .cap(Duration::from_secs(30))
```

### Predicate

`Predicate<T, E>` decides whether a completed outcome should retry.
Composition methods are provided directly on the trait with
`where Self: Sized` bounds.

```rust
pub trait Predicate<T, E> {
    fn should_retry(&self, outcome: &Result<T, E>) -> bool;

    fn or<P: Predicate<T, E>>(self, other: P) -> predicate::PredicateAny<Self, P>
    where Self: Sized { ... }

    fn and<P: Predicate<T, E>>(self, other: P) -> predicate::PredicateAll<Self, P>
    where Self: Sized { ... }
}
```

`Predicate` uses `&self`. Most predicates are stateless closures; the rare
stateful predicate can use interior mutability (`Cell`, `AtomicUsize`).

> The `&self` receiver makes `Predicate<T, E>` trivially object-safe for
> fixed `T, E` and allows sharing across concurrent retry loops.

Built-in predicate constructors:

- `predicate::any_error() -> PredicateAnyError`: retries all `Err` values
  (`should_retry` returns `true` for any `Err`, `false` for any `Ok`)
- `predicate::error(f: impl Fn(&E) -> bool) -> PredicateError<F>`: retries
  when `f` returns `true` for the error (`should_retry` returns `true` for
  `Err(e)` when `f(&e)` is `true`, `false` for all `Ok` values, `false` for
  `Err(e)` when `f(&e)` is `false`)
- `predicate::ok(f: impl Fn(&T) -> bool) -> PredicateOk<F>`: retries when `f`
  returns `true` for the ok value (`should_retry` returns `true` for `Ok(v)`
  when `f(&v)` is `true`, `false` for all `Err` values)
- `predicate::result(f: impl Fn(&Result<T, E>) -> bool) -> PredicateResult<F>`:
  retries when `f` returns `true`

**`when` vs `until`.** The predicate is set on the policy or execution
builder via either `.when(p)` or `.until(p)`. Both set the same predicate
slot; the last call wins.

- `.when(p)` retries while `p.should_retry()` returns `true`.
- `.until(p)` retries until `p.should_retry()` returns `true` — i.e., it
  wraps `p` in `PredicateUntil`, which negates `should_retry()` results.

`.when()` is natural for error-based retry:
`.when(error(|e| e.is_transient()))` reads "retry when transient error."
`.until()` is natural for polling: `.until(ok(|s| s.is_ready()))` reads "retry
until ready."

When using `.until(ok(f))`, errors are retried by default because `ok(f)`
returns `false` for all `Err` values, and `until` inverts that to `true`
(retry). This is the expected behavior for a retry-first API — the user
reached for `retry()`, so errors are retriable by default. Users who want
specific errors to terminate during polling compose explicitly:
`.until(ok(|s| s.is_ready()).or(error(|e| e.is_fatal())))`.

`Predicate` is blanket-implemented for `Fn(&Result<T, E>) -> bool`. Named
predicate types (`PredicateAnyError`, `PredicateError<F>`, `PredicateOk<F>`,
`PredicateResult<F>`) have dedicated `impl Predicate<T, E>` blocks.

> Named types do not rely on the blanket `Fn` impl. This ensures they work
> consistently regardless of closure trait inference.

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

Sleep adapter helpers (`sleep::tokio()`, etc.) return closures that satisfy the
blanket `Sleeper` impl. They do not define named sleeper types.

Sync execution does not use `Sleeper`. It uses a blocking sleep function via
`.sleep(...)`, or `std::thread::sleep` when the `std` feature is active and no
explicit sync sleeper is provided.

### State types

The crate exposes three read-only state types. All are `#[non_exhaustive]` and
provide `new(...)` constructors for tests and custom strategy implementations.

> Public fields on `#[non_exhaustive]` structs are a deliberate choice for
> ergonomic read access. `#[non_exhaustive]` prevents construction outside the
> crate while allowing field reads. Field names are part of the stable API
> surface.

```rust
/// Shared state for Stop, Wait, the operation, and the before_attempt hook.
pub struct RetryState {
    /// 1-indexed attempt number. For the operation and `before_attempt`,
    /// this is the attempt about to start. For `Stop` and `Wait`, this
    /// is the just-completed attempt.
    pub attempt: u32,
    /// Wall-clock time since retry execution started, or `None` when
    /// no elapsed clock is available.
    pub elapsed: Option<Duration>,
}
```

```rust
/// State passed to the after_attempt hook.
pub struct AttemptState<'a, T, E> {
    /// 1-indexed attempt number just completed.
    pub attempt: u32,
    /// Wall-clock time since retry execution started, or `None`.
    pub elapsed: Option<Duration>,
    /// Outcome of the just-completed attempt.
    pub outcome: &'a Result<T, E>,
    /// Delay before the next attempt, or `None` when this is the final
    /// attempt (stop fired or predicate accepted).
    pub next_delay: Option<Duration>,
}
```

```rust
/// State passed to the on_exit hook.
pub struct ExitState<'a, T, E> {
    /// 1-indexed number of completed attempts. Always >= 1.
    pub attempt: u32,
    /// Wall-clock time since retry execution started, or `None`.
    pub elapsed: Option<Duration>,
    /// Outcome of the final attempt.
    pub outcome: &'a Result<T, E>,
    /// Why the retry loop terminated.
    pub stop_reason: StopReason,
}
```

Constructor signatures:

```rust
impl RetryState {
    pub fn new(attempt: u32, elapsed: Option<Duration>) -> Self;
}

impl<'a, T, E> AttemptState<'a, T, E> {
    pub fn new(
        attempt: u32,
        elapsed: Option<Duration>,
        outcome: &'a Result<T, E>,
        next_delay: Option<Duration>,
    ) -> Self;
}

impl<'a, T, E> ExitState<'a, T, E> {
    pub fn new(
        attempt: u32,
        elapsed: Option<Duration>,
        outcome: &'a Result<T, E>,
        stop_reason: StopReason,
    ) -> Self;
}
```

`RetryState` usage across contexts:

|Context              |`attempt`             |`elapsed`|
|---------------------|----------------------|---------|
|User operation       |about-to-start attempt|available|
|`before_attempt` hook|about-to-start attempt|available|
|`Wait::next_wait`    |just-completed attempt|available|
|`Stop::should_stop`  |just-completed attempt|available|

The operation receives `RetryState` by value (it is `Copy`). The numeric value
of `attempt` is the same across all four contexts within a single loop
iteration — the semantic distinction is whether the attempt has run yet.

> Passing by value avoids lifetime entanglement between the state and async
> futures produced by the operation.

Field meanings:

- `attempt` is 1-indexed for completed or about-to-start attempts
- `elapsed` is `None` when no elapsed clock is available
- `AttemptState.next_delay` is `None` on the final attempt; `Some(delay)` means
  another attempt will follow after the delay
- `ExitState.attempt` is always the number of completed attempts
- `ExitState.outcome` is always the outcome of the final attempt

## Error types

### RetryError

The predicate's job is binary: retry or don't. When it says don't retry, the
retry loop terminates regardless of whether the outcome was `Ok` or `Err`. This
means a predicate-accepted `Ok` and a predicate-accepted `Err` share the same
stop reason (`Accepted`). Predicate-accepted `Ok` values are returned directly
as `Ok(T)`. Predicate-accepted `Err` values are wrapped in
`RetryError::Rejected`.

```rust
pub enum RetryError<T, E> {
    /// Retries exhausted — the stop strategy fired while the predicate
    /// still wanted to retry. The last outcome is preserved.
    Exhausted { last: Result<T, E> },
    /// The predicate accepted an `Err` outcome as terminal (did not
    /// request retry).
    Rejected { last: E },
}
```

`RetryResult<T, E>` is `Result<T, RetryError<T, E>>`.

Accessor methods:

- `last() -> Option<&Result<T, E>>`: the final `Result<T, E>` if the variant
  carries one; returns `Some` for `Exhausted`, `None` for `Rejected` (which
  stores only `E`)
- `into_last() -> Option<Result<T, E>>`: consuming version with the same
  `None` cases as `last()`
- `last_error() -> Option<&E>`: the final `E` if the variant carries one;
  returns `Some` for `Rejected` and for `Exhausted` when the last outcome is
  `Err`; `None` otherwise
- `into_last_error() -> Option<E>`: consuming version
- `stop_reason() -> StopReason`: the termination reason as a typed enum

Display: `RetryError` implements `Display` when `E: Display`. Display output
is lowercase, without trailing punctuation, following the pattern
`{variant}: {error}`. Examples: `retries exhausted: connection refused`,
`rejected: invalid argument`.

`RetryError` implements `std::error::Error` when `std` is active and
`E: std::error::Error + 'static`, `T: fmt::Debug + 'static`.

### StopReason

```rust
pub enum StopReason {
    /// The predicate accepted the outcome (did not request retry).
    /// Covers both predicate-accepted `Ok` (returned as `Ok(T)`) and
    /// predicate-accepted `Err` (returned as `RetryError::Rejected`).
    Accepted,
    /// The stop strategy fired while the predicate still wanted to retry.
    Exhausted,
}
```

`StopReason` implements `Display` with lowercase labels: `"accepted"`,
`"retries exhausted"`.

Mapping from `RetryError` to `StopReason`:

|Terminal condition                    |`RetryError` variant  |`StopReason`|
|--------------------------------------|----------------------|------------|
|Predicate accepts `Ok(T)`             |(not an error)        |`Accepted`  |
|Predicate accepts `Err(E)` as terminal|`Rejected { last: E }`|`Accepted`  |
|Stop fires while retrying             |`Exhausted`           |`Exhausted` |

### RetryStats

```rust
pub struct RetryStats {
    /// Completed attempts. Always >= 1.
    pub attempts: u32,
    /// Wall-clock time from retry execution start until terminal exit,
    /// or `None` when no clock is available.
    pub total_elapsed: Option<Duration>,
    /// Sum of delays for retries that reached the sleep phase (step 9
    /// onward). This is requested wait budget, not measured sleep time.
    /// Includes zero-duration delays (which skip actual sleep). Excludes
    /// delays computed but preempted by stop at steps 7–8.
    pub total_wait: Duration,
    /// Why the retry loop terminated.
    pub stop_reason: StopReason,
}
```

## Policy model

`RetryPolicy<S, W, P>` stores owned stop, wait, and predicate values.
`RetryPolicy` carries no trait bounds on the struct definition. Bounds appear
only on `impl` blocks.

> This follows the Rust API guideline C-STRUCT-BOUNDS and ensures that adding
> derived traits is never a breaking change.

Construction:

```rust
impl RetryPolicy<StopAfterAttempts, WaitExponential, PredicateAnyError> {
    pub fn new() -> Self { ... }
}

impl Default for RetryPolicy<StopAfterAttempts, WaitExponential, PredicateAnyError> {
    fn default() -> Self { Self::new() }
}
```

`new()` creates a ready-to-run policy with bounded retries: `attempts(3)`,
`exponential(100ms)`, and `any_error()`. Because `PredicateAnyError` implements
`Predicate<T, E>` for all `T, E`, neither `T` nor `E` is fixed by `new()` —
they are inferred at the call site when the operation is provided.

Builder methods:

- `.when(p: impl Predicate<T, E>) -> RetryPolicy<S, W, P2>`
- `.until(p: impl Predicate<T, E>) -> RetryPolicy<S, W, P2>`
- `.wait(w: impl Wait) -> RetryPolicy<S, W2, P>`
- `.stop(s: impl Stop) -> RetryPolicy<S2, W, P>`
- `.boxed() -> RetryPolicy<Box<dyn Stop + Send + 'static>, Box<dyn Wait + Send + 'static>, Box<dyn Predicate<T, E> + Send + 'static>>`
  with `alloc`

`.when(p)` and `.until(p)` both set the predicate. `.when(p)` stores `p`
directly. `.until(p)` wraps `p` in `PredicateUntil<P>`, which negates
`should_retry()` results. The last call wins; they do not compose with each
other.

> This follows the same type-level composition pattern as `StopAny`,
> `WaitCapped`, and other combinator types. `PredicateUntil` is listed in the
> combinator type opacity section — users should not name it directly.

`RetryPolicy` is `Clone` when its components are `Clone`. Because all trait
methods use `&self`, policies are freely shareable without interior mutability.
`RetryPolicy<S, W, P>` is a pure composition of three strategy types with no
other internal state.

## Execution model

`RetryPolicy::retry(op)` borrows `&self` and returns `SyncRetry`, which
supports hook configuration and terminal execution. The operation receives
`RetryState` by value on each invocation.

`RetryPolicy::retry_async(op)` borrows `&self` and returns
`AsyncRetry`, which supports hook configuration, sleeper configuration,
and terminal execution. The operation receives `RetryState` by value on each
invocation.

`SyncRetry` and `AsyncRetry` are the policy-borrowing builder types. They
share the same method surface as `SyncRetryBuilder` / `AsyncRetryBuilder`
(hooks, sleep, timing, execution) but do not expose strategy overrides
(`.stop()`, `.wait()`, `.when()`, `.until()`) because those are configured on
the policy itself.

Because `Stop`, `Wait`, and `Predicate` all use `&self`, the policy holds no
mutable state. Multiple concurrent retry loops can share the same policy
without cloning.

### Free function entry points

Free functions provide an alternative entry point that does not require
constructing a `RetryPolicy` first. They use the same defaults as
`RetryPolicy::new()`: `attempts(3)`, `exponential(100ms)`, `any_error()`.

```rust
/// Sync retry with default policy.
pub fn retry<F, T, E>(op: F) -> SyncRetryBuilder<...>
where F: FnMut(RetryState) -> Result<T, E>;

/// Async retry with default policy.
pub fn retry_async<F, T, E, Fut>(op: F) -> AsyncRetryBuilder<...>
where F: FnMut(RetryState) -> Fut, Fut: Future<Output = Result<T, E>>;
```

Free functions accept `FnMut(RetryState) -> ...`, giving the operation access
to attempt number and elapsed time.

### Extension traits

`RetryExt` and `AsyncRetryExt` provide method-call syntax for closures and
functions that return `Result`. They use the same defaults as
`RetryPolicy::new()`.

```rust
pub trait RetryExt<T, E>: FnMut() -> Result<T, E> + Sized {
    fn retry(self) -> SyncRetryBuilder<...>;
}

pub trait AsyncRetryExt<T, E, Fut>: FnMut() -> Fut + Sized
where Fut: Future<Output = Result<T, E>>
{
    fn retry_async(self) -> AsyncRetryBuilder<...>;
}
```

Both traits have blanket implementations for all matching closures and function
pointers. The operation does not receive `RetryState`; the closure is wrapped
internally as `move |_| op()`.

> Extension traits are the convenience path for one-shot retry with minimal
> ceremony. Use the free functions `retry()` / `retry_async()` when the
> operation needs access to `RetryState`.

Usage:

```rust
use tenacious::{retry, retry_async, RetryExt, AsyncRetryExt};
use tenacious::{stop, wait, sleep};
use tenacious::predicate::{any_error, error, ok};

// Ext: one-shot retry with zero config
(|| fetch_data()).retry().call()?;

// Ext: one-shot with builder config
(|| fetch_data()).retry()
    .when(error(|e| e.is_transient()))
    .wait(wait::exponential(Duration::from_millis(200)))
    .stop(stop::attempts(5))
    .call()?;

// Ext: one-shot async
(|| fetch_data()).retry_async()
    .sleep(sleep::tokio())
    .await?;

// Ext: polling
(|| check_status()).retry()
    .until(ok(|s| s.is_complete()))
    .wait(wait::fixed(Duration::from_secs(1)))
    .stop(stop::attempts(20))
    .call()?;

// Free function: operation needs attempt state
retry(|state| {
    let timeout = Duration::from_secs(state.attempt as u64 * 2);
    fetch_data_with_timeout(timeout)
})
.stop(stop::attempts(5))
.call()?;

// Free function: async with attempt state
retry_async(|state| async move {
    let timeout = Duration::from_secs(state.attempt as u64 * 2);
    fetch_data_with_timeout(timeout).await
})
.sleep(sleep::tokio())
.await?;
```

### Builder method signatures

All execution builders (`SyncRetry`, `SyncRetryBuilder`, `AsyncRetry`,
`AsyncRetryBuilder`) are generic over `S: Stop`, `W: Wait`,
`P: Predicate<T, E>`, and (for async) `Sl: Sleeper`.

#### Strategy overrides

Strategy overrides are available only on `SyncRetryBuilder` and
`AsyncRetryBuilder` (the ext-trait / free-function builders that own their
policy). `SyncRetry` and `AsyncRetry` borrow the policy and do not expose
these methods.

- `.when(p: impl Predicate<T, E>) -> ...Builder<S, W, P2, ...>`
- `.until(p: impl Predicate<T, E>) -> ...Builder<S, W, P2, ...>`
- `.wait(w: impl Wait) -> ...Builder<S, W2, P, ...>`
- `.stop(s: impl Stop) -> ...Builder<S2, W, P, ...>`

#### Timing

- `.elapsed_clock(fn() -> Duration)` — sets the elapsed clock to a bare
  function pointer; always available, including `no_std` without `alloc`
- `.elapsed_clock_fn(impl Fn() -> Duration)` with `alloc` — boxes the closure
  internally, supporting captures for test clocks and runtime state
- `.timeout(dur: Duration)` — sets a wall-clock deadline for the entire retry
  execution, including all attempts and all sleeps (see Timeout)

See Elapsed clock contract and Timeout for detailed semantics.

#### Execution

Sync-only:

- `.sleep(f: impl FnMut(Duration)) -> ...Builder<...>` — sets the blocking
  sleep function; the closure is stored in the builder and called once per
  sleep; must be `Send` when the builder is `Send`

Async-only:

- `.sleep(sl: impl Sleeper) -> ...Builder<S, W, P, Sl2>`

#### Hooks

- `.before_attempt(f: impl FnMut(&RetryState))`
- `.after_attempt(f: impl FnMut(&AttemptState<'_, T, E>))`
- `.on_exit(f: impl FnMut(&ExitState<'_, T, E>))`

Hook methods do not change the strategy type parameters. See Hooks for timing,
ordering, and panic behavior.

### Sync execution

Calling `.sleep(...)` on `SyncRetryBuilder` is:

- optional with `std` (defaults to `std::thread::sleep`)
- required without `std`

Terminal execution:

- `.call() -> RetryResult<T, E>`: executes the retry loop and returns the
  result
- `.with_stats()` changes the builder so that `.call()` returns
  `(RetryResult<T, E>, RetryStats)` instead

The sync loop performs these steps:

```
attempt = 1
loop:
    1.  Fire `before_attempt` with RetryState { attempt }.
    2.  Call the user operation with RetryState { attempt }.
    3.  Evaluate the predicate.
    4.  If the predicate does not retry:
        a. Fire `after_attempt` with next_delay = None.
        b. Terminate.
    5.  Compute the next wait duration via Wait::next_wait.
    6.  If timeout is configured and elapsed is Some, clamp delay to
        max(0, timeout - elapsed). (See Timeout.)
    7.  Evaluate the stop strategy.
    8.  If stop fires:
        a. Fire `after_attempt` with next_delay = None.
        b. Terminate.
    9.  Fire `after_attempt` with next_delay = Some(delay).
    10. If delay > zero, sleep for the computed delay.
    11. attempt += 1, continue to step 1.
```

### Async execution

Async execution always requires `.sleep(...)` before the future can run. The
crate never auto-selects an async runtime.

Terminal execution:

- `AsyncRetryBuilder` implements `IntoFuture<Output = RetryResult<T, E>>`,
  consuming the builder to produce the retry future
- `.with_stats()` changes the builder to implement
  `IntoFuture<Output = (RetryResult<T, E>, RetryStats)>` instead

The async future is single-use:

- it is directly awaitable via `IntoFuture`
- polling after completion always panics

The async loop uses the same transition order as sync execution.

### Statistics

Retry execution always tracks statistics internally. The cost is one `u32`
counter, one `Option<Duration>` accumulator, one `Duration` accumulator, and
one `StopReason` — no additional allocations or timing beyond what the chosen
clock already provides.

Statistics are accessed via:

- `.with_stats().call()` (sync) or `.with_stats().await` (async): returns
  `(RetryResult<T, E>, RetryStats)` alongside the result
- the `on_exit` hook, which receives `ExitState` containing `attempt`,
  `elapsed`, and `stop_reason`

## Termination semantics

Retry termination is defined by the final accepted outcome, the predicate, and
the stop strategy.

### Termination table

|Final condition                       |Return value                |`StopReason`|`ExitState.outcome`|
|--------------------------------------|----------------------------|------------|-------------------|
|Predicate accepts `Ok(T)`             |`Ok(T)`                     |`Accepted`  |`&Ok(T)`           |
|Predicate accepts `Err(E)` as terminal|`Err(RetryError::Rejected)` |`Accepted`  |`&Err(E)`          |
|Stop fires while retrying             |`Err(RetryError::Exhausted)`|`Exhausted` |`&last_result`     |

### Additional guarantees

- `after_attempt` fires after every completed attempt, including the final one
- on the final attempt, `after_attempt` fires with `next_delay = None`
- `next_delay = Some(delay)` means another attempt will follow after the delay
- predicate evaluation always happens before stop evaluation
- stop evaluation always happens after wait computation
- `Exhausted` carries `last: Result<T, E>` — callers match on the inner
  `Result` to distinguish exhausted-error from unmet-condition cases
- `RetryResult<T, E>` is `Result<T, RetryError<T, E>>`
- `ExitState.attempt` is always the number of completed attempts (>= 1)

## Hooks

Hooks are configured on execution builders, not on `RetryPolicy`.

Timing guarantees:

- `before_attempt` fires before the user operation starts
- `after_attempt` fires after every completed attempt, including the final one
- `on_exit` fires exactly once for each non-panicking terminal path

Ordering guarantees:

- hooks of the same kind fire in registration order
- without `alloc`, each hook point stores at most one callback; registering a
  second callback for the same hook point silently replaces the first
- with `alloc`, hooks are stored in a `Vec`; multiple hooks of the same kind
  may be registered and all fire in registration order

Hook panics propagate normally. The crate does not catch them.

Panic behavior:

- if the operation or any hook panics, retry execution aborts immediately
- once a panic starts unwinding, remaining hooks for that execution are not run
  (including `on_exit`)
- if a hook panics, the operation's return value for that attempt, if any, is
  dropped during unwinding and is not recoverable by the caller
- a panic inside `on_exit` propagates normally; `on_exit` is not re-invoked or
  wrapped in a catch guard
- if a panic is caught by the caller via `catch_unwind`, `RetryStats` and
  `on_exit` are not recoverable; the execution is in an indeterminate state

## Cancellation guidance

The crate does not provide a built-in cancellation mechanism. Async callers use
standard Rust future cancellation; sync callers use `.timeout()` or in-closure
checks.

### Async cancellation

Dropping the retry future at any `.await` point cleanly terminates the retry
loop. Standard patterns work as expected:

```rust
// Timeout
tokio::time::timeout(Duration::from_secs(30),
    (|| fetch_data()).retry_async().sleep(sleep::tokio())
).await??;

// Select
tokio::select! {
    result = (|| fetch_data()).retry_async().sleep(sleep::tokio()) => {
        handle(result?);
    }
    _ = shutdown_signal() => {
        log::info!("cancelled");
    }
}
```

When the retry future is dropped, `on_exit` does not fire. This is the same
behavior as panics and is consistent with the Rust async cancellation model.
Callers who need guaranteed cleanup should use `Drop` impls on their own types,
not retry hooks.

### Sync cancellation

For deadline-based cancellation, use `.timeout(dur)`:

```rust
(|| fetch_data()).retry()
    .timeout(Duration::from_secs(30))
    .call()?;
```

For flag-based cancellation (e.g., Ctrl-C), check the flag inside the
operation closure:

```rust
let cancelled = Arc::new(AtomicBool::new(false));

// Set `cancelled` from a signal handler or another thread.

retry(|_| {
    if cancelled.load(Ordering::Relaxed) {
        return Err(MyError::Cancelled);
    }
    do_work()
})
.stop(stop::attempts(100))
.call()?;
```

This provides cancellation at attempt boundaries. The cancel error flows
through the normal predicate: with the default `any_error()`, it is retried
like any other error, so the flag must remain set. With a selective predicate
like `.when(error(|e| e.is_transient()))`, non-transient cancel errors
terminate immediately as `RetryError::Rejected`.

> The crate does not interrupt operations or sleeps mid-execution. Sync sleep
> (`std::thread::sleep`) is inherently uninterruptible. The maximum latency
> between flag-set and termination is one sleep duration plus one operation
> execution time. `.timeout()` bounds sleep duration via delay clamping.

## Thread safety

Async execution types (`AsyncRetryBuilder`) are `Send` when all of the
following are `Send`:

- the operation closure
- the operation's returned `Future`
- `T` and `E`
- `Sleeper::Sleep`
- all registered hooks

Async execution types are `!Send` otherwise. The crate never adds
unconditional `Send` bounds on public trait definitions (`Stop`, `Wait`,
`Predicate`, `Sleeper`). Concrete execution types derive `Send`/`Sync` from
their components via standard auto-trait rules.

Sync execution types are `Send` when their components are `Send`. No `Sync`
bound is required on any execution type because execution is driven by a single
owner (`.call()` for sync, `Future::poll()` for async).

The crate includes compile-time tests asserting that `RetryPolicy` with default
type parameters is `Send + Sync`, that `AsyncRetryBuilder` is `Send` when all
components are `Send`, and that `SyncRetryBuilder` is `Send` when all
components are `Send`.

## Elapsed time, clocks, and timeout

### Elapsed clock contract

The elapsed clock provides wall-clock timing to the retry loop. It is
configured on the execution builder and is set once per execution.

The clock function must return a monotonic timestamp — a `Duration` since an
arbitrary fixed epoch (e.g., system boot, program start, or hardware timer
origin). The library captures a baseline reading when execution starts and
computes elapsed time as `clock() - baseline` on each subsequent read.

With `std`, the default clock uses `std::time::Instant` internally: the
library calls `Instant::now()` at execution start and `.elapsed()` thereafter.
`Instant` does not directly satisfy `fn() -> Duration`; the `std` default is
handled as a special case. Without `std`, the clock defaults to `None` (no
elapsed tracking) unless explicitly configured via `.elapsed_clock()` or
`.elapsed_clock_fn()`.

Statistics do not force additional timing work beyond what the chosen clock
already provides.

### Hazard: elapsed-based stop without a clock

`stop::elapsed(dur)` never fires when `elapsed` is `None`. In `no_std`
environments without a configured elapsed clock, using only elapsed-based stop
strategies produces an unbounded retry loop.

To prevent this, combine elapsed-based stops with `stop::attempts(n)`:

```rust
stop::elapsed(Duration::from_secs(30)).or(stop::attempts(100))
```

The crate does not add a compile-time or runtime guard against this
configuration because whether a clock is present depends on the builder's
configuration. Users are responsible for ensuring at least one stop condition
will eventually fire.

### Hazard: elapsed-based stop does not account for upcoming sleep

`stop::elapsed(dur)` fires when elapsed time at attempt completion exceeds
`dur`. The retry loop may schedule a sleep that pushes total wall-clock time
well beyond the threshold. For example, `stop::elapsed(Duration::from_secs(30))`
will not prevent a retry sequence from running for 45 seconds total if the
elapsed check passes at 28 seconds and the next sleep is 17 seconds.

Users who need a wall-clock deadline should use `.timeout(dur)` instead of
or in addition to `stop::elapsed(dur)`.

### Timeout

`.timeout(dur)` on the execution builder sets a wall-clock deadline for the
entire retry execution. It combines two behaviors:

1. Implicitly OR's `stop::elapsed(dur)` into the effective stop strategy at
   execution time, so the stop check fires once elapsed time exceeds the
   deadline. The OR is applied to whatever stop strategy is in effect at
   execution time — whether set on the policy, overridden on the builder via
   `.stop()`, or the default. For example,
   `.stop(stop::attempts(5)).timeout(Duration::from_secs(30))` produces an
   effective stop of `stop::attempts(5).or(stop::elapsed(30s))`.
1. After computing the wait duration (step 5), if `elapsed` is `Some`, clamps
   the delay to the remaining budget:
   `delay = min(delay, max(0, timeout - elapsed))` (step 6). When `elapsed` is
   `None`, the clamp is skipped (no budget can be computed).

When the clamped delay is zero, sleep is skipped entirely per the zero-duration
sleep rule defined in the Wait section.

The stop reason when timeout causes termination is `Exhausted`, since the
elapsed stop fired. Users can distinguish timeout from attempt exhaustion by
comparing `RetryStats.total_elapsed` against their timeout duration.

Timeout relies on an elapsed clock. With `std`, `.timeout(dur)` automatically
configures the `std::time::Instant` clock if no clock has been set. Without
`std`, timeout requires `.elapsed_clock()` or `.elapsed_clock_fn()` to be
configured; if no clock is available, the elapsed stop never fires and the
delay clamp is skipped, so timeout has no effect. This is the same hazard as
`stop::elapsed` without a clock.

Timeout does not interrupt a running operation. The total wall-clock time may
exceed the deadline by the execution time of the final attempt. This is
consistent with the crate's guarantee that it never interrupts a user operation
that is already running.

## Feature-gated APIs

The crate exposes feature-gated helpers in these areas.

### Sleep adapters

The `sleep` module exports these helpers when their features are enabled:

- `sleep::tokio()` with `tokio`
- `sleep::embassy()` with `embassy`
- `sleep::gloo()` with `gloo-timers` on `wasm32`
- `sleep::futures_timer()` with `futures-timer`

These return closures satisfying the blanket `Sleeper` impl:
`impl Fn(Duration) -> impl Future<Output = ()>`. They do not define named
sleeper types. Callers may always pass their own compatible sleeper function or
type.

### Jitter

With the `jitter` feature:

Decorator methods on `Wait`:

- `.jitter(max_jitter)` — additive uniform jitter
- `.full_jitter()` — random in `[0, base]`
- `.equal_jitter()` — `base/2 + random(0, base/2)`

Standalone constructor:

- `wait::decorrelated_jitter(base: Duration) -> WaitDecorrelatedJitter` —
  random in `[base, last_sleep * 3]`

Exported types: `WaitJitter`, `WaitFullJitter`, `WaitEqualJitter`,
`WaitDecorrelatedJitter`.

All jitter types support `.with_seed([u8; 32])` and `.with_nonce(u64)` for
reproducible sequences.

### Boxed policies

With `alloc`:

`.boxed()` on `RetryPolicy` requires `S: Stop + Send + 'static`,
`W: Wait + Send + 'static`, `P: Predicate<T, E> + Send + 'static` and returns
`RetryPolicy<Box<dyn Stop + Send + 'static>, Box<dyn Wait + Send + 'static>, Box<dyn Predicate<T, E> + Send + 'static>>`.

> Object safety is satisfied because `Stop`, `Wait`, and `Predicate<T, E>`
> (for fixed `T, E`) each have a single non-generic method with an `&self`
> receiver. Composition methods are gated behind `where Self: Sized` and are
> not available on trait objects, which is correct.

No public type aliases are provided for boxed policy types. Users who need to
name the type can write it explicitly or use `impl` return types.

## Public API surface

The following items are part of the supported surface for new code.

### Crate root exports

Types:

- `RetryPolicy`
- `RetryError`, `RetryResult`
- `RetryStats`, `StopReason`
- `RetryState`, `AttemptState`, `ExitState`
- `SyncRetry`, `SyncRetryWithStats`, `AsyncRetry`, `AsyncRetryWithStats`
  (policy-borrowing builders and their stats variants)
- `SyncRetryBuilder`, `AsyncRetryBuilder` (ext-trait / free-function builders)

Traits:

- `Stop` (includes `.or()`, `.and()` as provided methods)
- `Wait` (includes `.cap()`, `.chain()`, `.add()` as provided methods;
  `.jitter()`, `.full_jitter()`, `.equal_jitter()` with `jitter`)
- `Predicate` (includes `.or()`, `.and()` as provided methods)
- `Sleeper`
- `RetryExt` (blanket-implemented for `FnMut() -> Result<T, E>`)
- `AsyncRetryExt` (blanket-implemented for
  `FnMut() -> Fut where Fut: Future<Output = Result<T, E>>`)

Free functions:

- `retry`
- `retry_async`

### Module exports

`stop` module:

- constructors: `attempts`, `elapsed`, `never`
- types: `StopAfterAttempts`, `StopAfterElapsed`, `StopNever`, `StopAny`,
  `StopAll`

`wait` module:

- constructors: `fixed`, `linear`, `exponential`
- types: `WaitFixed`, `WaitLinear`, `WaitExponential`, `WaitCapped`,
  `WaitChain`, `WaitCombine`
- conditionally (with `jitter`): `decorrelated_jitter` constructor;
  `WaitJitter`, `WaitFullJitter`, `WaitEqualJitter`, `WaitDecorrelatedJitter`
  types

`predicate` module:

- constructors: `any_error`, `error`, `ok`, `result`
- types: `PredicateAnyError`, `PredicateError`, `PredicateOk`,
  `PredicateResult`, `PredicateAny`, `PredicateAll`, `PredicateUntil`

`sleep` module:

- constructors: `tokio`, `embassy`, `gloo`, `futures_timer` (feature-gated)

`builders` module:

- `SyncRetryBuilder`, `SyncRetryBuilderWithStats`
- `AsyncRetryBuilder`, `AsyncRetryBuilderWithStats`
- `DefaultSyncRetryBuilder`, `DefaultSyncRetryBuilderWithStats`
- `DefaultAsyncRetryBuilder`, `DefaultAsyncRetryBuilderWithStats`

These are the full type-state builder types and default aliases. Use them when
builder types must appear in function signatures. The non-default variants are
also re-exported at the crate root; the `Default*` aliases and `*WithStats`
builder variants are only available through this module.

### Combinator type opacity

Combinator types (`StopAny`, `StopAll`, `WaitCapped`, `WaitChain`,
`WaitCombine`, `PredicateAny`, `PredicateAll`, `PredicateUntil`, etc.) are public for technical
reasons (they appear in return types of composition methods and `.until()`),
but users should not name them in function signatures. Use `impl Stop`,
`impl Wait`, or `impl Predicate<T, E>` instead.

> Combinator type names may change in minor releases.

## Standard trait implementations

All public types implement `Debug` (C-DEBUG). Types implement `Clone`, `Copy`,
`PartialEq`, `Eq`, `Hash`, and `Default` when all their components support it.
Composite types derive traits conditionally on their type parameters.

|Type                   |`Clone`|`Copy`|`PartialEq`|`Eq`|`Hash`|`Default`|`Display` |
|-----------------------|-------|------|-----------|----|------|---------|----------|
|`RetryState`           |yes    |yes   |yes        |—   |—     |—        |—         |
|`AttemptState<'a,T,E>` |yes    |yes   |—          |—   |—     |—        |—         |
|`ExitState<'a,T,E>`    |yes    |yes   |—          |—   |—     |—        |—         |
|`RetryStats`           |yes    |yes   |yes        |yes |—     |—        |—         |
|`StopReason`           |yes    |yes   |yes        |yes |yes   |—        |yes       |
|`RetryError<T,E>`      |T,E    |—     |T,E        |T,E |—     |—        |E: Display|
|All stop strategy types|yes    |yes   |yes        |yes |—     |—        |—         |
|All wait strategy types|yes    |yes   |yes        |*   |—     |—        |—         |
|All predicate types    |F      |—     |—          |—   |—     |—        |—         |
|Combinator types (A,B) |A,B    |—     |—          |—   |—     |—        |—         |

Cells with type names (e.g. "T,E" or "F" or "A,B") indicate the trait is
implemented conditionally when those components implement the trait.

\* `WaitExponential` implements `PartialEq` but not `Eq` because it contains
an `f64` field (the exponential base). All other wait strategy types implement
both `PartialEq` and `Eq`.

`RetryStats` and `StopReason` do not implement `Default` because there is no
meaningful default `StopReason`.

## Panic inventory

The following conditions cause a panic. No other public constructor or method
panics. Saturating arithmetic is used throughout wait computation — overflow
produces `Duration::MAX`, not a panic.

- `stop::attempts(0)` panics unconditionally
- Polling the async retry future after it has returned `Poll::Ready` panics

All panic conditions are documented with `# Panics` sections in rustdoc.

## Compatibility guarantees

The crate guarantees the following project-wide properties.

- MSRV is Rust `1.85.0`
- the crate forbids `unsafe` with `#![forbid(unsafe_code)]`
- `Duration` is always `core::time::Duration`
- `RetryError` implements `Display` when `E: Display`
- `RetryError::stop_reason()` is always available regardless of type parameter
  bounds
- `RetryError` implements `std::error::Error` when `std` is active and
  `E: std::error::Error + 'static`, `T: fmt::Debug + 'static`
- all public items have rustdoc examples using `?` for error handling
- license is dual MIT OR Apache-2.0
