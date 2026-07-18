//! Behavior tests for the runtime clock adapters (ADR-0005).
//!
//! Each adapter pairs a coherent `now` source with its runtime's wait, so the
//! read seam and the wait seam come from one value.

#[cfg(feature = "tokio-clock")]
mod tokio_clock {
    use core::time::Duration;
    use relentless::clock::{AsyncClock, Clock, TokioClock};

    const WAIT: Duration = Duration::from_millis(50);

    /// The headline coherence win: under `tokio::time::pause`, one value
    /// supplies both `now` and the wait, so awaited waits are visible to the
    /// read seam without wiring two matching sources.
    #[tokio::test(start_paused = true)]
    async fn paused_time_wait_advances_now_coherently() {
        let clock = TokioClock::new();
        let before = clock.now();

        clock.wait_async(WAIT).await;

        let advanced = clock.now().saturating_sub(before);
        assert!(
            advanced >= WAIT,
            "paused-time wait must advance the same clock the read seam uses \
             (advanced {advanced:?})"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn now_is_monotonically_nondecreasing() {
        let clock = TokioClock::new();
        let first = clock.now();
        let second = clock.now();
        assert!(second >= first);
    }
}

#[cfg(feature = "futures-timer-clock")]
mod futures_timer_clock {
    use core::future::Future;
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};
    use core::time::Duration;
    use relentless::clock::{AsyncClock, Clock, FuturesTimerClock};

    const WAIT: Duration = Duration::from_millis(10);

    /// Spin-polls to completion; fine for the short timer future under test.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn wait_advances_now_by_at_least_the_requested_duration() {
        let clock = FuturesTimerClock::new();
        let before = clock.now();

        block_on(clock.wait_async(WAIT));

        assert!(clock.now().saturating_sub(before) >= WAIT);
    }

    /// Regression (SPEC 15.3): building a wait for a saturated duration must
    /// not panic. `futures-timer` < 3.0.4 overflowed `Instant + Duration`
    /// inside `Delay::new` for durations near `Duration::MAX`.
    #[test]
    fn building_a_max_duration_wait_does_not_panic() {
        let clock = FuturesTimerClock::new();
        let _wait = clock.wait_async(Duration::MAX);
        // Deliberately never awaited: a ~584-year wait never completes.
    }
}

#[cfg(feature = "embassy-clock")]
mod embassy_clock {
    use relentless::clock::{AsyncClock, EmbassyClock};

    /// Type-level check only: constructing an `EmbassyClock` (and driving its
    /// timer) needs a linked embassy time driver, which host tests do not
    /// have. The capability gate is what matters here.
    #[test]
    fn implements_async_clock() {
        fn requires_async_clock<C: AsyncClock>() {}
        requires_async_clock::<EmbassyClock>();
    }
}
