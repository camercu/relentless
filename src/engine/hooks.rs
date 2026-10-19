//! Hook dispatch for the classifier engine.
//!
//! Three dispatch traits, each with a zero-cost `()` no-op impl, an `FnMut`
//! impl for user closures, and a [`HookChain`] impl so several hooks can be
//! registered at one point. Registering a hook means passing a closure to
//! `before_attempt`/`after_attempt`/`on_exit`; the traits are named only in
//! the `where` bounds of code generic over a configured builder.

use super::termination::StopReason;
use crate::compat::Duration;
use crate::state::RetryState;

/// Dispatch trait for before-attempt hooks (outcome-free context).
///
/// Satisfied by `()` (no hook registered), by any `FnMut(&RetryState)`, and by
/// [`HookChain`] (several hooks at one point). Name it only to bound a
/// builder's hook type parameter — see [`Retry`](crate::Retry).
pub trait BeforeAttemptHook {
    /// Invokes the hook with the state of the attempt about to run.
    fn call(&mut self, state: &RetryState);
}

impl BeforeAttemptHook for () {
    fn call(&mut self, _state: &RetryState) {}
}

impl<F: FnMut(&RetryState)> BeforeAttemptHook for F {
    fn call(&mut self, state: &RetryState) {
        (self)(state);
    }
}

/// Dispatch trait for after-attempt hooks, over the outcome type `O`.
///
/// The counterpart of [`BeforeAttemptHook`] for hooks that see the completed
/// attempt's outcome.
pub trait AttemptHook<O> {
    /// Invokes the hook with the state of the attempt that just completed.
    fn call(&mut self, state: &AttemptState<'_, O>);
}

impl<O> AttemptHook<O> for () {
    fn call(&mut self, _state: &AttemptState<'_, O>) {}
}

impl<O, F: for<'a> FnMut(&AttemptState<'a, O>)> AttemptHook<O> for F {
    fn call(&mut self, state: &AttemptState<'_, O>) {
        (self)(state);
    }
}

/// Dispatch trait for on-exit hooks, over the return/abort/outcome types.
///
/// The counterpart of [`BeforeAttemptHook`] for the loop's terminal hook.
pub trait ExitHook<R, A, O> {
    /// Invokes the hook with the loop's terminal outcome.
    fn call(&mut self, exit: &Exit<'_, R, A, O>);
}

impl<R, A, O> ExitHook<R, A, O> for () {
    fn call(&mut self, _exit: &Exit<'_, R, A, O>) {}
}

impl<R, A, O, F: for<'a> FnMut(&Exit<'a, R, A, O>)> ExitHook<R, A, O> for F {
    fn call(&mut self, exit: &Exit<'_, R, A, O>) {
        (self)(exit);
    }
}

/// Links two hooks of the same kind so both fire in registration order.
///
/// Registering a second hook at the same point wraps the pair in this type, so
/// it appears in a builder's type after two `before_attempt` calls. Name it
/// only to write that type out — when a helper returns a builder that already
/// has hooks attached.
#[derive(Debug, Clone)]
pub struct HookChain<First, Second> {
    first: First,
    second: Second,
}

impl<First, Second> HookChain<First, Second> {
    fn new(first: First, second: Second) -> Self {
        Self { first, second }
    }
}

impl<First: BeforeAttemptHook, Second: BeforeAttemptHook> BeforeAttemptHook
    for HookChain<First, Second>
{
    fn call(&mut self, state: &RetryState) {
        self.first.call(state);
        self.second.call(state);
    }
}

impl<O, First: AttemptHook<O>, Second: AttemptHook<O>> AttemptHook<O> for HookChain<First, Second> {
    fn call(&mut self, state: &AttemptState<'_, O>) {
        self.first.call(state);
        self.second.call(state);
    }
}

impl<R, A, O, First: ExitHook<R, A, O>, Second: ExitHook<R, A, O>> ExitHook<R, A, O>
    for HookChain<First, Second>
{
    fn call(&mut self, exit: &Exit<'_, R, A, O>) {
        self.first.call(exit);
        self.second.call(exit);
    }
}

/// The three hook slots carried by the builder and driven by the loop.
#[derive(Clone)]
pub(crate) struct ExecutionHooks<BA, AA, OX> {
    pub before_attempt: BA,
    pub after_attempt: AA,
    pub on_exit: OX,
}

impl ExecutionHooks<(), (), ()> {
    pub(crate) fn new() -> Self {
        Self {
            before_attempt: (),
            after_attempt: (),
            on_exit: (),
        }
    }
}

impl<BA, AA, OX> ExecutionHooks<BA, AA, OX> {
    pub(crate) fn chain_before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<HookChain<BA, Hook>, AA, OX> {
        ExecutionHooks {
            before_attempt: HookChain::new(self.before_attempt, hook),
            after_attempt: self.after_attempt,
            on_exit: self.on_exit,
        }
    }

    pub(crate) fn chain_after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<BA, HookChain<AA, Hook>, OX> {
        ExecutionHooks {
            before_attempt: self.before_attempt,
            after_attempt: HookChain::new(self.after_attempt, hook),
            on_exit: self.on_exit,
        }
    }

    pub(crate) fn chain_on_exit<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<BA, AA, HookChain<OX, Hook>> {
        ExecutionHooks {
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            on_exit: HookChain::new(self.on_exit, hook),
        }
    }
}

// The outcome-carrying views handed to the hooks above. `RetryState` (the
// pre-attempt, outcome-free context for the operation and `before_attempt`)
// lives in `crate::state`.

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
