//! Acceptance tests for the `test-util` virtual-clock infrastructure.
//!
//! These exercise `VirtualClock` exactly as a consumer testing their own retry
//! policies would: deterministic backoff assertions with no real sleeping.

#![cfg(feature = "test-util")]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use std::sync::Arc;

use relentless::test_util::VirtualClock;
use relentless::{retry, retry_async, stop, wait};

const INITIAL_BACKOFF: Duration = Duration::from_millis(100);

fn noop_waker() -> Waker {
    struct NoopWake;
    impl std::task::Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    Waker::from(Arc::new(NoopWake))
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    loop {
        match Future::poll(Pin::as_mut(&mut future), &mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// GIVEN exponential backoff from 100ms and a 3-attempt budget
/// WHEN an always-failing operation runs with a `VirtualClock` sleeper
/// THEN the recorded sleeps are exactly the backoff schedule (attempts 1→2, 2→3)
#[test]
fn records_exponential_backoff_schedule() {
    let clock = VirtualClock::new();

    let result = retry(|_| Err::<(), &str>("boom"))
        .wait(wait::exponential(INITIAL_BACKOFF))
        .stop(stop::attempts(3))
        .sleep(clock.sync_sleep())
        .call();

    assert!(result.is_err());
    assert_eq!(
        clock.sleeps(),
        vec![Duration::from_millis(100), Duration::from_millis(200)]
    );
}

/// GIVEN a fresh `VirtualClock`
/// WHEN a retried operation sleeps through it
/// THEN virtual time advances by the sum of the sleeps
#[test]
fn sleeping_advances_virtual_time() {
    let clock = VirtualClock::new();

    let _ = retry(|_| Err::<(), &str>("boom"))
        .wait(wait::exponential(INITIAL_BACKOFF))
        .stop(stop::attempts(3))
        .sleep(clock.sync_sleep())
        .call();

    assert_eq!(clock.now(), Duration::from_millis(300));
}

/// GIVEN a 250ms timeout budget measured by the virtual clock and a fixed 100ms wait
/// WHEN the operation never succeeds
/// THEN the run terminates deterministically with the final sleep clamped to the remaining budget
#[test]
fn timeout_runs_deterministically_on_virtual_time() {
    let clock = VirtualClock::new();

    let result = retry(|_| Err::<(), &str>("boom"))
        .wait(wait::fixed(Duration::from_millis(100)))
        .stop(stop::never())
        .elapsed_clock_fn(clock.clock())
        .timeout(Duration::from_millis(250))
        .sleep(clock.sync_sleep())
        .call();

    assert!(result.is_err());
    assert_eq!(
        clock.sleeps(),
        vec![
            Duration::from_millis(100),
            Duration::from_millis(100),
            Duration::from_millis(50)
        ]
    );
}

/// GIVEN an operation that succeeds on the first attempt
/// WHEN it runs with a `VirtualClock` sleeper
/// THEN no sleeps are recorded and virtual time stays at zero
#[test]
fn first_attempt_success_records_nothing() {
    let clock = VirtualClock::new();

    let result = retry(|_| Ok::<_, &str>(42))
        .sleep(clock.sync_sleep())
        .call();

    assert_eq!(result.unwrap(), 42);
    assert_eq!((clock.sleeps(), clock.now()), (vec![], Duration::ZERO));
}

/// GIVEN an operation that advances the clock manually (simulating slow attempts)
/// WHEN the elapsed budget is exceeded inside the operation with no sleeps at all
/// THEN the stop strategy fires on virtual elapsed time
#[test]
fn manual_advance_consumes_elapsed_budget() {
    let clock = VirtualClock::new();
    let advancing = clock.clone();

    let result = retry(move |_| {
        advancing.advance(Duration::from_millis(100));
        Err::<(), &str>("boom")
    })
    .wait(wait::fixed(Duration::ZERO))
    .stop(stop::elapsed(Duration::from_millis(250)))
    .elapsed_clock_fn(clock.clock())
    .sleep(clock.sync_sleep())
    .call();

    assert!(result.is_err());
    assert_eq!(clock.now(), Duration::from_millis(300));
}

/// GIVEN the async engine sleeping through the virtual clock
/// WHEN an always-failing operation runs
/// THEN the recorded sleeps match the sync engine's schedule
#[test]
fn async_sleep_records_backoff_schedule() {
    let clock = VirtualClock::new();

    let result = block_on(
        retry_async(|_| core::future::ready(Err::<(), &str>("boom")))
            .wait(wait::exponential(INITIAL_BACKOFF))
            .stop(stop::attempts(3))
            .sleep(clock.async_sleep())
            .call(),
    );

    assert!(result.is_err());
    assert_eq!(
        clock.sleeps(),
        vec![Duration::from_millis(100), Duration::from_millis(200)]
    );
}
