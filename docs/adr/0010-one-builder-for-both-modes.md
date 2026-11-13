# 10. One builder for both modes

Date: 2026-10-19

## Status

Accepted. Decision 5 is refined by
[ADR-0011](0011-bounded-call-type-on-operation.md). Amended by
[ADR-0012](0012-remove-observer-hooks.md): the builder carries no hook
parameters, so `Retry<M, F, C, S, W, Cl>` and the hook setters drop out of
the plan below.

Builds on [ADR-0004](0004-split-async-builder-from-state-machine.md).

## Context

`Retry`, `AsyncRetry` and `RetryPolicy` each write the same configuration
setters by hand. A setter rebuilds its whole struct to change one field, so
`stop`, `wait`, `timeout`, `when`, `until` and `decide` exist three times, and
`clock`, `with_stats` and the three hook setters exist twice. The two builders
are about 77% textually identical. `sync_engine.rs` and `async_engine.rs` are
the most frequently co-changed pair of source files, and ADR-0009 measured one
new hook event at 9 files. The only guard against drift is a line in
`AGENTS.md`.

The sync and async builders differ in four places: the operation trait
(`RetryOp` or `AsyncRetryOp`), the clock trait, what `.call()` returns, and the
poll state machine behind it. Everything else is shared.

## Decision

Breaking changes are allowed (the crate is at 0.x), so each choice below takes
the best design over the smallest diff.

1. **One builder type.** `Retry<M, F, C, S, W, Cl, BA, AA, OX>`, where `M` is
   the marker `Blocking` or `Async`. The entry point sets it: `retry(op)` and
   `RetryExt::retry` give `Retry<Blocking, …>`; `retry_async(op)` and
   `RetryExt::retry_async` give `Retry<Async, …>`. Mode cannot be inferred from
   the operation or the clock, because a closure returning a future satisfies
   both operation shapes. The marker is not named `Sync`, which would shadow
   `core::marker::Sync`.
2. **`.call()` is implemented once per marker.** The async one returns the
   existing `AsyncRun` future. Every other setter is written once.
3. **`Retry` embeds a `RetryPolicy<S, W, C>`.** The policy owns the setters for
   stop, wait, timeout and classifier. The builder forwards to them. A policy
   stays a plain reusable value: `Send + Sync`, boxable, and borrowed by
   `policy.retry(op)`.
4. **`AsyncRetry` and `AsyncRetryWithStats` are removed**, with no aliases.
   rustdoc does not inline inherent methods onto type-alias pages (ADR-0002),
   so an alias would be a method-less stub.
5. **`Operation<M>` replaces `RetryOp` and `AsyncRetryOp`.** It carries
   `type Output` (the outcome) and the per-mode attempt type. Closures and
   `StatelessOp<F>` implement it for both markers, through one blanket impl
   each that is generic over the mode. The five outcome-anchored setters
   (`decide`, `when`, `until`, `after_attempt`, `on_exit`) bound
   `F: Operation<M, Output = O>`.
6. **One `RetryExt` trait** replaces `RetryExt` and `AsyncRetryExt`, with two
   methods, `retry()` and `retry_async()`, each carrying its own bound. The
   method name carries the mode, as it does on `RetryPolicy`.
7. **ADR-0009 stays deferred.** Hooks remain three positional type parameters.
   Revisit when a real request for a new hook event arrives, the trigger that
   ADR already names. After this change that revisit threads one type instead
   of two engines and a policy.

## Consequences

- Adding a setting touches `RetryPolicy` and one forwarder on `Retry`.
- The sync and async test twins that exist only because there were two
  builders can be merged; `tests/parity.rs` keeps guarding behavior.
- Breaking for consumers that name builder types or the operation traits:
  `AsyncRetry<…>` becomes `Retry<Async, …>`, `AsyncRetryWithStats<…>` becomes
  `RetryWithStats<Async, …>`, `RetryOp`/`AsyncRetryOp` bounds become
  `Operation<Blocking>`/`Operation<Async>`, and `AsyncRetryExt` becomes
  `RetryExt`. Call sites that only chain methods are unaffected.
- Generic arity grows by one (`M`), so `nameable_surface.rs` changes.
- The migration lands in four commits, each green: embed `RetryPolicy` in both
  builders (internal, no API change); replace `RetryOp`/`AsyncRetryOp` with
  `Operation<M>` (introduces the markers); merge the two builders into
  `Retry<M, …>`; merge the two ext traits.
