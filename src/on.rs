//! Built-in retry predicate factories and composition.
//!
//! This module provides the default retry predicates:
//! - [`any_error`] retries on any `Err`.
//! - [`error`] retries on selected errors.
//! - [`result`] retries based on the full `Result<T, E>`.
//! - [`ok`] retries on selected `Ok` values (polling/wait-for pattern).
//!
//! Predicates compose with `|` ([`PredicateAny`]) and `&` ([`PredicateAll`]).

use crate::predicate::Predicate;
use core::ops::{BitAnd, BitOr};

/// Predicate that retries on any error.
///
/// Created by [`any_error`].
#[derive(Debug, Clone, Copy, Default)]
pub struct AnyError;

/// Produces a predicate that retries on any `Err(_)` and accepts any `Ok(_)`.
pub fn any_error() -> AnyError {
    AnyError
}

impl<T, E> Predicate<T, E> for AnyError {
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        outcome.is_err()
    }
}

/// Predicate that retries when an `Err(e)` matches `matcher`.
///
/// Created by [`error`].
#[derive(Debug, Clone)]
pub struct ErrorPredicate<F> {
    matcher: F,
}

/// Produces a predicate that retries when `outcome` is `Err(e)` and
/// `matcher(e)` returns `true`.
pub fn error<F>(matcher: F) -> ErrorPredicate<F> {
    ErrorPredicate { matcher }
}

impl<T, E, F> Predicate<T, E> for ErrorPredicate<F>
where
    F: Fn(&E) -> bool,
{
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        match outcome {
            Ok(_) => false,
            Err(error) => (self.matcher)(error),
        }
    }
}

/// Predicate that retries based on the full `Result<T, E>`.
///
/// Created by [`result`].
#[derive(Debug, Clone)]
pub struct ResultPredicate<F> {
    matcher: F,
}

/// Produces a predicate that retries when `matcher(outcome)` returns `true`.
pub fn result<F>(matcher: F) -> ResultPredicate<F> {
    ResultPredicate { matcher }
}

impl<T, E, F> Predicate<T, E> for ResultPredicate<F>
where
    F: Fn(&Result<T, E>) -> bool,
{
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        (self.matcher)(outcome)
    }
}

/// Predicate that retries when an `Ok(value)` matches `matcher`.
///
/// Created by [`ok`].
#[derive(Debug, Clone)]
pub struct OkPredicate<F> {
    matcher: F,
}

/// Produces a predicate that retries when `outcome` is `Ok(value)` and
/// `matcher(value)` returns `true`.
pub fn ok<F>(matcher: F) -> OkPredicate<F> {
    OkPredicate { matcher }
}

impl<T, E, F> Predicate<T, E> for OkPredicate<F>
where
    F: Fn(&T) -> bool,
{
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        match outcome {
            Ok(value) => (self.matcher)(value),
            Err(_) => false,
        }
    }
}

/// Composite predicate that retries when **either** predicate retries.
///
/// Created by combining predicates with `|`, or via [`PredicateAny::new`].
#[derive(Debug, Clone)]
pub struct PredicateAny<A, B> {
    left: A,
    right: B,
}

impl<A, B> PredicateAny<A, B> {
    /// Creates a composite predicate that retries when either side retries.
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }
}

impl<T, E, A, B> Predicate<T, E> for PredicateAny<A, B>
where
    A: Predicate<T, E>,
    B: Predicate<T, E>,
{
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        self.left.should_retry(outcome) || self.right.should_retry(outcome)
    }
}

/// Composite predicate that retries only when **both** predicates retry.
///
/// Created by combining predicates with `&`, or via [`PredicateAll::new`].
#[derive(Debug, Clone)]
pub struct PredicateAll<A, B> {
    left: A,
    right: B,
}

impl<A, B> PredicateAll<A, B> {
    /// Creates a composite predicate that retries only when both sides retry.
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }
}

impl<T, E, A, B> Predicate<T, E> for PredicateAll<A, B>
where
    A: Predicate<T, E>,
    B: Predicate<T, E>,
{
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        self.left.should_retry(outcome) && self.right.should_retry(outcome)
    }
}

// ---------------------------------------------------------------------------
// Predicate composition operator impls
// ---------------------------------------------------------------------------

/// Generates `BitOr` and `BitAnd` impls for a predicate type, producing
/// [`PredicateAny`] and [`PredicateAll`] composites respectively.
///
/// Each invocation takes `($ty:ty $(, $param:ident)*)` where `$param` lists
/// any generic parameters the type carries (e.g. `F` for `ErrorPredicate<F>`).
macro_rules! impl_predicate_ops {
    ($ty:ty $(, $param:ident)*) => {
        impl<$($param,)* Rhs> BitOr<Rhs> for $ty {
            type Output = PredicateAny<Self, Rhs>;

            fn bitor(self, rhs: Rhs) -> Self::Output {
                PredicateAny::new(self, rhs)
            }
        }

        impl<$($param,)* Rhs> BitAnd<Rhs> for $ty {
            type Output = PredicateAll<Self, Rhs>;

            fn bitand(self, rhs: Rhs) -> Self::Output {
                PredicateAll::new(self, rhs)
            }
        }
    };
}

impl_predicate_ops!(AnyError);
impl_predicate_ops!(ErrorPredicate<F>, F);
impl_predicate_ops!(ResultPredicate<F>, F);
impl_predicate_ops!(OkPredicate<F>, F);
impl_predicate_ops!(PredicateAny<A, B>, A, B);
impl_predicate_ops!(PredicateAll<A, B>, A, B);
