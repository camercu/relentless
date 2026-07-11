//! Deterministic virtual-clock test infrastructure.
//!
//! [`VirtualClock`] lets you test retry behavior — backoff schedules, timeout
//! budgets, attempt counts — without real sleeping: its sleep adapters record
//! each requested sleep and advance virtual time instead of blocking.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::vec::Vec;

use crate::compat::Duration;

#[derive(Debug, Default)]
struct Inner {
    now: Duration,
    sleeps: Vec<Duration>,
}

/// A deterministic clock for testing retry behavior without real sleeps.
///
/// Cloning yields a handle to the same underlying clock, so the clock and its
/// adapters can be shared between the retry builder and test assertions.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use relentless::test_util::VirtualClock;
/// use relentless::{retry, stop, wait};
///
/// let clock = VirtualClock::new();
///
/// let result = retry(|_| Err::<(), &str>("boom"))
///     .wait(wait::fixed(Duration::from_millis(50)))
///     .stop(stop::attempts(2))
///     .sleep(clock.sync_sleep())
///     .call();
///
/// assert!(result.is_err());
/// assert_eq!(clock.sleeps(), vec![Duration::from_millis(50)]);
/// ```
#[derive(Clone, Debug, Default)]
pub struct VirtualClock {
    inner: Arc<Mutex<Inner>>,
}

impl VirtualClock {
    /// Creates a virtual clock at time zero with no recorded sleeps.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current virtual time.
    ///
    /// Starts at zero; advances when a sleep adapter runs.
    #[must_use]
    pub fn now(&self) -> Duration {
        self.lock().now
    }

    /// Advances virtual time by `dur` without recording a sleep.
    ///
    /// Use inside an operation to simulate attempts that consume the elapsed
    /// budget (e.g. slow calls) without any waiting between attempts.
    /// Saturates at [`Duration::MAX`].
    pub fn advance(&self, dur: Duration) {
        let mut inner = self.lock();
        inner.now = inner.now.saturating_add(dur);
    }

    /// Returns an elapsed-clock function reading this clock's virtual time
    /// ([`.elapsed_clock_fn(...)`](crate::SyncRetryExec::elapsed_clock_fn)).
    ///
    /// Pair it with a sleep adapter from the same clock so waits consume the
    /// elapsed budget deterministically.
    pub fn clock(&self) -> impl Fn() -> Duration + Clone + Send + Sync + 'static {
        let inner = Arc::clone(&self.inner);
        move || lock(&inner).now
    }

    /// Returns a sleep function for sync execution
    /// ([`.sleep(...)`](crate::SyncRetryExec::sleep)).
    ///
    /// The function records each requested sleep and advances virtual time by
    /// that amount instead of blocking.
    pub fn sync_sleep(&self) -> impl FnMut(Duration) + Send + 'static {
        let inner = Arc::clone(&self.inner);
        move |dur| record_sleep(&inner, dur)
    }

    /// Returns a sleep function for async execution
    /// ([`.sleep(...)`](crate::AsyncRetryExec::sleep)).
    ///
    /// The returned future completes immediately; the requested sleep is
    /// recorded and virtual time advances by that amount instead of waiting.
    pub fn async_sleep(
        &self,
    ) -> impl Fn(Duration) -> core::future::Ready<()> + Clone + Send + Sync + 'static {
        let inner = Arc::clone(&self.inner);
        move |dur| {
            record_sleep(&inner, dur);
            core::future::ready(())
        }
    }

    /// Returns every sleep requested so far, in request order.
    #[must_use]
    pub fn sleeps(&self) -> Vec<Duration> {
        self.lock().sleeps.clone()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }
}

/// Poisoning is recoverable here: the state is plain data (a `Duration` and a
/// `Vec` of them), left consistent by every critical section even if a caller
/// panicked while holding the lock.
fn lock(inner: &Arc<Mutex<Inner>>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(PoisonError::into_inner)
}

fn record_sleep(inner: &Arc<Mutex<Inner>>, dur: Duration) {
    let mut inner = lock(inner);
    inner.now = inner.now.saturating_add(dur);
    inner.sleeps.push(dur);
}
