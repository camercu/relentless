use crate::state::{AttemptState, ExitState, RetryState};

/// Sealed dispatch trait for before-attempt hooks.
///
/// The `()` impl is the zero-cost no-op used when no hook is configured;
/// the `FnMut` impl handles user-supplied closures. Both cases are monomorphised
/// away at compile time with no runtime overhead.
#[doc(hidden)]
pub(crate) trait BeforeAttemptHook {
    fn call(&mut self, state: &RetryState);
}

impl BeforeAttemptHook for () {
    fn call(&mut self, _state: &RetryState) {}
}

impl<F> BeforeAttemptHook for F
where
    F: FnMut(&RetryState),
{
    fn call(&mut self, state: &RetryState) {
        (self)(state);
    }
}

/// Sealed dispatch trait for after-attempt hooks.
///
/// Same `()` / `FnMut` dual-impl pattern as [`BeforeAttemptHook`].
#[doc(hidden)]
pub(crate) trait AttemptHook<T, E> {
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

/// Sealed dispatch trait for on-exit hooks.
///
/// Same `()` / `FnMut` dual-impl pattern as [`BeforeAttemptHook`].
#[doc(hidden)]
pub(crate) trait ExitHook<T, E> {
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

/// Links two hooks of the same kind so both are called in registration order.
///
/// Each call to `before_attempt`, `after_attempt`, or `on_exit` wraps the
/// existing hook and the new one into a `HookChain`, building a linked list
/// of callbacks in the type system rather than at runtime.
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
    fn call(&mut self, state: &RetryState) {
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
pub(crate) struct ExecutionHooks<BA, AA, OX> {
    pub(crate) before_attempt: BA,
    pub(crate) after_attempt: AA,
    pub(crate) on_exit: OX,
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

#[cfg(feature = "alloc")]
impl<BA, AA, OX> ExecutionHooks<BA, AA, OX> {
    pub(crate) fn chain_before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<HookChain<BA, Hook>, AA, OX> {
        let Self {
            before_attempt,
            after_attempt,
            on_exit,
        } = self;
        ExecutionHooks {
            before_attempt: HookChain::new(before_attempt, hook),
            after_attempt,
            on_exit,
        }
    }

    pub(crate) fn chain_after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<BA, HookChain<AA, Hook>, OX> {
        let Self {
            before_attempt,
            after_attempt,
            on_exit,
        } = self;
        ExecutionHooks {
            before_attempt,
            after_attempt: HookChain::new(after_attempt, hook),
            on_exit,
        }
    }

    pub(crate) fn chain_on_exit<Hook>(
        self,
        hook: Hook,
    ) -> ExecutionHooks<BA, AA, HookChain<OX, Hook>> {
        let Self {
            before_attempt,
            after_attempt,
            on_exit,
        } = self;
        ExecutionHooks {
            before_attempt,
            after_attempt,
            on_exit: HookChain::new(on_exit, hook),
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<AA, OX> ExecutionHooks<(), AA, OX> {
    pub(crate) fn set_before_attempt<Hook>(self, hook: Hook) -> ExecutionHooks<Hook, AA, OX> {
        let Self {
            after_attempt,
            on_exit,
            ..
        } = self;
        ExecutionHooks {
            before_attempt: hook,
            after_attempt,
            on_exit,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<BA, OX> ExecutionHooks<BA, (), OX> {
    pub(crate) fn set_after_attempt<Hook>(self, hook: Hook) -> ExecutionHooks<BA, Hook, OX> {
        let Self {
            before_attempt,
            on_exit,
            ..
        } = self;
        ExecutionHooks {
            before_attempt,
            after_attempt: hook,
            on_exit,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<BA, AA> ExecutionHooks<BA, AA, ()> {
    pub(crate) fn set_on_exit<Hook>(self, hook: Hook) -> ExecutionHooks<BA, AA, Hook> {
        let Self {
            before_attempt,
            after_attempt,
            ..
        } = self;
        ExecutionHooks {
            before_attempt,
            after_attempt,
            on_exit: hook,
        }
    }
}
