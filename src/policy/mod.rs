//! RetryPolicy builder and sync/async execution engines.

use crate::compat::Duration;
use crate::error::RetryError;
use crate::on;
use crate::predicate::Predicate;
use crate::state::{AttemptState, BeforeAttemptState, RetryState};
use crate::stats::RetryStats;
use crate::stop::{self, Stop};
use crate::wait::{self, Wait};
#[cfg(feature = "serde")]
use serde::ser::SerializeStruct;

#[cfg(feature = "alloc")]
use crate::compat::Box;

/// Default maximum attempts used by the safe policy constructor.
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Default initial backoff used by the safe policy constructor.
const DEFAULT_INITIAL_WAIT: Duration = Duration::from_millis(100);

/// Function pointer type used to supply elapsed time in no_std or custom runtimes.
type ElapsedClockFn = fn() -> Duration;

#[derive(Clone)]
struct PolicyHooks<BA, AA, BS, OE> {
    before_attempt: BA,
    after_attempt: AA,
    before_sleep: BS,
    on_exhausted: OE,
}

#[derive(Clone, Copy)]
struct PolicyMeta {
    predicate_is_default: bool,
    elapsed_clock: Option<ElapsedClockFn>,
}

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
    hooks: PolicyHooks<BA, AA, BS, OE>,
    meta: PolicyMeta,
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
        state.serialize_field("predicate_is_default", &self.meta.predicate_is_default)?;
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
            hooks: PolicyHooks {
                before_attempt: (),
                after_attempt: (),
                before_sleep: (),
                on_exhausted: (),
            },
            meta: PolicyMeta {
                predicate_is_default: serialized.predicate_is_default,
                elapsed_clock: None,
            },
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
    hooks: PolicyHooks<BA, AA, BS, OE>,
    meta: PolicyMeta,
}

fn build_policy<S, W, P, BA, AA, BS, OE>(
    parts: PolicyParts<S, W, P, BA, AA, BS, OE>,
) -> RetryPolicy<S, W, P, BA, AA, BS, OE> {
    RetryPolicy {
        stop: parts.stop,
        wait: parts.wait,
        predicate: parts.predicate,
        hooks: parts.hooks,
        meta: parts.meta,
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
            hooks: PolicyHooks {
                before_attempt: (),
                after_attempt: (),
                before_sleep: (),
                on_exhausted: (),
            },
            meta: PolicyMeta {
                predicate_is_default: true,
                elapsed_clock: None,
            },
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
            hooks: PolicyHooks {
                before_attempt: (),
                after_attempt: (),
                before_sleep: (),
                on_exhausted: (),
            },
            meta: PolicyMeta {
                predicate_is_default: true,
                elapsed_clock: None,
            },
        })
    }
}

impl<S, W, P, BA, AA, BS, OE> RetryPolicy<S, W, P, BA, AA, BS, OE> {
    fn decompose(self) -> (S, W, P, PolicyHooks<BA, AA, BS, OE>, PolicyMeta) {
        (self.stop, self.wait, self.predicate, self.hooks, self.meta)
    }

    /// Replaces the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> RetryPolicy<NewStop, W, P, BA, AA, BS, OE> {
        let (_, wait, predicate, hooks, meta) = self.decompose();
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks,
            meta,
        })
    }

    /// Replaces the wait strategy.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> RetryPolicy<S, NewWait, P, BA, AA, BS, OE> {
        let (stop, _, predicate, hooks, meta) = self.decompose();
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks,
            meta,
        })
    }

    /// Replaces the retry predicate.
    #[must_use]
    pub fn when<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> RetryPolicy<S, W, NewPredicate, BA, AA, BS, OE> {
        let (stop, wait, _, hooks, meta) = self.decompose();
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks,
            meta: PolicyMeta {
                predicate_is_default: false,
                ..meta
            },
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
        let (stop, wait, predicate, hooks, meta) = self.decompose();
        build_policy(PolicyParts {
            stop: Box::new(stop),
            wait: Box::new(wait),
            predicate: Box::new(predicate),
            hooks,
            meta,
        })
    }

    /// Sets a custom elapsed-time clock used for stop strategies and stats.
    ///
    /// The provided function should return a monotonically increasing duration.
    /// Elapsed is computed as `clock() - clock_at_start` with saturating math.
    #[must_use]
    pub fn elapsed_clock(mut self, clock: ElapsedClockFn) -> Self {
        self.meta.elapsed_clock = Some(clock);
        self
    }

    /// Clears any custom elapsed-time clock and restores default behavior.
    #[must_use]
    pub fn clear_elapsed_clock(mut self) -> Self {
        self.meta.elapsed_clock = None;
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
        let (stop, wait, predicate, hooks, meta) = self.decompose();
        let PolicyHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exhausted,
        } = hooks;
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks: PolicyHooks {
                before_attempt: HookChain::new(before_attempt, hook),
                after_attempt,
                before_sleep,
                on_exhausted,
            },
            meta,
        })
    }

    /// Appends an after-attempt hook.
    #[must_use]
    pub fn after_attempt<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, BA, HookChain<AA, Hook>, BS, OE> {
        let (stop, wait, predicate, hooks, meta) = self.decompose();
        let PolicyHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exhausted,
        } = hooks;
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks: PolicyHooks {
                before_attempt,
                after_attempt: HookChain::new(after_attempt, hook),
                before_sleep,
                on_exhausted,
            },
            meta,
        })
    }

    /// Appends a before-sleep hook.
    #[must_use]
    pub fn before_sleep<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, BA, AA, HookChain<BS, Hook>, OE> {
        let (stop, wait, predicate, hooks, meta) = self.decompose();
        let PolicyHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exhausted,
        } = hooks;
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks: PolicyHooks {
                before_attempt,
                after_attempt,
                before_sleep: HookChain::new(before_sleep, hook),
                on_exhausted,
            },
            meta,
        })
    }

    /// Appends an on-exhausted hook.
    #[must_use]
    pub fn on_exhausted<Hook>(
        self,
        hook: Hook,
    ) -> RetryPolicy<S, W, P, BA, AA, BS, HookChain<OE, Hook>> {
        let (stop, wait, predicate, hooks, meta) = self.decompose();
        let PolicyHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            on_exhausted,
        } = hooks;
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks: PolicyHooks {
                before_attempt,
                after_attempt,
                before_sleep,
                on_exhausted: HookChain::new(on_exhausted, hook),
            },
            meta,
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
        let (stop, wait, predicate, hooks, meta) = self.decompose();
        let PolicyHooks {
            after_attempt,
            before_sleep,
            on_exhausted,
            ..
        } = hooks;
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks: PolicyHooks {
                before_attempt: hook,
                after_attempt,
                before_sleep,
                on_exhausted,
            },
            meta,
        })
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, BS, OE> RetryPolicy<S, W, P, BA, (), BS, OE> {
    /// Sets the sole after-attempt hook (no-alloc mode).
    #[must_use]
    pub fn after_attempt<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, BA, Hook, BS, OE> {
        let (stop, wait, predicate, hooks, meta) = self.decompose();
        let PolicyHooks {
            before_attempt,
            before_sleep,
            on_exhausted,
            ..
        } = hooks;
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks: PolicyHooks {
                before_attempt,
                after_attempt: hook,
                before_sleep,
                on_exhausted,
            },
            meta,
        })
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, OE> RetryPolicy<S, W, P, BA, AA, (), OE> {
    /// Sets the sole before-sleep hook (no-alloc mode).
    #[must_use]
    pub fn before_sleep<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, BA, AA, Hook, OE> {
        let (stop, wait, predicate, hooks, meta) = self.decompose();
        let PolicyHooks {
            before_attempt,
            after_attempt,
            on_exhausted,
            ..
        } = hooks;
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks: PolicyHooks {
                before_attempt,
                after_attempt,
                before_sleep: hook,
                on_exhausted,
            },
            meta,
        })
    }
}

#[cfg(not(feature = "alloc"))]
impl<S, W, P, BA, AA, BS> RetryPolicy<S, W, P, BA, AA, BS, ()> {
    /// Sets the sole on-exhausted hook (no-alloc mode).
    #[must_use]
    pub fn on_exhausted<Hook>(self, hook: Hook) -> RetryPolicy<S, W, P, BA, AA, BS, Hook> {
        let (stop, wait, predicate, hooks, meta) = self.decompose();
        let PolicyHooks {
            before_attempt,
            after_attempt,
            before_sleep,
            ..
        } = hooks;
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            hooks: PolicyHooks {
                before_attempt,
                after_attempt,
                before_sleep,
                on_exhausted: hook,
            },
            meta,
        })
    }
}

mod common;
mod sync;
mod time;

#[cfg(feature = "alloc")]
mod async_retry;

#[cfg(feature = "alloc")]
pub use async_retry::{AsyncRetry, AsyncRetryWithStats};
pub use sync::{SyncRetry, SyncRetryWithStats};
