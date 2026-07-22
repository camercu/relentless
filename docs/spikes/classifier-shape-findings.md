# Spike: classifier shape — findings

## Question

Which concrete Rust representation of the three-way outcome classifier
(`Return`/`Retry`/`Abort`, replacing the boolean `Predicate`) survives
real-world type inference and type-state builder integration? Enumerated
candidates: (1) free-generic enum `Decision<R,A,O>` + closure blanket impl;
(2) assoc-type `Classify` trait + closure adapters; (3) pass-through
`Decision<O>` fallback. Clean-room agents may reveal unenumerated regions.

Settled design context (fixed, not relitigated here): grill session decisions
D1–D9 in the session brief — engine re-founded on general classifier, familiar
surface, op returns arbitrary `O`, `call()` stays `Result`-shaped with
`RetryError::{Aborted, Exhausted}`, predicate sugar kept, one type-state slot.

## Constraints (requirements, not solutions)

- Ordinary retry code keeps today's feel; S1–S3 harness scenarios must need
  zero annotations beyond current doctest level.
- Classifier consumes outcome by value, no `Clone` bound; `Retry` carries the
  outcome back to the engine (hooks, `Exhausted{last}`, feedback waits).
- Stable Rust, no unsafe, must not break `no_std`; sugar layer (`.when`/
  `.until`) implementable over the representation; async engine
  (pin-projected state machine) must tolerate the generics.

## Contract

- Comparability: byte-identical scenario stdout (S1–S9) + mandatory
  ERGONOMICS.md per spike — spec in spike workspace `HARNESS.md`
  (scratchpad `spikes/` dir, git-tracked there; throwaway).
- Verify: `nix-shell /Users/cameron/repos/relentless/shell.nix --run
  'cargo test --manifest-path spikes/<id>/Cargo.toml'` (toolchain 1.94.1).
- Honesty mandate in every brief; misuse diagnostics captured verbatim (S10).

## Run state (checkpoint)

- Phase: **COMPLETE (2026-07-21).** Winner J1; concluding ADR-0006 drafted.
  Rounds A/B/D/E (breadth) → F1–F4 (refine) → G1–G4 (Outcome trait) →
  H (obsolete) → J1 (winner) → K (observation-layer analysis). History below
  retained as written; live verdict + recommendation at end of doc.
- Tier: heavy
- Workspace: `/Users/cameron/repos/relentless/.spike-workspace/` (durable, on
  real disk, git-init'd, excluded via `.git/info/exclude`). NOTE: ephemeral
  `/private/tmp/.../scratchpad` was WIPED once by a process restart — never put
  spike artifacts there.
- Round-1 all built (some cut off by session limit mid-fix; artifacts salvaged
  from disk and committed). Verified by orchestrator (reviewer≠author):
  - **E-cleanroom**: builds + runs; all 11 scenarios BYTE-IDENTICAL incl. S9.
  - **C-passthrough**: runs all 11 (reported round-0).
  - **B-assoc-trait / D-cleanroom**: E0282 as committed (abort-less closure).
  - **A-free-generic**: mechanical errors (missing PartialEq derive, async
    helper) — cut off before inference proof.
- Live finalists (proposed): **B** (transform representative),
  **E** (pass-through representative).
- Proposed cuts (GATED, awaiting user): **A** (dominated by B/D), **C**
  (dominated by E). **D** folds into B as convergence evidence.
- Next action: user approves cuts + refine round → dispatch 2 informed spikes
  (escape hatches F1, F2 below) 2-at-a-time per usage-limit constraint.

## The bracketed axis (round-1 conclusion)

Independent convergence, two regions, each with one fatal-ish flaw:

- **Transform `Decision<R,A,O>`** (A directed-free-generic, B directed-assoc-
  type, D clean-room — 3 independent agents landed here). Preserves D1/D5
  NATIVELY: today's `call() -> Result<T, RetryError<E, Result<T,E>>>` is the
  exact default instantiation; abort carries bare `E`. FLAW: **E0282** whenever
  a classifier closure omits an `Abort` arm — `A` is unconstrained. Reproduced
  in B (`tests/scenarios.rs:76`) and D (`src/scenarios.rs:316`), identical
  message `cannot infer type of the type parameter A`. Assoc-type indirection
  (B) and `classify_fn` adapter did NOT rescue it; clean-room framing (D) hit
  it too. Mirrors the crate's own documented E0282 note (predicate/mod.rs
  :12-18). Cost = mandatory type annotation on abort-less classifiers (the
  common case) — precisely the ergonomic axis being scored.
- **Pass-through `Decision<O>`** (C directed, E clean-room — 2 independent
  agents). No unconstrained type → no E0282; S9 green in E. FLAW: cannot
  preserve D1/D5 for free (WART-F: Return/Abort can't carry a type ≠ O). Two
  independent ways to pay the SAME tax:
  - **E** keeps ONE builder (`RetryBuilder<F,C,H>`, arity 3) but the common
    path returns nested `Result<Result<T,E>, RetryError<Result<T,E>>>` — user
    double-matches `Ok(Ok(v))` (E `scenarios.rs:24`), abort is
    `Aborted{last: Err(e)}` not bare `e`. Breaks `retry.call()?  -> T`.
  - **C** preserves D5 payloads by FORKING into a second builder +
    `PredicateError<T,E>` — dual maintenance, net surface increase.

Neither enumerated candidate wins clean. Decision-ready for a REFINE round
targeting escape hatches that keep transform's D1/D5 ergonomics WITHOUT the
E0282 wall:

- **F1 — two-tier classify, Abort defaults to Never.** Common
  `.classify(Fn(O) -> Decision2<R, O>)` carries only Return|Retry (one free `R`,
  pinned by the `Return(r)` arm — no unconstrained type, no E0282). Rare 3-way
  via opt-in `.classify_aborting(Fn(O) -> Decision<R, A, O>)`. KEY fit: today's
  default + `.until` polling are Return|Retry ONLY (default never aborts; abort
  arises only from `.when`/explicit) — so the COMMON path never introduces the
  third type. Likely yields `Result<T, RetryError<…>>` with `?`→T intact.
- **F2 — decompose: `.classify(Fn(O) -> ControlFlow<R, O>)` + separate
  `.abort_when(Fn(&O) -> bool)`.** Abort becomes a boolean over `O`; no third
  payload type ever. Loses "abort carries transformed `A`" (abort payload = the
  outcome) — check whether anyone needs the transform.

## Round log (append)

- Round 1 built + salvaged + verified. Convergence: 3 agents→transform,
  2→pass-through. Both regions flawed; axis bracketed (see above). Gating cuts
  A, C; proposing refine round F1/F2.

## Round log

- Round 1 (breadth sweep): 3 directed space-exhaustion agents (one per
  enumerated candidate) + 2 pure clean-room agents (candidate list withheld —
  convergence signal). All parallel, mutually forbidden from reading each
  other's dirs and this findings doc.
  - INCIDENT: process restart wiped the ephemeral scratchpad workspace and
    killed A/B/D/E mid-build; C had already reported. Workspace relocated to
    repo-local `.spike-workspace/` (durable); briefs regenerated; A/B/D/E
    re-dispatched.

## Candidate reports (captured before elimination)

### C-passthrough — `Decision<O>` single-generic pass-through (DIRECTED)

Self-verdict: **dominated; recommend eliminate** (pending independent review).
All 11 scenarios byte-identical; 5 tests green; no unsafe; no_std-clean.

- Signatures: `enum Decision<O> { Return(O), Retry(O), Abort(O) }`;
  `enum RetryError<O> { Aborted{last:O}, Exhausted{last:O} }` (classify path) +
  a SEPARATE `PredicateError<T,E>{ Aborted{last:E}, Exhausted{last:Result} }`
  (sugar path); TWO builders (`PredicateBuilder`, `ClassifyBuilder`) because
  `call()` return types differ; engine generic over ONE type `O`.
- Key finding: inference safety real but NARROW (one `O`, nothing to
  under-constrain), and NOT worth it. Costs land on the COMMON path: dual
  builders + dual error types maintained forever (net surface INCREASE over
  today's single 10-param builder), ~15-LoC mapping layer with
  compiler-forced-unreachable arms (loses D5 elegance), and +11 lines of
  caller double-match across S4/S5/S6/S7/S9 to re-destructure a value the
  classifier already matched.
- New regression it introduces: WART-A lifetime — identity `Decision::Retry(o)`
  on a bare `&str` outcome fails "lifetime may not live long enough"; user
  forced to `&'static`/named lifetime. Transform shapes (R/A) avoid this.
- WART-B entry split: no single `retry()` door for both `Result` and
  non-`Result` ops; non-`Result` needs separate `classify(op,c)` — violates
  spirit of D1.
- WART-F (root): `Return`/`Abort` carry the same `O`; cannot express "return
  `T`, abort with `E`". Candidates 1/2/D/E all descend from wanting to fix this.
- S10 misuse diagnostic: E0308 "expected `Result`, found `PollState`" — clean
  but its suggested fix (wrap in `Result`) is wrong for user intent; cleanness
  is a symptom of the entry split.

Standing: viable **only as fallback if candidates 1 and 2 both fail** real
`.classify` inference. Artifact was lost in the restart (committed only to the
wiped scratchpad git); report above is the durable record. Re-materialize from
this report only if it survives to the finalist round.

## Eliminated (round 1 — user-approved; artifacts retained in workspace git)

- **A-free-generic** — ELIMINATED (dominated by B/D). Why lost: transform
  `Decision<R,A,O>` free-generic; failed on mechanical errors (missing
  PartialEq derive, async `block_on` helper) before reaching the inference
  proof, and its region's decisive lesson (E0282 on abort-less closures) is
  demonstrated more completely by B (assoc-type) and D (clean-room). What didn't
  work: free generics offer even LESS inference help than assoc-types, so same
  wall, worse. Carry forward: transform region = native D1/D5 but E0282 wall;
  fully represented by B.
- **C-passthrough** — ELIMINATED (dominated by E). Why lost: pass-through
  `Decision<O>` done with dual builders + dual error type (`PredicateError<T,E>`
  vs `RetryError<O>`) — net surface increase; E occupies the same region with a
  single `RetryBuilder<F,C,H>` (arity 3). What didn't work: forking to preserve
  D5 payloads trades type-nesting for dual maintenance. Carry forward (KEY
  LESSON): under pass-through, preserving D5's bare-`E` abort REQUIRES either a
  fork (C) or nested `Result<Result<..>,..>` (E) — WART-F is structural. This
  is the constraint the refine round must escape.

## Refine round (round 2) — informed + clean-room

Goal: dominate BOTH finalists — transform's native D1/D5 WITHOUT the E0282 wall.

- **F1-two-tier** (informed): common `.classify(Fn(O)->Decision2<R,O>)`
  Return|Retry only; opt-in `.classify_aborting(...)` for 3-way. Default +
  `.until` are Return|Retry-only, so common path never introduces the
  unconstrained abort type.
- **F2-decompose** (informed): `.classify(Fn(O)->ControlFlow<R,O>)` +
  `.abort_when(Fn(&O)->bool)`. Abort = boolean over O; no third payload type.
- **F3-cleanroom** (clean-room, unanchored): given requirements + the harder S9
  probe only, NOT the bracketed-axis findings. Independent convergence check on
  the escape-hatch space.

Live finalists carried into round 2: B (transform), E (pass-through), plus
whatever F1/F2/F3 produce.

### F1-two-tier — RESULT (informed) — strongest so far

Verified by orchestrator: 3 tests green, all 11 scenarios byte-identical, S9b
green. Artifact committed.

- Shape: `Decision2<R,O> { Return, Retry }` (common, NO abort → no unconstrained
  param) + opt-in `Decision<R,A,O> { Return, Abort, Retry }`. `Classify<O>`
  trait with assoc `type R; type A;`; two newtypes `Two<C>`/`Three<C>` because
  a single trait with both `Fn` blanket impls would overlap (coherence) → forces
  TWO methods `.classify` (Fn(O)->Decision2) and `.classify_aborting`
  (Fn(O)->Decision). `call() -> Result<C::R, RetryError<C::A, O>>`.
- **D1/D5 preserved NATIVELY**: default/when impl `Classify` with `R=T, A=E` →
  `Result<T, RetryError<E, Result<T,E>>>`; `Ok(v)` returned DIRECTLY (no
  `Ok(Ok(v))` nesting — beats E), bare-`E` abort (beats E), single error type
  (beats C's dual builder).
- **KEY: S9b common inline `.classify(|o| Return/Retry)` compiles with ZERO
  annotation** — the exact round-1 transform killer, defeated on the common
  path.
- Warts: (1) two-method split = headline teachability/surface cost, forced by
  coherence; (2) tier-2 `.classify_aborting` inline with no abort arm STILL hits
  E0282 (quarantined to opt-in; S5 fuzzing works via a named `fn` w/ explicit
  return type); (3) by-value lifetime paper cut — tier-1 inline over elided
  `&str` fails with a LIFETIME error (not E0282; `Retry(o)` re-emits borrow),
  needs `&'static`; `.when` (by-ref) immune; (4) default is "secretly
  three-way" (shares tier-2 surface since `.when` aborts); (5) dead `Aborted`
  variant when `A=Infallible` on tier-1.
- Verdict (F1's own, to be checked by independent review): dominates both B
  (fixes the common-path E0282) and E (no nesting, no dual builder) FOR THE
  COMMON CASE, if the project accepts a two-method surface + opt-in abort.

### F2-decompose — RESULT (informed), then ELIMINATED on values axis

- Shape: `.classify(Fn(O) -> ControlFlow<R,O>)` (Break=Return, Continue=Retry) +
  SEPARATE `.abort_when(Fn(&O) -> bool)`. `call() -> Result<C::Return,
  RetryError<O>>`. Kills E0282 (proven: abort-less `ControlFlow<R,O>` closure
  compiles; identical `Decision<R,A,O>` fails). Native `? -> T`. Builder arity
  4 (`F,C,A,H`). All 11 scenarios + S9b green.
- Distinguishing property: **abort is CARRIED, never PROJECTED** — abort value
  is always the whole outcome (`Aborted{last: O}`), no third payload type.
- Compiled abort-on-`Ok` comparison (examples/abort_on_ok.rs in both spikes):
  poll where `Ok(Corrupted)` is fatal → F1 prints `Aborted(Corrupted)` (projects
  a domain `PollError`), F2 prints `Aborted(Ok(Corrupted))` (carries raw
  outcome).
- **USER DECISION: projected abort REQUIRED.** → F2 ELIMINATED. Why lost:
  cannot project the abort value (carry-only). What didn't work: abort as a
  boolean axis is orthogonal + inference-clean but structurally cannot produce
  a value ≠ the outcome. Carry forward: the decomposition (abort as a separate
  predicate axis) is a clean OPTIONAL convenience — F1 could ALSO offer
  `.abort_when` sugar for the carry case atop `.classify_aborting` for the
  projected case. Open item for the real build, not a blocker.

## Terminology (ubiquitous language — settled with user)

- **Outcome** (`O`): what one attempt produces; Result-agnostic.
- **Classifier**: `Outcome -> Decision`; replaces the boolean predicate.
- **Decision**: Return(`R`) | Retry(`O`) | Abort(`A`).
- **Return value** (`R`), **Abort value** (`A`).
- **Carried** vs **Projected**: terminal value IS the outcome (carried) vs
  computed by the classifier, type independent of `O` (projected). "bare-`E`
  abort" = the default/`.when` sugar's built-in projection to `E`.
- **RetryError**: Aborted | Exhausted. **Predicate**: `.when`/`.until` sugar =
  degenerate classifier.

## Standing after user values input

- **Projected abort required** → shapes must support Abort carrying a value ≠
  outcome. Survivors: **F1** (two-tier, projected abort via `.classify_aborting`,
  common-path E0282-free) and **B** (transform, projected natively but
  common-path E0282 wall). F1 DOMINATES B. F2, E, C, A all out.
- F1 is the presumptive winner. Its one real wart = the two-method split
  (`.classify` return/retry-only vs `.classify_aborting` adding projected
  abort), forced by trait coherence + inference.
- Open: does **F3-cleanroom** (still building, optimizing for projected-capable
  + single-method + annotation-free) find a shape that delivers projected abort
  WITHOUT F1's two-method split? That is the last question before verdict.

### F3-cleanroom — RESULT (clean-room, fully unanchored) — CONVERGES on F1

Independently landed on the SAME necessary split as F1. 6 tests green (incl.
S9b), all 11 scenarios byte-identical, `#![forbid(unsafe_code)]`, core-only.

- Shape: `Decision<R, O, A = Infallible> { Return, Retry, Abort }` + `Classify<O>`
  trait with assoc `type Return; type Abort;` and TWO closure blanket impls:
  (A) `FnMut(O) -> Decision<R,O,Infallible>` — no-abort, Abort FIXED to
  Infallible; (B) `Aborting<F>` newtype wrapping `FnMut(O) -> Decision<R,O,A>`
  for genuine projected abort. `Builder<Op,O,C,H>` arity 4; `call() ->
  Result<C::Return, RetryError<O, C::Abort>>`.
- SAME split as F1, DIFFERENT encoding: F1 puts the opt-in in a second builder
  METHOD (`.classify_aborting`); F3 puts it in a call-site NEWTYPE
  (`.classify(Aborting(|o| ...))`). Both isolate "no-abort closure
  (Infallible-fixed, inference-clean)" from "abort closure (needs an explicit
  opt-in to pin `A`)".
- **PROVES the split is the floor (key negative result):** the `A = Infallible`
  DEFAULT TYPE PARAM is a RED HERRING — it does NOT fire when `A` is inferred
  through the call with no downstream pin; captured `error[E0282]: cannot infer
  type of the type parameter A`. What makes no-abort closures infer is the
  dedicated blanket impl that FIXES Abort=Infallible for bodies building only
  Return/Retry — NOT the default param. So "one Decision<R,A,O> + default A"
  (a tempting single-method unification) is disproven by construction.
- Convergence verdict: informed-on-F1 (F1) and fully-clean-room (F3) landed on
  the identical split from opposite starting points → strong evidence the
  return/retry-vs-abort split is NECESSARY, not an artifact. The only remaining
  design freedom is its ENCODING: second builder method (F1) vs call-site
  newtype (F3).
- Shared warts: opt-in marker for 3-way (method or wrapper); async needs a
  parallel classify surface (mirrors the crate's existing sync/async split);
  standalone borrowed-outcome closure needs a `fn` signature (lifetime, not
  E0282). F3 `Exhausted` carries whole `Result` (F1 same).

## Standing before F4

Split is the FLOOR (F1≈F3 converged independently; default-param unification
disproven). F4-refine (Fable, informed) is the last attempt to beat it — if it
too fails to unify, verdict is locked: adopt the split, choose encoding
(method vs newtype), write ADR.

### F4-refine — RESULT (Fable, informed) — single-method encoding, but RELOCATES the split into a newcomer trap

Claimed to "dominate F1": ONE `.classify` + ONE `Decision<R,A,O>` via smart
constructors `Decision::ret(r)`/`retry(o)` (fix `A=Infallible`) + raw
`Decision::abort(a)`/variants for projected abort. 6 tests green, 11 lines +
S11 projected-abort-on-Ok byte-identical. Orchestrator verified tests pass.

**Independent refutation (orchestrator, reviewer≠author) — the domination claim
does NOT hold:**
- Compiled the newcomer trap (`examples/newcomer_trap.rs`): a no-abort closure
  written with the OBVIOUS variant names `Decision::Return(v)`/`Decision::Retry(o)`
  → **`error[E0282]: cannot infer type of the type parameter A`** — the exact
  round-1 killer the redesign exists to eliminate. F4 compiles ONLY with the
  non-obvious `ret`/`retry` smart constructors.
- So F4 did NOT eliminate the split — it RELOCATED it from two named methods
  into a constructor-naming convention, and put the trap on the MOST NATURAL
  syntax. F4's own report rated this "tie/slight better"; the compiled trap
  shows it is a first-use faceplant → WORSE UX on the axis that matters.
- Contrast F1: its common-path `.classify` takes `Decision2<R,O>` which HAS NO
  Abort variant, so the trap is UNTYPEABLE — the two-enum "wart" is precisely
  what makes F1 trap-safe. F4 traded trap-safety for one fewer type.

Net: F4 is a THIRD independent encoding of the SAME necessary split (confirms
F3's floor), not a dissolution of it. Value: proves the single-enum unification
reintroduces the E0282 trap → the split's floor is now triangulated by 3 teams
(F1 informed, F3 clean-room, F4 Fable) from 3 directions.

## Interim verdict (round F — SUPERSEDED by rounds G/H/J below)

At the close of round F this doc recommended **E1 (F1) — two methods**,
believing the no-abort vs projected-abort split was structurally necessary and
that any single-method encoding either wrapped abort (F3) or reintroduced the
E0282 newcomer trap (F4). Rounds G–J disproved the "necessary two methods"
claim. Retained here for history; the live verdict is below.

- E1 (F1) — two methods. Trap-safe, cost 2 methods + 2 enums.
- E2 (F3) — one method + `Aborting(..)` newtype. Unusual wrapper idiom.
- E3 (F4) — one method + one enum + smart-ctor convention. E0282 trap on
  obvious variant names.

## Verdict (FINAL — tournament complete 2026-07-21)

Winner: **J1 — paired decision enums + op-anchored bounds**
(`.spike-workspace/J1-paired-decision/`). One classifier method, raw variants
everywhere, no smart constructors, no wrapper idiom, no two-method split.

The round-F premise ("the split is structurally necessary") was wrong: it
assumed a single three-parameter decision enum. J1 splits the enum, not the
method — the no-abort currency (`Decision<R, O>`) has no abort parameter, so
E0282 is *structurally impossible* rather than trap-avoided; the abort-capable
currency (`Verdict<R, A, O>`) pins `A` via its `Abort` arm. A sealed
`IntoDecision<O>` trait unifies both under one method. Orthogonally,
op-anchored classifier bounds remove even the closure-parameter annotation
that every earlier finalist paid, and move misuse errors to the call site.

Later rounds in full:

- **Round G** — `Outcome` trait (G2/G3): zero-call-site classification for
  owned domain types; orphan rule forces `.classify(closure)` for custom
  `Result`. Adopted as a complementary path, not the closure representation.
- **Round H** — builder-carries-`A` (`.abort_type::<A>()`): never built; J1
  delivers its promised wins (raw variants, inline 3-way, actionable errors)
  without a builder type-state parameter or ordering rule. Obsolete.
- **Round J** — J1: proved E0282 removable structurally. Winner.
- **Round K** (analysis) — observation-layer consequences: `StopReason`
  simplifies to the terminal-verdict discriminant (`Rejected`→`Aborted`);
  `ExitState.outcome: &O` is unimplementable under the by-value classifier and
  is replaced by a borrowed `Exit<'a, R, A, O>` view; `RetryStats`/
  `RetryState` unchanged.

## Finalists

- **J1** (winner) — paired decisions + op-anchored bounds.
- F4 (round-2/3 best single-enum) — dominated by J1 on every measured axis.
- G3 `Outcome` trait — orthogonal, adopted alongside J1.

## Recommendation

Adopt **J1 closure path + G3 `Outcome` trait + unchanged when/until/default
paths**. Concluding decision record:
[ADR-0006](../adr/0006-paired-decision-classifier-engine.md). Flagged open for
review before implementation: exact names (`.classify()` vs `.decide()`,
`Decision`, `Verdict`) and the `after_attempt` timing choice — see the ADR's
Open questions.
