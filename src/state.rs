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
/// use tenacious::RetryState;
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

    /// Wall-clock time elapsed since the first attempt began.
    /// `None` when no clock is available (e.g. `no_std` without a time source).
    pub elapsed: Option<Duration>,
}

impl RetryState {
    /// Creates a retry state value for tests and custom strategy code.
    #[must_use]
    pub const fn new(attempt: u32, elapsed: Option<Duration>) -> Self {
        Self { attempt, elapsed }
    }
}

/// Read-only context passed to the `after_attempt` hook.
///
/// This contains the attempt outcome plus timing/counting fields for the
/// completed attempt. The `next_delay` field tells whether a retry will
/// happen: `Some(delay)` means the engine will sleep for `delay` before
/// the next attempt, while `None` means this was a terminal attempt
/// (predicate accepted, stop condition fired, or first-attempt success).
///
/// # Examples
///
/// ```
/// use tenacious::AttemptState;
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

    /// Wall-clock time elapsed since the first attempt began.
    /// `None` when no clock is available.
    pub elapsed: Option<Duration>,

    /// A reference to the outcome of the most recent attempt.
    pub outcome: &'a Result<T, E>,

    /// The delay that will be applied before the next attempt.
    ///
    /// `Some(delay)` when a retry will happen (the engine will sleep for
    /// `delay`). `None` when this is a terminal attempt — predicate
    /// accepted, stop condition fired, or first-attempt success.
    pub next_delay: Option<Duration>,
}

impl<'a, T, E> AttemptState<'a, T, E> {
    /// Creates an attempt state value for tests and custom integrations.
    #[must_use]
    pub const fn new(
        attempt: u32,
        elapsed: Option<Duration>,
        outcome: &'a Result<T, E>,
        next_delay: Option<Duration>,
    ) -> Self {
        Self {
            attempt,
            elapsed,
            outcome,
            next_delay,
        }
    }
}

/// Final read-only context passed to the `on_exit` hook.
///
/// This contains the last attempt's outcome (when available) and termination
/// reason, and fires once whenever retry execution exits (success, stop
/// strategy triggered, rejected error, or cancellation).
///
/// `outcome` is `None` only when cancellation happens before the first attempt
/// starts. In all other exit paths, it is `Some(&Result<T, E>)`.
///
/// # Examples
///
/// ```
/// use tenacious::{ExitState, StopReason};
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
    /// The number of completed attempts. `0` only when cancelled before
    /// the first attempt.
    pub attempt: u32,

    /// Wall-clock time elapsed since the first attempt began.
    /// `None` when no clock is available.
    pub elapsed: Option<Duration>,

    /// A reference to the final outcome.
    ///
    /// `None` only when cancellation happens before the first attempt starts.
    pub outcome: Option<&'a Result<T, E>>,

    /// Why the retry loop terminated.
    pub stop_reason: crate::stats::StopReason,
}

impl<'a, T, E> ExitState<'a, T, E> {
    /// Creates an exit state value for tests and custom integrations.
    #[must_use]
    pub const fn new(
        attempt: u32,
        elapsed: Option<Duration>,
        outcome: Option<&'a Result<T, E>>,
        stop_reason: crate::stats::StopReason,
    ) -> Self {
        Self {
            attempt,
            elapsed,
            outcome,
            stop_reason,
        }
    }
}
