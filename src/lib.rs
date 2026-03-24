//! `tenacious` — a Rust library for retrying fallible operations and polling for conditions.
//!
//! This crate provides composable retry strategies with support for `std`, `alloc`,
//! and `no_std` environments.
//!
//! # Custom wait strategies
//!
//! ```
//! use core::time::Duration;
//! use tenacious::{RetryState, Wait, wait};
//!
//! struct CustomWait(Duration);
//!
//! impl Wait for CustomWait {
//!     fn next_wait(&self, _state: &RetryState) -> Duration {
//!         self.0
//!     }
//! }
//!
//! let strategy = CustomWait(Duration::from_millis(20))
//!     .cap(Duration::from_millis(15))
//!     .chain(wait::fixed(Duration::from_millis(50)), 2);
//!
//! let state = RetryState::new(3, None);
//! assert_eq!(strategy.next_wait(&state), Duration::from_millis(50));
//! ```
//!
//! # Extension-first usage
//!
//! In sync `std` builds, `.sleep(...)` is optional because `tenacious` falls
//! back to `std::thread::sleep`. The example below still calls `.sleep(...)`
//! so it compiles under `no_std` documentation test runs too.
//!
//! ```
//! use core::time::Duration;
//! use tenacious::{RetryExt, stop, wait};
//!
//! let result = (|| Err::<u32, &str>("transient"))
//!     .retry()
//!     .stop(stop::attempts(3))
//!     .wait(wait::fixed(Duration::from_millis(5)))
//!     .sleep(|_dur| {})
//!     .call();
//!
//! assert!(result.is_err());
//! ```

#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

mod compat;

mod error;
mod policy;
pub mod predicate;
pub mod sleep;
mod state;
mod stats;
pub mod stop;
pub mod wait;

// Re-export core public types at the crate root (spec 10.1).
pub use error::{RetryError, RetryResult};
pub use policy::RetryPolicy;
pub use policy::{AsyncRetry, AsyncRetryExt, AsyncRetryWithStats};
pub use policy::{RetryExt, SyncRetry, SyncRetryWithStats};
pub use predicate::Predicate;
pub use sleep::Sleeper;
pub use state::{AttemptState, ExitState, RetryState};
pub use stats::{RetryStats, StopReason};
pub use stop::{Stop, StopAll, StopAny};
pub use wait::{Wait, WaitCapped, WaitChain, WaitCombine};
#[cfg(feature = "jitter")]
pub use wait::{WaitDecorrelatedJitter, WaitEqualJitter, WaitFullJitter, WaitJitter};

/// Sync retry with default policy.
///
/// Creates a [`SyncRetryBuilder`](builders::SyncRetryBuilder) using
/// [`RetryPolicy::new()`] defaults: `attempts(3)`, `exponential(100ms)`,
/// `any_error()`.
///
/// # Examples
///
/// ```
/// use tenacious::{retry, stop};
///
/// let result = retry(|_| Ok::<u32, &str>(42))
///     .stop(stop::attempts(1))
///     .sleep(|_| {})
///     .call();
/// assert_eq!(result.unwrap(), 42);
/// ```
pub fn retry<F, T, E>(
    op: F,
) -> builders::SyncRetryBuilder<
    stop::StopAfterAttempts,
    wait::WaitExponential,
    predicate::PredicateAnyError,
    (),
    (),
    (),
    F,
    policy::NoSyncSleep,
    T,
    E,
>
where
    F: FnMut(RetryState) -> Result<T, E>,
{
    builders::SyncRetryBuilder::from_policy(RetryPolicy::new(), op)
}

/// Async retry with default policy.
///
/// Creates an [`AsyncRetryBuilder`](builders::AsyncRetryBuilder) using
/// [`RetryPolicy::new()`] defaults.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use tenacious::retry_async;
///
/// let retry = retry_async(|_| async { Ok::<u32, &str>(42) })
///     .sleep(|_dur: Duration| async {});
/// let _ = retry;
/// ```
pub fn retry_async<F, T, E, Fut>(
    op: F,
) -> builders::AsyncRetryBuilder<
    stop::StopAfterAttempts,
    wait::WaitExponential,
    predicate::PredicateAnyError,
    (),
    (),
    (),
    F,
    Fut,
    policy::NoAsyncSleep,
    T,
    E,
>
where
    F: FnMut(RetryState) -> Fut,
    Fut: core::future::Future<Output = Result<T, E>>,
{
    builders::AsyncRetryBuilder::from_policy(RetryPolicy::new(), op)
}

/// Advanced builder types and aliases.
///
/// This module contains the full type-state builder matrix (default/policy
/// aliases and with-stats variants.
/// Keep imports explicit when you need these names in signatures:
/// `use tenacious::builders::DefaultSyncRetryBuilder`.
pub mod builders {
    pub use crate::policy::{
        AsyncRetryBuilder, AsyncRetryBuilderWithStats, DefaultAsyncRetryBuilder,
        DefaultAsyncRetryBuilderWithStats, DefaultSyncRetryBuilder,
        DefaultSyncRetryBuilderWithStats, SyncRetryBuilder, SyncRetryBuilderWithStats,
    };
}

/// Common traits and constructors for ergonomic imports.
///
/// The prelude intentionally exports the most common retry-building items:
/// core traits, builder entry points, terminal error/result types, and the
/// built-in stop, wait, and predicate constructors that appear most often in
/// retry chains.
///
/// It does not export the `cancel`, `predicate`, `sleep`, `stop`, or `wait` modules
/// themselves, and it leaves runtime-specific sleep helpers such as
/// `sleep::tokio()` on their modules. That keeps
/// `use tenacious::prelude::*;` useful for day-to-day call sites without
/// flattening the entire crate root into one import.
///
/// # Examples
///
/// ```
/// use tenacious::prelude::*;
/// use core::time::Duration;
///
/// let policy = RetryPolicy::new()
///     .stop(attempts(3) | elapsed(Duration::from_secs(1)))
///     .wait(exponential(Duration::from_millis(10)))
///     .when(any_error());
///
/// let result = policy.retry(|_| Err::<(), _>("fail")).sleep(|_dur| {}).call();
/// assert!(matches!(result, Err(RetryError::Exhausted { .. })));
/// ```
pub mod prelude {
    pub use crate::AsyncRetryExt;
    pub use crate::predicate::{any_error, error, ok, result};
    pub use crate::sleep::Sleeper;
    pub use crate::stop::{attempts, elapsed, never};
    pub use crate::wait::{exponential, fixed, linear};
    pub use crate::{
        Predicate, RetryError, RetryExt, RetryPolicy, RetryResult, RetryStats, Stop, StopReason,
        Wait,
    };
}
