use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

#[cfg(feature = "alloc")]
use super::easy_shared::{
    ErasedAsyncSleep, ErasedCanceler, VecAttemptHooks, VecBeforeHooks, VecExitHooks,
    default_erased_policy, empty_erased_hooks,
};
use crate::compat::Duration;
use pin_project_lite::pin_project;

#[cfg(feature = "alloc")]
use super::super::HookChain;
use super::super::execution::async_exec::{AsyncRetryCore, NoAsyncSleep};
use super::super::time::ElapsedTracker;
use super::super::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, RetryPolicy};
use crate::cancel::{Canceler, NeverCancel};
#[cfg(feature = "alloc")]
use crate::compat::Box;
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
    fn retry_async(self) -> DefaultAsyncRetryBuilder<Self, Fut, T, E>;

    /// Starts an `alloc`-backed async retry builder with erased internal
    /// strategy types.
    ///
    /// Call `.sleep(...)` to select the runtime-specific sleeper and obtain an
    /// awaitable runner.
    #[cfg(feature = "alloc")]
    fn retry_async_easy(self) -> EasyAsyncRetryBuilder<'static, Self, Fut, T, E>
    where
        Self: 'static,
        Fut: 'static;
}

impl<T, E, Fut, F> AsyncRetryExt<T, E, Fut> for F
where
    F: FnMut() -> Fut + Sized,
    Fut: Future<Output = Result<T, E>>,
{
    fn retry_async(self) -> DefaultAsyncRetryBuilder<Self, Fut, T, E> {
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

    #[cfg(feature = "alloc")]
    fn retry_async_easy(self) -> EasyAsyncRetryBuilder<'static, Self, Fut, T, E>
    where
        Self: 'static,
        Fut: 'static,
    {
        EasyAsyncRetryBuilder {
            inner: AsyncRetryBuilder {
                inner: AsyncRetryCore::new(
                    default_erased_policy(),
                    empty_erased_hooks(),
                    self,
                    NoAsyncSleep,
                    ErasedCanceler::default(),
                    ElapsedTracker::new(None),
                    true,
                ),
            },
        }
    }
}

/// Alias for the default owned async retry builder returned by
/// [`AsyncRetryExt::retry_async`].
///
/// This hides the default stop, wait, predicate, hook, sleeper, sleep-future,
/// and canceler state from user-facing type signatures.
pub type DefaultAsyncRetryBuilder<F, Fut, T, E> = AsyncRetryBuilder<
    stop::StopAfterAttempts,
    wait::WaitExponential,
    on::AnyError,
    (),
    (),
    (),
    F,
    Fut,
    NoAsyncSleep,
    T,
    E,
    (),
    NeverCancel,
>;

/// Alias for the owned async retry builder returned by
/// [`RetryPolicy::retry_async_clone`].
///
/// This keeps the policy strategy types visible while hiding the initial hook,
/// sleeper, sleep-future, and canceler plumbing from user-facing type
/// signatures.
pub type PolicyAsyncRetryBuilder<S, W, P, F, Fut, T, E> =
    AsyncRetryBuilder<S, W, P, (), (), (), F, Fut, NoAsyncSleep, T, E, (), NeverCancel>;

/// Alias for the default owned async retry builder-with-stats returned by
/// calling `.with_stats()` on [`AsyncRetryExt::retry_async`].
pub type DefaultAsyncRetryBuilderWithStats<
    F,
    Fut,
    SleepImpl,
    T,
    E,
    SleepFut = (),
    C = NeverCancel,
> = AsyncRetryBuilderWithStats<
    stop::StopAfterAttempts,
    wait::WaitExponential,
    on::AnyError,
    (),
    (),
    (),
    F,
    Fut,
    SleepImpl,
    T,
    E,
    SleepFut,
    C,
>;

/// Alias for the owned async retry builder-with-stats returned by calling
/// `.with_stats()` on [`RetryPolicy::retry_async_clone`].
pub type PolicyAsyncRetryBuilderWithStats<
    S,
    W,
    P,
    F,
    Fut,
    SleepImpl,
    T,
    E,
    SleepFut = (),
    C = NeverCancel,
> = AsyncRetryBuilderWithStats<S, W, P, (), (), (), F, Fut, SleepImpl, T, E, SleepFut, C>;

#[cfg(feature = "alloc")]
type EasyAsyncInner<'a, F, Fut, T, E> = AsyncRetryBuilder<
    Box<dyn Stop + 'a>,
    Box<dyn Wait + 'a>,
    Box<dyn Predicate<T, E> + 'a>,
    VecBeforeHooks<'a>,
    VecAttemptHooks<'a, T, E>,
    VecExitHooks<'a, T, E>,
    F,
    Fut,
    NoAsyncSleep,
    T,
    E,
    (),
    ErasedCanceler<'a>,
>;

#[cfg(feature = "alloc")]
type EasyAsyncRunnerInner<'a, F, Fut, T, E> = AsyncRetryBuilder<
    Box<dyn Stop + 'a>,
    Box<dyn Wait + 'a>,
    Box<dyn Predicate<T, E> + 'a>,
    VecBeforeHooks<'a>,
    VecAttemptHooks<'a, T, E>,
    VecExitHooks<'a, T, E>,
    F,
    Fut,
    ErasedAsyncSleep<'a>,
    T,
    E,
    Pin<Box<dyn Future<Output = ()> + 'a>>,
    ErasedCanceler<'a>,
>;

/// `alloc`-backed async retry builder with erased internal strategy types.
#[cfg(feature = "alloc")]
pub struct EasyAsyncRetryBuilder<'a, F, Fut, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    inner: EasyAsyncInner<'a, F, Fut, T, E>,
}

#[cfg(feature = "alloc")]
impl<F, Fut, T, E> fmt::Debug for EasyAsyncRetryBuilder<'_, F, Fut, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EasyAsyncRetryBuilder")
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "alloc")]
pin_project! {
    /// Awaitable runner returned by [`EasyAsyncRetryBuilder::sleep`].
    pub struct EasyAsyncRetryRunner<'a, F, Fut, T, E>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        #[pin]
        inner: EasyAsyncRunnerInner<'a, F, Fut, T, E>,
    }
}

#[cfg(feature = "alloc")]
impl<F, Fut, T, E> fmt::Debug for EasyAsyncRetryRunner<'_, F, Fut, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EasyAsyncRetryRunner")
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "alloc")]
pub struct EasyAsyncRetryBuilderWithStats<'a, F, Fut, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    inner: EasyAsyncRetryBuilder<'a, F, Fut, T, E>,
}

#[cfg(feature = "alloc")]
impl<F, Fut, T, E> fmt::Debug for EasyAsyncRetryBuilderWithStats<'_, F, Fut, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EasyAsyncRetryBuilderWithStats")
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "alloc")]
pin_project! {
    /// Awaitable stats runner returned by [`EasyAsyncRetryBuilderWithStats::sleep`].
    pub struct EasyAsyncRetryRunnerWithStats<'a, F, Fut, T, E>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        #[pin]
        inner: AsyncRetryBuilderWithStats<
            Box<dyn Stop + 'a>,
            Box<dyn Wait + 'a>,
            Box<dyn Predicate<T, E> + 'a>,
            VecBeforeHooks<'a>,
            VecAttemptHooks<'a, T, E>,
            VecExitHooks<'a, T, E>,
            F,
            Fut,
            ErasedAsyncSleep<'a>,
            T,
            E,
            Pin<Box<dyn Future<Output = ()> + 'a>>,
            ErasedCanceler<'a>,
        >,
    }
}

#[cfg(feature = "alloc")]
impl<F, Fut, T, E> fmt::Debug for EasyAsyncRetryRunnerWithStats<'_, F, Fut, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EasyAsyncRetryRunnerWithStats")
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "alloc")]
impl<'a, F, Fut, T, E> Future for EasyAsyncRetryRunner<'a, F, Fut, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>> + 'a,
{
    type Output = Result<T, RetryError<E, T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}

#[cfg(feature = "alloc")]
impl<'a, F, Fut, T, E> Future for EasyAsyncRetryRunnerWithStats<'a, F, Fut, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>> + 'a,
{
    type Output = (Result<T, RetryError<E, T>>, RetryStats);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}

impl<S, W, P> RetryPolicy<S, W, P>
where
    S: Stop,
    W: Wait,
    Self: Clone,
{
    /// Starts an owned async retry builder by cloning this policy.
    ///
    /// This is the mutability-free counterpart to
    /// [`RetryPolicy::retry_async`]. It is useful when you keep a shared
    /// template policy and want each execution to own an independent copy of
    /// the stop, wait, and predicate state.
    ///
    /// Async execution still requires `.sleep(...)` before awaiting the
    /// builder.
    ///
    /// ```
    /// use core::future::ready;
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let template = RetryPolicy::new().stop(stop::attempts(1));
    /// let retry = template
    ///     .retry_async_clone(|| ready(Ok::<(), &str>(())))
    ///     .sleep(|_dur| ready(()));
    ///
    /// let _ = retry;
    /// ```
    #[must_use]
    #[allow(clippy::type_complexity)]
    pub fn retry_async_clone<T, E, F, Fut>(
        &self,
        op: F,
    ) -> PolicyAsyncRetryBuilder<S, W, P, F, Fut, T, E>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let policy = self.clone();
        let elapsed_tracker = ElapsedTracker::new(policy.meta.elapsed_clock);
        AsyncRetryBuilder {
            inner: AsyncRetryCore::new(
                policy,
                ExecutionHooks::new(),
                op,
                NoAsyncSleep,
                NeverCancel,
                elapsed_tracker,
                true,
            ),
        }
    }
}

#[cfg(feature = "alloc")]
impl<'a, F, Fut, T, E> EasyAsyncRetryBuilder<'a, F, Fut, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    #[must_use]
    pub fn stop<S>(self, stop: S) -> Self
    where
        S: Stop + 'a,
    {
        let AsyncRetryBuilder { inner } = self.inner;
        Self {
            inner: AsyncRetryBuilder {
                inner: inner.map_policy(|policy| policy.stop(Box::new(stop) as Box<dyn Stop + 'a>)),
            },
        }
    }

    #[must_use]
    pub fn wait<W>(self, wait: W) -> Self
    where
        W: Wait + 'a,
    {
        let AsyncRetryBuilder { inner } = self.inner;
        Self {
            inner: AsyncRetryBuilder {
                inner: inner.map_policy(|policy| policy.wait(Box::new(wait) as Box<dyn Wait + 'a>)),
            },
        }
    }

    #[must_use]
    pub fn when<P>(self, predicate: P) -> Self
    where
        P: Predicate<T, E> + 'a,
    {
        let AsyncRetryBuilder { inner } = self.inner;
        Self {
            inner: AsyncRetryBuilder {
                inner: inner.map_policy(|policy| {
                    policy.when(Box::new(predicate) as Box<dyn Predicate<T, E> + 'a>)
                }),
            },
        }
    }

    #[must_use]
    pub fn elapsed_clock(self, clock: fn() -> Duration) -> Self {
        Self {
            inner: self.inner.elapsed_clock(clock),
        }
    }

    #[must_use]
    pub fn before_attempt<Hook>(self, hook: Hook) -> Self
    where
        Hook: FnMut(&BeforeAttemptState) + 'a,
    {
        Self {
            inner: self.inner.map_hooks(|mut hooks| {
                hooks.before_attempt.push(hook);
                hooks
            }),
        }
    }

    #[must_use]
    pub fn after_attempt<Hook>(self, hook: Hook) -> Self
    where
        Hook: for<'state> FnMut(&AttemptState<'state, T, E>) + 'a,
    {
        Self {
            inner: self.inner.map_hooks(|mut hooks| {
                hooks.after_attempt.push(hook);
                hooks
            }),
        }
    }

    #[must_use]
    pub fn on_exit<Hook>(self, hook: Hook) -> Self
    where
        Hook: for<'state> FnMut(&ExitState<'state, T, E>) + 'a,
    {
        Self {
            inner: self.inner.map_hooks(|mut hooks| {
                hooks.on_exit.push(hook);
                hooks
            }),
        }
    }

    #[must_use]
    pub fn with_stats(self) -> EasyAsyncRetryBuilderWithStats<'a, F, Fut, T, E> {
        EasyAsyncRetryBuilderWithStats { inner: self }
    }

    #[must_use]
    pub fn sleep<SleepImpl>(self, sleeper: SleepImpl) -> EasyAsyncRetryRunner<'a, F, Fut, T, E>
    where
        SleepImpl: Sleeper + 'a,
        SleepImpl::Sleep: 'a,
    {
        let AsyncRetryBuilder { inner } = self.inner;
        EasyAsyncRetryRunner {
            inner: AsyncRetryBuilder {
                inner: inner.with_sleeper(
                    ErasedAsyncSleep::new(sleeper),
                    "easy async retry builder requires sleep before await",
                ),
            },
        }
    }
}

#[cfg(feature = "alloc")]
impl<'a, F, Fut, T, E> EasyAsyncRetryBuilderWithStats<'a, F, Fut, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    #[must_use]
    pub fn sleep<SleepImpl>(
        self,
        sleeper: SleepImpl,
    ) -> EasyAsyncRetryRunnerWithStats<'a, F, Fut, T, E>
    where
        SleepImpl: Sleeper + 'a,
        SleepImpl::Sleep: 'a,
    {
        EasyAsyncRetryRunnerWithStats {
            inner: self.inner.sleep(sleeper).inner.with_stats(),
        }
    }
}

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
    ///
    /// # Examples
    ///
    /// ```
    /// use core::future::ready;
    /// use core::time::Duration;
    /// use tenacious::AsyncRetryExt;
    ///
    /// let retry = (|| ready(Ok::<u32, &str>(1)))
    ///     .retry_async()
    ///     .sleep(|_dur: Duration| async {});
    ///
    /// let _ = retry;
    /// ```
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

impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C> fmt::Debug
    for AsyncRetryBuilder<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
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
    /// use tenacious::AsyncRetryExt;
    ///
    /// let retry = (|| ready(Ok::<u32, &str>(1)))
    ///     .retry_async()
    ///     .sleep(|_dur: Duration| async {})
    ///     .with_stats();
    ///
    /// let _ = retry;
    /// ```
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

impl<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C> fmt::Debug
    for AsyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncRetryBuilderWithStats")
            .finish_non_exhaustive()
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

#[cfg(not(feature = "alloc"))]
impl<S, W, P, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetryBuilder<S, W, P, (), AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
{
    /// Sets the sole before-attempt hook (no-alloc mode).
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetryBuilder<S, W, P, Hook, AA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetryBuilder<S, W, P, BA, (), OX, F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
{
    /// Sets the sole after-attempt hook (no-alloc mode).
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetryBuilder<S, W, P, BA, Hook, OX, F, Fut, SleepImpl, T, E, SleepFut, C>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, F, Fut, SleepImpl, T, E, SleepFut, C>
    AsyncRetryBuilder<S, W, P, BA, AA, (), F, Fut, SleepImpl, T, E, SleepFut, C>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Canceler,
{
    /// Sets the sole on-exit hook (no-alloc mode).
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> AsyncRetryBuilder<S, W, P, BA, AA, Hook, F, Fut, SleepImpl, T, E, SleepFut, C>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_on_exit(hook))
    }
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
