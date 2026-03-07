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
#[cfg(feature = "alloc")]
pub use policy::{AsyncRetry, AsyncRetryWithStats};
#[cfg(feature = "alloc")]
pub use policy::{AsyncRetryBuilder, AsyncRetryBuilderWithStats, AsyncRetryExt};
pub use policy::{
    RetryExt, SyncRetry, SyncRetryBuilder, SyncRetryBuilderWithStats, SyncRetryWithStats,
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
    #[cfg(feature = "alloc")]
    pub use crate::AsyncRetryExt;
    pub use crate::on::{any_error, error, ok, until_ready};
    pub use crate::sleep::Sleeper;
    pub use crate::stop::{attempts, elapsed};
    pub use crate::wait::{exponential, fixed};
    pub use crate::{
        Canceler, Predicate, PredicateExt, RetryError, RetryExt, RetryPolicy, RetryResult,
        RetryStats, Stop, StopExt, StopReason, Wait, WaitExt,
    };
}
