//! Helpers shared by integration-test crates (`mod common;`).

// Each test crate uses a subset of these helpers.
#![allow(dead_code)]

use core::cell::{Cell, RefCell};
use core::time::Duration;

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
