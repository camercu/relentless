//! RetryPolicy builder and sync/async execution engines.
//!
//! This module exposes three related entry-point families:
//!
//! - [`RetryPolicy::retry`] and [`RetryPolicy::retry_async`] borrow a policy by
//!   `&mut self`. Use these when you want one reusable policy value whose
//!   stateful `Stop` and `Wait` strategies are reset and reused in place across
//!   sequential executions.
//! - [`RetryPolicy::retry_clone`] and
//!   [`RetryPolicy::retry_async_clone`] take `&self` and clone the policy into
//!   an owned execution builder. Use these when you want to keep a shared
//!   template policy and start independent retry executions without taking a
//!   mutable borrow of the original value.
//! - [`crate::RetryExt`] and [`crate::AsyncRetryExt`] start from
//!   [`RetryPolicy::default()`] and own the policy immediately. Use those for
//!   one-off operations where you want to configure retries inline from the
//!   operation itself.
//!
//! Hook callbacks live on the execution builders, not on `RetryPolicy`. That
//! keeps the reusable policy focused on stop, wait, predicate, and clock
//! configuration, while per-call hooks, sleepers, cancelers, and stats remain
//! local to a specific execution.

use crate::compat::Duration;
use crate::on;
#[cfg(feature = "alloc")]
use crate::predicate::Predicate;
use crate::stop;
#[cfg(feature = "alloc")]
use crate::stop::Stop;
use crate::wait;
#[cfg(feature = "alloc")]
use crate::wait::Wait;
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

#[derive(Debug, Clone, Copy)]
struct PolicyMeta {
    elapsed_clock: Option<ElapsedClockFn>,
}

impl PartialEq for PolicyMeta {
    fn eq(&self, other: &Self) -> bool {
        match (self.elapsed_clock, other.elapsed_clock) {
            (Some(left), Some(right)) => core::ptr::fn_addr_eq(left, right),
            (None, None) => true,
            _ => false,
        }
    }
}

impl Eq for PolicyMeta {}

/// Reusable retry configuration.
///
/// `RetryPolicy` stores retry strategies and can be reused across multiple
/// operations. Hook callbacks are configured per-execution on `SyncRetry` and
/// `AsyncRetry` builders.
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
///
/// When you need shared-template reuse without taking `&mut self`, clone into
/// an owned execution:
///
/// ```
/// use tenacious::{RetryPolicy, stop};
///
/// let template = RetryPolicy::new().stop(stop::attempts(3));
/// let result = template
///     .retry_clone(|| Err::<(), _>("fail"))
///     .sleep(|_dur| {})
///     .call();
///
/// assert!(result.is_err());
/// ```
///
/// ```compile_fail
/// use tenacious::{RetryPolicy, stop};
///
/// let _ = RetryPolicy::new()
///     .stop(stop::attempts(1))
///     .before_attempt(|_state| {});
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy<S = stop::StopAfterAttempts, W = wait::WaitExponential, P = on::AnyError> {
    stop: S,
    wait: W,
    predicate: P,
    meta: PolicyMeta,
}

#[cfg(feature = "serde")]
impl<S, W, P> serde::Serialize for RetryPolicy<S, W, P>
where
    S: serde::Serialize,
    W: serde::Serialize,
    P: serde::Serialize,
{
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("RetryPolicy", 3)?;
        state.serialize_field("stop", &self.stop)?;
        state.serialize_field("wait", &self.wait)?;
        state.serialize_field("predicate", &self.predicate)?;
        state.end()
    }
}

/// # Serde round-trip note
///
/// The `elapsed_clock` function pointer is not serializable and is dropped
/// during serialization. Deserialized policies always have `elapsed_clock`
/// set to `None`. If you rely on a custom clock, re-apply it after
/// deserialization via [`RetryPolicy::elapsed_clock`].
#[cfg(feature = "serde")]
impl<'de, S, W, P> serde::Deserialize<'de> for RetryPolicy<S, W, P>
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
        }

        let serialized = SerializedRetryPolicy::deserialize(deserializer)?;
        Ok(Self {
            stop: serialized.stop,
            wait: serialized.wait,
            predicate: serialized.predicate,
            meta: PolicyMeta {
                elapsed_clock: None,
            },
        })
    }
}

/// Type-erased retry policy for runtime-configured storage.
///
/// This alias is available when `alloc` is enabled and erases stop/wait/predicate
/// concrete types into trait objects.
///
/// # Examples
///
/// ```
/// use tenacious::{BoxedRetryPolicy, RetryPolicy, stop};
///
/// let _policy: BoxedRetryPolicy<i32, &'static str> =
///     RetryPolicy::new().stop(stop::attempts(3)).boxed();
/// ```
#[cfg(feature = "alloc")]
pub type BoxedRetryPolicy<T, E> =
    RetryPolicy<Box<dyn Stop>, Box<dyn Wait>, Box<dyn Predicate<T, E>>>;

struct PolicyParts<S, W, P> {
    stop: S,
    wait: W,
    predicate: P,
    meta: PolicyMeta,
}

fn build_policy<S, W, P>(parts: PolicyParts<S, W, P>) -> RetryPolicy<S, W, P> {
    RetryPolicy {
        stop: parts.stop,
        wait: parts.wait,
        predicate: parts.predicate,
        meta: parts.meta,
    }
}

impl RetryPolicy<stop::NeedsStop, wait::WaitFixed, on::AnyError> {
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
            meta: PolicyMeta {
                elapsed_clock: None,
            },
        })
    }
}

impl Default for RetryPolicy<stop::StopAfterAttempts, wait::WaitExponential, on::AnyError> {
    fn default() -> Self {
        build_policy(PolicyParts {
            stop: stop::attempts(DEFAULT_MAX_ATTEMPTS),
            wait: wait::exponential(DEFAULT_INITIAL_WAIT),
            predicate: on::any_error(),
            meta: PolicyMeta {
                elapsed_clock: None,
            },
        })
    }
}

impl<S, W, P> RetryPolicy<S, W, P> {
    fn decompose(self) -> (S, W, P, PolicyMeta) {
        (self.stop, self.wait, self.predicate, self.meta)
    }

    /// Replaces the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> RetryPolicy<NewStop, W, P> {
        let (_, wait, predicate, meta) = self.decompose();
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            meta,
        })
    }

    /// Replaces the wait strategy.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> RetryPolicy<S, NewWait, P> {
        let (stop, _, predicate, meta) = self.decompose();
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            meta,
        })
    }

    /// Replaces the retry predicate.
    #[must_use]
    pub fn when<NewPredicate>(self, predicate: NewPredicate) -> RetryPolicy<S, W, NewPredicate> {
        let (stop, wait, _, meta) = self.decompose();
        build_policy(PolicyParts {
            stop,
            wait,
            predicate,
            meta,
        })
    }

    /// Converts this policy into a type-erased boxed variant.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn boxed<T, E>(self) -> BoxedRetryPolicy<T, E>
    where
        S: Stop + 'static,
        W: Wait + 'static,
        P: Predicate<T, E> + 'static,
    {
        let (stop, wait, predicate, meta) = self.decompose();
        build_policy(PolicyParts {
            stop: Box::new(stop),
            wait: Box::new(wait),
            predicate: Box::new(predicate),
            meta,
        })
    }

    /// Sets a custom elapsed-time clock used for stop strategies and stats.
    ///
    /// The provided function should return a monotonically increasing duration.
    /// Elapsed is computed as `clock() - clock_at_start` with saturating math.
    ///
    /// The clock is a bare `fn()` pointer rather than a generic or boxed closure
    /// so that `RetryPolicy` remains `Copy` and `'static` without allocation.
    /// This means the clock cannot capture state directly. For test clocks that
    /// need mutation, use a `static AtomicU64` (or similar) that the `fn()`
    /// reads:
    ///
    /// ```
    /// use core::sync::atomic::{AtomicU64, Ordering};
    /// use core::time::Duration;
    ///
    /// static MILLIS: AtomicU64 = AtomicU64::new(0);
    ///
    /// fn test_clock() -> Duration {
    ///     Duration::from_millis(MILLIS.load(Ordering::Relaxed))
    /// }
    /// ```
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

/// Internal abstraction over owned and borrowed policy storage.
pub(crate) trait PolicyHandle<S, W, P> {
    fn policy_mut(&mut self) -> &mut RetryPolicy<S, W, P>;
}

impl<S, W, P> PolicyHandle<S, W, P> for RetryPolicy<S, W, P> {
    fn policy_mut(&mut self) -> &mut RetryPolicy<S, W, P> {
        self
    }
}

impl<S, W, P> PolicyHandle<S, W, P> for &mut RetryPolicy<S, W, P> {
    fn policy_mut(&mut self) -> &mut RetryPolicy<S, W, P> {
        self
    }
}

/// Generates `#[cfg(feature = "alloc")]` hook-chaining methods for a builder.
///
/// Produces `before_attempt`, `after_attempt`, and `on_exit` methods that
/// delegate to `self.map_hooks(|h| h.chain_*(hook))`.
macro_rules! impl_alloc_hook_chain {
    (
        impl[$($gen:tt)*] $Builder:ty
        $(where { $($wc:tt)* })? =>
        before_attempt -> { $($ba:tt)* },
        after_attempt -> { $($aa:tt)* },
        on_exit -> { $($ox:tt)* } $(,)?
    ) => {
        #[cfg(feature = "alloc")]
        // Intentional: hook chaining preserves type-state and avoids runtime
        // indirection; signatures are long but mechanically structured.
        #[allow(clippy::type_complexity)]
        impl<$($gen)*> $Builder
        $(where $($wc)*)?
        {
            /// Appends a before-attempt hook.
            #[must_use]
            pub fn before_attempt<Hook>(
                self,
                hook: Hook,
            ) -> $($ba)*
            where
                Hook: FnMut(&BeforeAttemptState),
            {
                self.map_hooks(|hooks| hooks.chain_before_attempt(hook))
            }

            /// Appends an after-attempt hook.
            #[must_use]
            pub fn after_attempt<Hook>(
                self,
                hook: Hook,
            ) -> $($aa)*
            where
                Hook: for<'a> FnMut(&AttemptState<'a, T, E>),
            {
                self.map_hooks(|hooks| hooks.chain_after_attempt(hook))
            }

            /// Appends an on-exit hook.
            #[must_use]
            pub fn on_exit<Hook>(
                self,
                hook: Hook,
            ) -> $($ox)*
            where
                Hook: for<'a> FnMut(&ExitState<'a, T, E>),
            {
                self.map_hooks(|hooks| hooks.chain_on_exit(hook))
            }
        }
    };
}

mod execution;
mod ext;
mod time;
pub use execution::async_exec::{AsyncRetry, AsyncRetryWithStats};
#[cfg(feature = "alloc")]
pub(crate) use execution::hooks::HookChain;
pub(crate) use execution::hooks::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook};
pub use execution::sync_exec::{SyncRetry, SyncRetryWithStats};
pub use ext::{AsyncRetryBuilder, AsyncRetryBuilderWithStats, AsyncRetryExt};
pub use ext::{RetryExt, SyncRetryBuilder, SyncRetryBuilderWithStats};
