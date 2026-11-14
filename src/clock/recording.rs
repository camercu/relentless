//! A clock wrapper that records every wait it performs.

use super::{AsyncClock, Clock, SyncClock};
use alloc::vec::Vec;
use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use pin_project_lite::pin_project;

/// Wraps a clock and records each wait the engine performs, in request order.
///
/// Reads and waits pass through to the wrapped clock unchanged, so wrapping
/// never alters timing. Use it in tests to assert a backoff schedule:
///
/// ```
/// use core::time::Duration;
/// use relentless::clock::{Recording, VirtualClock};
/// use relentless::{retry, stop, wait};
///
/// let clock = Recording::new(VirtualClock::new());
/// let _ = retry(|_| Err::<(), &str>("busy"))
///     .stop(stop::attempts(3))
///     .wait(wait::fixed(Duration::from_millis(10)))
///     .clock(&clock)
///     .call();
/// assert_eq!(clock.waits(), [Duration::from_millis(10); 2]);
/// ```
///
/// A shared borrow (`&Recording<C>`) is an [`AsyncClock`] whenever `&C` is
/// one. An async wait is recorded when its future completes, so a wait that
/// is dropped unpolled is not recorded.
///
/// Only available with the `alloc` feature: the record is a `Vec` that grows
/// by one entry per wait.
#[derive(Debug, Default)]
pub struct Recording<C> {
    inner: C,
    waits: RefCell<Vec<Duration>>,
}

impl<C> Recording<C> {
    /// Wraps `inner` with an empty record.
    #[must_use]
    pub const fn new(inner: C) -> Self {
        Self {
            inner,
            waits: RefCell::new(Vec::new()),
        }
    }

    /// Returns every wait performed so far, in request order.
    ///
    /// The engine skips the clock for a zero delay, so zero-delay waits are
    /// absent and the count is of performed sleeps, not of gaps between
    /// attempts. The returned `Vec` is a snapshot.
    #[must_use]
    pub fn waits(&self) -> Vec<Duration> {
        self.waits.borrow().clone()
    }

    /// Returns the wrapped clock.
    #[must_use]
    pub fn inner(&self) -> &C {
        &self.inner
    }
}

impl<C: Clock> Clock for Recording<C> {
    fn now(&self) -> Duration {
        self.inner.now()
    }
}

impl<C: SyncClock> SyncClock for Recording<C> {
    fn wait(&self, dur: Duration) {
        self.inner.wait(dur);
        self.waits.borrow_mut().push(dur);
    }
}

impl<'clock, C> AsyncClock for &'clock Recording<C>
where
    C: Clock,
    &'clock C: AsyncClock,
{
    type Wait = RecordingWait<'clock, <&'clock C as AsyncClock>::Wait>;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        RecordingWait {
            inner: (&self.inner).wait_async(dur),
            waits: &self.waits,
            dur,
            recorded: false,
        }
    }
}

pin_project! {
    /// Future returned by [`Recording`]'s [`AsyncClock::wait_async`]: the wrapped
    /// clock's wait future, recorded once it completes.
    #[derive(Debug)]
    #[must_use = "futures do nothing unless polled"]
    pub struct RecordingWait<'clock, W> {
        #[pin]
        inner: W,
        waits: &'clock RefCell<Vec<Duration>>,
        dur: Duration,
        recorded: bool,
    }
}

impl<W: Future<Output = ()>> Future for RecordingWait<'_, W> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.project();
        let polled = this.inner.poll(cx);
        if polled.is_ready() && !*this.recorded {
            *this.recorded = true;
            this.waits.borrow_mut().push(*this.dur);
        }
        polled
    }
}
