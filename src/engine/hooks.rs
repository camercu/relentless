//! Hook dispatch for the classifier engine.
//!
//! Three sealed dispatch traits, each with a zero-cost `()` no-op impl, an
//! `FnMut` impl for user closures, and a [`HookChain`] impl so several hooks can
//! be registered at one point. Users never name these traits; they pass
//! closures to `before_attempt`/`after_attempt`/`on_exit`.

use super::state::{AttemptState, Exit};
use crate::state::RetryState;

/// Dispatch trait for before-attempt hooks (outcome-free context).
pub trait BeforeAttemptHook {
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
pub trait AttemptHook<O> {
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
pub trait ExitHook<R, A, O> {
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
#[derive(Clone)]
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
pub struct ExecutionHooks<BA, AA, OX> {
    pub before_attempt: BA,
    pub after_attempt: AA,
    pub on_exit: OX,
}

impl ExecutionHooks<(), (), ()> {
    pub fn new() -> Self {
        Self {
            before_attempt: (),
            after_attempt: (),
            on_exit: (),
        }
    }
}

impl<BA, AA, OX> ExecutionHooks<BA, AA, OX> {
    pub fn chain_before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<HookChain<BA, Hook>, AA, OX> {
        ExecutionHooks {
            before_attempt: HookChain::new(self.before_attempt, hook),
            after_attempt: self.after_attempt,
            on_exit: self.on_exit,
        }
    }

    pub fn chain_after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<BA, HookChain<AA, Hook>, OX> {
        ExecutionHooks {
            before_attempt: self.before_attempt,
            after_attempt: HookChain::new(self.after_attempt, hook),
            on_exit: self.on_exit,
        }
    }

    pub fn chain_on_exit<Hook>(self, hook: Hook) -> ExecutionHooks<BA, AA, HookChain<OX, Hook>> {
        ExecutionHooks {
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            on_exit: HookChain::new(self.on_exit, hook),
        }
    }
}
