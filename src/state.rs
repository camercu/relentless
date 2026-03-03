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
/// shared reference. Direct construction is available for testing and custom
/// strategy implementations.
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
#[derive(Debug)]
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

/// Read-only context passed to [`Predicate::should_retry`](crate::Predicate::should_retry)
/// and the `after_attempt` and `before_sleep` hooks.
///
/// This contains the attempt outcome plus timing/counting fields for the
/// completed attempt, mirroring the execution context needed by predicates and
/// hooks.
///
/// This struct is constructed internally by the execution engine and passed by
/// shared reference. It is never constructed by user code in normal usage.
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
#[derive(Debug)]
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

/// Final read-only context passed to the `on_exit` hook.
///
/// This contains the last attempt's outcome and termination reason, and fires
/// once whenever retry execution exits (success, stop condition, or predicate
/// acceptance).
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
#[derive(Debug)]
pub struct ExitState<'a, T, E> {
    /// The 1-indexed attempt number that just completed.
    pub attempt: u32,

    /// A reference to the final outcome.
    pub outcome: &'a Result<T, E>,

    /// Wall-clock time elapsed since the first attempt began.
    /// `None` when no clock is available.
    pub elapsed: Option<Duration>,

    /// Cumulative time spent sleeping across all attempts.
    pub total_wait: Duration,

    /// Why the retry loop terminated.
    pub reason: crate::stats::StopReason,
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
#[derive(Debug)]
pub struct BeforeAttemptState {
    /// The 1-indexed attempt number about to begin.
    pub attempt: u32,

    /// Wall-clock time elapsed since the first attempt began.
    /// `None` when no clock is available.
    pub elapsed: Option<Duration>,

    /// Cumulative time spent sleeping across all previous attempts.
    pub total_wait: Duration,
}
