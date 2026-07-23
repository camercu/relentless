# Overview

`relentless` is a Rust library for retrying fallible operations and polling for
conditions.

The spec is in [SPEC.md](/docs/SPEC.md).

For developer workflow and tool setup, follow
[CONTRIBUTING.md](/CONTRIBUTING.md). Treat it as the source of truth for the
pinned shell environment, setup script, Git hooks, and `just`-based CI workflow.

## Repository context map

Use this section when you need fast architectural context before reviewing or
changing code.

- Start behavior checks from [docs/SPEC.md](/docs/SPEC.md), then read
  [src/lib.rs](/src/lib.rs) for the public surface, then
  [src/decision.rs](/src/decision.rs) for the classifier model
  (`Decide`/`Decision`/`Verdict`/`Outcome`) and
  [src/policy/mod.rs](/src/policy/mod.rs) for the reusable `RetryPolicy` config.
  The engine is classifier-driven (ADR-6): each outcome is consumed by value by
  a `Decide` classifier, not tested by a boolean predicate.
- Treat [src/engine/step.rs](/src/engine/step.rs) `step` as the semantic center
  of the crate. It is the shared decision core: given a completed attempt's
  outcome it fires `after_attempt`, classifies, and returns either
  `Step::Done` (fires `on_exit`, terminates) or `Step::Continue` (next backoff).
  All classify → stop → wait → hook → exit transitions and stats accounting live
  here once, so this is where correctness changes belong.
- Two thin drivers wrap that core and differ only in how they invoke the
  operation and how they sleep: [src/engine/mod.rs](/src/engine/mod.rs)
  `Retry::run` (sync, blocks on `clock.wait`; entry points `retry`, `RetryExt`)
  and [src/engine/async_engine.rs](/src/engine/async_engine.rs) `AsyncRun::poll`
  (async `Phase` state machine, polls a `wait_async` future; `retry_async`,
  `AsyncRetryExt`). When you touch driver-only concerns — the sleep, the poll
  machine, `before_attempt` timing — edit both in lockstep; core changes touch
  `step` alone.
- The core and drivers share these building blocks:
  [src/engine/state.rs](/src/engine/state.rs) (`AttemptState`, `Exit`,
  `StopReason`), [src/engine/hooks.rs](/src/engine/hooks.rs) (`HookChain`
  type-state), [src/engine/op.rs](/src/engine/op.rs)
  (`RetryOp`/`AsyncRetryOp`), [src/engine/error.rs](/src/engine/error.rs)
  (`RetryError`), and [src/engine/stats.rs](/src/engine/stats.rs)
  (`RetryStats`); the outcome-agnostic `Stop`/`Wait`/`Clock`/`RetryState`
  infrastructure is reused unchanged.
- `step` has direct unit tests; behavioral drift between the sync and async
  drivers is caught by the differential suite in
  [tests/parity.rs](/tests/parity.rs) (needs `alloc`, so it runs in any default
  or `alloc`-enabled run, including bare `just test`). When you change hooks,
  cancellation, stats, or type-state ergonomics, extend a parity scenario for
  the new behavior and audit the builder methods in both driver files for
  surface/docs drift the suite cannot see.
- Verify feature claims explicitly. Prefer the repo's `just` targets over ad hoc
  `cargo` commands. The fastest useful checks are `just test`,
  `just test-no-default`, `just check-no-std`, and `just check-wasm`.

## Iteration workflow

Use this loop for every feature-development iteration.

- Start with the narrowest relevant validation command for the area you are
  changing: `just test`, `just test-no-default`, `just check-no-std`,
  `just check-wasm`, `just doc`, or `just bench-no-run`.
- After each substantive code change, run the smallest `just` command that can
  catch regressions in that area before moving on.
- Before every commit, run `just fmt` then `just pre-commit`. The pre-commit
  hook only checks formatting; `just fmt` fixes it, avoiding a failed-commit
  retry loop.
- Before handing work back for review or declaring the feature complete, run
  `just pre-push`.
- Run `just ci` before merge or when you need the full pinned-toolchain gate.
- Treat `just ci-stable` as advisory only. It is useful for detecting
  newest-stable drift, but it does not replace `just ci`.

## Coding Rules

- Never use magic numbers whose meaning isn't obvious from context. Extract them
  into named constants. Values that carry domain meaning (thresholds, limits,
  configuration) must always be constants.
- In tests, values that are genuinely arbitrary (any valid value would work)
  should use a small set of shared `ARBITRARY_*` constants to signal intent,
  e.g. `const ARBITRARY_DURATION: Duration = Duration::from_millis(10)`. Do not
  create per-test-site constants for values that have no semantic significance —
  this obscures rather than clarifies. Standard values like `Duration::ZERO`,
  `true`/`false`, and contextually obvious literals (e.g. `Ok(())`,
  `Err("msg")`) may be used inline.

## Execution guardrails

- When other agents are working concurrently, prefer edits in disjoint files or
  code areas. Never revert changes you did not make.
- Before committing, run `git status --short` and include only intended files
  associated with a single logical change.
- Treat [`docs/SPEC.md`](/docs/SPEC.md) as authoritative by default.
- If an implementation improvement conflicts with spec behavior, stop and
  present the exact conflict, implications, and recommendation before changing
  behavior.
- Keep tests deterministic and fast. Randomized/property-style tests must
  support an environment-injected seed and print the seed on failures.
- Avoid timing-flaky tests and non-essential test/example cruft.

## Code review rubric

- Prioritize findings over summaries: list bugs, behavioral regressions,
  security risks, performance issues, and missing tests first.
- Order findings by severity and include precise file/line references.
- Evaluate design quality: complexity, cohesion, modularity, coupling, and
  abstraction boundaries.
- Look for opportunities to simplify code and remove non-value-add
  implementation and test/example cruft.
- Audit public API surface area and flag items that should be private or
  crate-private.
- Confirm tests are deterministic, fast, intent-revealing, and maintainable.
- Identify significant coverage gaps, especially around edge cases and failure
  paths.
