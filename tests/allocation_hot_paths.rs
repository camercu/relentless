//! Allocation profile checks for hot retry execution paths.

use core::time::Duration;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tenacious::{RetryPolicy, stop, wait};

#[global_allocator]
static GLOBAL: &StatsAlloc<std::alloc::System> = &INSTRUMENTED_SYSTEM;

const MAX_ATTEMPTS: u32 = 3;
const ERROR_VALUE: &str = "fail";
const SUCCESS_VALUE: i32 = 7;
const ALLOCATION_SAMPLE_RUNS: u32 = 16;

fn allocation_test_guard() -> MutexGuard<'static, ()> {
    static ALLOC_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ALLOC_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn instant_sleep(_dur: Duration) {}

fn min_allocated_during(mut operation: impl FnMut()) -> (usize, usize) {
    let mut min_allocations = usize::MAX;
    let mut min_bytes = usize::MAX;

    for _ in 0..ALLOCATION_SAMPLE_RUNS {
        let region = Region::new(GLOBAL);
        operation();
        let change = region.change();
        min_allocations = min_allocations.min(change.allocations);
        min_bytes = min_bytes.min(change.bytes_allocated);
    }

    (min_allocations, min_bytes)
}

#[test]
fn concrete_sync_retry_execution_is_allocation_free() {
    let _guard = allocation_test_guard();
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    // Warm up one run to avoid one-time initialization noise.
    let _ = policy
        .retry(|| Ok::<i32, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();

    let (min_allocations, min_bytes) = min_allocated_during(|| {
        let _ = policy
            .retry(|| Err::<i32, &str>(ERROR_VALUE))
            .sleep(instant_sleep)
            .call();
    });

    assert_eq!(min_allocations, 0, "concrete execution should not allocate");
    assert_eq!(min_bytes, 0, "concrete execution should not allocate bytes");
}

#[cfg(feature = "alloc")]
#[test]
fn boxed_policy_construction_performs_heap_allocation() {
    let _guard = allocation_test_guard();
    let (min_allocations, min_bytes) = min_allocated_during(|| {
        let _policy = RetryPolicy::new()
            .stop(stop::attempts(MAX_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .boxed::<i32, &str>();
    });

    assert!(min_allocations > 0, "boxed construction should allocate");
    assert!(min_bytes > 0, "boxed construction should allocate bytes");
}

#[cfg(feature = "alloc")]
#[test]
fn boxed_sync_retry_execution_is_allocation_free_after_warmup() {
    let _guard = allocation_test_guard();
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed::<i32, &str>();

    let _ = policy
        .retry(|| Err::<i32, &str>(ERROR_VALUE))
        .sleep(instant_sleep)
        .call();

    let (min_allocations, min_bytes) = min_allocated_during(|| {
        let _ = policy
            .retry(|| Err::<i32, &str>(ERROR_VALUE))
            .sleep(instant_sleep)
            .call();
    });

    assert_eq!(
        min_allocations, 0,
        "boxed execution should not allocate after warmup"
    );
    assert_eq!(
        min_bytes, 0,
        "boxed execution should not allocate bytes after warmup"
    );
}
