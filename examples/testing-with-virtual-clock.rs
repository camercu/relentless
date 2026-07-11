//! Testing retry logic deterministically with `test_util::VirtualClock`.
//!
//! Run with: `cargo run --example testing-with-virtual-clock --features test-util`
//!
//! The lesson: structure production retry code to accept its sleeper and
//! elapsed clock as parameters. Production wires real ones
//! (`std::thread::sleep`, an `Instant`-based clock); tests wire a
//! `VirtualClock`, which records the backoff schedule and advances virtual
//! time instead of blocking — so timeout and backoff behavior is asserted
//! exactly, in zero real milliseconds.

use core::time::Duration;
use relentless::test_util::VirtualClock;
use relentless::{RetryError, Wait, retry, stop, wait};

const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const BACKOFF_CAP: Duration = Duration::from_secs(5);
const BUDGET: Duration = Duration::from_millis(400);

/// A retry-wrapped operation, parameterized over its sleeper and elapsed clock
/// so the same code runs in production and under test.
///
/// `sleep` is any sync sleeper; `clock` reports elapsed time for `.timeout()`.
fn fetch_with_retries<S, C>(
    mut operation: impl FnMut() -> Result<u32, &'static str>,
    sleep: S,
    clock: C,
) -> Result<u32, RetryError<u32, &'static str>>
where
    S: FnMut(Duration),
    C: Fn() -> Duration + 'static,
{
    retry(move |_| operation())
        .wait(wait::exponential(INITIAL_BACKOFF).cap(BACKOFF_CAP))
        // Bound the whole operation by wall-clock time, not attempt count.
        .stop(stop::never())
        .elapsed_clock_fn(clock)
        .timeout(BUDGET)
        .sleep(sleep)
        .call()
}

fn main() {
    // A dependency that never recovers, so the timeout is what stops us.
    let always_failing = || Err::<u32, _>("service unavailable");

    // Under test: wire the operation to a VirtualClock. No real time passes.
    let clock = VirtualClock::new();
    let result = fetch_with_retries(always_failing, clock.sync_sleep(), clock.clock());

    // The timeout fired rather than an attempt limit...
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));

    // ...and we can assert the exact backoff schedule the policy produced.
    // Exponential 100ms → 200ms, then the third wait (400ms) is clamped to the
    // 100ms of budget remaining before the 400ms deadline.
    assert_eq!(
        clock.sleeps(),
        vec![
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(100),
        ],
    );

    // Virtual time advanced to exactly the budget; the wall clock did not move.
    assert_eq!(clock.now(), BUDGET);

    println!("retry gave up after backoff schedule {:?}", clock.sleeps());
    println!("virtual time elapsed: {:?} (real time: ~0ms)", clock.now());

    // In production you would call the same function with real infrastructure:
    //
    //     fetch_with_retries(
    //         real_operation,
    //         |dur| std::thread::sleep(dur),
    //         {
    //             let start = std::time::Instant::now();
    //             move || start.elapsed()
    //         },
    //     );
}
