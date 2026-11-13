# 12. Remove the observer hooks

Date: 2026-11-13

## Status

Accepted

Supersedes [ADR-0003](0003-ungate-multi-hook-from-alloc.md) and
[ADR-0009](0009-observer-hook-encoding.md). Amends
[ADR-0010](0010-one-builder-for-both-modes.md): the single builder carries no
hook parameters. Settles the `on_cancel` question
[ADR-0008](0008-async-cancellation-semantics.md) deferred: there is no hook to
add it to.

## Context

`Retry` and `AsyncRetry` offered three hook points, each a type parameter on
every builder and run type (`BA`, `AA`, `OX`), chained through `HookChain`:

- `before_attempt(|state: &RetryState|)`
- `after_attempt(|state: &AttemptState<O>|)`, with the attempt number, elapsed
  time and a borrow of the outcome, before classification
- `on_exit(|exit: &Exit<R, A, O>|)`, once, with the terminal variant, attempt
  number, elapsed time and a borrow of the result

Each one duplicates something a caller already holds:

| Hook | Same information, without the hook |
|---|---|
| `before_attempt` | The operation closure receives the same `RetryState`. Code at its top runs before every attempt. |
| `after_attempt` | Wrap the operation: call it, inspect the outcome, return it. Async: `let out = fut.await; …; out`. |
| `on_exit` | `.with_stats().call()` returns `Ok(R)` or `RetryError::{Exhausted, Aborted} { last }` plus `RetryStats { attempts, total_elapsed, stop_reason, .. }`, owned. |

On async cancellation `on_exit` does not fire (ADR-0008), while a `Drop` guard
inside a wrapped operation does, so the wrapper is the stronger tool there.

What the hooks cost: three of eight type parameters on five public types and
every bound a user writes to name a builder; the `HookChain` type-state and
three hook traits; `AttemptState` and `Exit`; a `hooks` argument through the
shared decision step; and two test files plus parity scenarios. ADR-0009
measured that adding a fourth event touched nine files.

The one observation callers ask for that a wrapped operation cannot make is
"about to sleep for `d`" before a long wait. The hooks never offered it:
`after_attempt` runs before classification and never sees the next delay. A
wrapper around the wait strategy does, because the engine consults the strategy
only once a retry is certain.

## Decision

Remove the hooks: the `before_attempt`, `after_attempt` and `on_exit` setters,
the `BA`/`AA`/`OX` type parameters, `HookChain`, `BeforeAttemptHook`,
`AttemptHook`, `ExitHook`, `AttemptState` and `Exit`. Keep `RetryStats`,
`StopReason` and `with_stats`: they hand the caller owned facts about the run
that it cannot rebuild from the result alone.

Document observation as three recipes: code at the top of the operation, a
wrapped operation, and a match on the result with stats. Document the
pre-sleep log as a wrapper around the wait strategy.

## Consequences

- `Retry<F, C, S, W, Cl>` and `AsyncRetry<F, C, S, W, Cl>` take five type
  parameters. A helper generic over a configured builder names three fewer
  bounds.
- The shared decision step takes no hooks and fires nothing: given the policy,
  the run's progress and an outcome, it returns the next step. That is the
  shape a public stepper for event-driven callers needs.
- Breaking for every hook user. Migration is mechanical, one recipe per hook.
- A future need for an event the wrapper recipes cannot express (none is known)
  reopens this ADR rather than ADR-0009.
