//! Acceptance tests for the sync builder (`retry(..)` and `RetryExt`).
//!
//! Each test drives one builder surface end to end under a virtual clock:
//! the default classifier path, `.decide`/`.when`/`.until`, self-classifying
//! outcomes, the three hook points, `.with_stats()` and `.timeout`.

use core::cell::Cell;
use core::time::Duration;

use relentless::clock::VirtualClock;
use relentless::{
    AttemptState, Decision, Exit, Outcome, RetryError, RetryExt, StopReason, Verdict, predicate,
    retry, stop, wait,
};

const ARBITRARY_ATTEMPTS: u32 = 5;

/// A non-`Result` poll outcome that classifies itself, so the default path
/// drives it with no `.decide` at the call site.
#[derive(Debug, PartialEq)]
enum Poll {
    Pending,
    Ready(i32),
    Failed(&'static str),
}

impl Outcome for Poll {
    type Return = i32;
    type Abort = &'static str;

    fn classify(self) -> Verdict<i32, &'static str, Poll> {
        match self {
            Poll::Ready(value) => Verdict::Return(value),
            Poll::Failed(error) => Verdict::Abort(error),
            Poll::Pending => Verdict::Retry(self),
        }
    }
}

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

#[test]
fn owned_outcome_type_returns_via_the_default_path() {
    let counter = Cell::new(0);
    let result = retry(|_| {
        let n = counter.get() + 1;
        counter.set(n);
        if n >= 3 {
            Poll::Ready(9)
        } else {
            Poll::Pending
        }
    })
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(9));
    assert_eq!(counter.get(), 3);
}

#[test]
fn owned_outcome_type_aborts_via_the_default_path() {
    let result = retry(|_| Poll::Failed("io"))
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(result, Err(RetryError::Aborted { last: "io" }));
}

type IntResult = Result<i32, &'static str>;

#[test]
fn after_attempt_fires_before_classification_on_every_attempt() {
    // `after_attempt` observes the raw outcome for every attempt, including
    // the terminal `Ok` — proving it fires before the classifier consumes it.
    let before = Cell::new(0u32);
    let after = Cell::new(0u32);
    let last_after_ok = Cell::new(false);

    let counter = Cell::new(0);
    let result = retry(|_| {
        let n = counter.get() + 1;
        counter.set(n);
        if n >= 2 { Ok::<i32, &str>(7) } else { Err("x") }
    })
    .before_attempt(|_| before.set(before.get() + 1))
    .after_attempt(|s: &AttemptState<IntResult>| {
        after.set(after.get() + 1);
        last_after_ok.set(s.outcome.is_ok());
    })
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(7));
    assert_eq!(before.get(), 2);
    assert_eq!(after.get(), 2);
    assert!(last_after_ok.get(), "after_attempt saw the terminal Ok");
}

#[test]
fn on_exit_reports_returned_with_the_final_attempt_count() {
    let seen = Cell::new(None);
    let _ = retry(|_| Ok::<i32, &str>(1))
        .on_exit(|e: &Exit<i32, &str, IntResult>| {
            seen.set(Some((e.stop_reason(), e.attempt())));
        })
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(seen.get(), Some((StopReason::Returned, 1)));
}

#[test]
fn on_exit_reports_aborted_from_a_rejected_error() {
    let reason = Cell::new(None);
    let _ = retry(|_| Err::<i32, &str>("fatal"))
        .when(predicate::error(|e: &&str| *e == "transient"))
        .on_exit(|e: &Exit<i32, &str, IntResult>| reason.set(Some(e.stop_reason())))
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(reason.get(), Some(StopReason::Aborted));
}

#[test]
fn on_exit_reports_exhausted_after_the_full_stop_budget() {
    let reason = Cell::new(None);
    let after = Cell::new(0u32);
    let _ = retry(|_| Err::<i32, &str>("boom"))
        .after_attempt(|_: &AttemptState<IntResult>| after.set(after.get() + 1))
        .on_exit(|e: &Exit<i32, &str, IntResult>| reason.set(Some(e.stop_reason())))
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::ZERO))
        .clock(VirtualClock::new())
        .call();

    assert_eq!(reason.get(), Some(StopReason::Exhausted));
    assert_eq!(
        after.get(),
        3,
        "after_attempt fires on the terminal attempt too"
    );
}

#[test]
fn retry_ext_starts_from_a_no_arg_closure() {
    let counter = Cell::new(0);
    let result = (|| {
        let n = counter.get() + 1;
        counter.set(n);
        if n >= 2 { Ok::<i32, &str>(5) } else { Err("x") }
    })
    .retry()
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(5));
    assert_eq!(counter.get(), 2);
}

#[test]
fn retry_ext_supports_the_classifier_surface() {
    // The stateless-ext builder still reaches `.until`, because the op slot
    // is unified behind `RetryOp` rather than a bare `FnMut`.
    let counter = Cell::new(0);
    let result = (|| {
        counter.set(counter.get() + 1);
        Ok::<i32, &str>(counter.get())
    })
    .retry()
    .until(predicate::ok(|v: &i32| *v >= 3))
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(Duration::ZERO))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(3));
}

#[test]
fn with_stats_reports_attempts_wait_elapsed_and_reason() {
    let (result, stats) = retry(|_| Err::<i32, &str>("boom"))
        .stop(stop::attempts(3))
        .wait(wait::fixed(Duration::from_millis(5)))
        .clock(VirtualClock::new())
        .with_stats()
        .call();

    assert_eq!(result, Err(RetryError::Exhausted { last: Err("boom") }));
    assert_eq!(stats.attempts, 3);
    // Two inter-attempt waits of 5ms; the terminal attempt does not wait.
    assert_eq!(stats.total_wait, Duration::from_millis(10));
    assert_eq!(stats.total_elapsed, Duration::from_millis(10));
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
}

#[test]
fn timeout_stops_the_loop_once_the_budget_is_exceeded() {
    // 10ms waits under a 25ms budget: attempts run at t=0,10,20,25; the last
    // wait is clamped to 5ms to land exactly on the deadline, where the next
    // boundary check exhausts the loop.
    let (result, stats) = retry(|_| Err::<i32, &str>("x"))
        .stop(stop::attempts(100))
        .wait(wait::fixed(Duration::from_millis(10)))
        .timeout(Duration::from_millis(25))
        .clock(VirtualClock::new())
        .with_stats()
        .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.attempts, 4);
    assert_eq!(stats.total_elapsed, Duration::from_millis(25));
    assert_eq!(stats.total_wait, Duration::from_millis(25));
    assert_eq!(stats.stop_reason, StopReason::Exhausted);
}
