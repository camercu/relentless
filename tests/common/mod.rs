//! Helpers shared by integration-test crates (`mod common;`).

// Each test crate uses a subset of these helpers.
#![allow(dead_code)]

use core::cell::{Cell, RefCell};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use std::rc::Rc;

// Arbitrary values: any valid value would do. Tests that depend on a specific
// number declare their own constant instead.
pub const ARBITRARY_ATTEMPTS: u32 = 3;
pub const ARBITRARY_VALUE: i32 = 42;
pub const ARBITRARY_ERROR: &str = "fail";
pub const ARBITRARY_DELAY: Duration = Duration::from_millis(10);
pub const CONTINUE_PAYLOAD: i32 = 7;
/// Deadline shorter than `CUSTOM_CLOCK_STEP_MILLIS`, so one attempt exhausts it.
pub const CUSTOM_CLOCK_DEADLINE: Duration = Duration::from_millis(5);
pub const CUSTOM_CLOCK_STEP_MILLIS: u64 = 10;
pub const CLOCK_STEP_MILLIS: u64 = 100;
pub const TIMEOUT_DEADLINE: Duration = Duration::from_millis(50);

/// Test-side clock that records each wait into a `Vec`, so wait-sequence
/// assertions also run under `--no-default-features`, where the library's
/// `VirtualClock` has no `alloc`-gated recorder. (The test binary always
/// links `std`.)
pub struct RecordingClock {
    now: Cell<Duration>,
    waits: RefCell<Vec<Duration>>,
}

impl RecordingClock {
    pub fn new() -> Self {
        Self {
            now: Cell::new(Duration::ZERO),
            waits: RefCell::new(Vec::new()),
        }
    }

    pub fn waits(&self) -> Vec<Duration> {
        self.waits.borrow().clone()
    }

    /// Advances virtual time without recording a wait (simulates op runtime).
    pub fn advance(&self, dur: Duration) {
        self.now.set(self.now.get().saturating_add(dur));
    }
}

impl relentless::Clock for RecordingClock {
    fn now(&self) -> Duration {
        self.now.get()
    }
}

impl relentless::SyncClock for RecordingClock {
    fn wait(&self, dur: Duration) {
        self.now.set(self.now.get().saturating_add(dur));
        self.waits.borrow_mut().push(dur);
    }
}

/// Test-side [`relentless::AsyncClock`]: waits resolve immediately, advancing
/// the same `now` cell the elapsed seam reads (coherent by construction) and
/// recording each requested wait. Cloning shares the underlying cells.
#[derive(Clone, Default)]
pub struct RecordingAsyncClock {
    inner: Rc<RecordingAsyncClockCells>,
}

#[derive(Default)]
pub struct RecordingAsyncClockCells {
    now: Cell<Duration>,
    calls: RefCell<Vec<Duration>>,
}

impl RecordingAsyncClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Waits requested so far, in request order.
    pub fn calls(&self) -> Vec<Duration> {
        self.inner.calls.borrow().clone()
    }

    /// Advances virtual time without recording a wait (simulates op runtime).
    pub fn advance(&self, dur: Duration) {
        self.inner.now.set(self.inner.now.get().saturating_add(dur));
    }
}

impl relentless::Clock for RecordingAsyncClock {
    fn now(&self) -> Duration {
        self.inner.now.get()
    }
}

impl relentless::AsyncClock for RecordingAsyncClock {
    type Wait = RecordingAsyncWait;

    fn wait_async(&self, dur: Duration) -> Self::Wait {
        RecordingAsyncWait {
            inner: Rc::clone(&self.inner),
            dur,
            done: false,
        }
    }
}

/// Wait future of [`RecordingAsyncClock`]: advances and records on first poll, so
/// an unpolled (cancelled) wait leaves time untouched.
pub struct RecordingAsyncWait {
    inner: Rc<RecordingAsyncClockCells>,
    dur: Duration,
    done: bool,
}

impl Future for RecordingAsyncWait {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if !this.done {
            this.done = true;
            this.inner
                .now
                .set(this.inner.now.get().saturating_add(this.dur));
            this.inner.calls.borrow_mut().push(this.dur);
        }
        Poll::Ready(())
    }
}

/// Polls a future to completion under a no-op waker, yielding the thread on
/// each `Pending`. Correct for futures that resolve without an external wake,
/// such as those driven by a virtual or recording clock.
pub fn block_on<F: core::future::Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    loop {
        if let core::task::Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        std::thread::yield_now();
    }
}
