//! Verifies that concrete (non-boxed) sync and async retry execution paths allocate
//! zero heap memory after one-time initialization. The `stats_alloc` instrumented
//! allocator tracks every allocation, so any regression is caught immediately.
//! Boxed policies are also tested to confirm that construction allocates but
//! subsequent execution does not.

use core::time::Duration;
#[cfg(feature = "alloc")]
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};
#[cfg(feature = "alloc")]
use relentless::sleep::Sleeper;
use relentless::{RetryPolicy, Wait, stop, wait};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::sync::{Mutex, MutexGuard, OnceLock};
#[cfg(feature = "alloc")]
use std::{cell::Cell, pin::pin};

#[global_allocator]
static GLOBAL: &StatsAlloc<std::alloc::System> = &INSTRUMENTED_SYSTEM;

const MAX_ATTEMPTS: u32 = 3;
// Nonzero so the full-jitter draw actually runs: `random_duration_in` returns
// early without touching the PRNG when the range is zero, so a zero base would
// skip the very path this test guards.
const JITTER_BASE: Duration = Duration::from_millis(1);
const ERROR_VALUE: &str = "fail";
const SUCCESS_VALUE: i32 = 7;
const ALLOCATION_SAMPLE_RUNS: u32 = 16;

fn allocation_test_guard() -> MutexGuard<'static, ()> {
    static ALLOC_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ALLOC_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Wait-free clock with no wait recorder, so retry execution stays
/// allocation-free (`clock::VirtualClock` records waits into a `Vec`).
struct InstantClock(core::cell::Cell<Duration>);

impl InstantClock {
    fn new() -> Self {
        Self(core::cell::Cell::new(Duration::ZERO))
    }
}

impl relentless::Clock for InstantClock {
    fn now(&self) -> Duration {
        self.0.get()
    }
}

impl relentless::SyncClock for InstantClock {
    fn wait(&self, dur: Duration) {
        self.0.set(self.0.get().saturating_add(dur));
    }
}

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
        .clock(InstantClock::new())
        .call();

    let (min_allocations, min_bytes) = min_allocated_during(|| {
        let _ = policy
            .retry(|_| Err::<i32, &str>(ERROR_VALUE))
            .clock(InstantClock::new())
            .call();
    });

    assert_eq!(min_allocations, 0, "concrete execution should not allocate");
    assert_eq!(min_bytes, 0, "concrete execution should not allocate bytes");
}

#[test]
fn jittered_sync_retry_execution_is_allocation_free() {
    let _guard = allocation_test_guard();
    // The recommended production config: exponential backoff with full jitter.
    // Its PRNG draw happens inside `next_wait`, so a jittered wait must not
    // reintroduce allocation on the retry hot path.
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::exponential(JITTER_BASE).full_jitter());

    // Warm up one run to avoid one-time initialization noise.
    let _ = policy
        .retry(|_| Err::<i32, &str>(ERROR_VALUE))
        .clock(InstantClock::new())
        .call();

    let (min_allocations, min_bytes) = min_allocated_during(|| {
        let _ = policy
            .retry(|_| Err::<i32, &str>(ERROR_VALUE))
            .clock(InstantClock::new())
            .call();
    });

    assert_eq!(min_allocations, 0, "jittered execution should not allocate");
    assert_eq!(min_bytes, 0, "jittered execution should not allocate bytes");
}

#[cfg(feature = "alloc")]
#[test]
fn boxed_policy_construction_performs_heap_allocation() {
    let _guard = allocation_test_guard();
    let (min_allocations, min_bytes) = min_allocated_during(|| {
        let _policy = RetryPolicy::new()
            .stop(stop::attempts(MAX_ATTEMPTS))
            .wait(wait::fixed(Duration::ZERO))
            .boxed();
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
        .boxed();

    let _ = policy
        .retry(|_| Err::<i32, &str>(ERROR_VALUE))
        .clock(InstantClock::new())
        .call();

    let (min_allocations, min_bytes) = min_allocated_during(|| {
        let _ = policy
            .retry(|_| Err::<i32, &str>(ERROR_VALUE))
            .clock(InstantClock::new())
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
                .sleep(InstantSleeper)
                .call(),
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
