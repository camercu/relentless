use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use pin_project_lite::pin_project;

use crate::cancel::{Canceler, NeverCancel};
use crate::compat::Duration;
use crate::error::RetryError;
#[cfg(feature = "alloc")]
use crate::policy::HookChain;
use crate::policy::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, RetryPolicy};
use crate::predicate::Predicate;
use crate::sleep::Sleeper;
use crate::state::{BeforeAttemptState, ExitState};
use crate::stats::RetryStats;
use crate::stop::Stop;
use crate::wait::Wait;

use super::common::{AsyncPhase, poll_async_loop, remap_no_sleep_phase};
use crate::policy::time::ElapsedTracker;

/// Marker for the absence of an explicit async sleep implementation.
///
/// Users must call `.sleep(sleeper)` on [`AsyncRetry`] to provide a concrete
/// [`Sleeper`] before the future can be polled.
#[doc(hidden)]
pub struct NoAsyncSleep;

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
    pub struct AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut = (), C = NeverCancel>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        C: Canceler,
    {
        policy: &'policy mut RetryPolicy<S, W, P>,
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
        _marker: PhantomData<fn() -> (T, E)>,
    }
}

#[cfg(feature = "alloc")]
#[doc(hidden)]
/// ```compile_fail
/// use tenacious::{RetryPolicy, stop};
///
/// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
/// let _ = async {
///     let _ = policy.retry_async(|| async { Ok::<(), &str>(()) }).await;
/// };
/// ```
#[allow(dead_code)]
fn _async_retry_requires_sleep_before_await() {}

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
    pub struct AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut = (), C = NeverCancel>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        C: Canceler,
    {
        #[pin]
        inner: AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>,
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

type AsyncRetryStats<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C> =
    AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>;

impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
{
    // Intentional: this helper preserves type-state hook tracking and avoids
    // runtime indirection, which necessarily yields a long generic return type.
    #[allow(clippy::type_complexity)]
    fn map_hooks<NewBA, NewAA, NewBS, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, BS, OX>) -> ExecutionHooks<NewBA, NewAA, NewBS, NewOX>,
    ) -> AsyncRetry<
        'policy,
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
        let AsyncRetry {
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
            ..
        } = self;
        AsyncRetry {
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
    C,
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
    C,
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
    C,
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
    C,
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
    C,
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
    C,
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
    C,
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
    C,
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
            canceler: self.canceler,
            last_result: self.last_result,
            phase: remap_no_sleep_phase(self.phase, "NoAsyncSleep cannot create sleeping futures"),
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
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, NeverCancel>
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
    ) -> AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, NewC> {
        AsyncRetry {
            policy: self.policy,
            hooks: self.hooks,
            op: self.op,
            sleeper: self.sleeper,
            canceler,
            last_result: self.last_result,
            phase: remap_no_sleep_phase(
                self.phase,
                "cancel_on cannot observe a sleeping phase before polling",
            ),
            attempt: self.attempt,
            total_wait: self.total_wait,
            collect_stats: self.collect_stats,
            final_stats: self.final_stats,
            elapsed_tracker: self.elapsed_tracker,
            _marker: PhantomData,
        }
    }
}

impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
{
    /// Wraps this async retry with statistics collection.
    #[must_use]
    // Intentional: the stats wrapper carries the full builder type-state.
    #[allow(clippy::type_complexity)]
    pub fn with_stats(
        self,
    ) -> AsyncRetryStats<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    {
        let mut inner = self;
        inner.collect_stats = true;
        AsyncRetryWithStats { inner }
    }
}

#[cfg(feature = "alloc")]
// Intentional: hook chaining APIs preserve compile-time type-state for no-alloc
// and zero-cost execution; signatures are verbose but mechanically constrained.
#[allow(clippy::type_complexity)]
impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
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
        C,
        Hook,
    >
    where
        Hook: for<'a> FnMut(&crate::AttemptState<'a, T, E>),
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
        C,
        Hook,
    >
    where
        Hook: for<'a> FnMut(&crate::AttemptState<'a, T, E>),
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
        C,
        Hook,
    >
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.chain_on_exit(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetry<'policy, S, W, P, (), AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
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
    ) -> AsyncRetry<'policy, S, W, P, Hook, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetry<'policy, S, W, P, BA, (), BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
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
    ) -> AsyncRetry<'policy, S, W, P, BA, Hook, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    where
        Hook: for<'a> FnMut(&crate::AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetry<'policy, S, W, P, BA, AA, (), OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
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
    ) -> AsyncRetry<'policy, S, W, P, BA, AA, Hook, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    where
        Hook: for<'a> FnMut(&crate::AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_before_sleep(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, AA, BS, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, (), F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
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
    ) -> AsyncRetry<'policy, S, W, P, BA, AA, BS, Hook, F, Fut, SleepImpl, T, E, SleepFut, C>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_on_exit(hook))
    }
}

#[allow(private_bounds)]
impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C> Future
    for AsyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
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
    C: Canceler,
{
    type Output = Result<T, RetryError<E, T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        poll_async_loop(
            cx,
            &mut **this.policy,
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
            "AsyncRetry",
        )
    }
}

#[allow(private_bounds)]
impl<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C> Future
    for AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
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
            canceler: NeverCancel,
            last_result: None,
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
