//! Acceptance tests for the async builder (`retry_async(..)` and `AsyncRetryExt`).
//!
//! Mirrors the sync builder scenarios in `builder.rs`, driven under a virtual
//! clock and a no-op waker so no executor is needed.

mod common;

use common::{ARBITRARY_ATTEMPTS, ARBITRARY_VALUE, ReadyClock, block_on};
use core::cell::Cell;
use core::time::Duration;
use std::rc::Rc;

use relentless::clock::VirtualClock;
use relentless::{
    AsyncRetryExt, Decision, Exit, RetryError, StopReason, Verdict, predicate, retry_async, stop,
    wait,
};

type IntResult = Result<i32, &'static str>;

#[test]
fn async_retry_ext_starts_from_a_no_arg_closure() {
    let counter = Cell::new(0);
    let clock = VirtualClock::new();
    let result = block_on(
        (|| {
            let n = counter.get() + 1;
            counter.set(n);
            async move { if n >= 2 { Ok::<i32, &str>(5) } else { Err("x") } }
        })
        .retry_async()
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(&clock)
        .call(),
    );

    assert_eq!(result, Ok(5));
    assert_eq!(counter.get(), 2);
}

#[test]
fn async_default_path_retries_then_returns_ok() {
    let counter = Cell::new(0);
    let clock = VirtualClock::new();
    let result = block_on(
        retry_async(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            async move { if n >= 2 { Ok::<i32, &str>(7) } else { Err("x") } }
        })
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(&clock)
        .call(),
    );

    assert_eq!(result, Ok(7));
    assert_eq!(counter.get(), 2);
}

#[test]
fn async_decide_infers_through_fut_output() {
    // Inline classifier with no annotations: the parameter is typed through
    // `Fut::Output`.
    let clock = VirtualClock::new();
    let result = block_on(
        retry_async(|_| async { Err::<i32, &str>("fatal") })
            .decide(|o| match o {
                Ok(v) => Verdict::Return(v),
                Err("fatal") => Verdict::Abort("boom"),
                Err(_) => Verdict::Retry(o),
            })
            .stop(stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .clock(&clock)
            .call(),
    );

    assert_eq!(result, Err(RetryError::Aborted { last: "boom" }));
}

#[test]
fn async_decide_returns_sought_error_through_ok() {
    let counter = Cell::new(0);
    let clock = VirtualClock::new();
    let result = block_on(
        retry_async(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            async move { if n >= 3 { Err("crash") } else { Ok(()) } }
        })
        .decide(|o| match o {
            Err(e) => Decision::Return(e),
            Ok(()) => Decision::Retry(o),
        })
        .stop(stop::attempts(10))
        .wait(wait::fixed(Duration::ZERO))
        .clock(&clock)
        .call(),
    );

    assert_eq!(result, Ok("crash"));
    assert_eq!(counter.get(), 3);
}

#[test]
fn async_until_polls_a_result() {
    let counter = Cell::new(0);
    let clock = VirtualClock::new();
    let result = block_on(
        retry_async(|_| {
            let n = counter.get() + 1;
            counter.set(n);
            async move { Ok::<i32, &str>(n) }
        })
        .until(predicate::ok(|v: &i32| *v >= 3))
        .stop(stop::attempts(ARBITRARY_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .clock(&clock)
        .call(),
    );

    assert_eq!(result, Ok(3));
    assert_eq!(counter.get(), 3);
}

#[test]
fn async_with_stats_and_on_exit_match_the_sync_engine() {
    let reason = Cell::new(None);
    let clock = VirtualClock::new();
    let (result, stats) = block_on(
        retry_async(|_| async { Err::<i32, &str>("boom") })
            .on_exit(|e: &Exit<i32, &str, IntResult>| reason.set(Some(e.stop_reason())))
            .stop(stop::attempts(3))
            .wait(wait::fixed(Duration::from_millis(5)))
            .clock(&clock)
            .with_stats()
            .call(),
    );

    assert_eq!(result, Err(RetryError::Exhausted { last: Err("boom") }));
    assert_eq!(stats.attempts, 3);
    assert_eq!(stats.total_wait, Duration::from_millis(10));
    assert_eq!(reason.get(), Some(StopReason::Exhausted));
}

/// Returns `Pending` on its first poll (waking the task) and `Ready` after.
struct YieldOnce {
    yielded: bool,
}

impl core::future::Future for YieldOnce {
    type Output = ();

    fn poll(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        if self.yielded {
            return core::task::Poll::Ready(());
        }
        self.yielded = true;
        cx.waker().wake_by_ref();
        core::task::Poll::Pending
    }
}

/// GIVEN an operation whose future is pending before it resolves
/// WHEN the retry future is driven to completion
/// THEN the same future is polled again: the operation runs once and its
/// value is returned after one attempt.
#[test]
fn a_pending_operation_future_is_repolled_not_restarted() {
    let calls = Cell::new(0);
    let clock = VirtualClock::new();
    let (result, stats) = block_on(
        retry_async(|_| {
            calls.set(calls.get() + 1);
            // Only the first call's future yields, so a driver that restarted
            // the operation on `Pending` would finish with two calls, not hang.
            let first_call = calls.get() == 1;
            async move {
                if first_call {
                    YieldOnce { yielded: false }.await;
                }
                Ok::<i32, &str>(ARBITRARY_VALUE)
            }
        })
        .clock(&clock)
        .with_stats()
        .call(),
    );

    assert_eq!(
        (result, stats.attempts, calls.get()),
        (Ok(ARBITRARY_VALUE), 1, 1)
    );
}

/// §10.1 — the retry future never holds an outcome across a wait, so a `!Send`
/// outcome still yields a `Send` future when the operation's future is `Send`.
#[test]
fn a_non_send_outcome_still_gives_a_send_retry_future() {
    fn assert_send<T: Send>(_: &T) {}

    let future = retry_async(|_| async { Ok::<Rc<i32>, &str>(Rc::new(ARBITRARY_VALUE)) })
        .clock(ReadyClock)
        .call();
    assert_send(&future);
    assert_eq!(block_on(future).as_deref(), Ok(&ARBITRARY_VALUE));
}
