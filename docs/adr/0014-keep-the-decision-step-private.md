# 14. Keep the decision step private

Date: 2026-11-13

## Status

Accepted

## Context

An event-driven caller (no loop, its own timers, each outcome arriving as an
event) asked for a public stepper: "attempt `n` finished with outcome `X`;
retry after `d`, or stop with this result", in one call. The crate already has
that function. Since [ADR-0012](0012-remove-observer-hooks.md) the shared
decision step is pure: given the policy, the run's progress and an outcome, it
returns the next step. It is `pub(crate)`.

Without it, such a caller builds `RetryState::for_attempt(n)` and calls
`Stop::should_stop` and `Wait::next_wait` itself, re-deriving the loop's order
(classify, stop, wait).

The caller that asked has since stopped using the crate: its shutdown loop
became a single count compare. No other consumer is known.

## Decision

Keep the step `pub(crate)`. Do not add a public stepper or cursor type now.

## Consequences

- No new public surface, and no stability promise for `Progress` or the
  step's return type.
- Event-driven callers keep composing `Stop`, `Wait` and a classifier by hand.
- Publishing the step later is cheap and non-breaking: it is already pure and
  has direct unit tests. A real event-driven consumer reopens this ADR; that
  consumer's code is the acceptance test for the API.
