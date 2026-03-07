//! Retry execution statistics.

use crate::compat::Duration;

/// Why a retry loop terminated.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryPolicy, StopReason, stop};
///
/// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
/// let (_result, stats) = policy
///     .retry(|| Err::<(), _>("fail"))
///     .sleep(|_dur| {})
///     .with_stats()
///     .call();
///
/// assert_eq!(stats.stop_reason, StopReason::StopCondition);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StopReason {
    /// The retry loop terminated with an accepted `Ok` outcome.
    ///
    /// This includes acceptance under custom predicates such as `on::ok(...)`
    /// or `on::result(...)`, not only the default predicate.
    Success,
    /// A stop strategy fired and terminated retries.
    StopCondition,
    /// A predicate terminated retries on an `Err` outcome before stop fired.
    PredicateAccepted,
    /// An external cancellation signal interrupted the retry loop.
    Cancelled,
}

/// Aggregate statistics for a completed retry execution.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryPolicy, RetryStats, stop, wait};
/// use core::time::Duration;
///
/// let mut policy = RetryPolicy::new()
///     .stop(stop::attempts(3))
///     .wait(wait::fixed(Duration::from_millis(5)));
/// let (_result, stats): (Result<(), _>, RetryStats) = policy
///     .retry(|| Err::<(), _>("fail"))
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
