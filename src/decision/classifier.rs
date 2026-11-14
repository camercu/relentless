//! Classifiers: the [`Decide`] trait the engine drives, and the classifiers
//! the crate installs (default, `.decide`, `.when`, `.until`).

use super::{IntoDecision, Outcome, Verdict, sealed};

/// The engine-facing classifier trait, carried in the builder's classifier
/// slot and driven by the retry loop. Users name it only in the bounds of code
/// generic over a configured builder (`C: Decide<O>`), where `C::R` and `C::A`
/// spell the result types.
///
/// A classifier consumes each outcome **by value** and returns a [`Verdict`].
/// The `&self` receiver lets one classifier serve every attempt of a loop.
///
/// Sealed: the only implementations are the crate's own classifiers — the
/// default one and those `.decide`, `.when` and `.until` install. No other
/// crate can add one:
///
/// ```compile_fail
/// use relentless::{Decide, Verdict};
///
/// struct Mine;
///
/// impl Decide<u32> for Mine {
///     type R = u32;
///     type A = u32;
///     fn decide(&self, outcome: u32) -> Verdict<u32, u32, u32> {
///         Verdict::Return(outcome)
///     }
/// }
/// ```
pub trait Decide<O>: sealed::SealedDecide {
    /// The value produced on `Return` (what `Ok` carries).
    type R;
    /// The value produced on `Abort` (what `RetryError::Aborted { last }` carries).
    type A;

    /// Consumes an outcome and decides its fate.
    fn decide(&self, outcome: O) -> Verdict<Self::R, Self::A, O>;
}

impl<C: sealed::SealedDecide + ?Sized> sealed::SealedDecide for &C {}

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
/// any operation exists. The operation pins `O` at `.call()`, where the
/// `O: Outcome` bound applies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DefaultClassifier;

impl sealed::SealedDecide for DefaultClassifier {}

impl<O: Outcome> Decide<O> for DefaultClassifier {
    type R = O::Return;
    type A = O::Abort;

    fn decide(&self, outcome: O) -> Verdict<O::Return, O::Abort, O> {
        outcome.classify()
    }
}

/// Wraps any `Fn(O) -> impl IntoDecision<O>` closure as a classifier.
///
/// Installed by `.decide(closure)`. One struct, one impl; the closure's return
/// type (`Decision` or `Verdict`) selects the [`IntoDecision`] impl, so there
/// is no coherence overlap between the two currencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureClassifier<C>(pub(crate) C);

impl<C> sealed::SealedDecide for ClosureClassifier<C> {}

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

/// Classifier from `.when(f)`: retry an error while `f` says it is worth
/// retrying.
///
/// An `Ok` returns. An `Err(e)` retries when `f(&e)` is `true` and otherwise
/// aborts with the bare error, which the caller receives as
/// `RetryError::Aborted { last: e }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct When<F>(pub(crate) F);

impl<F> sealed::SealedDecide for When<F> {}

impl<T, E, F> Decide<Result<T, E>> for When<F>
where
    F: Fn(&E) -> bool,
{
    type R = T;
    type A = E;

    fn decide(&self, outcome: Result<T, E>) -> Verdict<T, E, Result<T, E>> {
        match outcome {
            Ok(value) => Verdict::Return(value),
            Err(error) if (self.0)(&error) => Verdict::Retry(Err(error)),
            Err(error) => Verdict::Abort(error),
        }
    }
}

/// Classifier from `.until(f)`: retry until an `Ok` value satisfies `f`.
///
/// An `Ok(v)` returns when `f(&v)` is `true` and retries otherwise. Every
/// `Err` retries, so the loop never aborts. Its abort type stays `E`, so both
/// setters return the same [`RetryResult`](crate::RetryResult) shape and the
/// `Result` helpers such as `last_error` apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Until<F>(pub(crate) F);

impl<F> sealed::SealedDecide for Until<F> {}

impl<T, E, F> Decide<Result<T, E>> for Until<F>
where
    F: Fn(&T) -> bool,
{
    type R = T;
    type A = E;

    fn decide(&self, outcome: Result<T, E>) -> Verdict<T, E, Result<T, E>> {
        match outcome {
            Ok(value) if (self.0)(&value) => Verdict::Return(value),
            other => Verdict::Retry(other),
        }
    }
}
