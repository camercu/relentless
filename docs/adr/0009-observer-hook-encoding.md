# 9. Carry hooks as one observer parameter

Date: 2026-09-14

## Status

**Proposed, deferred on YAGNI.** The shape is established and measured; nothing
is implemented. The trigger for revisiting is a real request for a new hook
event — see [ADR-0008](0008-async-cancellation-semantics.md), which deferred
`on_cancel` on a cost this encoding removes. Until someone asks, the current
positional encoding stays.

This ADR exists so that when the request arrives, the design work is already
done and the decision is a lookup rather than a fresh investigation.

## Context

`Retry` and `AsyncRetry` carry eight type parameters, three of which are hook
slots — one positional parameter per hook point:

```rust
pub struct Retry<F, C, S, W, Cl, BA, AA, OX> { .. }
//                             ^^  ^^  ^^  before_attempt / after_attempt / on_exit
```

Each is `()` until something is registered. That encoding is why adding a hook
event is expensive: a fourth event needs a fourth parameter, threaded through
every signature that mentions the builder.

Measured, by adding a fourth event (`on_cancel`, registered but fired nowhere)
to the current design: **197 lines added, 77 existing lines rewritten, across 9
files** — including `tests/nameable_surface.rs`, because the builder's type is
written out there. `AsyncRun` went from 10 type parameters to 11.

Three findings from the preceding hardening run pointed at this same encoding:
documentation drift between the two builders, the default constants declared in
three files, and an earlier spike reporting 42 rewritten signatures for one line
of new capability.

## Decision

Carry hooks as a **single** parameter holding an observer — one trait with a
no-op default per event.

```rust
pub trait Observe<O, R, A> {
    fn before_attempt(&mut self, _state: &RetryState) {}
    fn after_attempt(&mut self, _state: &AttemptState<'_, O>) {}
    fn on_exit(&mut self, _exit: &Exit<'_, R, A, O>) {}
}

impl<O, R, A> Observe<O, R, A> for () {}                    // nothing registered
impl<O, R, A, T: Observe<O, R, A> + ?Sized> Observe<O, R, A> for &mut T { .. }
```

The builder loses two parameters and gains a bound only where it executes:

```rust
pub struct Retry<F, C, S, W, Cl, H> { .. }

impl<F, C, S, W, Cl, H, O> Retry<F, C, S, W, Cl, H>
where
    F: RetryOp<Output = O>,
    C: Decide<O>,
    H: Observe<O, C::R, C::A>,   // only `.call()` needs it
{ .. }
```

Registration is unchanged at the call site. Each setter wraps its closure in an
adapter overriding exactly one event, and links it onto what is already there:

```rust
pub struct BeforeAttempt<F>(F);
impl<O, R, A, F: FnMut(&RetryState)> Observe<O, R, A> for BeforeAttempt<F> {
    fn before_attempt(&mut self, state: &RetryState) { (self.0)(state); }
}

pub fn before_attempt<Hook>(self, hook: Hook)
    -> Retry<F, C, S, W, Cl, HookChain<H, BeforeAttempt<Hook>>>
where Hook: FnMut(&RetryState)
```

The slot is a cons list of single-event adapters, and every event walks the same
list. `HookChain` calls `first` then `second` per event, which is what preserves
SPEC 8.5's registration order.

`Observe<O, R, A>` orders its parameters outcome-first because the outcome is
what the builder knows earliest, from `F::Output`; `R` and `A` exist only once a
classifier is chosen.

One new method has no equivalent today:

```rust
pub fn observe<O, Obs>(self, observer: Obs) -> Retry<F, C, S, W, Cl, HookChain<H, Obs>>
where Obs: Observe<O, C::R, C::A>
```

## Consequences

### What it buys

**A hook event becomes additive.** The same fourth-event experiment on the
observer design: **3 existing lines rewritten instead of 77**, across 4 files
instead of 9, touching no test file. The event is a defaulted method on one
trait plus one adapter.

**Two fewer type parameters**: builders 8 → 6, `AsyncRun` 10 → 8. A side effect,
not the goal — a rejected alternative reached 6 parameters while changing
nothing that mattered.

**The closure API is untouched.** `tests/hooks.rs` needed zero changes.

**Less duplicated signature between the two builders.** Every parameter removed
is one fewer place the sync and async surfaces can drift — the failure this run
hit twice, and which only prose currently guards against.

**State shared across events stops needing a `RefCell`.** Two closures touching
one counter share it through interior mutability today; `.observe(my_type)`
takes one value whose fields are that state.

**Type erasure becomes opt-in.** `&mut dyn Observe<..>` satisfies the slot, so a
consumer can have dynamic dispatch without the crate paying a vtable call per
attempt by default — and the observer stays inspectable after the run, having
been borrowed rather than moved.

### What it costs

**Public API surface grows per event, and this is inherent.** This is the one
cost that needs spelling out, because it is a direct consequence of the thing
that makes the design work.

Defaulted trait methods are what let an adapter override one event and ignore
the rest. But a defaulted method is still a method *on every implementor*.
`BeforeAttempt<F>` overrides `before_attempt` and inherits the other two as
no-ops — and those inherited no-ops are public API:

```rust
pub struct BeforeAttempt<F>(F);
impl<O, R, A, F: FnMut(&RetryState)> Observe<O, R, A> for BeforeAttempt<F> {
    fn before_attempt(&mut self, state: &RetryState) { (self.0)(state); }
    // after_attempt and on_exit inherited as no-ops — and still public
}
```

So the API snapshot carries an entry for every (adapter × event) pair, not just
the pairs that do something. Adding a fourth event adds a method to the trait,
which adds an entry to *every* adapter — and the new adapter arrives carrying
entries for every *existing* event. Both directions of the grid fill in at once.

Here is the actual `public-api.txt` growth from adding `on_cancel`, grouped:

```text
# the new event, landing on every existing implementor — all no-ops
pub fn &mut T::on_cancel
pub fn ()::on_cancel
pub fn BeforeAttempt<F>::on_cancel
pub fn AfterAttempt<F>::on_cancel
pub fn OnExit<F>::on_cancel
pub fn HookChain<First, Second>::on_cancel

# the new adapter, carrying every existing event — all no-ops but one
pub fn OnCancel<F>::before_attempt
pub fn OnCancel<F>::after_attempt
pub fn OnCancel<F>::on_exit
pub fn OnCancel<F>::on_cancel      <- the only one that does anything

# the parts that are genuinely new capability
pub fn Observe::on_cancel
pub fn Retry<..>::on_cancel<Hook>
pub fn AsyncRetry<..>::on_cancel<Hook>
```

Of the **35 lines added, roughly four carry the feature**; the rest are the grid
filling in. With *E* events and *A* adapters the snapshot holds E×A entries, so
a fifth event would add another ~7 no-ops plus its adapter's ~5.

The trade is therefore not "77 rewritten lines become 3" — it is *77 rewritten
lines in existing code become 3 rewritten lines plus ~30 lines of no-op surface
in the committed API snapshot*. Churn converts into surface. Two independent
implementations paid it identically, so it belongs to the shape rather than to
either one.

Whether it can be trimmed is untested. `#[doc(hidden)]` keeps the no-ops out of
rustdoc but not out of `public-api.txt`. Sealing the adapters, or hand-writing
each impl so it declares only its own event, would cut the grid at the cost of
losing the defaulted-method property that makes a new event additive in the
first place — which may simply be the trade restated rather than avoided.

**It is a breaking change.** The builder's type parameters change, so any
consumer naming the type must update. Pre-1.0 that is a minor bump.

**`Observe` joins the public surface** and must be named in bounds by code
generic over a configured builder, as `BeforeAttemptHook` and its siblings are
today. The count of such names does not grow; their shape changes.

### What is preserved

Verified on both spike implementations rather than assumed: `no_std` builds
clean, the core loop stays allocation-free (`tests/allocation.rs` 5/5), no new
dependencies, MSRV unchanged, and op-anchored inference still works —
`.decide(|outcome| ..)` infers its parameter with no annotation, including after
hooks are registered. That last was untested by the spikes and checked
separately, because the hook carrier shares a type with the classifier slot and
could have forced a turbofish at every `.decide` call site.

## What this unlocks elsewhere

ADR-0008 investigated cleanup on async cancellation and found that
cancellation-safe cleanup is *already expressible* — the defect is that the safe
and leaky spellings look identical:

```rust
.on_exit(move |_| { slot.take(); })   // closure OWNS the guard → survives drop
.on_exit(move |_| gauge.dec())        // closure CALLS a method  → leaks
```

It proposed a dedicated `.holding(resource)` slot to make the safe spelling
visible. **Under this encoding that needs no new mechanism** — it is an
`Observe` impl that overrides nothing:

```rust
pub struct Holding<G>(G);
impl<O, R, A, G> Observe<O, R, A> for Holding<G> {}   // every event a no-op

.observe(Holding(permit))
```

The observer is owned by the future: normal exit drops it, cancellation drops
it, and the guard releases either way with no user code running from `Drop`.
That matters because ADR-0008's other finding was that firing a hook from `Drop`
aborts the process when the operation panics and the hook panics during
unwinding — uncatchable, and only guardable under `std`.

So the two investigations resolve into one design: the encoding change turns
ADR-0008's cancellation fix from a new feature into an instance of an existing
one. **This is the one claim here that is inference rather than measurement** —
it follows from two separately verified facts (the observer is owned by the
future; a no-op impl compiles away) but was not built.

## The conditional at the centre of this

**The 77-to-3 saving only pays out when an event is actually added.** If no
fourth event ever lands, this trades a breaking change and 35-lines-per-event of
future API surface for two fewer type parameters and a tidier internal shape.

That is why this ADR is deferred rather than accepted. The measurements settle
*how* to carry hooks; they do not establish that another hook event is wanted.
ADR-0008 deferred `on_cancel` on cost, and removing that cost is not the same as
demonstrating demand. Wait for a real request.

## Alternatives considered

**Collapse the three slots into one parameter carrying a struct** —
`Retry<F, C, S, W, Cl, ExecutionHooks<BA, AA, OX>>`. Built and measured. Reaches
the same 6 parameters and changes nothing: **16 sites still name all three
slots**, so a fourth event edits all 16. The arity leaks into the type whether
spelled as three parameters or as one struct with three. This is the result that
proves parameter count is the wrong metric.

**Erase the hooks entirely** — `Box<dyn Fn..>` or a `&mut dyn` slot on the
builder. A new event becomes genuinely free. Rejected on the hard requirements:
`Box` needs `alloc` and the default build is `no_std` with no dependencies,
while a `&mut dyn` slot forces a lifetime parameter onto the builder and costs a
vtable call per attempt even when nothing is registered. The observer design
permits this as opt-in instead of paying for it by default.

**Move the hooks off the builder into a separate runner.** Whatever holds them
still needs a type parameter, and it splits the configuration surface in two.

**Leave the encoding alone.** Taken seriously, and the right answer if the
current shape were already minimal. It is not: the cost of an event is
per-event and unbounded, and the async future stood at 10 parameters before a
fourth event pushed it to 11. Deferring adoption is not the same as rejecting
the shape.

## How this was established

A three-arm spike tournament (`spike/hook-typestate-*`, throwaway, not merged).
One arm directed at a trait-with-defaults, one at the struct-carrier
alternative, one clean-room — forbidden from reading the mechanism list so that
agreement would carry evidential weight.

The clean-room arm and the trait-directed arm **converged on the same design
independently**, and the clean-room arm rejected the struct-carrier alternative
in the terms measurement later confirmed. That convergence, rather than either
arm's own argument, is why one round was treated as decisive.
