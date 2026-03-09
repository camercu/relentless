//! `tenacious` — a Rust library for retrying fallible operations and polling for conditions.
//!
//! This crate provides composable retry strategies with support for `std`, `alloc`,
//! and `no_std` environments.
//!
//! # Custom wait strategies
//!
//! ```
//! use core::time::Duration;
//! use tenacious::{RetryState, Wait, WaitExt, wait};
//!
//! struct CustomWait(Duration);
//!
//! impl Wait for CustomWait {
//!     fn next_wait(&mut self, _state: &RetryState) -> Duration {
//!         self.0
//!     }
//! }
//!
//! let mut strategy = CustomWait(Duration::from_millis(20))
//!     .cap(Duration::from_millis(15))
//!     .chain(wait::fixed(Duration::from_millis(50)), 2);
//!
//! let state = RetryState::new(3, None, Duration::ZERO, Duration::ZERO);
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

pub mod cancel;
mod error;
pub mod on;
mod policy;
mod predicate;
pub mod sleep;
mod state;
mod stats;
pub mod stop;
pub mod wait;

// Re-export core public types at the crate root (spec 10.1).
pub use cancel::{Canceler, NeverCancel};
pub use error::{RetryError, RetryResult};
#[cfg(feature = "alloc")]
pub use policy::BoxedRetryPolicy;
pub use policy::RetryPolicy;
pub use policy::{AsyncRetry, AsyncRetryWithStats};
pub use policy::{
    AsyncRetryBuilder, AsyncRetryBuilderWithStats, AsyncRetryExt, DefaultAsyncRetryBuilder,
    PolicyAsyncRetryBuilder,
};
pub use policy::{
    DefaultSyncRetryBuilder, PolicySyncRetryBuilder, RetryExt, SyncRetry, SyncRetryBuilder,
    SyncRetryBuilderWithStats, SyncRetryWithStats,
};
pub use predicate::{Predicate, PredicateExt};
pub use sleep::Sleeper;
pub use state::{AttemptState, BeforeAttemptState, ExitState, RetryState};
pub use stats::{RetryStats, StopReason};
pub use stop::{NeedsStop, Stop, StopAll, StopAny, StopConfigError, StopExt};
#[cfg(feature = "jitter")]
pub use wait::WaitJitter;
pub use wait::{Wait, WaitCapped, WaitChain, WaitCombine, WaitExt};

/// Common traits and constructors for ergonomic imports.
///
/// The prelude intentionally exports the most common retry-building items:
/// core traits, builder entry points, terminal error/result types, and the
/// built-in stop, wait, and predicate constructors that appear most often in
/// retry chains.
///
/// It does not export the `cancel`, `on`, `sleep`, `stop`, or `wait` modules
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
/// let mut policy = RetryPolicy::new()
///     .stop(attempts(3) | elapsed(Duration::from_secs(1)))
///     .wait(exponential(Duration::from_millis(10)))
///     .when(any_error());
///
/// let result = policy.retry(|| Err::<(), _>("fail")).sleep(|_dur| {}).call();
/// assert!(matches!(result, Err(RetryError::Exhausted { attempts: 3, .. })));
/// ```
pub mod prelude {
    pub use crate::AsyncRetryExt;
    pub use crate::on::{any_error, error, ok, result};
    pub use crate::sleep::Sleeper;
    pub use crate::stop::{attempts, before_elapsed, elapsed, never};
    pub use crate::wait::{exponential, fixed, linear};
    pub use crate::{
        Canceler, Predicate, PredicateExt, RetryError, RetryExt, RetryPolicy, RetryResult,
        RetryStats, Stop, StopExt, StopReason, Wait, WaitExt,
    };
}
