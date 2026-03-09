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
/// When you want the familiar `Result<T, E>` ordering at the call site, prefer
/// [`RetryResult<T, E>`](crate::RetryResult).
///
/// Variant selection follows the retry predicate and terminal outcome:
///
/// - [`RetryError::Exhausted`] means the stop strategy fired while the loop was
///   still retrying an `Err(E)`.
/// - [`RetryError::ConditionNotMet`] means the stop strategy fired while the
///   loop was still retrying an `Ok(T)`, which is the common `on::ok(...)`
///   polling case.
/// - [`RetryError::NonRetryableError`] means the predicate rejected an
///   `Err(E)` immediately.
/// - [`RetryError::Cancelled`] means an external canceler interrupted the loop.
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

    /// The latest `Err` outcome was non-retryable, so retrying stopped immediately.
    ///
    /// This occurs when using a custom predicate (for example `on::error`) that
    /// classifies some errors as non-retryable.
    NonRetryableError {
        /// The non-retryable attempt outcome. In normal usage this is `Err(E)`.
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

impl<E, T> RetryError<E, T> {
    /// Returns the final attempt outcome, if one exists.
    ///
    /// This is `None` only for cancellation before the first attempt.
    #[must_use]
    pub fn last(&self) -> Option<&Result<T, E>> {
        match self {
            RetryError::Exhausted { last, .. }
            | RetryError::NonRetryableError { last, .. }
            | RetryError::ConditionNotMet { last, .. } => Some(last),
            RetryError::Cancelled { last, .. } => last.as_ref(),
        }
    }

    /// Consumes the error and returns the final attempt outcome, if one exists.
    ///
    /// This is `None` only for cancellation before the first attempt.
    #[must_use]
    pub fn into_last(self) -> Option<Result<T, E>> {
        match self {
            RetryError::Exhausted { last, .. }
            | RetryError::NonRetryableError { last, .. }
            | RetryError::ConditionNotMet { last, .. } => Some(last),
            RetryError::Cancelled { last, .. } => last,
        }
    }

    /// Returns the last error value when the terminal outcome carried `Err(E)`.
    ///
    /// This is `None` for `ConditionNotMet`, successful terminal values, and
    /// cancellation before the first attempt.
    #[must_use]
    pub fn last_error(&self) -> Option<&E> {
        self.last().and_then(|last| last.as_ref().err())
    }

    /// Consumes the retry error and returns the last error value, if present.
    ///
    /// This is `None` for `ConditionNotMet`, successful terminal values, and
    /// cancellation before the first attempt.
    #[must_use]
    pub fn into_last_error(self) -> Option<E> {
        self.into_last().and_then(Result::err)
    }

    /// Returns the number of completed attempts.
    #[must_use]
    pub fn attempts(&self) -> u32 {
        match self {
            RetryError::Exhausted { attempts, .. }
            | RetryError::NonRetryableError { attempts, .. }
            | RetryError::ConditionNotMet { attempts, .. }
            | RetryError::Cancelled { attempts, .. } => *attempts,
        }
    }

    /// Returns the measured elapsed time, if an elapsed clock was available.
    #[must_use]
    pub fn total_elapsed(&self) -> Option<Duration> {
        match self {
            RetryError::Exhausted { total_elapsed, .. }
            | RetryError::NonRetryableError { total_elapsed, .. }
            | RetryError::ConditionNotMet { total_elapsed, .. }
            | RetryError::Cancelled { total_elapsed, .. } => *total_elapsed,
        }
    }
}

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
            RetryError::NonRetryableError {
                last,
                attempts,
                total_elapsed,
            } => match last {
                Err(error) => write!(
                    f,
                    "non-retryable error after {} attempt(s) (elapsed: {:?}): {}",
                    attempts, total_elapsed, error
                ),
                Ok(value) => write!(
                    f,
                    "non-retryable result after {} attempt(s) (elapsed: {:?}): last value = {:?}",
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
            | RetryError::NonRetryableError { last, .. }
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
