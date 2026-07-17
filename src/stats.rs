use crate::compat::Duration;
use core::fmt;

/// Why a retry loop terminated.
///
/// # Examples
///
/// ```
/// use relentless::clock::VirtualClock;
/// use relentless::{RetryPolicy, StopReason, stop};
///
/// let policy = RetryPolicy::new().stop(stop::attempts(1));
/// let (_result, stats) = policy
///     .retry(|_| Err::<(), _>("fail"))
///     .clock(VirtualClock::new())
///     .with_stats()
///     .call();
///
/// assert_eq!(stats.stop_reason, StopReason::Exhausted);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StopReason {
    /// The predicate accepted an `Ok` outcome — the retry loop succeeded and
    /// returned `Ok(T)`.
    Succeeded,
    /// The predicate accepted an `Err` outcome as terminal (did not request
    /// retry) — the loop returned [`RetryError::Rejected`](crate::RetryError::Rejected).
    Rejected,
    /// The stop strategy fired while the predicate still wanted to retry.
    Exhausted,
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopReason::Succeeded => f.write_str("succeeded"),
            StopReason::Rejected => f.write_str("rejected"),
            StopReason::Exhausted => f.write_str("retries exhausted"),
        }
    }
}

/// Aggregate statistics for a completed retry execution.
///
/// # Examples
///
/// ```
/// use relentless::clock::VirtualClock;
/// use relentless::{RetryPolicy, RetryStats, stop, wait};
/// use core::time::Duration;
///
/// let policy = RetryPolicy::new()
///     .stop(stop::attempts(3))
///     .wait(wait::fixed(Duration::from_millis(5)));
/// let (_result, stats): (Result<(), _>, RetryStats) = policy
///     .retry(|_| Err::<(), _>("fail"))
///     .clock(VirtualClock::new())
///     .with_stats()
///     .call();
///
/// assert_eq!(stats.attempts, 3);
/// assert_eq!(stats.total_wait, Duration::from_millis(10));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryStats {
    /// The total number of attempts executed.
    pub attempts: u32,
    /// Total wall-clock elapsed time, or `None` when no clock is available.
    pub total_elapsed: Option<Duration>,
    /// Cumulative duration requested from the wait strategy.
    pub total_wait: Duration,
    /// The reason the retry loop terminated.
    pub stop_reason: StopReason,
}
