//! `RetryPolicy` builder and sync/async execution engines.
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

#[cfg(feature = "alloc")]
use crate::compat::Box;
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
const DEFAULT_MAX_ATTEMPTS: u32 = 3;
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
/// use relentless::{RetryPolicy, stop, wait};
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
/// use relentless::{RetryPolicy, stop};
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

impl RetryPolicy<stop::StopAfterAttempts, wait::WaitExponential, predicate::PredicateAnyError> {
    /// Creates a policy with safe defaults: `attempts(3)`, `exponential(100ms)`,
    /// `any_error()`.
    ///
    /// ```
    /// use relentless::RetryPolicy;
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
    /// Sets the stop condition for the retry policy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> RetryPolicy<NewStop, W, P> {
        RetryPolicy {
            stop,
            wait: self.wait,
            predicate: self.predicate,
        }
    }

    /// Sets the wait strategy used between retry attempts.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> RetryPolicy<S, NewWait, P> {
        RetryPolicy {
            stop: self.stop,
            wait,
            predicate: self.predicate,
        }
    }

    /// Sets the predicate that decides whether a failed attempt should be retried.
    #[must_use]
    pub fn when<NewPredicate>(self, predicate: NewPredicate) -> RetryPolicy<S, W, NewPredicate> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate,
        }
    }

    /// Sets a predicate that retries *until* `p.should_retry()` returns `true`.
    ///
    /// Wraps `p` in [`predicate::PredicateUntil`](crate::predicate::PredicateUntil),
    /// negating its result. Natural for polling:
    /// `.until(ok(|s| s.is_ready()))` reads "retry until ready."
    #[must_use]
    pub fn until<NewPredicate>(
        self,
        predicate: NewPredicate,
    ) -> RetryPolicy<S, W, predicate::PredicateUntil<NewPredicate>> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            predicate: predicate::until(predicate),
        }
    }

    /// Erases the generic stop, wait, and predicate parameters behind trait objects.
    ///
    /// Useful when the policy must be stored or passed without its concrete type
    /// parameters, such as in heterogeneous collections or across API boundaries.
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

    /// Erases the generic stop, wait, and predicate parameters behind local trait objects.
    ///
    /// Like [`boxed`](Self::boxed) but without `Send` bounds. Use this when
    /// the policy must be stored without its concrete type parameters but
    /// does not need to cross thread boundaries.
    ///
    /// # Examples
    ///
    /// ```
    /// use relentless::RetryPolicy;
    ///
    /// let policy = RetryPolicy::new().boxed_local::<(), &str>();
    /// let _ = policy.retry(|_| Err::<(), _>("fail")).sleep(|_| {}).call();
    /// ```
    #[cfg(feature = "alloc")]
    #[must_use]
    #[allow(clippy::type_complexity)]
    pub fn boxed_local<T, E>(
        self,
    ) -> RetryPolicy<
        Box<dyn Stop + 'static>,
        Box<dyn Wait + 'static>,
        Box<dyn Predicate<T, E> + 'static>,
    >
    where
        S: Stop + 'static,
        W: Wait + 'static,
        P: Predicate<T, E> + 'static,
    {
        RetryPolicy {
            stop: Box::new(self.stop),
            wait: Box::new(self.wait),
            predicate: Box::new(self.predicate),
        }
    }
}

/// Abstracts over owned (`RetryPolicy<S,W,P>`) and borrowed (`&RetryPolicy<S,W,P>`)
/// storage so that `SyncRetry`/`AsyncRetry` (which borrow) and the ext-trait
/// builders (which own) can share the same execution engine.
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
            /// Registers a hook that runs before each retry attempt.
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

            /// Registers a hook that runs after each retry attempt.
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

            /// Registers a hook that runs when the retry loop exits.
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
