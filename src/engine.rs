//! The classifier-driven retry engine (ADR-6).
//!
//! This is the parallel, not-yet-public engine that will replace the
//! predicate-driven [`crate::policy`] engine. It reuses the outcome-agnostic
//! infrastructure unchanged — [`Stop`], [`Wait`], [`SyncClock`], and
//! [`RetryState`] — and swaps the boolean predicate for a [`Decide`] classifier
//! that consumes each outcome by value.
//!
//! The sync path carries the classifier surface (`.decide`/`.when`/`.until`);
//! hooks, stats, and the async path arrive in later slices.

// Parallel ADR-6 engine: unreachable from the public API until cutover (S8)
// re-exports it. Remove this allow then.
#![allow(dead_code)]

use crate::clock::{SyncClock, SystemClock};
use crate::compat::Duration;
use crate::decision::{
    ClosureClassifier, Decide, DefaultClassifier, IntoDecision, Until, Verdict, When,
};
use crate::predicate::Predicate;
use crate::state::RetryState;
use crate::stop::{self, Stop, StopAfterAttempts};
use crate::wait::{self, Wait, WaitExponential};

const DEFAULT_MAX_ATTEMPTS: u32 = 3;
const DEFAULT_INITIAL_WAIT: Duration = Duration::from_millis(100);

/// Error returned when the retry loop terminates without a `Return`.
///
/// - `Aborted` — the classifier chose [`Verdict::Abort`]; `last` is the
///   projected abort payload.
/// - `Exhausted` — the stop strategy fired while the classifier still wanted to
///   retry; `last` is the final whole outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetryError<A, O> {
    /// The classifier rejected an outcome as fatal.
    Aborted {
        /// The abort payload chosen by the classifier.
        last: A,
    },
    /// The stop strategy fired while the classifier still wanted to retry.
    Exhausted {
        /// The final whole outcome seen before giving up.
        last: O,
    },
}

/// A classifier-driven sync retry builder.
///
/// Configure the stop strategy, wait strategy, and clock, then call
/// [`call`](Self::call). The classifier slot `C` defaults to
/// [`DefaultClassifier`] and is swapped by later slices' `.decide`/`.when`
/// methods.
pub struct Retry<F, C, S, W, Cl> {
    op: F,
    classifier: C,
    stop: S,
    wait: W,
    clock: Cl,
}

/// Begins a classifier-driven retry from an operation.
///
/// Defaults: `stop::attempts(3)`, `wait::exponential(100ms)`, the default
/// classifier, and [`SystemClock`]. In non-`std` builds `.clock(...)` must be
/// set before `.call()`.
pub fn retry<F, O>(
    op: F,
) -> Retry<F, DefaultClassifier, StopAfterAttempts, WaitExponential, SystemClock>
where
    F: FnMut(RetryState) -> O,
{
    Retry {
        op,
        classifier: DefaultClassifier,
        stop: stop::attempts(DEFAULT_MAX_ATTEMPTS),
        wait: wait::exponential(DEFAULT_INITIAL_WAIT),
        clock: SystemClock,
    }
}

impl<F, C, S, W, Cl> Retry<F, C, S, W, Cl> {
    /// Sets the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> Retry<F, C, NewStop, W, Cl> {
        Retry {
            op: self.op,
            classifier: self.classifier,
            stop,
            wait: self.wait,
            clock: self.clock,
        }
    }

    /// Sets the wait strategy used between attempts.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> Retry<F, C, S, NewWait, Cl> {
        Retry {
            op: self.op,
            classifier: self.classifier,
            stop: self.stop,
            wait,
            clock: self.clock,
        }
    }

    /// Sets the clock that supplies elapsed time and performs waits.
    #[must_use]
    pub fn clock<NewClock>(self, clock: NewClock) -> Retry<F, C, S, W, NewClock> {
        Retry {
            op: self.op,
            classifier: self.classifier,
            stop: self.stop,
            wait: self.wait,
            clock,
        }
    }

    /// Installs a classifier closure, replacing the classifier slot.
    ///
    /// The closure returns either [`Decision`](crate::decision::Decision) (no
    /// abort) or [`Verdict`] (abort-capable); [`IntoDecision`] unifies both. The
    /// `F: FnMut(RetryState) -> O` bound here (not deferred to `.call()`) lets
    /// the closure's parameter infer from the operation's output, so inline
    /// classifiers need no annotations.
    #[must_use]
    pub fn decide<O, D, NewC>(self, classifier: NewC) -> Retry<F, ClosureClassifier<NewC>, S, W, Cl>
    where
        F: FnMut(RetryState) -> O,
        NewC: Fn(O) -> D,
        D: IntoDecision<O>,
    {
        Retry {
            op: self.op,
            classifier: ClosureClassifier(classifier),
            stop: self.stop,
            wait: self.wait,
            clock: self.clock,
        }
    }

    /// Retries while `predicate` wants to; otherwise accepts the outcome
    /// (`Ok` returns, a rejected `Err` aborts with the bare error).
    ///
    /// `Result`-only sugar over [`decide`](Self::decide). The predicate's
    /// `&Result<T, E>` parameter infers from the operation's output.
    #[must_use]
    pub fn when<T, E, P>(self, predicate: P) -> Retry<F, When<P>, S, W, Cl>
    where
        F: FnMut(RetryState) -> Result<T, E>,
        P: Predicate<T, E>,
    {
        Retry {
            op: self.op,
            classifier: When(predicate),
            stop: self.stop,
            wait: self.wait,
            clock: self.clock,
        }
    }

    /// Retries *until* `predicate` is satisfied, then accepts. The inverse of
    /// [`when`](Self::when); natural for polling (`.until(ok(is_ready))`).
    #[must_use]
    pub fn until<T, E, P>(self, predicate: P) -> Retry<F, Until<P>, S, W, Cl>
    where
        F: FnMut(RetryState) -> Result<T, E>,
        P: Predicate<T, E>,
    {
        Retry {
            op: self.op,
            classifier: Until(predicate),
            stop: self.stop,
            wait: self.wait,
            clock: self.clock,
        }
    }
}

impl<F, C, S, W, Cl, O> Retry<F, C, S, W, Cl>
where
    F: FnMut(RetryState) -> O,
    C: Decide<O>,
    S: Stop,
    W: Wait,
    Cl: SyncClock,
{
    /// Drives the retry loop to completion.
    ///
    /// Returns `Ok(value)` on the classifier's first [`Verdict::Return`],
    /// `Err(RetryError::Aborted { .. })` on a [`Verdict::Abort`], and
    /// `Err(RetryError::Exhausted { .. })` when the stop strategy fires while
    /// the classifier still wants to retry.
    ///
    /// # Errors
    ///
    /// Returns [`RetryError`] on abort or exhaustion.
    pub fn call(mut self) -> Result<C::R, RetryError<C::A, O>> {
        let origin = self.clock.now();
        let elapsed = |clock: &Cl| clock.now().saturating_sub(origin);

        let mut attempt: u32 = 1;
        let mut previous_delay: Option<Duration> = None;

        loop {
            let op_state = RetryState::for_attempt(attempt)
                .with_elapsed(elapsed(&self.clock))
                .with_previous_delay(previous_delay);
            let outcome = (self.op)(op_state);

            // Elapsed is re-read after the operation so stop/wait see the time
            // the attempt actually took (matches the old engine).
            let state = RetryState::for_attempt(attempt)
                .with_elapsed(elapsed(&self.clock))
                .with_previous_delay(previous_delay);

            match self.classifier.decide(outcome) {
                Verdict::Return(value) => return Ok(value),
                Verdict::Abort(last) => return Err(RetryError::Aborted { last }),
                Verdict::Retry(last) => {
                    if self.stop.should_stop(&state) {
                        return Err(RetryError::Exhausted { last });
                    }
                    let delay = self.wait.next_wait(&state);
                    if !delay.is_zero() {
                        self.clock.wait(delay);
                    }
                    previous_delay = Some(delay);
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::VirtualClock;
    use core::cell::Cell;

    const ARBITRARY_ATTEMPTS: u32 = 5;

    #[test]
    fn retries_on_err_then_returns_the_ok_value() {
        let counter = Cell::new(0);
        let result = retry(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            if n >= 2 {
                Ok::<i32, &str>(7)
            } else {
                Err("not yet")
            }
        })
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Ok(7));
        assert_eq!(counter.get(), 2, "should stop retrying once Ok is returned");
    }

    #[test]
    fn exhausts_with_the_final_outcome_when_stop_fires() {
        let counter = Cell::new(0);
        let result = retry(|_| {
            counter.set(counter.get() + 1);
            Err::<i32, &str>("boom")
        })
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Err(RetryError::Exhausted { last: Err("boom") }));
        assert_eq!(counter.get(), 3, "should attempt exactly the stop budget");
    }

    #[test]
    fn decide_can_abort_on_a_projected_payload() {
        use crate::decision::Verdict;

        // Fuzzing shape: keep retrying transient errors, but abort fatally on a
        // specific one — the abort arm pins the payload type inline.
        let result = retry(|_| Err::<i32, &str>("fatal"))
            .decide(|o| match o {
                Ok(v) => Verdict::Return(v),
                Err("fatal") => Verdict::Abort("boom"),
                Err(_) => Verdict::Retry(o),
            })
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(VirtualClock::new())
            .call();

        assert_eq!(result, Err(RetryError::Aborted { last: "boom" }));
    }

    #[test]
    fn decide_returns_the_sought_error_through_ok() {
        use crate::decision::Decision;

        // The inverted-polling wart, gone: probe until a failure appears and
        // deliver that failure as the success value via `Ok` — no `RetryError`.
        let counter = Cell::new(0);
        let result = retry(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            if n >= 3 { Err("crash") } else { Ok(()) }
        })
        .decide(|o| match o {
            Err(e) => Decision::Return(e),
            Ok(()) => Decision::Retry(o),
        })
        .stop(stop::attempts(10))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Ok("crash"));
        assert_eq!(counter.get(), 3);
    }

    #[test]
    fn until_polls_a_result_until_ready() {
        use crate::predicate;

        #[derive(Debug, PartialEq)]
        enum Status {
            Pending,
            Done,
        }

        let counter = Cell::new(0);
        let result = retry(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            Ok::<Status, &str>(if n >= 2 {
                Status::Done
            } else {
                Status::Pending
            })
        })
        .until(predicate::ok(|s: &Status| *s == Status::Done))
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

        assert_eq!(result, Ok(Status::Done));
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn when_aborts_on_a_rejected_error() {
        use crate::predicate;

        // Retry only "transient"; any other error is non-retryable and aborts
        // with the bare error payload.
        let result = retry(|_| Err::<i32, &str>("fatal"))
            .when(predicate::error(|e: &&str| *e == "transient"))
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(VirtualClock::new())
            .call();

        assert_eq!(result, Err(RetryError::Aborted { last: "fatal" }));
    }
}
