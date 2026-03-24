//! Predicate trait and built-in retry predicate factories.
//!
//! This module provides the [`Predicate`] trait and the default retry predicates:
//! - [`any_error`] retries on any `Err`.
//! - [`error`] retries on selected errors.
//! - [`result`] retries based on the full `Result<T, E>`.
//! - [`ok`] retries on selected `Ok` values and treats any `Err` as terminal.
//!
//! Predicates compose with `|` ([`PredicateAny`]) and `&` ([`PredicateAll`]).

#[cfg(feature = "alloc")]
use crate::compat::Box;
use core::ops::{BitAnd, BitOr};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Examines the outcome of an operation and decides whether to retry.
///
/// `T` and `E` are type parameters on the trait, meaning each predicate is
/// typed to a specific operation's return type.
///
/// Composition methods are provided directly on the trait with
/// `where Self: Sized` bounds.
///
/// # Examples
///
/// ```
/// use tenacious::Predicate;
///
/// struct RetryOnError;
///
/// impl Predicate<String, &str> for RetryOnError {
///     fn should_retry(&self, outcome: &Result<String, &str>) -> bool {
///         outcome.is_err()
///     }
/// }
/// ```
pub trait Predicate<T, E> {
    /// Returns `true` if the retry loop should retry based on this outcome.
    fn should_retry(&self, outcome: &Result<T, E>) -> bool;

    /// Returns a predicate that retries when either side retries.
    #[must_use]
    fn or<P: Predicate<T, E>>(self, other: P) -> PredicateAny<Self, P>
    where
        Self: Sized,
    {
        PredicateAny::new(self, other)
    }

    /// Returns a predicate that retries only when both sides retry.
    #[must_use]
    fn and<P: Predicate<T, E>>(self, other: P) -> PredicateAll<Self, P>
    where
        Self: Sized,
    {
        PredicateAll::new(self, other)
    }
}

/// Blanket implementation allowing any `Fn(&Result<T, E>) -> bool` to serve
/// as a [`Predicate`]. This enables inline closure use:
///
/// ```
/// use tenacious::Predicate;
///
/// let pred = |outcome: &Result<i32, &str>| outcome.is_err();
/// let err: Result<i32, &str> = Err("boom");
/// assert!(pred.should_retry(&err));
/// ```
impl<T, E, F> Predicate<T, E> for F
where
    F: Fn(&Result<T, E>) -> bool,
{
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        (self)(outcome)
    }
}

#[cfg(feature = "alloc")]
impl<T, E> Predicate<T, E> for Box<dyn Predicate<T, E> + '_> {
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        (**self).should_retry(outcome)
    }
}

#[cfg(feature = "alloc")]
impl<T, E> Predicate<T, E> for Box<dyn Predicate<T, E> + Send + '_> {
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        (**self).should_retry(outcome)
    }
}

#[cfg(feature = "alloc")]
impl<T, E> Predicate<T, E> for Box<dyn Predicate<T, E> + Send + Sync + '_> {
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        (**self).should_retry(outcome)
    }
}

// ---------------------------------------------------------------------------
// Built-in predicate types and constructors
// ---------------------------------------------------------------------------

/// Predicate that retries on any error.
///
/// Created by [`any_error`].
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, predicate};
///
/// let predicate = predicate::any_error();
/// let outcome: Result<u32, &str> = Err("boom");
/// assert!(predicate.should_retry(&outcome));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PredicateAnyError;

/// Produces a predicate that retries on any `Err(_)` and accepts any `Ok(_)`.
#[must_use]
pub fn any_error() -> PredicateAnyError {
    PredicateAnyError
}

impl<T, E> Predicate<T, E> for PredicateAnyError {
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
/// use tenacious::{Predicate, predicate};
///
/// let predicate = predicate::error(|err: &&str| *err == "retryable");
/// let retryable: Result<u32, &str> = Err("retryable");
/// let fatal: Result<u32, &str> = Err("fatal");
///
/// assert!(predicate.should_retry(&retryable));
/// assert!(!predicate.should_retry(&fatal));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PredicateError<F> {
    matcher: F,
}

/// Produces a predicate that retries when `outcome` is `Err(e)` and
/// `matcher(e)` returns `true`.
#[must_use]
pub fn error<F>(matcher: F) -> PredicateError<F> {
    PredicateError { matcher }
}

impl<T, E, F> Predicate<T, E> for PredicateError<F>
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
/// use tenacious::{Predicate, predicate};
///
/// let predicate = predicate::result(|outcome: &Result<u32, &str>| {
///     matches!(outcome, Ok(value) if *value < 10)
/// });
///
/// assert!(predicate.should_retry(&Ok(3)));
/// assert!(!predicate.should_retry(&Ok(10)));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PredicateResult<F> {
    matcher: F,
}

/// Produces a predicate that retries when `matcher(outcome)` returns `true`.
#[must_use]
pub fn result<F>(matcher: F) -> PredicateResult<F> {
    PredicateResult { matcher }
}

impl<T, E, F> Predicate<T, E> for PredicateResult<F>
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
/// use tenacious::{Predicate, predicate};
///
/// let predicate = predicate::ok(|value: &u32| *value < 3);
///
/// assert!(predicate.should_retry(&Ok::<u32, &str>(2)));
/// assert!(!predicate.should_retry(&Ok::<u32, &str>(3)));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PredicateOk<F> {
    matcher: F,
}

/// Produces a predicate that retries when `outcome` is `Ok(value)` and
/// `matcher(value)` returns `true`.
///
/// Use this when `Err` outcomes should return immediately, and only selected
/// `Ok` values should continue retrying.
///
/// If the stop strategy fires while `ok(...)` is still asking for another
/// attempt, execution terminates with
/// [`RetryError::Exhausted`](crate::RetryError::Exhausted).
///
/// For polling flows:
/// - use `ok(|value| !is_ready(value))` when any `Err` should stop immediately
/// - combine it with [`error`] when only selected errors are retryable
/// - use [`result`] when the retry decision needs the full `Result<T, E>`
#[must_use]
pub fn ok<F>(matcher: F) -> PredicateOk<F> {
    PredicateOk { matcher }
}

impl<T, E, F> Predicate<T, E> for PredicateOk<F>
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

// ---------------------------------------------------------------------------
// PredicateUntil (negation wrapper)
// ---------------------------------------------------------------------------

/// Predicate that negates the inner predicate's decision.
///
/// Created by [`until`] or by calling `.until(p)` on a [`crate::RetryPolicy`]
/// or extension builder. Retries *until* the inner predicate returns `true`.
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, predicate};
///
/// let p = predicate::until(predicate::ok(|v: &u32| *v >= 10));
/// assert!(p.should_retry(&Ok::<u32, &str>(3)));   // not ready → retry
/// assert!(!p.should_retry(&Ok::<u32, &str>(10))); // ready → stop
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PredicateUntil<P> {
    inner: P,
}

/// Wraps a predicate so that its `should_retry()` result is negated.
///
/// The resulting predicate retries *until* the inner predicate returns `true`.
#[must_use]
pub fn until<P>(inner: P) -> PredicateUntil<P> {
    PredicateUntil { inner }
}

impl<T, E, P> Predicate<T, E> for PredicateUntil<P>
where
    P: Predicate<T, E>,
{
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        !self.inner.should_retry(outcome)
    }
}

// ---------------------------------------------------------------------------
// Composition types
// ---------------------------------------------------------------------------

/// Composite predicate that retries when **either** predicate retries.
///
/// Created by combining predicates with `|`, or via [`PredicateAny::new`].
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, predicate};
///
/// let p = predicate::error(|err: &&str| *err == "retryable") | predicate::ok(|value: &u32| *value < 2);
/// assert!(p.should_retry(&Err("retryable")));
/// assert!(p.should_retry(&Ok(1)));
/// assert!(!p.should_retry(&Ok(5)));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PredicateAny<A, B> {
    left: A,
    right: B,
}

impl<A, B> PredicateAny<A, B> {
    /// Creates a composite predicate that retries when either side retries.
    #[must_use]
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
///
/// # Examples
///
/// ```
/// use tenacious::{Predicate, predicate};
///
/// let p = predicate::result(|outcome: &Result<u32, &str>| outcome.is_err())
///     & predicate::error(|err: &&str| *err == "retryable");
///
/// assert!(p.should_retry(&Err("retryable")));
/// assert!(!p.should_retry(&Err("fatal")));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PredicateAll<A, B> {
    left: A,
    right: B,
}

impl<A, B> PredicateAll<A, B> {
    /// Creates a composite predicate that retries only when both sides retry.
    #[must_use]
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

/// Generates `BitOr` / `BitAnd` operator impls for a predicate type.
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

impl_predicate_ops!(PredicateAnyError);
impl_predicate_ops!(PredicateError<F>, F);
impl_predicate_ops!(PredicateResult<F>, F);
impl_predicate_ops!(PredicateOk<F>, F);
impl_predicate_ops!(PredicateAny<A, B>, A, B);
impl_predicate_ops!(PredicateAll<A, B>, A, B);
impl_predicate_ops!(PredicateUntil<P>, P);
