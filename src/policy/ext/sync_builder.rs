use core::fmt;

#[cfg(feature = "alloc")]
use super::super::HookChain;
use super::super::execution::sync_exec::{NoSyncSleep, SyncRetryCore, SyncSleep};
use super::super::time::ElapsedTracker;
use super::super::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, RetryPolicy};
use crate::compat::Duration;
use crate::predicate::Predicate;
use crate::state::{AttemptState, ExitState, RetryState};
use crate::{
    RetryError, RetryStats, predicate,
    stop::{self, Stop},
    wait::{self, Wait},
};

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

impl<T, E, F: FnMut() -> Result<T, E>> super::super::execution::common::RetryOp<T, E>
    for StatelessOp<F>
{
    fn call_op(&mut self, _state: RetryState) -> Result<T, E> {
        (self.0)()
    }
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
                StatelessOp(self),
                NoSyncSleep,
                ElapsedTracker::new(None),
            ),
        }
    }
}

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

/// Owned sync retry builder created from [`RetryExt::retry`].
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
#[allow(clippy::type_complexity)]
pub struct SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E> {
    inner: SyncRetryCore<RetryPolicy<S, W, P>, BA, AA, OX, F, SleepFn, T, E>,
}

impl<S, W, P, F, T, E> SyncRetryBuilder<S, W, P, (), (), (), F, NoSyncSleep, T, E> {
    pub(crate) fn from_policy(policy: RetryPolicy<S, W, P>, op: F) -> Self {
        SyncRetryBuilder {
            inner: SyncRetryCore::new(
                policy,
                ExecutionHooks::new(),
                op,
                NoSyncSleep,
                ElapsedTracker::new(None),
            ),
        }
    }
}

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E> fmt::Debug
    for SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E>
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
/// use relentless::RetryExt;
///
/// let retry = (|| Ok::<u32, &str>(1))
///     .retry()
///     .sleep(|_dur: Duration| {})
///     .with_stats();
///
/// let _ = retry;
/// ```
pub struct SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E> {
    inner: SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E>,
}

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E> fmt::Debug
    for SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRetryBuilderWithStats")
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "alloc")]
type SyncBuilderWithBeforeHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> =
    SyncRetryBuilder<S, W, P, HookChain<BA, Hook>, AA, OX, F, SleepFn, T, E>;

#[cfg(feature = "alloc")]
type SyncBuilderWithAfterHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> =
    SyncRetryBuilder<S, W, P, BA, HookChain<AA, Hook>, OX, F, SleepFn, T, E>;

#[cfg(feature = "alloc")]
type SyncBuilderWithOnExitHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> =
    SyncRetryBuilder<S, W, P, BA, AA, HookChain<OX, Hook>, F, SleepFn, T, E>;

impl<S, W, P, BA, AA, OX, F, SleepFn, T, E>
    SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E>
{
    fn map_hooks<NewBA, NewAA, NewOX>(
        self,
        map: impl FnOnce(ExecutionHooks<BA, AA, OX>) -> ExecutionHooks<NewBA, NewAA, NewOX>,
    ) -> SyncRetryBuilder<S, W, P, NewBA, NewAA, NewOX, F, SleepFn, T, E> {
        SyncRetryBuilder {
            inner: self.inner.map_hooks(map),
        }
    }

    #[must_use]
    pub fn stop<NewStop>(
        self,
        stop: NewStop,
    ) -> SyncRetryBuilder<NewStop, W, P, BA, AA, OX, F, SleepFn, T, E> {
        SyncRetryBuilder {
            inner: self.inner.map_policy(|policy| policy.stop(stop)),
        }
    }

    #[must_use]
    pub fn wait<NewWait>(
        self,
        wait: NewWait,
    ) -> SyncRetryBuilder<S, NewWait, P, BA, AA, OX, F, SleepFn, T, E> {
        SyncRetryBuilder {
            inner: self.inner.map_policy(|policy| policy.wait(wait)),
        }
    }

    #[must_use]
    pub fn when<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> SyncRetryBuilder<S, W, NewPredicate, BA, AA, OX, F, SleepFn, T, E> {
        SyncRetryBuilder {
            inner: self.inner.map_policy(|policy| policy.when(predicate)),
        }
    }

    /// Sets a predicate that retries *until* `p.should_retry()` returns `true`.
    ///
    /// Wraps `p` in [`PredicateUntil`](crate::predicate::PredicateUntil).
    /// Natural for polling: `.until(ok(|s| s.is_ready()))`.
    #[must_use]
    pub fn until<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> SyncRetryBuilder<S, W, predicate::PredicateUntil<NewPredicate>, BA, AA, OX, F, SleepFn, T, E>
    {
        SyncRetryBuilder {
            inner: self.inner.map_policy(|policy| policy.until(predicate)),
        }
    }

    /// Configures a custom elapsed clock for elapsed-based stop conditions.
    #[must_use]
    pub fn elapsed_clock(self, clock: fn() -> Duration) -> Self {
        SyncRetryBuilder {
            inner: self.inner.set_elapsed_clock(clock),
        }
    }

    /// Configures a custom elapsed clock from a boxed closure.
    ///
    /// This variant supports closures with captures for test clocks and
    /// runtime state. Requires the `alloc` feature.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn elapsed_clock_fn(self, clock: impl Fn() -> Duration + 'static) -> Self {
        SyncRetryBuilder {
            inner: self
                .inner
                .set_elapsed_clock_fn(crate::compat::Box::new(clock)),
        }
    }

    /// Sets a wall-clock deadline for the entire retry execution.
    ///
    /// Timeout combines two behaviors:
    /// 1. Implicitly ORs `stop::elapsed(dur)` into the effective stop strategy.
    /// 2. Clamps each sleep delay to the remaining budget.
    ///
    /// With `std`, the Instant clock is used automatically. Without `std`,
    /// requires `.elapsed_clock()` or `.elapsed_clock_fn()` to be configured;
    /// otherwise timeout has no effect.
    #[must_use]
    pub fn timeout(self, dur: Duration) -> Self {
        SyncRetryBuilder {
            inner: self.inner.set_timeout(dur),
        }
    }

    #[must_use]
    pub fn sleep<NewSleep>(
        self,
        sleeper: NewSleep,
    ) -> SyncRetryBuilder<S, W, P, BA, AA, OX, F, NewSleep, T, E> {
        SyncRetryBuilder {
            inner: self.inner.with_sleeper(sleeper),
        }
    }
}

impl_alloc_hook_chain! {
    impl[S, W, P, BA, AA, OX, F, SleepFn, T, E]
    SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E> =>
    before_attempt -> { SyncBuilderWithBeforeHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> },
    after_attempt -> { SyncBuilderWithAfterHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> },
    on_exit -> { SyncBuilderWithOnExitHook<S, W, P, BA, AA, OX, F, SleepFn, T, E, Hook> },
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, AA, OX, F, SleepFn, T, E> SyncRetryBuilder<S, W, P, (), AA, OX, F, SleepFn, T, E> {
    /// Sets the before-attempt hook.
    ///
    /// Without `alloc`, only one hook per slot is supported; calling this
    /// twice is a compile error.
    ///
    /// ```compile_fail
    /// use relentless::{RetryExt, stop};
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
    ) -> SyncRetryBuilder<S, W, P, Hook, AA, OX, F, SleepFn, T, E>
    where
        Hook: FnMut(&RetryState),
    {
        self.map_hooks(|hooks| hooks.set_before_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, OX, F, SleepFn, T, E> SyncRetryBuilder<S, W, P, BA, (), OX, F, SleepFn, T, E> {
    /// Sets the after-attempt hook.
    ///
    /// Without `alloc`, only one hook per slot is supported; calling this
    /// twice is a compile error.
    ///
    /// ```compile_fail
    /// use relentless::{RetryExt, stop};
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
    ) -> SyncRetryBuilder<S, W, P, BA, Hook, OX, F, SleepFn, T, E>
    where
        Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_after_attempt(hook))
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, F, SleepFn, T, E> SyncRetryBuilder<S, W, P, BA, AA, (), F, SleepFn, T, E> {
    /// Sets the on-exit hook.
    ///
    /// Without `alloc`, only one hook per slot is supported; calling this
    /// twice is a compile error.
    ///
    /// ```compile_fail
    /// use relentless::{RetryExt, stop};
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
    ) -> SyncRetryBuilder<S, W, P, BA, AA, Hook, F, SleepFn, T, E>
    where
        Hook: for<'a> FnMut(&ExitState<'a, T, E>),
    {
        self.map_hooks(|hooks| hooks.set_on_exit(hook))
    }
}

use super::super::execution::common::RetryOp;

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E> SyncRetryBuilder<S, W, P, BA, AA, OX, F, SleepFn, T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: RetryOp<T, E>,
    SleepFn: SyncSleep,
{
    pub fn call(self) -> Result<T, RetryError<T, E>> {
        self.execute::<false>().0
    }

    /// Wraps this builder to also return [`RetryStats`] on completion.
    ///
    /// Does not execute the retry loop; call `.call()` on the returned wrapper.
    #[must_use]
    pub fn with_stats(self) -> SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E> {
        SyncRetryBuilderWithStats { inner: self }
    }

    fn execute<const COLLECT_STATS: bool>(
        self,
    ) -> (Result<T, RetryError<T, E>>, Option<RetryStats>) {
        self.inner.execute::<S, W, P, COLLECT_STATS>()
    }
}

#[allow(private_bounds)]
impl<S, W, P, BA, AA, OX, F, SleepFn, T, E>
    SyncRetryBuilderWithStats<S, W, P, BA, AA, OX, F, SleepFn, T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: RetryOp<T, E>,
    SleepFn: SyncSleep,
{
    pub fn call(self) -> (Result<T, RetryError<T, E>>, RetryStats) {
        let (result, stats) = self.inner.execute::<true>();
        (
            result,
            stats.expect("sync retry builder completed without stats"),
        )
    }
}
