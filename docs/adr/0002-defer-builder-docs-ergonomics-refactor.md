# 2. Defer the builder type-state docs-ergonomics refactor

Date: 2026-06-21

## Status

Proposed

Amended by [ADR-0004](0004-split-async-builder-from-state-machine.md): the
async builder/state-machine conflation this ADR's surface sat on has been
split, dropping the `Fut`/`SleepFut` parameters from the async aliases. The
struct/alias docs-ergonomics collapse below remains deferred.

## Context

The crate root exports ~16 builder/exec type names: the four method-bearing
structs (`SyncRetryExec`, `AsyncRetryExec`, and their `*WithStats` variants) and
twelve type aliases over them (`SyncRetryBuilder`, `SyncRetry`,
`DefaultSyncRetryBuilder`, the `*WithStats` family, async mirrors). Every name is
the return type of some public method, so all must stay public and nameable.

The builder DSL methods (`.sleep`, `.call`, `.with_stats`, `before_attempt`,
`after_attempt`, `on_exit`) are inherent impls on the `*Exec` **structs**.
rustdoc (1.94) does not inline inherent methods onto type-alias pages, so the
friendly `*Builder` alias pages are stubs and the methods are documented only on
the awkwardly-named `*Exec` struct pages.

`#[doc(hidden)]` was considered to declutter the root but rejected: hiding the
structs erases the method docs; hiding the aliases dead-links every method's
return type; and it would not shrink the locked, semver-relevant surface anyway.

## Decision

Ship 0.10.0 with the current shape. Defer any restructuring to a later release.

The real improvement is to collapse the struct/alias split so the friendly
`*Builder` name is the method-bearing type itself (not an alias over `*Exec`),
giving one well-named, fully-documented page per builder state. That is a
breaking, wide-blast-radius change across the four execution/ext files and is
not pre-release polish.

## Consequences

- The docs front page lists more builder type names than a reader needs, and the
  method docs sit on `*Exec` pages rather than the friendly `*Builder` names.
- No functional or correctness impact; the API works and is fully nameable
  (`NoSyncSleep`/`NoAsyncSleep` are now re-exported at the root so the
  `retry`/`retry_async` return types are spellable).
- Revisit post-0.10.0 as a dedicated breaking change with its own ADR.
