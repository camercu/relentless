use crate::compat::Duration;

use super::ElapsedClockFn;

#[cfg(feature = "std")]
use std::time::Instant;

#[derive(Clone, Copy)]
struct CustomElapsedStart {
    clock: ElapsedClockFn,
    origin: Duration,
}

#[derive(Clone, Copy)]
pub(super) struct ElapsedTracker {
    start_clock: Option<CustomElapsedStart>,
    #[cfg(feature = "std")]
    start: Instant,
}

impl ElapsedTracker {
    pub(super) fn new(clock: Option<ElapsedClockFn>) -> Self {
        Self {
            start_clock: clock.map(|clock| CustomElapsedStart {
                clock,
                origin: clock(),
            }),
            #[cfg(feature = "std")]
            start: Instant::now(),
        }
    }

    pub(super) fn elapsed(&self) -> Option<Duration> {
        if let Some(start_clock) = self.start_clock {
            Some((start_clock.clock)().saturating_sub(start_clock.origin))
        } else {
            #[cfg(feature = "std")]
            {
                Some(self.start.elapsed())
            }

            #[cfg(not(feature = "std"))]
            {
                None
            }
        }
    }
}
