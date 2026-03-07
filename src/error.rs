//! RetryError — the error type returned when a retry loop terminates without success.

use crate::compat::Duration;
use core::fmt;

/// Error returned when a retry loop terminates without producing an accepted result.
///
/// # Type Parameters
///
/// - `E`: The error type from the retried operation.
/// - `T`: The `Ok` value type. Defaults to `()` for the common retry-on-error case.
///
/// The parameter order `<E, T>` is intentionally reversed from `Result<T, E>`
/// so that `T` can default to `()`. Rust requires default type parameters to
/// be trailing, and this ordering enables the common `RetryError<MyError>`
/// shorthand without specifying `T`.
///
/// # Examples
///
/// ```
/// use tenacious::RetryError;
/// use core::time::Duration;
///
/// // Common case: T defaults to ()
/// let err: RetryError<String> = RetryError::Exhausted {
///     last: Err("connection refused".to_string()),
///     attempts: 3,
///     total_elapsed: Some(Duration::from_secs(5)),
/// };
///
/// println!("{}", err);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryError<E, T = ()> {
    /// All retries exhausted; the operation kept returning `Err`.
    Exhausted {
        /// The final attempt outcome. In normal `on::error` usage this is `Err(E)`.
        last: Result<T, E>,
        /// Total number of attempts made.
        attempts: u32,
        /// Wall-clock time elapsed, or `None` if no clock was available.
        total_elapsed: Option<Duration>,
    },

    /// The predicate rejected an `Err` outcome, so retrying stopped immediately.
    ///
    /// This occurs when using a custom predicate (for example `on::error`) that
    /// classifies some errors as non-retryable.
    PredicateRejected {
        /// The rejected attempt outcome. In normal usage this is `Err(E)`.
        last: Result<T, E>,
        /// Total number of attempts made.
        attempts: u32,
        /// Wall-clock time elapsed, or `None` if no clock was available.
        total_elapsed: Option<Duration>,
    },

    /// The stop condition fired while the predicate was still rejecting `Ok` values.
    /// This variant is used when `on::ok` or `on::result` predicates cause retries
    /// on `Ok` values and the stop condition fires before the predicate accepts.
    ConditionNotMet {
        /// The final attempt outcome. In normal `on::ok` usage this is `Ok(T)`.
        last: Result<T, E>,
        /// Total number of attempts made.
        attempts: u32,
        /// Wall-clock time elapsed, or `None` if no clock was available.
        total_elapsed: Option<Duration>,
    },

    /// An external cancellation signal interrupted the retry loop.
    ///
    /// Cancellation is checked before each attempt and during sleeps.
    Cancelled {
        /// The outcome from the most recent completed attempt, or `None` when
        /// cancellation fires before the first attempt.
        last: Option<Result<T, E>>,
        /// Total number of attempts that completed before cancellation.
        attempts: u32,
        /// Wall-clock time elapsed, or `None` if no clock was available.
        total_elapsed: Option<Duration>,
    },
}

/// Convenience alias for retry-returning operations.
///
/// Expands to `Result<T, RetryError<E, T>>`.
pub type RetryResult<T, E> = core::result::Result<T, RetryError<E, T>>;

impl<E: fmt::Display, T: fmt::Debug> fmt::Display for RetryError<E, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RetryError::Exhausted {
                last,
                attempts,
                total_elapsed,
            } => match last {
                Err(error) => write!(
                    f,
                    "retry exhausted after {} attempt(s) (elapsed: {:?}): {}",
                    attempts, total_elapsed, error
                ),
                Ok(value) => write!(
                    f,
                    "retry exhausted after {} attempt(s) (elapsed: {:?}): last value = {:?}",
                    attempts, total_elapsed, value
                ),
            },
            RetryError::ConditionNotMet {
                last,
                attempts,
                total_elapsed,
            } => match last {
                Ok(value) => write!(
                    f,
                    "condition not met after {} attempt(s) (elapsed: {:?}): last value = {:?}",
                    attempts, total_elapsed, value
                ),
                Err(error) => write!(
                    f,
                    "condition not met after {} attempt(s) (elapsed: {:?}): {}",
                    attempts, total_elapsed, error
                ),
            },
            RetryError::PredicateRejected {
                last,
                attempts,
                total_elapsed,
            } => match last {
                Err(error) => write!(
                    f,
                    "predicate rejected error after {} attempt(s) (elapsed: {:?}): {}",
                    attempts, total_elapsed, error
                ),
                Ok(value) => write!(
                    f,
                    "predicate rejected result after {} attempt(s) (elapsed: {:?}): last value = {:?}",
                    attempts, total_elapsed, value
                ),
            },
            RetryError::Cancelled {
                last,
                attempts,
                total_elapsed,
            } => match last {
                Some(Err(error)) => write!(
                    f,
                    "cancelled after {} attempt(s) (elapsed: {:?}): {}",
                    attempts, total_elapsed, error
                ),
                Some(Ok(value)) => write!(
                    f,
                    "cancelled after {} attempt(s) (elapsed: {:?}): last value = {:?}",
                    attempts, total_elapsed, value
                ),
                None => write!(
                    f,
                    "cancelled after {} attempt(s) (elapsed: {:?})",
                    attempts, total_elapsed
                ),
            },
        }
    }
}

#[cfg(feature = "std")]
impl<E, T> std::error::Error for RetryError<E, T>
where
    E: std::error::Error + 'static,
    T: fmt::Debug,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RetryError::Exhausted { last, .. }
            | RetryError::PredicateRejected { last, .. }
            | RetryError::ConditionNotMet { last, .. } => match last {
                Err(error) => Some(error as _),
                _ => None,
            },
            RetryError::Cancelled { last, .. } => match last {
                Some(Err(error)) => Some(error as _),
                _ => None,
            },
        }
    }
}
