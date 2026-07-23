//! Predicate trait and built-in retry predicate factories.
//!
//! This module provides the [`Predicate`] trait and the default retry predicates:
//! - [`any_error`] retries on any `Err`.
//! - [`error`] retries on selected errors.
//! - [`result`] retries based on the full `Result<T, E>`.
//! - [`ok`] retries on selected `Ok` values and treats any `Err` as terminal.
//!
//! These are `Result`-shaped sugar over the classifier used by
//! [`Retry::when`](crate::Retry::when) / [`Retry::until`](crate::Retry::until).
//! For conditions that need boolean composition, put the logic inside a
//! [`result`] closure (`result(|o| a(o) || b(o))`); for full return / retry /
//! abort control, use [`Retry::decide`](crate::Retry::decide).

#[cfg(feature = "alloc")]
use crate::compat::Box;

/// Examines the outcome of an operation and decides whether to retry.
///
/// Any `Fn(&Result<T, E>) -> bool` is a `Predicate` via the blanket impl, so
/// the built-in factories ([`any_error`], [`error`], [`ok`], [`result`]) and
/// plain closures both work with `.when` / `.until`.
///
/// # Examples
///
/// ```
/// use relentless::Predicate;
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
}

/// Any `Fn(&Result<T, E>) -> bool` can be used directly as a [`Predicate`],
/// without wrapping in a named type.
///
/// ```
/// use relentless::Predicate;
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

/// Predicate that retries on any error.
///
/// Created by [`any_error`].
///
/// # Examples
///
/// ```
/// use relentless::{Predicate, predicate};
///
/// let predicate = predicate::any_error();
/// let outcome: Result<u32, &str> = Err("boom");
/// assert!(predicate.should_retry(&outcome));
/// ```
#[derive(Debug, Clone)]
pub struct PredicateAnyError;

/// Creates a predicate that retries on any `Err` value.
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
/// use relentless::{Predicate, predicate};
///
/// let predicate = predicate::error(|err: &&str| *err == "retryable");
/// let retryable: Result<u32, &str> = Err("retryable");
/// let fatal: Result<u32, &str> = Err("fatal");
///
/// assert!(predicate.should_retry(&retryable));
/// assert!(!predicate.should_retry(&fatal));
/// ```
#[derive(Debug, Clone)]
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
/// use relentless::{Predicate, predicate};
///
/// let predicate = predicate::result(|outcome: &Result<u32, &str>| {
///     matches!(outcome, Ok(value) if *value < 10)
/// });
///
/// assert!(predicate.should_retry(&Ok(3)));
/// assert!(!predicate.should_retry(&Ok(10)));
/// ```
#[derive(Debug, Clone)]
pub struct PredicateResult<F> {
    matcher: F,
}

/// Creates a predicate that retries when `matcher` returns `true` for the outcome.
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
/// use relentless::{Predicate, predicate};
///
/// let predicate = predicate::ok(|value: &u32| *value < 3);
///
/// assert!(predicate.should_retry(&Ok::<u32, &str>(2)));
/// assert!(!predicate.should_retry(&Ok::<u32, &str>(3)));
/// ```
#[derive(Debug, Clone)]
pub struct PredicateOk<F> {
    matcher: F,
}

/// Produces a predicate that retries when `outcome` is `Ok(value)` and
/// `matcher(value)` returns `true`. It never fires on `Err`.
///
/// How that maps onto `Err` at the engine level depends on which classifier
/// consumes the predicate:
///
/// - [`.until(ok(is_ready))`](crate::Retry::until) is the natural polling form:
///   the loop retries on everything *except* a matching `Ok`, so a matching `Ok`
///   returns while any `Err` keeps polling — transient poll failures are retried,
///   not surfaced.
/// - [`.when(ok(m))`](crate::Retry::when) retries only the matching `Ok`; a
///   non-matching `Ok` returns and an `Err` aborts with
///   [`RetryError::Aborted`](crate::RetryError::Aborted).
///
/// If the stop strategy fires while a retry is still wanted, execution
/// terminates with [`RetryError::Exhausted`](crate::RetryError::Exhausted).
///
/// To make a poll `Err` terminal rather than retried, classify the whole outcome
/// with [`Retry::decide`](crate::Retry::decide), or use [`result`] when the
/// decision needs the full `Result<T, E>`.
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
