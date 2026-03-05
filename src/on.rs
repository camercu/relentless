//! Built-in retry predicate factories and composition.
//!
//! This module provides the default retry predicates:
//! - [`any_error`] retries on any `Err`.
//! - [`error`] retries on selected errors.
//! - [`result`] retries based on the full `Result<T, E>`.
//! - [`ok`] retries on selected `Ok` values and treats any `Err` as terminal.
//! - [`until_ready`] retries on any error and on `Ok` values that are not ready.
//!
//! Predicates compose with `|` ([`PredicateAny`]) and `&` ([`PredicateAll`]).
//! Named combinators are also available via [`crate::PredicateExt`].

use crate::predicate::Predicate;
use core::ops::{BitAnd, BitOr};

/// Predicate that retries on any error.
///
/// Created by [`any_error`].
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, on};
///
/// let predicate = on::any_error();
/// let outcome: Result<u32, &str> = Err("boom");
/// assert!(predicate.should_retry(&outcome));
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AnyError;

/// Produces a predicate that retries on any `Err(_)` and accepts any `Ok(_)`.
pub fn any_error() -> AnyError {
    AnyError
}

impl AnyError {
    /// Returns a predicate that retries when either side retries.
    #[must_use]
    pub fn or<Rhs>(self, rhs: Rhs) -> PredicateAny<Self, Rhs> {
        PredicateAny::new(self, rhs)
    }

    /// Returns a predicate that retries only when both sides retry.
    #[must_use]
    pub fn and<Rhs>(self, rhs: Rhs) -> PredicateAll<Self, Rhs> {
        PredicateAll::new(self, rhs)
    }
}

impl<T, E> Predicate<T, E> for AnyError {
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        outcome.is_err()
    }
}

/// Predicate that retries when an `Err(e)` matches `matcher`.
///
/// Created by [`error`].
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, on};
///
/// let predicate = on::error(|err: &&str| *err == "retryable");
/// let retryable: Result<u32, &str> = Err("retryable");
/// let fatal: Result<u32, &str> = Err("fatal");
///
/// assert!(predicate.should_retry(&retryable));
/// assert!(!predicate.should_retry(&fatal));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ErrorPredicate<F> {
    matcher: F,
}

/// Produces a predicate that retries when `outcome` is `Err(e)` and
/// `matcher(e)` returns `true`.
pub fn error<F>(matcher: F) -> ErrorPredicate<F> {
    ErrorPredicate { matcher }
}

impl<F> ErrorPredicate<F> {
    /// Returns a predicate that retries when either side retries.
    #[must_use]
    pub fn or<Rhs>(self, rhs: Rhs) -> PredicateAny<Self, Rhs> {
        PredicateAny::new(self, rhs)
    }

    /// Returns a predicate that retries only when both sides retry.
    #[must_use]
    pub fn and<Rhs>(self, rhs: Rhs) -> PredicateAll<Self, Rhs> {
        PredicateAll::new(self, rhs)
    }
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
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, on};
///
/// let predicate = on::result(|outcome: &Result<u32, &str>| {
///     matches!(outcome, Ok(value) if *value < 10)
/// });
///
/// assert!(predicate.should_retry(&Ok(3)));
/// assert!(!predicate.should_retry(&Ok(10)));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResultPredicate<F> {
    matcher: F,
}

/// Produces a predicate that retries when `matcher(outcome)` returns `true`.
pub fn result<F>(matcher: F) -> ResultPredicate<F> {
    ResultPredicate { matcher }
}

impl<F> ResultPredicate<F> {
    /// Returns a predicate that retries when either side retries.
    #[must_use]
    pub fn or<Rhs>(self, rhs: Rhs) -> PredicateAny<Self, Rhs> {
        PredicateAny::new(self, rhs)
    }

    /// Returns a predicate that retries only when both sides retry.
    #[must_use]
    pub fn and<Rhs>(self, rhs: Rhs) -> PredicateAll<Self, Rhs> {
        PredicateAll::new(self, rhs)
    }
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
///
/// Behavior:
///
/// | Outcome | Retries? |
/// | --- | --- |
/// | `Err(e)` | no |
/// | `Ok(v)` and `matcher(v) == true` | yes |
/// | `Ok(v)` and `matcher(v) == false` | no |
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, on};
///
/// let predicate = on::ok(|value: &u32| *value < 3);
///
/// assert!(predicate.should_retry(&Ok::<u32, &str>(2)));
/// assert!(!predicate.should_retry(&Ok::<u32, &str>(3)));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OkPredicate<F> {
    matcher: F,
}

/// Produces a predicate that retries when `outcome` is `Ok(value)` and
/// `matcher(value)` returns `true`.
///
/// Use this when `Err` outcomes should return immediately, and only selected
/// `Ok` values should continue retrying.
pub fn ok<F>(matcher: F) -> OkPredicate<F> {
    OkPredicate { matcher }
}

impl<F> OkPredicate<F> {
    /// Returns a predicate that retries when either side retries.
    #[must_use]
    pub fn or<Rhs>(self, rhs: Rhs) -> PredicateAny<Self, Rhs> {
        PredicateAny::new(self, rhs)
    }

    /// Returns a predicate that retries only when both sides retry.
    #[must_use]
    pub fn and<Rhs>(self, rhs: Rhs) -> PredicateAll<Self, Rhs> {
        PredicateAll::new(self, rhs)
    }
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

/// Predicate for wait-for-condition flows that retries on transient errors and
/// on `Ok` values that are not yet ready.
///
/// Created by [`until_ready`].
///
/// Behavior:
///
/// | Outcome | Retries? |
/// | --- | --- |
/// | `Err(e)` | yes |
/// | `Ok(v)` and `is_ready(v) == false` | yes |
/// | `Ok(v)` and `is_ready(v) == true` | no |
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, on};
///
/// let predicate = on::until_ready(|value: &u32| *value >= 3);
///
/// assert!(predicate.should_retry(&Err::<u32, &str>("transient")));
/// assert!(predicate.should_retry(&Ok::<u32, &str>(1)));
/// assert!(!predicate.should_retry(&Ok::<u32, &str>(3)));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UntilReadyPredicate<F> {
    is_ready: F,
}

/// Produces a predicate that retries while the condition is not met yet.
///
/// This helper is tuned for polling loops:
/// - retries on any `Err(_)`
/// - retries on `Ok(value)` when `is_ready(value)` is `false`
/// - accepts `Ok(value)` when `is_ready(value)` is `true`
///
/// Equivalent behavior:
/// `until_ready(is_ready) == any_error() | ok(|value| !is_ready(value))`
pub fn until_ready<F>(is_ready: F) -> UntilReadyPredicate<F> {
    UntilReadyPredicate { is_ready }
}

impl<T, E, F> Predicate<T, E> for UntilReadyPredicate<F>
where
    F: Fn(&T) -> bool,
{
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        match outcome {
            Ok(value) => !(self.is_ready)(value),
            Err(_) => true,
        }
    }
}

/// Composite predicate that retries when **either** predicate retries.
///
/// Created by combining predicates with `|`, or via [`PredicateAny::new`].
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, on};
///
/// let predicate = on::error(|err: &&str| *err == "retryable") | on::ok(|value: &u32| *value < 2);
/// assert!(predicate.should_retry(&Err("retryable")));
/// assert!(predicate.should_retry(&Ok(1)));
/// assert!(!predicate.should_retry(&Ok(5)));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PredicateAny<A, B> {
    left: A,
    right: B,
}

impl<A, B> PredicateAny<A, B> {
    /// Creates a composite predicate that retries when either side retries.
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }

    /// Returns a predicate that retries when either side retries.
    #[must_use]
    pub fn or<Rhs>(self, rhs: Rhs) -> PredicateAny<Self, Rhs> {
        PredicateAny::new(self, rhs)
    }

    /// Returns a predicate that retries only when both sides retry.
    #[must_use]
    pub fn and<Rhs>(self, rhs: Rhs) -> PredicateAll<Self, Rhs> {
        PredicateAll::new(self, rhs)
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
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, on};
///
/// let predicate = on::result(|outcome: &Result<u32, &str>| outcome.is_err())
///     & on::error(|err: &&str| *err == "retryable");
///
/// assert!(predicate.should_retry(&Err("retryable")));
/// assert!(!predicate.should_retry(&Err("fatal")));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PredicateAll<A, B> {
    left: A,
    right: B,
}

impl<A, B> PredicateAll<A, B> {
    /// Creates a composite predicate that retries only when both sides retry.
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }

    /// Returns a predicate that retries when either side retries.
    #[must_use]
    pub fn or<Rhs>(self, rhs: Rhs) -> PredicateAny<Self, Rhs> {
        PredicateAny::new(self, rhs)
    }

    /// Returns a predicate that retries only when both sides retry.
    #[must_use]
    pub fn and<Rhs>(self, rhs: Rhs) -> PredicateAll<Self, Rhs> {
        PredicateAll::new(self, rhs)
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
impl_predicate_ops!(UntilReadyPredicate<F>, F);
impl_predicate_ops!(PredicateAny<A, B>, A, B);
impl_predicate_ops!(PredicateAll<A, B>, A, B);
