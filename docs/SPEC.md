# tenacious — Specification

> **Crate name placeholder:** `tenacious`. The final name should be confirmed before publishing.
> This document is self-contained. A coding agent can implement the library from this spec alone.

---

## Architecture

### Purpose

`tenacious` is a Rust library for retrying fallible operations and polling for conditions. It targets the full matrix of environments: `std`-based applications (servers, CLIs, tests), WASM, and `no_std` targets including embedded systems running embassy. The design is inspired by Python's tenacity library and improves on the existing Rust crate `backon` (v1.6.0) by adding composable strategy algebra, full-result retry predicates, rich callbacks, reusable policies, and optional statistics.

---

### Intended Usage

```rust
use tenacious::prelude::*;
use tenacious::sleep::tokio;

// One-shot inline retry
let result = RetryPolicy::new()
    .stop(stop::attempts(5) | stop::elapsed(Duration::from_secs(30)))
    .wait(wait::exponential(Duration::from_millis(100))
          .jitter(Duration::from_millis(50))
          .cap(Duration::from_secs(5)))
    .when(on::error(|e: &HttpError| e.status().is_server_error()))
    .retry_async(|| client.get("/api/data"))
    .before_sleep(|s| tracing::warn!(attempt = s.attempt, "retrying request"))
    .sleep(tokio())
    .await?;

// Reusable policy stored in a service struct
let mut policy = RetryPolicy::new()
    .stop(stop::attempts(3))
    .wait(wait::fixed(Duration::from_millis(500)));

let a = policy.retry(|| db.get_user(id)).call()?;
let b = policy.retry(|| db.get_order(id)).call()?;

// Polling / waitfor pattern — retry on Ok(None) until Ok(Some(_))
let record = RetryPolicy::new()
    .stop(stop::elapsed(Duration::from_secs(60)))
    .wait(wait::fixed(Duration::from_secs(1)))
    .when(on::wait_for_ok(Option::is_some))
    .retry_async(|| store.poll_record(id))
    .sleep(tokio())
    .await?;

// With statistics
let (result, stats) = RetryPolicy::new()
    .stop(stop::attempts(5))
    .wait(wait::exponential(Duration::from_millis(200)))
    .retry_async(|| fetch())
    .sleep(tokio())
    .with_stats()
    .await;

println!("completed in {} attempts, {}ms total", stats.attempts, stats.total_elapsed.unwrap_or_default().as_millis());
```

---

### Project Structure

```
tenacious/
├── Cargo.toml
├── src/
│   lib.rs               # crate root; re-exports public API
│   compat.rs            # conditional-import facade (core/alloc/std)
│   error.rs             # RetryError type
│   on.rs                # built-in retry predicate factories and composition
│   policy/
│   ├── mod.rs           # RetryPolicy builder + policy module surface
│   ├── ext/
│   │   ├── mod.rs        # extension-trait module surface
│   │   ├── sync_builder.rs
│   │   └── async_builder.rs
│   ├── execution/
│   │   ├── hooks.rs     # hook traits/chaining internals
│   │   ├── common.rs    # shared retry-loop transitions
│   │   ├── sync_exec.rs # sync execution engine
│   │   └── async_exec.rs# async execution engine
│   └── time.rs          # elapsed-time tracking helpers
│   predicate.rs         # Predicate trait definition
│   sleep.rs             # Sleeper trait + feature-gated implementations
│   state.rs             # RetryState, AttemptState, BeforeAttemptState, ExitState
│   stats.rs             # RetryStats and StopReason
│   stop/                # Stop trait + built-in stop strategies + NeedsStop
│   wait/                # Wait trait + WaitExt + built-in wait strategies
├── tests/
│   core_types.rs
│   stop_strategies.rs
│   wait_strategies.rs
│   retry_predicates.rs
│   policy_sync.rs
│   policy_sync/          # sync policy test modules
│   policy_async.rs
│   policy_async/         # async policy test modules
│   callbacks_hooks.rs
│   stats.rs
│   feature_compat.rs
│   quality_properties.rs
│   allocation_hot_paths.rs
│   wait_ext_ergonomics.rs
│   support/              # shared test utilities
```

> Modules listed as single files in examples (for example `stop.rs`, `wait.rs`)
> may be implemented as directories (`stop/mod.rs`, etc.) when internal
> splitting aids readability.

---

### Feature Flags

Feature flags follow the additive `core ⊂ alloc ⊂ std` hierarchy (serde convention). All flags are optional additions; the crate compiles and is useful at any level.

```toml
[features]
default = ["std"]

# std: enables std::thread::sleep for sync execution, Instant for elapsed tracking,
#      std::error::Error impl on RetryError, and std-gated sleep impls.
std = ["alloc"]

# alloc: enables heap allocation. Required for Box<dyn Stop>, Box<dyn Wait>,
#        Box<dyn Predicate>, and type-erased policy storage.
#        Not required when using concrete generic types.
alloc = []

# Runtime-specific async sleep implementations. Exactly zero or one should be
# activated per binary. All are no_std-compatible except tokio-sleep and
# futures-timer-sleep.
tokio-sleep = ["dep:tokio", "std"]
embassy-sleep = ["dep:embassy-time"]
gloo-timers-sleep = ["dep:gloo-timers"]
futures-timer-sleep = ["dep:futures-timer", "std"]

# tokio CancellationToken support for wake-driven cancellation during async
# sleep.
tokio-cancel = ["dep:tokio-util"]

# jitter: enables random jitter in wait strategies. Pulls in a small RNG.
#         Uses rand's SmallRng which is no_std-compatible.
jitter = ["dep:rand"]

# serde: enables Serialize/Deserialize on RetryPolicy and strategy types,
#        allowing policy configuration from files or environment.
serde = ["dep:serde"]

# strict-futures: panic on AsyncRetry repoll-after-completion in all builds.
#                 Without this feature, release builds return Poll::Pending.
strict-futures = []
```

`#![no_std]` is unconditional in `lib.rs`. The `std` and `alloc` features gate `extern crate std` and `extern crate alloc` respectively. Internal imports use a facade module (`src/compat.rs`) that re-exports from `core`/`alloc`/`std` depending on active features, keeping conditional compilation out of the main logic.

---

### Key Abstractions

#### `Stop` — when to give up

```rust
pub trait Stop {
    fn should_stop(&mut self, state: &RetryState) -> bool;
    fn reset(&mut self) {}   // called if policy is reused across independent retry loops
}
```

Built-in stop strategies compose with `|` (`StopAny`) and `&` (`StopAll`).

#### `Wait` — delay between attempts

```rust
pub trait Wait {
    fn next_wait(&mut self, state: &RetryState) -> Duration;
    fn reset(&mut self) {}
}
```

Built-in wait strategies compose with `+` (`WaitCombine`) and `.chain(other, after_n)` (`WaitChain`).

#### `Predicate` — what counts as failure

```rust
pub trait Predicate<T, E> {
    fn should_retry(&self, outcome: &Result<T, E>) -> bool;
}
```

Built-in predicates compose with `|` and `&`.

Because `T` and `E` are type parameters on the trait rather than on the method, predicates are typed to a specific operation's return type. The `on::error` and `on::result` factory functions produce typed predicates. Predicates are also blanket-implemented for `Fn(&Result<T, E>) -> bool`.

#### `Sleeper` — how to delay

```rust
pub trait Sleeper {
    type Sleep: Future<Output = ()>;
    fn sleep(&self, dur: Duration) -> Self::Sleep;
}
```

Blanket-implemented for `Fn(Duration) -> F where F: Future<Output = ()>`, so `tokio::time::sleep` works directly as a sleeper. Feature-gated concrete implementations are provided for each supported runtime. The sync execution path uses `std::thread::sleep` (std feature) or a user-supplied blocking sleep (no_std sync).

#### State types — shared read-only context

Four state types avoid exposing invalid fields at different points in the
retry loop:

```rust
// Passed to: Stop::should_stop, Wait::next_wait
// Contains only counters and timing — no outcome reference.
pub struct RetryState {
    pub attempt: u32,             // 1-indexed; the attempt that just completed
    pub elapsed: Option<Duration>,
    pub next_delay: Duration,     // populated after Wait::next_wait; zero before
    pub total_wait: Duration,
}

// Passed to: after_attempt, before_sleep hooks
pub struct AttemptState<'a, T, E> {
    pub attempt: u32,             // 1-indexed; the attempt that just completed
    pub outcome: &'a Result<T, E>,
    pub elapsed: Option<Duration>,
    pub next_delay: Duration,     // populated after Wait::next_wait; zero in other hooks
    pub total_wait: Duration,
}

// Passed to: before_attempt hook
pub struct BeforeAttemptState {
    pub attempt: u32,             // 1-indexed; the attempt about to begin
    pub elapsed: Option<Duration>,
    pub total_wait: Duration,
}

// Passed to: on_exit hook
pub struct ExitState<'a, T, E> {
    pub attempt: u32,             // 1-indexed; 0 only when cancelled before first attempt
    pub outcome: Option<&'a Result<T, E>>, // None only for pre-first-attempt cancellation
    pub elapsed: Option<Duration>,
    pub total_wait: Duration,
    pub reason: StopReason,
}
```

All four types are constructed internally by the execution engine and passed
by shared reference during normal retry execution. Direct construction is
supported for tests and custom strategy implementations.
`Predicate::should_retry` receives `&Result<T, E>` directly, not a state
struct.

#### `RetryPolicy` — the reusable configuration object

`RetryPolicy<S, W, P>` is a generic struct carrying owned `Stop`, `Wait`, and `Predicate` values. `RetryPolicy::new()` returns a policy with `S = NeedsStop`, a marker type that does not implement `Stop`. Retry execution methods (`retry`, `retry_async`, `boxed`) require `S: Stop`, so they are unavailable until `.stop(...)` is called. `RetryPolicy::default()` returns a safe, ready-to-run policy. When `alloc` is enabled, `BoxedRetryPolicy` provides a type-erased variant (`Box<dyn Stop>` etc.) for runtime-constructed policies and serde deserialization.

---

### Error Handling

```rust
pub enum RetryError<E, T = ()> {
    /// All retries exhausted; the operation kept returning Err.
    Exhausted { error: E, attempts: u32, total_elapsed: Option<Duration> },
    /// Predicate rejected an Err as non-retryable.
    PredicateRejected { error: E, attempts: u32, total_elapsed: Option<Duration> },
    /// The stop condition fired while the predicate was still rejecting Ok values.
    /// The last Ok value is moved here; no clone is required.
    ConditionNotMet { last: T, attempts: u32, total_elapsed: Option<Duration> },
}
```

In the common case (retry-on-error, accept any Ok), `T` defaults to `()` and
`ConditionNotMet` is unreachable. When custom predicates classify some errors
as non-retryable, `PredicateRejected` returns the current `Err` immediately.
When `on::ok` or `on::result` is used to retry on Ok values, the caller's
`Result<T, RetryError<E, T>>` carries the last Ok in the error variant if the
stop condition fires before the predicate accepts. The execution engine takes
ownership of the last value; no clone is required.

`RetryError` implements `std::error::Error` when `std` is active, `E: Error + 'static`, and `T: Debug`. `Display` is implemented unconditionally.

> **Planned change (iteration 13):** `RetryError` gains a `Cancelled` variant
> when cancellation support is added. See 13.6.

---

### Execution Model

Two execution paths share the same policy type:

**Sync:** `policy.retry(|| op()).sleep(std::thread::sleep).call()?`

The sync engine is a plain loop. It calls `op()`, consults `Stop` and `Predicate`, calls `Wait`, then calls the blocking sleep function. Requires no async runtime.

**Async:** `policy.retry_async(|| async { op() }).sleep(tokio::time::sleep).await?`

The async engine is an `async fn` (or equivalently a hand-written `Future` state machine). Between attempts it `.await`s the sleep future. The sleep function is accepted as a generic `impl Sleeper`, which the blanket impl covers for closures. No executor is required beyond `core::future::Future` machinery.

Both paths share all stop/wait/predicate/hook logic. The only difference is whether sleep is blocking or async.

Iteration 13 adds cancellation checks to both loops (poll-based for sync,
sleep-racing for async). See that iteration for the amended loop descriptions.

---

### Testing Strategy

All business logic (stop strategies, wait strategies, predicate composition, policy configuration) is tested with plain unit tests requiring no async runtime. The execution engine is tested with:

- **Sync tests:** real `std::thread::sleep` is avoided; tests inject a no-op or recording sleep closure. Elapsed-based stop strategies are tested via `std::thread::sleep` for necessary timing and by constructing `RetryState` directly for deterministic assertions.
- **Async tests:** a minimal executor via `core::task` polling (no external runtime dependency in tests). Async retry correctness shares the same transition logic as sync.
- **no_std compile test:** CI builds with `--target thumbv7m-none-eabi --no-default-features` and checks `wasm32-unknown-unknown` with `--features alloc,gloo-timers-sleep` to confirm the crate compiles without std. Runtime behavior is not tested on these targets.
- **Composition tests:** seeded property-style tests verify that `StopAny(a, b).should_stop()` equals `a.should_stop() || b.should_stop()` and analogous invariants for `Wait` and `Predicate` composition, over generated input sets with reproducible seeds.

Error paths that are genuinely difficult to trigger in real conditions (for
example, arithmetic overflow in backoff duration computation) use direct unit
tests on the strategy function with edge-case inputs. No dedicated mock
injection layer is required.

---

## Iteration 1: Core Types and Traits

**1.1** The crate root is `#![no_std]` unconditionally. It activates `extern crate alloc` when the `alloc` feature is enabled and `extern crate std` when the `std` feature is enabled.

**1.2** The `Stop` trait is defined in `stop.rs` with two methods: `should_stop(&mut self, state: &RetryState) -> bool` and `reset(&mut self)`. The `reset` method has a default no-op implementation.

**1.3** The `Wait` trait is defined in `wait.rs` with two methods: `next_wait(&mut self, state: &RetryState) -> Duration` and `reset(&mut self)`. The `reset` method has a default no-op implementation.

**1.4** The `Predicate<T, E>` trait is defined in `predicate.rs` with one method: `should_retry(&self, outcome: &Result<T, E>) -> bool`.

**1.5** The `Sleeper` trait is defined in `sleep.rs` with an associated type `Sleep: Future<Output = ()>` and a method `sleep(&self, dur: Duration) -> Self::Sleep`.

**1.6** `Sleeper` is blanket-implemented for any `F: Fn(Duration) -> Fut where Fut: Future<Output = ()>`, so callers can pass `tokio::time::sleep` directly without wrapping.

**1.7** `RetryState` is a struct with fields: `attempt: u32` (1-indexed), `elapsed: Option<Duration>`, `next_delay: Duration`, and `total_wait: Duration`. It carries only counters and timing — no outcome reference — and is passed to `Stop::should_stop` and `Wait::next_wait`. `AttemptState<'a, T, E>` extends this with `outcome: &'a Result<T, E>` and is passed to hooks that need outcome visibility.

**1.8** `RetryState` is passed by shared reference to `Stop::should_stop` and
`Wait::next_wait`. `AttemptState` is passed to `after_attempt` and
`before_sleep` hooks. `BeforeAttemptState` is passed to `before_attempt`.
`ExitState` is passed to `on_exit`. `Predicate::should_retry` receives
`&Result<T, E>` directly. During normal execution these state types are
constructed by the retry engine; direct construction is supported for tests and
custom strategy implementations.

**1.9** `RetryError<E, T = ()>` is defined in `error.rs` as an enum with
variants `Exhausted { error: E, attempts: u32, total_elapsed: Option<Duration>
}`, `PredicateRejected { error: E, attempts: u32, total_elapsed:
Option<Duration> }`, `ConditionNotMet { last: T, attempts: u32,
total_elapsed: Option<Duration> }`, and `Cancelled { last_result:
Option<Result<T, E>>, attempts: u32, total_elapsed: Option<Duration> }`.

**1.10** `RetryError<E, T>` implements `core::fmt::Display` unconditionally
and `std::error::Error` when the `std` feature is active and `E:
std::error::Error + 'static`.

**1.11** `Duration` is always `core::time::Duration`, which is available in no_std. No new duration type is introduced.

---

## Iteration 2: Stop Strategies

**2.1** `stop::attempts(n: u32)` produces a strategy that stops after `n`
completed attempts. The stop fires when `state.attempt >= n`. `n` must be at
least `1`; passing `0` panics. The fallible variant
`stop::attempts_checked(n) -> Result<StopAfterAttempts, StopConfigError>`
returns `Err(StopConfigError::ZeroAttempts)` instead of panicking. Use
`attempts` for hardcoded known-valid literals, and use `attempts_checked` for
runtime or untrusted configuration values.

**2.2** `stop::elapsed(dur: Duration)` produces a strategy that stops when `state.elapsed >= Some(dur)`. When `state.elapsed` is `None` (no clock), this strategy never fires.

**2.3** `stop::before_elapsed(dur: Duration)` produces a conservative strategy that stops when the elapsed time means the next attempt would likely exceed the deadline. It fires when `state.elapsed.map_or(false, |e| e + state.next_delay >= dur)`. This prevents starting an attempt that cannot complete within the budget.

`RetryPolicy::elapsed_clock(fn() -> Duration)` lets callers provide a custom
monotonic elapsed source. When configured, elapsed-based stop strategies use
that source even in `no_std` builds.

**2.4** `stop::never()` produces a strategy that always returns `false`. It is the correct explicit spelling of "retry indefinitely."

**2.5** Two stop strategies combine with `|` to produce `StopAny`, which stops when either constituent stops. The trait `BitOr<Rhs> for S where S: Stop, Rhs: Stop` is implemented for all `Stop` types, returning a `StopAny<S, Rhs>`.

**2.6** Two stop strategies combine with `&` to produce `StopAll`, which stops only when both constituents stop. The trait `BitAnd<Rhs> for S where S: Stop, Rhs: Stop` is implemented analogously.

**2.7** `StopAny` and `StopAll` are themselves `Stop` implementations, enabling chained composition: `stop::attempts(5) | stop::elapsed(dur) | stop::never()`.

**2.8** `Stop::reset` on `StopAny` and `StopAll` calls `reset` on both constituents.

**2.9** All stop strategy types are `Clone` and implement `Debug` where their fields implement `Debug`.

---

## Iteration 3: Wait Strategies

Fluent wait builders are provided by the `WaitExt` extension trait (re-exported
at the crate root and via `prelude`). Any type implementing `Wait` gets these
builder methods when `WaitExt` is in scope.

**3.1** `wait::fixed(dur: Duration)` produces a strategy that always returns `dur` regardless of attempt number or outcome.

**3.2** `wait::linear(initial: Duration, increment: Duration)` produces a strategy where the wait after attempt `n` is `initial + (n - 1) * increment`. Overflow saturates at `Duration::MAX`.

**3.3** `wait::exponential(initial: Duration)` produces a strategy where the wait after attempt `n` is `initial * 2^(n-1)`. Overflow saturates at `Duration::MAX`.

**3.4** `wait::exponential` accepts a builder method `.base(f: f64)` to change the multiplier from 2. Valid range is `[1.0, ∞)`. Values below 1.0 are clamped to 1.0 without panicking.

**3.5** All wait strategies expose `.cap(max: Duration)` via `WaitExt`. This
builder method clamps the computed wait to `max`. This is applied after jitter
if jitter is also configured.

**3.6** When the `jitter` feature is enabled, all wait strategies expose
`.jitter(max_jitter: Duration)` via `WaitExt`. Jitter is a uniformly random
value in `[0, max_jitter]` added to the computed wait before capping.
`WaitJitter` also exposes `.with_seed([u8; 32])` and `.with_nonce(u64)` for
deterministic and decorrelated sequences in tests or reproducible runs.

**3.7** Two wait strategies combine with `+` to produce `WaitCombine`, which returns the sum of both strategies' outputs. The `Add<Rhs> for W where W: Wait, Rhs: Wait` trait is implemented for all `Wait` types.

**3.8** A wait strategy can be chained to a fallback via
`WaitExt::chain(other: W2, after: u32)`. The resulting `WaitChain` uses `self`
for the first `after` attempts and `other` for all subsequent attempts.

**3.9** `Wait::reset` on `WaitCombine` calls reset on both constituents. `Wait::reset` on `WaitChain` resets both strategies and the internal attempt counter.

**3.10** All wait strategy types are `Clone` and implement `Debug` where their fields implement `Debug`.

**3.11** Wait strategies do not interact with the sleep mechanism directly. They return a `Duration`. The execution engine is responsible for sleeping.

---

## Iteration 4: Retry Predicates

**4.1** The `on` module provides factory functions for constructing predicates.

**4.2** `on::error(f: F) where F: Fn(&E) -> bool` produces a predicate that returns `true` (retry) when the outcome is `Err(e)` and `f(e)` is true.

**4.3** `on::any_error()` produces a predicate that returns `true` for any `Err(_)`, regardless of error type or content. This is the default behavior when no predicate is configured.

**4.4** `on::result(f: F) where F: Fn(&Result<T, E>) -> bool` produces a predicate that receives the full outcome and returns `true` (retry) when `f` does. This is the general form used for the waitfor pattern.

**4.5** `on::ok(f: F) where F: Fn(&T) -> bool` produces a predicate that returns `true` when the outcome is `Ok(v)` and `f(v)` is true, and `false` for any `Err`. This handles the polling use case cleanly.

**4.6** Two predicates combine with `|` to produce a predicate that retries when either constituent says to retry.

**4.7** Two predicates combine with `&` to produce a predicate that retries only when both constituents say to retry.

**4.8** `Predicate<T, E>` is blanket-implemented for any `Fn(&Result<T, E>) -> bool`, enabling inline closure use.

**4.9** The execution engine evaluates the predicate before evaluating `Stop`. If the predicate says do not retry (accept this outcome), the engine returns immediately regardless of remaining attempts.

**4.10** If no predicate is configured, the engine behaves as if `on::any_error()` is active — it retries on any `Err` and accepts any `Ok`.

---

## Iteration 5: Policy Builder and Sync Execution

**5.1** `RetryPolicy::new()` creates an unconfigured policy whose stop type is
`NeedsStop` (a marker that does not implement `Stop`). Retry execution methods
are unavailable until `.stop(...)` is called. `RetryPolicy::default()` returns
a safe, ready-to-run policy configured with `stop::attempts(3)`,
`wait::exponential(Duration::from_millis(100))`, and `on::any_error()`. The
unparameterized `RetryPolicy` type defaults to this safe configuration.

**5.2** `RetryPolicy` provides builder methods: `.stop(s: impl Stop)`,
`.wait(w: impl Wait)`, `.when(p: impl Predicate)`, and
`.elapsed_clock(clock: fn() -> Duration)`. Each method consumes and returns
`Self`. Hook builder methods (`.before_attempt(f)`, `.after_attempt(f)`,
`.before_sleep(f)`, `.on_exit(f)`) are provided on `SyncRetry` and `AsyncRetry`
instead of on `RetryPolicy`.

**5.3** The generic parameters of `RetryPolicy<S, W, P>` carry the concrete stop, wait, and predicate types. Calling `.stop(new_stop)` replaces the `S` type parameter, producing `RetryPolicy<NewStop, W, P>`. This preserves zero-cost abstraction for statically known policies.

**5.4** When the `alloc` feature is enabled, `RetryPolicy::boxed()` converts the policy to a `BoxedRetryPolicy` that stores `Box<dyn Stop>`, `Box<dyn Wait>`, and `Box<dyn Predicate>`. This enables storing policies in structs without threading type parameters.

**5.5** `RetryPolicy::retry(op: F) -> SyncRetry` where `F: FnMut() -> Result<T, E>` begins configuring a sync retry execution. The method takes `&mut self` so it can call `Stop::reset` and `Wait::reset` before beginning. `SyncRetry` borrows the policy for its lifetime and accepts `.sleep(f)` (optional when `std` is active).

**5.6** `SyncRetry::call() -> Result<T, RetryError<E, T>>` executes the retry
loop synchronously. The loop:
  1. Calls `op()`.
  2. Evaluates the predicate; if predicate says do not retry, returns the
     current outcome immediately (`Ok(value)` or
     `Err(RetryError::PredicateRejected { ... })`).
  3. Calls `Wait::next_wait` to compute the delay.
  4. Evaluates the stop condition with `state.next_delay` populated; if stop
     fires, returns `Err(RetryError::Exhausted { ... })` for `Err` outcomes or
     `Err(RetryError::ConditionNotMet { ... })` for retried `Ok` outcomes.
  5. Fires `before_sleep` hook with current `AttemptState`.
  6. Calls the sleep function with the computed delay.
  7. Increments attempt counter and repeats.

**5.7** `RetryPolicy` is `Clone` when all its constituent types are `Clone`.

**5.8** `RetryPolicy::retry` calls `Stop::reset` and `Wait::reset` at the start of each invocation, before the first attempt. This ensures a policy can be applied to multiple sequential operations without carrying state from a prior run.

**5.9** Hook callbacks are configured on `SyncRetry` and `AsyncRetry`.
`before_attempt` accepts `FnMut(&BeforeAttemptState)`. `after_attempt` and
`before_sleep` accept `FnMut(&AttemptState<'_, T, E>)`. `on_exit` accepts
`FnMut(&ExitState<'_, T, E>)`. All hooks have no return value. Panics in hooks
propagate normally.

**5.10** When no sleep function is provided and the `std` feature is inactive, the crate does not compile. Users on no_std must supply a sleep function via `.sleep(f)`.

---

## Iteration 6: Async Execution

**6.1** `RetryPolicy::retry_async(op: F) -> AsyncRetry` where `F: FnMut() -> Fut, Fut: Future<Output = Result<T, E>>` begins configuring an async retry execution.

**6.2** `AsyncRetry::sleep(s: impl Sleeper) -> AsyncRetry` sets the sleep implementation. This is required; there is no default async sleep even when a runtime feature is active. Rationale: in async contexts, the correct runtime is always knowable at the call site and implicit selection would silently break in multi-runtime binaries.

**6.3** `AsyncRetry` implements `Future<Output = Result<T, RetryError<E, T>>>`
and can be `.await`ed directly.

Polling an `AsyncRetry` after it has completed is misuse: debug builds panic.
Release builds return `Poll::Pending` unless the `strict-futures` feature is
enabled, in which case they also panic.

**6.4** The async execution loop follows the same logic as the sync loop
(5.6), replacing the blocking sleep call with an async sleep future. When a
canceler is configured (iteration 13), the loop polls both the sleep future
and `canceler.cancel()` so cancellation can interrupt sleeping attempts.

**6.5** The async engine does not spawn tasks or use any global state. It is a single poll-based state machine compatible with any executor that implements `core::task`, and does not perform per-attempt heap allocations in the retry loop.

**6.6** Runtime helper constructors are provided behind feature flags:
`tenacious::sleep::tokio()`, `tenacious::sleep::embassy()`,
`tenacious::sleep::gloo()`, and `tenacious::sleep::futures_timer()`. Each
returns a function pointer compatible with `.sleep(...)`.

**6.8** Hook callbacks in async execution are synchronous:
`before_attempt` receives `&BeforeAttemptState`, `after_attempt` and
`before_sleep` receive `&AttemptState<'_, T, E>`, and `on_exit` receives
`&ExitState<'_, T, E>`. Async hooks are not supported. Rationale: async
closures are not yet stable in Rust; adding them now would require boxing and
add complexity disproportionate to the benefit.

---

## Iteration 7: Callbacks and Hooks

**7.1** Four hook points are defined on execution builders (`SyncRetry` and
`AsyncRetry`): `before_attempt`, `after_attempt`, `before_sleep`, and
`on_exit`. `before_attempt` accepts `FnMut(&BeforeAttemptState)`.
`after_attempt` and `before_sleep` accept `FnMut(&AttemptState<'_, T, E>)`.
`on_exit` accepts `FnMut(&ExitState<'_, T, E>)`.

**7.2** `before_attempt` fires before `op()` is called on each attempt. It receives `BeforeAttemptState` with the attempt number about to execute, elapsed time, and cumulative sleep time. It does not have access to the previous attempt's outcome.

**7.3** `after_attempt` fires after `op()` returns and after the predicate is evaluated, but before the stop condition is checked. It receives the full `AttemptState` including the outcome.

**7.4** `before_sleep` fires after the stop condition has been checked and failed to stop (i.e., we have decided to retry). At this point `AttemptState.next_delay` is populated with the duration we are about to sleep.

**7.5** `on_exit` fires once when the retry loop terminates for any reason:
success, stop condition, predicate acceptance, or cancellation (when iteration
13 is implemented). It receives `ExitState`, which includes the final attempt
context and a `StopReason`. `ExitState.outcome` is `None` only when
cancellation fires before the first attempt.

**7.6** Multiple hook callbacks of the same kind can be registered. They fire in registration order. Builder method `.before_sleep(f)` appends to an internal list rather than replacing.

**7.7** When the `alloc` feature is inactive, each hook slot holds at most one callback. Attempting to register a second callback of the same kind is a compile-time error (not a runtime panic). This is enforced by the type system: the no-alloc hook setter methods are only available when the corresponding hook type parameter is `()`, so calling the setter once replaces `()` with a concrete type and the method disappears.

---

## Iteration 8: Statistics

**8.1** Calling `.with_stats()` on `SyncRetry` or `AsyncRetry` changes the
return type to `(Result<T, RetryError<E, T>>, RetryStats)`.

**8.2** `RetryStats` is a struct with fields: `attempts: u32`, `total_elapsed: Option<Duration>`, `total_wait: Duration`, `stop_reason: StopReason`.

**8.3** `StopReason` is an enum with variants: `Success`, `StopCondition`,
`PredicateAccepted` (predicate terminated retries before a stop condition,
including accepted `Ok` outcomes and predicate-rejected `Err` outcomes).

**8.4** Statistics are accumulated inside the execution engine only when `.with_stats()` is active. Without it, no timing calls are made solely for statistics purposes.

**8.5** When the `std` feature is inactive, `total_elapsed` in `RetryStats` is `None` unless a custom elapsed source is configured via `RetryPolicy::elapsed_clock`.

**8.6** `RetryStats` implements `Debug` and `Clone`. It implements `serde::Serialize` when the `serde` feature is active.

---

## Iteration 9: no_std and Feature Compatibility

**9.1** The crate compiles without errors or warnings for the `thumbv7m-none-eabi` target with `default-features = false`. CI enforces this with a `cargo build --target thumbv7m-none-eabi --no-default-features` step.

**9.2** The crate compiles for `wasm32-unknown-unknown` with `default-features = false, features = ["gloo-timers-sleep", "alloc"]`. CI enforces this.

**9.3** All public types are usable without heap allocation when the `alloc` feature is inactive. This means policies using concrete strategy types (not boxed trait objects) remain fully functional.

**9.4** The `jitter` feature pulls in `rand` with `default-features = false` and enables only `rand`'s `SmallRng` (which is no_std-compatible). It does not transitively enable `std` or `alloc` in `rand`.

**9.5** When `serde` is enabled, `RetryPolicy` serialization covers only the strategy configuration values (delay durations, max attempts, etc.). Hook callbacks and elapsed clock function pointers are not serializable and are not included. Deserialization of built-in strategy types validates constructor invariants (for example, `StopAfterAttempts` rejects `max == 0`). Jitter strategy serde includes `seed` and `nonce` so configured jitter streams can be reproduced.

**9.6** The facade module pattern is used to centralize conditional imports. A single internal module (`src/compat.rs`) re-exports `Duration` from `core::time`, `Vec` from `alloc::vec` (when `alloc`), `Box` from `alloc::boxed` (when `alloc`), and `String` from `alloc::string` (when `alloc`). All other modules import from this facade rather than from `core`/`alloc`/`std` directly.

**9.7** Elapsed time tracking uses `std::time::Instant` when `std` is active unless a custom elapsed source is provided. In `no_std`, callers can provide elapsed tracking through `RetryPolicy::elapsed_clock`; without it, elapsed remains `None`.

---

## Iteration 10: Public API Surface and Ergonomics

**10.1** The following items are re-exported from the crate root:
`RetryPolicy`, `BoxedRetryPolicy` (alloc), `RetryError`, `RetryStats`,
`StopReason`, `SyncRetry`, `SyncRetryWithStats`, `AsyncRetry` (alloc),
`AsyncRetryWithStats` (alloc), `RetryState`, `AttemptState`,
`BeforeAttemptState`, the `Stop` trait, `StopAll`, `StopAny`, `NeedsStop`,
`StopConfigError`, the `Wait` trait, `WaitExt`, `WaitCapped`, `WaitChain`,
`WaitCombine`, `WaitJitter` (jitter), the `Predicate` trait, the `Sleeper`
trait, the `stop` module, the `wait` module, the `on` module, and the `sleep`
module.

**10.2** The `sleep` module is re-exported and contains runtime-specific sleeper values (gated by features).

**10.3** The library provides no proc macros and no derive macros.

**10.4** All public types have complete documentation including at least one usage example in the doc comment.

**10.5** The crate exposes a `prelude` module re-exporting all traits and the
most common factory functions (`stop::attempts`, `stop::elapsed`,
`wait::exponential`, `wait::fixed`, `on::any_error`, `on::error`, `on::ok`,
`on::wait_for_ok`), allowing `use tenacious::prelude::*` to work without
further imports.

**10.6** The minimum supported Rust version (MSRV) is 1.85. This is required
by edition 2024 and declared in `Cargo.toml` via `rust-version = "1.85"`.

**10.7** The crate is `#![forbid(unsafe_code)]`. No unsafe is required for any feature. If a dependency requires unsafe, it is isolated and documented.

---

## Iteration 11: Release hardening, quality properties, and performance

This iteration adds release confidence guardrails in CI, seeded property-style
tests for composition logic, and lightweight benchmark and allocation
instrumentation for hot retry paths.

**11.1** CI enforces formatting (`cargo fmt --all --check`), tests
(`cargo test --all-targets`), and strict linting
(`cargo clippy --all-targets --all-features -- -D warnings`) on every push
and pull request.

**11.2** CI enforces documentation quality with doc tests (`cargo test --doc`)
and rustdoc warnings promoted to errors
(`RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps`).

**11.3** CI enforces MSRV compatibility at Rust 1.85 with
`cargo check --all-targets` and `cargo test --all-targets`.

**11.4** CI enforces no_std target compatibility by building
`thumbv7m-none-eabi` with `--no-default-features` and checking
`wasm32-unknown-unknown` with
`--no-default-features --features alloc,gloo-timers-sleep`.

**11.5** Seeded property-style tests validate composition invariants for
`Stop`, `Wait`, and `Predicate` over generated input sets. By default, tests
use a random run seed. Setting `TENACIOUS_PROPTEST_SEED` pins the run seed for
deterministic reproduction, and assertion failures must print the effective
seed and sample index.

**11.6** Allocation profile tests verify that concrete, non-boxed sync retry
execution paths are allocation-free during execution, and that boxed policy
paths allocate as expected.

**11.7** The crate provides a micro-benchmark target for hot sync execution
paths, runnable with `cargo bench --bench retry_hot_paths`, and CI verifies the
benchmark target compiles (`cargo bench --bench retry_hot_paths --no-run`).

---

## Iteration 12: Hooks Migration and `on_exit`

> **Status:** Implemented. This is a breaking change in the current
> unreleased API. Iterations 1–12 describe the current implemented API.

This iteration moves hook storage from `RetryPolicy` to the per-call execution
builders (`SyncRetry` / `AsyncRetry`), reducing `RetryPolicy` from 7 generic
type parameters to 3, and replaces the `on_exhausted` hook with a broader
`on_exit` hook.

This iteration supersedes: **5.2** (hook builder methods move off policy),
**5.3** (policy type params reduce to 3), **5.9** (hook callback signatures
unchanged but now on execution builder), **7.1–7.7** (hook mechanics unchanged
but host type changes from policy to builder), and the hook in the Intended
Usage one-shot example.

**12.1** `RetryPolicy<S, W, P>` stores only stop, wait, and predicate values.
It no longer carries hook type parameters. The current
`RetryPolicy<S, W, P, BA, AA, BS, OE>` signature becomes `RetryPolicy<S, W, P>`.

**12.2** Hook builder methods (`.before_attempt(f)`, `.after_attempt(f)`,
`.before_sleep(f)`, `.on_exit(f)`) move to `SyncRetry` and `AsyncRetry`. They
are chained between `.retry(op)` / `.retry_async(op)` and the terminal method
(`.call()` / `.await`):

```rust
policy
    .retry(|| op())
    .before_attempt(|s| log::info!("attempt {}", s.attempt))
    .after_attempt(|s| log::debug!("result: {:?}", s.outcome))
    .sleep(std::thread::sleep)
    .call()
```

**12.3** Hooks remain inline generic types on the execution builder (no
boxing). The no-alloc single-callback constraint (7.7) still applies, now
enforced on the builder's type parameters instead of the policy's.

**12.4** `on_exit` replaces `on_exhausted`. It fires once when the retry loop
terminates for any reason — success, stop condition, predicate rejection, or
cancellation (if iteration 13 is active). The callback receives an
`ExitState<'a, T, E>` containing termination context plus a `StopReason`
indicating why the loop ended. `outcome` is `None` only when cancellation
fires before the first attempt:

```rust
pub struct ExitState<'a, T, E> {
    pub attempt: u32,
    pub outcome: Option<&'a Result<T, E>>,
    pub elapsed: Option<Duration>,
    pub total_wait: Duration,
    pub reason: StopReason,
}
```

**12.5** `RetryPolicy` becomes significantly easier to store in structs and
name in type aliases:

```rust
type DbPolicy = RetryPolicy<StopAfterAttempts, WaitFixed, AnyError>;

struct Service {
    retry: DbPolicy,
}
```

**12.6** The policy remains reusable across operations. Hooks are per-call
configuration, matching the semantic reality that hooks are usually call-site
concerns (logging, metrics) rather than policy-level configuration.

---

## Iteration 13: Cancellation Support

> **Status:** Implemented.

This iteration adds cancellation support for both sync and async retry loops,
allowing external signals to interrupt retries between attempts and (in async)
during sleep.

This iteration amends: **1.9** / Error Handling (`Cancelled` variant added to
`RetryError`), **5.6** (loop gains cancellation checks), **8.3** (`StopReason`
gains `Cancelled`), and **10.1** (re-exports gain `Canceler`, `cancel` module).

**13.1** A `Canceler` trait is defined in `src/cancel.rs`:

```rust
pub trait Canceler {
    /// Future type used to detect cancellation while sleeping in async retry.
    type Cancel: Future<Output = ()>;

    /// Returns `true` if cancellation has been requested.
    fn is_cancelled(&self) -> bool;

    /// Returns a future that resolves when cancellation is requested.
    fn cancel(&self) -> Self::Cancel;
}
```

**13.2** `Canceler` is not intended for trait-object use. Async retry stores
`Canceler::Cancel` in its phase machine and polls it alongside the sleep
future, enabling wake-driven cancellation where implementations support it.

**13.3** Provided implementations:

- `cancel::never()` — ZST that always returns `false`. This is the default
  when no canceler is configured. Zero cost.
- `&AtomicBool` — `is_cancelled()` loads with `Acquire` ordering.
  `cancel()` returns `pending()` (poll-only, no wake-driven interruption).
  Available in `no_std`.
- `Arc<AtomicBool>` — same as `&AtomicBool` but owned. Requires `alloc`.
- `tokio_util::sync::CancellationToken` — `is_cancelled()` checks the token,
  `cancel()` returns `token.clone().cancelled_owned()` (a waking future that enables
  mid-sleep interruption). Requires the `tokio-cancel` feature.

**13.4** The execution builder gains `.cancel_on(c: impl Canceler)`:

```rust
policy
    .retry_async(|| fetch())
    .sleep(sleep::tokio())
    .cancel_on(token.clone())
    .await
```

**13.5** Cancellation amends the execution loop described in **5.6**:

- A new step 0 is inserted before each attempt: check
  `canceler.is_cancelled()`. If true, return
  `Err(RetryError::Cancelled { ... })` immediately.
- **Sync:** step 6 (blocking sleep) is followed by another `is_cancelled()`
  check before looping.
- **Async:** step 6 (async sleep) races the sleep future against
  `canceler.cancel()`. If cancellation wins, return
  `Err(RetryError::Cancelled { ... })`.
- Cancellation never interrupts a running user operation. It only fires
  between attempts and during sleep.

**13.6** `RetryError` gains a `Cancelled` variant:

```rust
Cancelled {
    last_result: Option<Result<T, E>>,
    attempts: u32,
    total_elapsed: Option<Duration>,
}
```

`last_result` is `Some` when cancellation happens after an attempt and `None`
when cancellation fires before the first attempt.

**13.7** `StopReason` gains a `Cancelled` variant for use in `RetryStats`.

**13.8** The `tokio-cancel` feature flag gates the `CancellationToken`
implementation:

```toml
tokio-cancel = ["dep:tokio-util"]
```

---

## Iteration 14: Extension Traits

> **Status:** Implemented.

This iteration adds extension traits that allow starting a retry directly from
a closure or function pointer, as an alternative to the policy-first style.

This iteration amends: **10.1** (re-exports gain `RetryExt`, `AsyncRetryExt`),
**10.5** (prelude gains `RetryExt`, `AsyncRetryExt`).

**14.1** `RetryExt` is defined in `src/policy/ext/sync_builder.rs` and provides a `.retry()`
method for any `FnMut() -> Result<T, E>`:

```rust
pub trait RetryExt<T, E>: FnMut() -> Result<T, E> + Sized {
    fn retry(self) -> SyncRetryBuilder<Self, NeedsStop, WaitFixed, AnyError>;
}
```

The returned `SyncRetryBuilder` is an owned builder (distinct from `SyncRetry`,
which borrows a `RetryPolicy`) that embeds both the operation and the
stop/wait/predicate configuration. It provides the same `.stop()`, `.wait()`,
`.when()`, `.cancel_on()`, hook, and terminal methods as the policy-based
path. Without `.cancel_on(...)`, it uses `NeverCancel`. `.stop()` must be
called before `.call()` (same `NeedsStop` gate as `RetryPolicy::new()`).

**14.2** `AsyncRetryExt` provides `.retry_async()` for any
`FnMut() -> Fut where Fut: Future<Output = Result<T, E>>`:

```rust
pub trait AsyncRetryExt<T, E, Fut>: FnMut() -> Fut + Sized
where
    Fut: Future<Output = Result<T, E>>,
{
    fn retry_async(self) -> AsyncRetryBuilder<Self, NeedsStop, WaitFixed, AnyError>;
}
```

`AsyncRetryBuilder` also supports `.cancel_on(c: impl Canceler)` with the same
cancellation semantics as iteration 13.

**14.3** Both traits are blanket-implemented. No manual implementation is
required.

**14.4** Usage:

```rust
use tenacious::prelude::*;

// Closure form
let v = (|| fetch())
    .retry()
    .stop(stop::attempts(5))
    .wait(wait::fixed(Duration::from_millis(200)))
    .sleep(std::thread::sleep)
    .call()?;

// Function pointer form
fn do_work() -> Result<u32, String> { Ok(42) }

let v = do_work
    .retry()
    .stop(stop::attempts(3))
    .call()?;
```

**14.5** `RetryExt` and `AsyncRetryExt` are re-exported from `prelude`.

---

## Iteration 15: Ergonomic Additions

> **Status:** Implemented.

This iteration amends: **10.2** (sleep module gains helper constructors),
**10.5** (prelude gains named combinator extension traits).

**15.1** Named combinators are added as methods alongside the existing
operators, providing an alternative spelling for contexts where operators are
hard to read:

- `Stop`: `.or(other)` equivalent to `|`, `.and(other)` equivalent to `&`
- `Wait`: `.add(other)` equivalent to `+`
- `Predicate`: `.or(other)` equivalent to `|`, `.and(other)` equivalent to `&`

These are provided via extension traits (`StopExt`, `WaitExt`, `PredicateExt`)
or directly on the traits. They return the same composition types (`StopAny`,
`StopAll`, `WaitCombine`, `PredicateAny`, `PredicateAll`).

**15.2** The `sleep` module provides feature-gated helper constructors that
return closures compatible with `.sleep(...)`:

- `sleep::tokio()` (requires `tokio-sleep`) — equivalent to the current
  runtime sleep adapter for Tokio
- `sleep::embassy()` (requires `embassy-sleep`)
- `sleep::gloo()` (requires `gloo-timers-sleep`)
- `sleep::futures_timer()` (requires `futures-timer-sleep`)

**15.3** The `on` module provides `on::wait_for_ok`, a predicate helper for
polling-style retries where success depends on an `Ok` value becoming "ready."
It retries on any `Err`, retries on `Ok(value)` when `is_ready(value)` is
`false`, and accepts `Ok(value)` when `is_ready(value)` is `true`.

---

## Iteration 16: `tenacious-config` Companion Crate

> **Status:** Planned. Future direction.

A companion crate `tenacious-config` extracts serde support for built-in
strategies into a separate crate, cleanly separating serializable
configuration from the closure-based core API.

**16.1** `tenacious-config` provides configuration enums that map to built-in
strategies:

```rust
pub enum StopConfig {
    Attempts(u32),
    Elapsed(Duration),
    BeforeElapsed(Duration),
    Any(Vec<StopConfig>),
    All(Vec<StopConfig>),
}

pub enum WaitConfig {
    Fixed(Duration),
    Linear { initial: Duration, increment: Duration },
    Exponential { initial: Duration, base: Option<f64> },
    Cap { max: Duration, inner: Box<WaitConfig> },
    Jitter { max: Duration },
    Chain { first: Box<WaitConfig>, second: Box<WaitConfig>, after: u32 },
    Sum(Vec<WaitConfig>),
}

pub struct RetryPolicyConfig {
    pub stop: StopConfig,
    pub wait: WaitConfig,
}
```

**16.2** `RetryPolicyConfig` provides `.build() -> BoxedRetryPolicy` (or
`TryFrom`) that produces a `tenacious::BoxedRetryPolicy` from the
configuration. This requires the `alloc` feature in the core crate.

**16.3** `tenacious-config` feature-gates serde derives behind its own `serde`
feature flag, keeping the core `tenacious` crate free of serde dependencies
when the companion crate is not used.

**16.4** The core crate's existing `serde` feature flag remains available for
direct serialization of strategy types. `tenacious-config` is an optional
higher-level alternative for configuration-file-driven policies.
