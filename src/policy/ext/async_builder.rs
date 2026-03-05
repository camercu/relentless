use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::compat::Duration;
use pin_project_lite::pin_project;

use super::super::execution::common::{AsyncPhase, poll_async_loop};
use super::super::time::ElapsedTracker;
use super::super::{
    AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, HookChain, RetryPolicy,
};
use crate::cancel::{Canceler, NeverCancel};
use crate::policy::execution::async_exec::NoAsyncSleep;
use crate::predicate::Predicate;
use crate::sleep::Sleeper;
use crate::state::{AttemptState, BeforeAttemptState, ExitState};
use crate::{
    RetryError, RetryStats, on,
    stop::{self, Stop},
    wait::{self, Wait},
};

/// Extension trait to start async retries directly from a closure/function.
#[allow(clippy::type_complexity)]
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
        C: Canceler,
    {
        policy: RetryPolicy<S, W, P>,
        hooks: ExecutionHooks<BA, AA, BS, OX>,
        op: F,
        sleeper: SleepImpl,
        canceler: C,
        last_result: Option<Result<T, E>>,
        #[pin]
        phase: AsyncPhase<Fut, SleepFut, C::Cancel>,
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
        C: Canceler,
    {
        #[pin]
        inner: AsyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>,
    }
}

type AsyncBuilderWithSleep<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, C> = AsyncRetryBuilder<
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
    C: Canceler,
{
    fn map_hooks<NewBA, NewAA, NewBS, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, BS, OX>) -> ExecutionHooks<NewBA, NewAA, NewBS, NewOX>,
    ) -> AsyncRetryBuilder<S, W, P, NewBA, NewAA, NewBS, NewOX, F, Fut, SleepImpl, T, E, SleepFut, C>
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
    ) -> AsyncRetryBuilder<S, W, NewPredicate, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
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
    C: Canceler,
{
    /// Sets the async sleep implementation.
    #[must_use]
    #[allow(clippy::type_complexity)]
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
    #[allow(clippy::type_complexity)]
    pub fn cancel_on<NewC: Canceler>(
        self,
        canceler: NewC,
    ) -> AsyncRetryBuilder<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, NewC> {
        AsyncRetryBuilder {
            policy: self.policy,
            hooks: self.hooks,
            op: self.op,
            sleeper: self.sleeper,
            canceler,
            last_result: self.last_result,
            phase: match self.phase {
                AsyncPhase::ReadyToStartAttempt => AsyncPhase::ReadyToStartAttempt,
                AsyncPhase::PollingOperation { op_future } => {
                    AsyncPhase::PollingOperation { op_future }
                }
                AsyncPhase::Sleeping { .. } => {
                    unreachable!("cancel_on cannot observe a sleeping phase before polling")
                }
                AsyncPhase::Finished => AsyncPhase::Finished,
            },
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
    C: Canceler,
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

        poll_async_loop(
            cx,
            &mut *this.policy,
            &mut *this.hooks,
            &mut *this.op,
            &*this.sleeper,
            &*this.canceler,
            &mut *this.last_result,
            this.phase.as_mut(),
            &mut *this.attempt,
            &mut *this.total_wait,
            *this.collect_stats,
            &mut *this.final_stats,
            this.elapsed_tracker,
            "AsyncRetryBuilder",
        )
    }
}

impl<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C> Future
    for AsyncRetryBuilderWithStats<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
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
