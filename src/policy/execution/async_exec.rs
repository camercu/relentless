use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use pin_project_lite::pin_project;

use crate::compat::Duration;
use crate::error::RetryError;
#[cfg(feature = "alloc")]
use crate::policy::HookChain;
use crate::policy::{
    AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, PolicyHandle, RetryPolicy,
};
use crate::predicate::Predicate;
use crate::sleep::Sleeper;
use crate::state::{AttemptState, ExitState, RetryState};
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
    pub(crate) struct AsyncRetryCore<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()>
    where
        F: FnMut(RetryState) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        policy: Policy,
        hooks: ExecutionHooks<BA, AA, OX>,
        op: F,
        sleeper: SleepImpl,
        last_result: Option<Result<T, E>>,
        #[pin]
        phase: AsyncPhase<Fut, SleepFut>,
        attempt: u32,
        total_wait: Duration,
        collect_stats: bool,
        final_stats: Option<RetryStats>,
        elapsed_tracker: ElapsedTracker,
        timeout: Option<Duration>,
        _marker: PhantomData<fn() -> (T, E)>,
    }
}

impl<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetryCore<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    pub(crate) fn new(
        policy: Policy,
        hooks: ExecutionHooks<BA, AA, OX>,
        op: F,
        sleeper: SleepImpl,
        elapsed_tracker: ElapsedTracker,
    ) -> Self {
        Self {
            policy,
            hooks,
            op,
            sleeper,
            last_result: None,
            phase: AsyncPhase::ReadyToStartAttempt,
            attempt: 1,
            total_wait: Duration::ZERO,
            collect_stats: false,
            final_stats: None,
            elapsed_tracker,
            timeout: None,
            _marker: PhantomData,
        }
    }

    pub(crate) fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> AsyncRetryCore<Policy, NewBA, NewAA, NewOX, F, Fut, SleepImpl, T, E, SleepFut> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            last_result,
            phase,
            attempt,
            total_wait,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            ..
        } = self;
        AsyncRetryCore {
            policy,
            hooks: map(hooks),
            op,
            sleeper,
            last_result,
            phase,
            attempt,
            total_wait,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            _marker: PhantomData,
        }
    }

    pub(crate) fn map_policy<NewPolicy>(
        self,
        map: impl FnOnce(Policy) -> NewPolicy,
    ) -> AsyncRetryCore<NewPolicy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            last_result,
            phase,
            attempt,
            total_wait,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            ..
        } = self;
        AsyncRetryCore {
            policy: map(policy),
            hooks,
            op,
            sleeper,
            last_result,
            phase,
            attempt,
            total_wait,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            _marker: PhantomData,
        }
    }

    pub(crate) fn set_elapsed_clock(mut self, clock: fn() -> Duration) -> Self {
        self.elapsed_tracker = ElapsedTracker::new(Some(clock));
        self
    }

    #[cfg(feature = "alloc")]
    pub(crate) fn set_elapsed_clock_fn(
        mut self,
        clock: crate::compat::Box<dyn Fn() -> Duration>,
    ) -> Self {
        self.elapsed_tracker = ElapsedTracker::new_boxed(clock);
        self
    }

    pub(crate) fn set_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub(crate) fn with_stats(mut self) -> Self {
        self.collect_stats = true;
        self
    }

    pub(crate) fn poll<S, W, P>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        completed_type_name: &'static str,
    ) -> Poll<Result<T, RetryError<T, E>>>
    where
        Policy: PolicyHandle<S, W, P>,
        S: Stop,
        W: Wait,
        P: Predicate<T, E>,
        BA: BeforeAttemptHook,
        AA: AttemptHook<T, E>,
        OX: ExitHook<T, E>,
        SleepImpl: Sleeper<Sleep = SleepFut>,
        SleepFut: Future<Output = ()>,
    {
        let mut this = self.project();

        let policy = this.policy.policy_ref();
        poll_async_loop(
            cx,
            policy,
            &mut *this.hooks,
            &mut *this.op,
            &*this.sleeper,
            &mut *this.last_result,
            this.phase.as_mut(),
            &mut *this.attempt,
            &mut *this.total_wait,
            *this.collect_stats,
            &mut *this.final_stats,
            this.elapsed_tracker,
            *this.timeout,
            completed_type_name,
        )
    }

    pub(crate) fn take_final_stats(self: Pin<&mut Self>) -> Option<RetryStats> {
        self.project().final_stats.take()
    }
}

impl<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E>
    AsyncRetryCore<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, ()>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    #[allow(clippy::type_complexity)]
    pub(crate) fn with_sleeper<NewSleep>(
        self,
        sleeper: NewSleep,
        unreachable_message: &'static str,
    ) -> AsyncRetryCore<Policy, BA, AA, OX, F, Fut, NewSleep, T, E, NewSleep::Sleep>
    where
        NewSleep: Sleeper,
    {
        let Self {
            policy,
            hooks,
            op,
            last_result,
            phase,
            attempt,
            total_wait,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            ..
        } = self;
        AsyncRetryCore {
            policy,
            hooks,
            op,
            sleeper,
            last_result,
            phase: remap_no_sleep_phase(phase, unreachable_message),
            attempt,
            total_wait,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            _marker: PhantomData,
        }
    }
}

pin_project! {
    /// Async retry execution object.
    ///
    /// Created by [`RetryPolicy::retry_async`]. Configure hooks and set a
    /// sleeper with `.sleep(...)`, then `.await` the returned future.
    ///
    /// `AsyncRetry` is a single-use future. Polling after completion is
    /// misuse and always panics.
    ///
    /// # Examples
    ///
    /// ```
    /// use tenacious::RetryPolicy;
    /// use core::time::Duration;
    ///
    /// let policy = RetryPolicy::new().stop(tenacious::stop::attempts(3));
    /// let retry = policy
    ///     .retry_async(|_| async { Ok::<u32, &str>(1) })
    ///     .before_attempt(|_state| {})
    ///     .sleep(|_dur: Duration| async {});
    /// let _ = retry;
    /// ```
    pub struct AsyncRetry<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()>
    where
        F: FnMut(RetryState) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        #[pin]
        inner: AsyncRetryCore<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>,
    }
}

impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> fmt::Debug
    for AsyncRetry<'_, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetry").finish_non_exhaustive()
    }
}

#[cfg(feature = "alloc")]
#[doc(hidden)]
/// ```compile_fail
/// use tenacious::{RetryPolicy, stop};
///
/// let policy = RetryPolicy::new().stop(stop::attempts(1));
/// let _ = async {
///     let _ = policy.retry_async(|_| async { Ok::<(), &str>(()) }).await;
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
    /// let policy = RetryPolicy::new().stop(tenacious::stop::attempts(3));
    /// let retry = policy
    ///     .retry_async(|_| async { Ok::<u32, &str>(1) })
    ///     .sleep(|_dur: Duration| async {})
    ///     .with_stats();
    /// let _ = retry;
    /// ```
    pub struct AsyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()>
    where
        F: FnMut(RetryState) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        #[pin]
        inner: AsyncRetry<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>,
    }
}

impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> fmt::Debug
    for AsyncRetryWithStats<'_, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetryWithStats")
            .finish_non_exhaustive()
    }
}

type AsyncRetryWithSleep<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E> = AsyncRetry<
    'policy,
    S,
    W,
    P,
    BA,
    AA,
    OX,
    F,
    Fut,
    SleepImpl,
    T,
    E,
    <SleepImpl as Sleeper>::Sleep,
>;

type AsyncRetryStats<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> =
    AsyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>;

impl<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    // Intentional: this helper preserves type-state hook tracking and avoids
    // runtime indirection, which necessarily yields a long generic return type.
    #[allow(clippy::type_complexity)]
    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> AsyncRetry<'policy, S, W, P, NewBA, NewAA, NewOX, F, Fut, SleepImpl, T, E, SleepFut> {
        let AsyncRetry { inner } = self;
        AsyncRetry {
            inner: inner.map_hooks(map),
        }
    }

    /// Configures a custom elapsed clock for elapsed-based stop conditions.
    #[must_use]
    pub fn elapsed_clock(self, clock: fn() -> Duration) -> Self {
        let AsyncRetry { inner } = self;
        AsyncRetry {
            inner: inner.set_elapsed_clock(clock),
        }
    }

    /// Configures a custom elapsed clock from a boxed closure.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn elapsed_clock_fn(self, clock: impl Fn() -> Duration + 'static) -> Self {
        let AsyncRetry { inner } = self;
        AsyncRetry {
            inner: inner.set_elapsed_clock_fn(crate::compat::Box::new(clock)),
        }
    }

    /// Sets a wall-clock deadline for the entire retry execution.
    #[must_use]
    pub fn timeout(self, dur: Duration) -> Self {
        let AsyncRetry { inner } = self;
        AsyncRetry {
            inner: inner.set_timeout(dur),
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
    OX,
    F,
    Fut,
    SleepImpl,
    T,
    E,
    SleepFut,
    Hook,
> = AsyncRetry<'policy, S, W, P, HookChain<BA, Hook>, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>;

#[cfg(feature = "alloc")]
type AsyncRetryWithAfterHook<
    'policy,
    S,
    W,
    P,
    BA,
    AA,
    OX,
    F,
    Fut,
    SleepImpl,
    T,
    E,
    SleepFut,
    Hook,
> = AsyncRetry<'policy, S, W, P, BA, HookChain<AA, Hook>, OX, F, Fut, SleepImpl, T, E, SleepFut>;

#[cfg(feature = "alloc")]
type AsyncRetryWithOnExitHook<
    'policy,
    S,
    W,
    P,
    BA,
    AA,
    OX,
    F,
    Fut,
    SleepImpl,
    T,
    E,
    SleepFut,
    Hook,
> = AsyncRetry<'policy, S, W, P, BA, AA, HookChain<OX, Hook>, F, Fut, SleepImpl, T, E, SleepFut>;

impl<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E>
    AsyncRetry<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, ()>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the async sleep implementation.
    #[must_use]
    pub fn sleep<NewSleep>(
        self,
        sleeper: NewSleep,
    ) -> AsyncRetryWithSleep<'policy, S, W, P, BA, AA, OX, F, Fut, NewSleep, T, E>
    where
        NewSleep: Sleeper,
    {
        let AsyncRetry { inner } = self;
        AsyncRetry {
            inner: inner.with_sleeper(sleeper, "NoAsyncSleep cannot create sleeping futures"),
        }
    }
}

impl<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Wraps this async retry with statistics collection.
    #[must_use]
    // Intentional: the stats wrapper carries the full builder type-state.
    #[allow(clippy::type_complexity)]
    pub fn with_stats(
        self,
    ) -> AsyncRetryStats<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        let AsyncRetry { inner } = self;
        AsyncRetryWithStats {
            inner: AsyncRetry {
                inner: inner.with_stats(),
            },
        }
    }
}

impl_alloc_hook_chain! {
    impl['policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut]
    AsyncRetry<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    where { F: FnMut(RetryState) -> Fut, Fut: Future<Output = Result<T, E>> } =>
    before_attempt -> { AsyncRetryWithBeforeHook<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, Hook> },
    after_attempt -> { AsyncRetryWithAfterHook<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, Hook> },
    on_exit -> { AsyncRetryWithOnExitHook<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, Hook> },
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, (), AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the sole before-attempt hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry_async(|_| async { Err::<(), _>("fail") })
    ///     .before_attempt(|_state| {})
    ///     .before_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetry<'policy, S, W, P, Hook, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    where
        Hook: FnMut(&RetryState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, (), OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the sole after-attempt hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry_async(|_| async { Err::<(), _>("fail") })
    ///     .after_attempt(|_state| {})
    ///     .after_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetry<'policy, S, W, P, BA, Hook, OX, F, Fut, SleepImpl, T, E, SleepFut>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, AA, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, AA, (), F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the sole on-exit hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry_async(|_| async { Err::<(), _>("fail") })
    ///     .on_exit(|_state| {})
    ///     .on_exit(|_state| {});
    /// ```
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetry<'policy, S, W, P, BA, AA, Hook, F, Fut, SleepImpl, T, E, SleepFut>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_on_exit(hook))
    }
}

#[allow(private_bounds)]
impl<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> Future
    for AsyncRetry<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>> + 'policy,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()> + 'policy,
{
    type Output = Result<T, RetryError<T, E>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().inner.poll::<S, W, P>(cx, "AsyncRetry")
    }
}

#[allow(private_bounds)]
impl<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> Future
    for AsyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut(RetryState) -> Fut,
    Fut: Future<Output = Result<T, E>> + 'policy,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()> + 'policy,
{
    type Output = (Result<T, RetryError<T, E>>, RetryStats);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        match this.inner.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                let stats = this
                    .inner
                    .as_mut()
                    .project()
                    .inner
                    .take_final_stats()
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
    ///
    /// # Examples
    ///
    /// ```
    /// use tenacious::{RetryPolicy, stop};
    /// use core::time::Duration;
    ///
    /// let policy = RetryPolicy::new().stop(stop::attempts(3));
    /// let retry = policy
    ///     .retry_async(|_| async { Ok::<u32, &str>(1) })
    ///     .sleep(|_dur: Duration| async {});
    /// let _ = retry;
    /// ```
    #[must_use]
    pub fn retry_async<T, E, F, Fut>(
        &self,
        op: F,
    ) -> AsyncRetry<'_, S, W, P, (), (), (), F, Fut, NoAsyncSleep, T, E>
    where
        F: FnMut(RetryState) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        AsyncRetry {
            inner: AsyncRetryCore::new(
                self,
                ExecutionHooks::new(),
                op,
                NoAsyncSleep,
                ElapsedTracker::new(None),
            ),
        }
    }
}
