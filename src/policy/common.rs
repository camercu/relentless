use super::sync::SyncSleep;
use super::time::ElapsedTracker;
use super::*;
use crate::cancel::Canceler;
use crate::error::RetryError;
use crate::state::{AttemptState, BeforeAttemptState, ExitState, RetryState};
use crate::stats::{RetryStats, StopReason};
#[cfg(feature = "alloc")]
use core::task::Poll;

fn attempt_state_from<'a, T, E>(
    retry_state: &RetryState,
    outcome: &'a Result<T, E>,
) -> AttemptState<'a, T, E> {
    AttemptState {
        attempt: retry_state.attempt,
        outcome,
        elapsed: retry_state.elapsed,
        next_delay: retry_state.next_delay,
        total_wait: retry_state.total_wait,
    }
}

fn exit_state_from<'a, T, E>(
    attempt_state: &AttemptState<'a, T, E>,
    reason: StopReason,
) -> ExitState<'a, T, E> {
    ExitState {
        attempt: attempt_state.attempt,
        outcome: attempt_state.outcome,
        elapsed: attempt_state.elapsed,
        total_wait: attempt_state.total_wait,
        reason,
    }
}

pub(super) enum AttemptTransition<T, E> {
    Finished {
        result: Result<T, RetryError<E, T>>,
        stats: Option<RetryStats>,
    },
    Sleep {
        next_delay: Duration,
        last_result: Result<T, E>,
    },
}

/// Builds a cancelled return value with optional stats.
///
/// Call `on_exit` with a reference to `last_result` before calling this,
/// since this function takes ownership.
pub(super) fn cancelled_return<T, E>(
    last_result: Option<Result<T, E>>,
    attempts: u32,
    elapsed: Option<Duration>,
    total_wait: Duration,
    collect_stats: bool,
) -> (Result<T, RetryError<E, T>>, Option<RetryStats>) {
    let stats = if collect_stats {
        Some(RetryStats {
            attempts,
            total_elapsed: elapsed,
            total_wait,
            stop_reason: StopReason::Cancelled,
        })
    } else {
        None
    };
    (
        Err(RetryError::Cancelled {
            last_result,
            attempts,
            total_elapsed: elapsed,
        }),
        stats,
    )
}

pub(super) fn process_attempt_transition<S, W, P, BA, AA, BS, OX, T, E>(
    policy: &mut RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    outcome: Result<T, E>,
    mut retry_state: RetryState,
    collect_stats: bool,
    total_wait: Duration,
) -> AttemptTransition<T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
{
    let should_retry = policy.predicate.should_retry(&outcome);
    {
        let attempt_state = attempt_state_from(&retry_state, &outcome);
        hooks.after_attempt.call(&attempt_state);
    }

    if !should_retry {
        let reason = stop_reason_for_predicate_accept(&outcome, policy.meta.predicate_is_default);
        let stats = if collect_stats {
            Some(RetryStats {
                attempts: retry_state.attempt,
                total_elapsed: retry_state.elapsed,
                total_wait,
                stop_reason: reason,
            })
        } else {
            None
        };

        {
            let attempt_state = attempt_state_from(&retry_state, &outcome);
            let exit_state = exit_state_from(&attempt_state, reason);
            hooks.on_exit.call(&exit_state);
        }

        return AttemptTransition::Finished {
            result: match outcome {
                Ok(value) => Ok(value),
                Err(error) => Err(RetryError::PredicateRejected {
                    error,
                    attempts: retry_state.attempt,
                    total_elapsed: retry_state.elapsed,
                }),
            },
            stats,
        };
    }

    let next_delay = policy.wait.next_wait(&retry_state);
    retry_state.next_delay = next_delay;

    if policy.stop.should_stop(&retry_state) {
        let reason = StopReason::StopCondition;
        let stats = if collect_stats {
            Some(RetryStats {
                attempts: retry_state.attempt,
                total_elapsed: retry_state.elapsed,
                total_wait,
                stop_reason: reason,
            })
        } else {
            None
        };

        {
            let attempt_state = attempt_state_from(&retry_state, &outcome);
            let exit_state = exit_state_from(&attempt_state, reason);
            hooks.on_exit.call(&exit_state);
        }

        return AttemptTransition::Finished {
            result: match outcome {
                Err(error) => Err(RetryError::Exhausted {
                    error,
                    attempts: retry_state.attempt,
                    total_elapsed: retry_state.elapsed,
                }),
                Ok(last) => Err(RetryError::ConditionNotMet {
                    last,
                    attempts: retry_state.attempt,
                    total_elapsed: retry_state.elapsed,
                }),
            },
            stats,
        };
    }

    {
        let attempt_state = attempt_state_from(&retry_state, &outcome);
        hooks.before_sleep.call(&attempt_state);
    }

    let last_error = outcome.err();

    AttemptTransition::Sleep {
        next_delay,
        last_error,
    }
}

pub(super) fn transition_from_outcome<S, W, P, BA, AA, BS, OX, T, E>(
    policy: &mut RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    outcome: Result<T, E>,
    attempt: u32,
    elapsed: Option<Duration>,
    total_wait: Duration,
    collect_stats: bool,
) -> AttemptTransition<T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
{
    let retry_state = RetryState {
        attempt,
        elapsed,
        next_delay: Duration::ZERO,
        total_wait,
    };

    process_attempt_transition(
        policy,
        hooks,
        outcome,
        retry_state,
        collect_stats,
        total_wait,
    )
}

pub(super) fn execute_sync_loop<
    S,
    W,
    P,
    BA,
    AA,
    BS,
    OX,
    F,
    SleepFn,
    T,
    E,
    C,
    const COLLECT_STATS: bool,
>(
    policy: &mut RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    op: &mut F,
    sleeper: &mut SleepFn,
    canceler: &C,
) -> (Result<T, RetryError<E, T>>, Option<RetryStats>)
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Result<T, E>,
    SleepFn: SyncSleep,
    C: Canceler,
{
    let mut attempt: u32 = 1;
    let mut total_wait = Duration::ZERO;
    let mut last_err: Option<E> = None;
    let elapsed_tracker = ElapsedTracker::new(policy.meta.elapsed_clock);

    loop {
        if canceler.is_cancelled() {
            let stats = if COLLECT_STATS {
                Some(RetryStats {
                    attempts: attempt - 1,
                    total_elapsed: elapsed_tracker.elapsed(),
                    total_wait,
                    stop_reason: StopReason::Cancelled,
                })
            } else {
                None
            };
            return (
                Err(RetryError::Cancelled {
                    last_error: last_err,
                    attempts: attempt - 1,
                    total_elapsed: elapsed_tracker.elapsed(),
                }),
                stats,
            );
        }

        let before_state = BeforeAttemptState {
            attempt,
            elapsed: elapsed_tracker.elapsed(),
            total_wait,
        };
        hooks.before_attempt.call(&before_state);

        let outcome = (op)();
        match transition_from_outcome(
            policy,
            hooks,
            outcome,
            attempt,
            elapsed_tracker.elapsed(),
            total_wait,
            COLLECT_STATS,
        ) {
            AttemptTransition::Finished { result, stats } => return (result, stats),
            AttemptTransition::Sleep {
                next_delay,
                last_error,
            } => {
                sleeper.sleep(next_delay);
                total_wait = total_wait.saturating_add(next_delay);

                if canceler.is_cancelled() {
                    let stats = if COLLECT_STATS {
                        Some(RetryStats {
                            attempts: attempt,
                            total_elapsed: elapsed_tracker.elapsed(),
                            total_wait,
                            stop_reason: StopReason::Cancelled,
                        })
                    } else {
                        None
                    };

                    if let Some(error) = last_error {
                        let outcome: Result<T, E> = Err(error);
                        let exit_state = ExitState {
                            attempt,
                            outcome: &outcome,
                            elapsed: elapsed_tracker.elapsed(),
                            total_wait,
                            reason: StopReason::Cancelled,
                        };
                        hooks.on_exit.call(&exit_state);
                        let error = match outcome {
                            Err(error) => error,
                            Ok(_) => unreachable!("cancellation outcome must be an error"),
                        };
                        return (
                            Err(RetryError::Cancelled {
                                last_error: Some(error),
                                attempts: attempt,
                                total_elapsed: elapsed_tracker.elapsed(),
                            }),
                            stats,
                        );
                    }

                    return (
                        Err(RetryError::Cancelled {
                            last_error: None,
                            attempts: attempt,
                            total_elapsed: elapsed_tracker.elapsed(),
                        }),
                        stats,
                    );
                }

                last_err = last_error;
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

#[cfg(feature = "alloc")]
pub(super) fn poll_after_completion<T>(type_name: &str) -> Poll<T> {
    #[cfg(any(debug_assertions, feature = "strict-futures"))]
    panic!("{type_name} polled after completion");

    #[cfg(all(not(debug_assertions), not(feature = "strict-futures")))]
    {
        Poll::Pending
    }
}

fn stop_reason_for_predicate_accept<T, E>(
    outcome: &Result<T, E>,
    predicate_is_default: bool,
) -> StopReason {
    if outcome.is_ok() && predicate_is_default {
        StopReason::Success
    } else {
        StopReason::PredicateAccepted
    }
}
