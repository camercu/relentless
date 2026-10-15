//! Classifiers built from a [`Predicate`]: the `.when` and `.until` adapters.

use super::Predicate;
use crate::decision::{Decide, Verdict};

/// Classifier from `.when(p)`: retry while the predicate wants to; otherwise
/// accept, returning an `Ok` and aborting on an `Err` with the bare error.
///
/// This bridges the `Result`-shaped [`Predicate`] world onto the classifier:
/// a rejected `Err(e)` becomes `Verdict::Abort(e)`, which the caller receives as
/// `RetryError::Aborted { last: e }`.
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
