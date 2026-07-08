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

/// Baseline reading captured when execution starts.
enum Baseline {
    NotStarted,
    /// Reading of the configured [`ClockSource`] at execution start.
    Origin(Duration),
    /// `std` fallback, used only when no custom clock is configured — a
    /// custom clock always wins in `elapsed()`, so taking `Instant::now()`
    /// then would be a wasted syscall.
    #[cfg(feature = "std")]
    StdInstant(Instant),
}

/// Tracks elapsed time for the retry loop.
///
/// Construction only records *which* clock to use. The baseline is captured by
/// [`start`](Self::start) when execution begins (SPEC 11.1.1), so idle time
/// between configuring a builder and running it never consumes the elapsed
/// budget.
pub(crate) struct ElapsedTracker {
    source: Option<ClockSource>,
    baseline: Baseline,
}

impl ElapsedTracker {
    pub(crate) fn new(clock: Option<ElapsedClockFn>) -> Self {
        Self {
            source: clock.map(ClockSource::FnPtr),
            baseline: Baseline::NotStarted,
        }
    }

    #[cfg(feature = "alloc")]
    pub(crate) fn new_boxed(clock: Box<dyn Fn() -> Duration>) -> Self {
        Self {
            source: Some(ClockSource::Boxed(clock)),
            baseline: Baseline::NotStarted,
        }
    }

    /// Captures the baseline reading. Idempotent: only the first call takes
    /// effect, so the async loop may invoke it on every poll while only the
    /// first poll sets the baseline.
    pub(crate) fn start(&mut self) {
        if !matches!(self.baseline, Baseline::NotStarted) {
            return;
        }
        self.baseline = match &self.source {
            Some(source) => Baseline::Origin(source.call()),
            // Without `std` there is no fallback clock: stay `NotStarted`,
            // so `elapsed()` keeps returning `None` (the SPEC 11.2 hazard).
            None => {
                #[cfg(feature = "std")]
                {
                    Baseline::StdInstant(Instant::now())
                }

                #[cfg(not(feature = "std"))]
                {
                    Baseline::NotStarted
                }
            }
        };
    }

    /// Returns time elapsed since [`start`](Self::start), or `None` when no
    /// clock is available or the tracker has not been started.
    pub(crate) fn elapsed(&self) -> Option<Duration> {
        match &self.baseline {
            Baseline::NotStarted => None,
            Baseline::Origin(origin) => {
                let source = self.source.as_ref()?;
                Some(source.call().saturating_sub(*origin))
            }
            #[cfg(feature = "std")]
            Baseline::StdInstant(start) => Some(start.elapsed()),
        }
    }
}
