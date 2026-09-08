# 8. Async cancellation semantics

Date: 2026-09-08

## Status

Accepted, both parts deferred. Neither recommendation is built. The re-poll
panic and the cancellation behaviour are unchanged; the one defect the
investigation found in shipped code — wrong remedial advice — was fixed
immediately and separately.

## Context

An adversarial dogfooding pass raised two findings that were not defects but
design questions, each with a real trade-off, so they were resolved empirically
by a spike tournament rather than by argument.

**Q-A — re-poll safety.** `AsyncRun` panics when polled after returning
`Poll::Ready` (SPEC 15.2). That is a deliberate contract: the state machine has
no valid state left. But `tokio::select!` over a `&mut` future in a loop
re-polls whichever branch did not complete, so a consumer reaches the panic from
ordinary code.

**Q-B — cleanup on cancellation.** `on_exit` does not fire when the retry future
is dropped (SPEC 9.1). A measured run had 32 of 32 cancelled retries leak the
in-flight gauge their `on_exit` decremented. Cancellation is the ordinary
lifecycle of an async future, so the hook's most natural use — releasing a
permit, decrementing a gauge, closing a span — is also its least reliable.

Six spikes were dispatched: four directed at named mechanisms, two clean-room
(forbidden the mechanism list) so that agreement between them would carry
evidential weight. Three reported. Three died to infrastructure and their
artifacts, along with those of the three that finished, were lost to a scratch
reaper. Round 1 closed as decision-ready rather than complete: on Q-A the
missing spikes held mechanisms already rejected on source evidence, and on Q-B
the two survivors agreed on every measured fact and differed only in judgment.

## Decision

### Q-A — keep the panic; prefer an inherent `is_terminated()` if anything

The panic is correct and stays. What was wrong was the crate's *remedy* for it.

`AsyncRetry::call` advised `futures::FutureExt::fuse`. Verified against tokio
1.47.1's macro source: `tokio::select!` contains zero occurrences of `Fused` and
never consults `FusedFuture`, while a fused future returns `Poll::Pending`
forever once resolved. The advice therefore traded a panic naming the offending
line for a task that hangs silently — in exactly the case the sentence singled
out. **Retracted in `fix(docs): retract the .fuse() remedy for the re-poll
panic`.** The remedy is to stop polling once the future completes; fusing is
correct only under `futures::select!`, which does consult the trait.

If re-poll safety is ever wanted, the shape is an inherent `is_terminated()`
reading the existing `Phase::Done`: no dependency, `no_std`-clean, and name- and
semantics-compatible with a later `FusedFuture` impl. A `futures-core` feature
is declined for now — it does nothing for the `tokio::select!` case that
motivated the question, and serves an audience that has not asked.

Self-fusing — returning `Pending` forever instead of panicking — is **rejected
on evidence**. It is precisely what the bad `.fuse()` advice produced by
accident, so it has already been observed converting a diagnosable failure into
a hang.

### Q-B — prefer `.holding(resource)`; `on_cancel` deferred on cost

Firing a hook from `Drop` was measured, not assumed. Both spikes that examined
it agree on the facts:

- unguarded, when the operation panics and the future drops mid-unwind and the
  hook panics, the process aborts (`exit=134`), uncatchable by `catch_unwind`;
- guarding on `std::thread::panicking()` skips the hook and survives, exiting
  101 with the original panic intact;
- that guard needs `std`, so `no_std` plus an unwinding panic runtime still
  aborts, unfixable in `core` without `unsafe` or a dependency.

They differ only on whether that residue disqualifies the mechanism for a
`no_std`-first crate. That is a values judgment, not a fact a further spike
settles.

`.holding(resource)` sidesteps it. The decisive observation is that
cancellation-safe cleanup is **already expressible**: a closure that *owns* its
guard survives drop, while one that *calls* a method leaks — same slot, same
single line, opposite behaviour. That is the crate's own "correct call and wrong
call look identical" smell, and it means the defect is a naming problem rather
than a capability gap. `.holding(g)` makes the safe spelling look different: a
value occupying an exit-hook position, released once, dropped with the future on
cancellation. It runs no user code from `Drop`, so it adds no abort path in
`std` or `no_std`.

`on_cancel` — a separate hook with a `Cancelled { attempt, elapsed }` payload —
is the more capable design and is **not** rejected on abort risk, which it
answered. It is deferred on cost: 42 pre-existing signatures rewritten to thread
a type parameter for one line of new capability, and `AsyncRun` at 11 type
parameters. That ratio is an argument about the positional type-state encoding,
not about cancellation. Revisit if hooks are ever held as one non-positional
value, at which point it would cost roughly 30 new lines and none of the
rewritten ones.

Documenting the behaviour better is **rejected on measurement**: the rule
already appears in four places including a worked example, and 32 of 32
cancelled retries still leaked.

## Consequences

- Neither recommendation ships. Building either means writing it fresh against
  main — the spike code that demonstrated both is gone, and spike code is not
  written to a shipping bar in any case.
- A `Cancelled` payload carrying no verdict and no outcome is the right shape
  for any future Q-B design: it leaves `Exit`'s contract untouched and makes the
  SPEC changes purely additive.
- The tournament found and fixed one live defect in shipped code (the `.fuse()`
  advice) and one latent defect introduced during the same hardening run (the
  zero-delay yield advanced the attempt counter at a different point than the
  sleeping path, landed as `refactor(async): park a zero-delay yield in its own
  phase`).
- `on_cancel`'s churn ratio is the third independent finding pointing at the
  builder's positional type-state encoding as the thing worth changing.
