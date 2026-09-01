# relentless

[![crates.io](https://img.shields.io/crates/v/relentless.svg)](https://crates.io/crates/relentless)
[![docs.rs](https://docs.rs/relentless/badge.svg)](https://docs.rs/relentless)
[![CI](https://github.com/camercu/relentless/actions/workflows/ci.yml/badge.svg)](https://github.com/camercu/relentless/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-blue.svg)](#msrv)
[![License](https://img.shields.io/crates/l/relentless.svg)](#license)

Retry and polling for Rust — with composable strategies, policy reuse, and
first-class support for polling workflows where `Ok(_)` doesn't always mean
"done."

```rust,no_run
use relentless::RetryExt;

// Retry any `Err` with exponential backoff — three attempts by default.
let body = (|| reqwest::blocking::get("https://api.example.com/data")?.text())
    .retry()
    .call();
```

Most retry libraries handle the simple case well: call a function, retry on
error, back off. `relentless` handles that too, but its core idea goes further:
**you classify each outcome, rather than just retrying errors.** Every completed
attempt is sorted into *return it*, *retry it*, or *abort* — so the retry
decision is independent of `Result` semantics. That unlocks the cases other
libraries make awkward:

- **Classification, not just error-retry**, where each outcome is sorted into
  *return / retry / abort* — so polling (`Ok("pending")` means "keep going"), a
  sought-after `Err`, a non-`Result` poll enum, or a search state can drive the
  loop directly. See the [classifier](#the-classifier-when--until--decide)
  section below.
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

Async retries inject an explicit clock adapter, gated behind a feature flag
(`tokio-clock`, `embassy-clock`, `gloo-timers-clock`, `futures-timer-clock`); see
the [feature-flag
reference](https://docs.rs/relentless/latest/relentless/#feature-flags) for the
full list. Async retry does not require `alloc`.

A `no_std` **sync** retry supplies its own clock instead: none of those four
adapters implements `SyncClock`, and `SystemClock` is `std`-gated, so an
embedded caller implements `Clock + SyncClock` over its own tick source — a
delay source is board-specific and the crate does not guess. See [*Implementing
your own
clock*](https://docs.rs/relentless/latest/relentless/clock/index.html) for the
two methods involved.

## The classifier: `when` → `until` → `decide`

The engine's one big idea: every completed outcome is sorted into a **verdict** —
**return** it to the caller (`Ok`), **retry** it, or **abort** with a payload
(`Err`). Three builder methods set that policy, at increasing power:

- `.when(pred)` — retry *while* a `Result` predicate matches; otherwise accept an
  `Ok` and abort an `Err`. The classic "retry these errors" knob.
- `.until(pred)` — the inverse: retry *until* the predicate matches, then accept
  an `Ok` and abort an `Err`, same as `.when`. The polling knob.
- `.decide(closure)` — the general three-way form. You return `Return` / `Retry`
  / `Abort` yourself, so *any* outcome type — a poll enum, a search state, a
  sought-after error — drives the loop directly, independent of `Result`.

`.when` and `.until` are conveniences that compile down to the same three-way
verdict `.decide` produces by hand.

---

## Examples

For full docs, see <https://docs.rs/relentless>. Behavior spec:
[docs/SPEC.md](./docs/SPEC.md). Runnable examples live in
[`examples/`](./examples).

Sync `std` builds default to `clock::SystemClock` (wall time +
`std::thread::sleep`), so the sync examples below omit `.clock(...)`.

### 1) Retry with defaults

The `.retry()` extension trait is the fastest way to add retries. Defaults: 3
attempts, exponential backoff from 100 ms, retry on any `Err`.

```rust,no_run
use relentless::RetryExt;

fn fetch_job_output() -> Result<String, std::io::Error> {
    std::fs::read_to_string("/var/run/background_job.output")
}

let _output = fetch_job_output.retry().call();
```

### 2) Customized retry

The `retry` free function is equivalent to the extension trait, with the added
ability to capture retry loop state. Both give full control over which outcomes
to retry, how long to wait, and when to stop.

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

### 3) Poll for a condition

Use `.until(predicate)` to keep retrying until a success condition is met.
Unlike `.when()`, which retries on matching outcomes, `.until()` retries on
everything *except* the matching outcome.

```rust,no_run
use relentless::{retry, predicate};

#[derive(Debug, PartialEq)]
enum Status { Pending, Done }

fn poll_status() -> Result<Status, std::io::Error> { todo!() }

let result = retry(|_| poll_status())
    .until(predicate::ok(|s: &Status| *s == Status::Done))
    .call();
```

When the probe is a plain two-state "ready yet?" with no error to carry, return
`Option` and skip the predicate entirely — `Option` classifies itself, so `None`
retries and `Some(v)` delivers `v`:

```rust,no_run
use relentless::retry;

fn read_pidfile() -> Option<u32> { todo!() }

let pid = retry(|_| read_pidfile()).call(); // Result<u32, RetryError<Infallible, Option<u32>>>
```

`ControlFlow<B, C>` classifies itself the same way when you prefer its naming —
`Continue(_)` retries, `Break(b)` returns `b`. (Which standard types get this
treatment, and why others don't, is [ADR-0007](docs/adr/0007-blanket-outcome-impls-for-std-types.md).)

> **Note:** give a self-classifying op a concrete signature, as the examples
> above do. An op that only ever produces one variant can leave a sibling type
> parameter unpinned (`E0282`); the API docs for each type spell out which
> parameters need fixing.

### 4) Classify a custom outcome (all three verdicts)

When an outcome has more than two meanings, `.decide(...)` sorts it directly.
Here a job poll is three-way: `Done` is the success value, `Failed` aborts as
fatal, and `Pending` retries — no need to encode "still working" as an error.

```rust,no_run
use relentless::{retry, stop, Verdict};

#[derive(Debug)]
enum Job {
    Pending,
    Done(String),   // the report we want
    Failed(String), // a terminal failure
}

fn poll_job() -> Result<Job, std::io::Error> { todo!() }

let report = retry(|_| poll_job())
    .decide(|outcome| match outcome {
        Ok(Job::Done(report)) => Verdict::Return(report),
        Ok(Job::Failed(why)) => Verdict::Abort(why),
        other => Verdict::Retry(other),
    })
    .stop(stop::attempts(20))
    .call();
// `report`: `Result<String, RetryError<String, Result<Job, std::io::Error>>>`
// (type is inference-derived). A job stuck at `Pending` exhausts after 20
// attempts as `Err(Exhausted { last: Ok(Job::Pending) })`.
```

With `.until(ok(f))` errors keep the loop going, because `ok(f)` is `false` for
any `Err` and `.until` inverts that. To give up on a fatal error instead, match
the whole outcome with `predicate::result`:

```rust,no_run
use relentless::{predicate, retry, stop};

#[derive(Debug)]
enum Job {
    Pending,
    Done(String),
}

fn poll_job() -> Result<Job, std::io::Error> { todo!() }

let report = retry(|_| poll_job())
    .until(predicate::result(|o: &Result<Job, std::io::Error>| {
        // Stop on either: the job finished, or the API failed for good.
        matches!(o, Ok(Job::Done(_))) || matches!(o, Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    }))
    .stop(stop::attempts(20))
    .call();
```

That stops on either, and — because a matched `Err` aborts — the fatal error
arrives as `Err(RetryError::Aborted { .. })`. To deliver a sought-after error as
the `Ok` value instead, you need the *inverted probe*: return
`Verdict::Return(e)` (or the two-way `Decision::Return(e)`) from `.decide` for
the error you were hunting. `.until` alone cannot do it.

### 5) Reuse a policy across call sites

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

### 6) Async retry

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

## Error handling

The retry loop returns `Ok(R)` — the value the classifier accepted — on success.
On failure it returns a `RetryError` with two variants:

- `Exhausted { last }` — the stop strategy (or the timeout) fired while the
  classifier still wanted to retry. `last` is the final *whole* outcome (a
  `Result<T, E>` on the default / `.when` / `.until` path, since polling can
  exhaust on an unaccepted `Ok` — e.g. a job stuck at `Pending`).
- `Aborted { last }` — the classifier rejected an outcome as fatal. `last` is the
  payload the classifier projected on `Abort` — the bare error `E` on the
  `.when` / `.until` path.

```rust,no_run
use relentless::{retry, RetryError};

match retry(|_| Err::<(), _>("timeout")).call() {
    Ok(val) => println!("success: {val:?}"),
    Err(RetryError::Exhausted { last }) => println!("gave up: {last:?}"),
    // Aborts only arise on the `.when`/`.until`/`.decide` path, not the default.
    Err(RetryError::Aborted { last }) => println!("aborted: {last}"),
    // `RetryError` is `#[non_exhaustive]`; match future variants here.
    Err(_) => {}
}
```

`RetryError` is `Result`-shaped by default, so it implements `Display` and
`std::error::Error` (with the terminal error as its `source`) on that path. The
`Option` poll error `RetryError<Infallible, Option<T>>` also implements both
(`Display` renders `"retries exhausted"`; `Error` has no `source`), so a poll's
error can `?`-propagate just like a `Result`'s. `last()` / `into_last()` and
`stop_reason()` are outcome-agnostic — they work on every shape.

## More

Full inline code for these lives in the [API docs](https://docs.rs/relentless),
with runnable versions in [`examples/`](./examples):

- **Hooks & stats** — observe the retry lifecycle for logging or metrics with
  `.before_attempt` / `.after_attempt`, and collect a `RetryStats` summary via
  `.with_stats()`. ([`hooks-and-stats.rs`](./examples/hooks-and-stats.rs))
- **Self-classifying outcomes** — implement `Outcome` for a domain type (a poll
  enum, a search state) so it sorts itself into return / retry / abort, and the
  default engine drives it with no `.decide` at the call site.
  ([`custom-outcome.rs`](./examples/custom-outcome.rs))
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

**Reading order.** Chains read best as **when/until/decide** → **wait** →
**stop** → clock → hooks → stats → call. That is a convention, not a compiler
contract: the types enforce only two structural rules — `.with_stats()` is
terminal, so configure everything before it, and an async chain must set
`.clock(...)` before it can be awaited (the default clock is synchronous). Order
is otherwise free, and repeating a setter simply overrides the previous value —
including `.clock(...)`, where the last one wins.

**Where you start sets the defaults, not a ceiling.** The free-function and
extension-trait builders begin from the default policy; a shared `RetryPolicy`
(`policy.retry(...)`) begins from that policy's strategies instead. Either way
you can still override `when`/`until`/`decide`/`wait`/`stop` on the resulting
builder — an override on a policy-built retry shadows the policy for that one
call. For reuse, prefer setting the strategies on the `RetryPolicy` itself so
every call site inherits them.

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
