use core::fmt;

#[cfg(feature = "alloc")]
use super::super::HookChain;
use super::super::execution::sync_exec::{NoSyncSleep, SyncRetryCore, SyncSleep};
use super::super::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, RetryPolicy};
use crate::cancel::{Canceler, NeverCancel};
use crate::compat::Duration;
use crate::predicate::Predicate;
use crate::state::{AttemptState, BeforeAttemptState, ExitState};
use crate::{
    RetryError, RetryStats, on,
    stop::{self, Stop},
    wait::{self, Wait},
};

/// Extension trait to start sync retries directly from a closure/function.
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
    /// use tenacious::RetryExt;
    ///
    /// let _ = (|| Ok::<(), &str>(()))
    ///     .retry()
    ///     .sleep(|_| {})
    ///     .call();
    /// ```
    fn retry(self) -> DefaultSyncRetryBuilder<Self, T, E>;
}

impl<T, E, F> RetryExt<T, E> for F
where
    F: FnMut() -> Result<T, E> + Sized,
{
    fn retry(self) -> DefaultSyncRetryBuilder<Self, T, E> {
        SyncRetryBuilder {
            inner: SyncRetryCore::new(
                RetryPolicy::default(),
                ExecutionHooks::new(),
                self,
                NoSyncSleep,
                NeverCancel,
                true,
            ),
        }
    }
}

/// Alias for the default owned sync retry builder returned by [`RetryExt::retry`].
///
/// This hides the default stop, wait, predicate, hook, sleeper, and canceler
/// state from user-facing type signatures.
pub type DefaultSyncRetryBuilder<F, T, E> = SyncRetryBuilder<
    stop::StopAfterAttempts,
    wait::WaitExponential,
    on::AnyError,
    (),
    (),
    (),
    F,
    NoSyncSleep,
    T,
    E,
    NeverCancel,
>;

/// Alias for the owned sync retry builder returned by
/// [`RetryPolicy::retry_clone`].
///
/// This keeps the policy strategy types visible while hiding the initial hook,
/// sleeper, and canceler plumbing from user-facing type signatures.
pub type PolicySyncRetryBuilder<S, W, P, F, T, E> =
    SyncRetryBuilder<S, W, P, (), (), (), F, NoSyncSleep, T, E, NeverCancel>;

impl<S, W, P> RetryPolicy<S, W, P>
where
    S: Stop,
    W: Wait,
    Self: Clone,
{
    /// Starts an owned sync retry builder by cloning this policy.
    ///
    /// This is the mutability-free counterpart to [`RetryPolicy::retry`]. It
    /// is useful when you keep a shared template policy and want each
    /// execution to own an independent copy of the stop, wait, and predicate
    /// state.
    ///
    /// In `std` builds, calling `.sleep(...)` remains optional. In non-`std`
    /// builds, configure `.sleep(...)` before `.call()`.
    ///
    /// ```
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let template = RetryPolicy::new().stop(stop::attempts(2));
    /// let result = template
    ///     .retry_clone(|| Err::<(), _>("fail"))
    ///     .sleep(|_dur| {})
    ///     .call();
    ///
    /// assert!(result.is_err());
    /// ```
    #[must_use]
    pub fn retry_clone<T, E, F>(&self, op: F) -> PolicySyncRetryBuilder<S, W, P, F, T, E>
    where
        F: FnMut() -> Result<T, E>,
    {
        SyncRetryBuilder {
            inner: SyncRetryCore::new(
                self.clone(),
                ExecutionHooks::new(),
                op,
                NoSyncSleep,
                NeverCancel,
                true,
            ),
        }
    }
}

#[cfg(not(feature = "std"))]
#[doc(hidden)]
/// ```compile_fail
/// use tenacious::RetryExt;
///
/// let _ = (|| Err::<(), &str>("fail"))
///     .retry()
///     .call();
/// ```
#[allow(dead_code)]
fn _sync_retry_builder_requires_sleep_in_no_std() {}

/// Owned sync retry builder created from [`RetryExt::retry`].
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use tenacious::{RetryExt, stop};
///
/// let retry = (|| Ok::<u32, &str>(1))
///     .retry()
///     .stop(stop::attempts(2))
///     .sleep(|_dur: Duration| {});
///
/// let _ = retry;
/// ```
#[allow(clippy::type_complexity)]
pub struct SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C = NeverCancel> {
    inner: SyncRetryCore<RetryPolicy<S, W, P>, BA, AA, OX, F, SleepFn, T, E, C>,
}

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C> fmt::Debug
    for SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRetryBuilder").finish_non_exhaustive()
    }
}

/// Owned sync retry builder wrapper that returns statistics.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use tenacious::RetryExt;
///
/// let retry = (|| Ok::<u32, &str>(1))
///     .retry()
///     .sleep(|_dur: Duration| {})
///     .with_stats();
///
/// let _ = retry;
/// ```
pub struct SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E, C = NeverCancel> {
    inner: SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>,
}

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C> fmt::Debug
    for SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRetryBuilderWithStats")
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "alloc")]
type SyncBuilderWithBeforeHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetryBuilder<S, W, P, HookChain<BA, Hook>, AA, OX, F, SleepFn, T, E, C>;

#[cfg(feature = "alloc")]
type SyncBuilderWithAfterHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetryBuilder<S, W, P, BA, HookChain<AA, Hook>, OX, F, SleepFn, T, E, C>;

#[cfg(feature = "alloc")]
type SyncBuilderWithOnExitHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> =
    SyncRetryBuilder<S, W, P, BA, AA, HookChain<OX, Hook>, F, SleepFn, T, E, C>;

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
{
    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> SyncRetryBuilder<S, W, P, NewBA, NewAA, NewOX, F, SleepFn, T, E, C> {
        SyncRetryBuilder {
            inner: self.inner.map_hooks(map),
        }
    }

    /// Replaces the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(
        self,
        stop: NewStop,
    ) -> SyncRetryBuilder<NewStop, W, P, BA, AA, OX, F, SleepFn, T, E, C> {
        SyncRetryBuilder {
            inner: self.inner.map_policy(|policy| policy.stop(stop)),
        }
    }

    /// Replaces the wait strategy.
    #[must_use]
    pub fn wait<NewWait>(
        self,
        wait: NewWait,
    ) -> SyncRetryBuilder<S, NewWait, P, BA, AA, OX, F, SleepFn, T, E, C> {
        SyncRetryBuilder {
            inner: self.inner.map_policy(|policy| policy.wait(wait)),
        }
    }

    /// Replaces the retry predicate.
    #[must_use]
    pub fn when<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> SyncRetryBuilder<S, W, NewPredicate, BA, AA, OX, F, SleepFn, T, E, C> {
        SyncRetryBuilder {
            inner: self.inner.map_policy(|policy| policy.when(predicate)),
        }
    }

    /// Configures a custom elapsed clock for elapsed-based stop conditions.
    #[must_use]
    pub fn elapsed_clock(self, clock: fn() -> Duration) -> Self {
        SyncRetryBuilder {
            inner: self.inner.map_policy(|policy| policy.elapsed_clock(clock)),
        }
    }

    /// Sets the blocking sleep implementation.
    #[must_use]
    pub fn sleep<NewSleep>(
        self,
        sleeper: NewSleep,
    ) -> SyncRetryBuilder<S, W, P, BA, AA, OX, F, NewSleep, T, E, C> {
        SyncRetryBuilder {
            inner: self.inner.with_sleeper(sleeper),
        }
    }
}

impl_alloc_hook_chain! {
    impl[S, W, P, BA, AA, OX, F, SleepFn, T, E, C]
    SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C> =>
    before_attempt -> { SyncBuilderWithBeforeHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> },
    after_attempt -> { SyncBuilderWithAfterHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> },
    on_exit -> { SyncBuilderWithOnExitHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, C, Hook> },
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, AA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, (), AA, OX, F, SleepFn, T, E, C>
{
    /// Sets the sole before-attempt hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryExt, stop};
    ///
    /// let _ = (|| Err::<(), _>("fail"))
    ///     .retry()
    ///     .stop(stop::attempts(1))
    ///     .before_attempt(|_state| {})
    ///     .before_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryBuilder<S, W, P, Hook, AA, OX, F, SleepFn, T, E, C>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, (), OX, F, SleepFn, T, E, C>
{
    /// Sets the sole after-attempt hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryExt, stop};
    ///
    /// let _ = (|| Err::<(), _>("fail"))
    ///     .retry()
    ///     .stop(stop::attempts(1))
    ///     .after_attempt(|_state| {})
    ///     .after_attempt(|_state| {});
    /// ```
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryBuilder<S, W, P, BA, Hook, OX, F, SleepFn, T, E, C>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, (), F, SleepFn, T, E, C>
{
    /// Sets the sole on-exit hook (no-alloc mode).
    ///
    /// ```compile_fail
    /// use tenacious::{RetryExt, stop};
    ///
    /// let _ = (|| Err::<(), _>("fail"))
    ///     .retry()
    ///     .stop(stop::attempts(1))
    ///     .on_exit(|_state| {})
    ///     .on_exit(|_state| {});
    /// ```
    #[must_use]
    pub fn on_exit<Hook>(
        self,
        hook: Hook,
    ) -> SyncRetryBuilder<S, W, P, BA, AA, Hook, F, SleepFn, T, E, C>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_on_exit(hook))
    }
}

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E>
    SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, NeverCancel>
{
    /// Attaches a canceler that is checked before each attempt and after each sleep.
    #[must_use]
    pub fn cancel_on<NewC: Canceler>(
        self,
        canceler: NewC,
    ) -> SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, NewC> {
        SyncRetryBuilder {
            inner: self.inner.with_canceler(canceler),
        }
    }
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
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
    /// Executes the sync retry loop.
    pub fn call(self) -> Result<T, RetryError<E, T>> {
        self.execute::<false>().0
    }

    /// Executes the sync retry loop and returns aggregate statistics.
    #[must_use]
    pub fn with_stats(self) -> SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E, C> {
        SyncRetryBuilderWithStats { inner: self }
    }

    fn execute<const COLLECT_STATS: bool>(
        self,
    ) -> (Result<T, RetryError<E, T>>, Option<RetryStats>) {
        self.inner.execute::<S, W, P, COLLECT_STATS>()
    }
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
    SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E, C>
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
    /// Executes the sync retry loop and returns `(result, stats)`.
    pub fn call(self) -> (Result<T, RetryError<E, T>>, RetryStats) {
        let (result, stats) = self.inner.execute::<true>();
        (
            result,
            stats.expect("sync retry builder completed without stats"),
        )
    }
}
