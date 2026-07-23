//! The shared decision step for both retry engines.
//!
//! The sync ([`Retry::run`](crate::engine::Retry)) and async
//! ([`AsyncRun::poll`](crate::engine::AsyncRun)) engines differ only in how they
//! invoke the operation and how they sleep; the classify → stop → wait → exit
//! semantics between "have an outcome" and "sleep or terminate" are identical.
//! [`step`] is that shared middle, so the drift-prone core lives in one place
//! and is exercised directly by the tests below and differentially by
//! `tests/parity.rs`.

use super::error::RetryError;
use super::hooks::{AttemptHook, ExecutionHooks, ExitHook};
use super::state::{AttemptState, Exit, StopReason};
use super::stats::RetryStats;
use crate::compat::Duration;
use crate::decision::{Decide, Verdict};
use crate::state::RetryState;
use crate::stop::Stop;
use crate::wait::Wait;

/// What a completed attempt tells the driving loop to do next.
pub(crate) enum Step<R, A, O> {
    /// The loop terminates with this result and stats. `on_exit` has fired.
    Done {
        result: Result<R, RetryError<A, O>>,
        stats: RetryStats,
    },
    /// The loop schedules another attempt after sleeping `delay`. `total_wait`
    /// is the running total *including* `delay`; the caller still owns advancing
    /// the attempt counter and recording `delay` as the previous delay.
    Continue {
        delay: Duration,
        total_wait: Duration,
    },
}

/// Processes one completed attempt's `outcome`.
///
/// Fires `after_attempt`, classifies the outcome, and then either fires
/// `on_exit` and returns [`Step::Done`] (on return, abort, or exhaustion) or
/// computes the next backoff and returns [`Step::Continue`]. The caller supplies
/// `before_attempt`, the operation call, and the sleep — the only parts that
/// differ between the sync and async engines.
#[allow(clippy::too_many_arguments)]
pub(crate) fn step<C, S, W, BA, AA, OX, O>(
    attempt: u32,
    post_elapsed: Duration,
    previous_delay: Option<Duration>,
    total_wait: Duration,
    outcome: O,
    classifier: &C,
    stop: &S,
    wait: &W,
    timeout: Option<Duration>,
    hooks: &mut ExecutionHooks<BA, AA, OX>,
) -> Step<C::R, C::A, O>
where
    C: Decide<O>,
    S: Stop,
    W: Wait,
    AA: AttemptHook<O>,
    OX: ExitHook<C::R, C::A, O>,
{
    // `after_attempt` fires before the classifier consumes the outcome, so the
    // hook sees the raw outcome under a uniform contract.
    {
        let attempt_state = AttemptState::new(attempt, post_elapsed, &outcome);
        hooks.after_attempt.call(&attempt_state);
    }

    let state = RetryState::for_attempt(attempt)
        .with_elapsed(post_elapsed)
        .with_previous_delay(previous_delay);

    let stats_for = |reason| RetryStats {
        attempts: attempt,
        total_elapsed: post_elapsed,
        total_wait,
        stop_reason: reason,
    };

    match classifier.decide(outcome) {
        Verdict::Return(value) => {
            hooks.on_exit.call(&Exit::Returned {
                attempt,
                elapsed: post_elapsed,
                value: &value,
            });
            Step::Done {
                result: Ok(value),
                stats: stats_for(StopReason::Returned),
            }
        }
        Verdict::Abort(last) => {
            hooks.on_exit.call(&Exit::Aborted {
                attempt,
                elapsed: post_elapsed,
                last: &last,
            });
            Step::Done {
                result: Err(RetryError::Aborted { last }),
                stats: stats_for(StopReason::Aborted),
            }
        }
        Verdict::Retry(last) => {
            let timeout_exceeded = timeout.is_some_and(|t| post_elapsed >= t);
            if stop.should_stop(&state) || timeout_exceeded {
                hooks.on_exit.call(&Exit::Exhausted {
                    attempt,
                    elapsed: post_elapsed,
                    last: &last,
                });
                return Step::Done {
                    result: Err(RetryError::Exhausted { last }),
                    stats: stats_for(StopReason::Exhausted),
                };
            }

            // The wait strategy is consulted only now that a retry is certain,
            // then clamped to the remaining timeout budget.
            let next_delay = wait.next_wait(&state);
            let delay = match timeout {
                Some(t) => next_delay.min(t.saturating_sub(post_elapsed)),
                None => next_delay,
            };
            Step::Continue {
                delay,
                total_wait: total_wait.saturating_add(delay),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::DefaultClassifier;
    use crate::{stop, wait};

    const DELAY: Duration = Duration::from_millis(10);
    type Res = Result<i32, &'static str>;

    fn no_hooks() -> ExecutionHooks<(), (), ()> {
        ExecutionHooks::new()
    }

    // A classifier that fails every outcome fatally, to reach the `Abort` arm
    // the default `Result` classifier never takes.
    struct AlwaysAbort;
    impl Decide<Res> for AlwaysAbort {
        type R = i32;
        type A = &'static str;
        fn decide(&self, outcome: Res) -> Verdict<i32, &'static str, Res> {
            Verdict::Abort(outcome.unwrap_err())
        }
    }

    #[test]
    fn return_terminates_with_returned_stats() {
        let mut hooks = no_hooks();
        let step = step(
            1,
            Duration::ZERO,
            None,
            Duration::ZERO,
            Ok::<i32, &str>(5),
            &DefaultClassifier,
            &stop::attempts(3),
            &wait::fixed(DELAY),
            None,
            &mut hooks,
        );
        match step {
            Step::Done { result, stats } => {
                assert_eq!(result.unwrap(), 5);
                assert_eq!(stats.stop_reason, StopReason::Returned);
                assert_eq!(stats.attempts, 1);
                assert_eq!(stats.total_wait, Duration::ZERO);
            }
            Step::Continue { .. } => panic!("expected Done, got Continue"),
        }
    }

    #[test]
    fn abort_terminates_with_aborted_stats() {
        let mut hooks = no_hooks();
        let step = step(
            2,
            DELAY,
            Some(DELAY),
            DELAY,
            Err::<i32, &str>("fatal"),
            &AlwaysAbort,
            &stop::attempts(5),
            &wait::fixed(DELAY),
            None,
            &mut hooks,
        );
        match step {
            Step::Done { result, stats } => {
                assert_eq!(result.unwrap_err(), RetryError::Aborted { last: "fatal" });
                assert_eq!(stats.stop_reason, StopReason::Aborted);
                assert_eq!(stats.attempts, 2);
                // No new wait on a terminal attempt.
                assert_eq!(stats.total_wait, DELAY);
            }
            Step::Continue { .. } => panic!("expected Done, got Continue"),
        }
    }

    #[test]
    fn retry_continues_and_adds_delay_to_total_wait() {
        let mut hooks = no_hooks();
        let step = step(
            2,
            Duration::ZERO,
            Some(DELAY),
            DELAY,
            Err::<i32, &str>("again"),
            &DefaultClassifier,
            &stop::attempts(5),
            &wait::fixed(DELAY),
            None,
            &mut hooks,
        );
        match step {
            Step::Continue { delay, total_wait } => {
                assert_eq!(delay, DELAY);
                assert_eq!(total_wait, DELAY + DELAY);
            }
            Step::Done { .. } => panic!("expected Continue, got Done"),
        }
    }

    #[test]
    fn retry_exhausts_when_stop_fires() {
        let mut hooks = no_hooks();
        let step = step(
            1,
            Duration::ZERO,
            None,
            Duration::ZERO,
            Err::<i32, &str>("last"),
            &DefaultClassifier,
            &stop::attempts(1),
            &wait::fixed(DELAY),
            None,
            &mut hooks,
        );
        match step {
            Step::Done { result, stats } => {
                assert_eq!(
                    result.unwrap_err(),
                    RetryError::Exhausted { last: Err("last") }
                );
                assert_eq!(stats.stop_reason, StopReason::Exhausted);
                assert_eq!(stats.total_wait, Duration::ZERO);
            }
            Step::Continue { .. } => panic!("expected Done, got Continue"),
        }
    }

    #[test]
    fn retry_clamps_delay_to_remaining_timeout_budget() {
        let mut hooks = no_hooks();
        let elapsed = Duration::from_millis(5);
        let step = step(
            1,
            elapsed,
            None,
            Duration::ZERO,
            Err::<i32, &str>("again"),
            &DefaultClassifier,
            &stop::attempts(5),
            &wait::fixed(DELAY), // 10ms, larger than the 3ms budget below
            Some(Duration::from_millis(8)),
            &mut hooks,
        );
        match step {
            Step::Continue { delay, total_wait } => {
                assert_eq!(delay, Duration::from_millis(3));
                assert_eq!(total_wait, Duration::from_millis(3));
            }
            Step::Done { .. } => panic!("expected Continue, got Done"),
        }
    }
}
