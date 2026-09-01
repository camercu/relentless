//! Elapsed-time accounting shared by the sync and async drivers.

use crate::clock::Clock;
use crate::compat::Duration;

/// Elapsed time since execution started, held to a non-decreasing sequence.
///
/// [`Clock::now`] documents monotonic non-decrease as a precondition, but a
/// precondition the type system cannot express is one a consumer can break —
/// the obvious hand-written clock reads wall time, which an NTP step or a VM
/// resume moves backwards. Elapsed time is what `.timeout()` and
/// [`stop::elapsed`](crate::stop::elapsed) spend, so a backwards jump would
/// refund a spent budget and leave a bounded retry running forever.
///
/// Retaining the highest reading makes that a no-op instead: the budget can
/// only ever be spent. Both drivers read through this type, so neither can
/// hold a different opinion about what "elapsed" means.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Elapsed {
    origin: Duration,
    highest: Duration,
}

impl Elapsed {
    /// Captures the baseline reading that execution is measured against.
    pub(crate) fn start(clock: &impl Clock) -> Self {
        Self {
            origin: clock.now(),
            highest: Duration::ZERO,
        }
    }

    /// Reads the clock and returns elapsed time since the baseline.
    ///
    /// Never returns less than a previous call. A backwards clock trips a
    /// `debug_assert!` — the violation is the consumer's to fix, and their
    /// tests are where it is cheapest to learn about it.
    pub(crate) fn read(&mut self, clock: &impl Clock) -> Duration {
        let seen = clock.now().saturating_sub(self.origin);
        debug_assert!(
            seen >= self.highest,
            "`Clock::now` must be monotonically non-decreasing; it returned \
             an elapsed reading below one it already reported"
        );
        self.highest = self.highest.max(seen);
        self.highest
    }
}
