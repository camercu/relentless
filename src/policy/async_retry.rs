use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use pin_project_lite::pin_project;

use crate::error::RetryError;
use crate::sleep::Sleeper;
use crate::state::{AttemptState, BeforeAttemptState, ExitState, RetryState};
use crate::stats::RetryStats;

use super::common::{AttemptTransition, process_attempt_transition};
use super::time::ElapsedTracker;
use super::*;

/// Marker for the absence of an explicit async sleep implementation.
///
/// Users must call `.sleep(sleeper)` on [`AsyncRetry`] to provide a concrete
/// [`Sleeper`] before the future can be polled.
#[doc(hidden)]
pub struct NoAsyncSleep;

pin_project! {
    #[project = AsyncPhaseProj]
    enum AsyncPhase<Fut, SleepFut> {
        ReadyToStartAttempt,
        PollingOperation {
            #[pin]
            op_future: Fut,
        },
        Sleeping {
            #[pin]
            sleep_future: SleepFut,
        },
        Finished,
    }
}

pin_project! {
    /// Async retry execution object.
    ///
    /// Created by [`RetryPolicy::retry_async`]. Configure hooks and set a
    /// sleeper with `.sleep(...)`, then `.await` the returned future.
    ///
    /// `AsyncRetry` is a single-use future. Polling after completion is
    /// misuse: debug builds panic. Release builds return `Poll::Pending`
    /// unless the `strict-futures` feature is enabled, in which case they
    /// also panic.
    ///
    /// # Examples
    ///
    /// ```
    /// use tenacious::RetryPolicy;
    /// use core::time::Duration;
    ///
    /// let mut policy = RetryPolicy::new().stop(tenacious::stop::attempts(3));
    /// let retry = policy
    ///     .retry_async(|| async { Ok::<u32, &str>(1) })
    ///     .before_attempt(|_state| {})
    ///     .sleep(|_dur: Duration| async {});
    /// let _ = retry;
    /// ```
    pub struct AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut = ()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        policy: &'policy mut RetryPolicy<S, W, P>,
        hooks: ExecutionHooks<BA, AA, BS, OX>,
        op: F,
        sleeper: SleepImpl,
        #[pin]
        phase: AsyncPhase<Fut, SleepFut>,
        attempt: u32,
        total_wait: Duration,
        collect_stats: bool,
        final_stats: Option<RetryStats>,
        elapsed_tracker: ElapsedTracker,
        _marker: PhantomData<fn() -> (T, E)>,
    }
}

pin_project! {
    /// Async retry execution wrapper that returns statistics.
    ///
    /// Created by calling `.with_stats()` on [`AsyncRetry`].
    ///
    /// # Examples
    ///
    /// ```
    /// use tenacious::RetryPolicy;
    /// use core::time::Duration;
    ///
    /// let mut policy = RetryPolicy::new().stop(tenacious::stop::attempts(3));
    /// let retry = policy
    ///     .retry_async(|| async { Ok::<u32, &str>(1) })
    ///     .sleep(|_dur: Duration| async {})
    ///     .with_stats();
    /// let _ = retry;
    /// ```
    pub struct AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut = ()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        #[pin]
        inner: AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>,
    }
}

type AsyncRetryWithSleep<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E> = AsyncRetry<
    'policy,
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
>;

type AsyncRetryStats<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut> =
    AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>;

impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    // Intentional: this helper preserves type-state hook tracking and avoids
    // runtime indirection, which necessarily yields a long generic return type.
    #[allow(clippy::type_complexity)]
    fn map_hooks<NewBA, NewAA, NewBS, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, BS, OX>) -> ExecutionHooks<NewBA, NewAA, NewBS, NewOX>,
    ) -> AsyncRetry<'policy, S, W, P, NewBA, NewAA, NewBS, NewOX, F, Fut, SleepImpl, T, E, SleepFut> {
        let AsyncRetry {
            policy,
            hooks,
            op,
            sleeper,
            phase,
            attempt,
            total_wait,
            collect_stats,
            final_stats,
            elapsed_tracker,
            ..
        } = self;
        AsyncRetry {
            policy,
            hooks: map(hooks),
            op,
            sleeper,
            phase,
            attempt,
            total_wait,
            collect_stats,
            final_stats,
            elapsed_tracker,
            _marker: PhantomData,
        }
    }
}

#[cfg(feature = "alloc")]
type AsyncRetryWithBeforeHook<
    'policy,
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
    Hook,
> = AsyncRetry<
    'policy,
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
>;

#[cfg(feature = "alloc")]
type AsyncRetryWithAfterHook<
    'policy,
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
    Hook,
> = AsyncRetry<
    'policy,
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
>;

#[cfg(feature = "alloc")]
type AsyncRetryWithBeforeSleepHook<
    'policy,
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
    Hook,
> = AsyncRetry<
    'policy,
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
>;

#[cfg(feature = "alloc")]
type AsyncRetryWithOnExitHook<
    'policy,
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
    Hook,
> = AsyncRetry<
    'policy,
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
>;

impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, ()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the async sleep implementation.
    #[must_use]
    pub fn sleep<NewSleep>(
        self,
        sleeper: NewSleep,
    ) -> AsyncRetryWithSleep<'policy, S, W, P, BA, AA, BS, OX, F, Fut, NewSleep, T, E>
    where
        NewSleep: Sleeper,
    {
        AsyncRetry {
            policy: self.policy,
            hooks: self.hooks,
            op: self.op,
            sleeper,
            phase: match self.phase {
                AsyncPhase::ReadyToStartAttempt => AsyncPhase::ReadyToStartAttempt,
                AsyncPhase::PollingOperation { op_future } => {
                    AsyncPhase::PollingOperation { op_future }
                }
                AsyncPhase::Sleeping { .. } => {
                    unreachable!("NoAsyncSleep cannot create sleeping futures")
                }
                AsyncPhase::Finished => AsyncPhase::Finished,
            },
            attempt: self.attempt,
            total_wait: self.total_wait,
            collect_stats: self.collect_stats,
            final_stats: self.final_stats,
            elapsed_tracker: self.elapsed_tracker,
            _marker: PhantomData,
        }
    }
}

impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Wraps this async retry with statistics collection.
    #[must_use]
    // Intentional: the stats wrapper carries the full builder type-state.
    #[allow(clippy::type_complexity)]
    pub fn with_stats(
        self,
    ) -> AsyncRetryStats<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        let mut inner = self;
        inner.collect_stats = true;
        AsyncRetryWithStats { inner }
    }
}

#[cfg(feature = "alloc")]
// Intentional: hook chaining APIs preserve compile-time type-state for no-alloc
// and zero-cost execution; signatures are verbose but mechanically constrained.
#[allow(clippy::type_complexity)]
impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Appends a before-attempt hook.
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetryWithBeforeHook<
        'policy,
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
    ) -> AsyncRetryWithAfterHook<
        'policy,
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
    ) -> AsyncRetryWithBeforeSleepHook<
        'policy,
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
    ) -> AsyncRetryWithOnExitHook<
        'policy,
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
        Hook,
    >
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.chain_on_exit(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, (), AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the sole before-attempt hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry_async(|| async { Err::<(), _>("fail") })
    ///     .before_attempt(|_state| {})
    ///     .before_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetry<'policy, S, W, P, Hook, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, (), BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the sole after-attempt hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry_async(|| async { Err::<(), _>("fail") })
    ///     .after_attempt(|_state| {})
    ///     .after_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetry<'policy, S, W, P, BA, Hook, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, AA, (), OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the sole before-sleep hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry_async(|| async { Err::<(), _>("fail") })
    ///     .before_sleep(|_state| {})
    ///     .before_sleep(|_state| {});
    /// ```
    #[must_use]
    pub fn before_sleep<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetry<'policy, S, W, P, BA, AA, Hook, OX, F, Fut, SleepImpl, T, E, SleepFut>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_before_sleep(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, AA, BS, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, (), F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the sole on-exit hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry_async(|| async { Err::<(), _>("fail") })
    ///     .on_exit(|_state| {})
    ///     .on_exit(|_state| {});
    /// ```
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetry<'policy, S, W, P, BA, AA, BS, Hook, F, Fut, SleepImpl, T, E, SleepFut>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_on_exit(hook))
    }
}

impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut> Future
    for AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>> + 'policy,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()> + 'policy,
{
    type Output = Result<T, RetryError<E, T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            match this.phase.as_mut().project() {
                AsyncPhaseProj::ReadyToStartAttempt => {
                    let elapsed_before_attempt = this.elapsed_tracker.elapsed();
                    let before_state = BeforeAttemptState {
                        attempt: *this.attempt,
                        elapsed: elapsed_before_attempt,
                        total_wait: *this.total_wait,
                    };
                    this.hooks.before_attempt.call(&before_state);

                    let op_future = (this.op)();
                    this.phase.set(AsyncPhase::PollingOperation { op_future });
                }
                AsyncPhaseProj::PollingOperation { op_future } => match op_future.poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(outcome) => {
                        let elapsed_after_attempt = this.elapsed_tracker.elapsed();
                        let retry_state = RetryState {
                            attempt: *this.attempt,
                            elapsed: elapsed_after_attempt,
                            next_delay: Duration::ZERO,
                            total_wait: *this.total_wait,
                        };

                        match process_attempt_transition(
                            &mut **this.policy,
                            &mut *this.hooks,
                            outcome,
                            retry_state,
                            *this.collect_stats,
                            *this.total_wait,
                        ) {
                            AttemptTransition::Finished { result, stats } => {
                                *this.final_stats = stats;
                                this.phase.set(AsyncPhase::Finished);
                                return Poll::Ready(result);
                            }
                            AttemptTransition::Sleep { next_delay } => {
                                *this.total_wait = this.total_wait.saturating_add(next_delay);
                                let sleep_future = this.sleeper.sleep(next_delay);
                                this.phase.set(AsyncPhase::Sleeping { sleep_future });
                            }
                        }
                    }
                },
                AsyncPhaseProj::Sleeping { sleep_future } => match sleep_future.poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => {
                        *this.attempt = this.attempt.saturating_add(1);
                        this.phase.set(AsyncPhase::ReadyToStartAttempt);
                    }
                },
                AsyncPhaseProj::Finished => {
                    #[cfg(any(debug_assertions, feature = "strict-futures"))]
                    panic!("AsyncRetry polled after completion");

                    #[cfg(all(not(debug_assertions), not(feature = "strict-futures")))]
                    {
                        return Poll::Pending;
                    }
                }
            }
        }
    }
}

impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut> Future
    for AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>> + 'policy,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()> + 'policy,
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
                    .expect("async retry completed without final stats");
                Poll::Ready((result, stats))
            }
        }
    }
}

impl<S, W, P> RetryPolicy<S, W, P>
where
    S: Stop,
    W: Wait,
{
    /// Begins configuring async retry execution.
    #[must_use]
    pub fn retry_async<T, E, F, Fut>(
        &mut self,
        op: F,
    ) -> AsyncRetry<'_, S, W, P, (), (), (), (), F, Fut, NoAsyncSleep, T, E>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        self.stop.reset();
        self.wait.reset();
        let elapsed_tracker = ElapsedTracker::new(self.meta.elapsed_clock);
        AsyncRetry {
            policy: self,
            hooks: ExecutionHooks::new(),
            op,
            sleeper: NoAsyncSleep,
            phase: AsyncPhase::ReadyToStartAttempt,
            attempt: 1,
            total_wait: Duration::ZERO,
            collect_stats: false,
            final_stats: None,
            elapsed_tracker,
            _marker: PhantomData,
        }
    }
}
