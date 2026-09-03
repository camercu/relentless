# Spike — async cancellation semantics

**Status:** in progress. Living record; doubles as the resumable checkpoint.
**Opened:** 2026-09-03, out of the 2026-08-31 adversarial dogfood pass and the
hardening run that followed it (40 commits, `ba44730f..`).
**Tier:** heavy tournament — two linked questions, several viable mechanisms,
at least one of which changes an authority document.

## 1. Frame

Two questions, linked because both are about what an async retry owes its
caller when the caller walks away.

**Q-A — should the async retry futures be re-poll-safe, and how?**
`AsyncRun` panics when polled after returning `Poll::Ready`. That is deliberate
and documented (SPEC 15.2), but `tokio::select!` over a `&mut` future in a loop
re-polls whichever branch did not complete, so an ordinary async pattern reaches
a panic. Today's only remedy is knowing to wrap the call in `.fuse()`.

**Q-B — should `on_exit` fire when the retry future is dropped?**
It does not today (SPEC 9.1). A dogfood run measured 32 of 32 cancelled retries
leaking the in-flight gauge their `on_exit` decremented. Cancellation is the
ordinary lifecycle of an async future, not an edge case, so the hook's most
natural use is also its least reliable one.

### Requirements (not solutions)

Anything proposed must hold all of these, or name the breach as a finding:

1. **R1** The crate is `no_std`-capable and its default build has zero
   dependencies. A new dependency must be optional and feature-gated.
2. **R2** `#![forbid(unsafe_code)]` stays. No `unsafe`, anywhere, including in
   a spike.
3. **R3** MSRV is 1.85. No unstable features.
4. **R4** The core retry loop is allocation-free; `tests/allocation.rs` asserts
   it. No allocation added to the hot path.
5. **R5** The sync driver is not touched. These are async-only questions.
6. **R6** Hooks are `FnMut` and may panic. Any design that runs a hook from a
   `Drop` impl must say what happens when it panics during unwinding — a panic
   there aborts the process.
7. **R7** An `Exit` value carries a verdict and an outcome. A cancelled run has
   neither. A design that reports cancellation through `Exit` must say what
   those fields hold and why that is honest.
8. **R8** Changing Q-B's answer contradicts SPEC 9.1 and 8.3. That is allowed,
   but the spike must state the authority-doc change it forces.

### Honesty contract

A spike that concludes "this mechanism loses" is a **success**. Never fake a
win, never hide a regression — a lost property, an `Rc` smuggled in to make it
compile, an added allocation, a widened public surface. An unmet requirement is
a first-class finding, not a failure to conceal. "Don't build either" is a
legitimate verdict for this whole tournament.

### Comparability harness

Every spike ships the same two demos, byte-identical in format, driven by a
hand-rolled single-threaded executor with a counting waker — no Tokio, no real
timers, deterministic run to run:

- `demo_repoll`: drive a retry to completion, then poll it again. Report what
  happens (panic / `Pending` / `Ready` / compile error).
- `demo_cancel`: run N retries with an `on_exit` that decrements an in-flight
  gauge, drop each mid-wait, report `fired=<n> gauge=<n>`.

## 2. Round 1 — breadth sweep

Six parallel spikes: four directed at a known mechanism, two pure clean-room
(forbidden from reading this document's mechanism list) to surface a region the
enumeration missed.

| Spike | Q | Mode | Mechanism |
|---|---|---|---|
| `a-futures-core` | A | directed | `FusedFuture` behind an optional `futures-core` feature |
| `a-self-fusing` | A | directed | re-poll returns `Pending` forever; no dependency, no panic |
| `a-cleanroom` | A | clean-room | unanchored |
| `b-drop-hook` | B | directed | `Drop` fires `on_exit` with a new cancelled variant |
| `b-on-cancel` | B | directed | a separate `on_cancel` hook, distinct from `on_exit` |
| `b-cleanroom` | B | clean-room | unanchored |

Status: **3 of 6 reported** (`b-cleanroom`, `a-cleanroom`, `b-on-cancel`).
Three still building: `a-futures-core`, `a-self-fusing`, `b-drop-hook`.

## 3. Lessons captured

### `b-cleanroom` — landed a design AND a kill. Not yet eliminated or accepted.

Two findings that reframe Q-B, both of which the directed spikes were not
looking for:

1. **This was never a documentation gap.** The rule is already stated in four
   places — `on_exit`'s rustdoc, `lib.rs`'s cancellation section, SPEC 9.1, and
   `examples/async-cancel.rs` — one of which names the gauge/permit/span leak by
   example. Correct, prominent, four-times-repeated prose still failed 32 of 32.
   Any verdict of "document it better" is now disproven, not merely unattractive.

2. **Cancellation-safe cleanup is already expressible today**, and that is the
   actual defect. A closure that *owns* the guard survives drop
   (`.on_exit(move |_| { slot.take(); })`); one that *calls* a method leaks
   (`.on_exit(move |_| gauge.dec())`). Same slot, same single line, opposite
   behavior — verbatim the repo's own "correct call and wrong call look
   identical" smell. That names the lever: make the two spellings *look*
   different rather than adding capability.

Its design: `.holding(resource)` — a `Holding<G>` occupying an exit-hook
position, released exactly once (taken on the verdict path, dropped with the
future on cancellation). No new type parameter: the exit-hook chain is already
a compile-time heterogeneous list of things that happen at exit.
`demo_cancel` goes `fired=0 gauge=8` → `fired=8 gauge=0`, `demo_repoll`
unchanged, 0 existing tests changed, 9 added.

**CARRY-FORWARD KILL — bears directly on `b-drop-hook` and `b-on-cancel`.**
It built the rejected alternative to disprove it: a cancellation hook fired
from `Drop`. Operation panics → future dropped mid-unwind → hook panics →
"panic in a destructor during cleanup" → **`exit=134`**, uncatchable by
`catch_unwind`. A crate that maintains a panic inventory (SPEC 15) cannot add
an unconditional SIGABRT. The asymmetry it names is the load-bearing one:
*dropping* a caller-owned value adds no failure mode; *calling* user code from
a destructor does.

Both directed B spikes were independently told to confront this hazard with a
demonstrating test. Their answers are worth having before this is treated as
settled — an independent reproduction, or a design that dodges it, is exactly
what the tournament is for.

Also rejected there, with reasons: docs-only (measured 0/8); a ninth type
parameter (same semantics, far more churn); a free future combinator (invisible
at the point of confusion); renaming `on_exit` (breaking, removes the wrong
slot). Tier 1 judged unreachable — the crate cannot inspect a closure body.

Self-declared limits: sugar, not new power; cannot fix already-written call
sites; cannot tell the guard *why* the run ended; `RetryPolicy` still cannot
carry a resource; a builder with `.holding()` is not `Clone`; async-only.

### `a-cleanroom` — found a live defect in the crate, not just a design answer

**Verdict it reached: the panic is correct and should stay; the crate's
documented *remedy* for it was wrong.** `AsyncRetry::call` told callers to
reach for `futures::FutureExt::fuse`. Verified independently against tokio
1.47.1's macro source: `tokio::select!` contains **zero** occurrences of
`Fused` and never consults `FusedFuture`, while a fused future returns
`Poll::Pending` forever once resolved. The advice traded a panic that names the
offending line for a task that hangs silently — in exactly the case the
sentence singled out. **Fixed on main immediately** (`fix(docs): retract the
.fuse() remedy`), along with the same wrong premise in this catalog's item 7.
That is not a spike outcome to be gated; it was shipped guidance that was
actively harmful.

Second finding, independent of the first: `#[doc(hidden)]` on `AsyncRun` and
`DropStats` meant `cargo doc` emitted no `struct.AsyncRun.html`, so **the
`# Panics` section SPEC 15.6 claims lives there was never rendered**, and
`cargo public-api --simplified` skips doc-hidden items, so the return types of
the crate's async entry point sat outside the drift gate entirely. This is hard
evidence for the third deferred design question (unhide the plumbing), which
had been argued on aesthetics alone.

Its design: an inherent `is_terminated()` on `AsyncRun`/`DropStats` reading the
existing `Phase::Done`, no dependency, name- and semantics-compatible with a
later `FusedFuture` impl. Zero existing tests modified; 5 added, including the
crate's first `#[should_panic]` pinning SPEC 15.2.

Rejected there, with reasons: self-fusing to `Pending` (a regression, and
precisely what the bad `.fuse()` advice caused by accident — so already
observed rather than merely predicted); `FusedFuture` behind `futures-core` (a
non-answer for the motivating case, since tokio ignores it); cached re-`Ready`
(needs `Clone` bounds the classifier model lacks); type-level prevention
(unreachable — `Future::poll` cannot consume, and `impl Future for &mut F` is
blanket).

Disclosure it volunteered: a repo-wide grep printed matching lines from two
forbidden files. It did not open them, itemized the lines it saw, and re-scoped
later searches. It had already reached the tokio conclusion from tokio's own
source, so nothing was anchored.

### `b-on-cancel` — mechanism works, loses on cost, and left two gifts

`demo_cancel` goes `fired=0 gauge=8` → `fired=8 gauge=0`. The defect is fixed
8/8. All of R1-R6 hold, with one named breach: the panic guard needs
`std::thread::panicking()`, so `no_std` plus an unwinding panic runtime still
aborts — inherited identically by *any* Q-B mechanism that runs a hook from
`Drop`.

**It answers `b-cleanroom`'s kill rather than dying to it.** Guarding on
`thread::panicking()` skips the hook during unwinding; child-process tests show
the unguarded destructor dying by `SIGABRT` while the guarded one exits 101
with the original panic intact. So "runs user code from `Drop`" is survivable
under `std` — the abort is a property of the naive form, not of the mechanism.

Why it still loses: `public-api.txt` goes 1538 → 1568, and of the 72 added
lines **42 are pre-existing signatures rewritten to thread a `CX` parameter for
zero new capability**; exactly one line is `on_cancel` itself. `AsyncRun`
reaches 11 type parameters. Two of the repo's own nameability tests break. A
42:1 churn-to-capability ratio is the finding, and it indicts the *type-state
encoding* rather than the feature: with hooks held as one non-positional value
instead of three positional parameters, a fourth event would cost ~30 new lines
and none of the 42 rewritten ones.

Two carry-forwards, both worth having whichever mechanism wins:

1. A `Cancelled { attempt, elapsed }` payload with no verdict, no outcome and
   no `Exit` type parameters **dissolves R7 by construction** — `Exit`'s
   contract survives untouched and the SPEC changes become purely additive
   (8.3, 8.7, 8.8, 9.1 all stay true). Any Q-B design should adopt this shape.
2. It found a **pre-existing latent defect in this run's own zero-delay yield**:
   the attempt counter advanced before `Pending` on the zero-delay path and
   after the sleep on the other, so the counter meant two different things at a
   poll boundary. Unobservable today, a trap for anything that later reports
   state from a drop. **Landed independently on main** as
   `refactor(async): park a zero-delay yield in its own phase`.

## 4. Verdict

_(pending)_
