//! Async API coverage that must remain available without the crate's `alloc` feature.

use core::cell::Cell;
use core::future::{Future, ready};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::Arc;

use tenacious::{AsyncRetryExt, RetryError, RetryPolicy, stop};

const SUCCESS_VALUE: i32 = 42;
const ERROR_VALUE: &str = "fail";
const MAX_ATTEMPTS: u32 = 3;

fn noop_waker() -> Waker {
    struct NoopWake;
    impl std::task::Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    Waker::from(Arc::new(NoopWake))
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    loop {
        match Future::poll(Pin::as_mut(&mut future), &mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[test]
fn policy_async_retry_remains_available_without_alloc() {
    let attempts = Cell::new(0_u32);
    let mut policy = RetryPolicy::new().stop(stop::attempts(MAX_ATTEMPTS));

    let result: Result<i32, RetryError<&str, i32>> = block_on(
        policy
            .retry_async(|| {
                attempts.set(attempts.get().saturating_add(1));
                if attempts.get() < MAX_ATTEMPTS {
                    ready(Err(ERROR_VALUE))
                } else {
                    ready(Ok(SUCCESS_VALUE))
                }
            })
            .before_attempt(|_state| {})
            .after_attempt(|_state| {})
            .on_exit(|_state| {})
            .sleep(|_dur| ready(())),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
}

#[test]
fn async_retry_ext_remains_available_without_alloc() {
    let attempts = Cell::new(0_u32);

    let result: Result<i32, RetryError<&str, i32>> = block_on(
        (|| {
            attempts.set(attempts.get().saturating_add(1));
            if attempts.get() < MAX_ATTEMPTS {
                ready(Err(ERROR_VALUE))
            } else {
                ready(Ok(SUCCESS_VALUE))
            }
        })
        .retry_async()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .before_attempt(|_state| {})
        .after_attempt(|_state| {})
        .on_exit(|_state| {})
        .sleep(|_dur| ready(())),
    );

    assert_eq!(result, Ok(SUCCESS_VALUE));
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
}
