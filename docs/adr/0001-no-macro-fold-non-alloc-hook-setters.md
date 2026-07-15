# 1. Do not macro-fold the non-alloc hook setters

Date: 2026-06-21

## Status

Superseded by [ADR-0003](0003-ungate-multi-hook-from-alloc.md) — the non-alloc
single-slot hook setters this ADR kept longhand were deleted entirely when
multi-hook registration was un-gated from `alloc`.

## Context

The execution builders expose three hook setters — `before_attempt`,
`after_attempt`, `on_exit` — on both `SyncRetryExec` and `AsyncRetryExec`.
They exist in two feature-gated forms:

- **`alloc`** (multi-hook chaining, `chain_*`): generated for both builders by
  the `impl_alloc_hook_chain!` macro in `src/policy/mod.rs`. The impl-generics
  are uniform across the three setters (one `impl[Policy, BA, AA, OX, ...]`
  header), so the macro is clean.
- **non-`alloc`** (single-hook, `set_*`): written **longhand** in both
  `sync_exec.rs` and `async_exec.rs` — three separate `#[cfg(not(feature =
  "alloc"))]` impl blocks per file, each fixing a *different* hook slot to `()`
  to enforce the single-hook constraint, each carrying a `compile_fail` doctest
  that proves calling the setter twice fails to compile.

This asymmetry (alloc folded, non-alloc longhand) looks like an oversight and
invites a "why not fold the non-alloc setters too?" change. CLAUDE.md also flags
the four-file builder duplication as a drift hazard, which reinforces the
temptation. This ADR records why we evaluated that fold and declined it.

## Decision

Keep the non-`alloc` hook setters longhand. Do **not** introduce a sibling
`impl_base_hook_setters!` macro.

## Consequences

Rationale (why the fold is not worth it):

1. **The duplicated content is ~75% documentation, not logic.** Each block is
   roughly a doc comment + a `compile_fail` doctest + `#[must_use]` + a
   one-line body (`self.map_hooks(|h| h.set_*(hook))`). The only drift-prone
   logic is one trivial line per setter; documentation changes rarely.

2. **A macro endangers the `compile_fail` coverage.** Those doctests live on
   `#[cfg(not(feature = "alloc"))]` items, so rustdoc only extracts them under
   `--no-default-features` (that is how they pass today). Whether rustdoc
   extracts and runs `compile_fail` doctests from *macro-generated* doc comments
   is unproven in this crate — `impl_alloc_hook_chain!` carries only descriptive
   docs, no doctests. A fold could silently drop the single-hook-constraint
   coverage while the suite still reports green.

3. **The macro would read worse than the longhand.** Because each non-`alloc`
   setter fixes a different slot to `()`, the three impl headers are
   non-uniform; a macro must take three distinct (impl-generics, self-type,
   return-type) triples per invocation. Three explicit, individually-doctested
   impl blocks are easier for a new reader than that invocation.

Trade-off accepted: ~90 lines of (mostly documentation) duplication remain
across the two files. The drift risk is low because the bodies are trivial
one-liners.

Revisit if: someone demonstrates that macro-generated `compile_fail` doctests
are extracted and run under `--no-default-features` (diff doctest counts before
and after). If that holds, a sibling macro that hardcodes the invariant
doc/bound/body and parameterizes only the three type-triples becomes viable.

The folds that *were* clean have already been done: `impl_alloc_hook_chain!`
(uniform alloc variants), the shared `execute_*_loop` engine in `common.rs`, the
extracted clamp helper, and the `random_duration_in` jitter helper.
