//! The default wall-time clock for synchronous execution.

#[cfg(feature = "std")]
use super::{Clock, SyncClock};
#[cfg(feature = "std")]
use crate::compat::Duration;

/// The default wall-time clock for synchronous execution.
///
/// With the `std` feature it reads a process-global monotonic anchor
/// (`std::time::Instant`) and waits with `std::thread::sleep`. Without `std`
/// the type still exists — it is the builders' "no clock configured yet"
/// default — but implements no clock capability, so `.call()` is unavailable
/// until a real clock is supplied via `.clock(...)`.
///
/// `SystemClock` is deliberately sync-only: it does not implement
/// [`AsyncClock`](super::AsyncClock), so the async engine rejects it at compile time instead of
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
