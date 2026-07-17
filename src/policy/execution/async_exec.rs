use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use pin_project_lite::pin_project;

use crate::clock::{AsyncClock, SystemClock};
use crate::compat::Duration;
use crate::error::RetryError;
use crate::policy::HookChain;
use crate::policy::{
    AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, PolicyHandle, RetryPolicy,
};
use crate::predicate::Predicate;
use crate::state::{AttemptState, ExitState, RetryState};
use crate::stats::RetryStats;
use crate::stop::Stop;
use crate::wait::Wait;

use super::common::{AsyncEngine, AsyncRetryOp};

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
pub struct AsyncRetryExec<Policy, BA, AA, OX, F, C, T, E> {
    policy: Policy,
    hooks: ExecutionHooks<BA, AA, OX>,
    op: F,
    clock: C,
    timeout: Option<Duration>,
    _marker: PhantomData<fn() -> (T, E)>,
}

/// Async retry builder wrapper whose future also yields [`RetryStats`].
///
/// Created by calling `.with_stats()` on an [`AsyncRetryExec`].
pub struct AsyncRetryExecWithStats<Policy, BA, AA, OX, F, C, T, E> {
    inner: AsyncRetryExec<Policy, BA, AA, OX, F, C, T, E>,
}

/// Async retry builder created by [`RetryPolicy::retry_async`].
///
/// Configure hooks and set a clock with `.clock(...)`, then `.call()` and
/// `.await` the returned future.
///
/// # Examples
///
/// ```
/// use relentless::RetryPolicy;
/// use relentless::clock::VirtualClock;
///
/// let policy = RetryPolicy::new().stop(relentless::stop::attempts(3));
/// let clock = VirtualClock::new();
/// let retry = policy
///     .retry_async(|_| async { Ok::<u32, &str>(1) })
///     .before_attempt(|_state| {})
///     .clock(&clock);
/// let _ = retry;
/// ```
pub type AsyncRetry<'policy, S, W, P, BA, AA, OX, F, C, T, E> =
    AsyncRetryExec<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, C, T, E>;

/// Async retry builder-with-stats created by `.with_stats()` on
/// [`AsyncRetry`].
///
/// # Examples
///
/// ```
/// use relentless::RetryPolicy;
/// use relentless::clock::VirtualClock;
///
/// let policy = RetryPolicy::new().stop(relentless::stop::attempts(3));
/// let clock = VirtualClock::new();
/// let retry = policy
///     .retry_async(|_| async { Ok::<u32, &str>(1) })
///     .clock(&clock)
///     .with_stats();
/// let _ = retry;
/// ```
pub type AsyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, C, T, E> =
    AsyncRetryExecWithStats<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, C, T, E>;

impl<Policy, BA, AA, OX, F, C, T, E> fmt::Debug for AsyncRetryExec<Policy, BA, AA, OX, F, C, T, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetryExec").finish_non_exhaustive()
    }
}

impl<Policy, BA, AA, OX, F, C, T, E> fmt::Debug
    for AsyncRetryExecWithStats<Policy, BA, AA, OX, F, C, T, E>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetryExecWithStats")
            .finish_non_exhaustive()
    }
}

#[doc(hidden)]
/// ```compile_fail
/// use relentless::{RetryPolicy, stop};
///
/// let policy = RetryPolicy::new().stop(stop::attempts(1));
/// let _ = async {
///     let _ = policy.retry_async(|_| async { Err::<(), _>("fail") }).call().await;
/// };
/// ```
#[allow(dead_code)]
fn _async_call_requires_clock() {}

impl<Policy, BA, AA, OX, F, C, T, E> AsyncRetryExec<Policy, BA, AA, OX, F, C, T, E> {
    pub(crate) fn new(policy: Policy, hooks: ExecutionHooks<BA, AA, OX>, op: F, clock: C) -> Self {
        Self {
            policy,
            hooks,
            op,
            clock,
            timeout: None,
            _marker: PhantomData,
        }
    }

    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> AsyncRetryExec<Policy, NewBA, NewAA, NewOX, F, C, T, E> {
        AsyncRetryExec {
            policy: self.policy,
            hooks: map(self.hooks),
            op: self.op,
            clock: self.clock,
            timeout: self.timeout,
            _marker: PhantomData,
        }
    }

    pub(crate) fn map_policy<NewPolicy>(
        self,
        map: impl FnOnce(Policy) -> NewPolicy,
    ) -> AsyncRetryExec<NewPolicy, BA, AA, OX, F, C, T, E> {
        AsyncRetryExec {
            policy: map(self.policy),
            hooks: self.hooks,
            op: self.op,
            clock: self.clock,
            timeout: self.timeout,
            _marker: PhantomData,
        }
    }

    /// Sets a wall-clock budget for the entire retry execution.
    ///
    /// A **boundary check, not a preemptive timeout** — see
    /// [`SyncRetryBuilder::timeout`](crate::SyncRetryBuilder::timeout) for full
    /// semantics. Elapsed time is read from the configured
    /// [`clock`](Self::clock) — the same value that performs the waits, so the
    /// two cannot disagree.
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
    pub fn with_stats(self) -> AsyncRetryExecWithStats<Policy, BA, AA, OX, F, C, T, E> {
        AsyncRetryExecWithStats { inner: self }
    }
}

impl<Policy, BA, AA, OX, F, T, E> AsyncRetryExec<Policy, BA, AA, OX, F, SystemClock, T, E> {
    /// Sets the clock that supplies elapsed time and performs the wait between
    /// retry attempts.
    ///
    /// One value owns both seams ([`Clock::now`](crate::clock::Clock::now) and
    /// [`AsyncClock::wait_async`](crate::clock::AsyncClock::wait_async)), so
    /// `timeout`, [`stop::elapsed`](crate::stop::elapsed), and recorded waits
    /// always agree — including under paused-time runtimes, whose clock
    /// adapters (e.g. `TokioClock`) pair a coherent `now` with the runtime's
    /// timer. There is no async default: `.clock(...)` is required before
    /// `.call()` is available.
    #[must_use]
    pub fn clock<NewClock>(
        self,
        clock: NewClock,
    ) -> AsyncRetryExec<Policy, BA, AA, OX, F, NewClock, T, E>
    where
        NewClock: AsyncClock,
    {
        AsyncRetryExec {
            policy: self.policy,
            hooks: self.hooks,
            op: self.op,
            clock,
            timeout: self.timeout,
            _marker: PhantomData,
        }
    }
}

impl_hook_chain! {
    impl[Policy, BA, AA, OX, F, C, T, E]
    AsyncRetryExec<Policy, BA, AA, OX, F, C, T, E>
    =>
    before_attempt -> { AsyncRetryExec<Policy, HookChain<BA, Hook>, AA, OX, F, C, T, E> },
    after_attempt -> { AsyncRetryExec<Policy, BA, HookChain<AA, Hook>, OX, F, C, T, E> },
    on_exit -> { AsyncRetryExec<Policy, BA, AA, HookChain<OX, Hook>, F, C, T, E> },
}

#[allow(private_bounds)]
impl<Policy, BA, AA, OX, F, C, T, E> AsyncRetryExec<Policy, BA, AA, OX, F, C, T, E> {
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
        C: AsyncClock,
    {
        AsyncRetryCall {
            engine: self.into_engine(),
        }
    }

    fn into_engine<Fut>(self) -> AsyncEngine<Policy, BA, AA, OX, F, Fut, C, T, E, C::Wait>
    where
        C: AsyncClock,
    {
        AsyncEngine::new(self.policy, self.hooks, self.op, self.clock, self.timeout)
    }
}

#[allow(private_bounds)]
impl<Policy, BA, AA, OX, F, C, T, E> AsyncRetryExecWithStats<Policy, BA, AA, OX, F, C, T, E> {
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
        C: AsyncClock,
    {
        AsyncRetryCallWithStats {
            engine: self.inner.into_engine(),
        }
    }
}

pin_project! {
    /// Future returned by [`AsyncRetryExec::call`]. Surfaced as `impl Future`.
    struct AsyncRetryCall<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut> {
        #[pin]
        engine: AsyncEngine<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut>,
    }
}

#[allow(private_bounds)]
impl<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut> Future
    for AsyncRetryCall<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut>
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
    C: AsyncClock<Wait = WaitFut>,
    WaitFut: Future<Output = ()>,
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
    struct AsyncRetryCallWithStats<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut> {
        #[pin]
        engine: AsyncEngine<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut>,
    }
}

#[allow(private_bounds)]
impl<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut> Future
    for AsyncRetryCallWithStats<Policy, BA, AA, OX, F, Fut, C, T, E, WaitFut>
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
    C: AsyncClock<Wait = WaitFut>,
    WaitFut: Future<Output = ()>,
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
    /// use relentless::clock::VirtualClock;
    ///
    /// let policy = RetryPolicy::new().stop(stop::attempts(3));
    /// let clock = VirtualClock::new();
    /// let retry = policy
    ///     .retry_async(|_| async { Ok::<u32, &str>(1) })
    ///     .clock(&clock);
    /// let _ = retry;
    /// ```
    #[must_use]
    pub fn retry_async<T, E, F, Fut>(
        &self,
        op: F,
    ) -> AsyncRetry<'_, S, W, P, (), (), (), F, SystemClock, T, E>
    where
        F: FnMut(RetryState) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        AsyncRetryExec::new(self, ExecutionHooks::new(), op, SystemClock)
    }
}
