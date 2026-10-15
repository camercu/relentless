//! Acceptance tests for the async builder (`retry_async(..)` and `AsyncRetryExt`).
//!
//! Mirrors the sync builder scenarios in `builder.rs`, driven under a virtual
//! clock and a no-op waker so no executor is needed.

use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;

use relentless::clock::VirtualClock;
use relentless::{
    AsyncRetryExt, Decision, Exit, RetryError, StopReason, Verdict, predicate, retry_async, stop,
    wait,
};

/// Polls a future to completion under a no-op waker. The virtual clock's
/// async waits resolve on poll, so no real executor is needed.
fn block_on<Fut: Future>(future: Fut) -> Fut::Output {
    let mut future = core::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = Future::poll(Pin::as_mut(&mut future), &mut cx) {
            return output;
        }
    }
}

type IntResult = Result<i32, &'static str>;
const ARBITRARY_ATTEMPTS: u32 = 5;

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
