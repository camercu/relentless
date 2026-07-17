//! The timeout-without-clock diagnostic (SPEC 11.2) fires only in `no_std`
//! builds, where `ElapsedTracker` has no fallback clock and a timeout set
//! without `.elapsed_clock(...)` would silently never fire. Since the sync
//! engine's clock became mandatory (ADR-0005), only the *async* engine can
//! still reach this state; the sync equivalent is a compile error.
//!
//! This is the one configuration where `debug_assert_timeout_has_clock` can
//! trip: under `std` the fallback `Instant` clock makes the asserted condition
//! unconditionally true, so the assertion is unreachable there (and therefore
//! mutation-untestable — see the `exclude_re` note in `.cargo/mutants.toml`).
//! Gated to `no_std` + debug so it neither compiles under the default `std`
//! build nor runs in release, where the assertion compiles out.
#![cfg(all(not(feature = "std"), debug_assertions))]

use core::future::{Future, ready};
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;

use relentless::RetryPolicy;

const ARBITRARY_TIMEOUT: Duration = Duration::from_millis(10);

/// Polls a future to completion with a no-op waker (every future in this test
/// is immediately ready).
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
    }
}

#[test]
#[should_panic(expected = "timeout configured without an elapsed clock")]
fn timeout_without_clock_panics_in_debug() {
    // The assertion fires at loop entry, before the first attempt, so the
    // operation's outcome is irrelevant — reaching the first poll is enough.
    let _ = block_on(
        RetryPolicy::new()
            .retry_async(|_| ready(Ok::<i32, &str>(0)))
            .sleep(|_dur| ready(()))
            .timeout(ARBITRARY_TIMEOUT)
            .call(),
    );
}
