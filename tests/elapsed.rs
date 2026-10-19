//! Tests for elapsed-time tracking on the synchronous path.
//!
//! Elapsed time is clock time since the execution's first attempt began: the
//! baseline starts at `.call()`, the operation and the wait strategy each see
//! their own snapshot, and a custom clock drives `stop::elapsed` without a
//! system clock.

mod common;

use common::{CUSTOM_CLOCK_DEADLINE, CUSTOM_CLOCK_STEP_MILLIS};

use core::cell::Cell;
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryPolicy, RetryState, stop, wait};

// Three whole 20 ms attempt steps (20, 40, 60) against a 50 ms deadline, so
// the loop stops on the third attempt.
const IDLE_BEFORE_CALL_MILLIS: u64 = 1_000;
const PER_ATTEMPT_MILLIS: u64 = 20;
const BASELINE_DEADLINE: Duration = Duration::from_millis(50);
use std::cell::RefCell;

/// End-to-end: the engine feeds each attempt's post-clamp delay forward as the
/// next attempt's `RetryState::previous_delay`. The unit tests construct
/// `RetryState` by hand; this proves the sync loop actually wires it, without
/// which feedback strategies (decorrelated jitter) silently degrade.
#[test]
fn engine_feeds_previous_delay_forward_sync() {
    struct RecordingWait<'a> {
        seen: &'a RefCell<Vec<Option<Duration>>>,
        delay: Duration,
    }
    impl relentless::Wait for RecordingWait<'_> {
        fn next_wait(&self, state: &RetryState) -> Duration {
            self.seen.borrow_mut().push(state.previous_delay);
            self.delay
        }
    }

    const FEEDBACK_DELAY: Duration = Duration::from_millis(7);
    let seen: RefCell<Vec<Option<Duration>>> = RefCell::new(Vec::new());

    let _ = RetryPolicy::new()
        .stop(stop::attempts(4))
        .wait(RecordingWait {
            seen: &seen,
            delay: FEEDBACK_DELAY,
        })
        .retry(|_| Err::<i32, &str>("fail"))
        .clock(VirtualClock::new())
        .call();

    // The wait strategy is consulted once per *retry* (not on the terminal
    // attempt, which stops before any wait). The first consultation sees no
    // previous delay; each later one sees the prior (constant) delay fed forward.
    assert_eq!(
        *seen.borrow(),
        vec![None, Some(FEEDBACK_DELAY), Some(FEEDBACK_DELAY)]
    );
}

/// The elapsed clock is read twice per attempt: once for the `RetryState` the
/// operation receives (attempt start) and again after the operation completes,
/// for the stop/wait evaluation. A clock that advances during the operation
/// must therefore show the operation an *earlier* elapsed than the wait
/// strategy sees for the same attempt. This guards the two-snapshot semantics
/// so engine refactors that thread `RetryState` as a unit cannot silently
/// collapse the pre-op and post-op snapshots into one.
#[cfg(feature = "alloc")]
#[test]
fn elapsed_is_snapshotted_separately_for_op_and_wait() {
    use std::rc::Rc;

    struct RecordingWait {
        seen: Rc<RefCell<Vec<Duration>>>,
    }
    impl relentless::Wait for RecordingWait {
        fn next_wait(&self, state: &relentless::RetryState) -> Duration {
            self.seen.borrow_mut().push(state.elapsed);
            Duration::ZERO
        }
    }

    let clock = VirtualClock::new();
    let op_elapsed: Cell<Option<Duration>> = Cell::new(None);
    let wait_elapsed: Rc<RefCell<Vec<Duration>>> = Rc::new(RefCell::new(Vec::new()));

    let _ = RetryPolicy::new()
        .stop(stop::attempts(2))
        .wait(RecordingWait {
            seen: Rc::clone(&wait_elapsed),
        })
        .retry(|state: RetryState| {
            // Record the elapsed the op sees on the first attempt, then advance
            // the clock so the post-op read differs.
            if op_elapsed.get().is_none() {
                op_elapsed.set(Some(state.elapsed));
            }
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .call();

    let op_saw = op_elapsed.get().expect("op ran at least once");
    let wait_saw = wait_elapsed.borrow()[0];
    assert!(
        wait_saw > op_saw,
        "wait should see post-op elapsed ({wait_saw:?}), later than the op ({op_saw:?})"
    );
}

#[test]
fn custom_elapsed_clock_drives_elapsed_stop_without_std_clock() {
    let policy = RetryPolicy::new().stop(stop::elapsed(CUSTOM_CLOCK_DEADLINE));
    let call_count = Cell::new(0_u32);
    let clock = VirtualClock::new();

    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            clock.advance(Duration::from_millis(CUSTOM_CLOCK_STEP_MILLIS));
            Err::<i32, _>("clocked failure")
        })
        .clock(&clock)
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}

/// 11.1.1 — the elapsed baseline is captured when execution starts (`.call()`),
/// not when the builder is configured. Idle time between configuring the
/// builder and calling it must not consume the elapsed budget.
#[test]
fn elapsed_baseline_starts_at_call_not_builder_construction() {
    let clock = VirtualClock::new();
    let policy = RetryPolicy::new()
        .stop(stop::elapsed(BASELINE_DEADLINE))
        .wait(wait::fixed(Duration::ZERO));
    let execution = policy
        .retry(|_| {
            clock.advance(Duration::from_millis(PER_ATTEMPT_MILLIS));
            Err::<i32, &str>("fail")
        })
        .clock(&clock)
        .with_stats();

    clock.advance(Duration::from_millis(IDLE_BEFORE_CALL_MILLIS));
    let (result, stats) = execution.call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.attempts, 3);
}
