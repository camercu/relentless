//! Allocation profile checks for retry execution hot paths.

use core::time::Duration;
#[cfg(feature = "alloc")]
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::sync::{Mutex, MutexGuard, OnceLock};
#[cfg(feature = "alloc")]
use std::{cell::Cell, pin::pin};
#[cfg(feature = "alloc")]
use tenacious::sleep::Sleeper;
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

#[cfg(feature = "alloc")]
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::noop().clone();
    let mut cx = Context::from_waker(&waker);
    loop {
        match Future::poll(Pin::as_mut(&mut future), &mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[cfg(feature = "alloc")]
#[derive(Clone, Copy)]
struct InstantSleeper;

#[cfg(feature = "alloc")]
impl Sleeper for InstantSleeper {
    type Sleep = core::future::Ready<()>;

    fn sleep(&self, _dur: Duration) -> Self::Sleep {
        core::future::ready(())
    }
}

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
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    // Warm up one run to avoid one-time initialization noise.
    let _ = policy
        .retry(|_| Ok::<i32, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();

    let (min_allocations, min_bytes) = min_allocated_during(|| {
        let _ = policy
            .retry(|_| Err::<i32, &str>(ERROR_VALUE))
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
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO))
        .boxed::<i32, &str>();

    let _ = policy
        .retry(|_| Err::<i32, &str>(ERROR_VALUE))
        .sleep(instant_sleep)
        .call();

    let (min_allocations, min_bytes) = min_allocated_during(|| {
        let _ = policy
            .retry(|_| Err::<i32, &str>(ERROR_VALUE))
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

#[cfg(feature = "alloc")]
#[test]
fn async_retry_execution_is_allocation_free_after_warmup() {
    let _guard = allocation_test_guard();
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));

    let run_once = || {
        let call_count = Cell::new(0_u32);
        block_on(
            policy
                .retry_async(|_| {
                    let call_count_ref = &call_count;
                    call_count_ref.set(call_count_ref.get().saturating_add(1));
                    async move {
                        if call_count_ref.get() < MAX_ATTEMPTS {
                            Err::<i32, _>(ERROR_VALUE)
                        } else {
                            Ok(SUCCESS_VALUE)
                        }
                    }
                })
                .sleep(InstantSleeper),
        )
    };

    let _ = run_once();
    let (min_allocations, min_bytes) = min_allocated_during(|| {
        let _ = run_once();
    });

    assert_eq!(
        min_allocations, 0,
        "async execution should not allocate after warmup"
    );
    assert_eq!(
        min_bytes, 0,
        "async execution should not allocate bytes after warmup"
    );
}
