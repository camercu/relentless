//! RetryPolicy builder and sync/async execution engines.

use crate::compat::Duration;
use crate::error::RetryError;
use crate::on;
use crate::predicate::Predicate;
use crate::state::{AttemptState, BeforeAttemptState, RetryState};
use crate::stats::{RetryStats, StopReason};
use crate::stop::{self, Stop};
use crate::wait::{self, Wait};
use core::marker::PhantomData;
#[cfg(feature = "serde")]
use serde::ser::SerializeStruct;

#[cfg(feature = "alloc")]
use crate::compat::Box;
#[cfg(feature = "alloc")]
use crate::sleep::Sleeper;
#[cfg(feature = "alloc")]
use core::future::Future;
#[cfg(feature = "alloc")]
use core::pin::Pin;
#[cfg(feature = "alloc")]
use core::task::{Context, Poll};
#[cfg(feature = "std")]
use std::time::Instant;

/// Hook callback shape for the `before_attempt` hook.
#[doc(hidden)]
pub trait BeforeAttemptHook {
    /// Invokes the hook.
    fn call(&mut self, state: &BeforeAttemptState);
}

impl BeforeAttemptHook for () {
    fn call(&mut self, _state: &BeforeAttemptState) {}
}

impl<F> BeforeAttemptHook for F
where
    F: FnMut(&BeforeAttemptState),
{
    fn call(&mut self, state: &BeforeAttemptState) {
        (self)(state);
    }
}

/// Hook callback shape for hooks receiving an [`AttemptState`].
#[doc(hidden)]
pub trait AttemptHook<T, E> {
    /// Invokes the hook.
    fn call(&mut self, state: &AttemptState<'_, T, E>);
}

impl<T, E> AttemptHook<T, E> for () {
    fn call(&mut self, _state: &AttemptState<'_, T, E>) {}
}

impl<T, E, F> AttemptHook<T, E> for F
where
    F: for<'a> FnMut(&AttemptState<'a, T, E>),
{
    fn call(&mut self, state: &AttemptState<'_, T, E>) {
        (self)(state);
    }
}

/// Internal hook-chain wrapper used when multiple hooks are appended.
#[doc(hidden)]
#[derive(Clone)]
pub struct HookChain<First, Second> {
    first: First,
    second: Second,
}

#[cfg(feature = "alloc")]
impl<First, Second> HookChain<First, Second> {
    fn new(first: First, second: Second) -> Self {
        Self { first, second }
    }
}

impl<First, Second> BeforeAttemptHook for HookChain<First, Second>
where
    First: BeforeAttemptHook,
    Second: BeforeAttemptHook,
{
    fn call(&mut self, state: &BeforeAttemptState) {
        self.first.call(state);
        self.second.call(state);
    }
}

impl<T, E, First, Second> AttemptHook<T, E> for HookChain<First, Second>
where
    First: AttemptHook<T, E>,
    Second: AttemptHook<T, E>,
{
    fn call(&mut self, state: &AttemptState<'_, T, E>) {
        self.first.call(state);
        self.second.call(state);
    }
}

/// Reusable retry configuration.
///
/// `RetryPolicy` stores retry strategies and hooks, and can be reused across
/// multiple operations.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryPolicy, stop, wait};
/// use core::time::Duration;
///
/// let mut policy = RetryPolicy::new()
///     .stop(stop::attempts(3))
///     .wait(wait::fixed(Duration::from_millis(5)));
///
/// let _ = policy.retry(|| Err::<(), _>("fail")).sleep(|_dur| {}).call();
/// ```
#[derive(Clone)]
pub struct RetryPolicy<
    S = stop::StopNever,
    W = wait::WaitFixed,
    P = on::AnyError,
    BA = (),
    AA = (),
    BS = (),
    OE = (),
> {
    stop: S,
    wait: W,
    predicate: P,
    before_attempt: BA,
    after_attempt: AA,
    before_sleep: BS,
    on_exhausted: OE,
    predicate_is_default: bool,
}

#[cfg(feature = "serde")]
impl<S, W, P, BA, AA, BS, OE> serde::Serialize for RetryPolicy<S, W, P, BA, AA, BS, OE>
where
    S: serde::Serialize,
    W: serde::Serialize,
    P: serde::Serialize,
{
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        // Hooks are intentionally omitted from serialized output.
        let mut state = serializer.serialize_struct("RetryPolicy", 4)?;
        state.serialize_field("stop", &self.stop)?;
        state.serialize_field("wait", &self.wait)?;
        state.serialize_field("predicate", &self.predicate)?;
        state.serialize_field("predicate_is_default", &self.predicate_is_default)?;
        state.end()
    }
}

#[cfg(feature = "serde")]
impl<'de, S, W, P> serde::Deserialize<'de> for RetryPolicy<S, W, P, (), (), (), ()>
where
    S: serde::Deserialize<'de>,
    W: serde::Deserialize<'de>,
    P: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct SerializedRetryPolicy<S, W, P> {
            stop: S,
            wait: W,
            predicate: P,
            #[serde(default)]
            predicate_is_default: bool,
        }

        let serialized = SerializedRetryPolicy::deserialize(deserializer)?;
        Ok(Self {
            stop: serialized.stop,
            wait: serialized.wait,
            predicate: serialized.predicate,
            before_attempt: (),
            after_attempt: (),
            before_sleep: (),
            on_exhausted: (),
            predicate_is_default: serialized.predicate_is_default,
        })
    }
}

/// Type-erased retry policy for runtime-configured storage.
///
/// This alias is available when `alloc` is enabled and erases stop/wait/predicate
/// concrete types into trait objects while preserving hook types.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryPolicy, BoxedRetryPolicy, stop};
///
/// let _policy: BoxedRetryPolicy<i32, &'static str> =
///     RetryPolicy::new().stop(stop::attempts(3)).boxed();
/// ```
#[cfg(feature = "alloc")]
pub type BoxedRetryPolicy<T, E, BA = (), AA = (), BS = (), OE = ()> =
    RetryPolicy<Box<dyn Stop>, Box<dyn Wait>, Box<dyn Predicate<T, E>>, BA, AA, BS, OE>;

impl RetryPolicy<stop::StopNever, wait::WaitFixed, on::AnyError, (), (), (), ()> {
    /// Creates a policy with `stop::never()`, `wait::fixed(Duration::ZERO)`,
    /// and `on::any_error()`.
    pub fn new() -> Self {
        Self {
            stop: stop::never(),
            wait: wait::fixed(Duration::ZERO),
            predicate: on::any_error(),
            before_attempt: (),
            after_attempt: (),
            before_sleep: (),
            on_exhausted: (),
            predicate_is_default: true,
        }
    }
}

impl Default for RetryPolicy<stop::StopNever, wait::WaitFixed, on::AnyError, (), (), (), ()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, W, P, BA, AA, BS, OE> RetryPolicy<S, W, P, BA, AA, BS, OE> {
    /// Replaces the stop strategy.
    pub fn stop<NewStop>(self, stop: NewStop) -> RetryPolicy<NewStop, W, P, BA, AA, BS, OE> {
        RetryPolicy {
            stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
        }
    }

    /// Replaces the wait strategy.
    pub fn wait<NewWait>(self, wait: NewWait) -> RetryPolicy<S, NewWait, P, BA, AA, BS, OE> {
        RetryPolicy {
            stop: self.stop,
            wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
        }
    }

    /// Replaces the retry predicate.
    pub fn when<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> RetryPolicy<S, W, NewPredicate, BA, AA, BS, OE> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: false,
        }
    }

    /// Converts this policy into a type-erased boxed variant.
    #[cfg(feature = "alloc")]
    pub fn boxed<T, E>(self) -> BoxedRetryPolicy<T, E, BA, AA, BS, OE>
    where
        S: Stop + 'static,
        W: Wait + 'static,
        P: Predicate<T, E> + 'static,
    {
        RetryPolicy {
            stop: Box::new(self.stop),
            wait: Box::new(self.wait),
            predicate: Box::new(self.predicate),
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
        }
    }
}

#[cfg(feature = "alloc")]
impl<S, W, P, BA, AA, BS, OE> RetryPolicy<S, W, P, BA, AA, BS, OE> {
    /// Appends a before-attempt hook.
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, HookChain<BA, Hook>, AA, BS, OE>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: HookChain::new(self.before_attempt, hook),
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
        }
    }

    /// Appends an after-attempt hook.
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, BA, HookChain<AA, Hook>, BS, OE> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: HookChain::new(self.after_attempt, hook),
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
        }
    }

    /// Appends a before-sleep hook.
    pub fn before_sleep<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, BA, AA, HookChain<BS, Hook>, OE> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: HookChain::new(self.before_sleep, hook),
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
        }
    }

    /// Appends an on-exhausted hook.
    pub fn on_exhausted<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, BA, AA, BS, HookChain<OE, Hook>> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: HookChain::new(self.on_exhausted, hook),
            predicate_is_default: self.predicate_is_default,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, AA, BS, OE> RetryPolicy<S, W, P, (), AA, BS, OE> {
    /// Sets the sole before-attempt hook (no-alloc mode).
    pub fn before_attempt<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, Hook, AA, BS, OE>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: hook,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, BS, OE> RetryPolicy<S, W, P, BA, (), BS, OE> {
    /// Sets the sole after-attempt hook (no-alloc mode).
    pub fn after_attempt<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, BA, Hook, BS, OE> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: hook,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, OE> RetryPolicy<S, W, P, BA, AA, (), OE> {
    /// Sets the sole before-sleep hook (no-alloc mode).
    pub fn before_sleep<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, BA, AA, Hook, OE> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: hook,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, BS> RetryPolicy<S, W, P, BA, AA, BS, ()> {
    /// Sets the sole on-exhausted hook (no-alloc mode).
    pub fn on_exhausted<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, BA, AA, BS, Hook> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: hook,
            predicate_is_default: self.predicate_is_default,
        }
    }
}

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

fn attempt_state_from<'a, T, E>(
    retry_state: &RetryState,
    outcome: &'a Result<T, E>,
) -> AttemptState<'a, T, E> {
    AttemptState {
        attempt: retry_state.attempt,
        outcome,
        elapsed: retry_state.elapsed,
        next_delay: retry_state.next_delay,
        total_wait: retry_state.total_wait,
    }
}

/// Sync retry execution object.
///
/// Created by [`RetryPolicy::retry`]. Call `.sleep(...)` to provide a sleep
/// implementation and `.call()` to execute.
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
        #[cfg(feature = "std")]
        let start = Instant::now();

        loop {
            let elapsed_before_attempt = current_elapsed(
                #[cfg(feature = "std")]
                &start,
            );
            let before_state = BeforeAttemptState {
                attempt,
                elapsed: elapsed_before_attempt,
                total_wait,
            };
            self.policy.before_attempt.call(&before_state);

            let outcome = (self.op)();
            let elapsed_after_attempt = current_elapsed(
                #[cfg(feature = "std")]
                &start,
            );
            let mut retry_state = RetryState {
                attempt,
                elapsed: elapsed_after_attempt,
                next_delay: Duration::ZERO,
                total_wait,
            };

            let should_retry = self.policy.predicate.should_retry(&outcome);
            {
                let attempt_state = attempt_state_from(&retry_state, &outcome);
                self.policy.after_attempt.call(&attempt_state);
            }
            if !should_retry {
                let stats = if COLLECT_STATS {
                    Some(RetryStats {
                        attempts: attempt,
                        total_elapsed: retry_state.elapsed,
                        total_wait,
                        stop_reason: stop_reason_for_predicate_accept(
                            &outcome,
                            self.policy.predicate_is_default,
                        ),
                    })
                } else {
                    None
                };

                return (
                    match outcome {
                        Ok(value) => Ok(value),
                        Err(error) => Err(RetryError::Exhausted {
                            error,
                            attempts: attempt,
                            total_elapsed: retry_state.elapsed,
                        }),
                    },
                    stats,
                );
            }

            let next_delay = self.policy.wait.next_wait(&retry_state);
            retry_state.next_delay = next_delay;

            if self.policy.stop.should_stop(&retry_state) {
                {
                    let attempt_state = attempt_state_from(&retry_state, &outcome);
                    self.policy.on_exhausted.call(&attempt_state);
                }
                let stats = if COLLECT_STATS {
                    Some(RetryStats {
                        attempts: attempt,
                        total_elapsed: retry_state.elapsed,
                        total_wait,
                        stop_reason: StopReason::StopCondition,
                    })
                } else {
                    None
                };
                return (
                    match outcome {
                        Err(error) => Err(RetryError::Exhausted {
                            error,
                            attempts: attempt,
                            total_elapsed: retry_state.elapsed,
                        }),
                        Ok(last) => Err(RetryError::ConditionNotMet {
                            last,
                            attempts: attempt,
                            total_elapsed: retry_state.elapsed,
                        }),
                    },
                    stats,
                );
            }
            {
                let attempt_state = attempt_state_from(&retry_state, &outcome);
                self.policy.before_sleep.call(&attempt_state);
            }

            self.sleeper.sleep(next_delay);
            total_wait = total_wait.saturating_add(next_delay);
            attempt = attempt.saturating_add(1);
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

/// Marker for the absence of an explicit async sleep implementation.
///
/// Users must call `.sleep(sleeper)` on [`AsyncRetry`] to provide a concrete
/// [`Sleeper`] before the future can be polled.
#[cfg(feature = "alloc")]
#[doc(hidden)]
pub struct NoAsyncSleep;

#[cfg(feature = "alloc")]
enum AsyncPhase<'policy, Fut> {
    ReadyToStartAttempt,
    PollingOperation(Pin<Box<Fut>>),
    Sleeping(Pin<Box<dyn Future<Output = ()> + 'policy>>),
    Finished,
}

/// Async retry execution object.
///
/// Created by [`RetryPolicy::retry_async`]. Set a sleeper with `.sleep(...)`
/// and then `.await` the returned future.
///
/// # Examples
///
/// ```
/// use tenacious::RetryPolicy;
/// use core::time::Duration;
///
/// let mut policy = RetryPolicy::new();
/// let retry = policy
///     .retry_async(|| async { Ok::<u32, &str>(1) })
///     .sleep(|_dur: Duration| async {});
/// let _ = retry;
/// ```
#[cfg(feature = "alloc")]
pub struct AsyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    policy: &'policy mut RetryPolicy<S, W, P, BA, AA, BS, OE>,
    op: F,
    sleeper: SleepImpl,
    phase: AsyncPhase<'policy, Fut>,
    attempt: u32,
    total_wait: Duration,
    collect_stats: bool,
    final_stats: Option<RetryStats>,
    #[cfg(feature = "std")]
    start: Instant,
    _marker: PhantomData<fn() -> (T, E)>,
}

/// Async retry execution wrapper that returns statistics.
///
/// Created by calling `.with_stats()` on [`AsyncRetry`].
///
/// # Examples
///
/// ```
/// use tenacious::RetryPolicy;
/// use core::time::Duration;
///
/// let mut policy = RetryPolicy::new();
/// let retry = policy
///     .retry_async(|| async { Ok::<u32, &str>(1) })
///     .sleep(|_dur: Duration| async {})
///     .with_stats();
/// let _ = retry;
/// ```
#[cfg(feature = "alloc")]
pub struct AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    inner: AsyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E>,
}

#[cfg(feature = "alloc")]
impl<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the async sleep implementation.
    pub fn sleep<NewSleep>(
        self,
        sleeper: NewSleep,
    ) -> AsyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, Fut, NewSleep, T, E> {
        AsyncRetry {
            policy: self.policy,
            op: self.op,
            sleeper,
            phase: self.phase,
            attempt: self.attempt,
            total_wait: self.total_wait,
            collect_stats: self.collect_stats,
            final_stats: self.final_stats,
            #[cfg(feature = "std")]
            start: self.start,
            _marker: PhantomData,
        }
    }

    /// Wraps this async retry with statistics collection.
    pub fn with_stats(
        self,
    ) -> AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E> {
        let mut inner = self;
        inner.collect_stats = true;
        AsyncRetryWithStats { inner }
    }
}

#[cfg(feature = "alloc")]
impl<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E> Future
    for AsyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OE: AttemptHook<T, E>,
    F: FnMut() -> Fut + Unpin,
    Fut: Future<Output = Result<T, E>> + 'policy,
    SleepImpl: Sleeper + Unpin,
    SleepImpl::Sleep: 'policy,
{
    type Output = Result<T, RetryError<E, T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match &mut this.phase {
                AsyncPhase::ReadyToStartAttempt => {
                    let elapsed_before_attempt = current_elapsed(
                        #[cfg(feature = "std")]
                        &this.start,
                    );
                    let before_state = BeforeAttemptState {
                        attempt: this.attempt,
                        elapsed: elapsed_before_attempt,
                        total_wait: this.total_wait,
                    };
                    this.policy.before_attempt.call(&before_state);

                    let op_future = (this.op)();
                    this.phase = AsyncPhase::PollingOperation(Box::pin(op_future));
                }
                AsyncPhase::PollingOperation(op_future) => match op_future.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(outcome) => {
                        let elapsed_after_attempt = current_elapsed(
                            #[cfg(feature = "std")]
                            &this.start,
                        );
                        let mut retry_state = RetryState {
                            attempt: this.attempt,
                            elapsed: elapsed_after_attempt,
                            next_delay: Duration::ZERO,
                            total_wait: this.total_wait,
                        };

                        let should_retry = this.policy.predicate.should_retry(&outcome);
                        {
                            let attempt_state = attempt_state_from(&retry_state, &outcome);
                            this.policy.after_attempt.call(&attempt_state);
                        }
                        if !should_retry {
                            if this.collect_stats {
                                let stop_reason = stop_reason_for_predicate_accept(
                                    &outcome,
                                    this.policy.predicate_is_default,
                                );
                                this.final_stats = Some(RetryStats {
                                    attempts: this.attempt,
                                    total_elapsed: retry_state.elapsed,
                                    total_wait: this.total_wait,
                                    stop_reason,
                                });
                            }
                            this.phase = AsyncPhase::Finished;
                            return Poll::Ready(match outcome {
                                Ok(value) => Ok(value),
                                Err(error) => Err(RetryError::Exhausted {
                                    error,
                                    attempts: this.attempt,
                                    total_elapsed: retry_state.elapsed,
                                }),
                            });
                        }

                        let next_delay = this.policy.wait.next_wait(&retry_state);
                        retry_state.next_delay = next_delay;

                        if this.policy.stop.should_stop(&retry_state) {
                            {
                                let attempt_state = attempt_state_from(&retry_state, &outcome);
                                this.policy.on_exhausted.call(&attempt_state);
                            }
                            if this.collect_stats {
                                this.final_stats = Some(RetryStats {
                                    attempts: this.attempt,
                                    total_elapsed: retry_state.elapsed,
                                    total_wait: this.total_wait,
                                    stop_reason: StopReason::StopCondition,
                                });
                            }
                            this.phase = AsyncPhase::Finished;
                            return Poll::Ready(match outcome {
                                Err(error) => Err(RetryError::Exhausted {
                                    error,
                                    attempts: this.attempt,
                                    total_elapsed: retry_state.elapsed,
                                }),
                                Ok(last) => Err(RetryError::ConditionNotMet {
                                    last,
                                    attempts: this.attempt,
                                    total_elapsed: retry_state.elapsed,
                                }),
                            });
                        }

                        {
                            let attempt_state = attempt_state_from(&retry_state, &outcome);
                            this.policy.before_sleep.call(&attempt_state);
                        }
                        this.total_wait = this.total_wait.saturating_add(next_delay);
                        this.phase = AsyncPhase::Sleeping(Box::pin(this.sleeper.sleep(next_delay)));
                    }
                },
                AsyncPhase::Sleeping(sleep_future) => match sleep_future.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => {
                        this.attempt = this.attempt.saturating_add(1);
                        this.phase = AsyncPhase::ReadyToStartAttempt;
                    }
                },
                AsyncPhase::Finished => {
                    panic!("AsyncRetry polled after completion");
                }
            }
        }
    }
}

#[cfg(feature = "alloc")]
impl<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E> Future
    for AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OE: AttemptHook<T, E>,
    F: FnMut() -> Fut + Unpin,
    Fut: Future<Output = Result<T, E>> + 'policy,
    SleepImpl: Sleeper + Unpin,
    SleepImpl::Sleep: 'policy,
{
    type Output = (Result<T, RetryError<E, T>>, RetryStats);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                let stats = this
                    .inner
                    .final_stats
                    .take()
                    .expect("async retry completed without final stats");
                Poll::Ready((result, stats))
            }
        }
    }
}

#[cfg(feature = "alloc")]
impl<S, W, P, BA, AA, BS, OE> RetryPolicy<S, W, P, BA, AA, BS, OE>
where
    S: Stop,
    W: Wait,
    BA: BeforeAttemptHook,
{
    /// Begins configuring async retry execution.
    pub fn retry_async<T, E, F, Fut>(
        &mut self,
        op: F,
    ) -> AsyncRetry<'_, S, W, P, BA, AA, BS, OE, F, Fut, NoAsyncSleep, T, E>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        self.stop.reset();
        self.wait.reset();
        AsyncRetry {
            policy: self,
            op,
            sleeper: NoAsyncSleep,
            phase: AsyncPhase::ReadyToStartAttempt,
            attempt: 1,
            total_wait: Duration::ZERO,
            collect_stats: false,
            final_stats: None,
            #[cfg(feature = "std")]
            start: Instant::now(),
            _marker: PhantomData,
        }
    }
}

fn stop_reason_for_predicate_accept<T, E>(
    outcome: &Result<T, E>,
    predicate_is_default: bool,
) -> StopReason {
    if outcome.is_ok() && predicate_is_default {
        StopReason::Success
    } else {
        StopReason::PredicateAccepted
    }
}

#[cfg(feature = "std")]
fn current_elapsed(start: &Instant) -> Option<Duration> {
    Some(start.elapsed())
}

#[cfg(not(feature = "std"))]
fn current_elapsed() -> Option<Duration> {
    None
}
