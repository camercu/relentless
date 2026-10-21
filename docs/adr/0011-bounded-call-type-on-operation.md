# 11. Bound the call type on `Operation`

Date: 2026-10-20

## Status

Accepted

Refines decision 5 of [ADR-0010](0010-one-builder-for-both-modes.md), which
replaced `RetryOp` and `AsyncRetryOp` with one `Operation<M>` trait.

## Context

ADR-0010 merges the blocking and async builders into one `Retry<M, …>`, where
the marker `M` is `Blocking` or `Async`. Its first cut of the operation trait
had two unrelated associated types:

```rust
pub trait Operation<M> {
    type Output;  // the outcome of one attempt
    type Attempt; // what calling yields: the outcome, or a future of it
    fn call_op(&mut self, state: RetryState) -> Self::Attempt;
}
```

Code generic over a blocking builder had to bind both to call `.call()`:
`F: Operation<Blocking, Output = O, Attempt = O>`. The compiler cannot see that
a blocking attempt is its own outcome, so `Output = O` alone did not work.

We compared six designs in a spike tournament over two rounds. Each design was
a standalone model that ran the same seven probes:

1. A generic blocking consumer calls `.call()`.
2. A generic async consumer awaits `.call()`.
3. Setter closures infer their parameter types with no annotations.
4. A closure that returns a future is used in blocking mode.
5. A no-argument closure works in both modes.
6. A generic async consumer requires the retry future to be `Send`.
7. A struct field stores the async retry future by name.

Every design passed probes 1 to 5. Every design also accepts probe 4, a future
used as a blocking outcome; closing that needs negative trait bounds, which
stable Rust lacks. Probes 6 and 7 came from the second round, so only the
three designs still standing were measured on them. "Public names" counts the
names a consumer writes across probes 1, 2, 6 and 7: `Retry`, `Blocking`,
`Async`, `Decide`, the operation trait or traits, and the named async future.

| Design | Blocking / async bound | `Send` bound for generic async code | Public names |
|---|---|---|---|
| Bounded call (chosen) | `Operation<Blocking, Output = O>` / `Operation<Async, Output = O>` | `F: Operation<Async, Output = O, Call: Send> + Send, C: Decide<O> + Send` | 6 |
| Per-mode call traits | `BlockingOp<Output = O>` / `AsyncOp<Output = O>` | `F: AsyncOp<Output = O, Fut: Send> + Send, …` | 7 |
| Attempt only | `Operation<Blocking, Attempt = O>` / `Operation<Async, Attempt: Future<Output = O>>` | `F: Operation<Async, Attempt: Future<Output = O> + Send> + Send, …` | 6 |
| Two traits plus a hidden helper | `RetryOp<Output = O>` / `AsyncRetryOp<Output = O>` | not measured (eliminated in round 1) | 6, plus a hidden `OpOutput<M>` that code generic over the mode must name |
| Alias traits | `BlockingOperation<O>` / `AsyncOperation<O>` | not measured (eliminated in round 1) | 6 |

A sixth design returned the attempt as `impl Trait` from the trait method. It
reached the ideal bound, but the async future had no name. Generic async code
could not require `Send` without return-type notation, which is unstable, and
the public `AsyncRun` type could not store the future. We dropped it.

## Decision

Give the operation trait a call type that the trait itself bounds by the
outcome:

```rust
// `SealedOp<M>` is a private supertrait: only closures and `StatelessOp`.
pub trait Operation<M>: SealedOp<M> {
    type Output;
    type Call: Attempt<M, Output = Self::Output>;
    fn call_op(&mut self, state: RetryState) -> Self::Call;
}

// `Sealed<M>` is a private supertrait, so only these two blanket impls exist.
pub trait Attempt<M>: Sealed<M> {
    type Output;
    #[doc(hidden)]
    fn poll_attempt(self: Pin<&mut Self>, cx: &mut Context<'_>, token: Token<M>)
        -> Poll<Self::Output>;
}
impl<T> Attempt<Blocking> for T { type Output = T; /* never polled */ }
impl<F: Future> Attempt<Async> for F { type Output = F::Output; /* polls F */ }
```

Item bounds on an associated type are implied wherever the trait is used. So
`F: Operation<M, Output = O>` is enough in both modes. The blocking driver
turns a call result into the outcome through a private identity function,
`fn ready_outcome<A>(a: A) -> <A as Attempt<Blocking>>::Output { a }`. Without
it the compiler rejects the assignment (E0308). The async driver stores
`F::Call` in the named `AsyncRun` future and polls it through `poll_attempt`.

`Token<M>` is a private type indexed by mode, and only `Token<Async>` has a
constructor. No code, in this crate or outside it, can call `poll_attempt` on a
blocking attempt. A mode-generic call fails with E0308 at compile time. The
blocking impl's body is therefore dead code. The spike's `Token<M>` is a
`PhantomData` wrapper, so the compiler cannot prove the body dead and it holds
an `unreachable!()`. Production can make the arm total with a per-mode token
type whose blocking variant is uninhabited (for example `core::convert::Infallible`),
then `match` on it.

Per-mode call traits stay the recorded fallback. They have no dead arm and the
clearest errors for people who implement `Operation` by hand, at the cost of
one more public name and three operation traits instead of one.

## Consequences

- Consumers bind one thing, `Output = O`, in both modes. Code that is generic
  over the mode and calls a setter needs no extra bound.
- Generic async code requires `Send` with `Call: Send`, and never names
  `Attempt`. The other builder parameters (`F`, the classifier, the stop and
  wait strategies, the hooks) need `Send` bounds too, as the table shows. A
  clock left generic needs `Cl: AsyncClock<Wait: Send> + Send`.
- `AsyncRun` keeps a nameable future. It pins the call with
  `pin_project_lite`, which the crate already uses under
  `#![forbid(unsafe_code)]`. No `Unpin` bound is needed; one would reject
  every plain `async {}` block.
- `Attempt` is public, sealed and hidden from rustdoc, like `Decide`.
  Consumers never write it.
- `Operation` is sealed too. Every entry point takes a closure, so a
  hand-written `Operation` could never run; sealing turns that dead end into
  a compile error. Consumers still name it in bounds. The per-mode call trait
  fallback's advantage for hand implementers is therefore moot.
- The method keeps the first cut's name, `call_op`, so it does not read like
  `Retry::call` or `AttemptHook::call` next to it.
- Two internals are load-bearing and need a comment each: `ready_outcome`, and
  calling `Attempt::<Async>::poll_attempt` with the mode spelled out, so the
  blanket blocking impl is never a candidate.
- A future-returning closure passed to `retry` still compiles as a blocking
  outcome, as it does today.
