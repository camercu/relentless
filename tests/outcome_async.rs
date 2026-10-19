//! End-to-end `Option` and `ControlFlow` outcomes through the async driver.
//!
//! The classifier lives in the shared `step` core both drivers call, so
//! sync/async drift is structurally impossible; these mirror the sync tests
//! in `outcome.rs` to guard it rather than assume it.

mod common;

use common::{ARBITRARY_ATTEMPTS, ARBITRARY_VALUE, CONTINUE_PAYLOAD, block_on};
use core::cell::Cell;
use core::future::ready;
use core::ops::ControlFlow;
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{AsyncRetryExt, RetryError, stop, wait};
use std::rc::Rc;

// The `Option` classifier lives in the shared `step` core both drivers call,
// so sync/async drift is structurally impossible — but the CLAUDE.md lockstep
// mandate wants it guarded, not assumed. These mirror the sync `option_*`
// tests through the async driver.
#[test]
fn async_option_outcome_retries_through_none_then_returns_some() {
    let clock = VirtualClock::new();
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);

    let future = (move || {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        ready((attempts_ref.get() >= ARBITRARY_ATTEMPTS).then_some(ARBITRARY_VALUE))
    })
    .retry_async()
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(Duration::from_millis(1)))
    .clock(&clock)
    .call();

    let result = block_on(future);
    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(attempts.get(), ARBITRARY_ATTEMPTS);
}

#[test]
fn async_option_outcome_all_none_exhausts_carrying_none() {
    let clock = VirtualClock::new();
    let future = (|| ready(None::<i32>))
        .retry_async()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::from_millis(1)))
        .clock(&clock)
        .call();

    let err = block_on(future).unwrap_err();
    assert_eq!(err, RetryError::Exhausted { last: None });
    // The engine-produced Option error — not just a hand-built value — is
    // Display-able and carries the last outcome (exercises the S2 impls end
    // to end).
    assert_eq!(err.to_string(), "retries exhausted");
    assert_eq!(err.into_last(), Some(None));
}

#[test]
fn async_control_flow_outcome_retries_through_continue_then_breaks() {
    let clock = VirtualClock::new();
    let attempts = Rc::new(Cell::new(0_u32));
    let attempts_ref = Rc::clone(&attempts);

    let future = (move || {
        attempts_ref.set(attempts_ref.get().saturating_add(1));
        ready(if attempts_ref.get() >= ARBITRARY_ATTEMPTS {
            ControlFlow::Break(ARBITRARY_VALUE)
        } else {
            ControlFlow::Continue(())
        })
    })
    .retry_async()
    .stop(stop::attempts(ARBITRARY_ATTEMPTS))
    .wait(wait::fixed(Duration::from_millis(1)))
    .clock(&clock)
    .call();

    let result = block_on(future);
    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(attempts.get(), ARBITRARY_ATTEMPTS);
}

#[test]
fn async_control_flow_outcome_all_continue_exhausts_carrying_continue() {
    let clock = VirtualClock::new();
    let future = (|| ready(ControlFlow::<i32, i32>::Continue(CONTINUE_PAYLOAD)))
        .retry_async()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::from_millis(1)))
        .clock(&clock)
        .call();

    let err = block_on(future).unwrap_err();
    assert_eq!(
        err,
        RetryError::Exhausted {
            last: ControlFlow::Continue(CONTINUE_PAYLOAD)
        }
    );
    assert_eq!(err.to_string(), "retries exhausted");
    assert_eq!(
        err.into_last(),
        Some(ControlFlow::Continue(CONTINUE_PAYLOAD))
    );
}
