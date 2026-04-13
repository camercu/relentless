use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::compat::Duration;
use pin_project_lite::pin_project;

#[cfg(feature = "alloc")]
use super::super::HookChain;
use super::super::execution::async_exec::{AsyncRetryCore, NoAsyncSleep};
use super::super::time::ElapsedTracker;
use super::super::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, RetryPolicy};
use crate::predicate::Predicate;
use crate::sleep::Sleeper;
use crate::state::{AttemptState, ExitState, RetryState};
use crate::{
    RetryError, RetryStats, predicate,
    stop::{self, Stop},
    wait::{self, Wait},
};

/// Extension trait to start async retries directly from a closure/function.
#[allow(clippy::type_complexity)]
pub trait AsyncRetryExt<T, E, Fut>: FnMut() -> Fut + Sized
where
    Fut: Future<Output = Result<T, E>>,
{
    /// Starts an owned async retry builder from [`RetryPolicy::default()`].
    ///
    /// `.sleep(...)` must be configured before the builder can be awaited.
    ///
    /// ```compile_fail
    /// use core::future::ready;
    /// use relentless::AsyncRetryExt;
    ///
    /// let _ = async {
    ///     let _ = (|| ready(Ok::<(), &str>(()))).retry_async().await;
    /// };
    /// ```
    fn retry_async(self) -> DefaultAsyncRetryBuilder<Self, Fut, T, E>;
}

/// Adapts a no-argument async closure to the [`AsyncRetryOp`] trait by
/// discarding the [`RetryState`] parameter the execution engine always passes.
#[doc(hidden)]
pub struct StatelessAsyncOp<F>(F);

impl<T, E, Fut: Future<Output = Result<T, E>>, F: FnMut() -> Fut>
    super::super::execution::common::AsyncRetryOp<T, E, Fut> for StatelessAsyncOp<F>
{
    fn call_op(&mut self, _state: RetryState) -> Fut {
        (self.0)()
    }
}

impl<T, E, Fut, F> AsyncRetryExt<T, E, Fut> for F
where
    F: FnMut() -> Fut + Sized,
    Fut: Future<Output = Result<T, E>>,
{
    fn retry_async(self) -> DefaultAsyncRetryBuilder<Self, Fut, T, E> {
        AsyncRetryBuilder {
            inner: AsyncRetryCore::new(
                RetryPolicy::default(),
                ExecutionHooks::new(),
                StatelessAsyncOp(self),
                NoAsyncSleep,
                ElapsedTracker::new(None),
            ),
        }
    }
}

/// Alias for the default owned async retry builder returned by
/// [`AsyncRetryExt::retry_async`].
///
/// This hides the default stop, wait, predicate, hook, sleeper, and sleep-future
/// state from user-facing type signatures.
pub type DefaultAsyncRetryBuilder<F, Fut, T, E> = AsyncRetryBuilder<
    stop::StopAfterAttempts,
    wait::WaitExponential,
    predicate::PredicateAnyError,
    (),
    (),
    (),
    StatelessAsyncOp<F>,
    Fut,
    NoAsyncSleep,
    T,
    E,
    (),
>;

/// Alias for the default owned async retry builder-with-stats returned by
/// calling `.with_stats()` on [`AsyncRetryExt::retry_async`].
pub type DefaultAsyncRetryBuilderWithStats<F, Fut, SleepImpl, T, E, SleepFut = ()> =
    AsyncRetryBuilderWithStats<
        stop::StopAfterAttempts,
        wait::WaitExponential,
        predicate::PredicateAnyError,
        (),
        (),
        (),
        StatelessAsyncOp<F>,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
    >;

#[doc(hidden)]
/// ```compile_fail
/// use core::future::ready;
/// use relentless::AsyncRetryExt;
///
/// let _ = async {
///     let _ = (|| ready(Ok::<(), &str>(())))
///         .retry_async()
///         .await;
/// };
/// ```
#[allow(dead_code)]
fn _async_retry_builder_requires_sleep_before_await() {}

pin_project! {
    /// Owned async retry builder created from [`AsyncRetryExt::retry_async`].
    ///
    /// This future is single-use. Polling after completion is misuse and
    /// always panics.
    ///
    /// # Examples
    ///
    /// ```
    /// use core::future::ready;
    /// use core::time::Duration;
    /// use relentless::AsyncRetryExt;
    ///
    /// let retry = (|| ready(Ok::<u32, &str>(1)))
    ///     .retry_async()
    ///     .sleep(|_dur: Duration| async {});
    ///
    /// let _ = retry;
    /// ```
    pub struct AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()>
    {
        #[pin]
        inner: AsyncRetryCore<RetryPolicy<S, W, P>, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>,
    }
}

impl<S, W, P, F, Fut, T, E> AsyncRetryBuilder<S, W, P, (), (), (), F, Fut, NoAsyncSleep, T, E, ()>
where
    F: FnMut(RetryState) -> Fut,
{
    pub(crate) fn from_policy(policy: RetryPolicy<S, W, P>, op: F) -> Self {
        AsyncRetryBuilder {
            inner: AsyncRetryCore::new(
                policy,
                ExecutionHooks::new(),
                op,
                NoAsyncSleep,
                ElapsedTracker::new(None),
            ),
        }
    }
}

impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> fmt::Debug
    for AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetryBuilder").finish_non_exhaustive()
    }
}

pin_project! {
    /// Owned async retry builder wrapper that returns statistics.
    ///
    /// # Examples
    ///
    /// ```
    /// use core::future::ready;
    /// use core::time::Duration;
    /// use relentless::AsyncRetryExt;
    ///
    /// let retry = (|| ready(Ok::<u32, &str>(1)))
    ///     .retry_async()
    ///     .sleep(|_dur: Duration| async {})
    ///     .with_stats();
    ///
    /// let _ = retry;
    /// ```
    pub struct AsyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = ()>
    {
        #[pin]
        inner: AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>,
    }
}

impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> fmt::Debug
    for AsyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetryBuilderWithStats")
            .finish_non_exhaustive()
    }
}

type AsyncBuilderWithSleep<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E> =
    AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, <SleepImpl as Sleeper>::Sleep>;

#[cfg(feature = "alloc")]
type AsyncBuilderWithBeforeHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, Hook> =
    AsyncRetryBuilder<S, W, P, HookChain<BA, Hook>, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>;

#[cfg(feature = "alloc")]
type AsyncBuilderWithAfterHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, Hook> =
    AsyncRetryBuilder<S, W, P, BA, HookChain<AA, Hook>, OX, F, Fut, SleepImpl, T, E, SleepFut>;

#[cfg(feature = "alloc")]
type AsyncBuilderWithOnExitHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, Hook> =
    AsyncRetryBuilder<S, W, P, BA, AA, HookChain<OX, Hook>, F, Fut, SleepImpl, T, E, SleepFut>;

// Intentional: hook/type-state plumbing keeps full static guarantees and
// zero-cost generics, which naturally yields long concrete return types.
#[allow(clippy::type_complexity)]
impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
{
    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> AsyncRetryBuilder<S, W, P, NewBA, NewAA, NewOX, F, Fut, SleepImpl, T, E, SleepFut> {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.map_hooks(map),
        }
    }

    /// Sets the stop condition for the retry policy.
    #[must_use]
    pub fn stop<NewStop>(
        self,
        stop: NewStop,
    ) -> AsyncRetryBuilder<NewStop, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.map_policy(|policy| policy.stop(stop)),
        }
    }

    /// Sets the wait strategy used between retry attempts.
    #[must_use]
    pub fn wait<NewWait>(
        self,
        wait: NewWait,
    ) -> AsyncRetryBuilder<S, NewWait, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.map_policy(|policy| policy.wait(wait)),
        }
    }

    /// Sets the predicate that decides whether a failed attempt should be retried.
    #[must_use]
    pub fn when<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> AsyncRetryBuilder<S, W, NewPredicate, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.map_policy(|policy| policy.when(predicate)),
        }
    }

    /// Sets a predicate that retries *until* `p.should_retry()` returns `true`.
    ///
    /// Wraps `p` in [`PredicateUntil`](crate::predicate::PredicateUntil).
    /// Natural for polling: `.until(ok(|s| s.is_ready()))`.
    #[must_use]
    pub fn until<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> AsyncRetryBuilder<
        S,
        W,
        predicate::PredicateUntil<NewPredicate>,
        BA,
        AA,
        OX,
        F,
        Fut,
        SleepImpl,
        T,
        E,
        SleepFut,
    > {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.map_policy(|policy| policy.until(predicate)),
        }
    }

    /// Configures a custom elapsed clock for elapsed-based stop conditions.
    #[must_use]
    pub fn elapsed_clock(self, clock: fn() -> Duration) -> Self {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.set_elapsed_clock(clock),
        }
    }

    /// Configures a custom elapsed clock from a boxed closure.
    ///
    /// This variant supports closures with captures for test clocks and
    /// runtime state. Requires the `alloc` feature.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn elapsed_clock_fn(self, clock: impl Fn() -> Duration + 'static) -> Self {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.set_elapsed_clock_fn(crate::compat::Box::new(clock)),
        }
    }

    /// Sets a wall-clock deadline for the entire retry execution.
    ///
    /// See [`SyncRetryBuilder::timeout`](super::sync_builder::SyncRetryBuilder::timeout)
    /// for full semantics.
    #[must_use]
    pub fn timeout(self, dur: Duration) -> Self {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.set_timeout(dur),
        }
    }

    /// Wraps this builder to also yield [`RetryStats`] on completion.
    ///
    /// Does not begin executing; the returned future must still be `.await`ed.
    #[must_use]
    pub fn with_stats(
        self,
    ) -> AsyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilderWithStats {
            inner: AsyncRetryBuilder {
                inner: inner.with_stats(),
            },
        }
    }
}

impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E>
    AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, ()>
{
    /// Sets the async sleep implementation used between retry attempts.
    #[must_use]
    #[allow(clippy::type_complexity)]
    pub fn sleep<NewSleep>(
        self,
        sleeper: NewSleep,
    ) -> AsyncBuilderWithSleep<S, W, P, BA, AA, OX, F, Fut, NewSleep, T, E>
    where
        NewSleep: Sleeper,
    {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.with_sleeper(sleeper, "NoAsyncSleep cannot create sleeping futures"),
        }
    }
}

impl_alloc_hook_chain! {
    impl[S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut]
    AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    =>
    before_attempt -> { AsyncBuilderWithBeforeHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, Hook> },
    after_attempt -> { AsyncBuilderWithAfterHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, Hook> },
    on_exit -> { AsyncBuilderWithOnExitHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, Hook> },
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetryBuilder<S, W, P, (), AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
{
    /// Sets the sole before-attempt hook (no-alloc mode).
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetryBuilder<S, W, P, Hook, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    where
        Hook: FnMut(&RetryState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetryBuilder<S, W, P, BA, (), OX, F, Fut, SleepImpl, T, E, SleepFut>
{
    /// Sets the sole after-attempt hook (no-alloc mode).
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetryBuilder<S, W, P, BA, Hook, OX, F, Fut, SleepImpl, T, E, SleepFut>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetryBuilder<S, W, P, BA, AA, (), F, Fut, SleepImpl, T, E, SleepFut>
{
    /// Sets the sole on-exit hook (no-alloc mode).
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetryBuilder<S, W, P, BA, AA, Hook, F, Fut, SleepImpl, T, E, SleepFut>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_on_exit(hook))
    }
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> Future
    for AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: super::super::execution::common::AsyncRetryOp<T, E, Fut>,
    Fut: Future<Output = Result<T, E>>,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()>,
{
    type Output = Result<T, RetryError<T, E>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project()
            .inner
            .poll::<S, W, P>(cx, "AsyncRetryBuilder")
    }
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut> Future
    for AsyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: super::super::execution::common::AsyncRetryOp<T, E, Fut>,
    Fut: Future<Output = Result<T, E>>,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()>,
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
                    .expect("async retry builder completed without final stats");
                Poll::Ready((result, stats))
            }
        }
    }
}
