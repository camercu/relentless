# 5. Unify the read-clock and the sleeper into one injected clock

Date: 2026-07-16

## Status

Accepted and implemented (2026-07-17). The port shipped as a deliberate
breaking release: `.clock()` replaced `elapsed_clock`/`elapsed_clock_fn`/
`.sleep`, `SystemClock` is the sync default, the runtime adapters became
`AsyncClock` implementors under renamed `*-clock` features, and the core
`clock::VirtualClock` (poll-advancing async wait) replaced the `test-util`
module. The interim "Option 1" test-seam unification was subsumed by the full
port before it shipped separately.

Informed by the 14-spike exploration recorded in
[the unified-clock spike findings](../spikes/unified-clock-findings.md).

## Context

The crate injects time through **two independent seams**, each duplicated across
the sync and async builders:

- a **read-clock** — `elapsed_clock(fn() -> Duration)` /
  `elapsed_clock_fn(impl Fn() -> Duration)` — backed by `ElapsedClockFn` /
  `ElapsedClock` (`policy/time.rs`) and `ElapsedTracker`. It drives `timeout`,
  `stop::elapsed`, and `stats.total_elapsed`.
- a **sleeper** — `sleep(...)`, via the `SyncSleep` (sync) and `Sleeper` (async)
  traits — which performs the real inter-attempt wait and drives
  `stats.total_wait`.

The two **must agree** (if the sleeper advances time by 50ms, the read-clock must
report 50ms passed) but nothing in the type system forces it. `test_util::
VirtualClock` documents exactly this footgun: a caller wires the clock's `now`
and the sleeper separately and can silently desync them, after which
`timeout`/`stop::elapsed` behave incoherently against the recorded waits. The
same hazard bites real paused-time async runtimes (`tokio::time::pause`), which
must separately wire the runtime's `Instant` and its `sleep`.

A related hazard rides on the read seam being optional: a `timeout` configured
without a clock silently no-ops under `no_std` (SPEC 11.2). This is guarded by
`debug_assert_timeout_has_clock`, which under `std` is a tautology (because
`ElapsedTracker::start` installs a fallback `Instant`), forcing a
`.cargo/mutants.toml` exclusion for the unkillable-under-std mutant plus a
dedicated `--no-default-features` test to cover it.

A 14-design spike round ("Option 2") explored unifying both seams into **one
injected value** supplying both `now()` and the wait. Every mechanism was tried —
trait hierarchies, a closure bundle, a single associated `Sleep`, a fat trait, a
descriptor token, enum dispatch, const generics, and type-state — across
independent clean-room and informed builds. The exploration converged, both by
four independent designers reaching the same structure and by elimination of the
alternatives: **capability separation via traits is the answer, and every other
mechanism routes its gate through a trait bound anyway while adding cost.** The
capstone, spike v15, is the recommended interface.

## Decision

Adopt spike v15's interface: **one injected clock value, capability split into
sibling traits over a read-only base.**

```rust
pub trait Clock {
    fn now(&self) -> Duration;                       // the read seam
}
pub trait SyncClock: Clock {
    fn wait(&self, dur: Duration);                   // blocking wait (sync engine)
}
pub trait AsyncClock: Clock {
    type Wait: Future<Output = ()>;                  // named concrete future (no TAIT)
    fn wait_async(&self, dur: Duration) -> Self::Wait;
}
```

The two builder setters (`elapsed_clock` / `elapsed_clock_fn` and `sleep`) are
replaced by a single `.clock(c)`, bound `C: SyncClock` on the sync builder and
`C: AsyncClock` on the async builder. `Clock::now` supersedes
`ElapsedClockFn`/`ElapsedClock`; `SyncClock::wait` supersedes `SyncSleep`;
`AsyncClock::wait_async` supersedes `Sleeper`.

Rationale for this shape (full record in SPIKE-FINDINGS.md):

- **Capability is type-visible.** A sync-only clock implements `Clock` +
  `SyncClock` and is *compile-rejected* by the async engine (`C: AsyncClock`
  unsatisfied) — it can never silently no-op an async wait. This is the
  load-bearing safety property that the naive "one flat trait" designs violate.
- **Sibling, not supertrait.** An async-only clock implements `Clock` +
  `AsyncClock` without being forced to carry a blocking `wait` (which a
  supertrait `AsyncClock: SyncClock` would force); a dual-capable clock
  implements all three.
- **Plain associated future, no GAT.** `wait_async` returns an *owned* named
  future (a runtime's timer future in production, `Ready<()>` for a virtual
  clock). A future that borrows `&self` would force a GAT plus higher-ranked
  bounds that propagate through every engine and builder signature.
- **Zero-cost.** Injected as a generic and monomorphized — no `dyn`, no vtable,
  no allocation — the same cost class as today's fn-pointer paths.

Deviations from the spike for the real crate:

- **Traits are NOT sealed.** The spike sealed them for its closed demo; the real
  crate must let third parties implement `Clock` for their own runtimes (this is
  also why the advance-coupling below cannot be type-forced).
- A default `SystemClock` (`std`: `Instant` + `thread::sleep`) ships as the
  sync-engine default, and the existing sleep feature-adapters
  (`tokio-sleep`, `embassy-sleep`, `futures-timer-sleep`, `gloo-timers-sleep`)
  become `AsyncClock` implementors that pair a coherent `now` with their wait.

## Consequences

### What it dissolves (net simplifications)

- **The desync footgun becomes unrepresentable.** One value owns both `now` and
  the wait, so `total_elapsed` and `total_wait` always come from the same source.
  The `test_util::VirtualClock` warning and the whole "clock A / sleeper B" class
  of bugs are deleted.
- **The SPEC 11.2 no-clock hazard is dissolved.** `now()` is mandatory on a
  `Clock`, so a `timeout` cannot be configured without a clock. This deletes
  `debug_assert_timeout_has_clock`, its `.cargo/mutants.toml` exclusion, the
  `timeout_without_clock_panics_in_debug` test, and the fallback-`Instant`
  special-casing in `ElapsedTracker` — real code and process complexity removed.
- **Duplicated builder surface shrinks.** Four setters
  (`elapsed_clock` + `elapsed_clock_fn`, each on the sync and async builder,
  plus `sleep`) collapse to one `.clock()` per engine, reducing the 4-file
  sync/async drift surface the parity suite exists to catch.
- **Paused-time runtimes become coherent by construction.** A `TokioClock`
  supplying both `now` (`tokio::time::Instant`) and `wait_async`
  (`tokio::time::sleep`) is correct under `tokio::time::pause` without the user
  wiring two matching sources.

### What it costs

- **Breaking, across the public builder surface.** `elapsed_clock`,
  `elapsed_clock_fn`, and `sleep` are removed in favour of `.clock()`; every
  consumer that sets a clock or sleeper migrates. `public-api.txt` re-blessed;
  the parity harness updated. (Pre-1.0, semantic-release bumps this as a MINOR;
  it is nonetheless a real API break.) ADR-0004 already dropped `Fut`/`SleepFut`
  from the async aliases, shrinking the signatures this touches.
- **Feature-adapters grow from sleeper to clock.** Each async runtime adapter
  must now pair a `now` source with its wait. This is the coherence win, but it
  is real per-adapter work and raises a genuine question for runtimes without an
  obvious monotonic `now` (`futures-timer`, `gloo-timers`/wasm need an explicit
  now-source shim). Adapters that cannot supply a coherent `now` cannot become
  `AsyncClock`s cleanly.
- **`std` ergonomics need a shipped default.** Today `std` users get the read
  clock free (fallback `Instant`) and only wire a sleeper. A mandatory `Clock`
  means naming one; mitigated by shipping `SystemClock` as the default so the
  common path stays a no-op, but it is more surface than the current implicit
  fallback.

### Open items to resolve during the port (not spike-solvable)

- **The gate does not force the advance.** The type system rejects the wrong
  *engine*, but nothing forces that a `wait` actually advances `now()` for an
  arbitrary third-party `Clock` (a hand-rolled impl reading one cell and
  advancing another still type-checks). This is structural only for clocks whose
  `now` and wait share one store (the shipped virtual clock does; a real OS clock
  is guaranteed by the scheduler). It remains a documented per-impl contract —
  forcing it in general would re-impose the fat-trait shape the split avoids and
  still could not constrain a real `thread::sleep`.
- **Eager vs poll-time async advance.** A virtual `wait_async` must advance on
  first `poll`, not at call time: the real async engine can build a wait future
  without awaiting it (select/timeout races, cancel-before-poll), and an eager
  advance would move `now`/`total_wait` for a wait that never happened. Real
  runtime timer futures already advance on poll; only the shipped virtual/test
  clock needs a small poll-advancing named future instead of `Ready<()>`.

### Net assessment

**Net positive — as a deliberate breaking release, not as incremental polish.**

The simplifications are genuine maintenance wins that partly pay for the churn:
they *delete* a whole hazard class (SPEC 11.2 + the assert + the mutants
exclusion + a test + the fallback-clock special-casing), make the crate's most
documented footgun unrepresentable, and shrink the duplicated builder surface —
all at zero runtime cost, and with an upside for real paused-time users, not just
tests. Against that, the cost is a real but bounded API break plus per-adapter
work, both of which are one-time and mechanical, and two well-understood port
items with known resolutions.

The verdict is therefore: **accept the design and interface now; schedule the
implementation for a planned breaking release** where the API break, the
`public-api`/parity updates, and the feature-adapter migration can be done
together and the hazard deletions banked as part of the same version. In the
interim, the immediate desync footgun is addressed by the smaller, non-breaking
test-seam unification ("Option 1") rather than by rushing this larger change.
