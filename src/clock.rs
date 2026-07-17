//! The unified clock abstraction: one injected value that owns both "what time
//! is it" and "wait this long" (ADR-0005).
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

use crate::compat::Duration;
use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

#[cfg(feature = "alloc")]
use core::cell::RefCell;

#[cfg(feature = "alloc")]
use crate::compat::Vec;

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
pub trait Clock {
    /// Returns the current time since the clock's origin.
    fn now(&self) -> Duration;
}

/// Adds the blocking wait used by the synchronous retry engine.
///
/// A sibling of [`AsyncClock`], not its supertrait: a sync-only clock carries
/// no async surface, and the async engine's `AsyncClock` bound rejects it at
/// compile time.
pub trait SyncClock: Clock {
    /// Waits for `dur`: blocks the thread on a real clock, or advances virtual
    /// time on a test clock. Afterwards, [`now()`](Clock::now) reflects the
    /// wait.
    fn wait(&self, dur: Duration);
}

/// Adds the future-producing wait used by the asynchronous retry engine.
///
/// A sibling of [`SyncClock`]: an async-only clock (e.g. a runtime timer) is
/// not forced to carry a blocking wait.
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

/// The default wall-time clock for synchronous execution.
///
/// With the `std` feature it reads a process-global monotonic anchor
/// (`std::time::Instant`) and waits with `std::thread::sleep`. Without `std`
/// the type still exists — it is the builders' "no clock configured yet"
/// default — but implements no clock capability, so `.call()` is unavailable
/// until a real clock is supplied via `.clock(...)`.
///
/// `SystemClock` is deliberately sync-only: it does not implement
/// [`AsyncClock`], so the async engine rejects it at compile time instead of
/// letting a blocking clock stall a reactor thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemClock;

#[cfg(feature = "std")]
impl Clock for SystemClock {
    fn now(&self) -> Duration {
        use std::sync::OnceLock;
        use std::time::Instant;

        // Process-global origin: `now()` values are only ever subtracted from
        // a baseline read from the same clock, so the anchor point is
        // arbitrary — it just has to be fixed.
        static ANCHOR: OnceLock<Instant> = OnceLock::new();
        ANCHOR.get_or_init(Instant::now).elapsed()
    }
}

#[cfg(feature = "std")]
impl SyncClock for SystemClock {
    fn wait(&self, dur: Duration) {
        std::thread::sleep(dur);
    }
}

/// A deterministic clock for testing retry behavior without real sleeping.
///
/// Waits advance virtual time by exactly the requested amount, and
/// [`now()`](Clock::now) reads the very cell the waits advance — one cell, one
/// writer, so the read seam and the wait seam cannot drift even by an
/// implementation bug.
///
/// An owned `VirtualClock` is a [`SyncClock`]; a shared borrow
/// (`&VirtualClock`) is additionally an [`AsyncClock`] whose wait future
/// advances time on first poll. Hand the engine a borrow and keep the handle
/// for assertions:
///
/// ```
/// use core::time::Duration;
/// use relentless::clock::{Clock, SyncClock, VirtualClock};
///
/// let clock = VirtualClock::new();
/// clock.wait(Duration::from_millis(50));
/// assert_eq!(clock.now(), Duration::from_millis(50));
/// ```
#[derive(Debug, Default)]
pub struct VirtualClock {
    /// Single source of truth for virtual "now". Written only by
    /// [`advance`](Self::advance) (and the wait paths that funnel through it),
    /// read only by `now()` — same cell, so read and wait cannot desync.
    now: Cell<Duration>,
    /// Test-only record of requested waits; not part of the time coupling.
    #[cfg(feature = "alloc")]
    waits: RefCell<Vec<Duration>>,
}

impl VirtualClock {
    /// Creates a virtual clock at time zero with no recorded waits.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Advances virtual time by `dur` without recording a wait.
    ///
    /// Use inside an operation to simulate attempts that consume the elapsed
    /// budget (e.g. slow calls) without any waiting between attempts.
    /// Saturates at [`Duration::MAX`].
    pub fn advance(&self, dur: Duration) {
        self.now.set(self.now.get().saturating_add(dur));
    }

    /// Returns every wait requested so far, in request order.
    ///
    /// A point-in-time snapshot: the returned `Vec` is unaffected by waits
    /// recorded after the call.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn waits(&self) -> Vec<Duration> {
        self.waits.borrow().clone()
    }

    /// Advances virtual time and records the wait. Both wait seams funnel
    /// through here, so recording and advancing happen together, once.
    fn record_wait(&self, dur: Duration) {
        self.advance(dur);
        #[cfg(feature = "alloc")]
        self.waits.borrow_mut().push(dur);
    }
}

impl Clock for VirtualClock {
    fn now(&self) -> Duration {
        self.now.get()
    }
}

impl SyncClock for VirtualClock {
    fn wait(&self, dur: Duration) {
        // No blocking: virtual time simply jumps.
        self.record_wait(dur);
    }
}

impl<'clock> AsyncClock for &'clock VirtualClock {
    type Wait = VirtualWait<'clock>;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        VirtualWait {
            clock: self,
            dur,
            advanced: false,
        }
    }
}

/// Future returned by [`VirtualClock`]'s [`AsyncClock::wait_async`].
///
/// Advances virtual time on its *first poll*, then resolves immediately. A
/// dropped, never-polled wait therefore leaves time untouched — matching real
/// runtime timer futures, which only take effect once polled.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct VirtualWait<'clock> {
    clock: &'clock VirtualClock,
    dur: Duration,
    advanced: bool,
}

impl Future for VirtualWait<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if !self.advanced {
            self.advanced = true;
            self.clock.record_wait(self.dur);
        }
        Poll::Ready(())
    }
}

/// An [`AsyncClock`] backed by the Tokio runtime's timer.
///
/// `now()` reads [`tokio::time::Instant`] and `wait_async` is
/// [`tokio::time::sleep`], so both seams follow Tokio's virtual time: under
/// `tokio::time::pause` (a `test-util` API of Tokio) the waits and the
/// elapsed reads stay coherent by construction, with no separate wiring.
#[cfg(feature = "tokio-sleep")]
#[derive(Debug, Clone, Copy)]
pub struct TokioClock {
    origin: tokio::time::Instant,
}

#[cfg(feature = "tokio-sleep")]
impl TokioClock {
    /// Creates a clock anchored at the current Tokio instant.
    ///
    /// Must be called within a Tokio runtime context (as must the waits).
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

#[cfg(feature = "tokio-sleep")]
impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "tokio-sleep")]
impl Clock for TokioClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[cfg(feature = "tokio-sleep")]
impl AsyncClock for TokioClock {
    type Wait = tokio::time::Sleep;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        tokio::time::sleep(dur)
    }
}

/// An [`AsyncClock`] backed by [`embassy_time`]'s tick clock and timer.
///
/// `now()` reads [`embassy_time::Instant`] and `wait_async` is
/// [`embassy_time::Timer::after`], both driven by the linked embassy time
/// driver.
#[cfg(feature = "embassy-sleep")]
#[derive(Debug, Clone, Copy)]
pub struct EmbassyClock {
    origin: embassy_time::Instant,
}

#[cfg(feature = "embassy-sleep")]
impl EmbassyClock {
    /// Creates a clock anchored at the current embassy instant.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: embassy_time::Instant::now(),
        }
    }
}

#[cfg(feature = "embassy-sleep")]
impl Default for EmbassyClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "embassy-sleep")]
impl Clock for EmbassyClock {
    fn now(&self) -> Duration {
        Duration::from_micros(self.origin.elapsed().as_micros())
    }
}

#[cfg(feature = "embassy-sleep")]
impl AsyncClock for EmbassyClock {
    type Wait = embassy_time::Timer;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        embassy_time::Timer::after(to_embassy_duration(dur))
    }
}

/// Embassy counts ticks in a `u64`; saturate rather than panic on very large
/// durations.
///
/// Computes ticks in `u128` (mirroring `embassy_time::Duration::from_micros`,
/// including its round-up-to-a-tick behavior) because Embassy's own `u64`
/// conversion arithmetic overflows near `u64::MAX` microseconds.
#[cfg(feature = "embassy-sleep")]
pub(crate) fn to_embassy_duration(dur: Duration) -> embassy_time::Duration {
    const MICROS_PER_SEC: u128 = 1_000_000;
    let ticks_ceil = dur
        .as_micros()
        .saturating_mul(u128::from(embassy_time::TICK_HZ))
        .div_ceil(MICROS_PER_SEC);
    let ticks = u64::try_from(ticks_ceil).unwrap_or(u64::MAX);
    embassy_time::Duration::from_ticks(ticks)
}

/// An [`AsyncClock`] backed by [`futures_timer::Delay`] and `std`'s
/// monotonic [`std::time::Instant`].
///
/// `futures-timer` waits on real wall time, which `Instant` also measures, so
/// the pairing is coherent.
#[cfg(feature = "futures-timer-sleep")]
#[derive(Debug, Clone, Copy)]
pub struct FuturesTimerClock {
    origin: std::time::Instant,
}

#[cfg(feature = "futures-timer-sleep")]
impl FuturesTimerClock {
    /// Creates a clock anchored at the current instant.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

#[cfg(feature = "futures-timer-sleep")]
impl Default for FuturesTimerClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "futures-timer-sleep")]
impl Clock for FuturesTimerClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[cfg(feature = "futures-timer-sleep")]
impl AsyncClock for FuturesTimerClock {
    type Wait = futures_timer::Delay;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        futures_timer::Delay::new(dur)
    }
}

/// An [`AsyncClock`] pairing `gloo-timers` waits with a caller-supplied `now`
/// source (wasm32 only).
///
/// Wasm has no `std::time::Instant`, so the monotonic reader must be supplied
/// explicitly — e.g. a `js_sys`/`web_sys` `performance.now()` shim converted
/// to a [`Duration`]. The supplied function must be monotonically
/// non-decreasing and must observe real time passing during the `gloo` waits,
/// or `timeout`/`stop::elapsed` will misbehave.
#[cfg(all(feature = "gloo-timers-sleep", target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy)]
pub struct GlooClock {
    now: fn() -> Duration,
}

#[cfg(all(feature = "gloo-timers-sleep", target_arch = "wasm32"))]
impl GlooClock {
    /// Creates a clock waiting through `gloo-timers` and reading `now`.
    #[must_use]
    pub fn with_now(now: fn() -> Duration) -> Self {
        Self { now }
    }
}

#[cfg(all(feature = "gloo-timers-sleep", target_arch = "wasm32"))]
impl Clock for GlooClock {
    fn now(&self) -> Duration {
        (self.now)()
    }
}

#[cfg(all(feature = "gloo-timers-sleep", target_arch = "wasm32"))]
impl AsyncClock for GlooClock {
    type Wait = gloo_timers::future::TimeoutFuture;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        gloo_timers::future::sleep(dur)
    }
}
