//! Why an execution ended.

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
