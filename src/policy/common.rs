use super::*;
use crate::stats::StopReason;

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

pub(super) enum AttemptTransition<T, E> {
    Finished {
        result: Result<T, RetryError<E, T>>,
        stats: Option<RetryStats>,
    },
    Sleep {
        next_delay: Duration,
    },
}

pub(super) fn process_attempt_transition<S, W, P, BA, AA, BS, OE, T, E>(
    policy: &mut RetryPolicy<S, W, P, BA, AA, BS, OE>,
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
    OE: AttemptHook<T, E>,
{
    let should_retry = policy.predicate.should_retry(&outcome);
    {
        let attempt_state = attempt_state_from(&retry_state, &outcome);
        policy.hooks.after_attempt.call(&attempt_state);
    }
    if !should_retry {
        let stats = if collect_stats {
            Some(RetryStats {
                attempts: retry_state.attempt,
                total_elapsed: retry_state.elapsed,
                total_wait,
                stop_reason: stop_reason_for_predicate_accept(
                    &outcome,
                    policy.meta.predicate_is_default,
                ),
            })
        } else {
            None
        };

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
        {
            let attempt_state = attempt_state_from(&retry_state, &outcome);
            policy.hooks.on_exhausted.call(&attempt_state);
        }
        let stats = if collect_stats {
            Some(RetryStats {
                attempts: retry_state.attempt,
                total_elapsed: retry_state.elapsed,
                total_wait,
                stop_reason: StopReason::StopCondition,
            })
        } else {
            None
        };
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
        policy.hooks.before_sleep.call(&attempt_state);
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
