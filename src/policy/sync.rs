use core::marker::PhantomData;

use super::common::{AttemptTransition, process_attempt_transition};
use super::time::ElapsedTracker;
use super::*;
use crate::error::RetryError;
use crate::state::{AttemptState, BeforeAttemptState, ExitState, RetryState};
use crate::stats::RetryStats;

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
pub struct SyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E> {
    policy: &'policy mut RetryPolicy<S, W, P>,
    hooks: ExecutionHooks<BA, AA, BS, OX>,
    op: F,
    sleeper: SleepFn,
    _marker: PhantomData<fn() -> (T, E)>,
}

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
pub struct SyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E> {
    inner: SyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E>,
}

#[cfg(feature = "alloc")]
type SyncRetryWithBeforeHook<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, Hook> =
    SyncRetry<'policy, S, W, P, HookChain<BA, Hook>, AA, BS, OX, F, SleepFn, T, E>;

#[cfg(feature = "alloc")]
type SyncRetryWithAfterHook<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, Hook> =
    SyncRetry<'policy, S, W, P, BA, HookChain<AA, Hook>, BS, OX, F, SleepFn, T, E>;

#[cfg(feature = "alloc")]
type SyncRetryWithBeforeSleepHook<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, Hook> =
    SyncRetry<'policy, S, W, P, BA, AA, HookChain<BS, Hook>, OX, F, SleepFn, T, E>;

#[cfg(feature = "alloc")]
type SyncRetryWithOnExitHook<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, Hook> =
    SyncRetry<'policy, S, W, P, BA, AA, BS, HookChain<OX, Hook>, F, SleepFn, T, E>;

impl<'policy, S, W, P, BA, AA, BS, OX, F, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, NoSyncSleep, T, E>
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
    ) -> SyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E> {
        SyncRetry {
            policy: self.policy,
            hooks: self.hooks,
            op: self.op,
            sleeper,
            _marker: PhantomData,
        }
    }
}

impl<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Result<T, E>,
    SleepFn: SyncSleep,
{
    /// Executes the retry loop synchronously.
    pub fn call(self) -> Result<T, RetryError<E, T>> {
        self.execute::<false>().0
    }

    /// Executes the retry loop and returns aggregate statistics.
    #[must_use]
    pub fn with_stats(
        self,
    ) -> SyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E> {
        SyncRetryWithStats { inner: self }
    }

    fn execute<const COLLECT_STATS: bool>(
        mut self,
    ) -> (Result<T, RetryError<E, T>>, Option<RetryStats>) {
        let mut attempt: u32 = 1;
        let mut total_wait = Duration::ZERO;
        let elapsed_tracker = ElapsedTracker::new(self.policy.meta.elapsed_clock);

        loop {
            let elapsed_before_attempt = elapsed_tracker.elapsed();
            let before_state = BeforeAttemptState {
                attempt,
                elapsed: elapsed_before_attempt,
                total_wait,
            };
            self.hooks.before_attempt.call(&before_state);

            let outcome = (self.op)();
            let elapsed_after_attempt = elapsed_tracker.elapsed();
            let retry_state = RetryState {
                attempt,
                elapsed: elapsed_after_attempt,
                next_delay: Duration::ZERO,
                total_wait,
            };

            match process_attempt_transition(
                self.policy,
                &mut self.hooks,
                outcome,
                retry_state,
                COLLECT_STATS,
                total_wait,
            ) {
                AttemptTransition::Finished { result, stats } => return (result, stats),
                AttemptTransition::Sleep { next_delay } => {
                    self.sleeper.sleep(next_delay);
                    total_wait = total_wait.saturating_add(next_delay);
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }
}

impl<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E>
    SyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Result<T, E>,
    SleepFn: SyncSleep,
{
    /// Executes the retry loop synchronously and returns `(result, stats)`.
    pub fn call(self) -> (Result<T, RetryError<E, T>>, RetryStats) {
        let (result, stats) = self.inner.execute::<true>();
        (result, stats.expect("sync retry completed without stats"))
    }
}

#[cfg(feature = "alloc")]
impl<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E>
{
    /// Appends a before-attempt hook.
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryWithBeforeHook<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, Hook>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        let ExecutionHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exit,
        } = self.hooks;
        SyncRetry {
            policy: self.policy,
            hooks: ExecutionHooks {
                before_attempt: HookChain::new(before_attempt, hook),
                after_attempt,
                before_sleep,
                on_exit,
            },
            op: self.op,
            sleeper: self.sleeper,
            _marker: PhantomData,
        }
    }

    /// Appends an after-attempt hook.
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryWithAfterHook<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, Hook>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        let ExecutionHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exit,
        } = self.hooks;
        SyncRetry {
            policy: self.policy,
            hooks: ExecutionHooks {
                before_attempt,
                after_attempt: HookChain::new(after_attempt, hook),
                before_sleep,
                on_exit,
            },
            op: self.op,
            sleeper: self.sleeper,
            _marker: PhantomData,
        }
    }

    /// Appends a before-sleep hook.
    #[must_use]
    pub fn before_sleep<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryWithBeforeSleepHook<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, Hook>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        let ExecutionHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exit,
        } = self.hooks;
        SyncRetry {
            policy: self.policy,
            hooks: ExecutionHooks {
                before_attempt,
                after_attempt,
                before_sleep: HookChain::new(before_sleep, hook),
                on_exit,
            },
            op: self.op,
            sleeper: self.sleeper,
            _marker: PhantomData,
        }
    }

    /// Appends an on-exit hook.
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryWithOnExitHook<'policy, S, W, P, BA, AA, BS, OX, F, SleepFn, T, E, Hook>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        let ExecutionHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exit,
        } = self.hooks;
        SyncRetry {
            policy: self.policy,
            hooks: ExecutionHooks {
                before_attempt,
                after_attempt,
                before_sleep,
                on_exit: HookChain::new(on_exit, hook),
            },
            op: self.op,
            sleeper: self.sleeper,
            _marker: PhantomData,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, AA, BS, OX, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, (), AA, BS, OX, F, SleepFn, T, E>
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
    ) -> SyncRetry<'policy, S, W, P, Hook, AA, BS, OX, F, SleepFn, T, E>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        let ExecutionHooks {
            after_attempt,
            before_sleep,
            on_exit,
            ..
        } = self.hooks;
        SyncRetry {
            policy: self.policy,
            hooks: ExecutionHooks {
                before_attempt: hook,
                after_attempt,
                before_sleep,
                on_exit,
            },
            op: self.op,
            sleeper: self.sleeper,
            _marker: PhantomData,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, BS, OX, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, (), BS, OX, F, SleepFn, T, E>
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
    ) -> SyncRetry<'policy, S, W, P, BA, Hook, BS, OX, F, SleepFn, T, E>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        let ExecutionHooks {
            before_attempt,
            before_sleep,
            on_exit,
            ..
        } = self.hooks;
        SyncRetry {
            policy: self.policy,
            hooks: ExecutionHooks {
                before_attempt,
                after_attempt: hook,
                before_sleep,
                on_exit,
            },
            op: self.op,
            sleeper: self.sleeper,
            _marker: PhantomData,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, AA, OX, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, (), OX, F, SleepFn, T, E>
{
    /// Sets the sole before-sleep hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    /// let _ = policy
    ///     .retry(|| Err::<(), _>("fail"))
    ///     .before_sleep(|_state| {})
    ///     .before_sleep(|_state| {});
    /// ```
    #[must_use]
    pub fn before_sleep<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetry<'policy, S, W, P, BA, AA, Hook, OX, F, SleepFn, T, E>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        let ExecutionHooks {
            before_attempt,
            after_attempt,
            on_exit,
            ..
        } = self.hooks;
        SyncRetry {
            policy: self.policy,
            hooks: ExecutionHooks {
                before_attempt,
                after_attempt,
                before_sleep: hook,
                on_exit,
            },
            op: self.op,
            sleeper: self.sleeper,
            _marker: PhantomData,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<'policy, S, W, P, BA, AA, BS, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, BS, (), F, SleepFn, T, E>
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
    ) -> SyncRetry<'policy, S, W, P, BA, AA, BS, Hook, F, SleepFn, T, E>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        let ExecutionHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            ..
        } = self.hooks;
        SyncRetry {
            policy: self.policy,
            hooks: ExecutionHooks {
                before_attempt,
                after_attempt,
                before_sleep,
                on_exit: hook,
            },
            op: self.op,
            sleeper: self.sleeper,
            _marker: PhantomData,
        }
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
    ) -> SyncRetry<'_, S, W, P, (), (), (), (), F, NoSyncSleep, T, E>
    where
        F: FnMut() -> Result<T, E>,
    {
        self.stop.reset();
        self.wait.reset();
        SyncRetry {
            policy: self,
            hooks: ExecutionHooks::new(),
            op,
            sleeper: NoSyncSleep,
            _marker: PhantomData,
        }
    }
}
