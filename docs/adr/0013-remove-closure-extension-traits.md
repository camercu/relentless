# 13. Remove the closure extension traits

Date: 2026-11-13

## Status

Accepted

Supersedes decision 6 of [ADR-0010](0010-one-builder-for-both-modes.md) (one
merged `RetryExt` trait): there is no extension trait left to merge. Narrows
probe 5 of [ADR-0011](0011-bounded-call-type-on-operation.md): a no-argument
operation is written `|_| op()` in both modes.

## Context

The crate had two spellings for starting a retry:

- `retry(|state| op(state))` and `retry_async(|state| async { … })`, free
  functions taking `FnMut(RetryState) -> O`.
- `(|| op()).retry()` and `(|| async { … }).retry_async()`, through the
  `RetryExt` and `AsyncRetryExt` extension traits, blanket-implemented for
  `FnMut() -> O`.

Both built the same builder and drove the same engine. The extension form saved
one `|_|` and cost:

- two public traits, a `StatelessOp<F>` newtype with its own `Operation<M>`
  impls and seal, and their prelude entries;
- a second closure shape. Refactoring from the extension form to one that reads
  `RetryState` changed the closure's arity, and the compiler error for the wrong
  shape (E0599 on the extension side) never named the sibling spelling;
- an equivalence test and cross-linking docs whose only job was to keep the two
  spellings in step.

Coherence rules out one blanket impl covering both closure shapes (E0119), so
the two spellings could only be kept, not merged.

## Decision

Remove `RetryExt`, `AsyncRetryExt` and `StatelessOp`. `retry` and
`retry_async` (and `RetryPolicy::{retry, retry_async}`) are the only entry
points. An operation that ignores the state writes `|_|`.

## Consequences

- One closure shape everywhere: `FnMut(RetryState) -> O`. Adding a use of the
  state is an edit to the closure body, not a change of entry point.
- `Operation<M>` is implemented by closures alone.
- Breaking for extension-trait users. Migration is mechanical:
  `(|| op()).retry()` becomes `retry(|_| op())`, `f.retry()` becomes
  `retry(|_| f())`, and the async forms use `retry_async` the same way.
- Method-call syntax on a bare function (`fetch.retry()`) is gone. That is the
  fluency the deferred option in the UX-polish spike gave up on purpose.
