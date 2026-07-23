//! The outcome-classification layer.
//!
//! Where the engine once asked a boolean [`Predicate`](crate::Predicate) "should
//! this outcome retry?", it now asks a *classifier* to sort each outcome into a
//! three-way [`Verdict`]: return it to the caller, retry it, or abort with a
//! projected payload. This lets the retry decision be independent of
//! `Result<T, E>` semantics — a sought-after `Err`, a non-`Result` poll enum, or
//! a search state can each drive the loop directly.
//!
//! This module is the vocabulary; the engine that consumes it lives in
//! [`crate::engine`].

use crate::predicate::Predicate;
use core::convert::Infallible;

/// A two-way decision about a completed outcome: accept it or try again.
///
/// The common-case classifier currency — polling, searching, inverted probes —
/// where no outcome is ever fatal. It has no abort type parameter, so a closure
/// returning `Decision` can never leave an abort type unconstrained. Reach for
/// [`Verdict`] when you need an `Abort` arm; the `Return`/`Retry` variants are
/// spelled the same, so upgrading is a one-word change plus the new arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision<R, O> {
    /// Accept this outcome as success. `call()` yields `Ok(value)`.
    Return(R),
    /// Try again; the whole outcome is handed back to the engine.
    Retry(O),
}

/// A three-way decision about a completed outcome: accept it, retry it, or abort.
///
/// `O` is the whole outcome type the operation produces; `R` is what the caller
/// receives on success (`Ok(R)`); `A` is the payload projected on a fatal abort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict<R, A, O> {
    /// Accept this outcome as success. `call()` yields `Ok(value)`.
    Return(R),
    /// Try again; the whole outcome is handed back to the engine.
    Retry(O),
    /// Reject as fatal. `call()` yields `Err(RetryError::Aborted { last })`.
    Abort(A),
}

/// Projects an owned outcome type into a [`Verdict`].
///
/// Implement this for a type you own to make it classify itself, so the default
/// engine path needs no `.decide(...)` closure at the call site. A blanket impl
/// covers `Result<T, E>` with today's semantics: `Ok(v)` returns `v`, any `Err`
/// retries.
///
/// # Orphan-rule note
///
/// The blanket `impl<T, E> Outcome for Result<T, E>` already covers every
/// `Result`, so you cannot write your own `impl Outcome for Result<MyT, MyE>`.
/// For custom `Result` classification, use `.decide(closure)` or wrap the
/// `Result` in a newtype you own.
pub trait Outcome: Sized {
    /// The value delivered to the caller on `Return` (what `Ok` carries).
    type Return;
    /// The value delivered on `Abort` (what `RetryError::Aborted { last }` carries).
    type Abort;

    /// Classifies this outcome into a three-way [`Verdict`].
    fn classify(self) -> Verdict<Self::Return, Self::Abort, Self>;
}

/// Default: `Ok(v)` → `Return(v)`, any `Err` → `Retry`.
///
/// This encodes today's engine semantics — every error is retried, so the loop
/// terminates on an error only by exhausting its stop strategy
/// (`RetryError::Exhausted`), never by aborting on the default path. `Abort` is
/// typed as `E` (not `Infallible`) so the default and `.when`/`.until` paths
/// share one `RetryError<E, Result<T, E>>` shape.
impl<T, E> Outcome for Result<T, E> {
    type Return = T;
    type Abort = E;

    fn classify(self) -> Verdict<T, E, Result<T, E>> {
        match self {
            Ok(value) => Verdict::Return(value),
            Err(_) => Verdict::Retry(self),
        }
    }
}

/// The engine-facing classifier trait. Users never name or implement it; it is
/// carried in the builder's classifier slot and driven by the retry loop.
///
/// A classifier consumes each outcome **by value** and returns a [`Verdict`].
/// The `&self` receiver lets one classifier serve every attempt of a loop.
pub trait Decide<O> {
    /// The value produced on `Return` (what `Ok` carries).
    type R;
    /// The value produced on `Abort` (what `RetryError::Aborted { last }` carries).
    type A;

    /// Consumes an outcome and decides its fate.
    fn decide(&self, outcome: O) -> Verdict<Self::R, Self::A, O>;
}

/// A shared reference to a classifier is itself one, so a builder can borrow a
/// classifier stored in a reusable [`RetryPolicy`](crate::RetryPolicy).
impl<O, C: Decide<O> + ?Sized> Decide<O> for &C {
    type R = C::R;
    type A = C::A;

    fn decide(&self, outcome: O) -> Verdict<C::R, C::A, O> {
        (**self).decide(outcome)
    }
}

/// The default classifier slot: delegates to `O: Outcome`.
///
/// Carries no outcome type of its own, so it can sit in a policy built before
/// any operation exists; `O` is pinned by the operation at `.call()`, where the
/// `O: Outcome` bound is required (mirroring the old `P: Predicate<T, E>` bound).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DefaultClassifier;

impl<O: Outcome> Decide<O> for DefaultClassifier {
    type R = O::Return;
    type A = O::Abort;

    fn decide(&self, outcome: O) -> Verdict<O::Return, O::Abort, O> {
        outcome.classify()
    }
}

mod sealed {
    pub trait Sealed {}
}

/// Unifies [`Decision`] and [`Verdict`] under one `.decide` method.
///
/// A `.decide` closure returns either enum; this trait converts it into the
/// engine's canonical [`Verdict`]. It is sealed — users return one of the two
/// enums, never implement this themselves.
pub trait IntoDecision<O>: sealed::Sealed {
    /// The value produced on `Return`.
    type R;
    /// The value produced on `Abort`.
    type A;
    /// Converts into the canonical three-way form.
    fn into_verdict(self) -> Verdict<Self::R, Self::A, O>;
}

impl<R, O> sealed::Sealed for Decision<R, O> {}
impl<R, A, O> sealed::Sealed for Verdict<R, A, O> {}

impl<R, O> IntoDecision<O> for Decision<R, O> {
    type R = R;
    // A no-abort decision has no abort payload; `Infallible` makes the abort
    // arm uninhabited.
    type A = Infallible;

    fn into_verdict(self) -> Verdict<R, Infallible, O> {
        match self {
            Decision::Return(r) => Verdict::Return(r),
            Decision::Retry(o) => Verdict::Retry(o),
        }
    }
}

impl<R, A, O> IntoDecision<O> for Verdict<R, A, O> {
    type R = R;
    type A = A;

    fn into_verdict(self) -> Verdict<R, A, O> {
        self
    }
}

/// Wraps any `Fn(O) -> impl IntoDecision<O>` closure as a classifier.
///
/// Installed by `.decide(closure)`. One struct, one impl; the closure's return
/// type (`Decision` or `Verdict`) selects the [`IntoDecision`] impl, so there
/// is no coherence overlap between the two currencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureClassifier<C>(pub(crate) C);

impl<O, D, C> Decide<O> for ClosureClassifier<C>
where
    C: Fn(O) -> D,
    D: IntoDecision<O>,
{
    type R = D::R;
    type A = D::A;

    fn decide(&self, outcome: O) -> Verdict<D::R, D::A, O> {
        (self.0)(outcome).into_verdict()
    }
}

/// Classifier from `.when(p)`: retry while the predicate wants to; otherwise
/// accept, returning an `Ok` and aborting on an `Err` with the bare error.
///
/// This bridges the `Result`-shaped [`Predicate`] world onto the classifier:
/// a rejected `Err(e)` becomes `Verdict::Abort(e)` (the payload the old engine
/// reported as `RetryError::Rejected`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct When<P>(pub(crate) P);

impl<P> When<P> {
    pub(crate) fn new(predicate: P) -> Self {
        When(predicate)
    }
}

impl<T, E, P> Decide<Result<T, E>> for When<P>
where
    P: Predicate<T, E>,
{
    type R = T;
    type A = E;

    fn decide(&self, outcome: Result<T, E>) -> Verdict<T, E, Result<T, E>> {
        if self.0.should_retry(&outcome) {
            Verdict::Retry(outcome)
        } else {
            match outcome {
                Ok(value) => Verdict::Return(value),
                Err(error) => Verdict::Abort(error),
            }
        }
    }
}

/// Classifier from `.until(p)`: the inverse of [`When`] — retry *until* the
/// predicate is satisfied, then accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Until<P>(pub(crate) P);

impl<P> Until<P> {
    pub(crate) fn new(predicate: P) -> Self {
        Until(predicate)
    }
}

impl<T, E, P> Decide<Result<T, E>> for Until<P>
where
    P: Predicate<T, E>,
{
    type R = T;
    type A = E;

    fn decide(&self, outcome: Result<T, E>) -> Verdict<T, E, Result<T, E>> {
        if self.0.should_retry(&outcome) {
            match outcome {
                Ok(value) => Verdict::Return(value),
                Err(error) => Verdict::Abort(error),
            }
        } else {
            Verdict::Retry(outcome)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_ok_classifies_as_return() {
        let outcome: Result<i32, &str> = Ok(7);
        assert_eq!(outcome.classify(), Verdict::Return(7));
    }

    #[test]
    fn result_err_classifies_as_retry_carrying_the_whole_outcome() {
        let outcome: Result<i32, &str> = Err("boom");
        assert_eq!(outcome.classify(), Verdict::Retry(Err("boom")));
    }

    #[test]
    fn default_classifier_delegates_to_outcome() {
        let classifier = DefaultClassifier;
        assert_eq!(classifier.decide(Ok::<i32, &str>(1)), Verdict::Return(1));
        assert_eq!(
            classifier.decide(Err::<i32, &str>("x")),
            Verdict::Retry(Err("x"))
        );
    }
}
