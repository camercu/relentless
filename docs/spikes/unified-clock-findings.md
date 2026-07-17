# Spike: unified `Clock` abstraction (Option 2) — findings

**Status:** concluded. The exploration produced ADR-0005, which is implemented:
the crate's `clock` module realizes the v15 capstone interface. The prototype
files (`examples/spike_unified_clock*.rs`) never landed on `main` — this
document and the ADR carry the lessons; the files remain reachable on the
`spike/unified-clock` branch.

## Question

Replace the two independent time seams —
`.elapsed_clock(fn)` (read "now") and `.sleep(fn)` (advance/block) — with a
single `Clock` object injected once via `.clock(c)`, so the read-clock and the
sleeper can never desync.

## What the spike shows

### Upsides (real, confirmed)

1. **Mismatch impossible by construction.** One object implements both `now()`
   and `sleep()`, so `stats.total_elapsed` (read side) and `stats.total_wait`
   (sleeper side) always come from the same source. Deletes the
   `VirtualClock` doc warning (test_util.rs:83-90) and the whole class of
   "sleeper from clock A, elapsed from clock B" bugs.
2. **Helps real `tokio::time::pause` users, not just tests.** Today a paused-time
   tokio user must separately wire tokio's `Instant` (reads) and tokio's `sleep`.
   A `TokioClock` supplying both makes paused virtual time coherent by
   construction. So the footgun is not test-only.
3. **Dissolves the SPEC 11.2 hazard.** `now()` is mandatory on a `Clock`, so a
   timeout can never be configured without a clock. That removes the no_std
   silent-no-op hazard, the `debug_assert_timeout_has_clock` guard, AND the
   mutants exclusion just added for it (see `.cargo/mutants.toml`). Option 2
   subsumes the CI mutant issue.
4. **Zero-cost preserved.** Injected as a generic `C: SyncClock` / `C: AsyncClock`
   the loop monomorphizes — no vtable, no alloc, same cost class as today's
   fn-pointer / const-generic paths. Passed by `&C`, so a shared `&VirtualClock`
   handle needs no `Rc`.

### Costs / tensions (why it isn't free)

1. **The trait bifurcates — there is no single `Clock` trait for both engines.**
   Sleep is sync-blocking on one engine, async-future on the other, so the
   abstraction is really `Clock` (shared `now`) + `SyncClock` (blocking sleep) +
   `AsyncClock` (future sleep). This mirrors the existing `SyncSleep` vs
   `Sleeper` split. "Inject one clock" works *per engine*; the read half is the
   only part that was ever cross-engine unifiable — and it is ALREADY unified
   (`ElapsedTracker` -> `state.elapsed`). So Option 2's true delta is "bundle the
   engine-specific sleeper WITH the shared reader", not "collapse N clocks to 1".
2. **Breaking, and broadly.** `.sleep(...)` is mandatory before `.call()` today
   (enforced by compile_fail doctests), so every sync and async consumer sets it.
   Replacing `.sleep` + `.elapsed_clock` with `.clock` breaks all of them.
   Migration path exists (keep `.sleep`/`.elapsed_clock` as deprecated shims that
   build an ad-hoc `Clock`), but it is a major-version change touching the 4-file
   duplicated builder surface + parity harness + public-api baseline.
3. **Ergonomic regression for the common std case** unless a default is shipped.
   Today std users get the read clock for free (fallback `Instant`) and only pass
   a runtime sleeper. A mandatory clock object means naming `StdClock` (sync) or a
   runtime clock (async). Mitigated by shipping `StdClock` as the `NoSyncSleep`-
   style default, but that is more surface than the current implicit fallback.
4. **Async `now()` source is a real choice, not free.** For async there is no
   single obvious `now()` — `std::Instant`? `tokio::time::Instant`? The unified
   object forces the user to pick a coherent pair, which is the point (upside 2)
   but also means no free default for non-tokio async runtimes.

## Assessment

Option 2 is a **legitimate, coherent design** with more upside than the earlier
"fights the runtime decomposition" framing implied — it helps `tokio::time::pause`
users and dissolves the SPEC 11.2 hazard. But it is a **breaking, major-version
change** across the duplicated builder surface, and its core simplification is
"bundle sleeper + reader", not "collapse many clocks" (elapsed/stop/stats already
share one). The read/sleep split cannot be removed; it can only be co-located.

**Recommendation:** ship **Option 1** now (unify only the test seam — small,
non-breaking, kills the immediate footgun). Hold Option 2 for a deliberate
breaking release, and if pursued, scope it to also delete the SPEC 11.2 hazard +
`debug_assert_timeout_has_clock` + the mutants exclusion (net simplification that
partly pays for the churn). A fresh ADR should record the read/sleep bifurcation
finding either way.

## Interface comparison: trait hierarchy vs closure bundle vs single trait

Three prototypes explore the SAME Option-2 goal (inject read + sleep as one
value) with different interfaces, so the design cost lives in the interface, not
the concept.

- **`spike_unified_clock.rs` — trait hierarchy.** `Clock` (shared `now`) +
  `SyncClock` / `AsyncClock` supertraits. A consumer supplies a clock by
  declaring a type and writing two `impl` blocks; the abstraction bifurcates
  into three traits. Virtual clock shares its cell via `&self` + one `RefCell`
  — no alloc, `no_std`-clean.
- **`spike_unified_clock_v2.rs` — closure bundle.** One plain struct
  `Clock<R, S> { now, sleep }` built from closures, injected once. The struct is
  engine-agnostic and never bifurcates; sync vs async appears ONLY in the
  consuming loop's sleeper bound (`S: Fn(Duration)` vs
  `S: Fn(Duration) -> impl Future`). Matches the crate's existing closure seams.
- **`spike_unified_clock_v3.rs` — single trait, associated sleep type.**
  Collapses v1's three traits to ONE: `trait Clock { type Sleep; fn now(&self)
  -> Duration; fn sleep(&self, dur) -> Self::Sleep; }`. Sync impls set
  `Sleep = ()`; async impls name a concrete future (`core::future::Ready<()>`
  for the virtual clock, a runtime's named sleep future in production). The
  sync/async split lives in the consuming loop's bound (`Clock<Sleep = ()>` vs
  `C::Sleep: Future`), like v2 — but the shared cell is owned by one `&self`
  object, so the coupled-state virtual clock needs **no `Rc`**, like v1.

**Verdict (fresh review, two independent agents):** v2 is a cleaner *interface*
than v1 (closures over trait impls, non-bifurcating), but it *relocates*
complexity rather than removing it, and pays for it:

- The irreducible read/sleep split moves from the type system (three traits)
  into the loop bounds. Honest, not eliminated.
- **v2's closure form pays an `Rc` cost the trait forms avoid.** Two
  independently-owned closures that must share mutable state (exactly the
  deterministic test clock) force an `Rc` heap allocation + an `alloc`/`std`
  dependency. v1 and v3 share the same cell for free via `&self`. This nuances
  Upside 4 above ("zero-cost preserved … needs no `Rc`"): it holds for the
  *loop* (monomorphized, no vtable) and for production clocks whose halves share
  nothing (`std_clock`), but NOT for the coupled-state clock the spikes center
  on.

**v3 is the best of the three.** It keeps v1's alloc-free `&self` sharing while
cutting v1's two impl blocks (+ two supertraits) to **one impl block on one
trait**, and it moves the split into a bound (v2's clean relocation) *without*
paying v2's `Rc`. It dominates v2 outright and dominates v1 on boilerplate at no
cost.

- **Residual cost (not a regression):** a clock *type* has one `Sleep`, so it
  targets one engine — a separate `AsyncVirtualClock` is needed for the async
  sketch. That is exactly v1's existing "a type is a `SyncClock` OR an
  `AsyncClock`" property, expressed via the associated type instead of which
  supertrait is implemented. Only v2 can reuse one *value* across engines, and
  it pays `Rc` for the privilege in the coupled-state case; production clocks
  are single-engine anyway.
- **Stable-toolchain caveat that shaped the design:** the async side must name a
  *concrete* `Sleep` type; `type Sleep = impl Future` needs
  `impl_trait_in_assoc_type` (unstable) and does not build on the pinned stable
  toolchain. Naming a concrete future is what real clocks do anyway, so this is
  free in practice but must be stated.

If Option 2 is pursued, **v3's single-trait shape is the interface to carry into
the ADR.**

## Branch contents

- `examples/spike_unified_clock.rs` — trait-hierarchy prototype (traits,
  `StdClock`, `VirtualClock`, a generic `retry_with_clock` loop, sync + real
  demos).
- `examples/spike_unified_clock_v2.rs` — closure-bundle prototype (same
  scenario + byte-identical output; single `Clock<R, S>` struct, closure
  constructors, sync loop + an async-loop sketch showing the struct is reused
  verbatim across engines).
- `examples/spike_unified_clock_v3.rs` — single-trait prototype (same scenario +
  byte-identical output; one `Clock` trait with an associated `Sleep` type,
  `StdClock` + sync/async `VirtualClock`s, sync loop + an async-loop sketch;
  alloc-free `&self` sharing with one impl block per clock).
- This file.

Not wired into the real engine (a full rewire is out of spike scope).

> **Note (updating):** the sections above predate the later spikes v5–v11 and
> refer to v2/v4/v6 which have since been eliminated (see below). A full,
> consolidated multi-version comparison is written at the end of the spike round;
> until then, treat the per-version agent reports + the ledger as authoritative
> for v5+.

## Eliminated designs and the lessons they bought

Spikes are kept only while they teach something no surviving design teaches.
When a survivor subsumes a spike's idea AND fixes its flaws, the spike is deleted
and its lesson recorded here so the knowledge outlives the file.

### v2 — closure bundle (`struct Clock<R, S> { now, sleep }`) — ELIMINATED (v3 dominates)
Lesson: bundling the two seams as two independently-owned closures makes the
mismatch unrepresentable, but a *coupled-state* clock (the deterministic test
clock, where a sleep must advance the very `now` reads report) then needs the two
closures to share one cell — which forces an `Rc` heap allocation and an
`alloc`/`std` dependency. Trait/`&self` forms (v1, v3, v5, v7, v8) share the same
cell for free and stay `no_std`. **Takeaway: for coupled read/sleep state, a
single `&self` object beats two closures — the closure form's ergonomics aren't
worth the `Rc` tax.**

### v4 — two sleep methods on ONE trait (`sleep_blocking` + `sleep_future`) — ELIMINATED (v5 dominates)
Lesson: putting both a blocking and a future-returning sleep on a single
non-separated trait lets one value serve both engines, but forces EVERY clock to
implement BOTH — so a sync-only production clock (`StdClock`) must supply a
meaningless `sleep_future` (a `panic!` stub), and a real async clock a meaningless
`sleep_blocking`. The wart lands on the common production clocks. v5 moves the
split into a supertrait so a sync-only clock implements only the sync half.
**Takeaway: don't force both wait shapes onto one flat trait; separate the async
capability so single-engine clocks stay clean.**

### v6 — single-trait wait *descriptor* WITHOUT capability separation — ELIMINATED (v7/v8 dominate)
v6 introduced the genuinely good **descriptor** idea: `sleep()` returns a
`WaitToken` value and each engine consumes it its own way (`block()` for sync,
`into_future().await` for async) — one `sleep`, no per-engine method. v7 and v8
both keep that idea. But v6 paired it with a **single** `Clock` trait that could
not express "this clock supports the sync engine but NOT the async engine," and
that is a **latent-correctness trap**, not a mere ergonomic wart:

- Because there was one trait, a real sync clock (`RealClock: Clock`) satisfied
  the async engine's bound (`C: Clock`). Its token's `into_future()` had to return
  *something*, so it returned `core::future::ready(())`.
- Wiring that real clock into the async engine then **compiled with no error** and
  **awaited nothing** — a silent busy-loop that never really waits, so `deadline`
  / `timeout` are defeated and `total_wait` is a lie. No compile-time signal, no
  panic; just wrong behavior.

**Takeaway (the load-bearing lesson): unifying the two seams into one abstraction
is not enough — the abstraction must make *capability* type-visible. A clock that
cannot correctly serve an engine must be REJECTED by that engine at compile time,
not allowed to satisfy the bound with a lying no-op wait.** v5 does this with a
`Timeline` / `AsyncTimeline` supertrait split; v7 and v8 do it on the wait side
(`Wait` / `AsyncWait`), so a sync-only token carries zero async surface and the
async engine's bound (`C::Wait: AsyncWait`) rejects a sync-only clock. v6's
`ready(())` stub is exactly the anti-pattern that separation exists to forbid.

Secondary v6 lesson: v6 also advanced virtual time *eagerly* inside `sleep()`
(before the token is consumed), so a token dropped un-consumed still moved `now`.
The advance belongs where the wait is actually performed (on consume), which v7
(via a borrowed token + GAT) and v8 (via `&C` passed at consume-time, no GAT)
both fixed.

### v7 — descriptor + capability gate, with a GAT — ELIMINATED (v8 dominates)
v7 was the necessary bridge: it took v6's descriptor idea and made it SAFE by
splitting the token into `Wait` / `AsyncWait` capability traits (so a sync-only
token carries no `into_future`, closing v6's trap — proven: a real sync clock is
compile-rejected from the async engine, `RealWait: AsyncWait` unsatisfied). It
also moved the time advance onto token *consumption* (fixing v6's eager-advance
smell). To advance on consume, v7's token BORROWED the clock — and a borrow in an
associated type forces a **GAT** (`type Wait<'s>`) plus a **higher-ranked bound**
on the async engine (`for<'s> C::Wait<'s>: AsyncWait`).

**Takeaway: the borrow-to-advance-on-consume is what forces the GAT + HRTB, and
that higher-kinded machinery is not free — it propagates through every generic
engine signature and (in the real crate) the 4-file builder surface.** v8 showed
the borrow is unnecessary: the retry engine already holds `&C`, so it can pass
`&C` to the token's consume methods (`block(self, &C)` / `into_future(self, &C)`)
instead of the token storing a back-reference. That keeps `Clock::Wait` a PLAIN
associated type and the async bound FIRST-ORDER (`C::Wait: AsyncWait<C>`), while
preserving all three properties (coupling, type-visible gate, honest advance).
v8's only give-back is that its virtual *async* token advances at `into_future`
call-time rather than on first poll (observably identical to the engine, moot for
real runtime clocks). Net: v7 proved descriptor+gate could be made SAFE; v8 proved
it could be made safe AND cheap. v7's GAT is the tax v8 removes.

### v1 — original trait hierarchy (`Clock`/`SyncClock`/`AsyncClock`) — ELIMINATED (v11 dominates)
v1 was the first prototype and established the shape four later designs converged
on: a read-clock base trait plus per-engine capability sub-traits. But it treats a
clock as sync XOR async (no single value serving both engines), and it predates
sealing, alloc-free borrow coupling, and the one-value-both-engines property.
Every property v1 has, v11's sibling split has and more.
**Takeaway: the capability-sub-trait shape is the right backbone (v1 found it, and
v5/v9/v10/v11 independently re-found it) — but the baseline itself is fully
subsumed; keep the shape, not the file.**

### v3 — single trait + one associated `Sleep` type — ELIMINATED (v5/v8/v11 dominate)
v3 put the whole wait behind one associated future type per clock (`Sleep = ()`
for sync, a named future for async). Consequence: a clock TYPE targets exactly one
engine, so exercising both engines needs two clock types (`VirtualClock` +
`AsyncVirtualClock`). Every one-value-both-engines design supersedes it, and its
`Sleep = ()` "unit future" trick buys no zero-cost the finalists lack.
**Takeaway: tying the sole wait to one associated future per clock precludes
cross-engine value reuse — this is the wall that motivated splitting capability
into separate traits.**

### v9 — sibling capability split WITH a GAT — ELIMINATED (v11 dominates)
v9 reached the same sibling structure as v11 (`Clock{now}` + `SyncClock` +
`AsyncClock`) but returned a *borrowing* async wait future, which forced a GAT
(`type Wait<'a>`). v11 shows that returning an OWNED `Ready<()>` future needs no
GAT — so v11 is v9's structure minus the GAT tax. v9's only edge (an owning
virtual clock vs v11's borrow-handle) is a clock-implementation detail, not a
trait difference, and ports trivially.
**Takeaways: (1) a *borrowing* async wait future forces a GAT — return an OWNED
concrete future to keep the associated type plain; (2) v9's own insight: capability
separation type-guards *which engine* a clock reaches, but the read/sleep *advance*
coupling stays convention-within-a-clock — no design in this round fully
type-forces the advance itself.**

### v10 — fat trait + separate marker sub-traits — ELIMINATED (weakest; v5/v11 dominate)
v10's base trait carries `now` + `block` + `sleep` together (the eliminated-v4
fat-trait shape), forcing every clock to implement BOTH waits. Its real
`SystemClock` therefore had to give the async `sleep` a body and chose
`thread::sleep(dur)` then `Ready<()>` — i.e. it BLOCKS THE ASYNC REACTOR THREAD, an
async anti-pattern. Worse, it expressed capability as *separate* marker traits but
then marked `SystemClock` as BOTH `SyncClock` and `AsyncClock`, so the gate never
actually stops the sync-native clock from reaching the async engine — the markers
are defeated by their own opt-in. Coupling also regresses to `Rc<Cell>` (alloc)
where v5/v8/v9/v11 use plain `&self` + `Cell`.
**Takeaway: express capability by which METHODS a trait carries (so a sync-only
clock simply cannot implement the async wait), not by marker traits layered on a
fat trait that already forces both methods — the latter is more surface AND easy to
gate wrong.**

### Mechanism sweep — enum (v12), const generics (v13), type-state (v14) — all ELIMINATED

Three designs deliberately probed non-trait mechanisms to be sure the trait-based
capability split wasn't leaving a cleaner idea unexplored. All three reached a
working compile-time gate — but in EVERY case the gate was carried by a
trait-bound capability split, with the headline mechanism adding cost and no
gating power. This closes the design space: capability gating for this problem is
a trait-bound job.

#### v12 — enum-dispatched wait — ELIMINATED (ties-to-loses)
The wait was an enum (`Wait<F> { Ready, Blocking, Future(F) }`) the engines match
on. But an enum tag is a RUNTIME value: it cannot prove at compile time that a
clock supplies a real async wait. The gate worked only because v12 ALSO split
`AsyncClock: Clock` with a named associated future — the trait bound, not the
enum, is what rejects a sync-only clock. Worse, the enum leaves a futureless
`Blocking`/`Ready` arm reachable-by-type inside the async matcher, excluded only
by contract (v12 honestly shipped a `naive_probe()` demonstrating the silent
no-op-spin hazard of the un-split enum).
**Takeaway: runtime-tagged dispatch cannot express a compile-time capability —
the trait bound does the gating regardless, and the enum only adds a
discipline-only wart arm.**

#### v13 — const-generic engine selector — ELIMINATED (loses)
A `const ASYNC: bool` on the engine entry points selected the sync vs async
branch. But a const selects behavior by VALUE; it proves nothing about
capability. A pure-const design (`if ASYNC { wait_future } else { wait_blocking }`)
forces `C: SyncWait + AsyncWait` on one signature — every clock must implement
every engine — because both arms must typecheck regardless of the const. The gate
worked only once the const rode alongside `SyncWait`/`AsyncWait` trait bounds, at
which point the bound does the entire job and the const is inert decoration (plus
turbofish `::<true>`/`::<false>` noise and a const/method-mismatch footgun).
**Takeaway: const generics SELECT a code path; they do not PROVE a capability.
Capability gating is a trait-bound job; the const adds noise, not safety.**

#### v14 — type-state (phantom mode markers) — ELIMINATED (works, not worth it)
Encoded engine capability in the clock's type via phantom mode markers
(`Sync`/`Async`/`Dual`) + capability marker traits. It DID give a clean
compile-time gate (`Sync: AsyncCapable` unsatisfied), but paid a tax a plain
trait split avoids: three markers plus a `Dual` type, overlapping impls
(combinatorial past two capabilities), a `now()` duplicated across gated impl
blocks, and mode-picking constructors that couple call sites to the phantom
vocabulary. Sharpest finding: **type-state gates the WRONG layer.** The desync it
was meant to prevent is stopped by the single backing value, not by the phantom;
and because the source trait still declares both waits, a single-mode source (a
real async timer) still needs a stub blocking wait — the same fat-trait wart from
v4/v10, hidden one layer up.
**Takeaway: type-state earns its keep for tracking mode TRANSITIONS on a value
(e.g. a builder consuming `Clock<Unset>` into `Clock<Sync>`), which this problem
does not need. For "which capabilities does this clock have," trait bounds are
lighter and gate the right layer.**

## Surviving finalists (post-cut): v5, v8, v11

Three designs remain, one per still-open axis:
- **v5 — supertrait split** (`AsyncTimeline: Timeline`): async capability *extends*
  the sync-inclusive base. Owning clock, most-reviewed. Live wart: async-only
  clocks must still carry a blocking `sleep`.
- **v8 — descriptor + capability-split token**: `sleep()` returns a `Wait` /
  `AsyncWait` token the engine consumes; plain associated type, first-order bounds,
  advance-on-consume. Live cost: a token type + a `<C: ?Sized>` parameter on the
  wait traits.
- **v11 — sibling split** (`SyncClock` / `AsyncClock` over `Clock{now}`): capability
  as sibling traits; async-only clocks stay clean (removes v5's wart), plain
  associated future, no GAT. Live wart: borrow-handle ceremony in its virtual clock
  (cosmetic, portable to an owning form).

Open questions for the finalists: supertrait vs sibling capability separation
(v5 vs v11), and direct-method vs descriptor wait delivery (v5/v11 vs v8).
