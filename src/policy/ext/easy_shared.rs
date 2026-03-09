use crate::cancel::{Canceler, NeverCancel};
use crate::compat::{Box, Duration, Vec};
#[cfg(feature = "std")]
use crate::policy::execution::sync_exec::{NoSyncSleep, SyncSleep};
use crate::policy::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, RetryPolicy};
use crate::predicate::Predicate;
use crate::state::{AttemptState, BeforeAttemptState, ExitState};
use crate::{on, stop, wait};
use crate::{stop::Stop, wait::Wait};
use core::future::Future;
use core::pin::Pin;

pub(crate) type ErasedPolicy<'a, T, E> =
    RetryPolicy<Box<dyn Stop + 'a>, Box<dyn Wait + 'a>, Box<dyn Predicate<T, E> + 'a>>;

pub(crate) fn default_erased_policy<'a, T, E>() -> ErasedPolicy<'a, T, E> {
    RetryPolicy::default()
        .stop(Box::new(stop::attempts(3)) as Box<dyn Stop + 'a>)
        .wait(Box::new(wait::exponential(Duration::from_millis(100))) as Box<dyn Wait + 'a>)
        .when(Box::new(on::any_error()) as Box<dyn Predicate<T, E> + 'a>)
}

pub(crate) struct VecBeforeHooks<'a> {
    hooks: Vec<Box<dyn BeforeAttemptHook + 'a>>,
}

impl<'a> VecBeforeHooks<'a> {
    pub(crate) fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub(crate) fn push<Hook>(&mut self, hook: Hook)
    where
        Hook: FnMut(&BeforeAttemptState) + 'a,
    {
        self.hooks.push(Box::new(hook));
    }
}

impl BeforeAttemptHook for VecBeforeHooks<'_> {
    fn call(&mut self, state: &BeforeAttemptState) {
        for hook in &mut self.hooks {
            hook.call(state);
        }
    }
}

pub(crate) struct VecAttemptHooks<'a, T, E> {
    hooks: Vec<Box<dyn AttemptHook<T, E> + 'a>>,
}

impl<'a, T, E> VecAttemptHooks<'a, T, E> {
    pub(crate) fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub(crate) fn push<Hook>(&mut self, hook: Hook)
    where
        Hook: for<'state> FnMut(&AttemptState<'state, T, E>) + 'a,
    {
        self.hooks.push(Box::new(hook));
    }
}

impl<T, E> AttemptHook<T, E> for VecAttemptHooks<'_, T, E> {
    fn call(&mut self, state: &AttemptState<'_, T, E>) {
        for hook in &mut self.hooks {
            hook.call(state);
        }
    }
}

pub(crate) struct VecExitHooks<'a, T, E> {
    hooks: Vec<Box<dyn ExitHook<T, E> + 'a>>,
}

impl<'a, T, E> VecExitHooks<'a, T, E> {
    pub(crate) fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub(crate) fn push<Hook>(&mut self, hook: Hook)
    where
        Hook: for<'state> FnMut(&ExitState<'state, T, E>) + 'a,
    {
        self.hooks.push(Box::new(hook));
    }
}

impl<T, E> ExitHook<T, E> for VecExitHooks<'_, T, E> {
    fn call(&mut self, state: &ExitState<'_, T, E>) {
        for hook in &mut self.hooks {
            hook.call(state);
        }
    }
}

pub(crate) fn empty_erased_hooks<'a, T, E>()
-> ExecutionHooks<VecBeforeHooks<'a>, VecAttemptHooks<'a, T, E>, VecExitHooks<'a, T, E>> {
    ExecutionHooks {
        before_attempt: VecBeforeHooks::new(),
        after_attempt: VecAttemptHooks::new(),
        on_exit: VecExitHooks::new(),
    }
}

trait DynCanceler<'a> {
    fn is_cancelled(&self) -> bool;
    fn cancel(&self) -> Pin<Box<dyn Future<Output = ()> + 'a>>;
}

impl<'a, C> DynCanceler<'a> for C
where
    C: Canceler + 'a,
    C::Cancel: 'a,
{
    fn is_cancelled(&self) -> bool {
        Canceler::is_cancelled(self)
    }

    fn cancel(&self) -> Pin<Box<dyn Future<Output = ()> + 'a>> {
        Box::pin(Canceler::cancel(self))
    }
}

pub(crate) struct ErasedCanceler<'a> {
    inner: Box<dyn DynCanceler<'a> + 'a>,
}

impl<'a> ErasedCanceler<'a> {
    pub(crate) fn new<C>(canceler: C) -> Self
    where
        C: Canceler + 'a,
        C::Cancel: 'a,
    {
        Self {
            inner: Box::new(canceler),
        }
    }
}

impl Default for ErasedCanceler<'_> {
    fn default() -> Self {
        Self::new(NeverCancel)
    }
}

impl<'a> Canceler for ErasedCanceler<'a> {
    type Cancel = Pin<Box<dyn Future<Output = ()> + 'a>>;

    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    fn cancel(&self) -> Self::Cancel {
        self.inner.cancel()
    }
}

#[cfg(feature = "std")]
pub(crate) fn default_sync_sleep<'a>() -> Box<dyn SyncSleep + 'a> {
    Box::new(NoSyncSleep)
}

#[cfg(feature = "std")]
pub(crate) fn boxed_sync_sleep<'a, SleepFn>(sleeper: SleepFn) -> Box<dyn SyncSleep + 'a>
where
    SleepFn: FnMut(Duration) + 'a,
{
    Box::new(sleeper)
}

#[cfg(feature = "std")]
impl SyncSleep for Box<dyn SyncSleep + '_> {
    fn sleep(&mut self, dur: Duration) {
        (**self).sleep(dur);
    }
}

trait DynSleeper<'a> {
    fn sleep(&self, dur: Duration) -> Pin<Box<dyn Future<Output = ()> + 'a>>;
}

impl<'a, SleepImpl> DynSleeper<'a> for SleepImpl
where
    SleepImpl: crate::sleep::Sleeper + 'a,
    SleepImpl::Sleep: 'a,
{
    fn sleep(&self, dur: Duration) -> Pin<Box<dyn Future<Output = ()> + 'a>> {
        Box::pin(crate::sleep::Sleeper::sleep(self, dur))
    }
}

pub(crate) struct ErasedAsyncSleep<'a> {
    inner: Box<dyn DynSleeper<'a> + 'a>,
}

impl<'a> ErasedAsyncSleep<'a> {
    pub(crate) fn new<SleepImpl>(sleeper: SleepImpl) -> Self
    where
        SleepImpl: crate::sleep::Sleeper + 'a,
        SleepImpl::Sleep: 'a,
    {
        Self {
            inner: Box::new(sleeper),
        }
    }
}

impl<'a> crate::sleep::Sleeper for ErasedAsyncSleep<'a> {
    type Sleep = Pin<Box<dyn Future<Output = ()> + 'a>>;

    fn sleep(&self, dur: Duration) -> Self::Sleep {
        self.inner.sleep(dur)
    }
}
