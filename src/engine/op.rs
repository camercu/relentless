//! Operation abstraction shared by the op-first and stateless-ext entry points.
//!
//! The builder stores its operation behind [`RetryOp`] so the same engine drives
//! both `retry(|state| ...)` (an `FnMut(RetryState) -> O`) and
//! `(|| ...).retry()` (an `FnMut() -> O` wrapped in [`StatelessOp`], which
//! discards the state). The op-anchored classifier methods bound `F: RetryOp<O>`,
//! which — through the blanket impl — still pins `O` to the closure's output, so
//! inline classifiers keep inferring with no annotations.

use crate::state::RetryState;
use core::future::Future;

/// Unifies stateful and stateless operations behind one call interface.
///
/// `Output` is an associated (not a generic) type so that `F: RetryOp` uniquely
/// determines the outcome type — exactly as `FnMut(..) -> O` does through
/// `Fn::Output` — which both satisfies the constrained-type-parameter rule and
/// lets op-anchored classifier closures keep inferring with no annotations.
pub trait RetryOp {
    /// The outcome an attempt produces.
    type Output;
    /// Runs one attempt, given the current retry state.
    fn call_op(&mut self, state: RetryState) -> Self::Output;
}

impl<O, F: FnMut(RetryState) -> O> RetryOp for F {
    type Output = O;
    fn call_op(&mut self, state: RetryState) -> O {
        (self)(state)
    }
}

/// Adapts a no-argument `FnMut() -> O` operation to [`RetryOp`] by discarding the
/// [`RetryState`]. Produced by the `retry()` extension method.
#[doc(hidden)]
pub struct StatelessOp<F>(pub F);

impl<O, F: FnMut() -> O> RetryOp for StatelessOp<F> {
    type Output = O;
    fn call_op(&mut self, _state: RetryState) -> O {
        (self.0)()
    }
}

/// Async counterpart to [`RetryOp`]: unifies stateful and stateless async
/// operations. `Future` and `Output` are associated so `F: AsyncRetryOp` pins
/// both, preserving op-anchored inference (see [`RetryOp`]).
pub trait AsyncRetryOp {
    /// The outcome an attempt produces.
    type Output;
    /// The future an attempt returns.
    type Future: Future<Output = Self::Output>;
    /// Runs one attempt, given the current retry state.
    fn call_op(&mut self, state: RetryState) -> Self::Future;
}

impl<Fut, O, F> AsyncRetryOp for F
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = O>,
{
    type Output = O;
    type Future = Fut;
    fn call_op(&mut self, state: RetryState) -> Fut {
        (self)(state)
    }
}

impl<Fut, O, F> AsyncRetryOp for StatelessOp<F>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = O>,
{
    type Output = O;
    type Future = Fut;
    fn call_op(&mut self, _state: RetryState) -> Fut {
        (self.0)()
    }
}
