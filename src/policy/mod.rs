//! RetryPolicy builder and sync/async execution engines.
//!
//! This module exposes three related entry-point families:
//!
//! - [`RetryPolicy::retry`] and [`RetryPolicy::retry_async`] borrow a policy by
//!   `&self`. All core traits use `&self` receivers, so policies are freely
//!   reusable across sequential executions without mutation.
//! - [`crate::RetryExt`] and [`crate::AsyncRetryExt`] start from
//!   [`RetryPolicy::default()`] and own the policy immediately. Use those for
//!   one-off operations where you want to configure retries inline from the
//!   operation itself.
//!
//! Hook callbacks live on the execution builders, not on `RetryPolicy`. That
//! keeps the reusable policy focused on stop, wait, predicate, and clock
//! configuration, while per-call hooks, sleepers, and stats remain
//! local to a specific execution.

use crate::compat::Duration;
use crate::predicate;
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

/// Reusable retry configuration.
///
/// `RetryPolicy` stores retry strategies and can be reused across multiple
/// operations. Hook callbacks are configured per-execution on `SyncRetry` and
/// `AsyncRetry` builders.
///
/// Construction options:
/// - [`RetryPolicy::new`] returns a safe default policy: `attempts(3)`,
///   `exponential(100ms)`, `any_error()`.
/// - [`Default::default`] delegates to `new()`.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryPolicy, stop, wait};
/// use core::time::Duration;
///
/// let policy = RetryPolicy::new()
///     .stop(stop::attempts(3))
///     .wait(wait::fixed(Duration::from_millis(5)));
///
/// let _ = policy.retry(|_| Err::<(), _>("fail")).sleep(|_dur| {}).call();
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
pub struct RetryPolicy<
    S = stop::StopAfterAttempts,
    W = wait::WaitExponential,
    P = predicate::PredicateAnyError,
> {
    stop: S,
    wait: W,
    predicate: P,
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
        })
    }
}

impl RetryPolicy<stop::StopAfterAttempts, wait::WaitExponential, predicate::PredicateAnyError> {
    /// Creates a policy with safe defaults: `attempts(3)`, `exponential(100ms)`,
    /// `any_error()`.
    ///
    /// ```
    /// use tenacious::RetryPolicy;
    ///
    /// let policy = RetryPolicy::new();
    /// let _ = policy.retry(|_| Ok::<(), &str>(())).sleep(|_| {}).call();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        RetryPolicy {
            stop: stop::attempts(DEFAULT_MAX_ATTEMPTS),
            wait: wait::exponential(DEFAULT_INITIAL_WAIT),
            predicate: predicate::any_error(),
        }
    }
}

impl Default
    for RetryPolicy<stop::StopAfterAttempts, wait::WaitExponential, predicate::PredicateAnyError>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S, W, P> RetryPolicy<S, W, P> {
    /// Replaces the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> RetryPolicy<NewStop, W, P> {
        RetryPolicy {
            stop,
            wait: self.wait,
            predicate: self.predicate,
        }
    }

    /// Replaces the wait strategy.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> RetryPolicy<S, NewWait, P> {
        RetryPolicy {
            stop: self.stop,
            wait,
            predicate: self.predicate,
        }
    }

    /// Replaces the retry predicate.
    #[must_use]
    pub fn when<NewPredicate>(self, predicate: NewPredicate) -> RetryPolicy<S, W, NewPredicate> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate,
        }
    }

    /// Converts this policy into a type-erased boxed variant.
    #[cfg(feature = "alloc")]
    #[must_use]
    #[allow(clippy::type_complexity)]
    pub fn boxed<T, E>(
        self,
    ) -> RetryPolicy<
        Box<dyn Stop + Send + 'static>,
        Box<dyn Wait + Send + 'static>,
        Box<dyn Predicate<T, E> + Send + 'static>,
    >
    where
        S: Stop + Send + 'static,
        W: Wait + Send + 'static,
        P: Predicate<T, E> + Send + 'static,
    {
        RetryPolicy {
            stop: Box::new(self.stop),
            wait: Box::new(self.wait),
            predicate: Box::new(self.predicate),
        }
    }
}

/// Internal abstraction over owned and borrowed policy storage.
pub(crate) trait PolicyHandle<S, W, P> {
    fn policy_ref(&self) -> &RetryPolicy<S, W, P>;
}

impl<S, W, P> PolicyHandle<S, W, P> for RetryPolicy<S, W, P> {
    fn policy_ref(&self) -> &RetryPolicy<S, W, P> {
        self
    }
}

impl<S, W, P> PolicyHandle<S, W, P> for &RetryPolicy<S, W, P> {
    fn policy_ref(&self) -> &RetryPolicy<S, W, P> {
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
                Hook: FnMut(&RetryState),
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
pub use execution::async_exec::{AsyncRetry, AsyncRetryWithStats, NoAsyncSleep};
#[cfg(feature = "alloc")]
pub(crate) use execution::hooks::HookChain;
pub(crate) use execution::hooks::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook};
pub use execution::sync_exec::NoSyncSleep;
pub use execution::sync_exec::{SyncRetry, SyncRetryWithStats};
pub use ext::{
    AsyncRetryBuilder, AsyncRetryBuilderWithStats, AsyncRetryExt, DefaultAsyncRetryBuilder,
    DefaultAsyncRetryBuilderWithStats, DefaultSyncRetryBuilder, DefaultSyncRetryBuilderWithStats,
    RetryExt, SyncRetryBuilder, SyncRetryBuilderWithStats,
};
