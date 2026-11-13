//! The classifier-driven retry engine.
//!
//! This is the crate's retry engine, exposed through [`retry`]/[`retry_async`]
//! and [`crate::RetryPolicy`]. It uses the outcome-agnostic [`Stop`](crate::Stop),
//! [`Wait`](crate::Wait), [`SyncClock`](crate::SyncClock), and
//! [`RetryState`](crate::RetryState), and drives retries with a
//! [`Decide`](crate::Decide) classifier that consumes each outcome by value.
//!
//! One builder, [`Retry`], serves both modes; its [`Blocking`] or [`Async`]
//! marker selects the sync or async `.call()`.

mod async_engine;
mod builder;
mod elapsed;
mod error;
mod op;
mod stats;
mod step;
mod sync_engine;
mod termination;

pub use async_engine::{AsyncRun, DropStats, retry_async};
pub use builder::Retry;
pub use error::{RetryError, RetryResult};
pub use op::{Async, Attempt, Blocking, Operation};
pub use stats::RetryStats;
pub use sync_engine::retry;
pub use termination::StopReason;
