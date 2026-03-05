use crate::state::{AttemptState, BeforeAttemptState, ExitState};

/// Hook callback shape for the `before_attempt` hook.
#[doc(hidden)]
pub(crate) trait BeforeAttemptHook {
    /// Invokes the hook.
    fn call(&mut self, state: &BeforeAttemptState);
}

impl BeforeAttemptHook for () {
    fn call(&mut self, _state: &BeforeAttemptState) {}
}

impl<F> BeforeAttemptHook for F
where
    F: FnMut(&BeforeAttemptState),
{
    fn call(&mut self, state: &BeforeAttemptState) {
        (self)(state);
    }
}

/// Hook callback shape for hooks receiving an [`AttemptState`].
#[doc(hidden)]
pub(crate) trait AttemptHook<T, E> {
    /// Invokes the hook.
    fn call(&mut self, state: &AttemptState<'_, T, E>);
}

impl<T, E> AttemptHook<T, E> for () {
    fn call(&mut self, _state: &AttemptState<'_, T, E>) {}
}

impl<T, E, F> AttemptHook<T, E> for F
where
    F: for<'a> FnMut(&AttemptState<'a, T, E>),
{
    fn call(&mut self, state: &AttemptState<'_, T, E>) {
        (self)(state);
    }
}

/// Hook callback shape for the `on_exit` hook.
#[doc(hidden)]
pub(crate) trait ExitHook<T, E> {
    /// Invokes the hook.
    fn call(&mut self, state: &ExitState<'_, T, E>);
}

impl<T, E> ExitHook<T, E> for () {
    fn call(&mut self, _state: &ExitState<'_, T, E>) {}
}

impl<T, E, F> ExitHook<T, E> for F
where
    F: for<'a> FnMut(&ExitState<'a, T, E>),
{
    fn call(&mut self, state: &ExitState<'_, T, E>) {
        (self)(state);
    }
}

/// Internal hook-chain wrapper used when multiple hooks are appended.
#[doc(hidden)]
#[derive(Clone)]
#[cfg(feature = "alloc")]
pub struct HookChain<First, Second> {
    first: First,
    second: Second,
}

#[cfg(feature = "alloc")]
impl<First, Second> HookChain<First, Second> {
    pub(crate) fn new(first: First, second: Second) -> Self {
        Self { first, second }
    }
}

#[cfg(feature = "alloc")]
impl<First, Second> BeforeAttemptHook for HookChain<First, Second>
where
    First: BeforeAttemptHook,
    Second: BeforeAttemptHook,
{
    fn call(&mut self, state: &BeforeAttemptState) {
        self.first.call(state);
        self.second.call(state);
    }
}

#[cfg(feature = "alloc")]
impl<T, E, First, Second> AttemptHook<T, E> for HookChain<First, Second>
where
    First: AttemptHook<T, E>,
    Second: AttemptHook<T, E>,
{
    fn call(&mut self, state: &AttemptState<'_, T, E>) {
        self.first.call(state);
        self.second.call(state);
    }
}

#[cfg(feature = "alloc")]
impl<T, E, First, Second> ExitHook<T, E> for HookChain<First, Second>
where
    First: ExitHook<T, E>,
    Second: ExitHook<T, E>,
{
    fn call(&mut self, state: &ExitState<'_, T, E>) {
        self.first.call(state);
        self.second.call(state);
    }
}

#[derive(Clone)]
pub(crate) struct ExecutionHooks<BA, AA, BS, OX> {
    pub(crate) before_attempt: BA,
    pub(crate) after_attempt: AA,
    pub(crate) before_sleep: BS,
    pub(crate) on_exit: OX,
}

impl ExecutionHooks<(), (), (), ()> {
    pub(crate) fn new() -> Self {
        Self {
            before_attempt: (),
            after_attempt: (),
            before_sleep: (),
            on_exit: (),
        }
    }
}

#[cfg(feature = "alloc")]
impl<BA, AA, BS, OX> ExecutionHooks<BA, AA, BS, OX> {
    pub(crate) fn chain_before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<HookChain<BA, Hook>, AA, BS, OX> {
        let Self {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exit,
        } = self;
        ExecutionHooks {
            before_attempt: HookChain::new(before_attempt, hook),
            after_attempt,
            before_sleep,
            on_exit,
        }
    }

    pub(crate) fn chain_after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<BA, HookChain<AA, Hook>, BS, OX> {
        let Self {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exit,
        } = self;
        ExecutionHooks {
            before_attempt,
            after_attempt: HookChain::new(after_attempt, hook),
            before_sleep,
            on_exit,
        }
    }

    pub(crate) fn chain_before_sleep<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<BA, AA, HookChain<BS, Hook>, OX> {
        let Self {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exit,
        } = self;
        ExecutionHooks {
            before_attempt,
            after_attempt,
            before_sleep: HookChain::new(before_sleep, hook),
            on_exit,
        }
    }

    pub(crate) fn chain_on_exit<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<BA, AA, BS, HookChain<OX, Hook>> {
        let Self {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exit,
        } = self;
        ExecutionHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exit: HookChain::new(on_exit, hook),
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<AA, BS, OX> ExecutionHooks<(), AA, BS, OX> {
    pub(crate) fn set_before_attempt<Hook>(self, hook: Hook) -> ExecutionHooks<Hook, AA, BS, OX> {
        let Self {
            after_attempt,
            before_sleep,
            on_exit,
            ..
        } = self;
        ExecutionHooks {
            before_attempt: hook,
            after_attempt,
            before_sleep,
            on_exit,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<BA, BS, OX> ExecutionHooks<BA, (), BS, OX> {
    pub(crate) fn set_after_attempt<Hook>(self, hook: Hook) -> ExecutionHooks<BA, Hook, BS, OX> {
        let Self {
            before_attempt,
            before_sleep,
            on_exit,
            ..
        } = self;
        ExecutionHooks {
            before_attempt,
            after_attempt: hook,
            before_sleep,
            on_exit,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<BA, AA, OX> ExecutionHooks<BA, AA, (), OX> {
    pub(crate) fn set_before_sleep<Hook>(self, hook: Hook) -> ExecutionHooks<BA, AA, Hook, OX> {
        let Self {
            before_attempt,
            after_attempt,
            on_exit,
            ..
        } = self;
        ExecutionHooks {
            before_attempt,
            after_attempt,
            before_sleep: hook,
            on_exit,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<BA, AA, BS> ExecutionHooks<BA, AA, BS, ()> {
    pub(crate) fn set_on_exit<Hook>(self, hook: Hook) -> ExecutionHooks<BA, AA, BS, Hook> {
        let Self {
            before_attempt,
            after_attempt,
            before_sleep,
            ..
        } = self;
        ExecutionHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exit: hook,
        }
    }
}
