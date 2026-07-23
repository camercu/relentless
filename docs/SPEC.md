# relentless specification

This document is the normative behavior and public-API contract for
`relentless`. It defines what the crate guarantees at runtime and which items
are part of the supported surface. It does not prescribe internal file layout,
development workflow, or historical migration steps.

## 1. Overview

`relentless` is a Rust library for retrying fallible operations and polling for
conditions. It models retries with three composable parts:

- a **classifier**: how each outcome is sorted into return / retry / abort
- `Wait`: how long to wait between attempts
- `Stop`: when to stop retrying

The classifier consumes each outcome **by value** and returns a three-way
[`Verdict`](#34-classifier), so the retry decision is independent of `Result`
semantics — a sought-after `Err`, a non-`Result` poll enum, or a search state
can each drive the loop directly. The common `Result` case retries on any `Err`
by default; `.decide(...)` installs a custom classifier, and `.when(p)` /
`.until(p)` are `Result`-shaped predicate sugar over it.

The same model applies to sync and async execution. Policies are reusable, hook
callbacks are configured per execution, and the crate supports `std`,
`no_std`, `wasm32`, and embedded-oriented environments.

Polling for a condition is expressed with `.until(p)` ("retry until `p` holds")
or its inverse `.when(p)` ("retry while `p` holds"); both accept a `Predicate`
and set the classifier slot. See Classifier for details.

## 2. Support matrix

The crate is `#![no_std]` unconditionally. Feature flags add capabilities on
top of that base.

|Capability                              |`core` only|`alloc`|`std`|
|----------------------------------------|-----------|-------|-----|
|`Stop`, `Wait`, classifier, state types |yes        |yes    |yes  |
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

The public model centers on the classifier plus the `Stop`, `Wait`, and clock
traits, and a reusable policy type. The strategy traits (`Stop`, `Wait`,
`Predicate`, and the clock family) use `&self` receivers; the engine-facing
classifier trait (`Decide`) also uses `&self`, so one classifier serves every
attempt. Strategies that need internal mutation use interior mutability
(`Cell`, `AtomicUsize`).

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
- **3.1.5** `stop::elapsed(dur)` fires when `state.elapsed >= dur`
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

### 3.4 Classifier

The classifier sorts each completed outcome into a three-way `Verdict`. It
consumes the outcome **by value**, so the retry decision is independent of
`Result` semantics.

```rust
pub enum Decision<R, O>   { Return(R), Retry(O) }            // no-abort
pub enum Verdict<R, A, O> { Return(R), Retry(O), Abort(A) }  // abort-capable
```

`O` is the whole outcome the operation produces; `R` is delivered to the caller
on success (`Ok(R)`); `A` is the payload projected on abort. `Decision` has no
abort type parameter (the common case); `Verdict` adds an `Abort(A)` arm. The
sealed `IntoDecision<O>` trait unifies them, so one `.decide` method accepts a
closure returning either.

**3.4.1 Verdict outcomes.**

|verdict     |terminal?|result                                 |
|------------|---------|---------------------------------------|
|`Return(r)` |yes      |`Ok(r)`                                |
|`Abort(a)`  |yes      |`Err(RetryError::Aborted { last: a })` |
|`Retry(o)`  |no       |retry, subject to the stop strategy    |

When the classifier returns `Retry` but the stop strategy fires (or the timeout
elapses), the loop terminates with `RetryError::Exhausted { last: o }`.

**Outcome trait (owned types).**

```rust
pub trait Outcome { type Return; type Abort; fn classify(self) -> Verdict<..>; }
```

- **3.4.2** A blanket `impl<T, E> Outcome for Result<T, E>` is the default path:
  `Ok(v)` → `Return(v)`, any `Err` → `Retry` (`Abort = E`, never produced on the
  default path). A type you own may implement `Outcome` to classify itself with
  no `.decide` at the call site. The orphan rule reserves `Result` for the
  blanket impl; use `.decide` or a newtype for custom `Result` classification.

**Installing a classifier.** The classifier slot is set by, in last-wins order:

- **3.4.3** default — `DefaultClassifier` delegates to `O: Outcome`.
- **3.4.4** `.decide(c)` — a closure `Fn(O) -> impl IntoDecision<O>` (returning
  `Decision` or `Verdict`). On the op-first builders the bound is op-anchored,
  so an inline closure's parameter infers with no annotation; policy-first
  `.decide` defers the bound and needs one parameter annotation.
- **3.4.5** `.when(p)` / `.until(p)` — `Result`-shaped `Predicate` sugar (below).

**Predicate (`.when` / `.until`).** `Predicate<T, E>` is retained as `Result`-shaped
sugar; it is a single method, blanket-implemented for any
`Fn(&Result<T, E>) -> bool`, so the built-in factories and plain closures both
work.

```rust
pub trait Predicate<T, E> {
    fn should_retry(&self, outcome: &Result<T, E>) -> bool;
}
```

- **3.4.6** `.when(p)` retries while `p.should_retry()` is `true`; otherwise it
  accepts — `Ok(v)` → `Return(v)`, `Err(e)` → `Abort(e)` (the bare error).
- **3.4.7** `.until(p)` is the inverse: retry until `p.should_retry()` is `true`,
  then accept with the same `Ok`→return / `Err`→abort mapping.
- **3.4.8** Predicates carry no composition algebra. Compose boolean conditions
  inside a `result` closure — `.when(result(|o| a(o) || b(o)))` — or drop to
  `.decide` for full return / retry / abort control.

`.when(error(|e| e.is_transient()))` reads "retry when transient error";
`.until(ok(|s| s.is_ready()))` reads "retry until ready."

**3.4.9** With `.until(ok(f))`, errors are retried by default (`ok(f)` is `false`
for any `Err`, which `until` inverts to retry). Make errors terminal with a
`result` closure — `.until(result(|o| is_ready(o) || is_fatal(o)))` — or with
`.decide`.

Built-in predicate constructors (`should_retry` semantics):

- **3.4.10** `predicate::any_error() -> PredicateAnyError`: `true` for any `Err`,
  `false` for any `Ok`.
- **3.4.11** `predicate::error(f) -> PredicateError<F>`: `true` for `Err(e)` when
  `f(&e)`; `false` for any `Ok` and for `Err(e)` when `!f(&e)`.
- **3.4.12** `predicate::ok(f) -> PredicateOk<F>`: `true` for `Ok(v)` when `f(&v)`;
  `false` for any `Err`.
- **3.4.13** `predicate::result(f) -> PredicateResult<F>`: `true` when `f(&outcome)`.

**3.4.14** `Predicate` is blanket-implemented for `Fn(&Result<T, E>) -> bool`;
the named types have dedicated `impl Predicate<T, E>` blocks. The engine-facing
`Decide<O>` trait (the installed classifier) is sealed — users never implement
it; `Outcome` is the user-implementable trait for owned outcome types.

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

**3.5.4** All three traits are blanket-implemented for `&C` where `C`
implements them, so a caller can inject `&clock` and keep the handle for
assertions. Additionally, `&VirtualClock` implements `AsyncClock` directly
(its wait future borrows the clock; the owned `VirtualClock` implements only
the sync capability, so the direct impl does not overlap the blanket).

**3.5.5** `AsyncClock::Wait` is an owned, named future. The wait must take
effect when the future is polled, not when it is created: the engine may build
a wait future and drop it unpolled (cancellation), and an unpolled wait must
not advance time.

### 3.6 State types

The crate exposes read-only state passed to strategies and hooks.

**3.6.1** `RetryState` (for `Stop`, `Wait`, the operation, and `before_attempt`)
and `AttemptState` (for `after_attempt`) are `#[non_exhaustive]` structs with
public fields for read access. `RetryState::for_attempt(attempt)` (plus `with_*`
setters) constructs one for tests and custom strategies, and `debug_assert!`s
`attempt >= 1`. `AttemptState` and `Exit` (for `on_exit`) are produced only by
the engine — `AttemptState` read by field access, `Exit` by matching — and are
not constructed externally.

> Public fields on `#[non_exhaustive]` structs are a deliberate choice for
> ergonomic read access. `#[non_exhaustive]` prevents construction outside the
> crate while allowing field reads. Field and variant names are part of the
> stable API surface.

```rust
/// Shared state for Stop, Wait, the operation, and the before_attempt hook.
pub struct RetryState {
    /// 1-indexed attempt number. For the operation and `before_attempt`,
    /// this is the attempt about to start. For `Stop` and `Wait`, this
    /// is the just-completed attempt.
    pub attempt: u32,
    /// Wall-clock time since retry execution started, read from the
    /// injected clock. Zero in hand-constructed states until set.
    pub elapsed: Duration,
    /// The delay applied before this attempt (the previous inter-attempt sleep,
    /// after cap/timeout clamping), or `None` on the first attempt. Wait
    /// strategies use this for feedback backoff (e.g. decorrelated jitter).
    pub previous_delay: Option<Duration>,
}
```

```rust
/// State passed to the after_attempt hook, over the whole outcome type `O`.
pub struct AttemptState<'a, O> {
    /// 1-indexed attempt number just completed.
    pub attempt: u32,
    /// Wall-clock time since retry execution started.
    pub elapsed: Duration,
    /// The raw outcome, borrowed BEFORE classification.
    pub outcome: &'a O,
}
```

**3.6.2** `after_attempt` fires once per attempt *before* the classifier
consumes the outcome, so it observes every outcome — including the terminal one
— under a uniform contract. `AttemptState` carries no next-delay field (removed
in the classifier engine); the applied delay reappears as the next attempt's
`RetryState.previous_delay`.

```rust
/// View passed to the on_exit hook: exactly what the caller receives.
pub enum Exit<'a, R, A, O> {
    Returned  { attempt: u32, elapsed: Duration, value: &'a R },
    Aborted   { attempt: u32, elapsed: Duration, last:  &'a A },
    Exhausted { attempt: u32, elapsed: Duration, last:  &'a O },
}

impl<'a, R, A, O> Exit<'a, R, A, O> {
    pub fn attempt(&self) -> u32;
    pub fn elapsed(&self) -> Duration;
    pub fn stop_reason(&self) -> StopReason; // derived from the variant
}
```

**3.6.3** `Exit` is a borrowed view of exactly what the caller receives on this
termination; its variant is the single source of truth for `stop_reason()`
(`Returned`/`Aborted`/`Exhausted`).

Constructor signatures:

```rust
impl RetryState {
    // `elapsed` defaults to zero and `previous_delay` to `None`; set them
    // via the `with_*` setters.
    pub fn for_attempt(attempt: u32) -> Self;
    pub fn with_elapsed(self, elapsed: Duration) -> Self;
    pub fn with_previous_delay(self, previous_delay: Option<Duration>) -> Self;
}
```

`RetryState` usage across contexts:

|Context              |`attempt`             |`elapsed`|`previous_delay`                     |
|---------------------|----------------------|---------|-------------------------------------|
|User operation       |about-to-start attempt|available|delay before this attempt (`None` on first)|
|`before_attempt` hook|about-to-start attempt|available|delay before this attempt (`None` on first)|
|`Wait::next_wait`    |just-completed attempt|available|delay before the just-completed attempt|
|`Stop::should_stop`  |just-completed attempt|available|delay before the just-completed attempt|

**3.6.4** The operation receives `RetryState` by value (it is `Copy`). **3.6.5**
The numeric value of `attempt` is the same across all four contexts within a
single loop iteration — the semantic distinction is whether the attempt has run
yet.

> Passing by value avoids lifetime entanglement between the state and async
> futures produced by the operation.

Field meanings:

- `attempt` is 1-indexed for completed or about-to-start attempts
- **3.6.6** `elapsed` is a plain `Duration`: the clock is mandatory, so the
  engines always supply a reading. Hand-constructed states (custom strategy
  tests) default it to zero until `with_elapsed` is called
- **3.6.7** `Exit::attempt()` is always the number of completed attempts; the
  variant's outcome field (`value`/`last`) is always the final attempt's
  return value, abort payload, or whole outcome respectively

## 4. Error types

### 4.1 RetryError

The classifier decides each outcome's fate (§3.4). A `Return` verdict yields
`Ok(R)` directly; the two failure modes are `RetryError`. **4.1.1** An `Abort`
verdict yields `RetryError::Aborted { last: A }` (on the `.when`/`.until` path,
`A` is the bare error `E`). **4.1.2** A `Retry` verdict that the stop strategy
(or timeout) cuts short yields `RetryError::Exhausted { last: O }` (the whole
final outcome).

```rust
#[non_exhaustive]
pub enum RetryError<A, O> {
    /// The classifier aborted; `last` is the projected abort payload.
    Aborted { last: A },
    /// The stop strategy fired while the classifier still wanted to retry;
    /// `last` is the final whole outcome.
    Exhausted { last: O },
}
```

`RetryError` is `#[non_exhaustive]`; downstream exhaustive matches must include a
wildcard arm. On the default / `.when` / `.until` path the outcome is
`Result<T, E>` and aborts carry the bare error, so the type is
`RetryError<E, Result<T, E>>`, and `RetryResult<T, E>` aliases
`Result<T, RetryError<E, Result<T, E>>>`.

**4.1.3** `stop_reason() -> StopReason` (`Aborted`/`Exhausted`) is available for
all `A, O`. The remaining `Result`-shaped accessors are provided on
`RetryError<E, Result<T, E>>`:

- **4.1.4** `last() -> Option<&Result<T, E>>`: `Some` for `Exhausted`, `None` for
  `Aborted` (which stores only the bare error)
- **4.1.5** `into_last() -> Option<Result<T, E>>`: consuming version
- **4.1.6** `last_error() -> Option<&E>`: `Some` for `Aborted`, and for
  `Exhausted` when the last outcome is `Err`; `None` otherwise
- **4.1.7** `into_last_error() -> Option<E>`: consuming version

**4.1.9** Display: `RetryError<E, Result<T, E>>` implements `Display` when
`E: Display`. Output is lowercase without trailing punctuation, `{variant}:
{error}`: `retries exhausted: connection refused`, `aborted: invalid argument`.

**4.1.10** It implements `std::error::Error` when `std` is active and
`E: std::error::Error + 'static`, `T: fmt::Debug + 'static`.

### 4.2 StopReason

The three stop reasons mirror the three terminal verdicts (§3.4.1).

```rust
#[non_exhaustive]
pub enum StopReason {
    /// The classifier returned an outcome — the loop succeeded with `Ok(R)`.
    Returned,
    /// The classifier aborted — the loop returned `RetryError::Aborted`.
    Aborted,
    /// The stop strategy fired while the classifier still wanted to retry —
    /// the loop returned `RetryError::Exhausted`.
    Exhausted,
}
```

**4.2.1** `Returned` for a `Return` verdict; `Aborted` for an `Abort` verdict.

**4.2.2** `Exhausted` when the stop strategy (or timeout) fired while the
classifier still wanted to retry.

**4.2.3** `StopReason` implements `Display` with lowercase labels: `"returned"`,
`"aborted"`, `"retries exhausted"`.

**4.2.4** `StopReason` is `#[non_exhaustive]`; downstream matches must include a
wildcard arm.

Mapping from the terminal verdict to `RetryError` and `StopReason`:

|Terminal verdict           |`RetryError` variant   |`StopReason`|
|---------------------------|-----------------------|------------|
|`Return(r)`                |(not an error)         |`Returned`  |
|`Abort(a)`                 |`Aborted { last: a }`  |`Aborted`   |
|`Retry` cut short by stop  |`Exhausted { last: o }`|`Exhausted` |

### 4.3 RetryStats

```rust
pub struct RetryStats {
    /// Completed attempts. Always >= 1.
    pub attempts: u32,
    /// Wall-clock time from retry execution start until terminal exit,
    /// read from the injected clock.
    pub total_elapsed: Duration,
    /// Sum of delays for retries that reached the sleep phase (step 9
    /// onward). This is requested wait budget, not measured sleep time.
    /// Includes zero-duration delays (which skip actual sleep). Excludes
    /// delays computed but preempted by stop at steps 6–7.
    pub total_wait: Duration,
    /// Why the retry loop terminated.
    pub stop_reason: StopReason,
}
```

**4.3.1** `attempts` is always ≥ 1.

**4.3.2** `total_elapsed` is always present: the injected clock is mandatory,
so elapsed tracking cannot be absent.

**4.3.3** `total_wait` is the sum of delays that reached the sleep phase (step 9 onward); excludes delays preempted by stop at steps 6–7; includes zero-duration delays.

**4.3.4** `stop_reason` matches the termination reason.

## 5. Policy model

`RetryPolicy<S, W, C>` stores owned stop, wait, and classifier values.
**5.8** `RetryPolicy` carries no trait bounds on the struct definition. Bounds appear
only on `impl` blocks.

> This follows the Rust API guideline C-STRUCT-BOUNDS and ensures that adding
> derived traits is never a breaking change.

Construction:

```rust
impl RetryPolicy<StopAfterAttempts, WaitExponential, DefaultClassifier> {
    pub fn new() -> Self { ... }
}

impl Default for RetryPolicy<StopAfterAttempts, WaitExponential, DefaultClassifier> {
    fn default() -> Self { Self::new() }
}
```

**5.1** `new()` creates a ready-to-run policy with bounded retries: `attempts(3)`,
`exponential(100ms)`, and the default classifier (retry on any `Err`). Because
`DefaultClassifier` classifies every `O: Outcome`, neither `T` nor `E` is fixed
by `new()` — they are inferred at the call site when the operation is provided.

**5.2** `Default::default()` delegates to `new()`.

Builder methods:

- `.when(p: impl Predicate<T, E>) -> RetryPolicy<S, W, When<P>>`
- `.until(p: impl Predicate<T, E>) -> RetryPolicy<S, W, Until<P>>`
- `.decide(c) -> RetryPolicy<S, W, ClosureClassifier<C>>` — policy-first, so the
  closure's parameter needs one annotation (no op to anchor inference on)
- `.wait(w: impl Wait) -> RetryPolicy<S, W2, C>`
- `.stop(s: impl Stop) -> RetryPolicy<S2, W, C>`
- `.boxed() -> RetryPolicy<Box<dyn Stop + Send + 'static>, Box<dyn Wait + Send + 'static>, C>`
  with `alloc` — erases stop and wait only; the classifier `C` is left intact so
  a default-classifier policy stays reusable across operations with different
  `(T, E)`
- `.boxed_local() -> RetryPolicy<Box<dyn Stop + 'static>, Box<dyn Wait + 'static>, C>`
  with `alloc` — same as `.boxed()` but without `Send` bounds; for policies
  that remain on a single thread

**5.3** `.stop()`, `.wait()`, `.when()`, `.until()`, `.decide()` each consume and return a new `RetryPolicy` with a changed type parameter.

**5.4** `.when(p)`, `.until(p)`, and `.decide(c)` all set the classifier slot
(last-wins; they do not compose). `.when(p)` stores `When<P>`; `.until(p)` stores
`Until<P>` (the inverse); `.decide(c)` stores `ClosureClassifier<C>`.

> This follows the same type-level composition pattern as `StopAny`,
> `WaitCapped`, and other combinator types. `When`/`Until`/`ClosureClassifier`
> are opaque classifier wrappers — users install them via the builder methods.

**5.7** `RetryPolicy` is `Clone` when its components are `Clone`. Because all trait
methods use `&self`, policies are freely shareable. `RetryPolicy<S, W, C>` is a
pure composition of three strategy types with no other internal state. The only
built-in strategy with interior state is `Jittered` (its PRNG), which uses an
atomic and stays `Sync` on targets with 64-bit atomics (see §10).

**5.5** `.boxed()` requires `S: Stop + Send + 'static`, `W: Wait + Send + 'static`;
erases stop and wait to `Box<dyn...+Send+'static>`. The classifier is **not**
erased: it is left as the generic parameter `C`. The default classifier
classifies every `O: Outcome`, so a default-classifier boxed policy is reused
across operations with different success and error types; erasing the classifier
would pin it to one `(T, E)`.

**5.6** `.boxed_local()` requires no `Send` bounds; erases stop and wait to
`Box<dyn...+'static>`, leaving the classifier as `C`.

**5.9** `RetryPolicy::retry`/`retry_async` borrow the policy by `&self` and lend
its stop/wait/classifier to the builder as shared references (`Stop`, `Wait`,
and `Decide` are implemented for `&T`), so a boxed policy stays reusable without
being `Clone`.

## 6. Execution model

`RetryPolicy::retry(op)` borrows `&self` and returns a `Retry` builder that
holds the policy's stop, wait, and classifier as shared references. The
operation receives `RetryState` by value on each invocation.
`RetryPolicy::retry_async(op)` is the async twin, returning an `AsyncRetry`
builder whose operation returns a future.

`Retry` and `AsyncRetry` are the single builder types for every entry path: the
policy path starts from borrowed strategies, and the free-function / ext-trait
path owns them. Both expose the full surface — strategy overrides
(`.stop`/`.wait`/`.when`/`.until`/`.decide`), hooks, `.clock`, `.timeout`,
`.with_stats`, and `.call`. A strategy override on a policy-borrowed builder
replaces the borrowed slot for that call only, leaving the policy unchanged.

Because `Stop`, `Wait`, `Decide`, and the clocks all use `&self`, multiple
concurrent retry loops can share the same policy without cloning. Jittered
strategies keep their PRNG state in an atomic (on targets with 64-bit atomics),
so this holds for jittered policies too; concurrent loops interleave draws from
one PRNG stream.

### 6.1 Free function entry points

Free functions provide an alternative entry point that does not require
constructing a `RetryPolicy` first. **6.1.1** They use the same defaults as
`RetryPolicy::new()`: `attempts(3)`, `exponential(100ms)`, and the default
classifier (retry on any `Err`).

```rust
/// Sync retry with default configuration; the operation may return any
/// outcome `O` (the default classifier requires `O: Outcome` at `.call()`).
pub fn retry<F, O>(op: F) -> Retry<...>
where F: FnMut(RetryState) -> O;

/// Async retry with default configuration.
pub fn retry_async<F, Fut, O>(op: F) -> AsyncRetry<...>
where F: FnMut(RetryState) -> Fut, Fut: Future<Output = O>;
```

**6.1.2** Free functions accept `FnMut(RetryState) -> ...`, giving the operation access
to attempt number and elapsed time.

### 6.2 Extension traits

**6.2.1** `RetryExt` and `AsyncRetryExt` provide method-call syntax for no-argument
closures and functions returning any outcome `O`. They use the same defaults as
`RetryPolicy::new()`.

```rust
pub trait RetryExt<O>: FnMut() -> O + Sized {
    fn retry(self) -> Retry<...>;
}

pub trait AsyncRetryExt<Fut, O>: FnMut() -> Fut + Sized
where Fut: Future<Output = O>
{
    fn retry_async(self) -> AsyncRetry<...>;
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

The `Retry` and `AsyncRetry` builders are generic over the operation `F`, the
classifier `C`, `S: Stop`, `W: Wait`, a clock (`SyncClock` for sync,
`AsyncClock` for async), and the three hook slots. At `.call()` the classifier
must satisfy `C: Decide<O>` for the operation's outcome type `O`.

#### Strategy overrides

**6.3.1** Strategy overrides are available on every `Retry` / `AsyncRetry`
builder, whether it owns its strategies (ext-trait / free-function path) or
borrows them from a policy. On a policy-borrowed builder an override replaces
the borrowed slot with an owned value for that call only.

- `.when(p: impl Predicate<T, E>) -> Retry<F, When<P>, S, W, ...>`
- `.until(p: impl Predicate<T, E>) -> Retry<F, Until<P>, S, W, ...>`
- `.decide(c) -> Retry<F, ClosureClassifier<C>, S, W, ...>` — op-anchored, so an
  inline closure infers with no annotation
- `.wait(w: impl Wait) -> Retry<F, C, S, W2, ...>`
- `.stop(s: impl Stop) -> Retry<F, C, S2, W, ...>`

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
- `.after_attempt(f: impl FnMut(&AttemptState<'_, O>))`
- `.on_exit(f: impl FnMut(&Exit<'_, R, A, O>))`

Hook methods do not change the strategy type parameters. See Hooks for timing,
ordering, and panic behavior.

### 6.4 Sync execution

**6.4.1** Calling `.clock(...)` on sync builders is:

- optional with `std` (defaults to `clock::SystemClock`: a process-global
  monotonic `Instant` anchor for `now()`, `std::thread::sleep` for the wait)
- required without `std`; omitting it is a compile error (`SystemClock`
  implements no clock capability there, so `.call()` is not available)

Terminal execution:

- **6.4.2** `.call() -> Result<R, RetryError<A, O>>`: executes the retry loop and
  returns the result. On the default / `.when` / `.until` path this is
  `RetryResult<T, E>`.
- **6.4.3** `.with_stats()` changes the builder so that `.call()` returns
  `(Result<R, RetryError<A, O>>, RetryStats)` instead

**6.4.4** `RetryWithStats` and `AsyncRetryWithStats` expose only terminal
execution (`.call()` for both; the async `.call()` returns a future). They do
not expose hook or clock configuration methods. Configure everything before
calling `.with_stats()`.

The sync loop performs these steps:

```
attempt = 1
loop:
    1.  Fire `before_attempt` with RetryState { attempt, elapsed, previous_delay }.
    2.  Call the user operation with that RetryState; it yields an outcome O.
    3.  Fire `after_attempt` with AttemptState { attempt, elapsed, outcome: &O }
        — BEFORE classification, so it observes every outcome (including the
        terminal one) under a uniform contract.
    4.  Classify the outcome into a Verdict:
          Return(r) → fire `on_exit` (Returned), return Ok(r).
          Abort(a)  → fire `on_exit` (Aborted),  return Err(Aborted { last: a }).
          Retry(o)  → continue below.
    5.  Evaluate the stop strategy; a configured timeout whose deadline the
        elapsed time has met or exceeded also counts as a stop. (See Timeout.)
        If it fires: fire `on_exit` (Exhausted), return Err(Exhausted { last: o }).
    6.  Compute the next wait via Wait::next_wait — only now that a retry is
        certain (never on the terminal attempt).
    7.  If a timeout is configured, clamp delay to max(0, timeout - elapsed).
    8.  If delay > zero, wait for it via the clock; feed the applied delay
        forward as the next attempt's `previous_delay`.
    9.  attempt += 1, continue to step 1.
```

### 6.5 Async execution

**6.5.1** Async execution always requires `.clock(...)` before `.call()` is
available. The crate never auto-selects an async runtime; there is no default
async clock (`SystemClock`, the initial type-state, implements `AsyncClock`
nowhere, so the bound rejects it at compile time).

Terminal execution:

- **6.5.2** `AsyncRetry::call` consumes the builder and returns a future with
  `Output = Result<R, RetryError<A, O>>` (`RetryResult<T, E>` on the `Result`
  path). The builder itself does not implement `Future`/`IntoFuture`.
- **6.5.3** `.with_stats()` changes the builder so that `.call()` returns a
  future with `Output = (Result<R, RetryError<A, O>>, RetryStats)` instead

**6.5.4** `AsyncRetryWithStats` exposes only terminal execution.

The async builder is terminated with `.call()`, which returns a single-use
`Future`:

- the builder itself does **not** implement `Future`; you must call `.call()`
  (mirroring the synchronous `.call()`) and `.await` the returned future
- **6.5.5** polling the returned future after completion always panics

The async loop uses the same transition order as sync execution.

### 6.6 Statistics

Retry execution always tracks statistics internally. The cost is one `u32`
counter, two `Duration` accumulators, and one `StopReason` — no additional
allocations or timing beyond what the chosen clock already provides.

Statistics are accessed via:

- `.with_stats().call()` (sync) or `.with_stats().call().await` (async): returns
  the result paired with `RetryStats`
- the `on_exit` hook, which receives an `Exit` view exposing `attempt()`,
  `elapsed()`, and `stop_reason()`

## 7. Termination semantics

Retry termination is defined by the classifier's verdict and the stop strategy.

### 7.1 Termination table

|Final condition           |Return value                 |`StopReason`|`Exit` variant  |
|--------------------------|-----------------------------|------------|----------------|
|Classifier returns `Return`|`Ok(r)`                     |`Returned`  |`Returned{value}`|
|Classifier returns `Abort`|`Err(RetryError::Aborted)`   |`Aborted`   |`Aborted{last}` |
|`Retry` cut short by stop |`Err(RetryError::Exhausted)` |`Exhausted` |`Exhausted{last}`|

### 7.2 Additional guarantees

- **7.2.1** `after_attempt` fires after every completed attempt, including the final one
- **7.2.2** `after_attempt` fires *before* the classifier consumes the outcome, so
  it observes every raw outcome (including the terminal one)
- **7.2.3** classification always happens before stop evaluation
- **7.2.4** the wait strategy is consulted only once a retry is certain (after the
  stop check passes), never on the terminal attempt
- **7.2.5** `Exhausted` carries `last: O` — on the `Result` path callers match on
  the inner `Result` to distinguish exhausted-error from unmet-condition cases
- `RetryResult<T, E>` is `Result<T, RetryError<E, Result<T, E>>>`
- **7.2.6** `Exit::attempt()` is always the number of completed attempts (>= 1)

## 8. Hooks

Hooks are configured on execution builders, not on `RetryPolicy`.

Timing guarantees:

- **8.1** `before_attempt` fires before the user operation starts, receiving a
  `RetryState`
- **8.2** `after_attempt` fires after every completed attempt (including the final
  one) *before* the classifier consumes the outcome, receiving an
  `AttemptState` that borrows the raw outcome
- **8.3** `on_exit` fires exactly once for each non-panicking terminal path,
  receiving an `Exit` view of the result

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
terminate immediately as `RetryError::Aborted`.

> The crate does not interrupt operations or sleeps mid-execution. Sync sleep
> (`std::thread::sleep`) is inherently uninterruptible. The maximum latency
> between flag-set and termination is one sleep duration plus one operation
> execution time. `.timeout()` bounds sleep duration via delay clamping.

## 10. Thread safety

Async execution types (`AsyncRetry`, `AsyncRun`) are `Send` when all of the
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
type parameters is `Send + Sync`, that `AsyncRetry` is `Send` when all
components are `Send`, and that `Retry` is `Send` when all
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
   (step 8). The wait itself consumes elapsed budget — the clock that waits is
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
  executors need a user-provided lock-based clock. (A lock-based `Sync`
  variant was considered and declined: `Cell` keeps the clock `no_std`-clean
  and dependency-free, and the hand-rolled pattern is small; revisit on real
  demand.)

## 13. Public API surface

The following items are part of the supported surface for new code.

### 13.1 Crate root exports

Types:

- `RetryPolicy`
- `RetryError`, `RetryResult`
- `RetryStats`, `StopReason`
- `RetryState`, `AttemptState`, `Exit`
- `Retry`, `RetryWithStats` (sync builders), `AsyncRetry`,
  `AsyncRetryWithStats` (async builders), `AsyncRun`, `DropStats` (the async
  builder futures)
- classifier vocabulary: `Decision`, `Verdict`, `DefaultClassifier`,
  `ClosureClassifier`, `When`, `Until`

Traits:

- `Stop` (includes `.or()`, `.and()` as provided methods)
- `Wait` (includes `.cap()`, `.chain()`, `.add()`, `.jitter()`,
  `.full_jitter()`, `.equal_jitter()` as provided methods)
- `Predicate` (single method, blanket-implemented for `Fn(&Result<T, E>) -> bool`)
- `Outcome` (user-implementable for owned outcome types); `Decide`,
  `IntoDecision` (sealed engine-facing classifier traits, named only in bounds)
- `Clock`, `SyncClock`, `AsyncClock` (re-exported from `clock`)
- `RetryExt` (blanket-implemented for `FnMut() -> O`)
- `AsyncRetryExt` (blanket-implemented for
  `FnMut() -> Fut where Fut: Future<Output = O>`)

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
  `PredicateResult`

`clock` module:

- traits: `Clock`, `SyncClock`, `AsyncClock`
- types: `SystemClock`, `VirtualClock`, `VirtualWait`
- feature-gated adapter types: `TokioClock`, `EmbassyClock`, `GlooClock`,
  `FuturesTimerClock` (see 12.1)

### 13.3 Combinator type opacity

Combinator and classifier-wrapper types (`StopAny`, `StopAll`, `WaitCapped`,
`WaitChain`, `WaitCombine`, `Jittered`, and the
classifier wrappers `When`, `Until`, `ClosureClassifier`) are **exposed but
unstable**: they are `pub` for technical reasons (they appear in the return
types of composition and classifier-installing methods), but users should not
name them in function signatures. Use `impl Stop`, `impl Wait`,
`impl Predicate<T, E>`, or the builder methods instead.

> Combinator type names and their generic parameters may change in minor
> releases.

## 14. Standard trait implementations

All public types implement `Debug` (C-DEBUG). Types implement `Clone`, `Copy`,
`PartialEq`, `Eq`, `Hash`, and `Default` when all their components support it.
Composite types derive traits conditionally on their type parameters.

|Type                   |`Clone`|`Copy`|`PartialEq`|`Eq`|`Hash`|`Default`|`Display` |
|-----------------------|-------|------|-----------|----|------|---------|----------|
|`RetryState`           |yes    |yes   |yes        |—   |—     |—        |—         |
|`AttemptState<'a,O>`   |yes    |yes   |—          |—   |—     |—        |—         |
|`Exit<'a,R,A,O>`       |yes    |yes   |—          |—   |—     |—        |—         |
|`RetryStats`           |yes    |yes   |yes        |yes |—     |—        |—         |
|`StopReason`           |yes    |yes   |yes        |yes |yes   |—        |yes       |
|`RetryError<A,O>`      |A,O    |—     |A,O        |A,O |—     |—        |†         |
|All stop strategy types|yes    |yes   |yes        |yes |—     |—        |—         |
|All wait strategy types|yes    |yes   |yes        |*   |—     |—        |—         |
|All predicate types    |F      |—     |—          |—   |—     |—        |—         |
|Combinator types (A,B) |A,B    |—     |—          |—   |—     |—        |—         |

Cells with type names (e.g. "T,E" or "F" or "A,B") indicate the trait is
implemented conditionally when those components implement the trait.

\* `WaitExponential` implements `PartialEq` but not `Eq` because it contains
an `f64` field (the exponential base). All other wait strategy types implement
both `PartialEq` and `Eq`.

† `RetryError`'s `Display` (and `std::error::Error`) are provided on the
`Result` shape `RetryError<E, Result<T, E>>`, when `E: Display` (respectively
`E: Error + 'static`, `T: Debug + 'static`).

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
| `tests/composition.rs` | §3.1–3.3 (seeded property tests) |
| `tests/async_no_alloc.rs` | §2 (no_std/no_alloc async compilation) |

**Seeded property tests.** `tests/composition.rs` verifies that `Stop` and
`Wait` composition obeys boolean and arithmetic algebra across 1,024
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
