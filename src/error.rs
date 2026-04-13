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
///
/// # Examples
///
/// ```
/// use relentless::RetryError;
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
}

/// Convenience alias: `Result<T, RetryError<T, E>>`.
pub type RetryResult<T, E> = core::result::Result<T, RetryError<T, E>>;

impl<T, E> RetryError<T, E> {
    /// Returns the final attempt outcome, if one exists.
    ///
    /// Returns `Some` for `Exhausted`, `None` for `Rejected` (which stores
    /// only `E`).
    #[must_use]
    pub fn last(&self) -> Option<&Result<T, E>> {
        match self {
            RetryError::Exhausted { last } => Some(last),
            RetryError::Rejected { .. } => None,
        }
    }

    /// Consumes the error and returns the final attempt outcome, if one exists.
    ///
    /// Returns `None` for `Rejected`, which stores only `E` — use
    /// [`into_last_error`](Self::into_last_error) in that case.
    #[must_use]
    pub fn into_last(self) -> Option<Result<T, E>> {
        match self {
            RetryError::Exhausted { last } => Some(last),
            RetryError::Rejected { .. } => None,
        }
    }

    /// Returns the last error value when the terminal outcome carried `Err(E)`.
    ///
    /// Returns `Some` for `Rejected`, and for `Exhausted` when the last outcome
    /// is `Err`; `None` otherwise.
    #[must_use]
    pub fn last_error(&self) -> Option<&E> {
        match self {
            RetryError::Exhausted { last } => last.as_ref().err(),
            RetryError::Rejected { last } => Some(last),
        }
    }

    /// Consumes the retry error and returns the last error value, if present.
    ///
    /// Returns `Some` for `Rejected` and for `Exhausted` when the final
    /// outcome was `Err`; `None` when `Exhausted` with a final `Ok`.
    #[must_use]
    pub fn into_last_error(self) -> Option<E> {
        match self {
            RetryError::Exhausted { last } => last.err(),
            RetryError::Rejected { last } => Some(last),
        }
    }

    /// Returns the [`StopReason`] that caused the retry loop to terminate.
    #[must_use]
    pub fn stop_reason(&self) -> StopReason {
        match self {
            RetryError::Exhausted { .. } => StopReason::Exhausted,
            RetryError::Rejected { .. } => StopReason::Accepted,
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
        }
    }
}
