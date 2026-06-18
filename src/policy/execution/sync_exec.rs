use core::fmt;
use core::marker::PhantomData;

use super::common::{RetryOp, execute_sync_loop};
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

/// Sync retry execution object, generic over how the policy is stored.
///
/// A single engine drives both entry points:
/// - [`SyncRetry`] borrows a policy (`Policy = &RetryPolicy<S, W, P>`), created
///   by [`RetryPolicy::retry`].
/// - [`SyncRetryBuilder`](crate::SyncRetryBuilder) owns a policy
///   (`Policy = RetryPolicy<S, W, P>`), created by [`crate::RetryExt::retry`].
///
/// Methods that only make sense when the policy is owned (`stop`, `wait`,
/// `when`, `until`) are implemented separately on the owned alias.
#[allow(clippy::type_complexity)]
pub struct SyncRetryExec<Policy, BA, AA, OX, F, SleepFn, T, E> {
    policy: Policy,
    hooks: ExecutionHooks<BA, AA, OX>,
    op: F,
    sleeper: SleepFn,
    elapsed_tracker: ElapsedTracker,
    timeout: Option<Duration>,
    _marker: PhantomData<fn() -> (T, E)>,
}

/// Sync retry execution object created by [`RetryPolicy::retry`].
///
/// Configure hooks and `.sleep(...)`, then call `.call()`.
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
pub type SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E> =
    SyncRetryExec<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, SleepFn, T, E>;

/// Sync retry execution wrapper that returns statistics.
///
/// Created by calling `.with_stats()` on a [`SyncRetryExec`].
#[allow(clippy::type_complexity)]
pub struct SyncRetryExecWithStats<Policy, BA, AA, OX, F, SleepFn, T, E> {
    inner: SyncRetryExec<Policy, BA, AA, OX, F, SleepFn, T, E>,
}

/// Sync retry execution-with-stats object created by `.with_stats()` on
/// [`SyncRetry`].
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
pub type SyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E> =
    SyncRetryExecWithStats<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, SleepFn, T, E>;

impl<Policy, BA, AA, OX, F, SleepFn, T, E> fmt::Debug
    for SyncRetryExec<Policy, BA, AA, OX, F, SleepFn, T, E>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRetryExec").finish_non_exhaustive()
    }
}

impl<Policy, BA, AA, OX, F, SleepFn, T, E> fmt::Debug
    for SyncRetryExecWithStats<Policy, BA, AA, OX, F, SleepFn, T, E>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRetryExecWithStats")
            .finish_non_exhaustive()
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

impl<Policy, BA, AA, OX, F, SleepFn, T, E> SyncRetryExec<Policy, BA, AA, OX, F, SleepFn, T, E> {
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

    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> SyncRetryExec<Policy, NewBA, NewAA, NewOX, F, SleepFn, T, E> {
        SyncRetryExec {
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
    ) -> SyncRetryExec<NewPolicy, BA, AA, OX, F, SleepFn, T, E> {
        SyncRetryExec {
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
    #[must_use]
    pub fn elapsed_clock(mut self, clock: fn() -> Duration) -> Self {
        self.elapsed_tracker = ElapsedTracker::new(Some(clock));
        self
    }

    /// Configures a custom elapsed clock from a boxed closure.
    ///
    /// This variant supports closures with captures for test clocks and
    /// runtime state. Requires the `alloc` feature.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn elapsed_clock_fn(mut self, clock: impl Fn() -> Duration + 'static) -> Self {
        self.elapsed_tracker = ElapsedTracker::new_boxed(crate::compat::Box::new(clock));
        self
    }

    /// Sets a wall-clock deadline for the entire retry execution.
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    fn execute<const COLLECT_STATS: bool>(
        mut self,
    ) -> (Result<T, RetryError<T, E>>, Option<RetryStats>)
    where
        Policy: PolicyHandle,
        Policy::Stop: Stop,
        Policy::Wait: Wait,
        Policy::Predicate: Predicate<T, E>,
        BA: BeforeAttemptHook,
        AA: AttemptHook<T, E>,
        OX: ExitHook<T, E>,
        F: RetryOp<T, E>,
        SleepFn: SyncSleep,
    {
        let policy = self.policy.policy_ref();
        execute_sync_loop::<
            Policy::Stop,
            Policy::Wait,
            Policy::Predicate,
            BA,
            AA,
            OX,
            F,
            SleepFn,
            T,
            E,
            COLLECT_STATS,
        >(
            policy,
            &mut self.hooks,
            &mut self.op,
            &mut self.sleeper,
            &self.elapsed_tracker,
            self.timeout,
        )
    }

    /// Wraps this execution object to also return [`RetryStats`] on completion.
    ///
    /// Does not execute the retry loop; call `.call()` on the returned wrapper.
    #[must_use]
    pub fn with_stats(self) -> SyncRetryExecWithStats<Policy, BA, AA, OX, F, SleepFn, T, E> {
        SyncRetryExecWithStats { inner: self }
    }

    /// Executes the retry loop and returns the final result.
    ///
    /// # Errors
    ///
    /// Returns [`RetryError`] if all attempts are exhausted or a non-retryable
    /// error is encountered.
    #[allow(private_bounds)]
    pub fn call(self) -> Result<T, RetryError<T, E>>
    where
        Policy: PolicyHandle,
        Policy::Stop: Stop,
        Policy::Wait: Wait,
        Policy::Predicate: Predicate<T, E>,
        BA: BeforeAttemptHook,
        AA: AttemptHook<T, E>,
        OX: ExitHook<T, E>,
        F: RetryOp<T, E>,
        SleepFn: SyncSleep,
    {
        self.execute::<false>().0
    }
}

impl<Policy, BA, AA, OX, F, T, E> SyncRetryExec<Policy, BA, AA, OX, F, NoSyncSleep, T, E> {
    /// Sets the blocking sleep function used between retry attempts.
    #[must_use]
    pub fn sleep<SleepFn>(
        self,
        sleeper: SleepFn,
    ) -> SyncRetryExec<Policy, BA, AA, OX, F, SleepFn, T, E> {
        SyncRetryExec {
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

impl<Policy, BA, AA, OX, F, SleepFn, T, E>
    SyncRetryExecWithStats<Policy, BA, AA, OX, F, SleepFn, T, E>
{
    /// Executes the retry loop and returns both the result and collected stats.
    ///
    /// # Panics
    ///
    /// Panics if stats collection fails internally (should not happen in
    /// practice).
    #[allow(private_bounds)]
    pub fn call(self) -> (Result<T, RetryError<T, E>>, RetryStats)
    where
        Policy: PolicyHandle,
        Policy::Stop: Stop,
        Policy::Wait: Wait,
        Policy::Predicate: Predicate<T, E>,
        BA: BeforeAttemptHook,
        AA: AttemptHook<T, E>,
        OX: ExitHook<T, E>,
        F: RetryOp<T, E>,
        SleepFn: SyncSleep,
    {
        let (result, stats) = self.inner.execute::<true>();
        (result, stats.expect("sync retry completed without stats"))
    }
}

impl_alloc_hook_chain! {
    impl[Policy, BA, AA, OX, F, SleepFn, T, E]
    SyncRetryExec<Policy, BA, AA, OX, F, SleepFn, T, E> =>
    before_attempt -> { SyncRetryExec<Policy, HookChain<BA, Hook>, AA, OX, F, SleepFn, T, E> },
    after_attempt -> { SyncRetryExec<Policy, BA, HookChain<AA, Hook>, OX, F, SleepFn, T, E> },
    on_exit -> { SyncRetryExec<Policy, BA, AA, HookChain<OX, Hook>, F, SleepFn, T, E> },
}

#[cfg(not(feature = "alloc"))]
impl<Policy, AA, OX, F, SleepFn, T, E> SyncRetryExec<Policy, (), AA, OX, F, SleepFn, T, E> {
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
    ) -> SyncRetryExec<Policy, Hook, AA, OX, F, SleepFn, T, E>
    where
        Hook: FnMut(&RetryState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<Policy, BA, OX, F, SleepFn, T, E> SyncRetryExec<Policy, BA, (), OX, F, SleepFn, T, E> {
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
    ) -> SyncRetryExec<Policy, BA, Hook, OX, F, SleepFn, T, E>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<Policy, BA, AA, F, SleepFn, T, E> SyncRetryExec<Policy, BA, AA, (), F, SleepFn, T, E> {
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
    pub fn on_exit<Hook>(self, hook: Hook) -> SyncRetryExec<Policy, BA, AA, Hook, F, SleepFn, T, E>
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
        SyncRetryExec::new(
            self,
            ExecutionHooks::new(),
            op,
            NoSyncSleep,
            ElapsedTracker::new(None),
        )
    }
}
