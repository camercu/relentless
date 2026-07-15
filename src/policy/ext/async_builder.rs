use core::future::Future;

use super::super::execution::async_exec::{AsyncRetryExec, AsyncRetryExecWithStats, NoAsyncSleep};
use super::super::execution::common::AsyncRetryOp;
use super::super::time::ElapsedTracker;
use super::super::{ExecutionHooks, RetryPolicy};
use crate::state::RetryState;
use crate::{predicate, stop, wait};

/// Extension trait to start async retries directly from a closure/function.
///
/// The operation takes no parameters. Use the free function
/// [`crate::retry_async`] when you need access to [`crate::RetryState`]. This
/// is the deliberate two-tier split: the ext method is the stateless shortcut,
/// the free function is the stateful form. The method is `retry_async` (not
/// `retry`): `_async` is the conventional async-variant naming, and async
/// closures (`FnMut() -> Future`) are a different type than the sync ext
/// trait's `FnMut() -> Result`.
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
    ///     let _ = (|| ready(Ok::<(), &str>(()))).retry_async().call().await;
    /// };
    /// ```
    fn retry_async(self) -> DefaultAsyncRetryBuilder<Self, T, E>;
}

/// Adapts a no-argument async closure to the [`AsyncRetryOp`] trait by
/// discarding the [`RetryState`] parameter the execution engine always passes.
#[doc(hidden)]
pub struct StatelessAsyncOp<F>(F);

impl<T, E, Fut: Future<Output = Result<T, E>>, F: FnMut() -> Fut> AsyncRetryOp<T, E, Fut>
    for StatelessAsyncOp<F>
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
    fn retry_async(self) -> DefaultAsyncRetryBuilder<Self, T, E> {
        AsyncRetryExec::new(
            RetryPolicy::default(),
            ExecutionHooks::new(),
            StatelessAsyncOp(self),
            NoAsyncSleep,
            ElapsedTracker::new(None),
        )
    }
}

/// Owned async retry builder created from [`AsyncRetryExt::retry_async`] or
/// [`crate::retry_async`].
///
/// Backed by [`AsyncRetryExec`] with an owned [`RetryPolicy`]; the owned policy
/// is what enables the `stop`/`wait`/`when`/`until` builder methods. The
/// future returned by `.call()` is single-use; polling it after completion is
/// misuse and always panics.
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
pub type AsyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepImpl, T, E> =
    AsyncRetryExec<RetryPolicy<S, W, P>, BA, AA, OX, F, SleepImpl, T, E>;

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
pub type AsyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepImpl, T, E> =
    AsyncRetryExecWithStats<RetryPolicy<S, W, P>, BA, AA, OX, F, SleepImpl, T, E>;

/// Alias for the default owned async retry builder returned by
/// [`AsyncRetryExt::retry_async`].
///
/// This hides the default stop, wait, predicate, hook, and sleeper state from
/// user-facing type signatures.
pub type DefaultAsyncRetryBuilder<F, T, E> = AsyncRetryBuilder<
    stop::StopAfterAttempts,
    wait::WaitExponential,
    predicate::PredicateAnyError,
    (),
    (),
    (),
    StatelessAsyncOp<F>,
    NoAsyncSleep,
    T,
    E,
>;

/// Alias for the default owned async retry builder-with-stats returned by
/// calling `.with_stats()` on [`AsyncRetryExt::retry_async`].
pub type DefaultAsyncRetryBuilderWithStats<F, SleepImpl, T, E> = AsyncRetryBuilderWithStats<
    stop::StopAfterAttempts,
    wait::WaitExponential,
    predicate::PredicateAnyError,
    (),
    (),
    (),
    StatelessAsyncOp<F>,
    SleepImpl,
    T,
    E,
>;

#[doc(hidden)]
/// ```compile_fail
/// use core::future::ready;
/// use relentless::AsyncRetryExt;
///
/// let _ = async {
///     let _ = (|| ready(Ok::<(), &str>(())))
///         .retry_async()
///         .call()
///         .await;
/// };
/// ```
#[allow(dead_code)]
fn _async_retry_builder_requires_sleep_before_call() {}

impl<S, W, P, F, Fut, T, E> AsyncRetryExec<RetryPolicy<S, W, P>, (), (), (), F, NoAsyncSleep, T, E>
where
    F: FnMut(RetryState) -> Fut,
{
    pub(crate) fn from_policy(policy: RetryPolicy<S, W, P>, op: F) -> Self {
        AsyncRetryExec::new(
            policy,
            ExecutionHooks::new(),
            op,
            NoAsyncSleep,
            ElapsedTracker::new(None),
        )
    }
}

/// Policy-mutating builder methods, available only when the policy is owned.
//
// Intentional: threading the full type-state keeps the zero-cost generics,
// which naturally yields long concrete return types.
#[allow(clippy::type_complexity)]
impl<S, W, P, BA, AA, OX, F, SleepImpl, T, E>
    AsyncRetryExec<RetryPolicy<S, W, P>, BA, AA, OX, F, SleepImpl, T, E>
{
    /// Sets the stop condition for the retry policy.
    #[must_use]
    pub fn stop<NewStop>(
        self,
        stop: NewStop,
    ) -> AsyncRetryExec<RetryPolicy<NewStop, W, P>, BA, AA, OX, F, SleepImpl, T, E> {
        self.map_policy(|policy| policy.stop(stop))
    }

    /// Sets the wait strategy used between retry attempts.
    #[must_use]
    pub fn wait<NewWait>(
        self,
        wait: NewWait,
    ) -> AsyncRetryExec<RetryPolicy<S, NewWait, P>, BA, AA, OX, F, SleepImpl, T, E> {
        self.map_policy(|policy| policy.wait(wait))
    }

    /// Sets the predicate that decides whether a failed attempt should be retried.
    #[must_use]
    pub fn when<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> AsyncRetryExec<RetryPolicy<S, W, NewPredicate>, BA, AA, OX, F, SleepImpl, T, E> {
        self.map_policy(|policy| policy.when(predicate))
    }

    /// Sets a predicate that retries *until* `p.should_retry()` returns `true`.
    ///
    /// Wraps `p` in [`PredicateUntil`](crate::predicate::PredicateUntil).
    /// Natural for polling: `.until(ok(|s| s.is_ready()))`.
    #[must_use]
    pub fn until<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> AsyncRetryExec<
        RetryPolicy<S, W, predicate::PredicateUntil<NewPredicate>>,
        BA,
        AA,
        OX,
        F,
        SleepImpl,
        T,
        E,
    > {
        self.map_policy(|policy| policy.until(predicate))
    }
}
