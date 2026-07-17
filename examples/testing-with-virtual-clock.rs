//! Testing retry logic deterministically with `clock::VirtualClock`.
//!
//! Run with: `cargo run --example testing-with-virtual-clock`
//!
//! The lesson: structure production retry code to accept its clock as a
//! parameter. Production wires the default `SystemClock` (wall time +
//! `std::thread::sleep`); tests wire a `VirtualClock`, whose waits advance
//! virtual time instead of blocking and drive elapsed time from the same
//! cell — so timeout and backoff behavior is asserted exactly, in zero real
//! milliseconds, and the wait source and elapsed source can never disagree.

use core::time::Duration;
use relentless::clock::{Clock, SyncClock, VirtualClock};
use relentless::{RetryError, Wait, retry, stop, wait};

const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const BACKOFF_CAP: Duration = Duration::from_secs(5);
const BUDGET: Duration = Duration::from_millis(400);

/// A retry-wrapped operation, parameterized over its clock so the same code
/// runs in production and under test.
///
/// One clock value supplies both elapsed time (for `.timeout()`) and the wait
/// between attempts.
fn fetch_with_retries<C: SyncClock>(
    mut operation: impl FnMut() -> Result<u32, &'static str>,
    clock: C,
) -> Result<u32, RetryError<u32, &'static str>> {
    retry(move |_| operation())
        .wait(wait::exponential(INITIAL_BACKOFF).cap(BACKOFF_CAP))
        // Bound the whole operation by wall-clock time, not attempt count.
        .stop(stop::never())
        .clock(clock)
        .timeout(BUDGET)
        .call()
}

fn main() {
    // A dependency that never recovers, so the timeout is what stops us.
    let always_failing = || Err::<u32, _>("service unavailable");

    // Under test: inject a VirtualClock by reference. No real time passes,
    // and the handle stays available for assertions afterwards.
    let clock = VirtualClock::new();
    let result = fetch_with_retries(always_failing, &clock);

    // The timeout fired rather than an attempt limit...
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));

    // ...and we can assert the exact backoff schedule the policy produced.
    // Exponential 100ms → 200ms, then the third wait (400ms) is clamped to the
    // 100ms of budget remaining before the 400ms deadline.
    assert_eq!(
        clock.waits(),
        vec![
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(100),
        ],
    );

    // Virtual time advanced to exactly the budget; the wall clock did not move.
    assert_eq!(clock.now(), BUDGET);

    println!("retry gave up after backoff schedule {:?}", clock.waits());
    println!("virtual time elapsed: {:?} (real time: ~0ms)", clock.now());

    // In production you would call the same function with the real clock:
    //
    //     fetch_with_retries(real_operation, relentless::clock::SystemClock);
}
