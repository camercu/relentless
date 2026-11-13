# 7. Blanket `Outcome` impls for standard-library types

Date: 2026-07-24

## Status

Accepted — implemented 2026-07-24. Blanket `impl Outcome` ships for
`Result<T, E>`, `Option<T>`, and `ControlFlow<B, C>`. `Poll<T>` and the
transient-error classifier helpers are deferred; `bool` and other single-state
types are declined.

## Context

[ADR-0006](0006-paired-decision-classifier-engine.md) introduced the `Outcome`
trait: an owned outcome type classifies *itself* into a `Verdict`
(`Return`/`Retry`/`Abort`), so the default engine path drives it with no
`.decide(...)` at the call site. A blanket `impl<T, E> Outcome for Result<T, E>`
shipped with it; `impl<T> Outcome for Option<T>` followed when a real consumer
(`blivet`'s test poll helper) had to launder readiness through `Result<T, ()>`
purely because `Option` had no impl.

That raised the general question: **which standard types should receive a
blanket `Outcome` impl, and which should be left to `.decide(...)`?** A blanket
impl is not free. The orphan rule makes it a *permanent* reservation — once
`impl<..> Outcome for Foo<..>` exists in this crate, no downstream crate can
impl `Outcome` for `Foo`, and anyone whose `Foo` means something other than our
fixed classification must fall back to `.decide`. So each blanket impl trades a
one-time ergonomic win for a permanent commitment. We need a repeatable test
rather than case-by-case taste.

## Decision

### Criteria for a blanket `Outcome` impl

A standard type earns a blanket impl only if it meets **all three**:

1. **Unambiguous direction.** Exactly one variant is obviously "done, return
   this"; the rest obviously mean "retry". No competent reader would expect the
   opposite mapping.
2. **Non-empty return channel.** The accepted variant carries a value worth
   delivering to the caller — otherwise the retry loop yields nothing.
3. **Justified orphan cost.** The type is common enough as a poll/retry outcome
   to be worth the ergonomic win, but *not* so commonly returned with unrelated
   meaning that reserving it would trap consumers who need a different
   classification.

### Implemented

| Type | `Break`/done → `Return` | retry | `Abort` |
| --- | --- | --- | --- |
| `Result<T, E>` | `Ok(v)` → `v` | any `Err` | `E` (never produced on the default path) |
| `Option<T>` | `Some(v)` → `v` | `None` | `Infallible` |
| `ControlFlow<B, C>` | `Break(b)` → `b` | `Continue(_)` | `Infallible` |

`ControlFlow` is the strongest candidate after `Result`/`Option`: it is *the*
standard "keep going vs stop with a value" type, its variant names already read
as the verdicts (`Continue` = retry, `Break` = return), and its meaning is
inherently loop control, so trap risk (criterion 3) is near zero.

**Error-ergonomics obligation.** Every `Infallible`-abort blanket impl must also
provide `Display` and `std::error::Error` for its concrete
`RetryError<Infallible, O>` shape (see §4.1.9–4.1.10 of the SPEC), or that
outcome type is a second-class citizen whose exhaustion error cannot be
`?`-propagated or formatted — the exact gap the `Option` work had to repair. A
*blanket* `impl<O> Display for RetryError<Infallible, O>` is not possible:
`RetryError<Infallible, Result<T, Infallible>>` would overlap the existing
`Result`-shaped `Display` impl and fail coherence. So the paired `Display`/
`Error` impls are written per-type, once per blanket `Outcome` impl.

### Deferred (revisit when a concrete use appears)

- **`core::task::Poll<T>`** (`Ready(t)` → `Return(t)`, `Pending` → `Retry`). The
  mapping is unambiguous and on-theme for a polling crate, but `Poll` is
  async-task machinery returned by `Future::poll`; a blanket impl would conflate
  *reactor* readiness with *retry* readiness and permanently reserve a type few
  consumers deliberately return from a retry op. Add it if that demand
  materialises.
- **Transient-error classifier helpers for `io::Result` / `anyhow::Result`.**
  These are *already* `Outcome` via the `Result` blanket — nothing to add there.
  The genuinely useful addition is a test for the `.when` path (e.g. a plain
  `fn io_transient(&io::Error) -> bool` passed as `.when(io_transient)`, retrying `WouldBlock`/`Interrupted`/`TimedOut` and
  aborting `NotFound`/`PermissionDenied`), feature-gated so `io`/`anyhow` don't
  weigh on the core. That is a different mechanism, out of scope for this ADR,
  deferred until requested.

### Declined

- **`bool`.** Fails criteria 1 and 2: "retry while `true`" and "return on
  `true`" are both natural readings (ambiguous direction), and the return
  channel is empty (`Return = ()`). `bool` is also among the most commonly
  returned types with arbitrary meaning, so a blanket reservation would trap
  many consumers. Express a boolean poll as `Option<()>` via
  `cond.then_some(())`, or use `ControlFlow`.
- **Numeric types, `String`, `Cow`, `Box`/smart pointers.** Single-state — no
  natural done/retry split.
- **`cmp::Ordering`.** Three variants but no natural mapping of any one to
  "done".

## Consequences

- Consumers get zero-`.decide` polling for the three canonical outcome shapes;
  the `blivet` motivating case and any `ControlFlow`-based loop are covered
  directly.
- The three criteria give a repeatable test for future "please add `impl Outcome
  for X`" requests, and this ADR records the standing verdicts so they are not
  relitigated from scratch.
- Deferrals are cheap to revisit: adding a blanket impl later is additive and
  non-breaking (it only removes an `E0277`), so nothing here forecloses `Poll`
  or the transient-error helpers.
- New blanket `Outcome` impls carry a checklist obligation: pair every
  `Infallible`-abort impl with per-type `Display`/`Error` on its `RetryError`
  shape.
