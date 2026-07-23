//! Observation state passed to the classifier engine's hooks.
//!
//! [`RetryState`](crate::state::RetryState) (the pre-attempt, outcome-free
//! context for the operation and `before_attempt`) is reused unchanged from the
//! old engine. This module adds the two outcome-carrying views:
//!
//! - [`AttemptState`] — passed to `after_attempt`, once per attempt, holding a
//!   borrow of the raw outcome *before* classification.
//! - [`Exit`] — passed to `on_exit`, a borrowed view of exactly what the caller
//!   receives, with [`stop_reason`](Exit::stop_reason) derived from its variant.

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

/// Read-only context passed to the `after_attempt` hook, once per attempt.
///
/// Fires *before* the classifier consumes the outcome, so the hook sees every
/// raw outcome under a uniform contract — including the terminal one.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct AttemptState<'a, O> {
    /// The 1-indexed attempt number that just completed.
    pub attempt: u32,
    /// Wall-clock time elapsed since the first attempt began.
    pub elapsed: Duration,
    /// A borrow of the raw outcome, before classification.
    pub outcome: &'a O,
}

impl<'a, O> AttemptState<'a, O> {
    /// Creates an `AttemptState` for the given 1-indexed attempt and outcome.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `attempt` is `0` (attempts are 1-indexed).
    #[must_use]
    pub(crate) const fn new(attempt: u32, elapsed: Duration, outcome: &'a O) -> Self {
        debug_assert!(attempt >= 1, "attempt is 1-indexed");
        Self {
            attempt,
            elapsed,
            outcome,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_state_exposes_attempt_elapsed_and_outcome() {
        let outcome: Result<(), &str> = Err("network timeout");
        let state = AttemptState::new(1, Duration::ZERO, &outcome);

        assert_eq!(state.attempt, 1);
        assert_eq!(state.elapsed, Duration::ZERO);
        assert_eq!(state.outcome.unwrap_err(), "network timeout");
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "attempt is 1-indexed")]
    fn attempt_state_new_zero_panics_in_debug() {
        let outcome: Result<i32, &str> = Ok(1);
        let _ = AttemptState::new(0, Duration::ZERO, &outcome);
    }
}
