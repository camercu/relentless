//! Aggregate statistics for a completed classifier-engine execution.

use super::state::StopReason;
use crate::compat::Duration;

/// Aggregate statistics for a completed retry execution.
///
/// Returned alongside the result when the execution is wrapped with
/// `.with_stats()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryStats {
    /// The total number of attempts executed.
    pub attempts: u32,
    /// Total wall-clock elapsed time, read from the injected clock.
    pub total_elapsed: Duration,
    /// Cumulative duration requested from the wait strategy (after timeout
    /// clamping).
    pub total_wait: Duration,
    /// The reason the retry loop terminated.
    pub stop_reason: StopReason,
}
