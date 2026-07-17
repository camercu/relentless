//! Retry state types — shared context for retry hooks and strategies.

use crate::compat::Duration;

/// Non-generic retry context passed to [`Stop::should_stop`](crate::Stop::should_stop),
/// [`Wait::next_wait`](crate::Wait::next_wait), the operation, and the `before_attempt` hook.
///
/// This contains only the timing and counting fields that stop/wait strategies
/// need. It deliberately excludes the operation's outcome, keeping `Stop` and
/// `Wait` decoupled from the operation's `Result<T, E>` type.
///
/// The operation receives `RetryState` by value (it is `Copy`).
///
/// # Examples
///
/// ```
/// use relentless::RetryState;
/// use core::time::Duration;
///
/// fn log_state(state: &RetryState) {
///     println!("attempt {} elapsed {:?}", state.attempt, state.elapsed);
/// }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryState {
    /// The 1-indexed attempt number.
    ///
    /// For the operation and `before_attempt`, this is the attempt about to start.
    /// For `Stop` and `Wait`, this is the just-completed attempt.
    pub attempt: u32,

    /// Wall-clock time elapsed since the first attempt began, read from the
    /// injected clock. Zero in hand-constructed states until set with
    /// [`with_elapsed`](Self::with_elapsed).
    pub elapsed: Duration,

    /// The delay applied before this attempt — the previous inter-attempt sleep,
    /// after cap/timeout clamping — or `None` on the first attempt.
    ///
    /// Wait strategies use this for feedback backoff. For example, decorrelated
    /// jitter computes `random(base, previous_delay * 3)`.
    pub previous_delay: Option<Duration>,
}

impl RetryState {
    /// Creates a `RetryState` for the given 1-indexed attempt.
    ///
    /// [`elapsed`](Self::elapsed) defaults to zero and
    /// [`previous_delay`](Self::previous_delay) to `None`; set them with
    /// [`with_elapsed`](Self::with_elapsed) and
    /// [`with_previous_delay`](Self::with_previous_delay).
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `attempt` is `0` (attempts are 1-indexed).
    #[must_use]
    pub const fn for_attempt(attempt: u32) -> Self {
        debug_assert!(attempt >= 1, "attempt is 1-indexed");
        Self {
            attempt,
            elapsed: Duration::ZERO,
            previous_delay: None,
        }
    }

    /// Sets [`elapsed`](Self::elapsed), consuming and returning `self` for
    /// chaining.
    #[must_use]
    pub const fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = elapsed;
        self
    }

    /// Sets [`previous_delay`](Self::previous_delay), consuming and returning
    /// `self` for chaining.
    #[must_use]
    pub const fn with_previous_delay(mut self, previous_delay: Option<Duration>) -> Self {
        self.previous_delay = previous_delay;
        self
    }
}

/// Read-only context passed to the `after_attempt` hook.
///
/// This contains the attempt outcome plus timing/counting fields for the
/// completed attempt.
///
/// # Examples
///
/// ```
/// use relentless::AttemptState;
/// use core::time::Duration;
///
/// fn log_attempt(state: &AttemptState<i32, String>) {
///     println!("attempt {} result {:?}", state.attempt, state.outcome);
/// }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct AttemptState<'a, T, E> {
    /// The 1-indexed attempt number that just completed.
    pub attempt: u32,

    /// Wall-clock time elapsed since the first attempt began, read from the
    /// injected clock.
    pub elapsed: Duration,

    /// A reference to the outcome of the most recent attempt.
    pub outcome: &'a Result<T, E>,

    /// The delay that will be applied before the next attempt, or `None`
    /// if this is the terminal attempt (predicate accepted, stop condition
    /// fired, or first-attempt success).
    pub next_delay: Option<Duration>,
}

impl<'a, T, E> AttemptState<'a, T, E> {
    /// Creates an `AttemptState` for the given 1-indexed attempt and its
    /// outcome.
    ///
    /// [`elapsed`](Self::elapsed) defaults to zero and
    /// [`next_delay`](Self::next_delay) to `None`; set them with
    /// [`with_elapsed`](Self::with_elapsed) and
    /// [`with_next_delay`](Self::with_next_delay).
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `attempt` is `0` (attempts are 1-indexed).
    #[must_use]
    pub const fn for_attempt(attempt: u32, outcome: &'a Result<T, E>) -> Self {
        debug_assert!(attempt >= 1, "attempt is 1-indexed");
        Self {
            attempt,
            elapsed: Duration::ZERO,
            outcome,
            next_delay: None,
        }
    }

    /// Sets [`elapsed`](Self::elapsed), consuming and returning `self` for
    /// chaining.
    #[must_use]
    pub const fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = elapsed;
        self
    }

    /// Sets [`next_delay`](Self::next_delay), consuming and returning `self`
    /// for chaining.
    #[must_use]
    pub const fn with_next_delay(mut self, next_delay: Option<Duration>) -> Self {
        self.next_delay = next_delay;
        self
    }
}

/// Final read-only context passed to the `on_exit` hook.
///
/// This contains the last attempt's outcome and termination reason, and fires
/// once whenever retry execution exits (success, stop strategy triggered, or
/// rejected error).
///
/// # Examples
///
/// ```
/// use relentless::{ExitState, StopReason};
///
/// fn on_exit(state: &ExitState<i32, String>) {
///     if state.stop_reason == StopReason::Exhausted {
///         println!("stopped on attempt {}", state.attempt);
///     }
/// }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct ExitState<'a, T, E> {
    /// The number of completed attempts. Always >= 1.
    pub attempt: u32,

    /// Wall-clock time elapsed since the first attempt began, read from the
    /// injected clock.
    pub elapsed: Duration,

    /// A reference to the final outcome.
    pub outcome: &'a Result<T, E>,

    /// The reason the retry loop stopped.
    pub stop_reason: crate::stats::StopReason,
}

impl<'a, T, E> ExitState<'a, T, E> {
    /// Creates an `ExitState` for the given 1-indexed final attempt, its
    /// outcome, and the loop's termination reason.
    ///
    /// [`elapsed`](Self::elapsed) defaults to zero; set it with
    /// [`with_elapsed`](Self::with_elapsed).
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `attempt` is `0` (attempts are 1-indexed).
    #[must_use]
    pub const fn for_attempt(
        attempt: u32,
        outcome: &'a Result<T, E>,
        stop_reason: crate::stats::StopReason,
    ) -> Self {
        debug_assert!(attempt >= 1, "attempt is 1-indexed");
        Self {
            attempt,
            elapsed: Duration::ZERO,
            outcome,
            stop_reason,
        }
    }

    /// Sets [`elapsed`](Self::elapsed), consuming and returning `self` for
    /// chaining.
    #[must_use]
    pub const fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = elapsed;
        self
    }
}
