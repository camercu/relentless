# 4. Split the async builder from the retry state machine

Date: 2026-07-15

## Status

Accepted

Amends [ADR-0002](0002-defer-builder-docs-ergonomics-refactor.md)

## Context

`AsyncRetryExec` was simultaneously the configuration builder and the polling
state machine: alongside policy/hooks/op/sleeper it carried `phase`, `attempt`,
`total_wait`, `previous_delay`, `collect_stats`, and `final_stats`. SPEC 6.5.2
already modeled the two as distinct — the builder does not implement `Future`;
`.call()` returns the single-use future — so the conflation was pure
implementation debt, and it taxed everything around it:

- The builder needed a `SleepFut` type parameter (defaulted to `()`) purely so
  the phase enum was nameable before `.sleep()` was called, leaking into every
  public async alias.
- `.sleep()` had to remap the phase enum across the `SleepFut` change
  (`remap_no_sleep_phase`), including an `unreachable!` arm for a state the
  builder could never be in.
- The shared engine was a 13-parameter free function (`poll_async_loop`)
  because the state lived in another module's struct.
- Stats were threaded through a runtime `collect_stats: bool` plus a
  `final_stats` stash-and-take, while the sync engine used a
  `const COLLECT_STATS: bool` — a gratuitous asymmetry.

A harden-loop review flagged the conflation; the deepening was approved
(reversing the reviewer's own defer recommendation) with breaking changes
allowed.

## Decision

`AsyncRetryExec` is now configuration only: a plain (non-pinned) struct holding
policy, hooks, op, sleeper, elapsed tracker, and timeout. `.call()` constructs
the state machine — `AsyncEngine` in `execution/common.rs`, a pin-projected
struct owning the phase and counters, with
`poll_step::<const COLLECT_STATS: bool>` mirroring the sync engine's
`execute::<COLLECT_STATS>` — and returns it wrapped in a private single-use
future (`impl Future`).

`Fut` and `SleepFut` disappear from the builder's type parameters (`Fut`
becomes a generic on `.call()`); `.sleep()` is a plain field swap;
`remap_no_sleep_phase`, the `unreachable!` arm, `final_stats`, and
`take_final_stats` are deleted.

## Consequences

- Breaking (type-level only): `AsyncRetryExec`, `AsyncRetryExecWithStats`, and
  the six async aliases lose the `Fut` and `SleepFut` parameters
  (`DefaultAsyncRetryBuilder<F, Fut, T, E>` → `DefaultAsyncRetryBuilder<F, T,
  E>`). Code that only chains builder methods and `.call().await`s is
  unaffected; only explicit type annotations need updating.
- Runtime behavior is unchanged: same transition order, hook timing,
  cancellation semantics, and poll-after-completion panic (SPEC 15.2). The
  parity suite and the full test matrix pass unmodified.
- The builder is no longer self-referentially pinned, so it is plainly movable
  between configuration calls; misuse states (sleeping before `.sleep()`) are
  now unrepresentable rather than `unreachable!`-asserted.
- ADR-0002's deferred docs-ergonomics collapse (making `*Builder` the
  method-bearing type instead of an alias) remains open; this change shrinks
  the alias signatures it would touch.
