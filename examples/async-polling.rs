use core::future::{Future, ready};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use std::sync::Arc;
use tenacious::{RetryError, RetryPolicy, predicate, stop, wait};

const MAX_ATTEMPTS: u32 = 4;
const WAIT_DURATION: Duration = Duration::from_millis(25);

fn main() {
    let attempts = std::cell::Cell::new(0_u32);
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(WAIT_DURATION))
        .when(predicate::ok(|status: &&str| *status != "ready"));

    let result: Result<&'static str, RetryError<&'static str, &'static str>> = block_on(
        policy
            .retry_async(|_| {
                attempts.set(attempts.get().saturating_add(1));
                ready(Ok(if attempts.get() < MAX_ATTEMPTS {
                    "pending"
                } else {
                    "ready"
                }))
            })
            .sleep(|_dur: Duration| ready(())),
    );

    assert_eq!(result, Ok("ready"));
    assert_eq!(attempts.get(), MAX_ATTEMPTS);
}

fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
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

fn noop_waker() -> Waker {
    struct NoopWake;

    impl std::task::Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    Waker::from(Arc::new(NoopWake))
}
