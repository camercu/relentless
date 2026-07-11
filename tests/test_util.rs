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

/// A nonzero duration whose exact value is irrelevant to the test.
const ARBITRARY_DURATION: Duration = Duration::from_secs(1);

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

/// GIVEN a clock advanced to `Duration::MAX` by `advance` and by a recorded sleep
/// WHEN more time is added past the maximum
/// THEN virtual time saturates at `Duration::MAX` rather than overflowing,
///      while sleeps are still recorded verbatim (SPEC 12.4.4)
#[test]
fn time_arithmetic_saturates_at_max() {
    let advanced = VirtualClock::new();
    advanced.advance(Duration::MAX);
    advanced.advance(ARBITRARY_DURATION);
    assert_eq!(advanced.now(), Duration::MAX);

    let slept = VirtualClock::new();
    let mut sleep = slept.sync_sleep();
    sleep(Duration::MAX);
    sleep(ARBITRARY_DURATION);
    assert_eq!(slept.now(), Duration::MAX);
    assert_eq!(slept.sleeps(), vec![Duration::MAX, ARBITRARY_DURATION]);
}

/// GIVEN a `VirtualClock` shared across threads via `Clone`
/// WHEN many threads record sleeps through their own adapters concurrently
/// THEN every sleep is recorded and virtual time equals their sum, with no
///      lost updates (SPEC 12.4.6: adapters are `Send + Sync`)
#[test]
fn adapters_are_usable_across_threads() {
    const THREADS: u64 = 8;
    const SLEEPS_PER_THREAD: u64 = 1000;
    const SLEEP: Duration = Duration::from_nanos(1);

    let clock = VirtualClock::new();
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let mut sleep = clock.sync_sleep();
            std::thread::spawn(move || {
                for _ in 0..SLEEPS_PER_THREAD {
                    sleep(SLEEP);
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let total = THREADS * SLEEPS_PER_THREAD;
    assert_eq!(clock.sleeps().len() as u64, total);
    assert_eq!(clock.now(), SLEEP * u32::try_from(total).unwrap());
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
