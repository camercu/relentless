use core::marker::PhantomData;

use super::common::{AttemptTransition, process_attempt_transition};
use super::time::ElapsedTracker;
use super::*;

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
/// Created by [`RetryPolicy::retry`]. Call `.sleep(...)` to provide a sleep
/// implementation and `.call()` to execute.
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
/// let retry = policy.retry(|| Err::<(), _>("fail")).sleep(|_dur| {});
/// let _ = retry.call();
/// ```
pub struct SyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, SleepFn, T, E> {
    policy: &'policy mut RetryPolicy<S, W, P, BA, AA, BS, OE>,
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
pub struct SyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OE, F, SleepFn, T, E> {
    inner: SyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, SleepFn, T, E>,
}

impl<'policy, S, W, P, BA, AA, BS, OE, F, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, NoSyncSleep, T, E>
where
    S: Stop,
    W: Wait,
    BA: BeforeAttemptHook,
    F: FnMut() -> Result<T, E>,
{
    /// Sets a custom blocking sleep function.
    #[must_use]
    pub fn sleep<SleepFn>(
        self,
        sleeper: SleepFn,
    ) -> SyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, SleepFn, T, E> {
        SyncRetry {
            policy: self.policy,
            op: self.op,
            sleeper,
            _marker: PhantomData,
        }
    }
}

impl<'policy, S, W, P, BA, AA, BS, OE, F, SleepFn, T, E>
    SyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, SleepFn, T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OE: AttemptHook<T, E>,
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
    ) -> SyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OE, F, SleepFn, T, E> {
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
            self.policy.hooks.before_attempt.call(&before_state);

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

impl<'policy, S, W, P, BA, AA, BS, OE, F, SleepFn, T, E>
    SyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OE, F, SleepFn, T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OE: AttemptHook<T, E>,
    F: FnMut() -> Result<T, E>,
    SleepFn: SyncSleep,
{
    /// Executes the retry loop synchronously and returns `(result, stats)`.
    pub fn call(self) -> (Result<T, RetryError<E, T>>, RetryStats) {
        let (result, stats) = self.inner.execute::<true>();
        (result, stats.expect("sync retry completed without stats"))
    }
}

impl<S, W, P, BA, AA, BS, OE> RetryPolicy<S, W, P, BA, AA, BS, OE>
where
    S: Stop,
    W: Wait,
    BA: BeforeAttemptHook,
{
    /// Begins configuring sync retry execution.
    #[must_use]
    pub fn retry<T, E, F>(
        &mut self,
        op: F,
    ) -> SyncRetry<'_, S, W, P, BA, AA, BS, OE, F, NoSyncSleep, T, E>
    where
        F: FnMut() -> Result<T, E>,
    {
        self.stop.reset();
        self.wait.reset();
        SyncRetry {
            policy: self,
            op,
            sleeper: NoSyncSleep,
            _marker: PhantomData,
        }
    }
}
