//! The unified clock abstraction: one injected value that owns both "what time
//! is it" and "wait this long".
//!
//! The retry engines read elapsed time and perform inter-attempt waits through
//! a single clock value, so the two can never disagree: whatever advances time
//! is the same value that reports it. This makes the classic footgun — a sleep
//! source and an elapsed clock wired from different places, silently desyncing
//! `timeout`/`stop::elapsed` from the recorded waits — unrepresentable.
//!
//! Capability is type-visible, split into sibling traits over a read-only base:
//!
//! - [`Clock`] — the read seam: `now()`.
//! - [`SyncClock`] — adds the blocking wait used by the sync engine.
//! - [`AsyncClock`] — adds the future-producing wait used by the async engine.
//!
//! A sync-only clock (e.g. [`SystemClock`]) does not implement [`AsyncClock`],
//! so the async engine rejects it at compile time — it can never silently no-op
//! an async wait.
//!
//! [`VirtualClock`] is the deterministic clock for tests: waits advance virtual
//! time instead of sleeping, and reads report that same virtual time.
//!
//! # Implementing your own clock
//!
//! Implement [`Clock`] plus the capability trait(s) for the engines you
//! target. The contract: `now()` is monotonically non-decreasing, and a
//! completed wait is reflected in subsequent `now()` readings (see
//! [`Clock::now`]). For an async clock whose wait future needs access to the
//! clock's own state, implement [`AsyncClock`] on the *reference* type
//! (`impl AsyncClock for &MyClock`) so the returned future can borrow the
//! clock without a generic associated type — this is how [`VirtualClock`]
//! does it (its owned form stays sync-only, so the direct reference impl
//! does not overlap the blanket `AsyncClock for &C`). Production runtime
//! clocks usually return the runtime's own owned timer future instead and
//! implement [`AsyncClock`] directly.

use crate::compat::Duration;
use core::future::Future;

mod system_clock;
mod virtual_clock;

#[cfg(feature = "embassy-clock")]
mod embassy_clock;
#[cfg(feature = "futures-timer-clock")]
mod futures_timer_clock;
#[cfg(all(feature = "gloo-timers-clock", target_arch = "wasm32"))]
mod gloo_clock;
#[cfg(feature = "tokio-clock")]
mod tokio_clock;

pub use system_clock::SystemClock;
pub use virtual_clock::{VirtualClock, VirtualWait};

#[cfg(feature = "embassy-clock")]
pub use embassy_clock::EmbassyClock;
#[cfg(feature = "futures-timer-clock")]
pub use futures_timer_clock::FuturesTimerClock;
#[cfg(all(feature = "gloo-timers-clock", target_arch = "wasm32"))]
pub use gloo_clock::GlooClock;
#[cfg(feature = "tokio-clock")]
pub use tokio_clock::TokioClock;

/// The read seam: reports the current time as a [`Duration`] since an
/// arbitrary fixed origin.
///
/// `now()` must be monotonically non-decreasing. The retry engines capture a
/// baseline reading when execution starts and compute elapsed time as
/// `now() - baseline`, so the absolute origin never matters.
///
/// Implementations that also perform waits ([`SyncClock`] / [`AsyncClock`])
/// must advance `now()` by (at least) the waited duration once the wait
/// completes — that coherence is the point of the unified clock. A real OS
/// clock gets this from the scheduler; a virtual clock must couple the two
/// through shared state, as [`VirtualClock`] does.
///
/// Every real timer has a longest wait it can express, so that guarantee is
/// bounded by a platform ceiling: beyond it a wait may complete early rather
/// than panicking or firing immediately. [`TokioClock`] documents where its
/// own sits; the others inherit whatever their runtime provides and make no
/// promise about it. Bound the strategy with `.cap(...)` if a run must not
/// depend on where that ceiling falls.
pub trait Clock {
    /// Returns the current time since the clock's origin.
    fn now(&self) -> Duration;
}

/// Adds the blocking wait used by the synchronous retry engine.
///
/// A sibling of [`AsyncClock`], not its supertrait: a sync-only clock carries
/// no async surface, and the async engine's `AsyncClock` bound rejects it at
/// compile time.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot drive the synchronous retry engine",
    label = "not a `SyncClock`",
    note = "supply a clock with `.clock(...)` before `.call()` — e.g. \
            `clock::VirtualClock` for tests; under `std` the default \
            `SystemClock` already satisfies this"
)]
pub trait SyncClock: Clock {
    /// Waits for `dur`: blocks the thread on a real clock, or advances virtual
    /// time on a test clock. Afterwards, [`now()`](Clock::now) reflects the
    /// wait.
    ///
    /// The engine never calls this with [`Duration::ZERO`]. It skips the clock
    /// entirely for a zero delay, so an implementation counting calls here
    /// counts performed sleeps, not inter-attempt gaps.
    fn wait(&self, dur: Duration);
}

/// Adds the future-producing wait used by the asynchronous retry engine.
///
/// A sibling of [`SyncClock`]: an async-only clock (e.g. a runtime timer) is
/// not forced to carry a blocking wait.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot drive the asynchronous retry engine",
    label = "not an `AsyncClock`",
    note = "supply an async clock with `.clock(...)` before `.call()` — e.g. \
            `clock::TokioClock` (feature `tokio-clock`) in production or \
            `&VirtualClock` in tests; there is no async default"
)]
pub trait AsyncClock: Clock {
    /// The concrete future returned by [`wait_async`](AsyncClock::wait_async) —
    /// a runtime's named timer future in production, or a poll-advancing
    /// virtual wait for a test clock.
    type Wait: Future<Output = ()>;

    /// Returns a future that, when awaited, waits for `dur`. Once it
    /// completes, [`now()`](Clock::now) reflects the wait.
    ///
    /// The wait must take effect when the future is *polled*, not when it is
    /// created: the engine may build a wait future and drop it unpolled (e.g.
    /// when cancelled), and an unpolled wait must not advance time.
    ///
    /// The engine never calls this with [`Duration::ZERO`]. It skips the clock
    /// for a zero delay and yields cooperatively instead, so an implementation
    /// counting calls here counts performed sleeps, not inter-attempt gaps.
    fn wait_async(&self, dur: Duration) -> Self::Wait;
}

impl<C: Clock + ?Sized> Clock for &C {
    fn now(&self) -> Duration {
        (**self).now()
    }
}

impl<C: SyncClock + ?Sized> SyncClock for &C {
    fn wait(&self, dur: Duration) {
        (**self).wait(dur);
    }
}

impl<C: AsyncClock + ?Sized> AsyncClock for &C {
    type Wait = C::Wait;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        (**self).wait_async(dur)
    }
}
