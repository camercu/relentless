//! `tenacious` — a Rust library for retrying fallible operations and polling for conditions.
//!
//! This crate provides composable retry strategies with support for `std`, `alloc`,
//! and `no_std` environments.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

mod compat;

pub mod error;
pub mod on;
pub mod policy;
pub mod predicate;
pub mod sleep;
pub mod state;
pub mod stats;
pub mod stop;
pub mod wait;

// Re-export core public types at the crate root (spec 10.1).
pub use error::RetryError;
#[cfg(feature = "alloc")]
pub use policy::BoxedRetryPolicy;
pub use policy::RetryPolicy;
pub use predicate::Predicate;
pub use sleep::Sleeper;
pub use state::{AttemptState, BeforeAttemptState, RetryState};
pub use stats::{RetryStats, StopReason};
pub use stop::{Stop, StopAll, StopAny};
pub use wait::{Wait, WaitCapped, WaitChain, WaitCombine};
