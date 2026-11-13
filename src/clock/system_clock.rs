//! The default wall-time clock for synchronous execution.

#[cfg(all(
    feature = "std",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
use super::{Clock, SyncClock};
#[cfg(all(
    feature = "std",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
use crate::compat::Duration;

/// The default wall-time clock for synchronous execution.
///
/// With the `std` feature it reads a process-global monotonic anchor
/// (`std::time::Instant`) and waits with `std::thread::sleep`. Without `std`,
/// and on `wasm32-unknown-unknown` (which has no OS clock), the type still
/// exists — it is the builders' "no clock configured yet" default — but
/// implements no clock capability, so `.call()` does not compile until a clock
/// is supplied via `.clock(...)`. On that target, use
/// [`VirtualClock`](super::VirtualClock) for loops that need no timer.
///
/// `SystemClock` is deliberately sync-only: it does not implement
/// [`AsyncClock`](super::AsyncClock), so the async engine rejects it at
/// compile time instead of letting a blocking clock stall a reactor thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemClock;

#[cfg(all(
    feature = "std",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
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

#[cfg(all(
    feature = "std",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
impl SyncClock for SystemClock {
    fn wait(&self, dur: Duration) {
        std::thread::sleep(dur);
    }
}

// No OS clock exists on wasm32-unknown-unknown: `Instant::now` compiles there
// but panics. This fails to compile if `SystemClock` ever implements `Clock`
// on that target, so the panic can never return as a silent default.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
const _: () = {
    use super::Clock;
    trait AmbiguousIfClock<A> {
        fn check() {}
    }
    struct IsClock;
    impl<T: ?Sized> AmbiguousIfClock<()> for T {}
    impl<T: ?Sized + Clock> AmbiguousIfClock<IsClock> for T {}
    let _ = <SystemClock as AmbiguousIfClock<_>>::check;
};
