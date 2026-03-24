//! Retry execution statistics.

use crate::compat::Duration;
use core::fmt;

/// Why a retry loop terminated.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryPolicy, StopReason, stop};
///
/// let policy = RetryPolicy::new().stop(stop::attempts(1));
/// let (_result, stats) = policy
///     .retry(|_| Err::<(), _>("fail"))
///     .sleep(|_dur| {})
///     .with_stats()
///     .call();
///
/// assert_eq!(stats.stop_reason, StopReason::Exhausted);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StopReason {
    /// The predicate accepted the outcome (did not request retry).
    /// Covers both predicate-accepted `Ok` (returned as `Ok(T)`) and
    /// predicate-accepted `Err` (returned as `RetryError::Rejected`).
    Accepted,
    /// The stop strategy fired while the predicate still wanted to retry.
    Exhausted,
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopReason::Accepted => f.write_str("accepted"),
            StopReason::Exhausted => f.write_str("retries exhausted"),
        }
    }
}

/// Aggregate statistics for a completed retry execution.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryPolicy, RetryStats, stop, wait};
/// use core::time::Duration;
///
/// let policy = RetryPolicy::new()
///     .stop(stop::attempts(3))
///     .wait(wait::fixed(Duration::from_millis(5)));
/// let (_result, stats): (Result<(), _>, RetryStats) = policy
///     .retry(|_| Err::<(), _>("fail"))
///     .sleep(|_dur| {})
///     .with_stats()
///     .call();
///
/// assert_eq!(stats.attempts, 3);
/// assert_eq!(stats.total_wait, Duration::from_millis(10));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RetryStats {
    /// Number of attempts that were executed.
    pub attempts: u32,
    /// Total wall-clock elapsed time, or `None` when no clock is available.
    pub total_elapsed: Option<Duration>,
    /// Cumulative duration requested from the wait strategy.
    pub total_wait: Duration,
    /// The reason the retry loop terminated.
    pub stop_reason: StopReason,
}
