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
//! use relentless::clock::VirtualClock;
//!
//! let result = (|| Ok::<_, &str>(42)).retry().clock(VirtualClock::new()).call();
//! assert_eq!(result.unwrap(), 42);
//! ```
//!
//! The [`retry`] and [`retry_async`] free functions are equivalent, with the
//! added ability to capture retry loop state via the [`RetryState`] argument:
//!
//! ```
//! use relentless::retry;
//! use relentless::clock::VirtualClock;
//!
//! let result = retry(|state| {
//!     if state.attempt >= 2 { Ok(state.attempt) } else { Err("not yet") }
//! })
//! .clock(VirtualClock::new())
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
//! use relentless::clock::VirtualClock;
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
//!     .clock(VirtualClock::new())
//!     .call();
//!
//! assert!(result.is_err());
//! ```
//!
//! To bound the **total** wall-clock time across all attempts and sleeps, add
//! [`.timeout(dur)`](SyncRetryBuilder::timeout). It OR-folds an elapsed deadline
//! into the stop strategy and clamps each inter-attempt sleep to the remaining
//! budget; it does not interrupt an attempt already running. A sleep clamped to
//! the last of the budget still ends with one final attempt at the deadline, so
//! total wall-clock time can exceed `dur` by roughly that attempt's duration.
//! See [Cancellation] for the runtime-agnostic deadline pattern.
//!
//! [Cancellation]: #cancellation
//!
//! # Policy reuse
//!
//! [`RetryPolicy`] captures retry rules once. Compose wait strategies with `+`
//! and stop strategies with `|` or `&`.
//!
//! ```
//! use core::time::Duration;
//! use relentless::clock::VirtualClock;
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
//! let a = policy.retry(|_| Ok::<_, &str>("a")).clock(VirtualClock::new()).call();
//! let b = policy.retry(|_| Ok::<_, &str>("b")).clock(VirtualClock::new()).call();
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
//! use relentless::clock::VirtualClock;
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
//!     .clock(VirtualClock::new())
//!     .call();
//!
//! assert_eq!(result.unwrap(), Status::Done);
//! ```
//!
//! # Hooks and stats
//!
//! ```
//! use relentless::retry;
//! use relentless::clock::VirtualClock;
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
//!     .clock(VirtualClock::new())
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
//! and rejection (predicate deemed the error non-retryable). The payloads
//! differ: `Exhausted { last }` carries the final attempt's full
//! `Result<T, E>`, because polling with [`.until()`](SyncRetryBuilder::until)
//! can exhaust while the last outcome was still `Ok` (e.g. a job stuck at
//! `Pending`). `Rejected { last }` carries the non-retryable error itself —
//! terminating on an accepted `Ok` is always a plain `Ok` return, never an
//! error:
//!
//! ```
//! use relentless::clock::VirtualClock;
//! use relentless::{retry, RetryError};
//!
//! match retry(|_| Err::<(), &str>("boom")).clock(VirtualClock::new()).call() {
//!     Ok(val) => println!("success: {val:?}"),
//!     Err(RetryError::Exhausted { last }) => {
//!         println!("gave up: {last:?}");
//!     }
//!     Err(RetryError::Rejected { last }) => {
//!         println!("non-retryable: {last}");
//!     }
//!     // `RetryError` is `#[non_exhaustive]`; match future variants here.
//!     Err(_) => {}
//! }
//! ```
//!
//! Termination is classified by the final outcome's `Result` variant, not by
//! intent. Inverted polling — retrying *until an error appears*, e.g. probing
//! for a failure — therefore reports the found error as `Rejected`:
//!
//! ```
//! use relentless::clock::VirtualClock;
//! use relentless::{retry, predicate, stop, RetryError};
//!
//! let mut attempts = 0;
//! let probe = retry(|_| {
//!     attempts += 1;
//!     if attempts >= 3 { Err("crash") } else { Ok(()) }
//! })
//! .until(predicate::result(|o: &Result<(), &str>| o.is_err()))
//! .stop(stop::attempts(10))
//! .clock(VirtualClock::new())
//! .call();
//!
//! match probe {
//!     // The failure we were looking for arrives via `Rejected`.
//!     Err(RetryError::Rejected { last }) => assert_eq!(last, "crash"),
//!     other => panic!("expected Rejected, got {other:?}"),
//! }
//! ```
//!
//! # Cancellation
//!
//! By design, the retry engine has no built-in cancellation primitive — it
//! composes with the cancellation your environment already provides, observed at
//! attempt boundaries (a running operation or sleep is never interrupted
//! mid-flight).
//!
//! - **Async:** the future returned by [`.call()`](AsyncRetryExec::call) is
//!   cancel-safe — drop it (e.g. via `tokio::time::timeout` or `select!`) to
//!   stop at the next `.await`. Note `on_exit` does **not** fire on drop; use
//!   `Drop` on your own types for guaranteed cleanup. See the `async-cancel`
//!   example.
//! - **Sync:** bound the wall-clock with [`.timeout()`](SyncRetryBuilder::timeout),
//!   or check a flag inside the operation and return a sentinel error. With the
//!   default `any_error()` predicate that sentinel is *retried*, so make it
//!   non-retryable via [`.when()`](SyncRetryBuilder::when) (it then terminates as
//!   [`RetryError::Rejected`]). See the `sync-cancel` example.
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
//! let state = RetryState::for_attempt(3);
//! assert_eq!(strategy.next_wait(&state), Duration::from_millis(50));
//! ```
//!
//! # Feature flags
//!
//! | Flag | Purpose |
//! |------|---------|
//! | `std` (default) | [`clock::SystemClock`] default for sync retries, `std::error::Error` on `RetryError` |
//! | `alloc` | Boxed policies, [`clock::VirtualClock`] wait recording |
//! | `tokio-clock` | `clock::TokioClock` async clock adapter |
//! | `embassy-clock` | `clock::EmbassyClock` async clock adapter |
//! | `gloo-timers-clock` | `clock::GlooClock` async clock adapter (wasm32; needs a caller-supplied now-source) |
//! | `futures-timer-clock` | `clock::FuturesTimerClock` async clock adapter |
//!
//! Async retry does not require `alloc`. Sync `std` builds default to
//! [`clock::SystemClock`], so `.clock(...)` is optional there.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

// Compile-test README code examples as doctests.
// Gated on `tokio-clock` because the async example uses `clock::TokioClock`.
#[cfg(all(doctest, feature = "tokio-clock"))]
#[doc = include_str!("../README.md")]
mod readme_doctests {}

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

mod compat;

pub mod clock;
mod decision;
mod error;
// The classifier-driven engine (ADR-6), built in parallel and not yet
// re-exported; the predicate engine below remains the public surface until
// cutover.
mod engine;
mod policy;
pub mod predicate;
mod state;
mod stats;
pub mod stop;
pub mod wait;

pub use clock::{AsyncClock, Clock, SyncClock};
pub use error::{RetryError, RetryResult};
pub use policy::RetryPolicy;
pub use policy::{
    AsyncRetry, AsyncRetryExec, AsyncRetryExecWithStats, AsyncRetryExt, AsyncRetryWithStats,
};
pub use policy::{
    AsyncRetryBuilder, AsyncRetryBuilderWithStats, DefaultAsyncRetryBuilder,
    DefaultAsyncRetryBuilderWithStats, DefaultSyncRetryBuilder, DefaultSyncRetryBuilderWithStats,
    SyncRetryBuilder, SyncRetryBuilderWithStats,
};
pub use policy::{RetryExt, SyncRetry, SyncRetryWithStats};
pub use policy::{SyncRetryExec, SyncRetryExecWithStats};
pub use predicate::Predicate;
pub use state::{AttemptState, ExitState, RetryState};
pub use stats::{RetryStats, StopReason};
pub use stop::Stop;
pub use wait::Wait;

/// Commonly used traits, glob-imported to enable the builder DSL.
///
/// The combinator methods (`.cap()`, `.full_jitter()`, `.chain()` on [`Wait`];
/// `.or()`/`.and()` on [`Stop`] and [`Predicate`]) and the closure extensions
/// (`.retry()` on [`RetryExt`], `.retry_async()` on [`AsyncRetryExt`]) are trait
/// methods, so the trait must be in scope to call them. Glob-import this module
/// to bring them all in at once:
///
/// ```
/// use relentless::prelude::*;
/// use relentless::wait;
/// use core::time::Duration;
///
/// // `.full_jitter()` and `.cap()` resolve without naming `Wait`.
/// let _ = wait::exponential(Duration::from_millis(100))
///     .full_jitter()
///     .cap(Duration::from_secs(5));
/// ```
///
/// Operator composition (`+` on waits, `|`/`&` on stops and predicates) works
/// without any trait imports — the prelude is only needed for the method-form
/// combinators, the closure extension traits, and implementing custom
/// strategies.
///
/// Strategy constructors (`wait::exponential`, `stop::attempts`, …) are *not*
/// re-exported here; import them explicitly by name.
pub mod prelude {
    pub use crate::{AsyncClock, AsyncRetryExt, Clock, Predicate, RetryExt, Stop, SyncClock, Wait};
}

/// Returns a [`SyncRetryBuilder`] with default policy: `attempts(3)`,
/// `exponential(100ms)`, `any_error()`.
///
/// # Examples
///
/// ```
/// use relentless::clock::VirtualClock;
/// use relentless::{retry, stop};
///
/// let result = retry(|_| Ok::<u32, &str>(42))
///     .stop(stop::attempts(1))
///     .clock(VirtualClock::new())
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
    clock::SystemClock,
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
/// use relentless::clock::VirtualClock;
/// use relentless::retry_async;
///
/// # async fn doc() {
/// // Async terminates with `.call().await` (mirroring the sync `.call()`).
/// let clock = VirtualClock::new();
/// let result = retry_async(|_| async { Ok::<u32, &str>(42) })
///     .clock(&clock)
///     .call()
///     .await;
/// assert_eq!(result.unwrap(), 42);
/// # }
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
    clock::SystemClock,
    T,
    E,
>
where
    F: FnMut(RetryState) -> Fut,
    Fut: core::future::Future<Output = Result<T, E>>,
{
    AsyncRetryBuilder::from_policy(RetryPolicy::new(), op)
}
