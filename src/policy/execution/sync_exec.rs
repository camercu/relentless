use core::fmt;
use core::marker::PhantomData;

use super::common::execute_sync_loop;
use crate::compat::Duration;
use crate::error::RetryError;
#[cfg(feature = "alloc")]
use crate::policy::HookChain;
use crate::policy::time::ElapsedTracker;
use crate::policy::{
    AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, PolicyHandle, RetryPolicy,
};
use crate::predicate::Predicate;
use crate::state::{AttemptState, ExitState, RetryState};
use crate::stats::RetryStats;
use crate::stop::Stop;
use crate::wait::Wait;

/// Marker for the absence of an explicit sync sleep function.
#[doc(hidden)]
pub struct NoSyncSleep;

/// Blocking sleep abstraction for the sync retry execution engine.
///
/// A blanket implementation covers `FnMut(Duration)` closures, enabling
/// `.sleep(|dur| ...)` in the builder API. When the `std` feature is active,
/// [`NoSyncSleep`] defaults to [`std::thread::sleep`].
#[doc(hidden)]
pub trait SyncSleep {
    fn sleep(&mut self, dur: Duration);
}

impl<F> SyncSleep for F
where
    F: FnMut(Duration),
{
    fn sleep(&mut self, dur: Duration) {
        (self)(dur);
    }
}

#[cfg(feature = "std")]
impl SyncSleep for NoSyncSleep {
    fn sleep(&mut self, dur: Duration) {
        std::thread::sleep(dur);
    }
}

pub(crate) struct SyncRetryCore<Policy, BA, AA, OX, F, SleepFn, T, E> {
    policy: Policy,
    hooks: ExecutionHooks<BA, AA, OX>,
    op: F,
    sleeper: SleepFn,
    elapsed_tracker: ElapsedTracker,
    timeout: Option<Duration>,
    _marker: PhantomData<fn() -> (T, E)>,
}

impl<Policy, BA, AA, OX, F, SleepFn, T, E> SyncRetryCore<Policy, BA, AA, OX, F, SleepFn, T, E> {
    pub(crate) fn new(
        policy: Policy,
        hooks: ExecutionHooks<BA, AA, OX>,
        op: F,
        sleeper: SleepFn,
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

    pub(crate) fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> SyncRetryCore<Policy, NewBA, NewAA, NewOX, F, SleepFn, T, E> {
        SyncRetryCore {
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
    ) -> SyncRetryCore<NewPolicy, BA, AA, OX, F, SleepFn, T, E> {
        SyncRetryCore {
            policy: map(self.policy),
            hooks: self.hooks,
            op: self.op,
            sleeper: self.sleeper,
            elapsed_tracker: self.elapsed_tracker,
            timeout: self.timeout,
            _marker: PhantomData,
        }
    }

    pub(crate) fn with_sleeper<NewSleepFn>(
        self,
        sleeper: NewSleepFn,
    ) -> SyncRetryCore<Policy, BA, AA, OX, F, NewSleepFn, T, E> {
        SyncRetryCore {
            policy: self.policy,
            hooks: self.hooks,
            op: self.op,
            sleeper,
            elapsed_tracker: self.elapsed_tracker,
            timeout: self.timeout,
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

    pub(crate) fn execute<S, W, P, const COLLECT_STATS: bool>(
        mut self,
    ) -> (Result<T, RetryError<T, E>>, Option<RetryStats>)
    where
        Policy: PolicyHandle<S, W, P>,
        S: Stop,
        W: Wait,
        P: Predicate<T, E>,
        BA: BeforeAttemptHook,
        AA: AttemptHook<T, E>,
        OX: ExitHook<T, E>,
        F: super::common::RetryOp<T, E>,
        SleepFn: SyncSleep,
    {
        let policy = self.policy.policy_ref();
        execute_sync_loop::<S, W, P, BA, AA, OX, F, SleepFn, T, E, COLLECT_STATS>(
            policy,
            &mut self.hooks,
            &mut self.op,
            &mut self.sleeper,
            &self.elapsed_tracker,
            self.timeout,
        )
    }
}

/// Sync retry execution object.
///
/// Created by [`RetryPolicy::retry`]. Configure hooks and `.sleep(...)`, then
/// call `.call()`.
///
/// In `std` builds, calling `.sleep(...)` is optional because a default
/// blocking sleeper is available. In non-`std` builds, `.sleep(...)` is
/// required before `.call()` is available.
///
/// # Examples
///
/// ```
/// use relentless::{RetryPolicy, stop};
///
/// let policy = RetryPolicy::new().stop(stop::attempts(2));
/// let retry = policy
///     .retry(|_| Err::<(), _>("fail"))
///     .before_attempt(|_state| {})
///     .sleep(|_dur| {});
/// let _ = retry.call();
/// ```
#[allow(clippy::type_complexity)]
pub struct SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E> {
    inner: SyncRetryCore<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, SleepFn, T, E>,
}

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E> fmt::Debug
    for SyncRetry<'_, S, W, P, BA, AA, OX, F, SleepFn, T, E>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRetry").finish_non_exhaustive()
    }
}

#[cfg(not(feature = "std"))]
#[doc(hidden)]
/// ```compile_fail
/// use relentless::{RetryPolicy, stop};
///
/// let policy = RetryPolicy::new().stop(stop::attempts(1));
/// let _ = policy.retry(|_| Err::<(), _>("fail")).call();
/// ```
#[allow(dead_code)]
fn _sync_call_requires_sleep_in_no_std() {}

/// Sync retry execution wrapper that returns statistics.
///
/// Created by calling `.with_stats()` on [`SyncRetry`].
///
/// # Examples
///
/// ```
/// use relentless::{RetryPolicy, stop};
///
/// let policy = RetryPolicy::new().stop(stop::attempts(1));
/// let (_result, _stats) = policy
///     .retry(|_| Ok::<u32, &str>(1))
///     .sleep(|_dur| {})
///     .with_stats()
///     .call();
/// ```
pub struct SyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E> {
    inner: SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E>,
}

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E> fmt::Debug
    for SyncRetryWithStats<'_, S, W, P, BA, AA, OX, F, SleepFn, T, E>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRetryWithStats").finish_non_exhaustive()
    }
}

#[cfg(feature = "alloc")]
type SyncRetryWithBeforeHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> =
    SyncRetry<'policy, S, W, P, HookChain<BA, Hook>, AA, OX, F, SleepFn, T, E>;

#[cfg(feature = "alloc")]
type SyncRetryWithAfterHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> =
    SyncRetry<'policy, S, W, P, BA, HookChain<AA, Hook>, OX, F, SleepFn, T, E>;

#[cfg(feature = "alloc")]
type SyncRetryWithOnExitHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> =
    SyncRetry<'policy, S, W, P, BA, AA, HookChain<OX, Hook>, F, SleepFn, T, E>;

impl<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E>
{
    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> SyncRetry<'policy, S, W, P, NewBA, NewAA, NewOX, F, SleepFn, T, E> {
        SyncRetry {
            inner: self.inner.map_hooks(map),
        }
    }

    /// Configures a custom elapsed clock for elapsed-based stop conditions.
    #[must_use]
    pub fn elapsed_clock(self, clock: fn() -> Duration) -> Self {
        SyncRetry {
            inner: self.inner.set_elapsed_clock(clock),
        }
    }

    /// Configures a custom elapsed clock from a boxed closure.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn elapsed_clock_fn(self, clock: impl Fn() -> Duration + 'static) -> Self {
        SyncRetry {
            inner: self
                .inner
                .set_elapsed_clock_fn(crate::compat::Box::new(clock)),
        }
    }

    /// Sets a wall-clock deadline for the entire retry execution.
    #[must_use]
    pub fn timeout(self, dur: Duration) -> Self {
        SyncRetry {
            inner: self.inner.set_timeout(dur),
        }
    }
}

impl<'policy, S, W, P, BA, AA, OX, F, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, OX, F, NoSyncSleep, T, E>
{
    /// Sets the blocking sleep function used between retry attempts.
    #[must_use]
    pub fn sleep<SleepFn>(
        self,
        sleeper: SleepFn,
    ) -> SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E> {
        SyncRetry {
            inner: self.inner.with_sleeper(sleeper),
        }
    }
}

#[allow(private_bounds)]
impl<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: super::common::RetryOp<T, E>,
    SleepFn: SyncSleep,
{
    /// Executes the retry loop and returns the final result.
    ///
    /// # Errors
    ///
    /// Returns [`RetryError`] if all attempts are exhausted or a non-retryable
    /// error is encountered.
    pub fn call(self) -> Result<T, RetryError<T, E>> {
        self.execute::<false>().0
    }

    /// Wraps this execution object to also return [`RetryStats`] on completion.
    ///
    /// Does not execute the retry loop; call `.call()` on the returned wrapper.
    #[must_use]
    pub fn with_stats(self) -> SyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E> {
        SyncRetryWithStats { inner: self }
    }

    fn execute<const COLLECT_STATS: bool>(
        self,
    ) -> (Result<T, RetryError<T, E>>, Option<RetryStats>) {
        self.inner.execute::<S, W, P, COLLECT_STATS>()
    }
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E>
    SyncRetryWithStats<'_, S, W, P, BA, AA, OX, F, SleepFn, T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: super::common::RetryOp<T, E>,
    SleepFn: SyncSleep,
{
    /// Executes the retry loop and returns both the result and collected stats.
    ///
    /// # Panics
    ///
    /// Panics if stats collection fails internally (should not happen in
    /// practice).
    pub fn call(self) -> (Result<T, RetryError<T, E>>, RetryStats) {
        let (result, stats) = self.inner.execute::<true>();
        (result, stats.expect("sync retry completed without stats"))
    }
}

impl_alloc_hook_chain! {
    impl['policy, S, W, P, BA, AA, OX, F, SleepFn, T, E]
    SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E> =>
    before_attempt -> { SyncRetryWithBeforeHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> },
    after_attempt -> { SyncRetryWithAfterHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> },
    on_exit -> { SyncRetryWithOnExitHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> },
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, AA, OX, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, (), AA, OX, F, SleepFn, T, E>
{
    /// Sets the before-attempt hook.
    ///
    /// Without `alloc`, only one hook per slot is supported; calling this
    /// twice is a compile error (the `BA` type parameter would already be
    /// non-`()`).
    ///
    /// ```compile_fail
    /// use relentless::{RetryPolicy, stop};
    ///
    /// let policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry(|_| Err::<(), _>("fail"))
    ///     .before_attempt(|_state| {})
    ///     .before_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetry<'policy, S, W, P, Hook, AA, OX, F, SleepFn, T, E>
    where
        Hook: FnMut(&RetryState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, OX, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, (), OX, F, SleepFn, T, E>
{
    /// Sets the after-attempt hook.
    ///
    /// Without `alloc`, only one hook per slot is supported; calling this
    /// twice is a compile error.
    ///
    /// ```compile_fail
    /// use relentless::{RetryPolicy, stop};
    ///
    /// let policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry(|_| Err::<(), _>("fail"))
    ///     .after_attempt(|_state| {})
    ///     .after_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetry<'policy, S, W, P, BA, Hook, OX, F, SleepFn, T, E>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, AA, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, (), F, SleepFn, T, E>
{
    /// Sets the on-exit hook.
    ///
    /// Without `alloc`, only one hook per slot is supported; calling this
    /// twice is a compile error.
    ///
    /// ```compile_fail
    /// use relentless::{RetryPolicy, stop};
    ///
    /// let policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry(|_| Err::<(), _>("fail"))
    ///     .on_exit(|_state| {})
    ///     .on_exit(|_state| {});
    /// ```
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetry<'policy, S, W, P, BA, AA, Hook, F, SleepFn, T, E>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_on_exit(hook))
    }
}

impl<S, W, P> RetryPolicy<S, W, P>
where
    S: Stop,
    W: Wait,
{
    /// Creates a synchronous retry execution for the given operation.
    #[must_use]
    pub fn retry<T, E, F>(&self, op: F) -> SyncRetry<'_, S, W, P, (), (), (), F, NoSyncSleep, T, E>
    where
        F: FnMut(RetryState) -> Result<T, E>,
    {
        SyncRetry {
            inner: SyncRetryCore::new(
                self,
                ExecutionHooks::new(),
                op,
                NoSyncSleep,
                ElapsedTracker::new(None),
            ),
        }
    }
}
