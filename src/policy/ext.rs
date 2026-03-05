use core::marker::PhantomData;

use super::common::execute_sync_loop;
#[cfg(feature = "alloc")]
use super::common::{
    AsyncOperationPoll, AsyncPhase, AsyncPhaseProj, fire_before_attempt, poll_after_completion,
    poll_operation_future,
};
#[cfg(feature = "alloc")]
use super::time::ElapsedTracker;
use super::*;
#[cfg(feature = "alloc")]
use crate::StopReason;
#[cfg(feature = "alloc")]
use crate::sleep::Sleeper;
use crate::state::{AttemptState, BeforeAttemptState, ExitState};
use crate::{
    RetryError, RetryStats, on,
    stop::{self, Stop},
    wait::{self, Wait},
};

use super::sync_retry::{NoSyncSleep, SyncSleep};
use crate::cancel::{Canceler, NeverCancel};

/// Extension trait to start sync retries directly from a closure/function.
pub trait RetryExt<T, E>: FnMut() -> Result<T, E> + Sized {
    /// Starts an owned sync retry builder with default wait/predicate and
    /// an unconfigured stop strategy.
    ///
    /// `.stop(...)` must be configured before `.call()` is available.
    ///
    /// ```compile_fail
    /// use tenacious::RetryExt;
    ///
    /// let _ = (|| Ok::<(), &str>(())).retry().call();
    /// ```
    fn retry(
        self,
    ) -> SyncRetryBuilder<
        stop::NeedsStop,
        wait::WaitFixed,
        on::AnyError,
        (),
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
        stop::NeedsStop,
        wait::WaitFixed,
        on::AnyError,
        (),
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
            policy: RetryPolicy::new(),
            hooks: ExecutionHooks::new(),
            op: self,
            sleeper: NoSyncSleep,
            canceler: NeverCancel,
            _marker: PhantomData,
        }
    }
}

/// Owned sync retry builder created from [`RetryExt::retry`].
pub struct SyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C = NeverCancel> {
    policy: RetryPolicy<S, W, P>,
    hooks: ExecutionHooks<BA, AA, BS, OX>,
    op: F,
    sleeper: SleepFn,
    canceler: C,
    _marker: PhantomData<fn() -> (T, E)>,
}

/// Owned sync retry builder wrapper that returns statistics.
pub struct SyncRetryBuilderWithStats<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C = NeverCancel> {
    inner: SyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C>,
}

#[cfg(feature = "alloc")]
type SyncBuilderWithBeforeHook<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetryBuilder<S, W, P, HookChain<BA, Hook>, AA, BS, OX, F, SleepFn, T, E, C>;

#[cfg(feature = "alloc")]
type SyncBuilderWithAfterHook<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetryBuilder<S, W, P, BA, HookChain<AA, Hook>, BS, OX, F, SleepFn, T, E, C>;

#[cfg(feature = "alloc")]
type SyncBuilderWithBeforeSleepHook<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetryBuilder<S, W, P, BA, AA, HookChain<BS, Hook>, OX, F, SleepFn, T, E, C>;

#[cfg(feature = "alloc")]
type SyncBuilderWithOnExitHook<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetryBuilder<S, W, P, BA, AA, BS, HookChain<OX, Hook>, F, SleepFn, T, E, C>;

impl<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C>
{
    fn map_hooks<NewBA, NewAA, NewBS, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, BS, OX>) -> ExecutionHooks<NewBA, NewAA, NewBS, NewOX>,
    ) -> SyncRetryBuilder<S, W, P, NewBA, NewAA, NewBS, NewOX, F, SleepFn, T, E, C> {
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
    ) -> SyncRetryBuilder<NewStop, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C> {
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
    ) -> SyncRetryBuilder<S, NewWait, P, BA, AA, BS, OX, F, SleepFn, T, E, C> {
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
    ) -> SyncRetryBuilder<S, W, NewPredicate, BA, AA, BS, OX, F, SleepFn, T, E, C> {
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
    ) -> SyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, NewSleep, T, E, C> {
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
impl<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C>
{
    /// Appends a before-attempt hook.
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncBuilderWithBeforeHook<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C, Hook>
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
    ) -> SyncBuilderWithAfterHook<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C, Hook>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.chain_after_attempt(hook))
    }

    /// Appends a before-sleep hook.
    #[must_use]
    pub fn before_sleep<Hook>(
        self,
        hook: Hook,
    ) -> SyncBuilderWithBeforeSleepHook<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C, Hook>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.chain_before_sleep(hook))
    }

    /// Appends an on-exit hook.
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> SyncBuilderWithOnExitHook<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C, Hook>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.chain_on_exit(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, AA, BS, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, (), AA, BS, OX, F, SleepFn, T, E, C>
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
    ) -> SyncRetryBuilder<S, W, P, Hook, AA, BS, OX, F, SleepFn, T, E, C>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, BS, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, (), BS, OX, F, SleepFn, T, E, C>
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
    ) -> SyncRetryBuilder<S, W, P, BA, Hook, BS, OX, F, SleepFn, T, E, C>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, (), OX, F, SleepFn, T, E, C>
{
    /// Sets the sole before-sleep hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryExt, stop};
    ///
    /// let _ = (|| Err::<(), _>("fail"))
    ///     .retry()
    ///     .stop(stop::attempts(1))
    ///     .before_sleep(|_state| {})
    ///     .before_sleep(|_state| {});
    /// ```
    #[must_use]
    pub fn before_sleep<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryBuilder<S, W, P, BA, AA, Hook, OX, F, SleepFn, T, E, C>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_before_sleep(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, BS, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, BS, (), F, SleepFn, T, E, C>
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
    ) -> SyncRetryBuilder<S, W, P, BA, AA, BS, Hook, F, SleepFn, T, E, C>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_on_exit(hook))
    }
}

impl<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E>
    SyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, NeverCancel>
{
    /// Attaches a canceler that is checked before each attempt and after each sleep.
    #[must_use]
    pub fn cancel_on<NewC: Canceler>(
        self,
        canceler: NewC,
    ) -> SyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, NewC> {
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

impl<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
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
    pub fn with_stats(
        self,
    ) -> SyncRetryBuilderWithStats<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C> {
        SyncRetryBuilderWithStats { inner: self }
    }

    fn execute<const COLLECT_STATS: bool>(
        mut self,
    ) -> (Result<T, RetryError<E, T>>, Option<RetryStats>) {
        self.policy.stop.reset();
        self.policy.wait.reset();
        execute_sync_loop::<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C, COLLECT_STATS>(
            &mut self.policy,
            &mut self.hooks,
            &mut self.op,
            &mut self.sleeper,
            &self.canceler,
        )
    }
}

impl<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C>
    SyncRetryBuilderWithStats<S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, C>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
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

#[cfg(feature = "alloc")]
mod async_ext {
    use core::future::Future;
    use core::marker::PhantomData;
    use core::pin::Pin;
    use core::task::{Context, Poll};

    use pin_project_lite::pin_project;

    use super::*;
    use crate::policy::async_retry::NoAsyncSleep;

    /// Extension trait to start async retries directly from a closure/function.
    pub trait AsyncRetryExt<T, E, Fut>: FnMut() -> Fut + Sized
    where
        Fut: Future<Output = Result<T, E>>,
    {
        /// Starts an owned async retry builder with default wait/predicate and
        /// an unconfigured stop strategy.
        ///
        /// `.stop(...)` must be configured before the builder can be awaited.
        ///
        /// ```compile_fail
        /// use core::future::ready;
        /// use tenacious::AsyncRetryExt;
        ///
        /// let _ = async {
        ///     let _ = (|| ready(Ok::<(), &str>(()))).retry_async().await;
        /// };
        /// ```
        fn retry_async(
            self,
        ) -> AsyncRetryBuilder<
            stop::NeedsStop,
            wait::WaitFixed,
            on::AnyError,
            (),
            (),
            (),
            (),
            Self,
            Fut,
            NoAsyncSleep,
            T,
            E,
            (),
            NeverCancel,
        >;
    }

    impl<T, E, Fut, F> AsyncRetryExt<T, E, Fut> for F
    where
        F: FnMut() -> Fut + Sized,
        Fut: Future<Output = Result<T, E>>,
    {
        fn retry_async(
            self,
        ) -> AsyncRetryBuilder<
            stop::NeedsStop,
            wait::WaitFixed,
            on::AnyError,
            (),
            (),
            (),
            (),
            Self,
            Fut,
            NoAsyncSleep,
            T,
            E,
            (),
            NeverCancel,
        > {
            let policy = RetryPolicy::new();
            let elapsed_tracker = ElapsedTracker::new(policy.meta.elapsed_clock);
            AsyncRetryBuilder {
                policy,
                hooks: ExecutionHooks::new(),
                op: self,
                sleeper: NoAsyncSleep,
                canceler: NeverCancel,
                last_result: None,
                phase: AsyncPhase::ReadyToStartAttempt,
                attempt: 1,
                total_wait: Duration::ZERO,
                collect_stats: false,
                final_stats: None,
                elapsed_tracker,
                started: false,
                _marker: PhantomData,
            }
        }
    }

    pin_project! {
        /// Owned async retry builder created from [`AsyncRetryExt::retry_async`].
        ///
        /// This future is single-use. Polling after completion is misuse:
        /// debug builds panic, and release builds return `Poll::Pending` unless
        /// `strict-futures` is enabled, in which case they also panic.
        pub struct AsyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut = (), C = NeverCancel>
        where
            F: FnMut() -> Fut,
            Fut: Future<Output = Result<T, E>>,
        {
            policy: RetryPolicy<S, W, P>,
            hooks: ExecutionHooks<BA, AA, BS, OX>,
            op: F,
            sleeper: SleepImpl,
            canceler: C,
            last_result: Option<Result<T, E>>,
            #[pin]
            phase: AsyncPhase<Fut, SleepFut>,
            attempt: u32,
            total_wait: Duration,
            collect_stats: bool,
            final_stats: Option<RetryStats>,
            elapsed_tracker: ElapsedTracker,
            started: bool,
            _marker: PhantomData<fn() -> (T, E)>,
        }
    }

    pin_project! {
        /// Owned async retry builder wrapper that returns statistics.
        pub struct AsyncRetryBuilderWithStats<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut = (), C = NeverCancel>
        where
            F: FnMut() -> Fut,
            Fut: Future<Output = Result<T, E>>,
        {
            #[pin]
            inner: AsyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>,
        }
    }

    type AsyncBuilderWithSleep<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, C> =
        AsyncRetryBuilder<
            S,
            W,
            P,
            BA,
            AA,
            BS,
            OX,
            F,
            Fut,
            SleepImpl,
            T,
            E,
            <SleepImpl as Sleeper>::Sleep,
            C,
        >;

    #[cfg(feature = "alloc")]
    type AsyncBuilderWithBeforeHook<
        S,
        W,
        P,
        BA,
        AA,
        BS,
        OX,
        F,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
        C,
        Hook,
    > = AsyncRetryBuilder<
        S,
        W,
        P,
        HookChain<BA, Hook>,
        AA,
        BS,
        OX,
        F,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
        C,
    >;

    #[cfg(feature = "alloc")]
    type AsyncBuilderWithAfterHook<
        S,
        W,
        P,
        BA,
        AA,
        BS,
        OX,
        F,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
        C,
        Hook,
    > = AsyncRetryBuilder<
        S,
        W,
        P,
        BA,
        HookChain<AA, Hook>,
        BS,
        OX,
        F,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
        C,
    >;

    #[cfg(feature = "alloc")]
    type AsyncBuilderWithBeforeSleepHook<
        S,
        W,
        P,
        BA,
        AA,
        BS,
        OX,
        F,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
        C,
        Hook,
    > = AsyncRetryBuilder<
        S,
        W,
        P,
        BA,
        AA,
        HookChain<BS, Hook>,
        OX,
        F,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
        C,
    >;

    #[cfg(feature = "alloc")]
    type AsyncBuilderWithOnExitHook<
        S,
        W,
        P,
        BA,
        AA,
        BS,
        OX,
        F,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
        C,
        Hook,
    > = AsyncRetryBuilder<
        S,
        W,
        P,
        BA,
        AA,
        BS,
        HookChain<OX, Hook>,
        F,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
        C,
    >;

    // Intentional: hook/type-state plumbing keeps full static guarantees and
    // zero-cost generics, which naturally yields long concrete return types.
    #[allow(clippy::type_complexity)]
    impl<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
        AsyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        fn map_hooks<NewBA, NewAA, NewBS, NewOX>(
            self,
            map: impl FnOnce(
                ExecutionHooks<BA, AA, BS, OX>,
            ) -> ExecutionHooks<NewBA, NewAA, NewBS, NewOX>,
        ) -> AsyncRetryBuilder<
            S,
            W,
            P,
            NewBA,
            NewAA,
            NewBS,
            NewOX,
            F,
            Fut,
            SleepImpl,
            T,
            E,
            SleepFut,
            C,
        > {
            let Self {
                policy,
                hooks,
                op,
                sleeper,
                canceler,
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker,
                started,
                ..
            } = self;
            AsyncRetryBuilder {
                policy,
                hooks: map(hooks),
                op,
                sleeper,
                canceler,
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker,
                started,
                _marker: PhantomData,
            }
        }

        /// Replaces the stop strategy.
        #[must_use]
        pub fn stop<NewStop>(
            self,
            stop: NewStop,
        ) -> AsyncRetryBuilder<NewStop, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
        {
            let Self {
                policy,
                hooks,
                op,
                sleeper,
                canceler,
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker: _,
                started,
                _marker: _,
            } = self;
            let policy = policy.stop(stop);
            let elapsed_tracker = ElapsedTracker::new(policy.meta.elapsed_clock);
            AsyncRetryBuilder {
                policy,
                hooks,
                op,
                sleeper,
                canceler,
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker,
                started,
                _marker: PhantomData,
            }
        }

        /// Replaces the wait strategy.
        #[must_use]
        pub fn wait<NewWait>(
            self,
            wait: NewWait,
        ) -> AsyncRetryBuilder<S, NewWait, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
        {
            let Self {
                policy,
                hooks,
                op,
                sleeper,
                canceler,
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker: _,
                started,
                _marker: _,
            } = self;
            let policy = policy.wait(wait);
            let elapsed_tracker = ElapsedTracker::new(policy.meta.elapsed_clock);
            AsyncRetryBuilder {
                policy,
                hooks,
                op,
                sleeper,
                canceler,
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker,
                started,
                _marker: PhantomData,
            }
        }

        /// Replaces the retry predicate.
        #[must_use]
        pub fn when<NewPredicate>(
            self,
            predicate: NewPredicate,
        ) -> AsyncRetryBuilder<
            S,
            W,
            NewPredicate,
            BA,
            AA,
            BS,
            OX,
            F,
            Fut,
            SleepImpl,
            T,
            E,
            SleepFut,
            C,
        > {
            let Self {
                policy,
                hooks,
                op,
                sleeper,
                canceler,
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker: _,
                started,
                _marker: _,
            } = self;
            let policy = policy.when(predicate);
            let elapsed_tracker = ElapsedTracker::new(policy.meta.elapsed_clock);
            AsyncRetryBuilder {
                policy,
                hooks,
                op,
                sleeper,
                canceler,
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker,
                started,
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
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                started,
                ..
            } = self;
            let policy = policy.elapsed_clock(clock);
            let elapsed_tracker = ElapsedTracker::new(policy.meta.elapsed_clock);
            AsyncRetryBuilder {
                policy,
                hooks,
                op,
                sleeper,
                canceler,
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker,
                started,
                _marker: PhantomData,
            }
        }

        /// Wraps this async retry builder with statistics collection.
        #[must_use]
        pub fn with_stats(
            self,
        ) -> AsyncRetryBuilderWithStats<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
        {
            let mut inner = self;
            inner.collect_stats = true;
            AsyncRetryBuilderWithStats { inner }
        }
    }

    impl<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, C>
        AsyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, (), C>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        /// Sets the async sleep implementation.
        #[must_use]
        pub fn sleep<NewSleep>(
            self,
            sleeper: NewSleep,
        ) -> AsyncBuilderWithSleep<S, W, P, BA, AA, BS, OX, F, Fut, NewSleep, T, E, C>
        where
            NewSleep: Sleeper,
        {
            let Self {
                policy,
                hooks,
                op,
                canceler,
                last_result,
                phase,
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker,
                started,
                ..
            } = self;
            AsyncRetryBuilder {
                policy,
                hooks,
                op,
                sleeper,
                canceler,
                last_result,
                phase: match phase {
                    AsyncPhase::ReadyToStartAttempt => AsyncPhase::ReadyToStartAttempt,
                    AsyncPhase::PollingOperation { op_future } => {
                        AsyncPhase::PollingOperation { op_future }
                    }
                    AsyncPhase::Sleeping { .. } => {
                        unreachable!("NoAsyncSleep cannot create sleeping futures")
                    }
                    AsyncPhase::Finished => AsyncPhase::Finished,
                },
                attempt,
                total_wait,
                collect_stats,
                final_stats,
                elapsed_tracker,
                started,
                _marker: PhantomData,
            }
        }
    }

    impl<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
        AsyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, NeverCancel>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        /// Attaches a canceler that is checked before each attempt and after each sleep.
        #[must_use]
        pub fn cancel_on<NewC: Canceler>(
            self,
            canceler: NewC,
        ) -> AsyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, NewC>
        {
            AsyncRetryBuilder {
                policy: self.policy,
                hooks: self.hooks,
                op: self.op,
                sleeper: self.sleeper,
                canceler,
                last_result: self.last_result,
                phase: self.phase,
                attempt: self.attempt,
                total_wait: self.total_wait,
                collect_stats: self.collect_stats,
                final_stats: self.final_stats,
                elapsed_tracker: self.elapsed_tracker,
                started: self.started,
                _marker: PhantomData,
            }
        }
    }

    #[cfg(feature = "alloc")]
    // Intentional: hook chaining preserves type-state and avoids runtime
    // indirection; signatures are long but mechanically structured.
    #[allow(clippy::type_complexity)]
    impl<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
        AsyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        /// Appends a before-attempt hook.
        #[must_use]
        pub fn before_attempt<Hook>(
            self,
            hook: Hook,
        ) -> AsyncBuilderWithBeforeHook<
            S,
            W,
            P,
            BA,
            AA,
            BS,
            OX,
            F,
            Fut,
            SleepImpl,
            T,
            E,
            SleepFut,
            C,
            Hook,
        >
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
        ) -> AsyncBuilderWithAfterHook<
            S,
            W,
            P,
            BA,
            AA,
            BS,
            OX,
            F,
            Fut,
            SleepImpl,
            T,
            E,
            SleepFut,
            C,
            Hook,
        >
        where
            Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
        {
            self.map_hooks(|hooks| hooks.chain_after_attempt(hook))
        }

        /// Appends a before-sleep hook.
        #[must_use]
        pub fn before_sleep<Hook>(
            self,
            hook: Hook,
        ) -> AsyncBuilderWithBeforeSleepHook<
            S,
            W,
            P,
            BA,
            AA,
            BS,
            OX,
            F,
            Fut,
            SleepImpl,
            T,
            E,
            SleepFut,
            C,
            Hook,
        >
        where
            Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
        {
            self.map_hooks(|hooks| hooks.chain_before_sleep(hook))
        }

        /// Appends an on-exit hook.
        #[must_use]
        pub fn on_exit<Hook>(
            self,
            hook: Hook,
        ) -> AsyncBuilderWithOnExitHook<
            S,
            W,
            P,
            BA,
            AA,
            BS,
            OX,
            F,
            Fut,
            SleepImpl,
            T,
            E,
            SleepFut,
            C,
            Hook,
        >
        where
            Hook: for<'a> FnMut(&ExitState<'a, T, E>),
        {
            self.map_hooks(|hooks| hooks.chain_on_exit(hook))
        }
    }

    impl<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C> Future
        for AsyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    where
        S: Stop,
        W: Wait,
        P: Predicate<T, E>,
        BA: BeforeAttemptHook,
        AA: AttemptHook<T, E>,
        BS: AttemptHook<T, E>,
        OX: ExitHook<T, E>,
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        SleepImpl: Sleeper<Sleep = SleepFut>,
        SleepFut: Future<Output = ()>,
        C: Canceler,
    {
        type Output = Result<T, RetryError<E, T>>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let mut this = self.project();

            if !*this.started {
                this.policy.stop.reset();
                this.policy.wait.reset();
                *this.started = true;
            }

            loop {
                match this.phase.as_mut().project() {
                    AsyncPhaseProj::ReadyToStartAttempt => {
                        if this.canceler.is_cancelled() {
                            let stats = if *this.collect_stats {
                                Some(RetryStats {
                                    attempts: *this.attempt - 1,
                                    total_elapsed: this.elapsed_tracker.elapsed(),
                                    total_wait: *this.total_wait,
                                    stop_reason: StopReason::Cancelled,
                                })
                            } else {
                                None
                            };
                            let exit_state = ExitState {
                                attempt: this.attempt.saturating_sub(1),
                                outcome: this.last_result.as_ref(),
                                elapsed: this.elapsed_tracker.elapsed(),
                                total_wait: *this.total_wait,
                                reason: StopReason::Cancelled,
                            };
                            this.hooks.on_exit.call(&exit_state);

                            *this.final_stats = stats;
                            this.phase.set(AsyncPhase::Finished);
                            return Poll::Ready(Err(RetryError::Cancelled {
                                last_result: this.last_result.take(),
                                attempts: *this.attempt - 1,
                                total_elapsed: this.elapsed_tracker.elapsed(),
                            }));
                        }

                        fire_before_attempt(
                            &mut *this.hooks,
                            *this.attempt,
                            this.elapsed_tracker.elapsed(),
                            *this.total_wait,
                        );
                        this.phase.set(AsyncPhase::PollingOperation {
                            op_future: (this.op)(),
                        });
                    }
                    AsyncPhaseProj::PollingOperation { op_future } => match poll_operation_future(
                        op_future,
                        cx,
                        &mut *this.policy,
                        &mut *this.hooks,
                        *this.attempt,
                        this.elapsed_tracker,
                        *this.total_wait,
                        *this.collect_stats,
                    ) {
                        AsyncOperationPoll::Pending => return Poll::Pending,
                        AsyncOperationPoll::Finished { result, stats } => {
                            *this.final_stats = stats;
                            this.phase.set(AsyncPhase::Finished);
                            return Poll::Ready(result);
                        }
                        AsyncOperationPoll::Sleep {
                            next_delay,
                            last_result,
                        } => {
                            *this.last_result = Some(last_result);
                            *this.total_wait = this.total_wait.saturating_add(next_delay);
                            this.phase.set(AsyncPhase::Sleeping {
                                sleep_future: this.sleeper.sleep(next_delay),
                            });
                        }
                    },
                    AsyncPhaseProj::Sleeping { sleep_future } => match sleep_future.poll(cx) {
                        Poll::Pending => {
                            if this.canceler.is_cancelled() {
                                let stats = if *this.collect_stats {
                                    Some(RetryStats {
                                        attempts: *this.attempt,
                                        total_elapsed: this.elapsed_tracker.elapsed(),
                                        total_wait: *this.total_wait,
                                        stop_reason: StopReason::Cancelled,
                                    })
                                } else {
                                    None
                                };

                                let exit_state = ExitState {
                                    attempt: *this.attempt,
                                    outcome: this.last_result.as_ref(),
                                    elapsed: this.elapsed_tracker.elapsed(),
                                    total_wait: *this.total_wait,
                                    reason: StopReason::Cancelled,
                                };
                                this.hooks.on_exit.call(&exit_state);
                                let last_result = this.last_result.take();

                                *this.final_stats = stats;
                                this.phase.set(AsyncPhase::Finished);
                                return Poll::Ready(Err(RetryError::Cancelled {
                                    last_result,
                                    attempts: *this.attempt,
                                    total_elapsed: this.elapsed_tracker.elapsed(),
                                }));
                            }
                            return Poll::Pending;
                        }
                        Poll::Ready(()) => {
                            if this.canceler.is_cancelled() {
                                let stats = if *this.collect_stats {
                                    Some(RetryStats {
                                        attempts: *this.attempt,
                                        total_elapsed: this.elapsed_tracker.elapsed(),
                                        total_wait: *this.total_wait,
                                        stop_reason: StopReason::Cancelled,
                                    })
                                } else {
                                    None
                                };
                                let exit_state = ExitState {
                                    attempt: *this.attempt,
                                    outcome: this.last_result.as_ref(),
                                    elapsed: this.elapsed_tracker.elapsed(),
                                    total_wait: *this.total_wait,
                                    reason: StopReason::Cancelled,
                                };
                                this.hooks.on_exit.call(&exit_state);

                                *this.final_stats = stats;
                                this.phase.set(AsyncPhase::Finished);
                                return Poll::Ready(Err(RetryError::Cancelled {
                                    last_result: this.last_result.take(),
                                    attempts: *this.attempt,
                                    total_elapsed: this.elapsed_tracker.elapsed(),
                                }));
                            }

                            *this.attempt = this.attempt.saturating_add(1);
                            this.phase.set(AsyncPhase::ReadyToStartAttempt);
                        }
                    },
                    AsyncPhaseProj::Finished => {
                        return poll_after_completion("AsyncRetryBuilder");
                    }
                }
            }
        }
    }

    impl<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C> Future
        for AsyncRetryBuilderWithStats<
            S,
            W,
            P,
            BA,
            AA,
            BS,
            OX,
            F,
            Fut,
            SleepImpl,
            T,
            E,
            SleepFut,
            C,
        >
    where
        S: Stop,
        W: Wait,
        P: Predicate<T, E>,
        BA: BeforeAttemptHook,
        AA: AttemptHook<T, E>,
        BS: AttemptHook<T, E>,
        OX: ExitHook<T, E>,
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        SleepImpl: Sleeper<Sleep = SleepFut>,
        SleepFut: Future<Output = ()>,
        C: Canceler,
    {
        type Output = (Result<T, RetryError<E, T>>, RetryStats);

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let mut this = self.project();
            match this.inner.as_mut().poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(result) => {
                    let stats = this
                        .inner
                        .as_mut()
                        .project()
                        .final_stats
                        .take()
                        .expect("async retry builder completed without final stats");
                    Poll::Ready((result, stats))
                }
            }
        }
    }
}

#[cfg(feature = "alloc")]
pub use async_ext::{AsyncRetryBuilder, AsyncRetryBuilderWithStats, AsyncRetryExt};
