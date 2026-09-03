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

Status: **dispatched**.

## 3. Lessons captured

_(filled as rounds complete — why-lost, what-didn't-work, carry-forward leads)_

## 4. Verdict

_(pending)_
