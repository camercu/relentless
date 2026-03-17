//! RetryError — the error type returned when a retry loop terminates without success.

use crate::stats::StopReason;
use core::fmt;

/// Error returned when a retry loop terminates without producing an accepted result.
///
/// # Type Parameters
///
/// - `T`: The `Ok` value type from the retried operation.
/// - `E`: The error type from the retried operation.
///
/// Variant selection follows the retry predicate and terminal outcome:
///
/// - [`RetryError::Exhausted`] means the stop strategy fired while the predicate
///   still wanted to retry.
/// - [`RetryError::Rejected`] means the predicate accepted an `Err` outcome
///   as terminal (did not request retry).
/// - [`RetryError::Cancelled`] means an external canceler interrupted the loop.
///
/// # Examples
///
/// ```
/// use tenacious::RetryError;
///
/// let err: RetryError<(), String> = RetryError::Exhausted {
///     last: Err("connection refused".to_string()),
/// };
///
/// println!("{}", err);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryError<T, E> {
    /// Retries exhausted — the stop strategy fired while the predicate
    /// still wanted to retry. The last outcome is preserved.
    Exhausted {
        /// The final attempt outcome.
        last: Result<T, E>,
    },

    /// The predicate accepted an `Err` outcome as terminal (did not
    /// request retry).
    Rejected {
        /// The non-retryable error.
        last: E,
    },

    /// An external cancellation signal interrupted the retry loop.
    Cancelled {
        /// The outcome from the most recent completed attempt, or `None` when
        /// cancellation fires before the first attempt.
        last: Option<Result<T, E>>,
    },
}

/// Convenience alias for retry-returning operations.
///
/// Expands to `Result<T, RetryError<T, E>>`.
pub type RetryResult<T, E> = core::result::Result<T, RetryError<T, E>>;

impl<T, E> RetryError<T, E> {
    /// Returns the final attempt outcome, if one exists.
    ///
    /// Returns `Some` for `Exhausted` and `Cancelled` (when an attempt
    /// completed), `None` for `Rejected` (which stores only `E`) and
    /// `Cancelled` before the first attempt.
    #[must_use]
    pub fn last(&self) -> Option<&Result<T, E>> {
        match self {
            RetryError::Exhausted { last } => Some(last),
            RetryError::Rejected { .. } => None,
            RetryError::Cancelled { last } => last.as_ref(),
        }
    }

    /// Consumes the error and returns the final attempt outcome, if one exists.
    ///
    /// Same `None` cases as [`last()`](Self::last).
    #[must_use]
    pub fn into_last(self) -> Option<Result<T, E>> {
        match self {
            RetryError::Exhausted { last } => Some(last),
            RetryError::Rejected { .. } => None,
            RetryError::Cancelled { last } => last,
        }
    }

    /// Returns the last error value when the terminal outcome carried `Err(E)`.
    ///
    /// Returns `Some` for `Rejected`, for `Exhausted` when the last outcome is
    /// `Err`, and for `Cancelled` when the last outcome is `Err`; `None` otherwise.
    #[must_use]
    pub fn last_error(&self) -> Option<&E> {
        match self {
            RetryError::Exhausted { last } => last.as_ref().err(),
            RetryError::Rejected { last } => Some(last),
            RetryError::Cancelled { last } => last.as_ref().and_then(|r| r.as_ref().err()),
        }
    }

    /// Consumes the retry error and returns the last error value, if present.
    ///
    /// Same `Some` cases as [`last_error()`](Self::last_error).
    #[must_use]
    pub fn into_last_error(self) -> Option<E> {
        match self {
            RetryError::Exhausted { last } => last.err(),
            RetryError::Rejected { last } => Some(last),
            RetryError::Cancelled { last } => last.and_then(Result::err),
        }
    }

    /// Returns the termination reason as a typed enum.
    #[must_use]
    pub fn stop_reason(&self) -> StopReason {
        match self {
            RetryError::Exhausted { .. } => StopReason::Exhausted,
            RetryError::Rejected { .. } => StopReason::Accepted,
            RetryError::Cancelled { .. } => StopReason::Cancelled,
        }
    }
}

impl<T, E: fmt::Display> fmt::Display for RetryError<T, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RetryError::Exhausted { last } => match last {
                Err(error) => write!(f, "retries exhausted: {error}"),
                Ok(_) => f.write_str("retries exhausted"),
            },
            RetryError::Rejected { last } => write!(f, "rejected: {last}"),
            RetryError::Cancelled { last } => match last {
                Some(Err(error)) => write!(f, "cancelled: {error}"),
                _ => f.write_str("cancelled"),
            },
        }
    }
}

#[cfg(feature = "std")]
impl<T, E> std::error::Error for RetryError<T, E>
where
    E: std::error::Error + 'static,
    T: fmt::Debug + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RetryError::Exhausted { last } => match last {
                Err(error) => Some(error as _),
                _ => None,
            },
            RetryError::Rejected { last } => Some(last as _),
            RetryError::Cancelled { last } => match last {
                Some(Err(error)) => Some(error as _),
                _ => None,
            },
        }
    }
}
