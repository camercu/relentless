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

## Run state (checkpoint — resume from here)

- Phase: converge → gating round-1 eliminations + refine-round proposal
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

## Finalists

(none yet)

## Recommendation

(pending)
