use core::fmt;
use core::marker::PhantomData;

use super::common::{RetryOp, execute_sync_loop};
use crate::compat::Duration;
use crate::error::RetryError;
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
    ///
    /// The clock must return a monotonic "now" timestamp — a [`Duration`]
    /// since an arbitrary fixed epoch (e.g. system boot or program start).
    /// The library captures a baseline reading when execution starts and
    /// computes elapsed time as `clock() - baseline`; the clock's absolute
    /// value is never used directly.
    ///
    /// A clock that does not advance pins elapsed time at zero. Combined
    /// with a stop strategy that only checks elapsed time (such as
    /// [`stop::elapsed`](crate::stop::elapsed)) it produces an unbounded
    /// retry loop; pair elapsed-based stops with
    /// [`stop::attempts`](crate::stop::attempts) to stay bounded.
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
    /// This is a **boundary check, not a preemptive timeout.** It is evaluated
    /// between attempts: the next inter-attempt sleep is clamped so the loop
    /// terminates close to the deadline, and the loop stops once the elapsed
    /// time exceeds `dur`. It **cannot** interrupt an operation or a sleep that
    /// is already in progress.
    ///
    /// The blocking sync path has no external preemption point. To abandon
    /// in-flight work early, check a cancellation flag inside the operation
    /// closure and return a non-retryable error; see the `sync-cancel`
    /// example.
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
            &mut self.elapsed_tracker,
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
    ) -> SyncRetryExec<Policy, BA, AA, OX, F, SleepFn, T, E>
    where
        SleepFn: SyncSleep,
    {
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

impl_hook_chain! {
    impl[Policy, BA, AA, OX, F, SleepFn, T, E]
    SyncRetryExec<Policy, BA, AA, OX, F, SleepFn, T, E> =>
    before_attempt -> { SyncRetryExec<Policy, HookChain<BA, Hook>, AA, OX, F, SleepFn, T, E> },
    after_attempt -> { SyncRetryExec<Policy, BA, HookChain<AA, Hook>, OX, F, SleepFn, T, E> },
    on_exit -> { SyncRetryExec<Policy, BA, AA, HookChain<OX, Hook>, F, SleepFn, T, E> },
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
