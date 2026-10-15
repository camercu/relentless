//! The deterministic test clock and its async wait future.

use super::{AsyncClock, Clock, SyncClock};
use crate::compat::Duration;
use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

#[cfg(feature = "alloc")]
use crate::compat::Vec;
#[cfg(feature = "alloc")]
use core::cell::RefCell;

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
///
/// The async engine takes the clock the same way — **by shared reference**, so
/// its wait future can borrow the clock's virtual time from a scope outliving
/// the retry future. Pass `.clock(&clock)` and keep `clock` for assertions:
///
/// ```
/// use core::time::Duration;
/// use relentless::clock::{Clock, VirtualClock};
/// use relentless::{retry_async, stop, wait};
/// # // Minimal executor: `VirtualClock`'s async waits resolve on first poll,
/// # // so a no-op waker suffices — no runtime needed.
/// # fn block_on<F: core::future::Future>(future: F) -> F::Output {
/// #     use core::task::{Context, Poll};
/// #     let mut future = core::pin::pin!(future);
/// #     let mut cx = Context::from_waker(core::task::Waker::noop());
/// #     loop {
/// #         if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
/// #             return out;
/// #         }
/// #     }
/// # }
/// let clock = VirtualClock::new();
/// let result = block_on(
///     retry_async(|_| async { Err::<(), &str>("busy") })
///         .stop(stop::attempts(3))
///         .wait(wait::fixed(Duration::from_millis(10)))
///         .clock(&clock) // shared borrow, not `VirtualClock::new()`
///         .call(),
/// );
/// assert!(result.is_err());
/// // Keep `clock` to assert after the run: two 10ms inter-attempt waits
/// // between three attempts advanced virtual time by 20ms (no real sleeping).
/// assert_eq!(clock.now(), Duration::from_millis(20));
/// ```
///
/// Passing an *owned* `VirtualClock` to the async engine is rejected at compile
/// time — it is not an [`AsyncClock`], only `&VirtualClock` is:
///
/// ```compile_fail
/// use relentless::clock::VirtualClock;
/// use relentless::{retry_async, stop};
///
/// let _ = retry_async(|_| async { Ok::<(), &str>(()) })
///     .stop(stop::attempts(1))
///     .clock(VirtualClock::new()) // owned: not an `AsyncClock`
///     .call();
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

    /// Returns every wait the engine actually performed, in request order.
    ///
    /// Zero-delay waits are absent: both engines skip the clock entirely when
    /// a strategy yields [`Duration::ZERO`], so the clock is never told about
    /// them. A run under `wait::fixed(Duration::ZERO)` records nothing, and
    /// `waits().len()` is therefore a count of performed sleeps, not of
    /// inter-attempt gaps — assert on the schedule, not on `attempts - 1`.
    ///
    /// A point-in-time snapshot: the returned `Vec` is unaffected by waits
    /// recorded after the call. Only available with the `alloc` feature
    /// (recording needs a `Vec`); the clock's time-keeping never does.
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
