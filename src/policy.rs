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
#[cfg(feature = "alloc")]
use pin_project_lite::pin_project;
#[cfg(feature = "std")]
use std::time::Instant;

#[cfg(all(feature = "alloc", feature = "std"))]
type RetryStart = Instant;
#[cfg(all(feature = "alloc", not(feature = "std")))]
type RetryStart = ();

/// Default maximum attempts used by the safe policy constructor.
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Default initial backoff used by the safe policy constructor.
const DEFAULT_INITIAL_WAIT: Duration = Duration::from_millis(100);

/// Function pointer type used to supply elapsed time in no_std or custom runtimes.
type ElapsedClockFn = fn() -> Duration;

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
#[cfg(feature = "alloc")]
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

#[cfg(feature = "alloc")]
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

#[cfg(feature = "alloc")]
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
/// Construction options:
/// - [`RetryPolicy::new`] starts with `NeedsStop` and intentionally blocks
///   execution until `.stop(...)` is configured.
/// - [`Default::default`] returns a fully configured safe policy
///   (`attempts(3)` plus `exponential(100ms)`).
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
    S = stop::StopAfterAttempts,
    W = wait::WaitExponential,
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
    elapsed_clock: Option<ElapsedClockFn>,
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
            elapsed_clock: None,
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

struct PolicyParts<S, W, P, BA, AA, BS, OE> {
    stop: S,
    wait: W,
    predicate: P,
    before_attempt: BA,
    after_attempt: AA,
    before_sleep: BS,
    on_exhausted: OE,
    predicate_is_default: bool,
    elapsed_clock: Option<ElapsedClockFn>,
}

fn build_policy<S, W, P, BA, AA, BS, OE>(
    parts: PolicyParts<S, W, P, BA, AA, BS, OE>,
) -> RetryPolicy<S, W, P, BA, AA, BS, OE> {
    RetryPolicy {
        stop: parts.stop,
        wait: parts.wait,
        predicate: parts.predicate,
        before_attempt: parts.before_attempt,
        after_attempt: parts.after_attempt,
        before_sleep: parts.before_sleep,
        on_exhausted: parts.on_exhausted,
        predicate_is_default: parts.predicate_is_default,
        elapsed_clock: parts.elapsed_clock,
    }
}

impl RetryPolicy<stop::NeedsStop, wait::WaitFixed, on::AnyError, (), (), (), ()> {
    /// Creates an unconfigured policy with no stop strategy selected yet.
    ///
    /// This constructor sets zero wait and the default retry predicate
    /// (`on::any_error()`), but requires calling `.stop(...)` before retry
    /// execution methods are available.
    ///
    /// ```compile_fail
    /// use tenacious::RetryPolicy;
    ///
    /// let mut policy = RetryPolicy::new();
    /// let _ = policy.retry(|| Ok::<(), &str>(())).sleep(|_| {}).call();
    /// ```
    ///
    /// ```
    /// use tenacious::{RetryPolicy, stop};
    ///
    /// let mut policy = RetryPolicy::new().stop(stop::attempts(3));
    /// let _ = policy.retry(|| Ok::<(), &str>(())).sleep(|_| {}).call();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        build_policy(PolicyParts {
            stop: stop::NeedsStop,
            wait: wait::fixed(Duration::ZERO),
            predicate: on::any_error(),
            before_attempt: (),
            after_attempt: (),
            before_sleep: (),
            on_exhausted: (),
            predicate_is_default: true,
            elapsed_clock: None,
        })
    }
}

impl Default
    for RetryPolicy<stop::StopAfterAttempts, wait::WaitExponential, on::AnyError, (), (), (), ()>
{
    fn default() -> Self {
        build_policy(PolicyParts {
            stop: stop::attempts(DEFAULT_MAX_ATTEMPTS),
            wait: wait::exponential(DEFAULT_INITIAL_WAIT),
            predicate: on::any_error(),
            before_attempt: (),
            after_attempt: (),
            before_sleep: (),
            on_exhausted: (),
            predicate_is_default: true,
            elapsed_clock: None,
        })
    }
}

impl<S, W, P, BA, AA, BS, OE> RetryPolicy<S, W, P, BA, AA, BS, OE> {
    /// Replaces the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> RetryPolicy<NewStop, W, P, BA, AA, BS, OE> {
        build_policy(PolicyParts {
            stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
    }

    /// Replaces the wait strategy.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> RetryPolicy<S, NewWait, P, BA, AA, BS, OE> {
        build_policy(PolicyParts {
            stop: self.stop,
            wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
    }

    /// Replaces the retry predicate.
    #[must_use]
    pub fn when<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> RetryPolicy<S, W, NewPredicate, BA, AA, BS, OE> {
        build_policy(PolicyParts {
            stop: self.stop,
            wait: self.wait,
            predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: false,
            elapsed_clock: self.elapsed_clock,
        })
    }

    /// Converts this policy into a type-erased boxed variant.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn boxed<T, E>(self) -> BoxedRetryPolicy<T, E, BA, AA, BS, OE>
    where
        S: Stop + 'static,
        W: Wait + 'static,
        P: Predicate<T, E> + 'static,
    {
        build_policy(PolicyParts {
            stop: Box::new(self.stop),
            wait: Box::new(self.wait),
            predicate: Box::new(self.predicate),
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
    }

    /// Sets a custom elapsed-time clock used for stop strategies and stats.
    ///
    /// The provided function should return a monotonically increasing duration.
    /// Elapsed is computed as `clock() - clock_at_start` with saturating math.
    #[must_use]
    pub fn elapsed_clock(mut self, clock: ElapsedClockFn) -> Self {
        self.elapsed_clock = Some(clock);
        self
    }

    /// Clears any custom elapsed-time clock and restores default behavior.
    #[must_use]
    pub fn clear_elapsed_clock(mut self) -> Self {
        self.elapsed_clock = None;
        self
    }
}

#[cfg(feature = "alloc")]
impl<S, W, P, BA, AA, BS, OE> RetryPolicy<S, W, P, BA, AA, BS, OE> {
    /// Appends a before-attempt hook.
    #[must_use]
    pub fn before_attempt<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, HookChain<BA, Hook>, AA, BS, OE>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        build_policy(PolicyParts {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: HookChain::new(self.before_attempt, hook),
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
    }

    /// Appends an after-attempt hook.
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, BA, HookChain<AA, Hook>, BS, OE> {
        build_policy(PolicyParts {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: HookChain::new(self.after_attempt, hook),
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
    }

    /// Appends a before-sleep hook.
    #[must_use]
    pub fn before_sleep<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, BA, AA, HookChain<BS, Hook>, OE> {
        build_policy(PolicyParts {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: HookChain::new(self.before_sleep, hook),
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
    }

    /// Appends an on-exhausted hook.
    #[must_use]
    pub fn on_exhausted<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, BA, AA, BS, HookChain<OE, Hook>> {
        build_policy(PolicyParts {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: HookChain::new(self.on_exhausted, hook),
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, AA, BS, OE> RetryPolicy<S, W, P, (), AA, BS, OE> {
    /// Sets the sole before-attempt hook (no-alloc mode).
    #[must_use]
    pub fn before_attempt<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, Hook, AA, BS, OE>
    where
        Hook: FnMut(&BeforeAttemptState),
    {
        build_policy(PolicyParts {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: hook,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, BS, OE> RetryPolicy<S, W, P, BA, (), BS, OE> {
    /// Sets the sole after-attempt hook (no-alloc mode).
    #[must_use]
    pub fn after_attempt<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, BA, Hook, BS, OE> {
        build_policy(PolicyParts {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: hook,
            before_sleep: self.before_sleep,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, OE> RetryPolicy<S, W, P, BA, AA, (), OE> {
    /// Sets the sole before-sleep hook (no-alloc mode).
    #[must_use]
    pub fn before_sleep<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, BA, AA, Hook, OE> {
        build_policy(PolicyParts {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: hook,
            on_exhausted: self.on_exhausted,
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, BS> RetryPolicy<S, W, P, BA, AA, BS, ()> {
    /// Sets the sole on-exhausted hook (no-alloc mode).
    #[must_use]
    pub fn on_exhausted<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, BA, AA, BS, Hook> {
        build_policy(PolicyParts {
            stop: self.stop,
            wait: self.wait,
            predicate: self.predicate,
            before_attempt: self.before_attempt,
            after_attempt: self.after_attempt,
            before_sleep: self.before_sleep,
            on_exhausted: hook,
            predicate_is_default: self.predicate_is_default,
            elapsed_clock: self.elapsed_clock,
        })
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

enum AttemptTransition<T, E> {
    Finished {
        result: Result<T, RetryError<E, T>>,
        stats: Option<RetryStats>,
    },
    Sleep {
        next_delay: Duration,
    },
}

fn process_attempt_transition<S, W, P, BA, AA, BS, OE, T, E>(
    policy: &mut RetryPolicy<S, W, P, BA, AA, BS, OE>,
    outcome: Result<T, E>,
    mut retry_state: RetryState,
    collect_stats: bool,
    total_wait: Duration,
) -> AttemptTransition<T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OE: AttemptHook<T, E>,
{
    let should_retry = policy.predicate.should_retry(&outcome);
    {
        let attempt_state = attempt_state_from(&retry_state, &outcome);
        policy.after_attempt.call(&attempt_state);
    }
    if !should_retry {
        let stats = if collect_stats {
            Some(RetryStats {
                attempts: retry_state.attempt,
                total_elapsed: retry_state.elapsed,
                total_wait,
                stop_reason: stop_reason_for_predicate_accept(
                    &outcome,
                    policy.predicate_is_default,
                ),
            })
        } else {
            None
        };

        return AttemptTransition::Finished {
            result: match outcome {
                Ok(value) => Ok(value),
                Err(error) => Err(RetryError::PredicateRejected {
                    error,
                    attempts: retry_state.attempt,
                    total_elapsed: retry_state.elapsed,
                }),
            },
            stats,
        };
    }

    let next_delay = policy.wait.next_wait(&retry_state);
    retry_state.next_delay = next_delay;

    if policy.stop.should_stop(&retry_state) {
        {
            let attempt_state = attempt_state_from(&retry_state, &outcome);
            policy.on_exhausted.call(&attempt_state);
        }
        let stats = if collect_stats {
            Some(RetryStats {
                attempts: retry_state.attempt,
                total_elapsed: retry_state.elapsed,
                total_wait,
                stop_reason: StopReason::StopCondition,
            })
        } else {
            None
        };
        return AttemptTransition::Finished {
            result: match outcome {
                Err(error) => Err(RetryError::Exhausted {
                    error,
                    attempts: retry_state.attempt,
                    total_elapsed: retry_state.elapsed,
                }),
                Ok(last) => Err(RetryError::ConditionNotMet {
                    last,
                    attempts: retry_state.attempt,
                    total_elapsed: retry_state.elapsed,
                }),
            },
            stats,
        };
    }

    {
        let attempt_state = attempt_state_from(&retry_state, &outcome);
        policy.before_sleep.call(&attempt_state);
    }

    AttemptTransition::Sleep { next_delay }
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
        let elapsed_start = start_elapsed_clock(self.policy.elapsed_clock);
        #[cfg(feature = "std")]
        let start = Instant::now();

        loop {
            let elapsed_before_attempt = current_elapsed(
                elapsed_start,
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
                elapsed_start,
                #[cfg(feature = "std")]
                &start,
            );
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

/// Marker for the absence of an explicit async sleep implementation.
///
/// Users must call `.sleep(sleeper)` on [`AsyncRetry`] to provide a concrete
/// [`Sleeper`] before the future can be polled.
#[cfg(feature = "alloc")]
#[doc(hidden)]
pub struct NoAsyncSleep;

#[cfg(feature = "alloc")]
pin_project! {
    #[project = AsyncPhaseProj]
    enum AsyncPhase<Fut, SleepFut> {
        ReadyToStartAttempt,
        PollingOperation {
            #[pin]
            op_future: Fut,
        },
        Sleeping {
            #[pin]
            sleep_future: SleepFut,
        },
        Finished,
    }
}

#[cfg(feature = "alloc")]
pin_project! {
    /// Async retry execution object.
    ///
    /// Created by [`RetryPolicy::retry_async`]. Set a sleeper with `.sleep(...)`
    /// and then `.await` the returned future.
    ///
    /// `AsyncRetry` is a single-use future. Polling after completion is
    /// misuse: debug builds panic. Release builds return `Poll::Pending`
    /// unless the `strict-futures` feature is enabled, in which case they
    /// also panic.
    ///
    /// # Examples
    ///
    /// ```
    /// use tenacious::RetryPolicy;
    /// use core::time::Duration;
    ///
    /// let mut policy = RetryPolicy::new().stop(tenacious::stop::attempts(3));
    /// let retry = policy
    ///     .retry_async(|| async { Ok::<u32, &str>(1) })
    ///     .sleep(|_dur: Duration| async {});
    /// let _ = retry;
    /// ```
    pub struct AsyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut = ()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        policy: &'policy mut RetryPolicy<S, W, P, BA, AA, BS, OE>,
        op: F,
        sleeper: SleepImpl,
        #[pin]
        phase: AsyncPhase<Fut, SleepFut>,
        attempt: u32,
        total_wait: Duration,
        collect_stats: bool,
        final_stats: Option<RetryStats>,
        elapsed_start: Option<CustomElapsedStart>,
        start: RetryStart,
        _marker: PhantomData<fn() -> (T, E)>,
    }
}

#[cfg(feature = "alloc")]
pin_project! {
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
    /// let mut policy = RetryPolicy::new().stop(tenacious::stop::attempts(3));
    /// let retry = policy
    ///     .retry_async(|| async { Ok::<u32, &str>(1) })
    ///     .sleep(|_dur: Duration| async {})
    ///     .with_stats();
    /// let _ = retry;
    /// ```
    pub struct AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut = ()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        #[pin]
        inner: AsyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut>,
    }
}

#[cfg(feature = "alloc")]
type AsyncRetryWithSleep<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E> = AsyncRetry<
    'policy,
    S,
    W,
    P,
    BA,
    AA,
    BS,
    OE,
    F,
    Fut,
    SleepImpl,
    T,
    E,
    <SleepImpl as Sleeper>::Sleep,
>;

#[cfg(feature = "alloc")]
type AsyncRetryStats<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut> =
    AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut>;

#[cfg(feature = "alloc")]
impl<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, ()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Sets the async sleep implementation.
    #[must_use]
    pub fn sleep<NewSleep>(
        self,
        sleeper: NewSleep,
    ) -> AsyncRetryWithSleep<'policy, S, W, P, BA, AA, BS, OE, F, Fut, NewSleep, T, E>
    where
        NewSleep: Sleeper,
    {
        AsyncRetry {
            policy: self.policy,
            op: self.op,
            sleeper,
            phase: match self.phase {
                AsyncPhase::ReadyToStartAttempt => AsyncPhase::ReadyToStartAttempt,
                AsyncPhase::PollingOperation { op_future } => {
                    AsyncPhase::PollingOperation { op_future }
                }
                AsyncPhase::Sleeping { .. } => {
                    unreachable!("NoAsyncSleep cannot create sleeping futures")
                }
                AsyncPhase::Finished => AsyncPhase::Finished,
            },
            attempt: self.attempt,
            total_wait: self.total_wait,
            collect_stats: self.collect_stats,
            final_stats: self.final_stats,
            elapsed_start: self.elapsed_start,
            start: self.start,
            _marker: PhantomData,
        }
    }
}

#[cfg(feature = "alloc")]
impl<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut>
    AsyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    /// Wraps this async retry with statistics collection.
    #[must_use]
    #[allow(clippy::type_complexity)]
    pub fn with_stats(
        self,
    ) -> AsyncRetryStats<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut> {
        let mut inner = self;
        inner.collect_stats = true;
        AsyncRetryWithStats { inner }
    }
}

#[cfg(feature = "alloc")]
impl<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut> Future
    for AsyncRetry<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OE: AttemptHook<T, E>,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>> + 'policy,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()> + 'policy,
{
    type Output = Result<T, RetryError<E, T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            match this.phase.as_mut().project() {
                AsyncPhaseProj::ReadyToStartAttempt => {
                    let elapsed_before_attempt = current_elapsed(
                        *this.elapsed_start,
                        #[cfg(feature = "std")]
                        this.start,
                    );
                    let before_state = BeforeAttemptState {
                        attempt: *this.attempt,
                        elapsed: elapsed_before_attempt,
                        total_wait: *this.total_wait,
                    };
                    this.policy.before_attempt.call(&before_state);

                    let op_future = (this.op)();
                    this.phase.set(AsyncPhase::PollingOperation { op_future });
                }
                AsyncPhaseProj::PollingOperation { op_future } => match op_future.poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(outcome) => {
                        let elapsed_after_attempt = current_elapsed(
                            *this.elapsed_start,
                            #[cfg(feature = "std")]
                            this.start,
                        );
                        let retry_state = RetryState {
                            attempt: *this.attempt,
                            elapsed: elapsed_after_attempt,
                            next_delay: Duration::ZERO,
                            total_wait: *this.total_wait,
                        };

                        match process_attempt_transition(
                            &mut **this.policy,
                            outcome,
                            retry_state,
                            *this.collect_stats,
                            *this.total_wait,
                        ) {
                            AttemptTransition::Finished { result, stats } => {
                                *this.final_stats = stats;
                                this.phase.set(AsyncPhase::Finished);
                                return Poll::Ready(result);
                            }
                            AttemptTransition::Sleep { next_delay } => {
                                *this.total_wait = this.total_wait.saturating_add(next_delay);
                                let sleep_future = this.sleeper.sleep(next_delay);
                                this.phase.set(AsyncPhase::Sleeping { sleep_future });
                            }
                        }
                    }
                },
                AsyncPhaseProj::Sleeping { sleep_future } => match sleep_future.poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => {
                        *this.attempt = this.attempt.saturating_add(1);
                        this.phase.set(AsyncPhase::ReadyToStartAttempt);
                    }
                },
                AsyncPhaseProj::Finished => {
                    #[cfg(any(debug_assertions, feature = "strict-futures"))]
                    panic!("AsyncRetry polled after completion");

                    #[cfg(all(not(debug_assertions), not(feature = "strict-futures")))]
                    {
                        return Poll::Pending;
                    }
                }
            }
        }
    }
}

#[cfg(feature = "alloc")]
impl<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut> Future
    for AsyncRetryWithStats<'policy, S, W, P, BA, AA, BS, OE, F, Fut, SleepImpl, T, E, SleepFut>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OE: AttemptHook<T, E>,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>> + 'policy,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()> + 'policy,
{
    type Output = (Result<T, RetryError<E, T>>, RetryStats);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        match this.inner.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                let stats = this
                    .inner
                    .as_mut()
                    .project()
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
    #[must_use]
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
        let elapsed_start = start_elapsed_clock(self.elapsed_clock);
        AsyncRetry {
            policy: self,
            op,
            sleeper: NoAsyncSleep,
            phase: AsyncPhase::ReadyToStartAttempt,
            attempt: 1,
            total_wait: Duration::ZERO,
            collect_stats: false,
            final_stats: None,
            elapsed_start,
            start: retry_start_now(),
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

#[derive(Clone, Copy)]
struct CustomElapsedStart {
    clock: ElapsedClockFn,
    origin: Duration,
}

fn start_elapsed_clock(clock: Option<ElapsedClockFn>) -> Option<CustomElapsedStart> {
    clock.map(|clock| CustomElapsedStart {
        clock,
        origin: clock(),
    })
}

#[cfg(all(feature = "alloc", feature = "std"))]
fn retry_start_now() -> RetryStart {
    Instant::now()
}

#[cfg(all(feature = "alloc", not(feature = "std")))]
fn retry_start_now() -> RetryStart {}

#[cfg(feature = "std")]
fn current_elapsed(start_clock: Option<CustomElapsedStart>, start: &Instant) -> Option<Duration> {
    start_clock.map_or_else(
        || Some(start.elapsed()),
        |start_clock| Some((start_clock.clock)().saturating_sub(start_clock.origin)),
    )
}

#[cfg(not(feature = "std"))]
fn current_elapsed(start_clock: Option<CustomElapsedStart>) -> Option<Duration> {
    start_clock.map(|start_clock| (start_clock.clock)().saturating_sub(start_clock.origin))
}
