# Overview

This library, tenacious, is a Rust library for retrying fallible operations and polling for conditions.

For more information on it's specifications, see [SPEC.md](/docs/SPEC.md).

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
