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
//! let state = RetryState {
//!     attempt: 3,
//!     elapsed: None,
//!     next_delay: Duration::ZERO,
//!     total_wait: Duration::ZERO,
//! };
//! assert_eq!(strategy.next_wait(&state), Duration::from_millis(50));
//! ```

#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

mod compat;

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
pub use error::RetryError;
#[cfg(feature = "alloc")]
pub use policy::BoxedRetryPolicy;
pub use policy::RetryPolicy;
#[cfg(feature = "alloc")]
pub use policy::{AsyncRetry, AsyncRetryWithStats};
pub use policy::{SyncRetry, SyncRetryWithStats};
pub use predicate::Predicate;
pub use sleep::Sleeper;
pub use state::{AttemptState, BeforeAttemptState, RetryState};
pub use stats::{RetryStats, StopReason};
pub use stop::{NeedsStop, Stop, StopAll, StopAny, StopConfigError};
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
    pub use crate::on::{any_error, error, ok};
    pub use crate::sleep::Sleeper;
    pub use crate::stop::{attempts, elapsed};
    pub use crate::wait::{exponential, fixed};
    pub use crate::{
        Predicate, RetryError, RetryPolicy, RetryStats, Stop, StopReason, Wait, WaitExt,
    };
}
