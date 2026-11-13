//! The classifier-driven retry engine.
//!
//! This is the crate's retry engine, exposed through [`retry`]/[`retry_async`]
//! and [`crate::RetryPolicy`]. It uses the outcome-agnostic [`Stop`](crate::Stop),
//! [`Wait`](crate::Wait), [`SyncClock`](crate::SyncClock), and
//! [`RetryState`](crate::RetryState), and drives retries with a
//! [`Decide`](crate::Decide) classifier that consumes each outcome by value.
//!
//! Both the sync ([`Retry`]) and async ([`AsyncRetry`]) paths carry the full
//! classifier surface — `.decide`/`.when`/`.until`, stats, and timeout.

mod async_engine;
mod elapsed;
mod error;
mod op;
mod stats;
mod step;
mod sync_engine;
mod termination;

pub use async_engine::{AsyncRetry, AsyncRun, DropStats, retry_async};
pub use error::{RetryError, RetryResult};
pub use op::{Async, Attempt, Blocking, Operation};
pub use stats::RetryStats;
pub use sync_engine::{Retry, retry};
pub use termination::StopReason;
