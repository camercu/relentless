use core::fmt;
use core::marker::PhantomData;

use super::common::{RetryOp, execute_sync_loop};
use crate::clock::{SyncClock, SystemClock};
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
pub struct SyncRetryExec<Policy, BA, AA, OX, F, C, T, E> {
    policy: Policy,
    hooks: ExecutionHooks<BA, AA, OX>,
    op: F,
    clock: C,
    timeout: Option<Duration>,
    _marker: PhantomData<fn() -> (T, E)>,
}

/// Sync retry execution object created by [`RetryPolicy::retry`].
///
/// Configure hooks and optionally a clock, then call `.call()`.
///
/// In `std` builds, calling `.clock(...)` is optional because the default
/// [`SystemClock`](crate::clock::SystemClock) provides wall time and a blocking
/// sleep. In non-`std` builds, `.clock(...)` is required before `.call()` is
/// available.
///
/// # Examples
///
/// ```
/// use relentless::clock::VirtualClock;
/// use relentless::{RetryPolicy, stop};
///
/// let policy = RetryPolicy::new().stop(stop::attempts(2));
/// let retry = policy
///     .retry(|_| Err::<(), _>("fail"))
///     .before_attempt(|_state| {})
///     .clock(VirtualClock::new());
/// let _ = retry.call();
/// ```
pub type SyncRetry<'policy, S, W, P, BA, AA, OX, F, C, T, E> =
    SyncRetryExec<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, C, T, E>;

/// Sync retry execution wrapper that returns statistics.
///
/// Created by calling `.with_stats()` on a [`SyncRetryExec`].
#[allow(clippy::type_complexity)]
pub struct SyncRetryExecWithStats<Policy, BA, AA, OX, F, C, T, E> {
    inner: SyncRetryExec<Policy, BA, AA, OX, F, C, T, E>,
}

/// Sync retry execution-with-stats object created by `.with_stats()` on
/// [`SyncRetry`].
///
/// # Examples
///
/// ```
/// use relentless::clock::VirtualClock;
/// use relentless::{RetryPolicy, stop};
///
/// let policy = RetryPolicy::new().stop(stop::attempts(1));
/// let (_result, _stats) = policy
///     .retry(|_| Ok::<u32, &str>(1))
///     .clock(VirtualClock::new())
///     .with_stats()
///     .call();
/// ```
pub type SyncRetryWithStats<'policy, S, W, P, BA, AA, OX, F, C, T, E> =
    SyncRetryExecWithStats<&'policy RetryPolicy<S, W, P>, BA, AA, OX, F, C, T, E>;

impl<Policy, BA, AA, OX, F, C, T, E> fmt::Debug for SyncRetryExec<Policy, BA, AA, OX, F, C, T, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRetryExec").finish_non_exhaustive()
    }
}

impl<Policy, BA, AA, OX, F, C, T, E> fmt::Debug
    for SyncRetryExecWithStats<Policy, BA, AA, OX, F, C, T, E>
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
fn _sync_call_requires_clock_in_no_std() {}

impl<Policy, BA, AA, OX, F, C, T, E> SyncRetryExec<Policy, BA, AA, OX, F, C, T, E> {
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
    ) -> SyncRetryExec<Policy, NewBA, NewAA, NewOX, F, C, T, E> {
        SyncRetryExec {
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
    ) -> SyncRetryExec<NewPolicy, BA, AA, OX, F, C, T, E> {
        SyncRetryExec {
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
    /// This is a **boundary check, not a preemptive timeout.** It is evaluated
    /// between attempts: the next inter-attempt wait is clamped so the loop
    /// terminates close to the deadline, and the loop stops once the elapsed
    /// time exceeds `dur`. It **cannot** interrupt an operation or a wait that
    /// is already in progress.
    ///
    /// Elapsed time is read from the configured [`clock`](Self::clock) — the
    /// same value that performs the waits, so the two cannot disagree.
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
        C: SyncClock,
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
            C,
            T,
            E,
            COLLECT_STATS,
        >(
            policy,
            &mut self.hooks,
            &mut self.op,
            &self.clock,
            self.timeout,
        )
    }

    /// Wraps this execution object to also return [`RetryStats`] on completion.
    ///
    /// Does not execute the retry loop; call `.call()` on the returned wrapper.
    #[must_use]
    pub fn with_stats(self) -> SyncRetryExecWithStats<Policy, BA, AA, OX, F, C, T, E> {
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
        C: SyncClock,
    {
        self.execute::<false>().0
    }
}

impl<Policy, BA, AA, OX, F, T, E> SyncRetryExec<Policy, BA, AA, OX, F, SystemClock, T, E> {
    /// Sets the clock that supplies elapsed time and performs the wait between
    /// retry attempts.
    ///
    /// One value owns both seams ([`Clock::now`](crate::clock::Clock::now) and
    /// [`SyncClock::wait`](crate::clock::SyncClock::wait)), so `timeout`,
    /// [`stop::elapsed`](crate::stop::elapsed), and recorded waits always agree.
    /// Replaces the default [`SystemClock`](crate::clock::SystemClock)
    /// (wall time + `std::thread::sleep`); use
    /// [`VirtualClock`](crate::clock::VirtualClock) for deterministic tests.
    /// Callable at most once: the method exists only while the builder still
    /// carries the default clock type.
    #[must_use]
    pub fn clock<NewClock>(
        self,
        clock: NewClock,
    ) -> SyncRetryExec<Policy, BA, AA, OX, F, NewClock, T, E>
    where
        NewClock: SyncClock,
    {
        SyncRetryExec {
            policy: self.policy,
            hooks: self.hooks,
            op: self.op,
            clock,
            timeout: self.timeout,
            _marker: PhantomData,
        }
    }
}

impl<Policy, BA, AA, OX, F, C, T, E> SyncRetryExecWithStats<Policy, BA, AA, OX, F, C, T, E> {
    /// Executes the retry loop and returns both the result and collected stats.
    // No `# Panics` section: the `expect` is unreachable by construction —
    // `execute::<true>` always produces stats (SPEC 15.3).
    #[allow(clippy::missing_panics_doc)]
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
        C: SyncClock,
    {
        let (result, stats) = self.inner.execute::<true>();
        (result, stats.expect("sync retry completed without stats"))
    }
}

impl_hook_chain! {
    impl[Policy, BA, AA, OX, F, C, T, E]
    SyncRetryExec<Policy, BA, AA, OX, F, C, T, E> =>
    before_attempt -> { SyncRetryExec<Policy, HookChain<BA, Hook>, AA, OX, F, C, T, E> },
    after_attempt -> { SyncRetryExec<Policy, BA, HookChain<AA, Hook>, OX, F, C, T, E> },
    on_exit -> { SyncRetryExec<Policy, BA, AA, HookChain<OX, Hook>, F, C, T, E> },
}

impl<S, W, P> RetryPolicy<S, W, P>
where
    S: Stop,
    W: Wait,
{
    /// Creates a synchronous retry execution for the given operation.
    #[must_use]
    pub fn retry<T, E, F>(&self, op: F) -> SyncRetry<'_, S, W, P, (), (), (), F, SystemClock, T, E>
    where
        F: FnMut(RetryState) -> Result<T, E>,
    {
        SyncRetryExec::new(self, ExecutionHooks::new(), op, SystemClock)
    }
}
