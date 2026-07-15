# 2. Defer the builder type-state docs-ergonomics refactor

Date: 2026-06-21

## Status

Proposed

Amended by [ADR-0004](0004-split-async-builder-from-state-machine.md): the
async builder/state-machine conflation this ADR's surface sat on has been
split, dropping the `Fut`/`SleepFut` parameters from the async aliases. The
struct/alias docs-ergonomics collapse below remains deferred.

Deferral re-confirmed 2026-07-15 after a full read of both engines (see
"Why the collapse is a lateral move" below). The collapse is a naming/churn
trade, not a net improvement; it cannot fully eliminate the stub-alias
problem it targets.

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

## Why the collapse is a lateral move

Each `*Exec` struct is a *single* engine generic over `Policy`, and it backs
*two* aliases: the borrowed path (`SyncRetry`/`AsyncRetry`, from
`RetryPolicy::retry`/`retry_async`) and the owned path
(`SyncRetryBuilder`/`AsyncRetryBuilder`, from the ext traits and free
functions). This is deliberate — one state machine, two thin entry aliases —
and it holds on both sync and async (ADR-0004 split the async *state machine*
from config, not this borrowed/owned duality).

rustdoc documents inherent methods only on a real struct's page; every
`type X = Y` alias is a stub. So the friendly name can only carry the method
docs by *being* the struct, which forces renaming `*Exec`. But because the
same struct also backs the borrowed `*Retry` alias:

- renaming it `*Builder` misnames the borrowed-policy execution object, and the
  borrowed `*Retry` alias *still* stubs — the stub problem cannot be fully
  eliminated;
- the struct is `<Policy, ...>` while the friendly aliases expand `Policy` into
  `<S, W, P, ...>` (owned) or add a `'policy` lifetime (borrowed), so making the
  struct *be* `*Builder` changes its generic arity and loses the expanded
  convenience form (or needs the aliases kept anyway).

The three implementable shapes — (1) rename `*Exec → *Builder`, (2) split into
two real structs with duplicated forwarding methods, (3) newtype wrappers —
each land another imperfect name or duplicate ~10 methods across four structs
(dismantling the single-engine deep module). None is a net win over the
current shape.

## Consequences

- The docs front page lists more builder type names than a reader needs, and the
  method docs sit on `*Exec` pages rather than the friendly `*Builder` names.
- No functional or correctness impact; the API works and is fully nameable
  (`NoSyncSleep`/`NoAsyncSleep` are now re-exported at the root so the
  `retry`/`retry_async` return types are spellable).
- The docs-discoverability gap is a rustdoc presentation limitation, best
  mitigated by keeping worked examples on the alias doc-comments (done) rather
  than a breaking restructure. Revisit only if rustdoc gains alias-method
  inlining, or alongside a broader breaking release with its own ADR.
