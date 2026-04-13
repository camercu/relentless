//! Retry and polling for Rust — with composable strategies, policy reuse, and
//! first-class support for polling workflows where `Ok(_)` doesn't always mean
//! "done."
//!
//! Most retry libraries handle the simple case well: call a function, retry on
//! error, back off. `relentless` handles that too, but it also handles the cases
//! those libraries make awkward:
//!
//! - **Polling**, where `Ok("pending")` means "keep going" and you need
//!   [`.until(predicate)`](SyncRetryBuilder::until) rather than just retrying errors.
//! - **Policy reuse**, where a single [`RetryPolicy`] captures your retry rules and
//!   gets shared across multiple call sites — no duplicated builder chains.
//! - **Strategy composition**, where
//!   `wait::fixed(50ms) + wait::exponential(100ms)` and
//!   `stop::attempts(5) | stop::elapsed(2s)` express complex behavior in one line.
//! - **Hooks and stats**, where you observe the retry lifecycle (logging, metrics)
//!   without restructuring your retry logic.
//!
//! All of this works in sync and async code, across `std`, `no_std`, and `wasm`
//! targets.
//!
//! # Quick start
//!
//! The [`RetryExt`] extension trait is the fastest way to add retries. Defaults:
//! 3 attempts, exponential backoff from 100 ms, retry on any `Err`.
//!
//! ```
//! use relentless::RetryExt;
//!
//! let result = (|| Ok::<_, &str>(42)).retry().sleep(|_| {}).call();
//! assert_eq!(result.unwrap(), 42);
//! ```
//!
//! The [`retry`] and [`retry_async`] free functions are equivalent, with the
//! added ability to capture retry loop state via the [`RetryState`] argument:
//!
//! ```
//! use relentless::retry;
//!
//! let result = retry(|state| {
//!     if state.attempt >= 2 { Ok(state.attempt) } else { Err("not yet") }
//! })
//! .sleep(|_| {})
//! .call();
//!
//! assert_eq!(result.unwrap(), 2);
//! ```
//!
//! # Customizing retry behavior
//!
//! Builder methods control **when** to retry, **how long** to wait, and **when**
//! to stop:
//!
//! ```
//! use core::time::Duration;
//! use relentless::{Wait, retry, predicate, stop, wait};
//!
//! let result = retry(|_| Err::<(), &str>("boom"))
//!     .when(predicate::error(|e: &&str| *e == "boom"))
//!     .wait(
//!         wait::exponential(Duration::from_millis(100))
//!             .full_jitter()
//!             .cap(Duration::from_secs(5)),
//!     )
//!     .stop(stop::attempts(3))
//!     .sleep(|_| {})
//!     .call();
//!
//! assert!(result.is_err());
//! ```
//!
//! # Policy reuse
//!
//! [`RetryPolicy`] captures retry rules once. Compose wait strategies with `+`
//! and stop strategies with `|` or `&`.
//!
//! ```
//! use core::time::Duration;
//! use relentless::{RetryPolicy, stop, wait};
//!
//! let policy = RetryPolicy::new()
//!     .wait(
//!         wait::fixed(Duration::from_millis(50))
//!             + wait::exponential(Duration::from_millis(100)),
//!     )
//!     .stop(stop::attempts(5) | stop::elapsed(Duration::from_secs(30)));
//!
//! // Same policy, different operations.
//! let a = policy.retry(|_| Ok::<_, &str>("a")).sleep(|_| {}).call();
//! let b = policy.retry(|_| Ok::<_, &str>("b")).sleep(|_| {}).call();
//!
//! assert_eq!(a.unwrap(), "a");
//! assert_eq!(b.unwrap(), "b");
//! ```
//!
//! # Polling for a condition
//!
//! Use [`.until(predicate)`](SyncRetryBuilder::until) to keep retrying until a
//! success condition is met. Unlike [`.when()`](SyncRetryBuilder::when), which
//! retries on matching outcomes, `.until()` retries on everything *except* the
//! matching outcome.
//!
//! ```
//! use relentless::{RetryPolicy, predicate};
//!
//! #[derive(Debug, PartialEq)]
//! enum Status { Pending, Done }
//!
//! let mut count = 0;
//! let result = RetryPolicy::new()
//!     .until(predicate::ok(|s: &Status| *s == Status::Done))
//!     .retry(|_| {
//!         count += 1;
//!         Ok::<_, &str>(if count >= 2 { Status::Done } else { Status::Pending })
//!     })
//!     .sleep(|_| {})
//!     .call();
//!
//! assert_eq!(result.unwrap(), Status::Done);
//! ```
//!
//! # Hooks and stats
//!
//! ```
//! use relentless::retry;
//!
//! let (result, stats) = retry(|_| Ok::<_, &str>("done"))
//!     .before_attempt(|state| {
//!         if state.attempt > 1 {
//!             println!("retrying (attempt {})", state.attempt);
//!         }
//!     })
//!     .after_attempt(|state| {
//!         if let Err(e) = state.outcome {
//!             eprintln!("attempt {} failed: {e}", state.attempt);
//!         }
//!     })
//!     .sleep(|_| {})
//!     .with_stats()
//!     .call();
//!
//! println!("attempts: {}, total wait: {:?}", stats.attempts, stats.total_wait);
//! ```
//!
//! # Error handling
//!
//! The retry loop returns `Ok(T)` on success. On failure it returns
//! [`RetryError`], which distinguishes between exhaustion (stop strategy fired)
//! and rejection (predicate deemed the error non-retryable):
//!
//! ```
//! use relentless::{retry, RetryError};
//!
//! match retry(|_| Err::<(), &str>("boom")).sleep(|_| {}).call() {
//!     Ok(val) => println!("success: {val:?}"),
//!     Err(RetryError::Exhausted { last }) => {
//!         println!("gave up: {last:?}");
//!     }
//!     Err(RetryError::Rejected { last }) => {
//!         println!("non-retryable: {last}");
//!     }
//! }
//! ```
//!
//! # Custom wait strategies
//!
//! Implement [`Wait`] to build your own wait strategies. All combinators
//! ([`.cap()`](Wait::cap), [`.full_jitter()`](Wait::full_jitter),
//! [`.chain()`](Wait::chain), `+`) work on any `Wait` implementor.
//!
//! ```
//! use core::time::Duration;
//! use relentless::{RetryState, Wait, wait};
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
//! # Feature flags
//!
//! | Flag | Purpose |
//! |------|---------|
//! | `std` (default) | `std::thread::sleep` fallback, `Instant` elapsed clock, `std::error::Error` on `RetryError` |
//! | `alloc` | Boxed policies, closure elapsed clocks, multiple hooks per point |
//! | `tokio-sleep` | `sleep::tokio()` async sleep adapter |
//! | `embassy-sleep` | `sleep::embassy()` async sleep adapter |
//! | `gloo-timers-sleep` | `sleep::gloo()` async sleep adapter (wasm32) |
//! | `futures-timer-sleep` | `sleep::futures_timer()` async sleep adapter |
//!
//! Async retry does not require `alloc`. Sync `std` builds automatically fall
//! back to `std::thread::sleep`, so `.sleep(...)` is optional.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

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
/// Async sleep abstractions used by the retry engine between attempts.
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
/// use relentless::{retry, stop};
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
/// use relentless::retry_async;
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
