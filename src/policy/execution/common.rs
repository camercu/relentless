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
    AttemptState::new(
        retry_state.attempt,
        outcome,
        retry_state.elapsed,
        retry_state.next_delay,
        retry_state.total_wait,
    )
}

fn exit_state_from<'a, T, E>(
    attempt_state: &AttemptState<'a, T, E>,
    reason: StopReason,
) -> ExitState<'a, T, E> {
    ExitState::new(
        attempt_state.attempt,
        Some(attempt_state.outcome),
        attempt_state.elapsed,
        attempt_state.total_wait,
        reason,
    )
}

#[derive(Clone, Copy)]
enum TerminalOutcomeKind {
    PredicateAccepted { predicate_is_default: bool },
    StopCondition,
}

fn maybe_stats(
    collect_stats: bool,
    attempts: u32,
    total_elapsed: Option<Duration>,
    total_wait: Duration,
    stop_reason: StopReason,
) -> Option<RetryStats> {
    if collect_stats {
        Some(RetryStats {
            attempts,
            total_elapsed,
            total_wait,
            stop_reason,
        })
    } else {
        None
    }
}

fn finish_cancelled<T, E, BA, AA, BS, OX>(
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    last_result: Option<Result<T, E>>,
    attempts: u32,
    total_wait: Duration,
    collect_stats: bool,
    elapsed_tracker: &ElapsedTracker,
) -> (Result<T, RetryError<E, T>>, Option<RetryStats>)
where
    OX: ExitHook<T, E>,
{
    let elapsed = elapsed_tracker.elapsed();
    let stats = maybe_stats(
        collect_stats,
        attempts,
        elapsed,
        total_wait,
        StopReason::Cancelled,
    );

    let exit_state = ExitState::new(
        attempts,
        last_result.as_ref(),
        elapsed,
        total_wait,
        StopReason::Cancelled,
    );
    hooks.on_exit.call(&exit_state);

    (
        Err(RetryError::Cancelled {
            last: last_result,
            attempts,
            total_elapsed: elapsed,
        }),
        stats,
    )
}

fn finish_terminal_transition<BA, AA, BS, OX, T, E>(
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    retry_state: &RetryState,
    outcome: Result<T, E>,
    collect_stats: bool,
    outcome_kind: TerminalOutcomeKind,
) -> AttemptTransition<T, E>
where
    OX: ExitHook<T, E>,
{
    let reason = match outcome_kind {
        TerminalOutcomeKind::PredicateAccepted {
            predicate_is_default,
        } => stop_reason_for_predicate_accept(&outcome, predicate_is_default),
        TerminalOutcomeKind::StopCondition => StopReason::StopCondition,
    };

    let stats = maybe_stats(
        collect_stats,
        retry_state.attempt,
        retry_state.elapsed,
        retry_state.total_wait,
        reason,
    );

    {
        let attempt_state = attempt_state_from(retry_state, &outcome);
        let exit_state = exit_state_from(&attempt_state, reason);
        hooks.on_exit.call(&exit_state);
    }

    let result = match outcome_kind {
        TerminalOutcomeKind::PredicateAccepted { .. } => match outcome {
            Ok(value) => Ok(value),
            Err(error) => Err(RetryError::PredicateRejected {
                last: Err(error),
                attempts: retry_state.attempt,
                total_elapsed: retry_state.elapsed,
            }),
        },
        TerminalOutcomeKind::StopCondition => match outcome {
            Err(error) => Err(RetryError::Exhausted {
                last: Err(error),
                attempts: retry_state.attempt,
                total_elapsed: retry_state.elapsed,
            }),
            Ok(last) => Err(RetryError::ConditionNotMet {
                last: Ok(last),
                attempts: retry_state.attempt,
                total_elapsed: retry_state.elapsed,
            }),
        },
    };

    AttemptTransition::Finished { result, stats }
}

#[cfg(feature = "alloc")]
fn finish_async_poll<T, E, Fut, SleepFut, CancelFut>(
    mut phase: Pin<&mut AsyncPhase<Fut, SleepFut, CancelFut>>,
    final_stats: &mut Option<RetryStats>,
    result: Result<T, RetryError<E, T>>,
    stats: Option<RetryStats>,
) -> Poll<Result<T, RetryError<E, T>>> {
    *final_stats = stats;
    phase.set(AsyncPhase::Finished);
    Poll::Ready(result)
}

#[cfg(feature = "alloc")]
#[allow(clippy::too_many_arguments)]
fn finish_cancelled_async_poll<BA, AA, BS, OX, T, E, Fut, SleepFut, CancelFut>(
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    last_result: &mut Option<Result<T, E>>,
    attempts: u32,
    total_wait: Duration,
    collect_stats: bool,
    elapsed_tracker: &ElapsedTracker,
    phase: Pin<&mut AsyncPhase<Fut, SleepFut, CancelFut>>,
    final_stats: &mut Option<RetryStats>,
) -> Poll<Result<T, RetryError<E, T>>>
where
    OX: ExitHook<T, E>,
{
    let (result, stats) = finish_cancelled(
        hooks,
        last_result.take(),
        attempts,
        total_wait,
        collect_stats,
        elapsed_tracker,
    );
    finish_async_poll(phase, final_stats, result, stats)
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

pub(crate) fn fire_before_attempt<BA, AA, BS, OX>(
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    attempt: u32,
    elapsed: Option<Duration>,
    total_wait: Duration,
) where
    BA: BeforeAttemptHook,
{
    let before_state = BeforeAttemptState::new(attempt, elapsed, total_wait);
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

pub(super) fn process_attempt_transition<S, W, P, BA, AA, BS, OX, T, E>(
    policy: &mut RetryPolicy<S, W, P>,
    hooks: &mut ExecutionHooks<BA, AA, BS, OX>,
    outcome: Result<T, E>,
    mut retry_state: RetryState,
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
    let should_retry = policy.predicate.should_retry(&outcome);
    {
        let attempt_state = attempt_state_from(&retry_state, &outcome);
        hooks.after_attempt.call(&attempt_state);
    }

    if !should_retry {
        return finish_terminal_transition(
            hooks,
            &retry_state,
            outcome,
            collect_stats,
            TerminalOutcomeKind::PredicateAccepted {
                predicate_is_default: policy.meta.predicate_is_default,
            },
        );
    }

    let next_delay = policy.wait.next_wait(&retry_state);
    retry_state.next_delay = next_delay;

    if policy.stop.should_stop(&retry_state) {
        return finish_terminal_transition(
            hooks,
            &retry_state,
            outcome,
            collect_stats,
            TerminalOutcomeKind::StopCondition,
        );
    }

    {
        let attempt_state = attempt_state_from(&retry_state, &outcome);
        hooks.before_sleep.call(&attempt_state);
    }

    AttemptTransition::Sleep {
        next_delay,
        last_result: outcome,
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
    let retry_state = RetryState::new(attempt, elapsed, Duration::ZERO, total_wait);

    process_attempt_transition(policy, hooks, outcome, retry_state, collect_stats)
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
            let completed_attempts = attempt.saturating_sub(1);
            return finish_cancelled(
                hooks,
                last_result.take(),
                completed_attempts,
                total_wait,
                COLLECT_STATS,
                &elapsed_tracker,
            );
        }

        fire_before_attempt(hooks, attempt, elapsed_tracker.elapsed(), total_wait);

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
                    return finish_cancelled(
                        hooks,
                        Some(attempt_last_result),
                        attempt,
                        total_wait,
                        COLLECT_STATS,
                        &elapsed_tracker,
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
        let _ = type_name;
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
        if canceler.is_cancelled() {
            let completed_attempts = match phase.as_ref().get_ref() {
                AsyncPhase::ReadyToStartAttempt => Some(attempt.saturating_sub(1)),
                AsyncPhase::Sleeping { .. } => Some(*attempt),
                AsyncPhase::PollingOperation { .. } | AsyncPhase::Finished => None,
            };
            if let Some(attempts) = completed_attempts {
                return finish_cancelled_async_poll(
                    hooks,
                    last_result,
                    attempts,
                    *total_wait,
                    collect_stats,
                    elapsed_tracker,
                    phase,
                    final_stats,
                );
            }
        }

        match phase.as_mut().project() {
            AsyncPhaseProj::ReadyToStartAttempt => {
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
                    return finish_async_poll(phase, final_stats, result, stats);
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
                    return finish_cancelled_async_poll(
                        hooks,
                        last_result,
                        *attempt,
                        *total_wait,
                        collect_stats,
                        elapsed_tracker,
                        phase,
                        final_stats,
                    );
                }

                let sleep_poll = sleep_future.poll(cx);
                if canceler.is_cancelled() {
                    return finish_cancelled_async_poll(
                        hooks,
                        last_result,
                        *attempt,
                        *total_wait,
                        collect_stats,
                        elapsed_tracker,
                        phase,
                        final_stats,
                    );
                }

                match sleep_poll {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => {
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
    _predicate_is_default: bool,
) -> StopReason {
    if outcome.is_ok() {
        StopReason::Success
    } else {
        StopReason::PredicateAccepted
    }
}
