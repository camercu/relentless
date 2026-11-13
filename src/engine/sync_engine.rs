//! The blocking driver.
//!
//! A [`Blocking`] [`Retry`] runs the shared decision core in a loop that
//! sleeps on a [`SyncClock`]. The async driver lives in `async_engine`.

use super::builder::Retry;
use super::elapsed::Elapsed;
use super::op::{Blocking, Operation, ready_outcome};
use super::step::{Progress, Step, step};
use super::{RetryError, RetryStats};
use crate::clock::{SyncClock, SystemClock};
use crate::compat::Duration;
use crate::decision::{Decide, DefaultClassifier};
use crate::policy::RetryPolicy;
use crate::state::RetryState;
use crate::stop::{Stop, StopAfterAttempts};
use crate::wait::{Wait, WaitExponential};

/// Default builder returned by [`retry`].
type DefaultRetry<F> =
    Retry<Blocking, F, DefaultClassifier, StopAfterAttempts, WaitExponential, SystemClock>;

/// Begins a classifier-driven retry from an operation.
///
/// The operation receives the [`RetryState`] (attempt number, elapsed time,
/// previous delay); ignore it with `|_|`. Defaults: `stop::attempts(3)`,
/// `wait::exponential(100ms)`, the default classifier, and
/// [`SystemClock`]. Without `std`, and on `wasm32-unknown-unknown` (no OS
/// clock), `.clock(...)` must be set before `.call()`.
///
/// For an operation that returns a future, use
/// [`retry_async`](crate::retry_async). Passed here, the future itself becomes
/// the outcome, which the default classifier cannot classify.
pub fn retry<F, O>(op: F) -> DefaultRetry<F>
where
    F: FnMut(RetryState) -> O,
{
    Retry::from_policy(op, RetryPolicy::new())
}

impl<F, C, S, W, Cl, O> Retry<Blocking, F, C, S, W, Cl>
where
    F: Operation<Blocking, Output = O>,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: SyncClock,
{
    /// Drives the retry loop to completion.
    ///
    /// Returns `Ok(value)` on the classifier's first
    /// [`Verdict::Return`](crate::decision::Verdict::Return),
    /// `Err(RetryError::Aborted { .. })` on a
    /// [`Verdict::Abort`](crate::decision::Verdict::Abort), and
    /// `Err(RetryError::Exhausted { .. })` when the stop strategy fires (or the
    /// timeout is exceeded) while the classifier still wants to retry.
    ///
    /// # Errors
    ///
    /// Returns [`RetryError`] on abort or exhaustion.
    ///
    /// # Panics
    ///
    /// Panics, in debug builds only, if the injected [`Clock`](crate::clock::Clock) reports an
    /// elapsed reading below one it already reported — a violation of
    /// [`Clock::now`](crate::clock::Clock::now)'s monotonicity precondition. Release builds clamp
    /// instead, so a broken clock cannot un-spend the timeout budget either
    /// way; the assertion exists to surface the broken clock in its
    /// implementor's own tests. No clock shipped with this crate can trigger
    /// it.
    pub fn call(self) -> Result<C::R, RetryError<C::A, O>> {
        self.run().0
    }

    /// Drives the retry loop and returns both the result and the
    /// [`RetryStats`].
    ///
    /// # Errors
    ///
    /// Returns [`RetryError`] on abort or exhaustion.
    ///
    /// # Panics
    ///
    /// Panics, in debug builds only, if the injected [`Clock`](crate::clock::Clock) reports an
    /// elapsed reading below one it already reported — see
    /// [`call`](Self::call).
    #[allow(clippy::type_complexity)]
    pub fn call_with_stats(self) -> (Result<C::R, RetryError<C::A, O>>, RetryStats) {
        self.run()
    }

    /// Shared driver for [`call`](Self::call) and
    /// [`call_with_stats`](Self::call_with_stats): runs the
    /// loop and always produces [`RetryStats`] (they are `Copy`, so collecting
    /// them unconditionally is free and keeps the loop allocation-free).
    #[allow(clippy::type_complexity)]
    fn run(mut self) -> (Result<C::R, RetryError<C::A, O>>, RetryStats) {
        let mut elapsed = Elapsed::start(&self.clock);

        let mut attempt: u32 = 1;
        let mut previous_delay: Option<Duration> = None;
        let mut total_wait = Duration::ZERO;

        loop {
            // One clock read for the before-attempt state and the operation.
            let before_state = RetryState::for_attempt(attempt)
                .with_elapsed(elapsed.read(&self.clock))
                .with_previous_delay(previous_delay);
            let outcome = ready_outcome(self.op.call_op(before_state));

            // Elapsed is re-read after the operation so stop and wait see the time the attempt actually took.
            let post_elapsed = elapsed.read(&self.clock);

            match step(
                Progress {
                    attempt,
                    elapsed: post_elapsed,
                    previous_delay,
                    total_wait,
                },
                outcome,
                &self.policy,
            ) {
                Step::Done { result, stats } => return (result, stats),
                Step::Continue {
                    delay,
                    total_wait: running,
                } => {
                    // The blocking sleep is the sync engine's half of the loop;
                    // the async engine polls a sleep future instead.
                    if !delay.is_zero() {
                        self.clock.wait(delay);
                    }
                    total_wait = running;
                    previous_delay = Some(delay);
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }
}
