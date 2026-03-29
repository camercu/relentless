//! Tests for the Sleeper trait and its blanket impl for `Fn(Duration) -> Future`.
//!
//! Uses a no-op waker to drive futures to completion without a real async runtime,
//! and verifies that both direct struct impls and closure-based impls satisfy the trait.

use core::time::Duration;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tenacious::Sleeper;

const ARBITRARY_DURATION: Duration = Duration::from_millis(10);

struct Immediate;

impl Future for Immediate {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }
}

struct ImmediateSleeper;

impl Sleeper for ImmediateSleeper {
    type Sleep = Immediate;
    fn sleep(&self, _dur: Duration) -> Self::Sleep {
        Immediate
    }
}

fn noop_waker() -> std::task::Waker {
    struct NoopWake;
    impl std::task::Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    std::task::Waker::from(Arc::new(NoopWake))
}

#[test]
fn sleeper_trait_direct_impl() {
    let sleeper = ImmediateSleeper;
    let mut fut = sleeper.sleep(ARBITRARY_DURATION);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(Pin::new(&mut fut).poll(&mut cx), Poll::Ready(())));
}

#[test]
fn sleeper_blanket_impl_for_closure() {
    let sleeper_fn = |_dur: Duration| Immediate;
    let mut fut = Sleeper::sleep(&sleeper_fn, ARBITRARY_DURATION);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(Pin::new(&mut fut).poll(&mut cx), Poll::Ready(())));
}

struct DelayedReady(bool);

impl Future for DelayedReady {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[test]
fn sleeper_blanket_impl_different_future_type() {
    let sleeper_fn = |_dur: Duration| DelayedReady(false);
    let mut fut = Sleeper::sleep(&sleeper_fn, Duration::ZERO);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(Pin::new(&mut fut).poll(&mut cx), Poll::Pending));
    assert!(matches!(Pin::new(&mut fut).poll(&mut cx), Poll::Ready(())));
}

#[cfg(any(feature = "futures-timer-sleep", feature = "tokio-sleep"))]
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        match Future::poll(Pin::as_mut(&mut future), &mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => core::hint::spin_loop(),
        }
    }
}

#[cfg(feature = "tokio-sleep")]
#[test]
fn tokio_sleep_helper_is_sleep_compatible() {
    let helper: fn(Duration) -> tokio::time::Sleep = tenacious::sleep::tokio();

    let policy = tenacious::RetryPolicy::new().stop(tenacious::stop::attempts(1));
    let result: Result<(), tenacious::RetryError<(), &str>> = block_on(
        policy
            .retry_async(|_| async { Ok::<(), &str>(()) })
            .sleep(helper),
    );
    assert_eq!(result, Ok(()));
}

#[cfg(feature = "futures-timer-sleep")]
#[test]
fn futures_timer_sleep_helper_is_sleep_compatible() {
    let helper: fn(Duration) -> futures_timer::Delay = tenacious::sleep::futures_timer();

    let policy = tenacious::RetryPolicy::new().stop(tenacious::stop::attempts(1));
    let result: Result<(), tenacious::RetryError<(), &str>> = block_on(
        policy
            .retry_async(|_| async { Ok::<(), &str>(()) })
            .sleep(helper),
    );
    assert_eq!(result, Ok(()));
}

#[cfg(all(feature = "embassy-sleep", target_os = "none"))]
#[test]
fn embassy_sleep_helper_is_sleep_compatible() {
    let helper: fn(Duration) -> embassy_time::Timer = tenacious::sleep::embassy();
    assert_ne!(helper as usize, 0);
}

#[cfg(all(feature = "gloo-timers-sleep", target_arch = "wasm32"))]
#[test]
fn gloo_sleep_helper_is_sleep_compatible() {
    let helper: fn(Duration) -> gloo_timers::future::TimeoutFuture = tenacious::sleep::gloo();
    assert_ne!(helper as usize, 0);
}
