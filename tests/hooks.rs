//! Tests for `before_attempt`, `after_attempt`, and `on_exit` hooks.
//!
//! Verifies call ordering relative to the operation and classification, the
//! `Exit` view delivered to `on_exit`, and that multiple hooks of the same kind
//! fire in registration order.
mod common;

use common::{ARBITRARY_ATTEMPTS, ARBITRARY_DELAY, ARBITRARY_VALUE};

use core::cell::Cell;
use core::time::Duration;
use relentless::RetryPolicy;
use relentless::clock::VirtualClock;
use relentless::{StopReason, predicate, stop, wait};
use std::cell::RefCell;

#[test]
fn before_attempt_and_after_attempt_fire_at_defined_points() {
    let events: RefCell<Vec<(char, u32)>> = RefCell::new(Vec::new());
    let op_attempt = Cell::new(0_u32);

    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let _ = policy
        .retry(|_| {
            let attempt = op_attempt.get().saturating_add(1);
            op_attempt.set(attempt);
            events.borrow_mut().push(('o', attempt));
            Err::<i32, _>("fail")
        })
        .before_attempt(|state| events.borrow_mut().push(('b', state.attempt)))
        .after_attempt(|state: &relentless::AttemptState<Result<i32, &str>>| {
            events.borrow_mut().push(('a', state.attempt));
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(
        *events.borrow(),
        vec![
            ('b', 1),
            ('o', 1),
            ('a', 1),
            ('b', 2),
            ('o', 2),
            ('a', 2),
            ('b', 3),
            ('o', 3),
            ('a', 3),
        ]
    );
}

/// SPEC 3.6: `RetryState.previous_delay` is "the delay applied before this
/// attempt", shared with the `before_attempt` hook — `None` only on the first
/// attempt.
#[test]
fn before_attempt_sees_previous_delay() {
    let delays: RefCell<Vec<Option<Duration>>> = RefCell::new(Vec::new());

    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(ARBITRARY_DELAY));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .before_attempt(|state| delays.borrow_mut().push(state.previous_delay))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(
        *delays.borrow(),
        vec![None, Some(ARBITRARY_DELAY), Some(ARBITRARY_DELAY)]
    );
}

#[test]
fn after_attempt_runs_before_classification() {
    // ADR-6: after_attempt fires once per attempt *before* the classifier
    // consumes the outcome, so it observes the classifier not-yet-run for the
    // current attempt (counter is 0 then 1, never having advanced for this
    // attempt yet).
    let classifier_calls = Cell::new(0_u32);
    let after_observed: RefCell<Vec<u32>> = RefCell::new(Vec::new());

    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .when(predicate::result(|_outcome: &Result<i32, &str>| {
            classifier_calls.set(classifier_calls.get().saturating_add(1));
            true
        }));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .after_attempt(|_state: &relentless::AttemptState<Result<i32, &str>>| {
            after_observed.borrow_mut().push(classifier_calls.get());
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(*after_observed.borrow(), vec![0, 1]);
}

/// SPEC 8.3
#[test]
fn on_exit_fires_once_with_final_state() {
    let exits: RefCell<Vec<(u32, bool, StopReason)>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .on_exit(|exit: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            let outcome_is_err = match exit {
                relentless::Exit::Returned { .. } => false,
                relentless::Exit::Exhausted { last, .. } => last.is_err(),
                _ => true,
            };
            exits
                .borrow_mut()
                .push((exit.attempt(), outcome_is_err, exit.stop_reason()));
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(
        *exits.borrow(),
        vec![(ARBITRARY_ATTEMPTS, true, StopReason::Exhausted)]
    );
}

#[test]
fn on_exit_reports_non_retryable_error_reason() {
    let reasons: RefCell<Vec<StopReason>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .when(predicate::error(|err: &&str| *err == "retryable"));

    let _ = policy
        .retry(|_| Err::<i32, _>("fatal"))
        .on_exit(|exit: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            reasons.borrow_mut().push(exit.stop_reason());
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(*reasons.borrow(), vec![StopReason::Aborted]);
}

#[test]
fn multiple_hooks_of_same_kind_fire_in_registration_order() {
    let calls: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());

    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .before_attempt(|_state| calls.borrow_mut().push("before_1"))
        .before_attempt(|_state| calls.borrow_mut().push("before_2"))
        .after_attempt(|_state: &relentless::AttemptState<Result<i32, &str>>| {
            calls.borrow_mut().push("after_1");
        })
        .after_attempt(|_state: &relentless::AttemptState<Result<i32, &str>>| {
            calls.borrow_mut().push("after_2");
        })
        .on_exit(|_e: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            calls.borrow_mut().push("exit_1");
        })
        .on_exit(|_e: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            calls.borrow_mut().push("exit_2");
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(
        *calls.borrow(),
        vec![
            "before_1", "before_2", "after_1", "after_2", "exit_1", "exit_2",
        ]
    );
}

/// 3.6.6
#[test]
fn exit_attempt_is_exactly_one_for_single_attempt_stop() {
    use relentless::{Exit, RetryPolicy, stop};
    use std::cell::Cell;

    let exit_attempt = Cell::new(0_u32);
    let policy = RetryPolicy::new().stop(stop::attempts(1));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .on_exit(|exit: &Exit<i32, &str, Result<i32, &str>>| {
            exit_attempt.set(exit.attempt());
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(exit_attempt.get(), 1);
}

/// 3.6.6
#[test]
fn exit_attempt_matches_completed_attempts() {
    use relentless::{Exit, RetryPolicy, stop, wait};
    use std::cell::Cell;

    let exit_attempt = Cell::new(0_u32);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .on_exit(|exit: &Exit<i32, &str, Result<i32, &str>>| {
            exit_attempt.set(exit.attempt());
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(exit_attempt.get(), 3);
}

/// 3.6.7
#[test]
fn exit_exposes_the_final_exhausted_outcome() {
    use relentless::{Exit, RetryPolicy, stop};
    use std::cell::RefCell;

    let final_outcome: RefCell<Option<bool>> = RefCell::new(None); // true=ok, false=err

    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let _ = policy
        .retry(|_| Err::<i32, &str>("final error"))
        .on_exit(|exit: &Exit<i32, &str, Result<i32, &str>>| {
            if let Exit::Exhausted { last, .. } = exit {
                *final_outcome.borrow_mut() = Some(last.is_ok());
            }
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(*final_outcome.borrow(), Some(false));
}

/// 3.6.6
#[test]
fn exit_reports_elapsed_since_first_attempt() {
    use relentless::{Exit, RetryPolicy, stop};
    use std::cell::Cell;

    // The op advances the virtual clock by one step before failing, so the
    // terminal `Exit` must report exactly that elapsed — not zero.
    const STEP: Duration = Duration::from_millis(50);
    let clock = VirtualClock::new();
    let exit_elapsed = Cell::new(Duration::ZERO);

    let _ = RetryPolicy::new()
        .stop(stop::attempts(1))
        .retry(|_| {
            clock.advance(STEP);
            Err::<i32, &str>("fail")
        })
        .on_exit(|exit: &Exit<i32, &str, Result<i32, &str>>| {
            exit_elapsed.set(exit.elapsed());
        })
        .clock(&clock)
        .call();

    assert_eq!(exit_elapsed.get(), STEP);
}

#[test]
fn hooks_are_per_call_and_do_not_persist_across_retries() {
    let before_calls = Cell::new(0_u32);
    let policy = RetryPolicy::new().stop(stop::attempts(2));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .before_attempt(|_state| {
            before_calls.set(before_calls.get().saturating_add(1));
        })
        .clock(VirtualClock::new())
        .call();
    assert_eq!(before_calls.get(), 2);

    let _ = policy
        .retry(|_| Err::<i32, _>("fail again"))
        .clock(VirtualClock::new())
        .call();
    assert_eq!(
        before_calls.get(),
        2,
        "hook should not carry over to later retry invocations"
    );
}

#[test]
fn after_attempt_hook_fires_after_each_attempt() {
    let hook_results: RefCell<Vec<(u32, bool)>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let call_count = Cell::new(0_u32);
    let _ = policy
        .retry(|_| {
            let n = call_count.get() + 1;
            call_count.set(n);
            if n < ARBITRARY_ATTEMPTS {
                Err("fail")
            } else {
                Ok(ARBITRARY_VALUE)
            }
        })
        .after_attempt(|state: &relentless::AttemptState<Result<i32, &str>>| {
            let is_ok = state.outcome.is_ok();
            hook_results.borrow_mut().push((state.attempt, is_ok));
        })
        .clock(VirtualClock::new())
        .call();

    let results = hook_results.borrow();
    assert_eq!(results.len(), ARBITRARY_ATTEMPTS as usize);
    assert_eq!(results[0], (1, false));
    assert_eq!(results[1], (2, false));
    assert_eq!(results[2], (3, true));
}

#[test]
fn on_exit_hook_fires_when_stop_triggers() {
    let exit_reason = Cell::new(None);
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let _ = policy
        .retry(|_| Err::<i32, _>("fail"))
        .on_exit(|exit: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            exit_reason.set(Some(exit.stop_reason()));
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(exit_reason.get(), Some(StopReason::Exhausted));
}

#[test]
fn on_exit_hook_fires_on_success() {
    let exit_reason = Cell::new(None);
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let _ = policy
        .retry(|_| Ok::<_, &str>(ARBITRARY_VALUE))
        .on_exit(|exit: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            exit_reason.set(Some(exit.stop_reason()));
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(exit_reason.get(), Some(StopReason::Returned));
}

/// 7.2.1
#[test]
fn after_attempt_fires_including_final_attempt() {
    let attempt_nums: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new().stop(stop::attempts(ARBITRARY_ATTEMPTS));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .after_attempt(|state: &relentless::AttemptState<Result<i32, &str>>| {
            attempt_nums.borrow_mut().push(state.attempt);
        })
        .clock(VirtualClock::new())
        .call();

    assert_eq!(*attempt_nums.borrow(), vec![1, 2, 3]);
}

/// §6.4
#[test]
fn sleep_occurs_after_after_attempt_hook_fires() {
    /// Clock that logs each wait into a shared event list.
    struct EventClock<'a> {
        events: &'a RefCell<Vec<&'static str>>,
        now: Cell<Duration>,
    }
    impl relentless::Clock for EventClock<'_> {
        fn now(&self) -> Duration {
            self.now.get()
        }
    }
    impl relentless::SyncClock for EventClock<'_> {
        fn wait(&self, dur: Duration) {
            self.events.borrow_mut().push("sleep");
            self.now.set(self.now.get().saturating_add(dur));
        }
    }

    let events: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(wait::fixed(ARBITRARY_DELAY));

    let _ = policy
        .retry(|_| Err::<i32, &str>("fail"))
        .after_attempt(|_: &relentless::AttemptState<Result<i32, &str>>| {
            events.borrow_mut().push("after_attempt");
        })
        .clock(EventClock {
            events: &events,
            now: Cell::new(Duration::ZERO),
        })
        .call();

    // after_attempt fires, then sleep fires, then final after_attempt (no sleep after terminal).
    assert_eq!(
        *events.borrow(),
        vec!["after_attempt", "sleep", "after_attempt"]
    );
}

#[test]
fn on_exit_aborted_reports_attempt_count_and_elapsed() {
    let seen: RefCell<Vec<(u32, Duration)>> = RefCell::new(Vec::new());
    let policy = RetryPolicy::new()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(ARBITRARY_DELAY))
        .when(predicate::error(|err: &&str| *err == "retryable"));
    let clock = VirtualClock::new();
    let first = Cell::new(true);

    let _ = policy
        .retry(|_| {
            if first.replace(false) {
                Err::<i32, _>("retryable")
            } else {
                Err("fatal")
            }
        })
        .on_exit(|exit: &relentless::Exit<i32, &str, Result<i32, &str>>| {
            seen.borrow_mut().push((exit.attempt(), exit.elapsed()));
        })
        .clock(&clock)
        .call();

    assert_eq!(*seen.borrow(), vec![(2, ARBITRARY_DELAY)]);
}
