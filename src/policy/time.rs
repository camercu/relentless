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

/// Tracks elapsed time for the retry loop.
///
/// Construction only records *which* clock to use. The baseline is captured by
/// [`start`](Self::start) when execution begins (SPEC 11.1.1), so idle time
/// between configuring a builder and running it never consumes the elapsed
/// budget.
pub(crate) struct ElapsedTracker {
    source: Option<ClockSource>,
    /// Baseline reading of `source`, captured by `start()`.
    origin: Option<Duration>,
    // Only the `std` Instant fallback is captured here, and only when no
    // custom clock is configured — a custom clock always wins in `elapsed()`,
    // so taking `Instant::now()` then would be a wasted syscall.
    #[cfg(feature = "std")]
    start: Option<Instant>,
}

impl ElapsedTracker {
    pub(crate) fn new(clock: Option<ElapsedClockFn>) -> Self {
        Self {
            source: clock.map(ClockSource::FnPtr),
            origin: None,
            #[cfg(feature = "std")]
            start: None,
        }
    }

    #[cfg(feature = "alloc")]
    pub(crate) fn new_boxed(clock: Box<dyn Fn() -> Duration>) -> Self {
        Self {
            source: Some(ClockSource::Boxed(clock)),
            origin: None,
            #[cfg(feature = "std")]
            start: None,
        }
    }

    /// Captures the baseline reading. Idempotent: only the first call takes
    /// effect, so the async loop may invoke it on every poll while only the
    /// first poll sets the baseline.
    pub(crate) fn start(&mut self) {
        if self.is_started() {
            return;
        }
        match &self.source {
            Some(source) => self.origin = Some(source.call()),
            None => {
                #[cfg(feature = "std")]
                {
                    self.start = Some(Instant::now());
                }
            }
        }
    }

    fn is_started(&self) -> bool {
        #[cfg(feature = "std")]
        {
            self.origin.is_some() || self.start.is_some()
        }

        #[cfg(not(feature = "std"))]
        {
            self.origin.is_some()
        }
    }

    /// Returns time elapsed since [`start`](Self::start), or `None` when no
    /// clock is available or the tracker has not been started.
    pub(crate) fn elapsed(&self) -> Option<Duration> {
        match (&self.source, self.origin) {
            (Some(source), Some(origin)) => Some(source.call().saturating_sub(origin)),
            _ => {
                #[cfg(feature = "std")]
                {
                    self.start.map(|start| start.elapsed())
                }

                #[cfg(not(feature = "std"))]
                {
                    None
                }
            }
        }
    }
}
