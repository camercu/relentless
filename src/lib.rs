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

// Compile-test README code examples as doctests.
// Gated on `tokio-sleep` because the async example uses `sleep::tokio()`.
#[cfg(all(doctest, feature = "tokio-sleep"))]
#[doc = include_str!("../README.md")]
mod readme_doctests {}

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

pub use error::{RetryError, RetryResult};
pub use policy::RetryPolicy;
pub use policy::{AsyncRetry, AsyncRetryExt, AsyncRetryWithStats};
pub use policy::{
    AsyncRetryBuilder, AsyncRetryBuilderWithStats, DefaultAsyncRetryBuilder,
    DefaultAsyncRetryBuilderWithStats, DefaultSyncRetryBuilder, DefaultSyncRetryBuilderWithStats,
    SyncRetryBuilder, SyncRetryBuilderWithStats,
};
pub use policy::{RetryExt, SyncRetry, SyncRetryWithStats};
pub use predicate::Predicate;
pub use sleep::Sleeper;
pub use state::{AttemptState, ExitState, RetryState};
pub use stats::{RetryStats, StopReason};
pub use stop::Stop;
pub use wait::Wait;

/// Returns a [`SyncRetryBuilder`] with default policy: `attempts(3)`,
/// `exponential(100ms)`, `any_error()`.
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
) -> SyncRetryBuilder<
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
    SyncRetryBuilder::from_policy(RetryPolicy::new(), op)
}

/// Returns an [`AsyncRetryBuilder`] with default policy: `attempts(3)`,
/// `exponential(100ms)`, `any_error()`.
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
) -> AsyncRetryBuilder<
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
    AsyncRetryBuilder::from_policy(RetryPolicy::new(), op)
}
