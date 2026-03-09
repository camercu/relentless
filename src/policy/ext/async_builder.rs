use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::compat::Duration;
use pin_project_lite::pin_project;

use super::super::execution::async_exec::{AsyncRetryCore, NoAsyncSleep};
use super::super::time::ElapsedTracker;
use super::super::{
    AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, HookChain, RetryPolicy,
};
use crate::cancel::{Canceler, NeverCancel};
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
    /// Starts an owned async retry builder from [`RetryPolicy::default()`].
    ///
    /// `.sleep(...)` must be configured before the builder can be awaited.
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
        stop::StopAfterAttempts,
        wait::WaitExponential,
        on::AnyError,
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
        stop::StopAfterAttempts,
        wait::WaitExponential,
        on::AnyError,
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
        let policy = RetryPolicy::default();
        let elapsed_tracker = ElapsedTracker::new(policy.meta.elapsed_clock);
        AsyncRetryBuilder {
            inner: AsyncRetryCore::new(
                policy,
                ExecutionHooks::new(),
                self,
                NoAsyncSleep,
                NeverCancel,
                elapsed_tracker,
                true,
            ),
        }
    }
}

#[cfg(feature = "alloc")]
#[doc(hidden)]
/// ```compile_fail
/// use core::future::ready;
/// use tenacious::AsyncRetryExt;
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
    pub struct AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = (), C = NeverCancel>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        C: Canceler,
    {
        #[pin]
        inner: AsyncRetryCore<RetryPolicy<S, W, P>, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>,
    }
}

pin_project! {
    /// Owned async retry builder wrapper that returns statistics.
    pub struct AsyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut = (), C = NeverCancel>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        C: Canceler,
    {
        #[pin]
        inner: AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>,
    }
}

type AsyncBuilderWithSleep<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, C> = AsyncRetryBuilder<
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
    C,
>;

#[cfg(feature = "alloc")]
type AsyncBuilderWithBeforeHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C, Hook> =
    AsyncRetryBuilder<S, W, P, HookChain<BA, Hook>, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>;

#[cfg(feature = "alloc")]
type AsyncBuilderWithAfterHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C, Hook> =
    AsyncRetryBuilder<S, W, P, BA, HookChain<AA, Hook>, OX, F, Fut, SleepImpl, T, E, SleepFut, C>;

#[cfg(feature = "alloc")]
type AsyncBuilderWithOnExitHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C, Hook> =
    AsyncRetryBuilder<S, W, P, BA, AA, HookChain<OX, Hook>, F, Fut, SleepImpl, T, E, SleepFut, C>;

// Intentional: hook/type-state plumbing keeps full static guarantees and
// zero-cost generics, which naturally yields long concrete return types.
#[allow(clippy::type_complexity)]
impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
{
    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> AsyncRetryBuilder<S, W, P, NewBA, NewAA, NewOX, F, Fut, SleepImpl, T, E, SleepFut, C> {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.map_hooks(map),
        }
    }

    /// Replaces the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(
        self,
        stop: NewStop,
    ) -> AsyncRetryBuilder<NewStop, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C> {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.map_policy(|policy| policy.stop(stop)),
        }
    }

    /// Replaces the wait strategy.
    #[must_use]
    pub fn wait<NewWait>(
        self,
        wait: NewWait,
    ) -> AsyncRetryBuilder<S, NewWait, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C> {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.map_policy(|policy| policy.wait(wait)),
        }
    }

    /// Replaces the retry predicate.
    #[must_use]
    pub fn when<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> AsyncRetryBuilder<S, W, NewPredicate, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.map_policy(|policy| policy.when(predicate)),
        }
    }

    /// Configures a custom elapsed clock for elapsed-based stop conditions.
    #[must_use]
    pub fn elapsed_clock(self, clock: fn() -> Duration) -> Self {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.map_policy(|policy| policy.elapsed_clock(clock)),
        }
    }

    /// Wraps this async retry builder with statistics collection.
    #[must_use]
    pub fn with_stats(
        self,
    ) -> AsyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C> {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilderWithStats {
            inner: AsyncRetryBuilder {
                inner: inner.with_stats(),
            },
        }
    }
}

impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, C>
    AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, (), C>
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
    ) -> AsyncBuilderWithSleep<S, W, P, BA, AA, OX, F, Fut, NewSleep, T, E, C>
    where
        NewSleep: Sleeper,
    {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.with_sleeper(sleeper, "NoAsyncSleep cannot create sleeping futures"),
        }
    }
}

impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, NeverCancel>
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
    ) -> AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, NewC> {
        let AsyncRetryBuilder { inner } = self;
        AsyncRetryBuilder {
            inner: inner.with_canceler(
                canceler,
                "cancel_on cannot observe a sleeping phase before polling",
            ),
        }
    }
}

impl_alloc_hook_chain! {
    impl[S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C]
    AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    where { F: FnMut() -> Fut, Fut: Future<Output = Result<T, E>>, C: Canceler } =>
    before_attempt -> { AsyncBuilderWithBeforeHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C, Hook> },
    after_attempt -> { AsyncBuilderWithAfterHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C, Hook> },
    on_exit -> { AsyncBuilderWithOnExitHook<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C, Hook> },
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C> Future
    for AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()>,
    C: Canceler,
{
    type Output = Result<T, RetryError<E, T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project()
            .inner
            .poll::<S, W, P>(cx, "AsyncRetryBuilder")
    }
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C> Future
    for AsyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
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
                    .inner
                    .take_final_stats()
                    .expect("async retry builder completed without final stats");
                Poll::Ready((result, stats))
            }
        }
    }
}
