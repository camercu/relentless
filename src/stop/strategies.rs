use super::Stop;
use crate::compat::Duration;
use crate::state::RetryState;

/// Stops after a fixed number of completed attempts.
///
/// Created by [`attempts`]. Fires when `state.attempt >= max`.
///
/// # Examples
///
/// ```
/// use tenacious::Stop;
/// use tenacious::stop;
///
/// let s = stop::attempts(3);
/// # let state = tenacious::RetryState::new(3, None);
/// assert!(s.should_stop(&state));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopAfterAttempts {
    max: u32,
}

/// Produces a strategy that stops after `max` completed attempts.
///
/// The stop fires when `state.attempt >= max`.
///
/// # Panics
///
/// Panics if `max` is `0`.
#[must_use]
pub fn attempts(max: u32) -> StopAfterAttempts {
    assert!(max >= 1, "stop::attempts requires max >= 1");
    StopAfterAttempts { max }
}

impl Stop for StopAfterAttempts {
    fn should_stop(&self, state: &RetryState) -> bool {
        state.attempt >= self.max
    }
}

/// Stops when wall-clock elapsed time meets or exceeds a deadline.
///
/// Created by [`elapsed`]. When `state.elapsed` is `None` (no clock
/// available), this strategy never fires.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use tenacious::Stop;
/// use tenacious::stop;
///
/// let s = stop::elapsed(Duration::from_secs(30));
/// # let state = tenacious::RetryState::new(1, Some(Duration::from_secs(31)));
/// assert!(s.should_stop(&state));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopAfterElapsed {
    deadline: Duration,
}

/// Produces a strategy that stops when `state.elapsed >= Some(deadline)`.
///
/// When no clock is available (`elapsed` is `None`), this strategy never fires.
#[must_use]
pub fn elapsed(deadline: Duration) -> StopAfterElapsed {
    StopAfterElapsed { deadline }
}

impl Stop for StopAfterElapsed {
    fn should_stop(&self, state: &RetryState) -> bool {
        state
            .elapsed
            .is_some_and(|elapsed| elapsed >= self.deadline)
    }
}

/// A strategy that never stops — the retry loop continues indefinitely.
///
/// Created by [`never()`]. This is the correct explicit spelling of
/// "retry indefinitely."
///
/// # Examples
///
/// ```
/// use tenacious::Stop;
/// use tenacious::stop;
///
/// let s = stop::never();
/// # let state = tenacious::RetryState::new(u32::MAX, None);
/// assert!(!s.should_stop(&state));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopNever;

/// Produces a strategy that always returns `false` — never stops.
#[must_use]
pub fn never() -> StopNever {
    StopNever
}

impl Stop for StopNever {
    fn should_stop(&self, _state: &RetryState) -> bool {
        false
    }
}
