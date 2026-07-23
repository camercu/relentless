# UX polish candidates — findings

**Status:** open catalog. Not a concluded spike. Records UX warts surfaced by a
dogfooding pass over the public surface (2026-07-23), ranked as candidates for
future polish. Each entry names the friction, the evidence, and the open design
question a spike would resolve. None is scheduled; none is a defect in current
behavior. Two prior UX spikes already concluded (ADR-0005 unified clock,
ADR-0006 paired-decision classifier), and the obvious footguns are already
polished to a high bar — see "Already handled" below — so this list is the
next tier down.

## Ranking summary

| # | Wart | Value | Kind |
|---|------|-------|------|
| 1 | `RetryPolicy` cannot carry `timeout` or hooks | High | Design question |
| 2 | No shortcut for the most common tweak (attempt count) | Medium | Sugar |
| 3 | Closure arity differs silently across entry points | Medium | Signposting |
| 4 | Async `VirtualClock` needs `&`, sync does not | Low | Test ergonomics |
| 5 | Builder type-name sprawl | None (triaged) | Known, deferred |

## 1. `RetryPolicy` cannot carry `timeout` or hooks

`src/policy/mod.rs`. `RetryPolicy<S, W, C>` captures stop, wait, and classifier
only. SPEC 5.x, SPEC line 974 ("Hooks are configured on execution builders, not
on `RetryPolicy`"), and SPEC 11.4 (`.timeout` on the execution builder) make
`timeout` and the three hooks (`before_attempt`/`after_attempt`/`on_exit`)
per-call-site concerns by design.

The friction: the crate's headline pitch for `RetryPolicy` is "capture your
retry rules once, share across call sites — no duplicated builder chains"
(`src/lib.rs:11-12`). But a shared wall-clock deadline and a shared
observability hook (metrics, structured logging) are exactly the cross-cutting
concerns that N call sites repeat. The policy removes the *strategy*
duplication while leaving the *cross-cutting* duplication — often the part that
hurts most at scale.

`timeout` is the sharper case. It is a retry *rule* (it bounds the total budget
and clamps each inter-attempt sleep to the remaining budget, SPEC 11.4.2), not
an environment concern like `clock`. Its sleep-clamping behavior cannot be
reached through `stop::elapsed`, so a reusable policy literally cannot express
"all these operations share a 30s wall-clock budget."

Open design question for a spike: should `timeout` (and optionally the hooks)
move onto `RetryPolicy`? Constraints to weigh — hooks carry generic type-state
slots (`BA`/`AA`/`OX`) that the policy currently does not model; adding them
widens `RetryPolicy`'s arity and its boxing story (`.boxed`/`.boxed_local`).
`timeout` is a single `Duration` field and is cheaper to add than hooks. A
partial answer (timeout on the policy, hooks left on the builder) may be the
right cut.

## 2. No shortcut for the most common tweak (attempt count)

Changing the retry count from the default 3 requires importing the `stop`
module and replacing the whole stop strategy:
`use relentless::stop; …​.stop(stop::attempts(5))`. Compare backon's
`.with_max_times(5)`. The single most common customization carries the most
import ceremony.

Tension: the compositional stop model (`stop::attempts(5) | stop::elapsed(2s)`)
is deliberate and a convenience `.max_attempts(n)` introduces a second way to
set the count. The question is whether the common case should subsidize the
general one — a `.max_attempts` that ORs/replaces just the attempt bound, or a
documented one-liner, versus holding the line on composition.

## 3. Closure arity differs silently across entry points

`src/engine/op.rs`. `retry(|state| …)` and `policy.retry(|state| …)` take a
stateful `FnMut(RetryState) -> O`; the extension form `(|| …).retry()` takes a
stateless `FnMut() -> O` (adapted via `StatelessOp`, which discards the state).
Refactoring between the free-function/policy forms and the ext form silently
changes the closure's shape, and nothing at the call site signposts it.

This is inherent — an extension method cannot thread an argument into its
receiver — so the fix is signposting, not unification: clearer docs at the
switch point, or a naming cue that the ext form is stateless.

## 4. Async `VirtualClock` needs `&`, sync does not

`src/clock.rs:105` — `AsyncClock` is implemented for `&VirtualClock`, not
`VirtualClock`. So a sync test writes `.clock(VirtualClock::new())` while an
async test writes `.clock(&clock)`. A user who forgets the `&` hits a
`SystemClock: AsyncClock` / `VirtualClock: AsyncClock` bound error and may not
realize the fix is a single reference. The `on_unimplemented` note
(`src/clock.rs:89-95`) points at `.clock(...)` but does not mention the `&`.
Cheap fix: extend the note, or add worked async-test examples.

## 5. Builder type-name sprawl

Already triaged in ADR-0002 (deferred; judged a lateral move, not a net
improvement). Listed here only so a future reader does not re-discover it as
novel. No action pending a rustdoc capability change or a broader breaking
release.

## Already handled (bar reference)

These were checked and found already polished, confirming the quality bar this
list sits beneath:

- The async engine has no ambient default clock, and the default type parameter
  `SystemClock` cannot drive it — but `AsyncClock`'s `diagnostic::on_unimplemented`
  (`src/clock.rs:89-95`) explains exactly how to supply one, so the deferred
  bound error reads well.
- `.timeout`'s overshoot semantics (one final attempt can push total time past
  the deadline) are documented at `src/lib.rs:76-82` and SPEC 11.4.
- `stop::elapsed` vs `.timeout` overlap is disambiguated in SPEC 11.3–11.4.
