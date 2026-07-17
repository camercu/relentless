//! Acceptance tests for the `test-util` virtual-clock infrastructure.
//!
//! These exercise `VirtualClock` exactly as a consumer testing their own retry
//! policies would: deterministic backoff assertions with no real sleeping.
//! Sync execution injects the core [`relentless::clock::VirtualClock`] via
//! `.clock(...)`; the `test_util` clock adapts the remaining async sleep seam.

#![cfg(feature = "test-util")]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use std::sync::Arc;

use relentless::clock::{Clock as _, SyncClock as _, VirtualClock};
use relentless::test_util::VirtualClock as AsyncVirtualClock;
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
        .clock(&clock)
        .call();

    assert!(result.is_err());
    assert_eq!(
        clock.waits(),
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
        .clock(&clock)
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
        .clock(&clock)
        .timeout(Duration::from_millis(250))
        .call();

    assert!(result.is_err());
    assert_eq!(
        clock.waits(),
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

    let result = retry(|_| Ok::<_, &str>(42)).clock(&clock).call();

    assert_eq!(result.unwrap(), 42);
    assert_eq!((clock.waits(), clock.now()), (vec![], Duration::ZERO));
}

/// GIVEN an operation that advances the clock manually (simulating slow attempts)
/// WHEN the elapsed budget is exceeded inside the operation with no sleeps at all
/// THEN the stop strategy fires on virtual elapsed time
#[test]
fn manual_advance_consumes_elapsed_budget() {
    let clock = VirtualClock::new();

    let result = retry(|_| {
        clock.advance(Duration::from_millis(100));
        Err::<(), &str>("boom")
    })
    .wait(wait::fixed(Duration::ZERO))
    .stop(stop::elapsed(Duration::from_millis(250)))
    .clock(&clock)
    .call();

    assert!(result.is_err());
    assert_eq!(clock.now(), Duration::from_millis(300));
}

/// GIVEN a clock advanced to `Duration::MAX` by `advance` and by a recorded wait
/// WHEN more time is added past the maximum
/// THEN virtual time saturates at `Duration::MAX` rather than overflowing,
///      while waits are still recorded verbatim (SPEC 12.4.4)
#[test]
fn time_arithmetic_saturates_at_max() {
    let advanced = VirtualClock::new();
    advanced.advance(Duration::MAX);
    advanced.advance(ARBITRARY_DURATION);
    assert_eq!(advanced.now(), Duration::MAX);

    let waited = VirtualClock::new();
    waited.wait(Duration::MAX);
    waited.wait(ARBITRARY_DURATION);
    assert_eq!(waited.now(), Duration::MAX);
    assert_eq!(waited.waits(), vec![Duration::MAX, ARBITRARY_DURATION]);
}

/// GIVEN a `test_util::VirtualClock` shared across threads via `Clone`
/// WHEN many threads record sleeps through their own adapters concurrently
/// THEN every sleep is recorded and virtual time equals their sum, with no
///      lost updates (SPEC 12.4.6: adapters are `Send + Sync`)
#[test]
fn adapters_are_usable_across_threads() {
    const THREADS: u64 = 8;
    const SLEEPS_PER_THREAD: u64 = 1000;
    const SLEEP: Duration = Duration::from_nanos(1);

    let clock = AsyncVirtualClock::new();
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let sleep = clock.async_sleep();
            std::thread::spawn(move || {
                for _ in 0..SLEEPS_PER_THREAD {
                    let _ready = sleep(SLEEP);
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

/// GIVEN an elapsed clock and a sleeper sourced from two *different*
///       `test_util::VirtualClock` instances (the documented misuse on the
///       async seam: see `clock()`)
/// WHEN a retried async operation sleeps through the second clock's sleeper
/// THEN the elapsed clock stays at zero — only the sleeper's own clock
///      advances — so an elapsed-based stop or timeout would never fire and
///      the loop could not terminate without the independent attempt bound
///      (SPEC 12.4.3; the sync seam no longer permits this by construction)
#[test]
fn mismatched_clock_and_sleeper_leaves_elapsed_stuck() {
    let elapsed = AsyncVirtualClock::new();
    let sleeper = AsyncVirtualClock::new();

    // A stop that depends on `elapsed` would spin forever here; the attempt
    // bound is what actually terminates the loop, proving the hazard.
    let result = block_on(
        retry_async(|_| core::future::ready(Err::<(), &str>("boom")))
            .wait(wait::fixed(INITIAL_BACKOFF))
            .stop(stop::attempts(3))
            .elapsed_clock_fn(elapsed.clock())
            .sleep(sleeper.async_sleep())
            .call(),
    );

    assert!(result.is_err());
    assert_eq!(
        elapsed.now(),
        Duration::ZERO,
        "elapsed clock must not advance"
    );
    assert_eq!(
        sleeper.sleeps(),
        vec![INITIAL_BACKOFF, INITIAL_BACKOFF],
        "only the sleeper's own clock records and advances",
    );
}

/// GIVEN the async engine sleeping through the virtual clock
/// WHEN an always-failing operation runs
/// THEN the recorded sleeps match the sync engine's schedule
#[test]
fn async_sleep_records_backoff_schedule() {
    let clock = AsyncVirtualClock::new();

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
