//! The deterministic test clock and its async wait future.

use super::{AsyncClock, Clock, SyncClock};
use crate::compat::Duration;
use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

/// A clock that never sleeps and reads no wall time.
///
/// Waits advance virtual time by exactly the requested amount, and
/// [`now()`](Clock::now) reads the very cell the waits advance — one cell, one
/// writer, so the read seam and the wait seam cannot drift even by an
/// implementation bug.
///
/// Two uses:
///
/// - **Tests.** Run a retry deterministically, with no real sleeping. Wrap it
///   in [`Recording`](super::Recording) (feature `alloc`) to assert the
///   backoff schedule.
/// - **Loops that need no timer.** When each attempt blocks by itself (it
///   waits on a socket or a channel with its own bound), the retry needs
///   neither a sleep nor a wall clock. A `VirtualClock` supplies both seams on
///   every target, including `no_std` and `wasm32-unknown-unknown`, and holds
///   no growing state. Elapsed time then counts only the scheduled waits, so
///   bound the loop with [`stop::attempts`](crate::stop::attempts).
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
}

impl VirtualClock {
    /// Creates a virtual clock at time zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            now: Cell::new(Duration::ZERO),
        }
    }

    /// Advances virtual time by `dur` without a wait.
    ///
    /// Use inside an operation to simulate attempts that consume the elapsed
    /// budget (e.g. slow calls) without any waiting between attempts.
    /// Saturates at [`Duration::MAX`].
    pub fn advance(&self, dur: Duration) {
        self.now.set(self.now.get().saturating_add(dur));
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
        self.advance(dur);
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
            self.clock.advance(self.dur);
        }
        Poll::Ready(())
    }
}
