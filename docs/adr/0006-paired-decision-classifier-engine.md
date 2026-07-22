# 6. Re-found the engine on a paired-decision outcome classifier

Date: 2026-07-21

## Status

Proposed — design settled empirically. Naming reviewed 2026-07-21: method
name **`.decide()`** and the two-way type **`Decision`** are settled
(concurred); the abort-capable type name **`Verdict` is provisional** and the
`after_attempt` timing choice is still open (see Open questions).

## Context

The engine's boolean `Predicate` cannot express workflows where retry
behavior is independent of `Result<T, E>` semantics — fuzzing (an `Err` can
be the sought success), polling non-`Result` enums, search. The in-tree
symptom is the inverted-polling wart (lib.rs:190-213): probe-until-failure
must report its found success as `RetryError::Rejected`. The redesign brief
settled the semantics (three-way Return/Retry/Abort decision; classifier
consumes the outcome by value with no `Clone` bound; `Retry` carries the
outcome back; `call()` stays `Result`-shaped; today's API remains the
`Result` instantiation of the general engine).

The open question — which concrete Rust representation survives type
inference — was resolved by a 13-spike tournament
(`.spike-workspace/LEDGER.md`). Its central finding: any classifier closure
returning a three-parameter `Decision<R, A, O>` with no `Abort` arm leaves
`A` unconstrained (E0282), and every routing-around fix (smart constructors,
wrapper types, two methods, builder type-state) taxes the common case. The
winning spike (`J1-paired-decision`, all 11 harness scenarios byte-identical,
misuse errors captured) removes the problem structurally instead.

## Decision

Adopt the J1 composite design:

1. **Paired decision enums, one unifying trait.** The no-abort currency has
   no abort type parameter, so the tournament's E0282 cannot exist; choosing
   the abort-capable currency implies an `Abort(a)` arm, which pins `A`:

   ```rust
   pub enum Decision<R, O>   { Return(R), Retry(O) }            // common case
   pub enum Verdict<R, A, O> { Return(R), Retry(O), Abort(A) }  // abort-capable
   pub trait IntoDecision<O> { /* sealed; unifies both under one method */ }
   ```

   Raw variants everywhere; no smart constructors; upgrading a classifier to
   abort-capable is one type rename plus one arm. One builder method accepts
   both currencies.

2. **Op-anchored classifier bounds.** `.classify`/`.when`/`.until` on the
   op-first builders carry `F: FnMut(RetryState) -> O` themselves rather than
   deferring all bounds to `.call()`. Closure parameters then infer from the
   op's output: inline classifiers need zero annotations (verified through
   `Fut::Output` on the async builder at equal quality), and wrong-closure
   misuse errors fire at the classifier method, the site of the mistake.
   Policy-first construction (`RetryPolicy::new().classify(…)`) has no op to
   anchor and keeps deferred bounds; closures there need one parameter
   annotation, matching today's cost.

3. **`Outcome` trait for owned domain types** (from spike G3): associated
   `Return`/`Abort` types plus a blanket `impl<T, E> Outcome for Result<T, E>`
   give zero-call-site classification for types the user owns; the orphan rule
   makes `.classify(closure)` the escape hatch for custom `Result` semantics.

4. **Error and observation layer.** `RetryError<A, O>` with
   `Aborted { last: A }` / `Exhausted { last: O }`; Result-shaped helpers
   (`last_error`, `into_last_error`, `Display`/`Error` impls) survive via a
   specialized impl on `RetryError<E, Result<T, E>>`. `StopReason` becomes
   the terminal-verdict discriminant (`Return`→`Succeeded`,
   `Abort`→`Aborted` — renamed from `Rejected` — stop-during-`Retry`→
   `Exhausted`), deleting the engine's `TerminalOutcomeKind` +
   `outcome.is_ok()` double discrimination. `RetryStats` and `RetryState`
   are already outcome-free and unchanged.

5. **Exit view replaces `ExitState.outcome`.** The by-value classifier
   consumes the outcome, so no `&O` survives a `Return`/`Abort` exit — the
   brief's original "make `outcome: &O` generic" plan is unimplementable.
   Instead the exit hook receives a borrowed view of exactly what the caller
   will receive:

   ```rust
   pub enum Exit<'a, R, A, O> { Returned(&'a R), Aborted(&'a A), Exhausted(&'a O) }
   ```

   with `stop_reason()` derived from its discriminant (single source of
   truth).

## Consequences

- The inverted-polling wart disappears: probe-until-failure returns its found
  value through `Ok`, fuzzing aborts carry projected domain errors, and
  non-`Result` outcome types work directly.
- Default-path call sites (`retry(op).stop_attempts(3).call()?`) are
  unchanged; `.when`/`.until` keep bare-`E` abort payloads. Breaking changes
  are spelling-level (pre-1.0): `RetryError<T, E>` → `RetryError<A, O>`,
  `Rejected` → `Aborted`, `StopReason::Rejected` → `Aborted`, hook state
  types re-shaped as above.
- Builder arity is unchanged (classifier folds into the existing slot); the
  design is `core`-only and preserves the `no_std` + alloc-free story.
- Known residuals, accepted: two decision types to learn (the irreducible
  disambiguator, placed in the type name users already write); E0282 remains
  reachable only by returning `Verdict` with no `Abort` arm; standalone
  closure bindings still need a parameter annotation; policy-first loses
  op-anchored inference.
- Evidence base: `.spike-workspace/J1-paired-decision/` (17 green tests,
  byte-identical 11-scenario harness, six captured misuse probes,
  `ERGONOMICS.md`); history in `.spike-workspace/LEDGER.md`.
- The parity suite (`tests/parity.rs`) must grow scenarios for classifier
  behavior in both engines before the port is considered done.

## Open questions (resolve before implementation)

1. **Method name — SETTLED `.decide(c)`** (concurred 2026-07-21). Reads as
   plain language, imperative like the other builder verbs
   (`retry`/`stop_attempts`/`after_attempt`), and the verb agrees with its
   `Decision` return type; `classify` named the mechanism, not the user's
   intent, and read as a different register beside `.when`/`.until`. The
   engine-facing trait (never named by users; visible in some diagnostics)
   follows: `Decide<O>`.
2. **Type names — `Decision<R, O>` SETTLED, `Verdict<R, A, O>` PROVISIONAL.**
   `Decision` is the return of `.decide()`, plain and clean per-arm — keep.
   `Verdict` reads well per-arm but nothing in the word *says* abort, so it
   fails the discovery test; it is the one name still carrying arbitrariness
   and is the focus of any further naming discussion. Alternatives weighed:
   `AbortableDecision` (self-describing but stutters at every arm —
   `AbortableDecision::Retry`), `Ruling` (same weakness as `Verdict`),
   `Decision3`/numeric (rejected round F). If `Verdict` stands, lean on
   discovery aids: `Decision` docs open with "need to abort? return a
   `Verdict`", `#[doc(alias = "abort")]` on `Verdict`, and the E0599/E0308
   misuse errors already name both types.
3. **`after_attempt` timing** (forced by the by-value classifier):
   (a) fire before classification with `&O` for every attempt, dropping the
   `next_delay` field (the value reappears as the next attempt's
   `RetryState.previous_delay`) — recommended; or (b) fire after
   classification on the retry path only, keeping `next_delay`, with
   terminal attempts covered by the exit hook.
4. **`StopReason::Succeeded` vs `Returned`.** `Succeeded` stays accurate
   (the loop achieved its goal) and minimizes churn; `Returned` would mirror
   the verdict vocabulary exactly. Current lean: keep `Succeeded`.
