//! Acceptance tests for the `retry` and `retry_async` defaults.
//!
//! The free functions start a retry chain without a `RetryPolicy`. Tests verify
//! that the default policy they apply matches `RetryPolicy::default()`, that
//! stateful `Stop`/`Wait` implementors work through them, and that the builders
//! drive to completion — a regression guard for the public type API.
#![cfg(feature = "std")]

mod common;

use common::{
    ARBITRARY_ATTEMPTS, ARBITRARY_ERROR, ARBITRARY_VALUE, CLOCK_STEP_MILLIS, TIMEOUT_DEADLINE,
};

use common::block_on;

use core::cell::Cell;
use core::time::Duration;
use std::rc::Rc;

#[cfg(feature = "alloc")]
use relentless::clock::Recording;
use relentless::clock::VirtualClock;
use relentless::{RetryError, RetryPolicy, RetryState, Stop, Wait, retry, stop, wait};

const DEFAULT_INITIAL_WAIT: Duration = Duration::from_millis(100);
const DEFAULT_SECOND_WAIT: Duration = Duration::from_millis(200);
const DEFAULT_WAIT_SEQUENCE: [Duration; 2] = [DEFAULT_INITIAL_WAIT, DEFAULT_SECOND_WAIT];
/// The delay `StatefulWait` returns; asserted against the recorded waits.
const WAIT_DURATION: Duration = Duration::from_millis(1);
const STATEFUL_STOP_THRESHOLD: u32 = 2;
const UNTIL_TARGET: u32 = 3;

struct StatefulStop {
    consultations: Cell<u32>,
    threshold: u32,
}

impl Stop for StatefulStop {
    fn should_stop(&self, _state: &relentless::RetryState) -> bool {
        let next = self.consultations.get().saturating_add(1);
        self.consultations.set(next);
        next >= self.threshold
    }
}

struct StatefulWait {
    calls: Cell<u32>,
}

impl Wait for StatefulWait {
    fn next_wait(&self, _state: &relentless::RetryState) -> Duration {
        let next = self.calls.get().saturating_add(1);
        self.calls.set(next);
        Duration::from_millis(u64::from(next))
    }
}

#[test]
fn retry_closure_form_retries_until_success() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);

    let result: relentless::RetryResult<i32, &str> = retry(move |_| {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        if attempts_ref.get() < ARBITRARY_ATTEMPTS {
            Err(ARBITRARY_ERROR)
        } else {
            Ok(ARBITRARY_VALUE)
        }
    })
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(WAIT_DURATION))
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(attempts.get(), ARBITRARY_ATTEMPTS);
}

#[test]
fn retry_uses_default_policy_when_not_overridden() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);
    let clock = Recording::new(VirtualClock::new());

    let result = retry(move |_| {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        Err::<i32, &str>(ARBITRARY_ERROR)
    })
    .clock(&clock)
    .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(attempts.get(), ARBITRARY_ATTEMPTS);
    assert_eq!(clock.waits(), DEFAULT_WAIT_SEQUENCE);
}

#[test]
fn retry_call_with_stats_reports_attempts() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);

    let (result, stats): (relentless::RetryResult<i32, &str>, relentless::RetryStats) =
        retry(move |_| {
            attempts_ref.set(attempts_ref.get().saturating_add(1));
            Err(ARBITRARY_ERROR)
        })
        .stop(stop::attempts(2))
        .clock(VirtualClock::new())
        .call_with_stats();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(stats.attempts, 2);
    assert_eq!(attempts.get(), 2);
}

#[test]
fn retry_stateful_stop_and_wait_work() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);
    let clock = Recording::new(VirtualClock::new());

    let result = retry(move |_| {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        Err::<i32, &str>(ARBITRARY_ERROR)
    })
    .stop(StatefulStop {
        consultations: Cell::new(0),
        threshold: STATEFUL_STOP_THRESHOLD,
    })
    .wait(StatefulWait {
        calls: Cell::new(0),
    })
    .clock(&clock)
    .call();

    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    assert_eq!(attempts.get(), STATEFUL_STOP_THRESHOLD);
    assert_eq!(clock.waits(), vec![WAIT_DURATION]);
}

#[test]
fn retry_condition_not_met_for_ok_exhaustion() {
    let result = retry(|_| Ok::<i32, &str>(-1))
        .stop(stop::attempts(2))
        .until(|_value: &i32| false)
        .clock(VirtualClock::new())
        .call();

    assert!(matches!(
        result,
        Err(RetryError::Exhausted { last: Ok(-1), .. })
    ));
}

#[test]
fn retry_non_retryable_error_returns_immediately() {
    let result = retry(|_| Err::<i32, &str>(ARBITRARY_ERROR))
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .when(|err: &&str| *err == "retryable")
        .clock(VirtualClock::new())
        .call();

    assert!(matches!(
        result,
        Err(RetryError::Aborted {
            last: ARBITRARY_ERROR,
            ..
        })
    ));
}

#[test]
fn retry_until_retries_until_predicate_fires() {
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);

    let result = retry(move |_| {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        Ok::<u32, &str>(attempts_ref.get())
    })
    .stop(stop::attempts(5))
    .until(move |v: &u32| *v >= UNTIL_TARGET)
    .clock(VirtualClock::new())
    .call();

    assert_eq!(result, Ok(UNTIL_TARGET));
    assert_eq!(attempts.get(), UNTIL_TARGET);
}

#[test]
fn sync_retry_builder_debug_format() {
    let builder = retry(|_| Ok::<i32, &str>(1)).clock(VirtualClock::new());
    let debug = format!("{builder:?}");
    // The builder Debug is opaque (finish_non_exhaustive).
    assert!(debug.contains("Retry"));
}

#[cfg(feature = "alloc")]
mod async_tests {
    use core::future::{Future, ready};

    use common::ReadyClock;
    use core::pin::Pin;
    use core::task::{Context, Poll, Waker};

    use super::*;
    use relentless::retry_async;

    #[test]
    fn retry_async_retries_until_success() {
        let attempts = Rc::new(Cell::new(0_u32));
        let attempts_ref = Rc::clone(&attempts);

        let future = retry_async(move |_| {
            attempts_ref.set(attempts_ref.get().saturating_add(1));
            if attempts_ref.get() < ARBITRARY_ATTEMPTS {
                ready(Err(ARBITRARY_ERROR))
            } else {
                ready(Ok(ARBITRARY_VALUE))
            }
        })
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::from_millis(1)))
        .clock(ReadyClock)
        .call();

        let result: relentless::RetryResult<i32, &str> = block_on(future);
        assert_eq!(result, Ok(ARBITRARY_VALUE));
        assert_eq!(attempts.get(), ARBITRARY_ATTEMPTS);
    }

    #[test]
    fn retry_async_uses_default_policy_when_not_overridden() {
        let attempts = Rc::new(Cell::new(0_u32));
        let attempts_ref = Rc::clone(&attempts);
        let clock = Recording::new(VirtualClock::new());

        let result = block_on(
            retry_async(move |_| {
                attempts_ref.set(attempts_ref.get().saturating_add(1));
                ready::<Result<i32, &str>>(Err(ARBITRARY_ERROR))
            })
            .clock(&clock)
            .call(),
        );

        assert!(matches!(result, Err(RetryError::Exhausted { .. })));
        assert_eq!(attempts.get(), ARBITRARY_ATTEMPTS);
        assert_eq!(clock.waits(), DEFAULT_WAIT_SEQUENCE);
    }

    #[test]
    fn retry_async_call_with_stats_reports_attempts() {
        let future = retry_async(|_| ready::<Result<i32, &str>>(Err(ARBITRARY_ERROR)))
            .stop(stop::attempts(2))
            .when(|_: &&str| true)
            .clock(ReadyClock)
            .call_with_stats();

        let (result, stats) = block_on(future);
        assert!(matches!(result, Err(RetryError::Exhausted { .. })));
        assert_eq!(stats.attempts, 2);
    }

    #[test]
    fn retry_async_stateful_stop_and_wait_work() {
        let attempts = Rc::new(Cell::new(0_u32));
        let attempts_ref = Rc::clone(&attempts);
        let clock = Recording::new(VirtualClock::new());

        let result = block_on(
            retry_async(move |_| {
                attempts_ref.set(attempts_ref.get().saturating_add(1));
                ready::<Result<i32, &str>>(Err(ARBITRARY_ERROR))
            })
            .stop(StatefulStop {
                consultations: Cell::new(0),
                threshold: STATEFUL_STOP_THRESHOLD,
            })
            .wait(StatefulWait {
                calls: Cell::new(0),
            })
            .clock(&clock)
            .call(),
        );

        assert!(matches!(result, Err(RetryError::Exhausted { .. })));
        assert_eq!(attempts.get(), STATEFUL_STOP_THRESHOLD);
        assert_eq!(clock.waits(), vec![WAIT_DURATION]);
    }

    #[test]
    fn retry_async_repoll_after_completion_panics() {
        let mut retry = Box::pin(
            retry_async(|_| ready(Ok::<i32, &str>(ARBITRARY_VALUE)))
                .stop(stop::attempts(1))
                .clock(ReadyClock)
                .call(),
        );
        let mut cx = Context::from_waker(Waker::noop());

        let first_poll = Future::poll(Pin::as_mut(&mut retry), &mut cx);
        assert_eq!(first_poll, Poll::Ready(Ok(ARBITRARY_VALUE)));

        let second_poll = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = Future::poll(Pin::as_mut(&mut retry), &mut cx);
        }));
        assert!(second_poll.is_err());
    }

    #[test]
    fn free_function_retry_async_uses_default_policy() {
        let attempts = Rc::new(Cell::new(0_u32));
        let attempts_ref = Rc::clone(&attempts);

        let result = block_on(
            relentless::retry_async(move |_| {
                attempts_ref.set(attempts_ref.get().saturating_add(1));
                ready::<Result<i32, &str>>(Err(ARBITRARY_ERROR))
            })
            .clock(ReadyClock)
            .call(),
        );

        assert!(matches!(result, Err(RetryError::Exhausted { .. })));
        assert_eq!(attempts.get(), ARBITRARY_ATTEMPTS);
    }

    #[test]
    fn retry_async_until_retries_until_predicate_fires() {
        let attempts = Rc::new(Cell::new(0_u32));
        let attempts_ref = Rc::clone(&attempts);

        let result = block_on(
            retry_async(move |_| {
                attempts_ref.set(attempts_ref.get().saturating_add(1));
                ready(Ok::<u32, &str>(attempts_ref.get()))
            })
            .stop(stop::attempts(5))
            .until(move |v: &u32| *v >= UNTIL_TARGET)
            .clock(ReadyClock)
            .call(),
        );

        assert_eq!(result, Ok(UNTIL_TARGET));
        assert_eq!(attempts.get(), UNTIL_TARGET);
    }

    #[test]
    fn retry_async_timeout_stops_on_deadline() {
        let clock = VirtualClock::new();

        let result = block_on(
            retry_async(|_| {
                clock.advance(Duration::from_millis(CLOCK_STEP_MILLIS));
                ready(Err::<i32, &str>(ARBITRARY_ERROR))
            })
            .stop(stop::attempts(100))
            .wait(wait::fixed(WAIT_DURATION))
            .clock(&clock)
            .timeout(TIMEOUT_DEADLINE)
            .call(),
        );

        assert!(matches!(result, Err(RetryError::Exhausted { .. })));
    }

    #[test]
    fn async_retry_builder_debug_format() {
        let builder = retry_async(|_| ready(Ok::<i32, &str>(1))).clock(ReadyClock);
        let debug = format!("{builder:?}");
        // The builder Debug is opaque (finish_non_exhaustive).
        assert!(debug.contains("Retry"));
    }
}

#[test]
fn policy_and_extension_forms_are_equivalent_for_basic_case() {
    let from_ext = retry(|_| Err::<i32, &str>(ARBITRARY_ERROR))
        .clock(VirtualClock::new())
        .call();

    let from_policy = RetryPolicy::default()
        .retry(|_state: RetryState| Err::<i32, &str>(ARBITRARY_ERROR))
        .clock(VirtualClock::new())
        .call();

    assert!(matches!(
        from_ext,
        Err(RetryError::Exhausted {
            last: Err(ARBITRARY_ERROR),
            ..
        })
    ));
    assert!(matches!(
        from_policy,
        Err(RetryError::Exhausted {
            last: Err(ARBITRARY_ERROR),
            ..
        })
    ));
}

// behaves identically to the free `retry(|_| op())`, because the extension form
// only discards the `RetryState`. Both must agree on the returned value and the
// attempt count over a retry-then-succeed run under the shared defaults.
