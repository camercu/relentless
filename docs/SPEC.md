# relentless specification

This document is the normative behavior and public-API contract for
`relentless`. It defines what the crate guarantees at runtime and which items
are part of the supported surface. It does not prescribe internal file layout,
development workflow, or historical migration steps.

## 1. Overview

`relentless` is a Rust library for retrying fallible operations and polling for
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

## 2. Support matrix

The crate is `#![no_std]` unconditionally. Feature flags add capabilities on
top of that base.

|Capability                              |`core` only|`alloc`|`std`|
|----------------------------------------|-----------|-------|-----|
|`Stop`, `Wait`, `Predicate`, state types|yes        |yes    |yes  |
|Sync retry with explicit `.clock(...)`  |yes        |yes    |yes  |
|Sync retry with the default `SystemClock`|no        |no     |yes  |
|Async retry with explicit `.clock(...)` |yes        |yes    |yes  |
|Free functions and extension traits     |yes        |yes    |yes  |
|Hooks (multiple per hook point)         |yes        |yes    |yes  |
|`VirtualClock` (waits recorder gated)   |yes        |yes    |yes  |
|`std::error::Error` on `RetryError`     |no         |no     |yes  |

Runtime clock adapters are feature-gated separately (see 12.1):

- `tokio-clock`: `clock::TokioClock`
- `embassy-clock`: `clock::EmbassyClock`
- `gloo-timers-clock`: `clock::GlooClock` on `wasm32`
- `futures-timer-clock`: `clock::FuturesTimerClock`

`alloc` is not required for async retry itself. Within the clock module it
gates only `VirtualClock::waits()`, the recorded-waits accessor (see 12.4).

## 3. Core abstractions

The public model centers on four traits plus a reusable policy type. All four
core traits (`Stop`, `Wait`, `Predicate`, and the clock family) use `&self`
receivers.
Strategies that need internal mutation use interior mutability (`Cell`,
`AtomicUsize`).

> The uniform `&self` model means strategies are trivially shareable,
> cloneable, and object-safe without requiring wrapper types or `Arc`.

### 3.1 Stop

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

**3.1.1** `should_stop` uses `&self` and `&RetryState`; not generic over T or E.

**3.1.2** The `|` operator is equivalent to `.or()` and `&` is equivalent to `.and()`. Both sides are always evaluated (no short-circuit) so that stateful strategies receive every `should_stop` call.

Built-in strategies:

- `stop::attempts(n: u32) -> StopAfterAttempts`
- `stop::elapsed(dur: Duration) -> StopAfterElapsed`
- `stop::never() -> StopNever`

Stop semantics:

- `stop::attempts(n)` treats `n` as the maximum number of completed attempts
- **3.1.3** `attempts` fires when `state.attempt >= n`; `n = 1` means "run at most one attempt"
- **3.1.4** `attempts(0)` panics unconditionally
- **3.1.5** `stop::elapsed(dur)` fires when `state.elapsed >= Some(dur)` and never fires when `state.elapsed` is `None`
- **3.1.6** `stop::never()` always returns false

### 3.2 Wait

`Wait` returns the delay that should be applied before the next attempt.
Composition and builder methods are provided directly on the trait with
`where Self: Sized` bounds.

```rust
pub trait Wait {
    fn next_wait(&self, state: &RetryState) -> Duration;

    fn cap(self, max: Duration) -> wait::WaitCapped<Self>
    where Self: Sized { ... }

    fn jitter(self, max_jitter: Duration) -> wait::Jittered<Self>
    where Self: Sized { ... }

    fn full_jitter(self) -> wait::Jittered<Self>
    where Self: Sized { ... }

    fn equal_jitter(self) -> wait::Jittered<Self>
    where Self: Sized { ... }

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
- `wait::decorrelated_jitter(base: Duration) -> Jittered<WaitFixed>`

Built-in wait semantics:

- **3.2.2** `fixed(dur)` always returns `dur`
- **3.2.3** `linear(initial, increment)` returns
  `initial + (attempt - 1) * increment` with saturating arithmetic
- **3.2.4** `exponential(initial)` returns `initial * 2^(attempt - 1)` with saturating
  arithmetic; `.base(f: f64)` changes the exponential multiplier from the
  default `2.0` — values below `1.0` (including non-finite values) are clamped
  to `1.0`; a base of `1.0` produces a constant delay equal to `initial` on
  every attempt
- **3.2.5** `.chain(other, after)` uses the first strategy when `attempt <= after`, then
  uses `other` when `attempt > after`; when `after` is `0`, the first strategy
  is never consulted
- **3.2.6** the second strategy in `.chain(...)` receives the original global
  `RetryState` unchanged; attempt counting is not rebased at the switch point
- **3.2.7** `.add(other)` combines two strategies by summing their outputs. Equivalent to
  the `+` operator. `(a + b).next_wait(state)` returns
  `a.next_wait(state) + b.next_wait(state)` with saturating arithmetic.

**3.2.1** Wait strategies only compute `Duration`. They do not sleep directly.

**3.2.8** **Zero-duration sleep rule.** When a wait strategy returns `Duration::ZERO`
(or any other mechanism reduces the delay to zero), sleep is skipped entirely
— no sleep call is made and no async yield occurs. This makes
`wait::fixed(Duration::ZERO)` a valid "no delay" strategy for tight polling
loops. All other loop behavior (hooks, stop checks) proceeds normally. This
rule is referenced by the Timeout section and the loop pseudocode (step 10).

### 3.3 Jitter strategies

Three jitter strategies are decorator methods on the `Wait` trait that
transform the inner strategy's output. One is a standalone constructor that
computes delays independently.

**Additive jitter** (`.jitter(max_jitter)`): adds a uniformly distributed
duration in `[0, max_jitter]` to the inner strategy's output.

```
output = base + random(0, max_jitter)
```

**3.3.1** Additive jitter output = `base + random(0, max_jitter)`.

**Full jitter** (`.full_jitter()`): replaces the inner strategy's output with
a random value between zero and the computed base. This is the "Full Jitter"
strategy from the [AWS Architecture Blog][aws-jitter]. It produces the lowest total client
work under contention.

```
output = random(0, base)
```

**3.3.2** Full jitter output = `random(0, base)`.

**Equal jitter** (`.equal_jitter()`): keeps half the computed base and jitters
the other half. This is the "Equal Jitter" strategy from the [AWS Architecture
Blog][aws-jitter]. It guarantees a minimum delay of `base / 2` while still spreading
requests.

```
output = base / 2 + random(0, base / 2)
```

**3.3.3** Equal jitter output = `base / 2 + random(0, base / 2)`.

**3.3.4** Cloning any `Jittered<W>` strategy (additive, full, or equal jitter) produces
a decorrelated copy — the clone uses a fresh PRNG stream and diverges
immediately, generating a different jitter sequence from the original.
Caveat: instance/clone decorrelation relies on an atomic nonce counter and is
unavailable on targets without pointer-width atomic read-modify-write ops
(e.g. `thumbv6m`); there, default-configured instances and clones share one
stream. Use `.with_nonce(n)` to decorrelate manually on such targets.

**Decorrelated jitter** (`wait::decorrelated_jitter(base)`): a `Jittered`
strategy (over `wait::fixed(base)`) where each delay is random between `base`
and three times the previous delay. This is the "Decorrelated Jitter" strategy
from the [AWS Architecture Blog][aws-jitter]. The feedback (the previous delay)
is read from `RetryState::previous_delay`, so the strategy carries no
per-attempt state of its own (only its PRNG) and is freely shareable across
reused policies.

```
output = random(base, previous_delay * 3)
```

**3.3.5** Decorrelated jitter output = `random(base, previous_delay * 3)`; on
the first attempt `previous_delay` is `None`, so the output is
`random(base, base * 3)`. Decorrelated jitter composes with `.cap(max)` to bound
the maximum delay.

Because the feedback lives in `RetryState`, the strategy is stateless apart from
its PRNG. **3.3.6** Cloning assigns a fresh PRNG stream so two copies diverge
immediately.

**3.3.7** All jitter strategy types support `.with_seed(u64)` and `.with_nonce(u64)`
for reproducible sequences. `.with_seed(s)` alone fully pins the sequence: it
sets the seed and re-derives the instance nonce from it, replacing any prior
nonce. `.with_nonce(n)` decorrelates same-seed instances; call it after
`.with_seed`, which resets the nonce. Cloning still assigns a fresh nonce
(3.3.4), so a clone diverges from its seeded original.

**3.3.8** In the natural authoring order — a jitter decorator followed by
`.cap(...)` — jitter is applied to the base and the result is then capped:

```rust
// Jitter applied to base, then capped:
wait::exponential(Duration::from_millis(100))
    .full_jitter()
    .cap(Duration::from_secs(30))
```

The reversed order — `.cap(...)` followed by a jitter decorator — is treated
per decorator, because only additive jitter can breach the cap:

- **Additive** `.jitter(max_jitter)` is normalized so the cap stays the final
  operation: `.cap(max).jitter(j)` behaves as `.jitter(j).cap(max)`. Applying
  additive jitter *after* the cap would add `random(0, max_jitter)` on top of a
  value already at `max` and exceed the cap; normalization prevents that.
- **Full** `.full_jitter()` and **equal** `.equal_jitter()` never exceed the
  base (their outputs are `random(0, base)` and `base/2 + random(0, base/2)`),
  so applying them after a cap can never breach it. They apply in the written
  order: `.cap(max).full_jitter()` jitters the already-capped value.

### 3.4 Predicate

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

**3.4.1** The `|` operator is equivalent to `.or()` and `&` is equivalent to `.and()`. Composition short-circuits: `|` skips the right predicate once the left one retries, and `&` skips the right predicate once the left one declines. This differs from `Stop` composition (§3.1.2), which always evaluates both sides.

`Predicate` uses `&self`. Most predicates are stateless closures; the rare
stateful predicate can use interior mutability (`Cell`, `AtomicUsize`) but must
not rely on being consulted for every outcome under composition.

> The `&self` receiver makes `Predicate<T, E>` trivially object-safe for
> fixed `T, E` and allows sharing across concurrent retry loops.

Built-in predicate constructors:

- **3.4.2** `predicate::any_error() -> PredicateAnyError`: retries all `Err` values
  (`should_retry` returns `true` for any `Err`, `false` for any `Ok`)
- **3.4.3** `predicate::error(f: impl Fn(&E) -> bool) -> PredicateError<F>`: retries
  when `f` returns `true` for the error (`should_retry` returns `true` for
  `Err(e)` when `f(&e)` is `true`, `false` for all `Ok` values, `false` for
  `Err(e)` when `f(&e)` is `false`)
- **3.4.4** `predicate::ok(f: impl Fn(&T) -> bool) -> PredicateOk<F>`: retries when `f`
  returns `true` for the ok value (`should_retry` returns `true` for `Ok(v)`
  when `f(&v)` is `true`, `false` for all `Err` values)
- **3.4.5** `predicate::result(f: impl Fn(&Result<T, E>) -> bool) -> PredicateResult<F>`:
  retries when `f` returns `true`

**`when` vs `until`.** The predicate is set on the policy or execution
builder via either `.when(p)` or `.until(p)`. Both set the same predicate
slot; the last call wins.

- **3.4.6** `.when(p)` retries while `p.should_retry()` returns `true`.
- **3.4.7** `.until(p)` retries until `p.should_retry()` returns `true` — i.e., it
  wraps `p` in `PredicateUntil`, which negates `should_retry()` results.

`.when()` is natural for error-based retry:
`.when(error(|e| e.is_transient()))` reads "retry when transient error."
`.until()` is natural for polling: `.until(ok(|s| s.is_ready()))` reads "retry
until ready."

**3.4.8** When using `.until(ok(f))`, errors are retried by default because `ok(f)`
returns `false` for all `Err` values, and `until` inverts that to `true`
(retry). This is the expected behavior for a retry-first API — the user
reached for `retry()`, so errors are retriable by default. Users who want
specific errors to terminate during polling compose explicitly:
`.until(ok(|s| s.is_ready()).or(error(|e| e.is_fatal())))`.

**3.4.9** `Predicate` is blanket-implemented for `Fn(&Result<T, E>) -> bool`. Named
predicate types (`PredicateAnyError`, `PredicateError<F>`, `PredicateOk<F>`,
`PredicateResult<F>`) have dedicated `impl Predicate<T, E>` blocks.

> Named types do not rely on the blanket `Fn` impl. This ensures they work
> consistently regardless of closure trait inference.

### 3.5 Clock

One injected clock value owns both time seams: the read seam (`now()`) and the
inter-attempt wait. Capability is split into sibling traits over a read-only
base (ADR-0005):

```rust
pub trait Clock {
    fn now(&self) -> Duration;
}

pub trait SyncClock: Clock {
    fn wait(&self, dur: Duration);
}

pub trait AsyncClock: Clock {
    type Wait: Future<Output = ()>;
    fn wait_async(&self, dur: Duration) -> Self::Wait;
}
```

**3.5.1** The sync engine bounds its clock by `SyncClock`; the async engine by
`AsyncClock`. A clock that implements only the other capability is rejected at
compile time — a sync-only clock can never silently no-op an async wait.

**3.5.2** `SyncClock` and `AsyncClock` are siblings, not a supertrait chain: an
async-only runtime clock is not forced to carry a blocking wait, and vice
versa. A dual-capability clock (e.g. `VirtualClock`) implements both.

**3.5.3** The traits are not sealed; third parties implement them for their
own runtimes. Implementors must uphold the coherence contract: `now()` is
monotonically non-decreasing, and a completed wait is reflected in subsequent
`now()` readings. The type system cannot force the advance for arbitrary
implementations; it is a per-impl contract (structural for the shipped
`VirtualClock`, whose reads and waits share one cell; guaranteed by the OS for
real clocks).

**3.5.4** `Clock` and `SyncClock` are blanket-implemented for `&C` where `C`
implements them, so a test can inject `&VirtualClock` and keep the handle for
assertions. `AsyncClock` has no blanket reference impl; instead
`&VirtualClock` implements it directly (its wait future borrows the clock).

**3.5.5** `AsyncClock::Wait` is an owned, named future. The wait must take
effect when the future is polled, not when it is created: the engine may build
a wait future and drop it unpolled (cancellation), and an unpolled wait must
not advance time.

### 3.6 State types

The crate exposes three read-only state types. **3.6.1** All are `#[non_exhaustive]` and
provide `for_attempt(...)` constructors (required arguments only, 1-indexed
attempt first) plus `with_*` setters for the optional fields, for tests and
custom strategy implementations. Constructors `debug_assert!` that
`attempt >= 1`.

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
    /// The delay applied before this attempt (the previous inter-attempt sleep,
    /// after cap/timeout clamping), or `None` on the first attempt. Wait
    /// strategies use this for feedback backoff (e.g. decorrelated jitter).
    pub previous_delay: Option<Duration>,
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
    // `elapsed` and `previous_delay` default to `None`; set them via the
    // `with_*` setters.
    pub fn for_attempt(attempt: u32) -> Self;
    pub fn with_elapsed(self, elapsed: Option<Duration>) -> Self;
    pub fn with_previous_delay(self, previous_delay: Option<Duration>) -> Self;
}

impl<'a, T, E> AttemptState<'a, T, E> {
    // `elapsed` and `next_delay` default to `None`.
    pub fn for_attempt(attempt: u32, outcome: &'a Result<T, E>) -> Self;
    pub fn with_elapsed(self, elapsed: Option<Duration>) -> Self;
    pub fn with_next_delay(self, next_delay: Option<Duration>) -> Self;
}

impl<'a, T, E> ExitState<'a, T, E> {
    // `elapsed` defaults to `None`.
    pub fn for_attempt(
        attempt: u32,
        outcome: &'a Result<T, E>,
        stop_reason: StopReason,
    ) -> Self;
    pub fn with_elapsed(self, elapsed: Option<Duration>) -> Self;
}
```

`RetryState` usage across contexts:

|Context              |`attempt`             |`elapsed`|`previous_delay`                     |
|---------------------|----------------------|---------|-------------------------------------|
|User operation       |about-to-start attempt|available|delay before this attempt (`None` on first)|
|`before_attempt` hook|about-to-start attempt|available|delay before this attempt (`None` on first)|
|`Wait::next_wait`    |just-completed attempt|available|delay before the just-completed attempt|
|`Stop::should_stop`  |just-completed attempt|available|delay before the just-completed attempt|

**3.6.2** The operation receives `RetryState` by value (it is `Copy`). **3.6.3** The numeric value
of `attempt` is the same across all four contexts within a single loop
iteration — the semantic distinction is whether the attempt has run yet.

> Passing by value avoids lifetime entanglement between the state and async
> futures produced by the operation.

Field meanings:

- `attempt` is 1-indexed for completed or about-to-start attempts
- **3.6.4** `elapsed` is always `Some` in states produced by the engines (the
  clock is mandatory); it is `Option` because hand-constructed states (custom
  strategy tests) may omit it, and `None` then means "not provided"
- **3.6.5** `AttemptState.next_delay` is `None` on the final attempt; `Some(delay)` means
  another attempt will follow after the delay
- **3.6.6** `ExitState.attempt` is always the number of completed attempts
- **3.6.7** `ExitState.outcome` is always the outcome of the final attempt

## 4. Error types

### 4.1 RetryError

The predicate's job is binary: retry or don't. When it says don't retry, the
retry loop terminates regardless of whether the outcome was `Ok` or `Err`. This
means a predicate-accepted `Ok` terminates with stop reason `Succeeded` and a
predicate-accepted `Err` with `Rejected`. **4.1.1** Predicate-accepted `Ok` values are returned directly
as `Ok(T)`. **4.1.2** Predicate-accepted `Err` values are wrapped in
`RetryError::Rejected`.

```rust
#[non_exhaustive]
pub enum RetryError<T, E> {
    /// Retries exhausted — the stop strategy fired while the predicate
    /// still wanted to retry. The last outcome is preserved.
    Exhausted { last: Result<T, E> },
    /// The predicate accepted an `Err` outcome as terminal (did not
    /// request retry).
    Rejected { last: E },
}
```

`RetryError` is `#[non_exhaustive]`; downstream exhaustive matches must include a
wildcard arm. `RetryResult<T, E>` is `Result<T, RetryError<T, E>>`.

Accessor methods:

- **4.1.4** `last() -> Option<&Result<T, E>>`: the final `Result<T, E>` if the variant
  carries one; returns `Some` for `Exhausted`, `None` for `Rejected` (which
  stores only `E`)
- **4.1.5** `into_last() -> Option<Result<T, E>>`: consuming version with the same
  `None` cases as `last()`
- **4.1.6** `last_error() -> Option<&E>`: the final `E` if the variant carries one;
  returns `Some` for `Rejected` and for `Exhausted` when the last outcome is
  `Err`; `None` otherwise
- **4.1.7** `into_last_error() -> Option<E>`: consuming version
- **4.1.8** `stop_reason() -> StopReason`: the termination reason as a typed enum

**4.1.3** Stop fires while predicate wants retry → `Err(RetryError::Exhausted { last: Result<T, E> })`.

**4.1.9** Display: `RetryError` implements `Display` when `E: Display`. Display output
is lowercase, without trailing punctuation, following the pattern
`{variant}: {error}`. Examples: `retries exhausted: connection refused`,
`rejected: invalid argument`.

**4.1.10** `RetryError` implements `std::error::Error` when `std` is active and
`E: std::error::Error + 'static`, `T: fmt::Debug + 'static`.

### 4.2 StopReason

```rust
#[non_exhaustive]
pub enum StopReason {
    /// The predicate accepted an `Ok` outcome — the loop succeeded.
    Succeeded,
    /// The predicate accepted an `Err` outcome as terminal (returned as
    /// `RetryError::Rejected`).
    Rejected,
    /// The stop strategy fired while the predicate still wanted to retry.
    Exhausted,
}
```

**4.2.1** `Succeeded` for a predicate-accepted `Ok`; `Rejected` for a
predicate-accepted `Err`.

**4.2.2** `Exhausted` when the stop strategy fired.

**4.2.3** `StopReason` implements `Display` with lowercase labels: `"succeeded"`,
`"rejected"`, `"retries exhausted"`.

**4.2.4** `StopReason` is `#[non_exhaustive]`; downstream matches must include a
wildcard arm.

Mapping from `RetryError` to `StopReason`:

|Terminal condition                    |`RetryError` variant  |`StopReason`|
|--------------------------------------|----------------------|------------|
|Predicate accepts `Ok(T)`             |(not an error)        |`Succeeded` |
|Predicate accepts `Err(E)` as terminal|`Rejected { last: E }`|`Rejected`  |
|Stop fires while retrying             |`Exhausted`           |`Exhausted` |

### 4.3 RetryStats

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

**4.3.1** `attempts` is always ≥ 1.

**4.3.2** `total_elapsed` is `None` when no elapsed clock is available.

**4.3.3** `total_wait` is the sum of delays that reached the sleep phase (step 9 onward); excludes delays preempted by stop at steps 7–8; includes zero-duration delays.

**4.3.4** `stop_reason` matches the termination reason.

## 5. Policy model

`RetryPolicy<S, W, P>` stores owned stop, wait, and predicate values.
**5.8** `RetryPolicy` carries no trait bounds on the struct definition. Bounds appear
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

**5.1** `new()` creates a ready-to-run policy with bounded retries: `attempts(3)`,
`exponential(100ms)`, and `any_error()`. Because `PredicateAnyError` implements
`Predicate<T, E>` for all `T, E`, neither `T` nor `E` is fixed by `new()` —
they are inferred at the call site when the operation is provided.

**5.2** `Default::default()` delegates to `new()`.

Builder methods:

- `.when(p: impl Predicate<T, E>) -> RetryPolicy<S, W, P2>`
- `.until(p: impl Predicate<T, E>) -> RetryPolicy<S, W, P2>`
- `.wait(w: impl Wait) -> RetryPolicy<S, W2, P>`
- `.stop(s: impl Stop) -> RetryPolicy<S2, W, P>`
- `.boxed() -> RetryPolicy<Box<dyn Stop + Send + 'static>, Box<dyn Wait + Send + 'static>, P>`
  with `alloc` — erases stop and wait only; the predicate `P` is left intact so
  a default-predicate policy stays reusable across operations with different
  `(T, E)`
- `.boxed_local() -> RetryPolicy<Box<dyn Stop + 'static>, Box<dyn Wait + 'static>, P>`
  with `alloc` — same as `.boxed()` but without `Send` bounds; for policies
  that remain on a single thread

**5.3** `.stop()`, `.wait()`, `.when()`, `.until()` each consume and return a new `RetryPolicy` with changed type parameter.

**5.4** `.when(p)` and `.until(p)` both set the predicate. `.when(p)` stores `p`
directly. `.until(p)` wraps `p` in `PredicateUntil<P>`, which negates
`should_retry()` results. The last call wins; they do not compose with each
other.

> This follows the same type-level composition pattern as `StopAny`,
> `WaitCapped`, and other combinator types. `PredicateUntil` is listed in the
> combinator type opacity section — users should not name it directly.

**5.7** `RetryPolicy` is `Clone` when its components are `Clone`. Because all trait
methods use `&self`, policies are freely shareable. `RetryPolicy<S, W, P>` is a
pure composition of three strategy types with no other internal state. The only
built-in strategy with interior state is `Jittered` (its PRNG), which uses an
atomic and stays `Sync` on targets with 64-bit atomics (see §10).

**5.5** `.boxed()` requires `S: Stop + Send + 'static`, `W: Wait + Send + 'static`;
erases stop and wait to `Box<dyn...+Send+'static>`. The predicate is **not**
erased: it is left as the generic parameter `P`. Boxing the predicate to
`Box<dyn Predicate<T, E>>` would pin the policy to one `(T, E)`; leaving it
generic lets the default predicate (`PredicateAnyError`, which implements
`Predicate<T, E>` for all `T, E`) be reused across operations with different
success and error types.

**5.6** `.boxed_local()` requires no `Send` bounds; erases stop and wait to
`Box<dyn...+'static>`, leaving the predicate as `P`.

## 6. Execution model

`RetryPolicy::retry(op)` borrows `&self` and returns `SyncRetry`, which
supports hook configuration and terminal execution. The operation receives
`RetryState` by value on each invocation.

`RetryPolicy::retry_async(op)` borrows `&self` and returns
`AsyncRetry`, which supports hook configuration, clock configuration,
and terminal execution. The operation receives `RetryState` by value on each
invocation.

`SyncRetry` and `AsyncRetry` are the policy-borrowing builder types. They
share the same method surface as `SyncRetryBuilder` / `AsyncRetryBuilder`
(hooks, clock, timing, execution) but do not expose strategy overrides
(`.stop()`, `.wait()`, `.when()`, `.until()`) because those are configured on
the policy itself.

Because `Stop`, `Wait`, and `Predicate` all use `&self`, multiple concurrent
retry loops can share the same policy without cloning. Jittered strategies keep
their PRNG state in an atomic (on targets with 64-bit atomics), so this holds
for jittered policies too; concurrent loops interleave draws from one PRNG
stream.

### 6.1 Free function entry points

Free functions provide an alternative entry point that does not require
constructing a `RetryPolicy` first. **6.1.1** They use the same defaults as
`RetryPolicy::new()`: `attempts(3)`, `exponential(100ms)`, `any_error()`.

```rust
/// Sync retry with default policy.
pub fn retry<F, T, E>(op: F) -> SyncRetryBuilder<...>
where F: FnMut(RetryState) -> Result<T, E>;

/// Async retry with default policy.
pub fn retry_async<F, T, E, Fut>(op: F) -> AsyncRetryBuilder<...>
where F: FnMut(RetryState) -> Fut, Fut: Future<Output = Result<T, E>>;
```

**6.1.2** Free functions accept `FnMut(RetryState) -> ...`, giving the operation access
to attempt number and elapsed time.

### 6.2 Extension traits

**6.2.1** `RetryExt` and `AsyncRetryExt` provide method-call syntax for closures and
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
pointers. **6.2.2** The operation does not receive `RetryState`; the closure is wrapped
internally as `move |_| op()`.

> Extension traits are the convenience path for one-shot retry with minimal
> ceremony. Use the free functions `retry()` / `retry_async()` when the
> operation needs access to `RetryState`.

Usage:

```rust
use relentless::{retry, retry_async, RetryExt, AsyncRetryExt};
use relentless::clock::TokioClock;
use relentless::{stop, wait};
use relentless::predicate::{any_error, error, ok};

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
    .clock(TokioClock::new())
    .call()
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
.clock(TokioClock::new())
.call()
.await?;
```

### 6.3 Builder method signatures

All execution builders (`SyncRetry`, `SyncRetryBuilder`, `AsyncRetry`,
`AsyncRetryBuilder`) are generic over `S: Stop`, `W: Wait`,
`P: Predicate<T, E>`, and a clock `C` (`SyncClock` for sync execution,
`AsyncClock` for async execution).

#### Strategy overrides

**6.3.1** Strategy overrides are available only on `SyncRetryBuilder` and
`AsyncRetryBuilder` (the ext-trait / free-function builders that own their
policy). `SyncRetry` and `AsyncRetry` borrow the policy and do not expose
these methods.

- `.when(p: impl Predicate<T, E>) -> ...Builder<S, W, P2, ...>`
- `.until(p: impl Predicate<T, E>) -> ...Builder<S, W, P2, ...>`
- `.wait(w: impl Wait) -> ...Builder<S, W2, P, ...>`
- `.stop(s: impl Stop) -> ...Builder<S2, W, P, ...>`

#### Timing

- **6.3.2** `.clock(c)` — injects the clock that supplies both elapsed time
  and the inter-attempt wait; bound `impl SyncClock` on sync builders and
  `impl AsyncClock` on async builders. Available only while the builder still
  carries the default clock type, so the clock cannot be set twice. Always
  available, including `no_std` without `alloc`.
- **6.3.3** (retired — the boxed-closure elapsed clock was superseded by
  `.clock(...)`; number retained as a tombstone so later numbering is stable)
- `.timeout(dur: Duration)` — sets a wall-clock deadline for the entire retry
  execution, including all attempts and all waits (see Timeout)

See Elapsed time and Timeout for detailed semantics.

#### Hooks

- `.before_attempt(f: impl FnMut(&RetryState))`
- `.after_attempt(f: impl FnMut(&AttemptState<'_, T, E>))`
- `.on_exit(f: impl FnMut(&ExitState<'_, T, E>))`

Hook methods do not change the strategy type parameters. See Hooks for timing,
ordering, and panic behavior.

### 6.4 Sync execution

**6.4.1** Calling `.clock(...)` on sync builders is:

- optional with `std` (defaults to `clock::SystemClock`: a process-global
  monotonic `Instant` anchor for `now()`, `std::thread::sleep` for the wait)
- required without `std`; omitting it is a compile error (`SystemClock`
  implements no clock capability there, so `.call()` is not available)

Terminal execution:

- **6.4.2** `.call() -> RetryResult<T, E>`: executes the retry loop and returns the
  result
- **6.4.3** `.with_stats()` changes the builder so that `.call()` returns
  `(RetryResult<T, E>, RetryStats)` instead

**6.4.4** `SyncRetryWithStats` and `AsyncRetryWithStats` expose only terminal
execution (`.call()` for both; the async `.call()` returns a future). They do
not expose hook or clock configuration methods. Configure everything before
calling `.with_stats()`.

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
    10. If delay > zero, wait for the computed delay via the clock.
    11. attempt += 1, continue to step 1.
```

### 6.5 Async execution

**6.5.1** Async execution always requires `.clock(...)` before `.call()` is
available. The crate never auto-selects an async runtime; there is no default
async clock (`SystemClock`, the initial type-state, implements `AsyncClock`
nowhere, so the bound rejects it at compile time).

Terminal execution:

- **6.5.2** `AsyncRetryBuilder::call` consumes the builder and returns a future
  with `Output = RetryResult<T, E>`. The builder itself does not implement
  `Future`/`IntoFuture`.
- **6.5.3** `.with_stats()` changes the builder so that `.call()` returns a
  future with `Output = (RetryResult<T, E>, RetryStats)` instead

**6.5.4** `AsyncRetryWithStats` exposes only terminal execution.

The async builder is terminated with `.call()`, which returns a single-use
`Future`:

- the builder itself does **not** implement `Future`; you must call `.call()`
  (mirroring the synchronous `.call()`) and `.await` the returned future
- **6.5.5** polling the returned future after completion always panics

The async loop uses the same transition order as sync execution.

### 6.6 Statistics

Retry execution always tracks statistics internally. The cost is one `u32`
counter, one `Option<Duration>` accumulator, one `Duration` accumulator, and
one `StopReason` — no additional allocations or timing beyond what the chosen
clock already provides.

Statistics are accessed via:

- `.with_stats().call()` (sync) or `.with_stats().call().await` (async): returns
  `(RetryResult<T, E>, RetryStats)` alongside the result
- the `on_exit` hook, which receives `ExitState` containing `attempt`,
  `elapsed`, and `stop_reason`

## 7. Termination semantics

Retry termination is defined by the final accepted outcome, the predicate, and
the stop strategy.

### 7.1 Termination table

|Final condition                       |Return value                |`StopReason`|`ExitState.outcome`|
|--------------------------------------|----------------------------|------------|-------------------|
|Predicate accepts `Ok(T)`             |`Ok(T)`                     |`Succeeded` |`&Ok(T)`           |
|Predicate accepts `Err(E)` as terminal|`Err(RetryError::Rejected)` |`Rejected`  |`&Err(E)`          |
|Stop fires while retrying             |`Err(RetryError::Exhausted)`|`Exhausted` |`&last_result`     |

### 7.2 Additional guarantees

- **7.2.1** `after_attempt` fires after every completed attempt, including the final one
- **7.2.2** on the final attempt, `after_attempt` fires with `next_delay = None`
- **7.2.3** `next_delay = Some(delay)` means another attempt will follow after the delay
- **7.2.4** predicate evaluation always happens before stop evaluation
- **7.2.5** stop evaluation always happens after wait computation
- **7.2.6** `Exhausted` carries `last: Result<T, E>` — callers match on the inner
  `Result` to distinguish exhausted-error from unmet-condition cases
- `RetryResult<T, E>` is `Result<T, RetryError<T, E>>`
- **7.2.7** `ExitState.attempt` is always the number of completed attempts (>= 1)

## 8. Hooks

Hooks are configured on execution builders, not on `RetryPolicy`.

Timing guarantees:

- **8.1** `before_attempt` fires before the user operation starts
- **8.2** `after_attempt` fires after every completed attempt, including the final one
- **8.3** `on_exit` fires exactly once for each non-panicking terminal path

Ordering guarantees:

- **8.4** hooks of the same kind fire in registration order
- **8.5** multiple hooks of the same kind may be registered on one execution
  builder and all fire in registration order; this holds in every feature
  configuration (no `alloc` required) and storage is an implementation detail
- **8.6** (folded into 8.5; retained as a tombstone so later numbering is
  stable)

**8.7** Hook panics propagate normally. The crate does not catch them.

Panic behavior:

- **8.8** if the operation or any hook panics, retry execution aborts immediately
- once a panic starts unwinding, remaining hooks for that execution are not run
  (including `on_exit`)
- if a hook panics, the operation's return value for that attempt, if any, is
  dropped during unwinding and is not recoverable by the caller
- a panic inside `on_exit` propagates normally; `on_exit` is not re-invoked or
  wrapped in a catch guard
- if a panic is caught by the caller via `catch_unwind`, `RetryStats` and
  `on_exit` are not recoverable; the execution is in an indeterminate state

## 9. Cancellation guidance

The crate does not provide a built-in cancellation mechanism. Async callers use
standard Rust future cancellation; sync callers use `.timeout()` or in-closure
checks.

### 9.1 Async cancellation

Dropping the retry future at any `.await` point cleanly terminates the retry
loop. Standard patterns work as expected:

```rust
// Timeout
tokio::time::timeout(Duration::from_secs(30),
    (|| fetch_data()).retry_async().clock(TokioClock::new()).call()
).await??;

// Select
tokio::select! {
    result = (|| fetch_data()).retry_async().clock(TokioClock::new()).call() => {
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

### 9.2 Sync cancellation

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

## 10. Thread safety

Async execution types (`AsyncRetryBuilder`) are `Send` when all of the
following are `Send`:

- the operation closure
- the operation's returned `Future`
- `T` and `E`
- the clock and its `AsyncClock::Wait` future
- all registered hooks

**10.1** Async execution types are `Send` when the operation, its future, `T`, `E`, the clock, its `AsyncClock::Wait`, and all hooks are `Send`.

**10.2** Async execution types are `!Send` otherwise. The crate never adds
unconditional `Send` bounds on public trait definitions (`Stop`, `Wait`,
`Predicate`, `Clock`, `SyncClock`, `AsyncClock`). Concrete execution types
derive `Send`/`Sync` from their components via standard auto-trait rules.

Sync execution types are `Send` when their components are `Send`. No `Sync`
bound is required on any execution type because execution is driven by a single
owner (`.call()` for sync, `Future::poll()` for async).

**10.3** The crate includes compile-time tests asserting that `RetryPolicy` with default
type parameters is `Send + Sync`, that `AsyncRetryBuilder` is `Send` when all
components are `Send`, and that `SyncRetryBuilder` is `Send` when all
components are `Send`.

**10.4** `Jittered<W>` is `Send + Sync` when `W` is: its PRNG state is a single
atomic (`AtomicU64`) on targets with 64-bit atomics. On targets without
64-bit atomics it falls back to `Cell`-based state and is `!Sync` there.

## 11. Elapsed time, clocks, and timeout

### 11.1 Elapsed time

Elapsed time is read from the injected clock (see 3.5) — the same value that
performs the inter-attempt waits, so the elapsed readings and the recorded
waits cannot disagree.

**11.1.1** `Clock::now()` returns a monotonic timestamp — a `Duration` since an
arbitrary fixed origin (e.g., system boot, program start, or hardware timer
origin). The library captures a baseline reading when execution starts and
computes elapsed time as `now() - baseline` on each subsequent read. For sync
executions, execution starts when `.call()` is invoked; for async executions,
at the first poll of the future `.call()` returns. Idle time between
configuring a builder and starting execution never consumes the elapsed
budget.

**11.1.2** With `std`, the sync default is `clock::SystemClock`, which anchors
a process-global `std::time::Instant` and reports `now()` relative to it.
Because the anchor is only ever subtracted from a baseline read from the same
clock, its absolute origin is irrelevant. There is no async default clock.

Because a clock is mandatory on every execution, elapsed time is always
available: the engines never produce `elapsed = None`. Statistics do not force
additional timing work beyond what the chosen clock already provides.

### 11.2 Hazard: non-advancing clock

The former hazard class "elapsed-based stop without a clock" is dissolved:
`now()` is mandatory on every clock, so a `timeout` or `stop::elapsed` can no
longer be configured without a time source (ADR-0005).

The residual hazard is a clock whose `now()` does not advance (a constant
reader, or a buggy custom implementation whose wait does not move `now()`).
Against such a clock, elapsed time pins at zero and `stop::elapsed(dur)` /
`.timeout(dur)` never fire; if either is the sole stop condition the retry
loop is unbounded. To stay bounded regardless, combine elapsed-based stops
with `stop::attempts(n)`:

```rust
stop::elapsed(Duration::from_secs(30)).or(stop::attempts(100))
```

The advance contract is per-implementation (see 3.5.3); the shipped clocks
uphold it structurally (`VirtualClock`) or via the OS/runtime scheduler.

### 11.3 Hazard: elapsed-based stop does not account for upcoming sleep

`stop::elapsed(dur)` fires when elapsed time at attempt completion exceeds
`dur`. The retry loop may schedule a sleep that pushes total wall-clock time
well beyond the threshold. For example, `stop::elapsed(Duration::from_secs(30))`
will not prevent a retry sequence from running for 45 seconds total if the
elapsed check passes at 28 seconds and the next sleep is 17 seconds.

Users who need a wall-clock deadline should use `.timeout(dur)` instead of
or in addition to `stop::elapsed(dur)`.

### 11.4 Timeout

`.timeout(dur)` on the execution builder sets a wall-clock deadline for the
entire retry execution. It combines two behaviors:

1. **11.4.1** Implicitly OR's `stop::elapsed(dur)` into the effective stop strategy at
   execution time, so the stop check fires once elapsed time exceeds the
   deadline. The OR is applied to whatever stop strategy is in effect at
   execution time — whether set on the policy, overridden on the builder via
   `.stop()`, or the default. For example,
   `.stop(stop::attempts(5)).timeout(Duration::from_secs(30))` produces an
   effective stop of `stop::attempts(5).or(stop::elapsed(30s))`.
1. **11.4.2** After computing the wait duration (step 5), clamps the delay to
   the remaining budget: `delay = min(delay, max(0, timeout - elapsed))`
   (step 6). The wait itself consumes elapsed budget — the clock that waits is
   the clock that reports elapsed — so a clamped wait ends at the deadline and
   the loop performs one final attempt there (11.4.5).

**11.4.3** When the clamped delay is zero, sleep is skipped entirely per the zero-duration
sleep rule defined in the Wait section.

**11.4.4** The stop reason when timeout causes termination is `Exhausted`, since the
elapsed stop fired. Users can distinguish timeout from attempt exhaustion by
comparing `RetryStats.total_elapsed` against their timeout duration.

Timeout reads elapsed time from the mandatory clock, so it always has a time
source; the only way it can silently misbehave is a non-advancing clock
(see 11.2).

**11.4.5** Timeout does not interrupt a running operation. The total wall-clock time may
exceed the deadline by the execution time of the final attempt. This is
consistent with the crate's guarantee that it never interrupts a user operation
that is already running.

**11.4.6** (retired — the debug assertion for a timeout without a clock was
deleted along with the state it guarded; a clockless timeout is no longer
representable. Number retained as a tombstone so later numbering is stable.)

## 12. Feature-gated APIs

The crate exposes feature-gated helpers in these areas.

### 12.1 Runtime clock adapters

The `clock` module exports one `AsyncClock` implementor per runtime feature,
each pairing a coherent `now()` source with that runtime's timer:

- `clock::TokioClock` with `tokio-clock` — `tokio::time::Instant` +
  `tokio::time::sleep`; coherent under `tokio::time::pause`
- `clock::EmbassyClock` with `embassy-clock` — `embassy_time::Instant` +
  `embassy_time::Timer` (requires a linked embassy time driver)
- `clock::GlooClock` with `gloo-timers-clock` on `wasm32` — `gloo-timers`
  waits paired with a caller-supplied now-source (`GlooClock::with_now`,
  any `Fn() -> Duration` including capturing closures), because wasm has no
  `std::time::Instant`
- `clock::FuturesTimerClock` with `futures-timer-clock` —
  `std::time::Instant` + `futures_timer::Delay`

All are constructed with `new()` (and `Default`) except `GlooClock`, whose
now-source is explicit. Callers may always implement the clock traits for
their own runtime instead.

### 12.2 Jitter

Decorator methods on `Wait`:

- `.jitter(max_jitter)` — additive uniform jitter
- `.full_jitter()` — random in `[0, base]`
- `.equal_jitter()` — `base/2 + random(0, base/2)`

Standalone constructor:

- `wait::decorrelated_jitter(base: Duration) -> Jittered<WaitFixed>` —
  random in `[base, previous_delay * 3]`

Exported types: `Jittered`.

All jitter types support `.with_seed(u64)` and `.with_nonce(u64)` for
reproducible sequences; `.with_seed` alone fully pins the sequence (see 3.3.7).

### 12.3 Boxed policies

With `alloc`:

`.boxed()` on `RetryPolicy` requires `S: Stop + Send + 'static`,
`W: Wait + Send + 'static` and returns
`RetryPolicy<Box<dyn Stop + Send + 'static>, Box<dyn Wait + Send + 'static>, P>`.
Only stop and wait are erased; the predicate `P` is preserved so a
default-predicate policy remains reusable across different `(T, E)`.

`.boxed_local()` on `RetryPolicy` requires `S: Stop + 'static`,
`W: Wait + 'static` (no `Send` bounds) and returns
`RetryPolicy<Box<dyn Stop + 'static>, Box<dyn Wait + 'static>, P>`.
Use this when the policy does not need to cross thread boundaries.

> Object safety is satisfied because `Stop`, `Wait`, and `Predicate<T, E>`
> (for fixed `T, E`) each have a single non-generic method with an `&self`
> receiver. Composition methods are gated behind `where Self: Sized` and are
> not available on trait objects, which is correct.

No public type aliases are provided for boxed policy types. Users who need to
name the type can write it explicitly or use `impl` return types.

### 12.4 Virtual-clock test infrastructure

`clock::VirtualClock` is the deterministic clock for testing retry behavior
without real sleeping. It is always available (no feature gate, `no_std`-clean)
and implements the clock traits directly, so it is injected via `.clock(...)`
like any production clock.

- **12.4.1** `VirtualClock::new()` starts at virtual time zero with no
  recorded waits. `Default` is equivalent to `new()`.
- **12.4.2** Waits advance virtual time by exactly the requested duration
  instead of sleeping. `Clock::now()` reads the very cell the waits advance —
  one cell, one writer — so the read seam and the wait seam cannot desync even
  by an implementation bug. (This dissolves the former documented misuse of
  wiring an elapsed clock and a sleeper from different instances; there is no
  longer a second seam to mismatch.)
- **12.4.3** An owned `VirtualClock` is a `SyncClock`; a shared borrow
  (`&VirtualClock`) is additionally an `AsyncClock` whose wait future advances
  time on its first poll (an unpolled, dropped wait leaves time untouched).
  Tests inject `&clock` and keep the handle for assertions.
- **12.4.4** `.advance(dur)` adds `dur` to virtual time without recording a
  wait (simulates time passing inside an attempt). All time arithmetic
  saturates at `Duration::MAX`.
- **12.4.5** With `alloc`, `.waits()` returns every wait requested so far, in
  request order (a point-in-time snapshot).
- **12.4.6** All methods take `&self` via interior mutability (`Cell`); the
  clock is neither `Sync` nor intended to cross threads. Multi-threaded
  executors need a user-provided lock-based clock.

## 13. Public API surface

The following items are part of the supported surface for new code.

### 13.1 Crate root exports

Types:

- `RetryPolicy`
- `RetryError`, `RetryResult`
- `RetryStats`, `StopReason`
- `RetryState`, `AttemptState`, `ExitState`
- `SyncRetry`, `SyncRetryWithStats`, `AsyncRetry`, `AsyncRetryWithStats`
  (policy-borrowing builders and their stats variants)
- `SyncRetryBuilder`, `SyncRetryBuilderWithStats`,
  `AsyncRetryBuilder`, `AsyncRetryBuilderWithStats`
  (ext-trait / free-function builders)
- `DefaultSyncRetryBuilder`, `DefaultSyncRetryBuilderWithStats`,
  `DefaultAsyncRetryBuilder`, `DefaultAsyncRetryBuilderWithStats`
  (type aliases for the default-policy builder configurations)

Traits:

- `Stop` (includes `.or()`, `.and()` as provided methods)
- `Wait` (includes `.cap()`, `.chain()`, `.add()`, `.jitter()`,
  `.full_jitter()`, `.equal_jitter()` as provided methods)
- `Predicate` (includes `.or()`, `.and()` as provided methods)
- `Clock`, `SyncClock`, `AsyncClock` (re-exported from `clock`)
- `RetryExt` (blanket-implemented for `FnMut() -> Result<T, E>`)
- `AsyncRetryExt` (blanket-implemented for
  `FnMut() -> Fut where Fut: Future<Output = Result<T, E>>`)

Free functions:

- `retry`
- `retry_async`

### 13.2 Module exports

`stop` module:

- constructors: `attempts`, `elapsed`, `never`
- types: `StopAfterAttempts`, `StopAfterElapsed`, `StopNever`, `StopAny`,
  `StopAll`

`wait` module:

- constructors: `fixed`, `linear`, `exponential`
- types: `WaitFixed`, `WaitLinear`, `WaitExponential`, `WaitCapped`,
  `WaitChain`, `WaitCombine`
- jitter: `decorrelated_jitter` constructor;
  `Jittered` type

`predicate` module:

- constructors: `any_error`, `error`, `ok`, `result`
- types: `PredicateAnyError`, `PredicateError`, `PredicateOk`,
  `PredicateResult`, `PredicateAny`, `PredicateAll`, `PredicateUntil`

`clock` module:

- traits: `Clock`, `SyncClock`, `AsyncClock`
- types: `SystemClock`, `VirtualClock`, `VirtualWait`
- feature-gated adapter types: `TokioClock`, `EmbassyClock`, `GlooClock`,
  `FuturesTimerClock` (see 12.1)

### 13.3 Combinator type opacity

Combinator types (`StopAny`, `StopAll`, `WaitCapped`, `WaitChain`,
`WaitCombine`, `PredicateAny`, `PredicateAll`, `PredicateUntil`, `Jittered`,
`Jittered`, and similar) are **exposed but unstable**: they are
`pub` for technical reasons (they appear in return types of composition methods
and `.until()`), but users should not name them in function signatures. Use
`impl Stop`, `impl Wait`, or `impl Predicate<T, E>` instead.

> Combinator type names and their generic parameters may change in minor
> releases.

## 14. Standard trait implementations

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

## 15. Panic inventory

The following conditions cause a panic. No other public constructor or method
panics. Saturating arithmetic is used throughout wait computation — overflow
produces `Duration::MAX`, not a panic.

- **15.1** `stop::attempts(0)` panics unconditionally
- **15.2** Polling the async retry future after it has returned `Poll::Ready` panics

**15.3** No other public constructor or method panics; saturating arithmetic is used throughout.

All panic conditions are documented with `# Panics` sections in rustdoc.

## 16. Compatibility guarantees

The crate guarantees the following project-wide properties.

- **16.1** MSRV is Rust `1.85.0`
- **16.2** the crate forbids `unsafe` with `#![forbid(unsafe_code)]`
- **16.3** `Duration` is always `core::time::Duration`
- **16.4** `RetryError` implements `Display` when `E: Display`
- **16.5** `RetryError::stop_reason()` is always available regardless of type parameter
  bounds
- **16.6** `RetryError` implements `std::error::Error` when `std` is active and
  `E: std::error::Error + 'static`, `T: fmt::Debug + 'static`
- **16.7** all public items have rustdoc examples using `?` for error handling
- **16.8** license is dual MIT OR Apache-2.0

## 17. Testing strategy

Each test file covers the spec section it corresponds to. Requirement numbers
appear in test comments as traceability anchors (e.g., `/// 3.1.4`).

| Test file | Spec sections covered |
|---|---|
| `tests/stop.rs` | §3.1 |
| `tests/wait.rs` | §3.2 |
| `tests/jitter.rs` | §3.3 |
| `tests/predicate.rs` | §3.4 |
| `tests/clock.rs`, `tests/clock_adapters.rs` | §3.5, §12.1, §12.4 |
| `tests/state.rs` | §3.6 |
| `tests/error.rs` | §4.1, §4.2 |
| `tests/stats.rs` | §4.3, §7 |
| `tests/policy_sync.rs` | §5, §6.1–6.4, §11 |
| `tests/policy_async.rs` | §6.5 |
| `tests/hooks.rs` | §8 |
| `tests/ext.rs` | §6.2 |
| `tests/allocation.rs` | §12.3 |
| `tests/trait_impls.rs` | §14 |
| `tests/composition.rs` | §3.1–3.4 (seeded property tests) |
| `tests/async_no_alloc.rs` | §2 (no_std/no_alloc async compilation) |

**Seeded property tests.** `tests/composition.rs` verifies that `Stop`, `Wait`,
and `Predicate` composition obeys boolean and arithmetic algebra across 1,024
random samples per test. The seed is read from `RELENTLESS_PROPTEST_SEED` at
runtime; if absent, a random seed is generated and printed on failure for
reproduction.

**Compile-fail guarantees.** Typestate constraints (no strategy overrides on
policy-borrowing builders, sync `.call()` unavailable without a clock in
`no_std`, async `.call()` unavailable without a clock anywhere) are verified
via `compile_fail` doctests in the source files rather than integration tests.

**no_std coverage.** `tests/async_no_alloc.rs` verifies that async retry
compiles and runs without `std` or `alloc`. The §2 support matrix is the
authoritative reference for which features require which capability tier.

[aws-jitter]: https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/
