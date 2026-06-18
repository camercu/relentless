use super::super::execution::common::RetryOp;
use super::super::execution::sync_exec::{NoSyncSleep, SyncRetryExec, SyncRetryExecWithStats};
use super::super::time::ElapsedTracker;
use super::super::{ExecutionHooks, RetryPolicy};
use crate::state::RetryState;
use crate::{predicate, stop, wait};

/// Extension trait to start sync retries directly from a closure/function.
///
/// The operation takes no parameters. Use the free function [`crate::retry`]
/// when you need access to [`RetryState`].
pub trait RetryExt<T, E>: FnMut() -> Result<T, E> + Sized {
    /// Starts an owned sync retry builder from [`RetryPolicy::default()`].
    ///
    /// This means extension-based retries default to:
    /// - `stop::attempts(3)`
    /// - exponential backoff starting at 100ms
    /// - retry on any error
    ///
    /// In `std` builds, `.call()` works without `.sleep(...)`. The example
    /// below still configures `.sleep(...)` so it also works in non-`std`
    /// documentation tests.
    ///
    /// ```
    /// use relentless::RetryExt;
    ///
    /// let _ = (|| Ok::<(), &str>(()))
    ///     .retry()
    ///     .sleep(|_| {})
    ///     .call();
    /// ```
    fn retry(self) -> DefaultSyncRetryBuilder<Self, T, E>;
}

/// Adapts a no-argument closure to the [`RetryOp`] trait by discarding the
/// [`RetryState`] parameter that the execution engine always passes.
///
/// This is the bridge between the ext-trait API (`FnMut() -> Result`) and the
/// execution engine, which requires `FnMut(RetryState) -> Result`.
#[doc(hidden)]
pub struct StatelessOp<F>(F);

impl<T, E, F: FnMut() -> Result<T, E>> RetryOp<T, E> for StatelessOp<F> {
    fn call_op(&mut self, _state: RetryState) -> Result<T, E> {
        (self.0)()
    }
}

impl<T, E, F> RetryExt<T, E> for F
where
    F: FnMut() -> Result<T, E> + Sized,
{
    fn retry(self) -> DefaultSyncRetryBuilder<Self, T, E> {
        SyncRetryExec::new(
            RetryPolicy::default(),
            ExecutionHooks::new(),
            StatelessOp(self),
            NoSyncSleep,
            ElapsedTracker::new(None),
        )
    }
}

/// Owned sync retry builder created from [`RetryExt::retry`] or [`crate::retry`].
///
/// Backed by [`SyncRetryExec`] with an owned [`RetryPolicy`]; the owned policy
/// is what enables the `stop`/`wait`/`when`/`until` builder methods.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use relentless::{RetryExt, stop};
///
/// let retry = (|| Ok::<u32, &str>(1))
///     .retry()
///     .stop(stop::attempts(2))
///     .sleep(|_dur: Duration| {});
///
/// let _ = retry;
/// ```
pub type SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E> =
    SyncRetryExec<RetryPolicy<S, W, P>, BA, AA, OX, F, SleepFn, T, E>;

/// Owned sync retry builder wrapper that returns statistics.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use relentless::RetryExt;
///
/// let retry = (|| Ok::<u32, &str>(1))
///     .retry()
///     .sleep(|_dur: Duration| {})
///     .with_stats();
///
/// let _ = retry;
/// ```
pub type SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E> =
    SyncRetryExecWithStats<RetryPolicy<S, W, P>, BA, AA, OX, F, SleepFn, T, E>;

/// Alias for the default owned sync retry builder returned by [`RetryExt::retry`].
///
/// This hides the default stop, wait, predicate, hook, and sleeper
/// state from user-facing type signatures.
pub type DefaultSyncRetryBuilder<F, T, E> = SyncRetryBuilder<
    stop::StopAfterAttempts,
    wait::WaitExponential,
    predicate::PredicateAnyError,
    (),
    (),
    (),
    StatelessOp<F>,
    NoSyncSleep,
    T,
    E,
>;

/// Alias for the default owned sync retry builder-with-stats returned by
/// calling `.with_stats()` on [`RetryExt::retry`].
pub type DefaultSyncRetryBuilderWithStats<F, SleepFn, T, E> = SyncRetryBuilderWithStats<
    stop::StopAfterAttempts,
    wait::WaitExponential,
    predicate::PredicateAnyError,
    (),
    (),
    (),
    StatelessOp<F>,
    SleepFn,
    T,
    E,
>;

#[cfg(not(feature = "std"))]
#[doc(hidden)]
/// ```compile_fail
/// use relentless::RetryExt;
///
/// let _ = (|| Err::<(), &str>("fail"))
///     .retry()
///     .call();
/// ```
#[allow(dead_code)]
fn _sync_retry_builder_requires_sleep_in_no_std() {}

impl<S, W, P, F, T, E> SyncRetryExec<RetryPolicy<S, W, P>, (), (), (), F, NoSyncSleep, T, E> {
    pub(crate) fn from_policy(policy: RetryPolicy<S, W, P>, op: F) -> Self {
        SyncRetryExec::new(
            policy,
            ExecutionHooks::new(),
            op,
            NoSyncSleep,
            ElapsedTracker::new(None),
        )
    }
}

/// Policy-mutating builder methods, available only when the policy is owned.
//
// Intentional: threading the full type-state keeps the zero-cost generics,
// which naturally yields long concrete return types.
#[allow(clippy::type_complexity)]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E>
    SyncRetryExec<RetryPolicy<S, W, P>, BA, AA, OX, F, SleepFn, T, E>
{
    /// Sets the stop condition for the retry policy.
    #[must_use]
    pub fn stop<NewStop>(
        self,
        stop: NewStop,
    ) -> SyncRetryExec<RetryPolicy<NewStop, W, P>, BA, AA, OX, F, SleepFn, T, E> {
        self.map_policy(|policy| policy.stop(stop))
    }

    /// Sets the wait strategy used between retry attempts.
    #[must_use]
    pub fn wait<NewWait>(
        self,
        wait: NewWait,
    ) -> SyncRetryExec<RetryPolicy<S, NewWait, P>, BA, AA, OX, F, SleepFn, T, E> {
        self.map_policy(|policy| policy.wait(wait))
    }

    /// Sets the predicate that decides whether a failed attempt should be retried.
    #[must_use]
    pub fn when<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> SyncRetryExec<RetryPolicy<S, W, NewPredicate>, BA, AA, OX, F, SleepFn, T, E> {
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
    ) -> SyncRetryExec<
        RetryPolicy<S, W, predicate::PredicateUntil<NewPredicate>>,
        BA,
        AA,
        OX,
        F,
        SleepFn,
        T,
        E,
    > {
        self.map_policy(|policy| policy.until(predicate))
    }
}
