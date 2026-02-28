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
///     fn should_retry(&self, outcome: &Result<String, &str>) -> bool {
///         outcome.is_err()
///     }
/// }
/// ```
pub trait Predicate<T, E> {
    /// Returns `true` if the retry loop should retry based on this outcome.
    fn should_retry(&self, outcome: &Result<T, E>) -> bool;
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
impl<T, E> Predicate<T, E> for Box<dyn Predicate<T, E>> {
    fn should_retry(&self, outcome: &Result<T, E>) -> bool {
        (**self).should_retry(outcome)
    }
}
