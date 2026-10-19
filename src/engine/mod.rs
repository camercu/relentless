//! The classifier-driven retry engine.
//!
//! This is the crate's retry engine, exposed through [`retry`]/[`retry_async`],
//! the [`RetryExt`]/[`AsyncRetryExt`] closure extensions, and
//! [`crate::RetryPolicy`]. It uses the outcome-agnostic [`Stop`], [`Wait`],
//! [`SyncClock`], and [`RetryState`], and drives retries with a [`Decide`]
//! classifier that consumes each outcome by value, rather than a boolean
//! predicate over `Result`.
//!
//! Both the sync ([`Retry`]) and async ([`AsyncRetry`]) paths carry the full
//! classifier surface — `.decide`/`.when`/`.until`, hooks, stats, and timeout.

mod async_engine;
mod elapsed;
mod error;
mod hooks;
mod op;
mod stats;
mod step;
mod sync_engine;
mod termination;

pub use async_engine::{
    AsyncRetry, AsyncRetryExt, AsyncRetryWithStats, AsyncRun, DropStats, retry_async,
};
pub use error::{RetryError, RetryResult};
pub use hooks::{AttemptHook, AttemptState, BeforeAttemptHook, Exit, ExitHook, HookChain};
pub use op::{AsyncRetryOp, RetryOp, StatelessOp};
pub use stats::RetryStats;
pub use sync_engine::{Retry, RetryExt, RetryWithStats, retry};
pub use termination::StopReason;
