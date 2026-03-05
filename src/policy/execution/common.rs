use super::sync_exec::SyncSleep;
use crate::cancel::Canceler;
use crate::compat::Duration;
use crate::error::RetryError;
use crate::policy::time::ElapsedTracker;
use crate::policy::{AttemptHook, BeforeAttemptHook, ExecutionHooks, ExitHook, RetryPolicy};
use crate::predicate::Predicate;
#[cfg(feature = "alloc")]
use crate::sleep::Sleeper;
use crate::state::{AttemptState, BeforeAttemptState, ExitState, RetryState};
use crate::stats::{RetryStats, StopReason};
use crate::stop::Stop;
use crate::wait::Wait;
#[cfg(feature = "alloc")]
use core::future::Future;
#[cfg(feature = "alloc")]
use core::pin::Pin;
#[cfg(feature = "alloc")]
use core::task::Context;
#[cfg(feature = "alloc")]
use core::task::Poll;
#[cfg(feature = "alloc")]
use pin_project_lite::pin_project;

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
        outcome: Some(attempt_state.outcome),
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

#[cfg(feature = "alloc")]
pin_project! {
    #[project = AsyncPhaseProj]
pub(crate) enum AsyncPhase<Fut, SleepFut, CancelFut> {
        ReadyToStartAttempt,
        PollingOperation {
            #[pin]
            op_future: Fut,
        },
        Sleeping {
            #[pin]
            sleep_future: SleepFut,
            #[pin]
            cancel_future: CancelFut,
        },
        Finished,
    }
}

#[cfg(feature = "alloc")]
pub(crate) enum AsyncOperationPoll<T, E> {
    Pending,
    Finished {
        result: Result<T, RetryError<E, T>>,
        stats: Option<RetryStats>,
    },
    Sleep {
        next_delay: Duration,
        last_result: Result<T, E>,
    },
}

#[cfg(feature = "alloc")]
pub(crate) fn remap_no_sleep_phase<Fut, OldSleepFut, OldCancelFut, NewSleepFut, NewCancelFut>(
    phase: AsyncPhase<Fut, OldSleepFut, OldCancelFut>,
    unreachable_message: &'static str,
) -> AsyncPhase<Fut, NewSleepFut, NewCancelFut> {
    match phase {
        AsyncPhase::ReadyToStartAttempt => AsyncPhase::ReadyToStartAttempt,
        AsyncPhase::PollingOperation { op_future } => AsyncPhase::PollingOperation { op_future },
        AsyncPhase::Sleeping { .. } => unreachable!("{unreachable_message}"),
        AsyncPhase::Finished => AsyncPhase::Finished,
    }
}

#[cfg(feature = "alloc")]
pub(crate) fn fire_before_attempt<BA, AA, BS, OX>(
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    attempt: u32,
    elapsed: Option<Duration>,
    total_wait: Duration,
) where
    BA: BeforeAttemptHook,
{
    let before_state = BeforeAttemptState {
        attempt,
        elapsed,
        total_wait,
    };
    hooks.before_attempt.call(&before_state);
}

#[cfg(feature = "alloc")]
// Intentional: this helper wires all state-machine inputs in one place to keep
// async retry transition logic shared between policy and extension builders.
#[allow(clippy::too_many_arguments)]
pub(crate) fn poll_operation_future<S, W, P, BA, AA, BS, OX, Fut, T, E>(
    op_future: Pin<&mut Fut>,
    cx: &mut Context<'_>,
    policy: &mut RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    attempt: u32,
    elapsed_tracker: &ElapsedTracker,
    total_wait: Duration,
    collect_stats: bool,
) -> AsyncOperationPoll<T, E>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    Fut: Future<Output = Result<T, E>>,
{
    match op_future.poll(cx) {
        Poll::Pending => AsyncOperationPoll::Pending,
        Poll::Ready(outcome) => match transition_from_outcome(
            policy,
            hooks,
            outcome,
            attempt,
            elapsed_tracker.elapsed(),
            total_wait,
            collect_stats,
        ) {
            AttemptTransition::Finished { result, stats } => {
                AsyncOperationPoll::Finished { result, stats }
            }
            AttemptTransition::Sleep {
                next_delay,
                last_result,
            } => AsyncOperationPoll::Sleep {
                next_delay,
                last_result,
            },
        },
    }
}

#[cfg(feature = "alloc")]
fn finish_cancelled_async<T, E, BA, AA, BS, OX>(
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    last_result: &mut Option<Result<T, E>>,
    attempt: u32,
    total_wait: Duration,
    collect_stats: bool,
    elapsed_tracker: &ElapsedTracker,
) -> (Result<T, RetryError<E, T>>, Option<RetryStats>)
where
    OX: ExitHook<T, E>,
{
    let elapsed = elapsed_tracker.elapsed();
    let stats = if collect_stats {
        Some(RetryStats {
            attempts: attempt,
            total_elapsed: elapsed,
            total_wait,
            stop_reason: StopReason::Cancelled,
        })
    } else {
        None
    };

    let exit_state = ExitState {
        attempt,
        outcome: last_result.as_ref(),
        elapsed,
        total_wait,
        reason: StopReason::Cancelled,
    };
    hooks.on_exit.call(&exit_state);

    (
        Err(RetryError::Cancelled {
            last_result: last_result.take(),
            attempts: attempt,
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

    let last_result = outcome;

    AttemptTransition::Sleep {
        next_delay,
        last_result,
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

pub(crate) fn execute_sync_loop<
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
    let mut last_result: Option<Result<T, E>> = None;
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
            let exit_state = ExitState {
                attempt: attempt.saturating_sub(1),
                outcome: last_result.as_ref(),
                elapsed: elapsed_tracker.elapsed(),
                total_wait,
                reason: StopReason::Cancelled,
            };
            hooks.on_exit.call(&exit_state);
            return (
                Err(RetryError::Cancelled {
                    last_result,
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
                last_result: attempt_last_result,
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

                    let exit_state = ExitState {
                        attempt,
                        outcome: Some(&attempt_last_result),
                        elapsed: elapsed_tracker.elapsed(),
                        total_wait,
                        reason: StopReason::Cancelled,
                    };
                    hooks.on_exit.call(&exit_state);

                    return (
                        Err(RetryError::Cancelled {
                            last_result: Some(attempt_last_result),
                            attempts: attempt,
                            total_elapsed: elapsed_tracker.elapsed(),
                        }),
                        stats,
                    );
                }

                last_result = Some(attempt_last_result);
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

#[cfg(feature = "alloc")]
pub(crate) fn poll_after_completion<T>(type_name: &str) -> Poll<T> {
    #[cfg(any(debug_assertions, feature = "strict-futures"))]
    panic!("{type_name} polled after completion");

    #[cfg(all(not(debug_assertions), not(feature = "strict-futures")))]
    {
        Poll::Pending
    }
}

#[cfg(feature = "alloc")]
// Intentional: this is the shared async state-machine engine used by both
// policy-based and extension-trait async retry futures.
#[allow(clippy::too_many_arguments)]
pub(crate) fn poll_async_loop<S, W, P, BA, AA, BS, OX, F, Fut, SleepImpl, T, E, SleepFut, C>(
    cx: &mut Context<'_>,
    policy: &mut RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    op: &mut F,
    sleeper: &SleepImpl,
    canceler: &C,
    last_result: &mut Option<Result<T, E>>,
    mut phase: Pin<&mut AsyncPhase<Fut, SleepFut, C::Cancel>>,
    attempt: &mut u32,
    total_wait: &mut Duration,
    collect_stats: bool,
    final_stats: &mut Option<RetryStats>,
    elapsed_tracker: &ElapsedTracker,
    completed_type_name: &'static str,
) -> Poll<Result<T, RetryError<E, T>>>
where
    S: Stop,
    W: Wait,
    P: Predicate<T, E>,
    BA: BeforeAttemptHook,
    AA: AttemptHook<T, E>,
    BS: AttemptHook<T, E>,
    OX: ExitHook<T, E>,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    SleepImpl: Sleeper<Sleep = SleepFut>,
    SleepFut: Future<Output = ()>,
    C: Canceler,
{
    loop {
        match phase.as_mut().project() {
            AsyncPhaseProj::ReadyToStartAttempt => {
                if canceler.is_cancelled() {
                    let stats = if collect_stats {
                        Some(RetryStats {
                            attempts: *attempt - 1,
                            total_elapsed: elapsed_tracker.elapsed(),
                            total_wait: *total_wait,
                            stop_reason: StopReason::Cancelled,
                        })
                    } else {
                        None
                    };
                    let exit_state = ExitState {
                        attempt: attempt.saturating_sub(1),
                        outcome: last_result.as_ref(),
                        elapsed: elapsed_tracker.elapsed(),
                        total_wait: *total_wait,
                        reason: StopReason::Cancelled,
                    };
                    hooks.on_exit.call(&exit_state);
                    *final_stats = stats;
                    phase.set(AsyncPhase::Finished);
                    return Poll::Ready(Err(RetryError::Cancelled {
                        last_result: last_result.take(),
                        attempts: *attempt - 1,
                        total_elapsed: elapsed_tracker.elapsed(),
                    }));
                }

                fire_before_attempt(hooks, *attempt, elapsed_tracker.elapsed(), *total_wait);

                phase.set(AsyncPhase::PollingOperation { op_future: (op)() });
            }
            AsyncPhaseProj::PollingOperation { op_future } => match poll_operation_future(
                op_future,
                cx,
                policy,
                hooks,
                *attempt,
                elapsed_tracker,
                *total_wait,
                collect_stats,
            ) {
                AsyncOperationPoll::Pending => return Poll::Pending,
                AsyncOperationPoll::Finished { result, stats } => {
                    *final_stats = stats;
                    phase.set(AsyncPhase::Finished);
                    return Poll::Ready(result);
                }
                AsyncOperationPoll::Sleep {
                    next_delay,
                    last_result: attempt_last_result,
                } => {
                    *last_result = Some(attempt_last_result);
                    *total_wait = total_wait.saturating_add(next_delay);
                    phase.set(AsyncPhase::Sleeping {
                        sleep_future: sleeper.sleep(next_delay),
                        cancel_future: canceler.cancel(),
                    });
                }
            },
            AsyncPhaseProj::Sleeping {
                sleep_future,
                cancel_future,
            } => {
                if let Poll::Ready(()) = cancel_future.poll(cx) {
                    let (result, stats) = finish_cancelled_async(
                        hooks,
                        last_result,
                        *attempt,
                        *total_wait,
                        collect_stats,
                        elapsed_tracker,
                    );

                    *final_stats = stats;
                    phase.set(AsyncPhase::Finished);
                    return Poll::Ready(result);
                }

                match sleep_future.poll(cx) {
                    Poll::Pending => {
                        if canceler.is_cancelled() {
                            let (result, stats) = finish_cancelled_async(
                                hooks,
                                last_result,
                                *attempt,
                                *total_wait,
                                collect_stats,
                                elapsed_tracker,
                            );

                            *final_stats = stats;
                            phase.set(AsyncPhase::Finished);
                            return Poll::Ready(result);
                        }
                        return Poll::Pending;
                    }
                    Poll::Ready(()) => {
                        if canceler.is_cancelled() {
                            let (result, stats) = finish_cancelled_async(
                                hooks,
                                last_result,
                                *attempt,
                                *total_wait,
                                collect_stats,
                                elapsed_tracker,
                            );

                            *final_stats = stats;
                            phase.set(AsyncPhase::Finished);
                            return Poll::Ready(result);
                        }

                        *attempt = attempt.saturating_add(1);
                        phase.set(AsyncPhase::ReadyToStartAttempt);
                    }
                }
            }
            AsyncPhaseProj::Finished => {
                return poll_after_completion(completed_type_name);
            }
        }
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
