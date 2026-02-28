//! RetryError — the error type returned when a retry loop terminates without success.

use crate::compat::Duration;
use core::fmt;

/// Error returned when a retry loop terminates without producing an accepted result.
///
/// # Type Parameters
///
/// - `E`: The error type from the retried operation.
/// - `T`: The `Ok` value type. Defaults to `()` for the common retry-on-error case,
///   where `ConditionNotMet` is unreachable. When `on::ok` or `on::result`
///   predicates cause retries on `Ok` values, `T` carries the last `Ok` value.
///
/// # Examples
///
/// ```
/// use tenacious::RetryError;
/// use core::time::Duration;
///
/// // Common case: T defaults to ()
/// let err: RetryError<String> = RetryError::Exhausted {
///     error: "connection refused".to_string(),
///     attempts: 3,
///     total_elapsed: Some(Duration::from_secs(5)),
/// };
///
/// println!("{}", err);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum RetryError<E, T = ()> {
    /// All retries exhausted; the operation kept returning `Err`.
    Exhausted {
        /// The error from the final attempt.
        error: E,
        /// Total number of attempts made.
        attempts: u32,
        /// Wall-clock time elapsed, or `None` if no clock was available.
        total_elapsed: Option<Duration>,
    },

    /// The stop condition fired while the predicate was still rejecting `Ok` values.
    /// This variant is used when `on::ok` or `on::result` predicates cause retries
    /// on `Ok` values and the stop condition fires before the predicate accepts.
    /// The last `Ok` value is moved here; no clone is required.
    ConditionNotMet {
        /// The last `Ok` value that did not satisfy the predicate.
        last: T,
        /// Total number of attempts made.
        attempts: u32,
        /// Wall-clock time elapsed, or `None` if no clock was available.
        total_elapsed: Option<Duration>,
    },
}

impl<E: fmt::Display, T: fmt::Debug> fmt::Display for RetryError<E, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RetryError::Exhausted {
                error,
                attempts,
                total_elapsed,
            } => {
                write!(
                    f,
                    "retry exhausted after {} attempt(s) (elapsed: {:?}): {}",
                    attempts, total_elapsed, error
                )
            }
            RetryError::ConditionNotMet {
                last,
                attempts,
                total_elapsed,
            } => {
                write!(
                    f,
                    "condition not met after {} attempt(s) (elapsed: {:?}): last value = {:?}",
                    attempts, total_elapsed, last
                )
            }
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
            RetryError::Exhausted { error, .. } => Some(error),
            RetryError::ConditionNotMet { .. } => None,
        }
    }
}
