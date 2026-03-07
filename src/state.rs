//! Retry state types — shared context for retry hooks and strategies.

use crate::compat::Duration;

/// Non-generic retry context passed to [`Stop::should_stop`](crate::Stop::should_stop)
/// and [`Wait::next_wait`](crate::Wait::next_wait).
///
/// This contains only the timing and counting fields that stop/wait strategies
/// need. It deliberately excludes the operation's outcome, keeping `Stop` and
/// `Wait` decoupled from the operation's `Result<T, E>` type.
///
/// This struct is normally constructed by the execution engine and passed by
/// shared reference. For tests and custom strategy implementations, use
/// [`RetryState::new`] rather than a struct literal.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryState {
    /// The 1-indexed attempt number that just completed.
    ///
    /// The execution engine increments this with saturating arithmetic,
    /// so it will never overflow — it caps at [`u32::MAX`].
    pub attempt: u32,

    /// Wall-clock time elapsed since the first attempt began.
    /// `None` when no clock is available (e.g. `no_std` without a time source).
    pub elapsed: Option<Duration>,

    /// The delay that will be applied before the next attempt.
    /// Populated after [`Wait::next_wait`](crate::Wait::next_wait) runs;
    /// zero in hooks that fire before the wait is computed.
    pub next_delay: Duration,

    /// Cumulative time spent sleeping across all previous attempts.
    pub total_wait: Duration,
}

impl RetryState {
    /// Creates a retry state value for tests and custom strategy code.
    #[must_use]
    pub const fn new(
        attempt: u32,
        elapsed: Option<Duration>,
        next_delay: Duration,
        total_wait: Duration,
    ) -> Self {
        Self {
            attempt,
            elapsed,
            next_delay,
            total_wait,
        }
    }
}

/// Read-only context passed to [`Predicate::should_retry`](crate::Predicate::should_retry)
/// and the `after_attempt` and `before_sleep` hooks.
///
/// This contains the attempt outcome plus timing/counting fields for the
/// completed attempt, mirroring the execution context needed by predicates and
/// hooks.
///
/// This struct is constructed internally by the execution engine and passed by
/// shared reference. For tests and custom integrations, use
/// [`AttemptState::new`] rather than a struct literal.
///
/// # Examples
///
/// ```
/// use tenacious::AttemptState;
/// use core::time::Duration;
///
/// // AttemptState is typically received in callbacks:
/// fn log_attempt(state: &AttemptState<i32, String>) {
///     println!("attempt {} result {:?}", state.attempt, state.outcome);
/// }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptState<'a, T, E> {
    /// The 1-indexed attempt number that just completed.
    pub attempt: u32,

    /// A reference to the outcome of the most recent attempt.
    pub outcome: &'a Result<T, E>,

    /// Wall-clock time elapsed since the first attempt began.
    /// `None` when no clock is available.
    pub elapsed: Option<Duration>,

    /// The delay that will be applied before the next attempt.
    /// Populated after `Wait::next_wait` runs; zero in earlier hook points.
    pub next_delay: Duration,

    /// Cumulative time spent sleeping across all previous attempts.
    pub total_wait: Duration,
}

impl<'a, T, E> AttemptState<'a, T, E> {
    /// Creates an attempt state value for tests and custom integrations.
    #[must_use]
    pub const fn new(
        attempt: u32,
        outcome: &'a Result<T, E>,
        elapsed: Option<Duration>,
        next_delay: Duration,
        total_wait: Duration,
    ) -> Self {
        Self {
            attempt,
            outcome,
            elapsed,
            next_delay,
            total_wait,
        }
    }
}

/// Final read-only context passed to the `on_exit` hook.
///
/// This contains the last attempt's outcome (when available) and termination
/// reason, and fires once whenever retry execution exits (success, stop
/// condition, predicate acceptance, or cancellation).
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
///     if state.reason == StopReason::StopCondition {
///         println!("stopped on attempt {}", state.attempt);
///     }
/// }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitState<'a, T, E> {
    /// The 1-indexed attempt number that just completed.
    pub attempt: u32,

    /// A reference to the final outcome.
    ///
    /// `None` only when cancellation happens before the first attempt starts.
    pub outcome: Option<&'a Result<T, E>>,

    /// Wall-clock time elapsed since the first attempt began.
    /// `None` when no clock is available.
    pub elapsed: Option<Duration>,

    /// Cumulative time spent sleeping across all attempts.
    pub total_wait: Duration,

    /// Why the retry loop terminated.
    pub reason: crate::stats::StopReason,
}

impl<'a, T, E> ExitState<'a, T, E> {
    /// Creates an exit state value for tests and custom integrations.
    #[must_use]
    pub const fn new(
        attempt: u32,
        outcome: Option<&'a Result<T, E>>,
        elapsed: Option<Duration>,
        total_wait: Duration,
        reason: crate::stats::StopReason,
    ) -> Self {
        Self {
            attempt,
            outcome,
            elapsed,
            total_wait,
            reason,
        }
    }
}

/// Read-only context passed only to the `before_attempt` hook.
///
/// Unlike [`AttemptState`], this does not contain the outcome of the previous
/// attempt or the next delay, because neither is available before the attempt
/// executes.
///
/// # Examples
///
/// ```
/// use tenacious::BeforeAttemptState;
/// use core::time::Duration;
///
/// fn on_before(state: &BeforeAttemptState) {
///     println!("starting attempt {}", state.attempt);
/// }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeforeAttemptState {
    /// The 1-indexed attempt number about to begin.
    pub attempt: u32,

    /// Wall-clock time elapsed since the first attempt began.
    /// `None` when no clock is available.
    pub elapsed: Option<Duration>,

    /// Cumulative time spent sleeping across all previous attempts.
    pub total_wait: Duration,
}

impl BeforeAttemptState {
    /// Creates a before-attempt state value for tests and custom integrations.
    #[must_use]
    pub const fn new(attempt: u32, elapsed: Option<Duration>, total_wait: Duration) -> Self {
        Self {
            attempt,
            elapsed,
            total_wait,
        }
    }
}
