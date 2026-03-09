//! Predicate trait — determines which outcomes should trigger a retry.

#[cfg(feature = "alloc")]
use crate::compat::Box;

/// Examines the outcome of an operation and decides whether to retry.
///
/// `T` and `E` are type parameters on the trait, meaning each predicate is
/// typed to a specific operation's return type.
///
/// # Examples
///
/// ```
/// use tenacious::Predicate;
///
/// struct RetryOnError;
///
/// impl Predicate<String, &str> for RetryOnError {
///     fn should_retry(&mut self, outcome: &Result<String, &str>) -> bool {
///         outcome.is_err()
///     }
/// }
/// ```
pub trait Predicate<T, E> {
    /// Returns `true` if the retry loop should retry based on this outcome.
    fn should_retry(&mut self, outcome: &Result<T, E>) -> bool;
}

/// Ergonomic named combinators for [`Predicate`] composition.
///
/// These are equivalent to the operator forms:
/// - `.or(other)` is the same as `|`.
/// - `.and(other)` is the same as `&`.
pub trait PredicateExt<T, E>: Predicate<T, E> + Sized {
    /// Returns a predicate that retries when either side retries.
    #[must_use]
    fn or<Rhs>(self, rhs: Rhs) -> crate::on::PredicateAny<Self, Rhs> {
        crate::on::PredicateAny::new(self, rhs)
    }

    /// Returns a predicate that retries only when both sides retry.
    #[must_use]
    fn and<Rhs>(self, rhs: Rhs) -> crate::on::PredicateAll<Self, Rhs> {
        crate::on::PredicateAll::new(self, rhs)
    }
}

impl<T, E, P> PredicateExt<T, E> for P where P: Predicate<T, E> + Sized {}

/// Blanket implementation allowing any `Fn(&Result<T, E>) -> bool` to serve
/// as a [`Predicate`]. This enables inline closure use:
///
/// ```
/// use tenacious::Predicate;
///
/// let mut pred = |outcome: &Result<i32, &str>| outcome.is_err();
/// let err: Result<i32, &str> = Err("boom");
/// assert!(pred.should_retry(&err));
/// ```
impl<T, E, F> Predicate<T, E> for F
where
    F: FnMut(&Result<T, E>) -> bool,
{
    fn should_retry(&mut self, outcome: &Result<T, E>) -> bool {
        (self)(outcome)
    }
}

#[cfg(feature = "alloc")]
impl<T, E> Predicate<T, E> for Box<dyn Predicate<T, E>> {
    fn should_retry(&mut self, outcome: &Result<T, E>) -> bool {
        (**self).should_retry(outcome)
    }
}
