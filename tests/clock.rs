//! Behavior tests for the unified clock abstraction (ADR-0005).
//!
//! One injected value owns both the read seam (`Clock::now`) and the wait seam
//! (`SyncClock::wait` / `AsyncClock::wait_async`), so elapsed time and waits
//! can never desync.

use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;

use relentless::clock::{AsyncClock, Clock, SyncClock, VirtualClock};

const ARBITRARY_DURATION: Duration = Duration::from_millis(10);

/// Polls `future` once with a no-op waker.
fn poll_once<F: Future>(future: &mut core::pin::Pin<&mut F>) -> Poll<F::Output> {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    future.as_mut().poll(&mut cx)
}

#[test]
fn virtual_clock_starts_at_time_zero() {
    let clock = VirtualClock::new();
    assert_eq!(clock.now(), Duration::ZERO);
}

#[test]
fn sync_wait_advances_now_by_exactly_the_requested_duration() {
    let clock = VirtualClock::new();
    clock.wait(ARBITRARY_DURATION);
    assert_eq!(clock.now(), ARBITRARY_DURATION);
    clock.wait(ARBITRARY_DURATION);
    assert_eq!(clock.now(), ARBITRARY_DURATION * 2);
}

#[test]
fn advance_moves_now_without_recording_a_wait() {
    let clock = VirtualClock::new();
    clock.advance(ARBITRARY_DURATION);
    assert_eq!(clock.now(), ARBITRARY_DURATION);
    #[cfg(feature = "alloc")]
    assert_eq!(clock.waits(), Vec::new());
}

#[cfg(feature = "alloc")]
#[test]
fn waits_records_each_wait_in_request_order() {
    let clock = VirtualClock::new();
    clock.wait(Duration::from_millis(1));
    clock.wait(Duration::from_millis(2));
    assert_eq!(
        clock.waits(),
        vec![Duration::from_millis(1), Duration::from_millis(2)]
    );
}

#[test]
fn async_wait_advances_on_first_poll_not_at_creation() {
    let clock = VirtualClock::new();
    let future = (&clock).wait_async(ARBITRARY_DURATION);
    // Building the future must not advance time: the engine may race or cancel
    // the wait before ever polling it (ADR-0005 open item: poll-time advance).
    assert_eq!(clock.now(), Duration::ZERO);

    let mut future = pin!(future);
    assert_eq!(poll_once(&mut future), Poll::Ready(()));
    assert_eq!(clock.now(), ARBITRARY_DURATION);
}

#[test]
fn async_wait_advances_exactly_once_across_polls() {
    let clock = VirtualClock::new();
    let mut future = pin!((&clock).wait_async(ARBITRARY_DURATION));
    assert_eq!(poll_once(&mut future), Poll::Ready(()));
    assert_eq!(poll_once(&mut future), Poll::Ready(()));
    assert_eq!(clock.now(), ARBITRARY_DURATION);
}

#[test]
fn dropping_an_unpolled_async_wait_does_not_advance_time() {
    let clock = VirtualClock::new();
    drop((&clock).wait_async(ARBITRARY_DURATION));
    assert_eq!(clock.now(), Duration::ZERO);
    #[cfg(feature = "alloc")]
    assert_eq!(clock.waits(), Vec::new());
}

#[test]
fn sync_and_async_waits_flow_through_the_same_now() {
    let clock = VirtualClock::new();
    clock.wait(ARBITRARY_DURATION);
    let mut future = pin!((&clock).wait_async(ARBITRARY_DURATION));
    assert_eq!(poll_once(&mut future), Poll::Ready(()));
    assert_eq!(clock.now(), ARBITRARY_DURATION * 2);
    #[cfg(feature = "alloc")]
    assert_eq!(clock.waits(), vec![ARBITRARY_DURATION, ARBITRARY_DURATION]);
}

#[test]
fn virtual_time_saturates_at_duration_max() {
    let clock = VirtualClock::new();
    clock.advance(Duration::MAX);
    clock.wait(ARBITRARY_DURATION);
    assert_eq!(clock.now(), Duration::MAX);
}

#[test]
fn a_shared_reference_is_itself_a_sync_clock() {
    // `&VirtualClock` satisfies the same bounds as an owned clock, so a test
    // can hand the engine a borrow and keep the handle for assertions.
    fn takes_sync_clock<C: SyncClock>(clock: C) -> Duration {
        clock.wait(ARBITRARY_DURATION);
        clock.now()
    }
    let clock = VirtualClock::new();
    assert_eq!(takes_sync_clock(&clock), ARBITRARY_DURATION);
    assert_eq!(clock.now(), ARBITRARY_DURATION);
}

#[cfg(feature = "std")]
mod system_clock {
    use super::*;
    use relentless::clock::SystemClock;

    #[test]
    fn now_is_monotonically_nondecreasing() {
        let clock = SystemClock;
        let first = clock.now();
        let second = clock.now();
        assert!(second >= first);
    }

    #[test]
    fn wait_blocks_for_at_least_the_requested_duration() {
        // `thread::sleep` guarantees an at-least bound, so this cannot flake.
        let clock = SystemClock;
        let before = clock.now();
        clock.wait(ARBITRARY_DURATION);
        assert!(clock.now().saturating_sub(before) >= ARBITRARY_DURATION);
    }
}

/// Engine-level acceptance: the retry engines driven end-to-end by a
/// `VirtualClock`, exactly as a consumer testing their own retry policies
/// would — deterministic backoff assertions with no real sleeping.
#[cfg(feature = "alloc")]
mod engine_integration {
    use super::*;
    use relentless::{retry, retry_async, stop, wait};
    use std::vec;

    const INITIAL_BACKOFF: Duration = Duration::from_millis(100);

    /// Polls a future to completion (every wait here is immediately ready).
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = core::pin::pin!(future);
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
        }
    }

    /// GIVEN exponential backoff from 100ms and a 3-attempt budget
    /// WHEN an always-failing operation runs against a `VirtualClock`
    /// THEN the recorded waits are exactly the backoff schedule (1→2, 2→3)
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
    /// WHEN a retried operation waits through it
    /// THEN virtual time advances by the sum of the waits
    #[test]
    fn waiting_advances_virtual_time() {
        let clock = VirtualClock::new();

        let _ = retry(|_| Err::<(), &str>("boom"))
            .wait(wait::exponential(INITIAL_BACKOFF))
            .stop(stop::attempts(3))
            .clock(&clock)
            .call();

        assert_eq!(clock.now(), Duration::from_millis(300));
    }

    /// GIVEN a 250ms timeout budget and a fixed 100ms wait, both on one clock
    /// WHEN the operation never succeeds
    /// THEN the run terminates deterministically with the final wait clamped
    ///      to the remaining budget
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
    /// WHEN it runs against a `VirtualClock`
    /// THEN no waits are recorded and virtual time stays at zero
    #[test]
    fn first_attempt_success_records_nothing() {
        let clock = VirtualClock::new();

        let result = retry(|_| Ok::<_, &str>(42)).clock(&clock).call();

        assert_eq!(result.unwrap(), 42);
        assert_eq!((clock.waits(), clock.now()), (vec![], Duration::ZERO));
    }

    /// GIVEN an operation that advances the clock manually (simulating slow
    ///       attempts)
    /// WHEN the elapsed budget is exceeded inside the operation with no waits
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

    /// GIVEN the async engine waiting through the same virtual clock type
    /// WHEN an always-failing operation runs
    /// THEN the recorded waits match the sync engine's schedule
    #[test]
    fn async_engine_records_backoff_schedule() {
        let clock = VirtualClock::new();

        let result = block_on(
            retry_async(|_| core::future::ready(Err::<(), &str>("boom")))
                .wait(wait::exponential(INITIAL_BACKOFF))
                .stop(stop::attempts(3))
                .clock(&clock)
                .call(),
        );

        assert!(result.is_err());
        assert_eq!(
            clock.waits(),
            vec![Duration::from_millis(100), Duration::from_millis(200)]
        );
    }
}
