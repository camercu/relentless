# Overview

`tenacious` is a Rust library for retrying fallible operations and polling for
conditions.

The spec is in [SPEC.md](/docs/SPEC.md).

For developer workflow and tool setup, follow
[CONTRIBUTING.md](/CONTRIBUTING.md). Treat it as the source of truth for the
pinned shell environment, setup script, Git hooks, and `just`-based CI
workflow.

## Repository context map

Use this section when you need fast architectural context before reviewing or
changing code.

- Start behavior checks from [docs/SPEC.md](/docs/SPEC.md), then read
  [src/lib.rs](/src/lib.rs) for the public surface, then
  [src/policy/mod.rs](/src/policy/mod.rs) for policy construction.
- Treat [src/policy/execution/common.rs](/src/policy/execution/common.rs) as
  the semantic center of the crate. It owns retry-loop transitions, hook
  timing, cancellation exits, and stats reasons. The sync and async execution
  files mostly wrap this shared state machine.
- Expect the API to be duplicated across four files:
  [src/policy/execution/sync_exec.rs](/src/policy/execution/sync_exec.rs),
  [src/policy/execution/async_exec.rs](/src/policy/execution/async_exec.rs),
  [src/policy/ext/sync_builder.rs](/src/policy/ext/sync_builder.rs), and
  [src/policy/ext/async_builder.rs](/src/policy/ext/async_builder.rs). When
  you change hooks, cancellation, stats, or type-state ergonomics, audit all
  four paths for drift.
- Verify feature claims explicitly. The fastest useful checks are `cargo test`,
  `cargo test --no-default-features --lib`, `cargo build --target
  thumbv7m-none-eabi --no-default-features`, and `cargo check --target
  wasm32-unknown-unknown --no-default-features --features
  alloc,gloo-timers-sleep`.
- Do not assume `cargo test --all-features` is a healthy host-side lane. As of
  March 7, 2026, enabling `embassy-sleep` in host tests fails to link because
  `embassy-time` expects a time driver symbol. If you touch feature gating or
  CI coverage, review [Cargo.toml](/Cargo.toml) and [justfile](/justfile)
  together.

## Coding Rules

- Never use magic numbers whose meaning isn't obvious from context. Extract
  them into named constants. Values that carry domain meaning (thresholds,
  limits, configuration) must always be constants.
- In tests, values that are genuinely arbitrary (any valid value would work)
  should use a small set of shared `ARBITRARY_*` constants to signal intent,
  e.g. `const ARBITRARY_DURATION: Duration = Duration::from_millis(10)`.
  Do not create per-test-site constants for values that have no semantic
  significance — this obscures rather than clarifies. Standard values like
  `Duration::ZERO`, `true`/`false`, and contextually obvious literals (e.g.
  `Ok(())`, `Err("msg")`) may be used inline.

## Execution guardrails

- Use Conventional Commits with required scopes that describe functionality or
  domain (for example: `serialization`, `policy`, `security`, etc.), never
  phase labels. Split unrelated work into separate atomic commits.
- When other agents are working concurrently, prefer edits in disjoint files or
  code areas. Never revert changes you did not make.
- Before committing, run `git status --short` and include only intended files
  associated with a single logical change.
- Always commit atomically: `git add <explicit-files> && git commit -m "<message>" -- <explicit-files>`
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
