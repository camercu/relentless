# 3. Un-gate multi-hook registration from `alloc`

Date: 2026-07-15

## Status

Accepted

Supersedes [ADR-0001](0001-no-macro-fold-non-alloc-hook-setters.md)

## Context

Multiple hooks per hook point were gated behind the `alloc` feature (SPEC §2
matrix, old SPEC 8.5/8.6). A harden-loop review of the execution core found
the mechanism — `HookChain<First, Second>`, a type-level linked list — never
allocates: the old SPEC 8.6 claim that hooks are "stored in a `Vec`" was
false. The gate was therefore a pure surface-policy choice, not a technical
necessity, and it cost real machinery: three longhand single-slot
`#[cfg(not(feature = "alloc"))]` setter impls per engine (six blocks, ~180
lines) plus `compile_fail` doctests proving a constraint that protected
nothing.

## Decision

Remove the `alloc` gate from `HookChain` and the hook-chaining builder
methods. Multiple hooks of the same kind work in every feature configuration,
including `core`-only. The longhand no-alloc single-slot setters and their
`compile_fail` doctests are deleted; one macro (`impl_hook_chain!`, renamed
from `impl_alloc_hook_chain!`) generates the hook setters for both engines in
all configurations.

## Consequences

- SPEC §2 matrix row collapses to "Hooks (multiple per hook point): yes /
  yes / yes"; SPEC 8.5 now states multi-hook + registration order with no
  feature qualifier (8.6 is a tombstone).
- Breaking for no-alloc type-state observers only: without `alloc`,
  `before_attempt` previously produced hook slot type `Hook`, now
  `HookChain<(), Hook>`. Code that merely registers and runs hooks is
  unaffected; previously-rejected multi-hook code now compiles (widening).
- ADR-0001 is superseded, not refuted: it correctly declined macro-folding
  the longhand setters while they existed. This decision deletes them
  instead, which the ADR's own analysis rated the real fix ("repair before
  expand" — the duplication was the symptom, the gate the cause).
