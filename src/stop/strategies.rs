use super::Stop;
use crate::compat::Duration;
use crate::state::RetryState;
use core::fmt;

/// Stops after a fixed number of completed attempts.
///
/// Created by [`attempts`] or [`attempts_checked`]. Fires when
/// `state.attempt >= max`.
///
/// # Examples
///
/// ```
/// use tenacious::Stop;
/// use tenacious::stop;
///
/// let mut s = stop::attempts(3);
/// # let state = tenacious::RetryState::new(
/// #     3,
/// #     None,
/// #     core::time::Duration::ZERO,
/// #     core::time::Duration::ZERO,
/// # );
/// assert!(s.should_stop(&state));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopAfterAttempts {
    max: u32,
}

/// Minimum valid attempt count for `stop::attempts`.
const MIN_STOP_ATTEMPTS: u32 = 1;

/// Error returned when constructing stop strategies from invalid input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopConfigError {
    /// `stop::attempts_checked` was given `0`, which is invalid.
    ZeroAttempts,
}

impl fmt::Display for StopConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopConfigError::ZeroAttempts => f.write_str("stop::attempts requires max >= 1"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for StopConfigError {}

/// Produces a strategy that stops after `max` completed attempts.
///
/// The stop fires when `state.attempt >= max`.
///
/// # Panics
///
/// Panics if `max` is `0`.
#[must_use]
pub fn attempts(max: u32) -> StopAfterAttempts {
    attempts_checked(max).expect("stop::attempts requires max >= 1")
}

/// Produces a strategy that stops after `max` completed attempts.
///
/// This non-panicking variant is suitable when `max` comes from untrusted or
/// runtime configuration input.
///
/// # Errors
///
/// Returns [`StopConfigError::ZeroAttempts`] when `max` is `0`.
pub fn attempts_checked(max: u32) -> Result<StopAfterAttempts, StopConfigError> {
    if max < MIN_STOP_ATTEMPTS {
        return Err(StopConfigError::ZeroAttempts);
    }
    Ok(StopAfterAttempts { max })
}

impl Stop for StopAfterAttempts {
    fn should_stop(&mut self, state: &RetryState) -> bool {
        state.attempt >= self.max
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for StopAfterAttempts {
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("StopAfterAttempts", 1)?;
        state.serialize_field("max", &self.max)?;
        state.end()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for StopAfterAttempts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct SerializedStopAfterAttempts {
            max: u32,
        }

        let serialized = SerializedStopAfterAttempts::deserialize(deserializer)?;
        attempts_checked(serialized.max).map_err(serde::de::Error::custom)
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
/// let mut s = stop::elapsed(Duration::from_secs(30));
/// # let state = tenacious::RetryState::new(
/// #     1,
/// #     Some(Duration::from_secs(31)),
/// #     Duration::ZERO,
/// #     Duration::ZERO,
/// # );
/// assert!(s.should_stop(&state));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    fn should_stop(&mut self, state: &RetryState) -> bool {
        state
            .elapsed
            .is_some_and(|elapsed| elapsed >= self.deadline)
    }
}

/// Conservative stop strategy that fires when the next attempt would likely
/// exceed a deadline.
///
/// Created by [`before_elapsed`]. Fires when
/// `state.elapsed + state.next_delay >= deadline`. This prevents starting an
/// attempt when the computed pre-attempt sleep would already reach or exceed
/// the deadline.
///
/// This strategy does **not** account for the runtime of the *next* operation;
/// it only uses elapsed time so far plus the computed next delay.
///
/// When `state.elapsed` is `None` (no clock), this strategy never fires.
///
/// # Examples
///
/// ```
/// use core::time::Duration;
/// use tenacious::Stop;
/// use tenacious::stop;
///
/// let mut s = stop::before_elapsed(Duration::from_secs(10));
/// # let state = tenacious::RetryState::new(
/// #     1,
/// #     Some(Duration::from_secs(9)),
/// #     Duration::from_secs(2),
/// #     Duration::ZERO,
/// # );
/// assert!(s.should_stop(&state)); // 9s + 2s >= 10s
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StopBeforeElapsed {
    deadline: Duration,
}

/// Produces a conservative strategy that stops when elapsed time plus the
/// next delay would meet or exceed `deadline`.
///
/// This check uses only elapsed-so-far and the computed delay before the next
/// attempt. It does not estimate the next operation's runtime.
///
/// When no clock is available (`elapsed` is `None`), this strategy never fires.
#[must_use]
pub fn before_elapsed(deadline: Duration) -> StopBeforeElapsed {
    StopBeforeElapsed { deadline }
}

impl Stop for StopBeforeElapsed {
    fn should_stop(&mut self, state: &RetryState) -> bool {
        state
            .elapsed
            .is_some_and(|elapsed| elapsed.saturating_add(state.next_delay) >= self.deadline)
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
/// let mut s = stop::never();
/// # let state = tenacious::RetryState::new(
/// #     u32::MAX,
/// #     None,
/// #     core::time::Duration::ZERO,
/// #     core::time::Duration::ZERO,
/// # );
/// assert!(!s.should_stop(&state));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StopNever;

/// Produces a strategy that always returns `false` — never stops.
#[must_use]
pub fn never() -> StopNever {
    StopNever
}

impl Stop for StopNever {
    fn should_stop(&mut self, _state: &RetryState) -> bool {
        false
    }
}

/// Marker indicating no stop strategy has been configured.
///
/// This type intentionally does **not** implement [`Stop`], so retry
/// execution methods are unavailable until a concrete stop strategy is set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NeedsStop;
