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
    // Only the `std` Instant fallback is captured here, and only when no
    // custom clock is configured — a custom clock always wins in `elapsed()`,
    // so taking `Instant::now()` then would be a wasted syscall.
    #[cfg(feature = "std")]
    start: Option<Instant>,
}

impl ElapsedTracker {
    pub(crate) fn new(clock: Option<ElapsedClockFn>) -> Self {
        let start_clock = clock.map(|clock| ClockStart {
            origin: clock(),
            source: ClockSource::FnPtr(clock),
        });
        Self {
            #[cfg(feature = "std")]
            start: std_instant_fallback(start_clock.is_some()),
            start_clock,
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
            // A custom clock is always present here, so no Instant fallback.
            #[cfg(feature = "std")]
            start: None,
        }
    }

    pub(crate) fn elapsed(&self) -> Option<Duration> {
        self.start_clock
            .as_ref()
            .map(|start_clock| start_clock.source.call().saturating_sub(start_clock.origin))
            .or({
                #[cfg(feature = "std")]
                {
                    self.start.map(|start| start.elapsed())
                }

                #[cfg(not(feature = "std"))]
                {
                    None
                }
            })
    }
}

/// Captures `Instant::now()` only when no custom clock is configured.
#[cfg(feature = "std")]
fn std_instant_fallback(has_custom_clock: bool) -> Option<Instant> {
    if has_custom_clock {
        None
    } else {
        Some(Instant::now())
    }
}
