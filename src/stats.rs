//! Retry execution statistics.

use crate::compat::Duration;

/// Why a retry loop terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StopReason {
    /// The operation produced a successful result under the default predicate.
    Success,
    /// A stop strategy fired and terminated retries.
    StopCondition,
    /// A custom predicate accepted the current outcome.
    PredicateAccepted,
}

/// Aggregate statistics for a completed retry execution.
#[derive(Debug, Clone, PartialEq, Eq)]
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
