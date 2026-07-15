use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use pin_project_lite::pin_project;

use crate::compat::Duration;
use crate::error::RetryError;
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

use super::common::{AsyncPhase, AsyncRetryOp, poll_async_loop, remap_no_sleep_phase};
use crate::policy::time::ElapsedTracker;

/// Marker for the absence of an explicit async sleep implementation.
///
/// Users must call `.sleep(sleeper)` on [`AsyncRetry`] to provide a concrete
/// [`Sleeper`] before the future can be polled.
#[doc(hidden)]
pub struct NoAsyncSleep;

pin_project! {
    /// Async retry execution object, generic over how the policy is stored.
    ///
    /// A single engine drives both entry points:
    /// - [`AsyncRetry`] borrows a policy (`Policy = &RetryPolicy<S, W, P>`),
    ///   created by [`RetryPolicy::retry_async`].
    /// - [`AsyncRetryBuilder`](crate::AsyncRetryBuilder) owns a policy
    ///   (`Policy = RetryPolicy<S, W, P>`), created by
    ///   [`crate::AsyncRetryExt::retry_async`].
    ///
    /// Methods that only make sense when the policy is owned (`stop`, `wait`,
    /// `when`, `until`) are implemented separately on the owned alias.
    ///
    /// This future is single-use. Polling after completion is misuse and
    /// always panics.
    pub struct AsyncRetryExec<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()>
    {
        policy: Policy,
        hooks: ExecutionHooks<BA, AA, OX>,
        op: F,
        sleeper: SleepImpl,
        #[pin]
        phase: AsyncPhase<Fut, SleepFut>,
        attempt: u32,
        total_wait: Duration,
        previous_delay: Option<Duration>,
        collect_stats: bool,
        final_stats: Option<RetryStats>,
        elapsed_tracker: ElapsedTracker,
        timeout: Option<Duration>,
        _marker: PhantomData<fn() -> (T, E)>,
    }
}

pin_project! {
    /// Async retry execution wrapper that also yields [`RetryStats`].
    ///
    /// Created by calling `.with_stats()` on an [`AsyncRetryExec`].
    pub struct AsyncRetryExecWithStats<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()>
    {
        #[pin]
        inner: AsyncRetryExec<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>,
    }
}

/// Async retry execution object created by [`RetryPolicy::retry_async`].
///
/// Configure hooks and set a sleeper with `.sleep(...)`, then `.await` the
/// returned future.
///
/// # Examples
///
/// ```
/// use relentless::RetryPolicy;
/// use core::time::Duration;
///
/// let policy = RetryPolicy::new().stop(relentless::stop::attempts(3));
/// let retry = policy
///     .retry_async(|_| async { Ok::<u32, &str>(1) })
///     .before_attempt(|_state| {})
///     .sleep(|_dur: Duration| async {});
/// let _ = retry;
/// ```
pub type AsyncRetry<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()> =
    AsyncRetryExec<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>;

/// Async retry execution-with-stats object created by `.with_stats()` on
/// [`AsyncRetry`].
///
/// # Examples
///
/// ```
/// use relentless::RetryPolicy;
/// use core::time::Duration;
///
/// let policy = RetryPolicy::new().stop(relentless::stop::attempts(3));
/// let retry = policy
///     .retry_async(|_| async { Ok::<u32, &str>(1) })
///     .sleep(|_dur: Duration| async {})
///     .with_stats();
/// let _ = retry;
/// ```
pub type AsyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()> =
    AsyncRetryExecWithStats<
        &'policy RetryPolicy<S, W, P>,
        BA,
        AA,
        OX,
        F,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
    >;

impl<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> fmt::Debug
    for AsyncRetryExec<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetryExec").finish_non_exhaustive()
    }
}

impl<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> fmt::Debug
    for AsyncRetryExecWithStats<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetryExecWithStats")
            .finish_non_exhaustive()
    }
}

impl<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetryExec<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
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
            phase: AsyncPhase::ReadyToStartAttempt,
            attempt: 1,
            total_wait: Duration::ZERO,
            previous_delay: None,
            collect_stats: false,
            final_stats: None,
            elapsed_tracker,
            timeout: None,
            _marker: PhantomData,
        }
    }

    // Intentional: hook/type-state plumbing keeps full static guarantees and
    // zero-cost generics, which naturally yields long concrete return types.
    #[allow(clippy::type_complexity)]
    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> AsyncRetryExec<Policy, NewBA, NewAA, NewOX, F, Fut, SleepImpl, T, E, SleepFut> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            phase,
            attempt,
            total_wait,
            previous_delay,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            ..
        } = self;
        AsyncRetryExec {
            policy,
            hooks: map(hooks),
            op,
            sleeper,
            phase,
            attempt,
            total_wait,
            previous_delay,
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
    ) -> AsyncRetryExec<NewPolicy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            phase,
            attempt,
            total_wait,
            previous_delay,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            ..
        } = self;
        AsyncRetryExec {
            policy: map(policy),
            hooks,
            op,
            sleeper,
            phase,
            attempt,
            total_wait,
            previous_delay,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            _marker: PhantomData,
        }
    }

    /// Configures a custom elapsed clock for elapsed-based stop conditions.
    ///
    /// The clock must return a monotonic "now" timestamp; the baseline is
    /// captured at the first poll of the future returned by `.call()`. See
    /// [`SyncRetryExec::elapsed_clock`](crate::SyncRetryExec::elapsed_clock)
    /// for the full clock contract, including the unbounded-loop hazard of a
    /// non-advancing clock.
    #[must_use]
    pub fn elapsed_clock(mut self, clock: fn() -> Duration) -> Self {
        self.elapsed_tracker = ElapsedTracker::new(Some(clock));
        self
    }

    /// Configures a custom elapsed clock from a boxed closure.
    ///
    /// This variant supports closures with captures for test clocks and
    /// runtime state. Requires the `alloc` feature. See
    /// [`SyncRetryExec::elapsed_clock`](crate::SyncRetryExec::elapsed_clock)
    /// for the clock contract.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn elapsed_clock_fn(mut self, clock: impl Fn() -> Duration + 'static) -> Self {
        self.elapsed_tracker = ElapsedTracker::new_boxed(crate::compat::Box::new(clock));
        self
    }

    /// Sets a wall-clock budget for the entire retry execution.
    ///
    /// A **boundary check, not a preemptive timeout** — see
    /// [`SyncRetryBuilder::timeout`](crate::SyncRetryBuilder::timeout) for full
    /// semantics.
    ///
    /// For a hard wall-clock cancellation that can preempt in-flight work,
    /// wrap the future returned by `.call()` in your runtime's timeout (e.g.
    /// `tokio::time::timeout`); see the `async-cancel` example.
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    /// Wraps this future to also yield [`RetryStats`] on completion.
    ///
    /// Does not begin executing; the returned future must still be `.await`ed.
    #[must_use]
    pub fn with_stats(
        mut self,
    ) -> AsyncRetryExecWithStats<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        self.collect_stats = true;
        AsyncRetryExecWithStats { inner: self }
    }

    pub(crate) fn take_final_stats(self: Pin<&mut Self>) -> Option<RetryStats> {
        self.project().final_stats.take()
    }
}

impl<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E>
    AsyncRetryExec<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, ()>
{
    /// Sets the async sleep implementation used between retry attempts.
    #[must_use]
    #[allow(clippy::type_complexity)]
    pub fn sleep<NewSleep>(
        self,
        sleeper: NewSleep,
    ) -> AsyncRetryExec<Policy, BA, AA, OX, F, Fut, NewSleep, T, E, NewSleep::Sleep>
    where
        NewSleep: Sleeper,
    {
        let Self {
            policy,
            hooks,
            op,
            phase,
            attempt,
            total_wait,
            previous_delay,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            ..
        } = self;
        AsyncRetryExec {
            policy,
            hooks,
            op,
            sleeper,
            phase: remap_no_sleep_phase(phase, "NoAsyncSleep cannot create sleeping futures"),
            attempt,
            total_wait,
            previous_delay,
            collect_stats,
            final_stats,
            elapsed_tracker,
            timeout,
            _marker: PhantomData,
        }
    }
}

impl_hook_chain! {
    impl[Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut]
    AsyncRetryExec<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    =>
    before_attempt -> { AsyncRetryExec<Policy, HookChain<BA, Hook>, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> },
    after_attempt -> { AsyncRetryExec<Policy, BA, HookChain<AA, Hook>, OX, F, Fut, SleepImpl, T, E, SleepFut> },
    on_exit -> { AsyncRetryExec<Policy, BA, AA, HookChain<OX, Hook>, F, Fut, SleepImpl, T, E, SleepFut> },
}

#[allow(private_bounds)]
impl<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetryExec<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    Policy: PolicyHandle,
    Policy::Stop: Stop,
    Policy::Wait: Wait,
    Policy::Predicate: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: AsyncRetryOp<T, E, Fut>,
    Fut: Future<Output = Result<T, E>>,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()>,
{
    pub(crate) fn poll_inner(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<T, RetryError<T, E>>> {
        let mut this = self.project();
        let policy = this.policy.policy_ref();
        poll_async_loop(
            cx,
            policy,
            &mut *this.hooks,
            &mut *this.op,
            &*this.sleeper,
            this.phase.as_mut(),
            &mut *this.attempt,
            &mut *this.total_wait,
            &mut *this.previous_delay,
            *this.collect_stats,
            &mut *this.final_stats,
            this.elapsed_tracker,
            *this.timeout,
            "AsyncRetryExec",
        )
    }

    /// Runs the configured retry loop, returning a future that resolves to the
    /// final result.
    ///
    /// This is the async terminator, mirroring the synchronous
    /// [`SyncRetry::call`](crate::SyncRetry::call). Dropping the returned future
    /// cancels the loop at the next `.await` point (cancel-safe); `on_exit` does
    /// not fire on drop.
    pub fn call(self) -> impl Future<Output = Result<T, RetryError<T, E>>> {
        AsyncRetryCall { exec: self }
    }
}

#[allow(private_bounds)]
impl<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetryExecWithStats<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    Policy: PolicyHandle,
    Policy::Stop: Stop,
    Policy::Wait: Wait,
    Policy::Predicate: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: AsyncRetryOp<T, E, Fut>,
    Fut: Future<Output = Result<T, E>>,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()>,
{
    #[allow(clippy::type_complexity)]
    pub(crate) fn poll_inner(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<(Result<T, RetryError<T, E>>, RetryStats)> {
        let mut this = self.project();
        match this.inner.as_mut().poll_inner(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                let stats = this
                    .inner
                    .as_mut()
                    .take_final_stats()
                    .expect("async retry completed without final stats");
                Poll::Ready((result, stats))
            }
        }
    }

    /// Runs the configured retry loop, returning a future that resolves to the
    /// result paired with [`RetryStats`].
    pub fn call(self) -> impl Future<Output = (Result<T, RetryError<T, E>>, RetryStats)> {
        AsyncRetryCallWithStats { exec: self }
    }
}

pin_project! {
    /// Future returned by [`AsyncRetryExec::call`]. Surfaced as `impl Future`.
    struct AsyncRetryCall<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()> {
        #[pin]
        exec: AsyncRetryExec<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>,
    }
}

#[allow(private_bounds)]
impl<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> Future
    for AsyncRetryCall<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    Policy: PolicyHandle,
    Policy::Stop: Stop,
    Policy::Wait: Wait,
    Policy::Predicate: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: AsyncRetryOp<T, E, Fut>,
    Fut: Future<Output = Result<T, E>>,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()>,
{
    type Output = Result<T, RetryError<T, E>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().exec.poll_inner(cx)
    }
}

pin_project! {
    /// Future returned by [`AsyncRetryExecWithStats::call`]. Surfaced as
    /// `impl Future`.
    struct AsyncRetryCallWithStats<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()> {
        #[pin]
        exec: AsyncRetryExecWithStats<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>,
    }
}

#[allow(private_bounds)]
impl<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> Future
    for AsyncRetryCallWithStats<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    Policy: PolicyHandle,
    Policy::Stop: Stop,
    Policy::Wait: Wait,
    Policy::Predicate: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: AsyncRetryOp<T, E, Fut>,
    Fut: Future<Output = Result<T, E>>,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()>,
{
    type Output = (Result<T, RetryError<T, E>>, RetryStats);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().exec.poll_inner(cx)
    }
}

impl<S, W, P> RetryPolicy<S, W, P>
where
    S: Stop,
    W: Wait,
{
    /// ```
    /// use relentless::{RetryPolicy, stop};
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
        AsyncRetryExec::new(
            self,
            ExecutionHooks::new(),
            op,
            NoAsyncSleep,
            ElapsedTracker::new(None),
        )
    }
}
