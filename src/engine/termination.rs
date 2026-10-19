//! How an execution ended: the reason, and the view `on_exit` receives.

use crate::compat::Duration;

/// Why the retry loop terminated. Mirrors the three terminal verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StopReason {
    /// The classifier returned an outcome — the loop succeeded with `Ok(R)`.
    Returned,
    /// The classifier aborted — the loop returned
    /// [`RetryError::Aborted`](crate::engine::RetryError::Aborted).
    Aborted,
    /// The stop strategy fired while the classifier still wanted to retry — the
    /// loop returned [`RetryError::Exhausted`](crate::engine::RetryError::Exhausted).
    Exhausted,
}

impl core::fmt::Display for StopReason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StopReason::Returned => f.write_str("returned"),
            StopReason::Aborted => f.write_str("aborted"),
            StopReason::Exhausted => f.write_str("retries exhausted"),
        }
    }
}

/// Final context passed to the `on_exit` hook: a borrowed view of exactly what
/// the caller receives, plus the terminal attempt's counters.
///
/// [`stop_reason`](Self::stop_reason) is derived from the variant, so the reason
/// and the payload cannot disagree.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub enum Exit<'a, R, A, O> {
    /// The classifier returned `value` — the caller receives `Ok(value)`.
    Returned {
        /// The 1-indexed number of completed attempts.
        attempt: u32,
        /// Wall-clock time elapsed since the first attempt began.
        elapsed: Duration,
        /// The value the caller receives.
        value: &'a R,
    },
    /// The classifier aborted with `last`.
    Aborted {
        /// The 1-indexed number of completed attempts.
        attempt: u32,
        /// Wall-clock time elapsed since the first attempt began.
        elapsed: Duration,
        /// The abort payload the caller receives.
        last: &'a A,
    },
    /// The stop strategy fired while the classifier still wanted to retry.
    Exhausted {
        /// The 1-indexed number of completed attempts.
        attempt: u32,
        /// Wall-clock time elapsed since the first attempt began.
        elapsed: Duration,
        /// The final whole outcome the caller receives.
        last: &'a O,
    },
}

impl<R, A, O> Exit<'_, R, A, O> {
    /// The 1-indexed number of completed attempts.
    #[must_use]
    pub fn attempt(&self) -> u32 {
        match self {
            Exit::Returned { attempt, .. }
            | Exit::Aborted { attempt, .. }
            | Exit::Exhausted { attempt, .. } => *attempt,
        }
    }

    /// Wall-clock time elapsed since the first attempt began.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        match self {
            Exit::Returned { elapsed, .. }
            | Exit::Aborted { elapsed, .. }
            | Exit::Exhausted { elapsed, .. } => *elapsed,
        }
    }

    /// The reason the loop terminated, derived from the variant.
    #[must_use]
    pub fn stop_reason(&self) -> StopReason {
        match self {
            Exit::Returned { .. } => StopReason::Returned,
            Exit::Aborted { .. } => StopReason::Aborted,
            Exit::Exhausted { .. } => StopReason::Exhausted,
        }
    }
}
