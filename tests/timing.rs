//! Integration tests that use a real wall-clock to verify `stop::elapsed` behavior.
//!
//! These tests run the actual `std::thread::sleep` to simulate operation latency.
//! They are intentionally narrow: only elapsed-stop is verified here because all other
//! stop/wait/predicate correctness is covered by deterministic tests in `composition.rs`
//! and `policy_sync.rs`.
#![cfg(feature = "std")]

use core::cell::Cell;
use core::time::Duration;

use relentless::stop;
use relentless::{RetryError, RetryPolicy};

const OPERATION_RUNTIME: Duration = Duration::from_millis(5);
const ELAPSED_DEADLINE: Duration = Duration::from_millis(1);

#[test]
fn elapsed_stop_counts_operation_runtime_with_real_sleep() {
    let policy = RetryPolicy::new().stop(stop::elapsed(ELAPSED_DEADLINE));
    let call_count = Cell::new(0_u32);

    // Uses the default `SystemClock`: the 5 ms operation runtime alone
    // exhausts the 1 ms elapsed deadline, so the loop stops after one attempt
    // (and therefore never waits between attempts).
    let result = policy
        .retry(|_| {
            call_count.set(call_count.get().saturating_add(1));
            std::thread::sleep(OPERATION_RUNTIME);
            Err::<i32, _>("slow failure")
        })
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(matches!(result, Err(RetryError::Exhausted { .. })));
}
