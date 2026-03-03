use super::*;
use crate::error::RetryError;
use crate::state::{AttemptState, ExitState, RetryState};
use crate::stats::{RetryStats, StopReason};

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
    },
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

    AttemptTransition::Sleep { next_delay }
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
