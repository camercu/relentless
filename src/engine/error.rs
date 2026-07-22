//! The classifier engine's error type.

use super::state::StopReason;
use core::fmt;

/// Error returned when the retry loop terminates without a `Return`.
///
/// - `Aborted` — the classifier chose [`Verdict::Abort`](crate::decision::Verdict);
///   `last` is the projected abort payload.
/// - `Exhausted` — the stop strategy fired (or the timeout elapsed) while the
///   classifier still wanted to retry; `last` is the final whole outcome.
///
/// # Type parameters
///
/// - `A`: the abort payload (what the classifier projects on `Abort`).
/// - `O`: the whole outcome the operation produces.
///
/// On the default and `.when`/`.until` paths — where the outcome is
/// `Result<T, E>` and aborts carry the bare error — this is
/// `RetryError<E, Result<T, E>>`, and the `Result`-shaped helpers below
/// (`last`, `last_error`, `Display`, `Error`) apply.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetryError<A, O> {
    /// The classifier rejected an outcome as fatal.
    Aborted {
        /// The abort payload chosen by the classifier.
        last: A,
    },
    /// The stop strategy fired while the classifier still wanted to retry.
    Exhausted {
        /// The final whole outcome seen before giving up.
        last: O,
    },
}

/// Convenience alias for the common `Result` outcome shape:
/// `Result<T, RetryError<E, Result<T, E>>>`.
pub type RetryResult<T, E> = Result<T, RetryError<E, Result<T, E>>>;

impl<A, O> RetryError<A, O> {
    /// Returns the [`StopReason`] that terminated the loop.
    #[must_use]
    pub fn stop_reason(&self) -> StopReason {
        match self {
            RetryError::Aborted { .. } => StopReason::Aborted,
            RetryError::Exhausted { .. } => StopReason::Exhausted,
        }
    }
}

/// `Result`-shaped helpers, available on the default / `.when` / `.until` path
/// where the outcome is `Result<T, E>` and aborts carry the bare error.
impl<T, E> RetryError<E, Result<T, E>> {
    /// Returns the final attempt outcome, if one is retained.
    ///
    /// `Some` for `Exhausted`; `None` for `Aborted` (which stores only the
    /// bare error — see [`last_error`](Self::last_error)).
    #[must_use]
    pub fn last(&self) -> Option<&Result<T, E>> {
        match self {
            RetryError::Exhausted { last } => Some(last),
            RetryError::Aborted { .. } => None,
        }
    }

    /// Consumes the error and returns the final attempt outcome, if retained.
    #[must_use]
    pub fn into_last(self) -> Option<Result<T, E>> {
        match self {
            RetryError::Exhausted { last } => Some(last),
            RetryError::Aborted { .. } => None,
        }
    }

    /// Returns the last error value when the terminal outcome carried `Err(E)`.
    ///
    /// `Some` for `Aborted`, and for `Exhausted` when the last outcome was
    /// `Err`; `None` otherwise.
    #[must_use]
    pub fn last_error(&self) -> Option<&E> {
        match self {
            RetryError::Aborted { last } => Some(last),
            RetryError::Exhausted { last } => last.as_ref().err(),
        }
    }

    /// Consumes the error and returns the last error value, if present.
    #[must_use]
    pub fn into_last_error(self) -> Option<E> {
        match self {
            RetryError::Aborted { last } => Some(last),
            RetryError::Exhausted { last } => last.err(),
        }
    }
}

impl<T, E: fmt::Display> fmt::Display for RetryError<E, Result<T, E>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RetryError::Aborted { last } => write!(f, "aborted: {last}"),
            RetryError::Exhausted { last } => match last {
                Err(error) => write!(f, "retries exhausted: {error}"),
                Ok(_) => f.write_str("retries exhausted"),
            },
        }
    }
}

#[cfg(feature = "std")]
impl<T, E> std::error::Error for RetryError<E, Result<T, E>>
where
    E: std::error::Error + 'static,
    T: fmt::Debug + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RetryError::Aborted { last } => Some(last as _),
            RetryError::Exhausted { last } => match last {
                Err(error) => Some(error as _),
                Ok(_) => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Err = RetryError<&'static str, Result<i32, &'static str>>;

    #[test]
    fn stop_reason_matches_the_variant() {
        let aborted: Err = RetryError::Aborted { last: "x" };
        let exhausted: Err = RetryError::Exhausted { last: Err("y") };
        assert_eq!(aborted.stop_reason(), StopReason::Aborted);
        assert_eq!(exhausted.stop_reason(), StopReason::Exhausted);
    }

    #[test]
    fn last_retains_the_outcome_only_on_exhausted() {
        let exhausted: Err = RetryError::Exhausted { last: Ok(3) };
        let aborted: Err = RetryError::Aborted { last: "boom" };
        assert_eq!(exhausted.last(), Some(&Ok(3)));
        assert_eq!(aborted.last(), None);
    }

    #[test]
    fn last_error_collapses_to_the_error() {
        let aborted: Err = RetryError::Aborted { last: "boom" };
        let exhausted_err: Err = RetryError::Exhausted { last: Err("net") };
        let exhausted_ok: Err = RetryError::Exhausted { last: Ok(1) };
        assert_eq!(aborted.last_error(), Some(&"boom"));
        assert_eq!(exhausted_err.last_error(), Some(&"net"));
        assert_eq!(exhausted_ok.last_error(), None);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn display_reports_the_terminal_reason() {
        use alloc::string::ToString;
        let aborted: Err = RetryError::Aborted { last: "boom" };
        let exhausted: Err = RetryError::Exhausted { last: Err("net") };
        assert_eq!(aborted.to_string(), "aborted: boom");
        assert_eq!(exhausted.to_string(), "retries exhausted: net");
    }
}
