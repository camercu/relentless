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
use relentless::retry;

// Retry any `Err` with exponential backoff — three attempts by default.
let body = retry(|_| reqwest::blocking::get("https://api.example.com/data")?.text())
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
- **Stats**, where a run reports how many attempts it took, how long it spent
  waiting, and why it stopped.

All of this works in sync and async code, across `std`, `no_std`, and `wasm`
targets.

Inspired by Python's [`tenacity`](https://github.com/jd/tenacity) (composable
strategy algebra) and Rust's [`backon`](https://crates.io/crates/backon)
(ergonomic retry builders).

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
adapters implements `SyncClock`, and `SystemClock` needs `std` and an OS
clock, so an embedded caller implements `Clock + SyncClock` over its own tick
source — a delay source is board-specific and the crate does not guess. See
[*Implementing your own
clock*](https://docs.rs/relentless/latest/relentless/clock/index.html) for the
two methods involved.

## The classifier: `when` → `until` → `decide`

The engine's one big idea: every completed outcome is sorted into a **verdict** —
**return** it to the caller (`Ok`), **retry** it, or **abort** with a payload
(`Err`). Three builder methods set that policy, at increasing power:

- `.when(|e| …)` — retry an `Err` while the closure says it is worth retrying;
  any other `Err` aborts, and an `Ok` returns. The classic "retry these errors"
  knob.
- `.until(|v| …)` — retry until an `Ok` value satisfies the closure; every other
  outcome, `Err` included, retries. The polling knob.
- On `retry(op)` and `retry_async(op)`, both closures take their parameter
  from the operation's output, so they need no type annotation. A standalone
  `RetryPolicy` has no operation to infer from, so its closures do:
  `|e: &io::Error| …`.
- `.decide(closure)` — the general three-way form. You return `Return` / `Retry`
  / `Abort` yourself, so *any* outcome type — a poll enum, a search state, a
  sought-after error — drives the loop directly, independent of `Result`.

`.when` and `.until` are conveniences that compile down to the same three-way
verdict `.decide` produces by hand.

---

## Examples

For full docs, see <https://docs.rs/relentless>. Behavior spec:
[docs/SPEC.md](https://github.com/camercu/relentless/blob/main/docs/SPEC.md). Runnable examples live in
[`examples/`](./examples).

Sync `std` builds default to `clock::SystemClock` (wall time +
`std::thread::sleep`), so the sync example below omits `.clock(...)`. The
exception is `wasm32-unknown-unknown`, which has no OS clock: pass one there.
The `std` feature still builds on that target, so a workspace that also
targets wasm can keep default features on; only the default clock is missing.

### 1) Retry with defaults

Wrap the operation in `retry` and call it. Defaults: 3 attempts, exponential
backoff from 100 ms, retry on any `Err`.

```rust,no_run
use relentless::retry;

fn fetch_job_output() -> Result<String, std::io::Error> {
    std::fs::read_to_string("/var/run/background_job.output")
}

let _output = retry(|_| fetch_job_output()).call();
```

### 2) Async retry

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

## More recipes

The [API docs](https://docs.rs/relentless) hold the full recipes, each a
tested example, with runnable programs in [`examples/`](./examples):

- **[Customizing retry behavior](https://docs.rs/relentless/latest/relentless/#customizing-retry-behavior)** — choose
  which errors to retry with `.when`, back off with jitter and a cap, stop
  after N attempts, and bound the whole run with `.timeout`.
- **[Polling for a condition](https://docs.rs/relentless/latest/relentless/#polling-for-a-condition)** — `.until` retries
  until an `Ok` value is ready; an `Option` or `ControlFlow` probe classifies
  itself with no `.until` at all.
  ([`async-polling.rs`](./examples/async-polling.rs))
- **[Classifying an outcome three ways](https://docs.rs/relentless/latest/relentless/#classifying-an-outcome-three-ways)**
  — `.decide` returns `Verdict::Return`, `Retry` or `Abort` for a poll enum,
  so "still working" need not be an error.
- **[Retrying for a specific outcome](https://docs.rs/relentless/latest/relentless/#retrying-for-a-specific-outcome)** —
  hunt for an error and deliver it through `Ok`.
- **[Policy reuse](https://docs.rs/relentless/latest/relentless/#policy-reuse)** — one `RetryPolicy` shared across call
  sites, with strategies composed by `+`, `|` and `&`.
- **[Error handling](https://docs.rs/relentless/latest/relentless/#error-handling)** — what `RetryError::Exhausted` and
  `Aborted` carry, and how to match them.
- **[Observing retries](https://docs.rs/relentless/latest/relentless/#observing-retries)** — there are no hook
  callbacks. Log each attempt inside the operation, log each chosen delay by
  wrapping the wait strategy, and read a `RetryStats` summary via
  `.call_with_stats()`.
  ([`observing-retries.rs`](./examples/observing-retries.rs))
- **Self-classifying outcomes** — implement `Outcome` for a domain type so it
  sorts itself into return / retry / abort with no `.decide` at the call site.
  ([`custom-outcome.rs`](./examples/custom-outcome.rs))
- **Deterministic testing** — `clock::VirtualClock`, wrapped in
  `clock::Recording`, asserts the exact backoff schedule with zero wall-clock
  time spent. ([`testing-with-virtual-clock.rs`](./examples/testing-with-virtual-clock.rs))
- **[Cancellation](https://docs.rs/relentless/latest/relentless/#cancellation)** — the loop observes the cancellation
  your environment already provides (a dropped future, an `AtomicBool`,
  `.timeout(...)`) at attempt boundaries.
  ([`sync-cancel.rs`](./examples/sync-cancel.rs),
  [`async-cancel.rs`](./examples/async-cancel.rs))

---

## How the builders fit together

The full API surface — every strategy, classifier, and type — lives on
[docs.rs](https://docs.rs/relentless). Two things worth knowing up front:

**Reading order.** Chains read best as **when/until/decide** → **wait** →
**stop** → clock → call. That is a convention, not a compiler
contract: the types enforce one structural rule — an async chain must set
`.clock(...)` before it can be awaited (the default clock is synchronous). Order
is otherwise free, and repeating a setter simply overrides the previous value —
including `.clock(...)`, where the last one wins.

**Where you start sets the defaults, not a ceiling.** The free-function
builders begin from the default policy; a shared `RetryPolicy`
(`policy.retry(...)`) begins from that policy's strategies instead. Either way
you can still override `when`/`until`/`decide`/`wait`/`stop` on the resulting
builder — an override on a policy-built retry shadows the policy for that one
call. For reuse, prefer setting the strategies on the `RetryPolicy` itself so
every call site inherits them.

## MSRV

Minimum supported Rust version: **1.85**.

## Contributing

See [`CONTRIBUTING.md`](https://github.com/camercu/relentless/blob/main/CONTRIBUTING.md).

## Release notes

For user-facing changes, see the [changelog](./CHANGELOG.md).

## License

Licensed under either:

- MIT ([LICENSE-MIT](./LICENSE-MIT))
- Apache-2.0 ([LICENSE-APACHE](./LICENSE-APACHE))
