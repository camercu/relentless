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

use super::common::{AsyncEngine, AsyncRetryOp};
use crate::policy::time::ElapsedTracker;

/// Marker for the absence of an explicit async sleep implementation.
///
/// Users must call `.sleep(sleeper)` on [`AsyncRetry`] to provide a concrete
/// [`Sleeper`] before `.call()` becomes available.
#[doc(hidden)]
pub struct NoAsyncSleep;

/// Async retry builder, generic over how the policy is stored.
///
/// A single builder drives both entry points:
/// - [`AsyncRetry`] borrows a policy (`Policy = &RetryPolicy<S, W, P>`),
///   created by [`RetryPolicy::retry_async`].
/// - [`AsyncRetryBuilder`](crate::AsyncRetryBuilder) owns a policy
///   (`Policy = RetryPolicy<S, W, P>`), created by
///   [`crate::AsyncRetryExt::retry_async`].
///
/// Methods that only make sense when the policy is owned (`stop`, `wait`,
/// `when`, `until`) are implemented separately on the owned alias.
///
/// This type only configures the retry; execution state lives in the
/// single-use future returned by [`call`](Self::call).
pub struct AsyncRetryExec<Policy, BA, AA, OX, F, SleepImpl, T, E> {
    policy: Policy,
    hooks: ExecutionHooks<BA, AA, OX>,
    op: F,
    sleeper: SleepImpl,
    elapsed_tracker: ElapsedTracker,
    timeout: Option<Duration>,
    _marker: PhantomData<fn() -> (T, E)>,
}

/// Async retry builder wrapper whose future also yields [`RetryStats`].
///
/// Created by calling `.with_stats()` on an [`AsyncRetryExec`].
pub struct AsyncRetryExecWithStats<Policy, BA, AA, OX, F, SleepImpl, T, E> {
    inner: AsyncRetryExec<Policy, BA, AA, OX, F, SleepImpl, T, E>,
}

/// Async retry builder created by [`RetryPolicy::retry_async`].
///
/// Configure hooks and set a sleeper with `.sleep(...)`, then `.call()` and
/// `.await` the returned future.
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
pub type AsyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepImpl, T, E> =
    AsyncRetryExec<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, SleepImpl, T, E>;

/// Async retry builder-with-stats created by `.with_stats()` on
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
pub type AsyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, SleepImpl, T, E> =
    AsyncRetryExecWithStats<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, SleepImpl, T, E>;

impl<Policy, BA, AA, OX, F, SleepImpl, T, E> fmt::Debug
    for AsyncRetryExec<Policy, BA, AA, OX, F, SleepImpl, T, E>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetryExec").finish_non_exhaustive()
    }
}

impl<Policy, BA, AA, OX, F, SleepImpl, T, E> fmt::Debug
    for AsyncRetryExecWithStats<Policy, BA, AA, OX, F, SleepImpl, T, E>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetryExecWithStats")
            .finish_non_exhaustive()
    }
}

impl<Policy, BA, AA, OX, F, SleepImpl, T, E>
    AsyncRetryExec<Policy, BA, AA, OX, F, SleepImpl, T, E>
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
            elapsed_tracker,
            timeout: None,
            _marker: PhantomData,
        }
    }

    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> AsyncRetryExec<Policy, NewBA, NewAA, NewOX, F, SleepImpl, T, E> {
        AsyncRetryExec {
            policy: self.policy,
            hooks: map(self.hooks),
            op: self.op,
            sleeper: self.sleeper,
            elapsed_tracker: self.elapsed_tracker,
            timeout: self.timeout,
            _marker: PhantomData,
        }
    }

    pub(crate) fn map_policy<NewPolicy>(
        self,
        map: impl FnOnce(Policy) -> NewPolicy,
    ) -> AsyncRetryExec<NewPolicy, BA, AA, OX, F, SleepImpl, T, E> {
        AsyncRetryExec {
            policy: map(self.policy),
            hooks: self.hooks,
            op: self.op,
            sleeper: self.sleeper,
            elapsed_tracker: self.elapsed_tracker,
            timeout: self.timeout,
            _marker: PhantomData,
        }
    }

    /// Configures a custom elapsed clock for elapsed-based stop conditions.
    ///
    /// The clock must return a monotonic "now" timestamp; the baseline is
    /// captured at the first poll of the future returned by `.call()`. A
    /// non-advancing clock pins elapsed time at zero, so an elapsed-only stop
    /// would never fire; pair it with `stop::attempts` to stay bounded.
    #[must_use]
    pub fn elapsed_clock(mut self, clock: fn() -> Duration) -> Self {
        self.elapsed_tracker = ElapsedTracker::new(Some(clock));
        self
    }

    /// Configures a custom elapsed clock from a boxed closure.
    ///
    /// This variant supports closures with captures for test clocks and
    /// runtime state. Requires the `alloc` feature. See
    /// [`elapsed_clock`](Self::elapsed_clock) for the clock contract.
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

    /// Wraps this builder so its future also yields [`RetryStats`].
    ///
    /// Does not begin executing; call `.call()` on the returned wrapper and
    /// `.await` the future it returns.
    #[must_use]
    pub fn with_stats(self) -> AsyncRetryExecWithStats<Policy, BA, AA, OX, F, SleepImpl, T, E> {
        AsyncRetryExecWithStats { inner: self }
    }
}

impl<Policy, BA, AA, OX, F, T, E> AsyncRetryExec<Policy, BA, AA, OX, F, NoAsyncSleep, T, E> {
    /// Sets the async sleep implementation used between retry attempts.
    #[must_use]
    pub fn sleep<NewSleep>(
        self,
        sleeper: NewSleep,
    ) -> AsyncRetryExec<Policy, BA, AA, OX, F, NewSleep, T, E>
    where
        NewSleep: Sleeper,
    {
        AsyncRetryExec {
            policy: self.policy,
            hooks: self.hooks,
            op: self.op,
            sleeper,
            elapsed_tracker: self.elapsed_tracker,
            timeout: self.timeout,
            _marker: PhantomData,
        }
    }
}

impl_hook_chain! {
    impl[Policy, BA, AA, OX, F, SleepImpl, T, E]
    AsyncRetryExec<Policy, BA, AA, OX, F, SleepImpl, T, E>
    =>
    before_attempt -> { AsyncRetryExec<Policy, HookChain<BA, Hook>, AA, OX, F, SleepImpl, T, E> },
    after_attempt -> { AsyncRetryExec<Policy, BA, HookChain<AA, Hook>, OX, F, SleepImpl, T, E> },
    on_exit -> { AsyncRetryExec<Policy, BA, AA, HookChain<OX, Hook>, F, SleepImpl, T, E> },
}

#[allow(private_bounds)]
impl<Policy, BA, AA, OX, F, SleepImpl, T, E>
    AsyncRetryExec<Policy, BA, AA, OX, F, SleepImpl, T, E>
{
    /// Runs the configured retry loop, returning a future that resolves to the
    /// final result.
    ///
    /// This is the async terminator, mirroring the synchronous
    /// [`SyncRetry::call`](crate::SyncRetry::call). The returned future is
    /// single-use: polling it after completion is misuse and always panics.
    /// Dropping it cancels the loop at the next `.await` point (cancel-safe);
    /// `on_exit` does not fire on drop.
    pub fn call<Fut>(self) -> impl Future<Output = Result<T, RetryError<T, E>>>
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
        SleepImpl: Sleeper,
    {
        AsyncRetryCall {
            engine: self.into_engine(),
        }
    }

    fn into_engine<Fut>(
        self,
    ) -> AsyncEngine<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepImpl::Sleep>
    where
        SleepImpl: Sleeper,
    {
        AsyncEngine::new(
            self.policy,
            self.hooks,
            self.op,
            self.sleeper,
            self.elapsed_tracker,
            self.timeout,
        )
    }
}

#[allow(private_bounds)]
impl<Policy, BA, AA, OX, F, SleepImpl, T, E>
    AsyncRetryExecWithStats<Policy, BA, AA, OX, F, SleepImpl, T, E>
{
    /// Runs the configured retry loop, returning a future that resolves to the
    /// result paired with [`RetryStats`].
    ///
    /// The returned future is single-use: polling it after completion is
    /// misuse and always panics.
    pub fn call<Fut>(self) -> impl Future<Output = (Result<T, RetryError<T, E>>, RetryStats)>
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
        SleepImpl: Sleeper,
    {
        AsyncRetryCallWithStats {
            engine: self.inner.into_engine(),
        }
    }
}

pin_project! {
    /// Future returned by [`AsyncRetryExec::call`]. Surfaced as `impl Future`.
    struct AsyncRetryCall<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        #[pin]
        engine: AsyncEngine<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>,
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
        self.project()
            .engine
            .poll_step::<false>(cx)
            .map(|(result, _stats)| result)
    }
}

pin_project! {
    /// Future returned by [`AsyncRetryExecWithStats::call`]. Surfaced as
    /// `impl Future`.
    struct AsyncRetryCallWithStats<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        #[pin]
        engine: AsyncEngine<Policy, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>,
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
        self.project()
            .engine
            .poll_step::<true>(cx)
            .map(|(result, stats)| {
                let stats = stats.expect("async retry completed without stats");
                (result, stats)
            })
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
    ) -> AsyncRetry<'_, S, W, P, (), (), (), F, NoAsyncSleep, T, E>
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
