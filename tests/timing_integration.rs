//! Focused wall-clock timing integration tests.
#![cfg(feature = "std")]

use core::cell::Cell;
use core::time::Duration;
use std::cell::RefCell;

use tenacious::stop;
use tenacious::{RetryError, RetryPolicy};

const OPERATION_RUNTIME: Duration = Duration::from_millis(5);
const ELAPSED_DEADLINE: Duration = Duration::from_millis(1);

#[test]
fn elapsed_stop_counts_operation_runtime_with_real_sleep() {
    let mut policy = RetryPolicy::new().stop(stop::elapsed(ELAPSED_DEADLINE));
    let sleeps: RefCell<Vec<Duration>> = RefCell::new(Vec::new());
    let call_count = Cell::new(0_u32);

    let result = policy
        .retry(|| {
            call_count.set(call_count.get().saturating_add(1));
            std::thread::sleep(OPERATION_RUNTIME);
            Err::<i32, _>("slow failure")
        })
        .sleep(|dur| sleeps.borrow_mut().push(dur))
        .call();

    assert_eq!(call_count.get(), 1);
    assert!(sleeps.borrow().is_empty());
    assert!(matches!(
        result,
        Err(RetryError::Exhausted { attempts: 1, .. })
    ));
}
