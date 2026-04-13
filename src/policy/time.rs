use crate::compat::Duration;

#[cfg(feature = "alloc")]
use crate::compat::Box;

#[cfg(feature = "std")]
use std::time::Instant;

/// Function pointer type used to supply elapsed time in `no_std` or custom runtimes.
pub(crate) type ElapsedClockFn = fn() -> Duration;

enum ClockSource {
    FnPtr(ElapsedClockFn),
    #[cfg(feature = "alloc")]
    Boxed(Box<dyn Fn() -> Duration>),
}

impl ClockSource {
    fn call(&self) -> Duration {
        match self {
            ClockSource::FnPtr(f) => f(),
            #[cfg(feature = "alloc")]
            ClockSource::Boxed(f) => f(),
        }
    }
}

struct ClockStart {
    source: ClockSource,
    origin: Duration,
}

pub(crate) struct ElapsedTracker {
    start_clock: Option<ClockStart>,
    #[cfg(feature = "std")]
    start: Instant,
}

impl ElapsedTracker {
    pub(crate) fn new(clock: Option<ElapsedClockFn>) -> Self {
        Self {
            start_clock: clock.map(|clock| ClockStart {
                origin: clock(),
                source: ClockSource::FnPtr(clock),
            }),
            #[cfg(feature = "std")]
            start: Instant::now(),
        }
    }

    #[cfg(feature = "alloc")]
    pub(crate) fn new_boxed(clock: Box<dyn Fn() -> Duration>) -> Self {
        let origin = clock();
        Self {
            start_clock: Some(ClockStart {
                source: ClockSource::Boxed(clock),
                origin,
            }),
            #[cfg(feature = "std")]
            start: Instant::now(),
        }
    }

    pub(crate) fn elapsed(&self) -> Option<Duration> {
        self.start_clock
            .as_ref()
            .map(|start_clock| start_clock.source.call().saturating_sub(start_clock.origin))
            .or({
                #[cfg(feature = "std")]
                {
                    Some(self.start.elapsed())
                }

                #[cfg(not(feature = "std"))]
                {
                    None
                }
            })
    }
}
