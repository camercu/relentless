use core::fmt;
use core::marker::PhantomData;

use super::common::execute_sync_loop;
use crate::cancel::{Canceler, NeverCancel};
use crate::compat::Duration;
use crate::error::RetryError;
#[cfg(feature = "alloc")]
use crate::policy::HookChain;
use crate::policy::{
    AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, PolicyHandle, RetryPolicy,
};
use crate::predicate::Predicate;
use crate::state::{AttemptState, BeforeAttemptState, ExitState};
use crate::stats::RetryStats;
use crate::stop::Stop;
use crate::wait::Wait;

/// Marker for the absence of an explicit sync sleep function.
#[doc(hidden)]
pub struct NoSyncSleep;

/// Abstraction for blocking sleep functions used by [`SyncRetry`].
///
/// The sync execution engine calls this to pause between retry attempts.
/// A blanket implementation is provided for `FnMut(Duration)` closures.
/// When the `std` feature is active, [`NoSyncSleep`] defaults to
/// [`std::thread::sleep`].
#[doc(hidden)]
pub trait SyncSleep {
    /// Blocks the current thread for the given duration.
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

pub(crate) struct SyncRetryCore<Policy, BA, AA, OX, F, SleepFn, T, E, C = NeverCancel> {
    policy: Policy,
    hooks: ExecutionHooks<BA, AA, OX>,
    op: F,
    sleeper: SleepFn,
    canceler: C,
    reset_policy_before_run: bool,
    _marker: PhantomData<fn() -> (T, E)>,
}

impl<Policy, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetryCore<Policy, BA, AA, OX, F, SleepFn, T, E, C>
{
    pub(crate) fn new(
        policy: Policy,
        hooks: ExecutionHooks<BA, AA, OX>,
        op: F,
        sleeper: SleepFn,
        canceler: C,
        reset_policy_before_run: bool,
    ) -> Self {
        Self {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            reset_policy_before_run,
            _marker: PhantomData,
        }
    }

    pub(crate) fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> SyncRetryCore<Policy, NewBA, NewAA, NewOX, F, SleepFn, T, E, C> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            reset_policy_before_run,
            ..
        } = self;
        SyncRetryCore {
            policy,
            hooks: map(hooks),
            op,
            sleeper,
            canceler,
            reset_policy_before_run,
            _marker: PhantomData,
        }
    }

    pub(crate) fn map_policy<NewPolicy>(
        self,
        map: impl FnOnce(Policy) -> NewPolicy,
    ) -> SyncRetryCore<NewPolicy, BA, AA, OX, F, SleepFn, T, E, C> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            reset_policy_before_run,
            ..
        } = self;
        SyncRetryCore {
            policy: map(policy),
            hooks,
            op,
            sleeper,
            canceler,
            reset_policy_before_run,
            _marker: PhantomData,
        }
    }

    pub(crate) fn with_sleeper<NewSleepFn>(
        self,
        sleeper: NewSleepFn,
    ) -> SyncRetryCore<Policy, BA, AA, OX, F, NewSleepFn, T, E, C> {
        let Self {
            policy,
            hooks,
            op,
            canceler,
            reset_policy_before_run,
            ..
        } = self;
        SyncRetryCore {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            reset_policy_before_run,
            _marker: PhantomData,
        }
    }

    pub(crate) fn with_canceler<NewC>(
        self,
        canceler: NewC,
    ) -> SyncRetryCore<Policy, BA, AA, OX, F, SleepFn, T, E, NewC> {
        let Self {
            policy,
            hooks,
            op,
            sleeper,
            reset_policy_before_run,
            ..
        } = self;
        SyncRetryCore {
            policy,
            hooks,
            op,
            sleeper,
            canceler,
            reset_policy_before_run,
            _marker: PhantomData,
        }
    }

    pub(crate) fn execute<S, W, P, const COLLECT_STATS: bool>(
        mut self,
    ) -> (Result<T, RetryError<E, T>>, Option<RetryStats>)
    where
        Policy: PolicyHandle<S, W, P>,
        S: Stop,
        W: Wait,
        P: Predicate<T, E>,
        BA: BeforeAttemptHook,
        AA: AttemptHook<T, E>,
        OX: ExitHook<T, E>,
        F: FnMut() -> Result<T, E>,
        SleepFn: SyncSleep,
        C: Canceler,
    {
        if self.reset_policy_before_run {
            let policy = self.policy.policy_mut();
            policy.stop.reset();
            policy.wait.reset();
        }

        let policy = self.policy.policy_mut();
        execute_sync_loop::<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, COLLECT_STATS>(
            policy,
            &mut self.hooks,
            &mut self.op,
            &mut self.sleeper,
            &self.canceler,
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
/// use tenacious::{RetryPolicy, stop};
///
/// let mut policy = RetryPolicy::new().stop(stop::attempts(2));
/// let retry = policy
///     .retry(|| Err::<(), _>("fail"))
///     .before_attempt(|_state| {})
///     .sleep(|_dur| {});
/// let _ = retry.call();
/// ```
#[allow(clippy::type_complexity)]
pub struct SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C = NeverCancel> {
    inner: SyncRetryCore<&'policy mut RetryPolicy<S, W, P>, BA, AA, OX, F, SleepFn, T, E, C>,
}

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C> fmt::Debug
    for SyncRetry<'_, S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRetry").finish_non_exhaustive()
    }
}

#[cfg(not(feature = "std"))]
#[doc(hidden)]
/// ```compile_fail
/// use tenacious::{RetryPolicy, stop};
///
/// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
/// let _ = policy.retry(|| Err::<(), _>("fail")).call();
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
/// use tenacious::{RetryPolicy, stop};
///
/// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
/// let (_result, _stats) = policy
///     .retry(|| Ok::<u32, &str>(1))
///     .sleep(|_dur| {})
///     .with_stats()
///     .call();
/// ```
pub struct SyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C = NeverCancel> {
    inner: SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C>,
}

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C> fmt::Debug
    for SyncRetryWithStats<'_, S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRetryWithStats").finish_non_exhaustive()
    }
}

#[cfg(feature = "alloc")]
type SyncRetryWithBeforeHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetry<'policy, S, W, P, HookChain<BA, Hook>, AA, OX, F, SleepFn, T, E, C>;

#[cfg(feature = "alloc")]
type SyncRetryWithAfterHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetry<'policy, S, W, P, BA, HookChain<AA, Hook>, OX, F, SleepFn, T, E, C>;

#[cfg(feature = "alloc")]
type SyncRetryWithOnExitHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetry<'policy, S, W, P, BA, AA, HookChain<OX, Hook>, F, SleepFn, T, E, C>;

impl<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
{
    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> SyncRetry<'policy, S, W, P, NewBA, NewAA, NewOX, F, SleepFn, T, E, C> {
        SyncRetry {
            inner: self.inner.map_hooks(map),
        }
    }
}

impl<'policy, S, W, P, BA, AA, OX, F, T, E, C>
    SyncRetry<'policy, S, W, P, BA, AA, OX, F, NoSyncSleep, T, E, C>
where
    S: Stop,
    W: Wait,
    F: FnMut() -> Result<T, E>,
{
    /// Sets a custom blocking sleep function.
    #[must_use]
    pub fn sleep<SleepFn>(
        self,
        sleeper: SleepFn,
    ) -> SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C> {
        SyncRetry {
            inner: self.inner.with_sleeper(sleeper),
        }
    }
}

impl<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, NeverCancel>
{
    /// Attaches a canceler that is checked before each attempt and after each sleep.
    #[must_use]
    pub fn cancel_on<NewC: Canceler>(
        self,
        canceler: NewC,
    ) -> SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, NewC> {
        SyncRetry {
            inner: self.inner.with_canceler(canceler),
        }
    }
}

#[allow(private_bounds)]
impl<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Result<T, E>,
    SleepFn: SyncSleep,
    C: Canceler,
{
    /// Executes the retry loop synchronously.
    pub fn call(self) -> Result<T, RetryError<E, T>> {
        self.execute::<false>().0
    }

    /// Executes the retry loop and returns aggregate statistics.
    #[must_use]
    pub fn with_stats(
        self,
    ) -> SyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C> {
        SyncRetryWithStats { inner: self }
    }

    fn execute<const COLLECT_STATS: bool>(
        self,
    ) -> (Result<T, RetryError<E, T>>, Option<RetryStats>) {
        self.inner.execute::<S, W, P, COLLECT_STATS>()
    }
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetryWithStats<'_, S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Result<T, E>,
    SleepFn: SyncSleep,
    C: Canceler,
{
    /// Executes the retry loop synchronously and returns `(result, stats)`.
    pub fn call(self) -> (Result<T, RetryError<E, T>>, RetryStats) {
        let (result, stats) = self.inner.execute::<true>();
        (result, stats.expect("sync retry completed without stats"))
    }
}

impl_alloc_hook_chain! {
    impl['policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C]
    SyncRetry<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C> =>
    before_attempt -> { SyncRetryWithBeforeHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> },
    after_attempt -> { SyncRetryWithAfterHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> },
    on_exit -> { SyncRetryWithOnExitHook<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> },
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, AA, OX, F, SleepFn, T, E, C>
    SyncRetry<'policy, S, W, P, (), AA, OX, F, SleepFn, T, E, C>
{
    /// Sets the sole before-attempt hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry(|| Err::<(), _>("fail"))
    ///     .before_attempt(|_state| {})
    ///     .before_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetry<'policy, S, W, P, Hook, AA, OX, F, SleepFn, T, E, C>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, OX, F, SleepFn, T, E, C>
    SyncRetry<'policy, S, W, P, BA, (), OX, F, SleepFn, T, E, C>
{
    /// Sets the sole after-attempt hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry(|| Err::<(), _>("fail"))
    ///     .after_attempt(|_state| {})
    ///     .after_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetry<'policy, S, W, P, BA, Hook, OX, F, SleepFn, T, E, C>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, AA, F, SleepFn, T, E, C>
    SyncRetry<'policy, S, W, P, BA, AA, (), F, SleepFn, T, E, C>
{
    /// Sets the sole on-exit hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry(|| Err::<(), _>("fail"))
    ///     .on_exit(|_state| {})
    ///     .on_exit(|_state| {});
    /// ```
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetry<'policy, S, W, P, BA, AA, Hook, F, SleepFn, T, E, C>
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
    /// Begins configuring sync retry execution.
    #[must_use]
    pub fn retry<T, E, F>(
        &mut self,
        op: F,
    ) -> SyncRetry<'_, S, W, P, (), (), (), F, NoSyncSleep, T, E>
    where
        F: FnMut() -> Result<T, E>,
    {
        self.stop.reset();
        self.wait.reset();
        SyncRetry {
            inner: SyncRetryCore::new(
                self,
                ExecutionHooks::new(),
                op,
                NoSyncSleep,
                NeverCancel,
                false,
            ),
        }
    }
}
