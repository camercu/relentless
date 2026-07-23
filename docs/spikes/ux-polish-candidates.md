# UX polish candidates — findings

**Status:** living catalog. Records UX warts surfaced by a dogfooding pass over
the public surface (2026-07-23), ranked as candidates for future polish. Item 1a
(timeout on the policy) has since shipped; item 1b (hooks on the policy) and item
2 (an attempt-count shortcut) were worked through and declined; item 3 (closure
arity across entry points) was resolved with docs after a diagnostic fix was
prototyped and rejected. The rest remain
open — each names the friction, the
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
| 2 | No shortcut for the most common tweak (attempt count) | ~~Medium~~ | Declined (see below) |
| 3 | Closure arity differs across entry points | ~~Medium~~ | Resolved (docs) |
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

## 2. No shortcut for the most common tweak (attempt count) — declined (2026-07-23)

The friction: changing the retry count from the default 3 requires importing the
`stop` module and replacing the whole stop strategy:
`use relentless::stop; …​.stop(stop::attempts(5))`. Compare backon's
`.with_max_times(5)`. The single most common customization carries the most
import ceremony. The compositional stop model
(`stop::attempts(5) | stop::elapsed(2s)`) is deliberate, so any convenience
risks introducing a second way to set the count.

The candidate fix — a `.max_attempts(n)` builder method — was worked through and
**declined**. The cleanest form is an inherent method constrained to the default
stop type (`impl Retry<F, C, StopAfterAttempts, …>`), mutating the field and
returning `Self` so the type stays stable. It looked attractive: definitionally
identical to `.stop(stop::attempts(n))`, so not a *divergent* second way. But a
red-team pass found the shape does not hold up:

- **It misses the highest-value entry point.** `RetryPolicy::retry` borrows its
  parts and returns `Retry<F, &C, &S, &W, …>` (`src/policy/mod.rs:195`), so a
  default policy's stop type is `&StopAfterAttempts`, not `StopAfterAttempts`.
  The constrained impl does not match the reference, so
  `policy.retry(op).max_attempts(5)` would not compile — and "reuse a shared
  policy, bump attempts for one call" is precisely the case the sugar was for.
  The user's only fix there is `.stop(stop::attempts(5))`, the exact ceremony we
  set out to remove. Making it work needs a *type-changing* impl
  (`&StopAfterAttempts` → `StopAfterAttempts`), which forfeits the type-stable
  `-> Self` property that motivated the method.
- **Compose-vs-replace trap.** `.timeout` composes into the effective stop (ORs
  a deadline, SPEC 11.4.1); `.max_attempts` would *overwrite* the stop value.
  Same surface syntax, opposite semantics, so a later `.stop(...)` silently
  discards a preceding `.max_attempts(n)` with no diagnostic. `.timeout`'s field
  pattern is a misleading analogy, not a precedent.
- **Silent re-override.** `.stop(stop::attempts(3)).max_attempts(5)` would
  compile and silently move 3 → 5 — two spellings for one value in one chain.
- **Duplication tax for zero capability.** Uniform coverage needs the constrained
  impl on `Retry`, `AsyncRetry`, and `RetryPolicy` (plus a fourth for the
  policy-borrow case) — three-to-four near-identical blocks for sugar that adds
  no capability the composition path lacks. Against maintenance-first
  (remove more than add, single way to do things), a losing trade.

Resolution: **hold the line on composition; make the existing one-liner
discoverable** rather than adding surface. `.stop(stop::attempts(n))` already
works identically on every entry point, including the policy borrow, and keeps
composition the single mechanism. If the ergonomic gap is ever felt sharply
enough to reopen, the decision is purely "is backon-style sugar worth the
redundancy," made knowing the policy-borrow hole and the compose-vs-replace
trap — not a fresh design question. (Naming and off-by-one were *not* the
blockers: `max_attempts` correctly mirrors `stop::attempts` and
`DEFAULT_MAX_ATTEMPTS`, counting total attempts, not retries.)

Adjacent option, also declined: re-export `attempts()` from the `prelude`. It
removes only the import line, not the `.stop(...)` wrapper; unqualified
generic-named constructors (`elapsed`, `never`) hurt readability and risk
collisions; and it overturns the prelude's documented exclusion of strategy
constructors (`src/lib.rs:347`).

## 3. Closure arity differs across entry points — resolved as docs (2026-07-23)

`src/engine/op.rs`. `retry(|state| …)` and `policy.retry(|state| …)` take a
stateful `FnMut(RetryState) -> O`; the extension form `(|| …).retry()` takes a
stateless `FnMut() -> O` (adapted via `StatelessOp`, which discards the state).
Refactoring between the free-function/policy forms and the ext form changes the
closure's shape, and nothing at the call site signposted the sibling spelling.

Scoped down on inspection: a **learnability** wart, not a defect. There is no
semantic divergence — `retry(|_| f())` and `(|| f()).retry()` drive the same
engine to the same result and attempt count — and the compiler already rejects a
wrong-arity closure. Only *which* error, and whether it names the sibling form,
was ever at issue.

A compiler-diagnostic fix was **prototyped and rejected**: route `retry`'s bound
through `F: RetryOp<Output = O>` and hang a `#[diagnostic::on_unimplemented]` on
`RetryOp`/`RetryExt` naming the other entry point. It fails on two counts, both
verified by compiling the cases:

- **The note never fires for the target case.** A 0-arg closure still
  structurally matches the `FnMut(RetryState) -> O` blanket impl, so rustc
  selects that impl and reports its *nested* arity obligation (E0593, "closure
  takes 0 arguments, expected 1") — the same error class as today. A trait-level
  `on_unimplemented` only surfaces for a `Self` matching *no* impl (e.g.
  `retry(5i32)`), which a closure never is. The ext direction fails through
  method resolution (E0599), which `on_unimplemented` also does not reach.
- **The bound change regresses inference.** `FnMut(RetryState) -> O` pins the
  closure's argument structurally at the signature, so
  `retry(|state| state.attempt …)` infers `state: RetryState`.
  `RetryOp<Output = O>` determines the argument only indirectly via blanket-impl
  selection, so a body that *uses* `state` hits E0282 ("type annotations
  needed") — breaking the crate's own headline doctest (`src/lib.rs:42`) and the
  stateful tests. The change degrades the very entry point it touches.

True unification is separately **coherence-blocked** (confirmed E0119): a second
direct blanket `impl RetryOp for FnMut() -> O` conflicts with the
`FnMut(RetryState)` one — exactly why `StatelessOp` is a newtype. No stable path
merges the two spellings.

Resolution: **docs-only signposting**, shipped 2026-07-23. The equivalence
`(|| op()).retry()` ≡ `retry(|_| op())` now sits on `RetryExt::retry` and
`AsyncRetryExt::retry_async` (the stateless side, whose E0599 was the only
genuinely cryptic message), and the free `retry` cross-links the ext form for
no-arg operations. The equivalence is locked by an executable test
(`tests/ext.rs::free_fn_and_extension_forms_agree_when_state_is_ignored`), which
asserts both forms agree on result and attempt count, so the documented claim
cannot silently drift.

Deferred harder option, recorded only for a future breaking release: **delete
the ext form** (one shape, `retry(|_| …)`; drop `RetryExt`/`AsyncRetryExt`/
`StatelessOp` and half of `op.rs`). The purest maintenance-first move (one way
to do things) and it needs no bound change, but it forfeits the fluent
`op.retry()` style and the `|_|`-free ergonomics for a wart the compiler already
catches — not worth scheduling on its own.

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
