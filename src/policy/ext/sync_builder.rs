use core::marker::PhantomData;

#[cfg(feature = "alloc")]
use super::super::HookChain;
use super::super::execution::common::execute_sync_loop;
use super::super::execution::sync_exec::{NoSyncSleep, SyncSleep};
use super::super::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, RetryPolicy};
use crate::cancel::{Canceler, NeverCancel};
use crate::compat::Duration;
use crate::predicate::Predicate;
use crate::state::{AttemptState, BeforeAttemptState, ExitState};
use crate::{
    RetryError, RetryStats, on,
    stop::{self, Stop},
    wait::{self, Wait},
};

/// Extension trait to start sync retries directly from a closure/function.
pub trait RetryExt<T, E>: FnMut() -> Result<T, E> + Sized {
    /// Starts an owned sync retry builder from [`RetryPolicy::default()`].
    ///
    /// This means extension-based retries default to:
    /// - `stop::attempts(3)`
    /// - exponential backoff starting at 100ms
    /// - retry on any error
    ///
    /// ```
    /// use tenacious::RetryExt;
    ///
    /// let _ = (|| Ok::<(), &str>(()))
    ///     .retry()
    ///     .sleep(|_| {})
    ///     .call();
    /// ```
    fn retry(
        self,
    ) -> SyncRetryBuilder<
        stop::StopAfterAttempts,
        wait::WaitExponential,
        on::AnyError,
        (),
        (),
        (),
        Self,
        NoSyncSleep,
        T,
        E,
        NeverCancel,
    >;
}

impl<T, E, F> RetryExt<T, E> for F
where
    F: FnMut() -> Result<T, E> + Sized,
{
    fn retry(
        self,
    ) -> SyncRetryBuilder<
        stop::StopAfterAttempts,
        wait::WaitExponential,
        on::AnyError,
        (),
        (),
        (),
        Self,
        NoSyncSleep,
        T,
        E,
        NeverCancel,
    > {
        SyncRetryBuilder {
            policy: RetryPolicy::default(),
            hooks: ExecutionHooks::new(),
            op: self,
            sleeper: NoSyncSleep,
            canceler: NeverCancel,
            _marker: PhantomData,
        }
    }
}

#[cfg(not(feature = "std"))]
#[doc(hidden)]
/// ```compile_fail
/// use tenacious::RetryExt;
///
/// let _ = (|| Err::<(), &str>("fail"))
///     .retry()
///     .call();
/// ```
#[allow(dead_code)]
fn _sync_retry_builder_requires_sleep_in_no_std() {}

/// Owned sync retry builder created from [`RetryExt::retry`].
pub struct SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C = NeverCancel> {
    policy: RetryPolicy<S, W, P>,
    hooks: ExecutionHooks<BA, AA, OX>,
    op: F,
    sleeper: SleepFn,
    canceler: C,
    _marker: PhantomData<fn() -> (T, E)>,
}

/// Owned sync retry builder wrapper that returns statistics.
pub struct SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E, C = NeverCancel> {
    inner: SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>,
}

#[cfg(feature = "alloc")]
type SyncBuilderWithBeforeHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetryBuilder<S, W, P, HookChain<BA, Hook>, AA, OX, F, SleepFn, T, E, C>;

#[cfg(feature = "alloc")]
type SyncBuilderWithAfterHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetryBuilder<S, W, P, BA, HookChain<AA, Hook>, OX, F, SleepFn, T, E, C>;

#[cfg(feature = "alloc")]
type SyncBuilderWithOnExitHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetryBuilder<S, W, P, BA, AA, HookChain<OX, Hook>, F, SleepFn, T, E, C>;

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
{
    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> SyncRetryBuilder<S, W, P, NewBA, NewAA, NewOX, F, SleepFn, T, E, C> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            ..
        } = self;
        SyncRetryBuilder {
            policy,
            hooks: map(hooks),
            op,
            sleeper,
            canceler,
            _marker: PhantomData,
        }
    }

    /// Replaces the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(
        self,
        stop: NewStop,
    ) -> SyncRetryBuilder<NewStop, W, P, BA, AA, OX, F, SleepFn, T, E, C> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            ..
        } = self;
        SyncRetryBuilder {
            policy: policy.stop(stop),
            hooks,
            op,
            sleeper,
            canceler,
            _marker: PhantomData,
        }
    }

    /// Replaces the wait strategy.
    #[must_use]
    pub fn wait<NewWait>(
        self,
        wait: NewWait,
    ) -> SyncRetryBuilder<S, NewWait, P, BA, AA, OX, F, SleepFn, T, E, C> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            ..
        } = self;
        SyncRetryBuilder {
            policy: policy.wait(wait),
            hooks,
            op,
            sleeper,
            canceler,
            _marker: PhantomData,
        }
    }

    /// Replaces the retry predicate.
    #[must_use]
    pub fn when<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> SyncRetryBuilder<S, W, NewPredicate, BA, AA, OX, F, SleepFn, T, E, C> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            ..
        } = self;
        SyncRetryBuilder {
            policy: policy.when(predicate),
            hooks,
            op,
            sleeper,
            canceler,
            _marker: PhantomData,
        }
    }

    /// Configures a custom elapsed clock for elapsed-based stop conditions.
    #[must_use]
    pub fn elapsed_clock(self, clock: fn() -> Duration) -> Self {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            ..
        } = self;
        SyncRetryBuilder {
            policy: policy.elapsed_clock(clock),
            hooks,
            op,
            sleeper,
            canceler,
            _marker: PhantomData,
        }
    }

    /// Sets the blocking sleep implementation.
    #[must_use]
    pub fn sleep<NewSleep>(
        self,
        sleeper: NewSleep,
    ) -> SyncRetryBuilder<S, W, P, BA, AA, OX, F, NewSleep, T, E, C> {
        let Self {
            policy,
            hooks,
            op,
            canceler,
            ..
        } = self;
        SyncRetryBuilder {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            _marker: PhantomData,
        }
    }
}

#[cfg(feature = "alloc")]
// Intentional: hook chaining preserves type-state and avoids runtime
// indirection; signatures are long but mechanically structured.
#[allow(clippy::type_complexity)]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
{
    /// Appends a before-attempt hook.
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncBuilderWithBeforeHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        self.map_hooks(|hooks| hooks.chain_before_attempt(hook))
    }

    /// Appends an after-attempt hook.
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncBuilderWithAfterHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.chain_after_attempt(hook))
    }

    /// Appends an on-exit hook.
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> SyncBuilderWithOnExitHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.chain_on_exit(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, AA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, (), AA, OX, F, SleepFn, T, E, C>
{
    /// Sets the sole before-attempt hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryExt, stop};
    ///
    /// let _ = (|| Err::<(), _>("fail"))
    ///     .retry()
    ///     .stop(stop::attempts(1))
    ///     .before_attempt(|_state| {})
    ///     .before_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryBuilder<S, W, P, Hook, AA, OX, F, SleepFn, T, E, C>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, (), OX, F, SleepFn, T, E, C>
{
    /// Sets the sole after-attempt hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryExt, stop};
    ///
    /// let _ = (|| Err::<(), _>("fail"))
    ///     .retry()
    ///     .stop(stop::attempts(1))
    ///     .after_attempt(|_state| {})
    ///     .after_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryBuilder<S, W, P, BA, Hook, OX, F, SleepFn, T, E, C>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, (), F, SleepFn, T, E, C>
{
    /// Sets the sole on-exit hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryExt, stop};
    ///
    /// let _ = (|| Err::<(), _>("fail"))
    ///     .retry()
    ///     .stop(stop::attempts(1))
    ///     .on_exit(|_state| {})
    ///     .on_exit(|_state| {});
    /// ```
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryBuilder<S, W, P, BA, AA, Hook, F, SleepFn, T, E, C>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_on_exit(hook))
    }
}

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E>
    SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, NeverCancel>
{
    /// Attaches a canceler that is checked before each attempt and after each sleep.
    #[must_use]
    pub fn cancel_on<NewC: Canceler>(
        self,
        canceler: NewC,
    ) -> SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, NewC> {
        SyncRetryBuilder {
            policy: self.policy,
            hooks: self.hooks,
            op: self.op,
            sleeper: self.sleeper,
            canceler,
            _marker: PhantomData,
        }
    }
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Result<T, E>,
    SleepFn: SyncSleep,
    C: Canceler,
{
    /// Executes the sync retry loop.
    pub fn call(self) -> Result<T, RetryError<E, T>> {
        self.execute::<false>().0
    }

    /// Executes the sync retry loop and returns aggregate statistics.
    #[must_use]
    pub fn with_stats(self) -> SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E, C> {
        SyncRetryBuilderWithStats { inner: self }
    }

    fn execute<const COLLECT_STATS: bool>(
        mut self,
    ) -> (Result<T, RetryError<E, T>>, Option<RetryStats>) {
        self.policy.stop.reset();
        self.policy.wait.reset();
        execute_sync_loop::<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, COLLECT_STATS>(
            &mut self.policy,
            &mut self.hooks,
            &mut self.op,
            &mut self.sleeper,
            &self.canceler,
        )
    }
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Result<T, E>,
    SleepFn: SyncSleep,
    C: Canceler,
{
    /// Executes the sync retry loop and returns `(result, stats)`.
    pub fn call(self) -> (Result<T, RetryError<E, T>>, RetryStats) {
        let (result, stats) = self.inner.execute::<true>();
        (
            result,
            stats.expect("sync retry builder completed without stats"),
        )
    }
}
