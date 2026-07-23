# UX polish candidates — findings

**Status:** living catalog. Records UX warts surfaced by a dogfooding pass over
the public surface (2026-07-23), ranked as candidates for future polish. Item 1a
(timeout on the policy) has since shipped; item 1b (hooks on the policy) was
worked through and declined. The rest remain open — each names the friction, the
evidence, and the open design question a spike would resolve; none is scheduled;
none is a defect in current behavior. Two prior UX spikes already concluded
(ADR-0005 unified clock,
ADR-0006 paired-decision classifier), and the obvious footguns are already
polished to a high bar — see "Already handled" below — so this list is the
next tier down.

## Ranking summary

| # | Wart | Value | Kind |
|---|------|-------|------|
| 1a | `RetryPolicy` cannot carry `timeout` | ~~High~~ **DONE** | Resolved |
| 1b | `RetryPolicy` cannot carry hooks | Low | Declined (see below) |
| 2 | No shortcut for the most common tweak (attempt count) | Medium | Sugar |
| 3 | Closure arity differs silently across entry points | Medium | Signposting |
| 4 | Async `VirtualClock` needs `&`, sync does not | Low | Test ergonomics |
| 5 | Builder type-name sprawl | None (triaged) | Known, deferred |

## 1. `RetryPolicy` and cross-cutting concerns

The original wart bundled two problems under one coat. They split cleanly, and
the split changes the verdict: **timeout was the real gap and is now fixed;
hooks turn out to be the wrong thing to move.**

The shared friction that motivated both: the headline pitch for `RetryPolicy`
is "capture your retry rules once, share across call sites — no duplicated
builder chains" (`src/lib.rs`). A shared wall-clock deadline and a shared
observability hook are exactly the cross-cutting concerns N call sites repeat,
and the policy removed only the *strategy* duplication.

### 1a. Timeout on the policy — DONE (2026-07-23)

`timeout` was the sharper case: a retry *rule* (it bounds the budget and clamps
each inter-attempt sleep to the remaining budget, SPEC 11.4.2), not an
environment concern like `clock`, and its sleep-clamp behavior is *not*
expressible via `stop::elapsed` — so a reusable policy literally could not say
"all these operations share a 30s budget."

Implemented as a plain `Option<Duration>` field on `RetryPolicy` (not a type
parameter), a `.timeout()` policy method, and seeding through
`.retry`/`.retry_async`. A builder `.timeout()` **replaces** the seeded value
for that call (last-wins, not min). Cost was as predicted: no arity change, no
impact on `.boxed`/`Clone`, one-line public-API delta. SPEC 5.10 + 11.4;
tests in `tests/policy_sync.rs` and `tests/policy_async.rs`
(`policy_timeout_seeds_builder`, `policy_timeout_replaced_by_builder`, async
twins).

### 1b. Hooks on the policy — declined (updated perspective)

Initial notes floated "timeout on the policy, hooks left on the builder — a
partial answer may be the right cut." Working through the timeout change
sharpened that hedge into a firm **no** for hooks, for reasons that are
structural, not effort:

- **Arity.** Hooks are type-state slots (`BA`/`AA`/`OX`, `src/engine/hooks.rs`),
  not a plain field like timeout. Carrying them doubles `RetryPolicy<S, W, C>`
  to `<S, W, C, BA, AA, OX>`. Timeout dodged this precisely because it is a
  value, not a slot; hooks cannot.
- **Reuse across outcome types — the decisive one.** `after_attempt`/`on_exit`
  observe the outcome, so a hook closure references `O`. Storing it on the
  policy pins the policy to one outcome type, destroying the cross-`(T, E)`
  reuse that SPEC 5.9 and `.boxed`'s classifier-preservation deliberately
  protect. Timeout is outcome-agnostic; hooks are not. This is the crux: hooks
  on the policy fight the very property that makes a default-classifier policy
  worth sharing.
- **Boxing / `Clone`.** `.boxed()` erases stop and wait but keeps the
  classifier generic exactly to stay outcome-agnostic. Hook closures have no
  such erasure path and are not necessarily `Clone`.

So a naive "hooks on the policy" is a net loss. If shared observability ever
becomes a real, felt need (not a symmetry itch), it warrants its own spike on a
*different* mechanism — outcome-erased `dyn` hooks that keep the policy
outcome-agnostic — rather than lifting the current type-state hooks onto the
policy. Low priority until a concrete use case pushes on it.

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
