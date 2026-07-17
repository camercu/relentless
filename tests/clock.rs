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
        assert!(clock.now() - before >= ARBITRARY_DURATION);
    }
}
