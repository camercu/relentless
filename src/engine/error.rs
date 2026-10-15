//! The classifier engine's error type.

use super::state::StopReason;
use core::convert::Infallible;
use core::fmt;
use core::ops::ControlFlow;

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
/// [`last`](Self::last) and [`into_last`](Self::into_last) are
/// outcome-agnostic and available for every shape (including `Option` and a
/// self-classifying [`Outcome`](crate::Outcome)). On the default and
/// `.when`/`.until` paths — where the outcome is `Result<T, E>` and aborts
/// carry the bare error — this is `RetryError<E, Result<T, E>>`, and the
/// `Result`-shaped helpers below (`last_error`, `Display`, `Error`) also apply.
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

    /// Returns the final attempt outcome, if one is retained.
    ///
    /// `Some` for `Exhausted`; `None` for `Aborted` (which stores only the
    /// projected abort payload — see [`last_error`](Self::last_error) on the
    /// `Result` shape). Outcome-agnostic: available for every outcome type,
    /// including `Option` and a self-classifying [`Outcome`](crate::Outcome).
    #[must_use]
    pub fn last(&self) -> Option<&O> {
        match self {
            RetryError::Exhausted { last } => Some(last),
            RetryError::Aborted { .. } => None,
        }
    }

    /// Consumes the error and returns the final attempt outcome, if retained.
    #[must_use]
    pub fn into_last(self) -> Option<O> {
        match self {
            RetryError::Exhausted { last } => Some(last),
            RetryError::Aborted { .. } => None,
        }
    }
}

/// `Result`-shaped helpers, available on the default / `.when` / `.until` path
/// where the outcome is `Result<T, E>` and aborts carry the bare error.
impl<T, E> RetryError<E, Result<T, E>> {
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

/// `Option`-shaped `Display`, for the canonical two-state poll.
///
/// An `Option` outcome's abort type is [`Infallible`], so `Aborted` is
/// uninhabited and the only reachable terminus is `Exhausted` — a `None` that
/// never became `Some`. There is no error payload to report, hence no outcome
/// detail in the message (contrast the `Result` impl's `"retries exhausted:
/// {error}"`).
impl<T> fmt::Display for RetryError<Infallible, Option<T>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RetryError::Aborted { last } => match *last {},
            RetryError::Exhausted { .. } => f.write_str("retries exhausted"),
        }
    }
}

/// `Option`-shaped `Error`. There is no underlying cause — the abort arm is
/// uninhabited and an exhausted `None` carries no error — so `source` is `None`.
#[cfg(feature = "std")]
impl<T: fmt::Debug> std::error::Error for RetryError<Infallible, Option<T>> {}

/// `ControlFlow`-shaped `Display`, mirroring the `Option` shape: `Abort` is
/// [`Infallible`], so the only reachable terminus is `Exhausted` — a loop that
/// never broke. No value to report, so the message carries no outcome detail.
impl<B, C> fmt::Display for RetryError<Infallible, ControlFlow<B, C>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RetryError::Aborted { last } => match *last {},
            RetryError::Exhausted { .. } => f.write_str("retries exhausted"),
        }
    }
}

/// `ControlFlow`-shaped `Error`, with no `source` (uninhabited abort arm; an
/// exhausted `Continue` carries no error).
#[cfg(feature = "std")]
impl<B: fmt::Debug, C: fmt::Debug> std::error::Error for RetryError<Infallible, ControlFlow<B, C>> {}
