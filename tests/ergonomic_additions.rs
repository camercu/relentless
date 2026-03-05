//! Acceptance tests for ergonomic additions (Spec iteration 15).

use core::time::Duration;
#[cfg(any(feature = "futures-timer-sleep", feature = "tokio-sleep"))]
use core::{future::Future, pin::Pin, task::Context, task::Poll};
#[cfg(any(feature = "futures-timer-sleep", feature = "tokio-sleep"))]
use std::sync::Arc;
#[cfg(any(feature = "futures-timer-sleep", feature = "tokio-sleep"))]
use tenacious::RetryPolicy;
use tenacious::{Predicate, Stop, StopExt, Wait, WaitExt, on, stop, wait};

const ARBITRARY_ATTEMPT: u32 = 3;
const ARBITRARY_ELAPSED: Duration = Duration::from_secs(2);
const ARBITRARY_WAIT_A: Duration = Duration::from_millis(7);
const ARBITRARY_WAIT_B: Duration = Duration::from_millis(11);

#[cfg(any(feature = "futures-timer-sleep", feature = "tokio-sleep"))]
fn noop_waker() -> std::task::Waker {
    struct NoopWake;
    impl std::task::Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    std::task::Waker::from(Arc::new(NoopWake))
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

fn state(attempt: u32, elapsed: Option<Duration>) -> tenacious::RetryState {
    tenacious::RetryState {
        attempt,
        elapsed,
        next_delay: Duration::ZERO,
        total_wait: Duration::ZERO,
    }
}

#[test]
fn stop_named_combinators_match_operator_forms() {
    let mut named_or = stop::attempts(ARBITRARY_ATTEMPT).or(stop::elapsed(ARBITRARY_ELAPSED));
    let mut op_or = stop::attempts(ARBITRARY_ATTEMPT) | stop::elapsed(ARBITRARY_ELAPSED);

    let early = state(1, Some(Duration::from_secs(1)));
    let elapsed_hit = state(1, Some(ARBITRARY_ELAPSED));
    let attempt_hit = state(ARBITRARY_ATTEMPT, None);
    assert_eq!(named_or.should_stop(&early), op_or.should_stop(&early));
    assert_eq!(
        named_or.should_stop(&elapsed_hit),
        op_or.should_stop(&elapsed_hit)
    );
    assert_eq!(
        named_or.should_stop(&attempt_hit),
        op_or.should_stop(&attempt_hit)
    );

    let mut named_and = stop::attempts(ARBITRARY_ATTEMPT).and(stop::elapsed(ARBITRARY_ELAPSED));
    let mut op_and = stop::attempts(ARBITRARY_ATTEMPT) & stop::elapsed(ARBITRARY_ELAPSED);

    let both_hit = state(ARBITRARY_ATTEMPT, Some(ARBITRARY_ELAPSED));
    assert_eq!(named_and.should_stop(&early), op_and.should_stop(&early));
    assert_eq!(
        named_and.should_stop(&elapsed_hit),
        op_and.should_stop(&elapsed_hit)
    );
    assert_eq!(
        named_and.should_stop(&both_hit),
        op_and.should_stop(&both_hit)
    );
}

#[test]
fn predicate_named_combinators_match_operator_forms() {
    let named_or = on::error(|err: &&str| *err == "retryable").or(on::ok(|value: &u32| *value < 2));
    let op_or = on::error(|err: &&str| *err == "retryable") | on::ok(|value: &u32| *value < 2);

    assert_eq!(
        named_or.should_retry(&Err("retryable")),
        op_or.should_retry(&Err("retryable"))
    );
    assert_eq!(
        named_or.should_retry(&Ok(1_u32)),
        op_or.should_retry(&Ok(1_u32))
    );
    assert_eq!(
        named_or.should_retry(&Err("fatal")),
        op_or.should_retry(&Err("fatal"))
    );

    let named_and = on::any_error().and(on::error(|err: &&str| *err == "retryable"));
    let op_and = on::any_error() & on::error(|err: &&str| *err == "retryable");
    assert_eq!(
        named_and.should_retry(&Err::<u32, &str>("retryable")),
        op_and.should_retry(&Err::<u32, &str>("retryable"))
    );
    assert_eq!(
        named_and.should_retry(&Err::<u32, &str>("fatal")),
        op_and.should_retry(&Err::<u32, &str>("fatal"))
    );
    assert_eq!(
        named_and.should_retry(&Ok(1_u32)),
        op_and.should_retry(&Ok(1_u32))
    );
}

#[test]
fn wait_named_add_matches_operator_and_supports_custom_wait() {
    let retry_state = state(1, None);

    let mut named = wait::fixed(ARBITRARY_WAIT_A).add(wait::fixed(ARBITRARY_WAIT_B));
    let mut op = wait::fixed(ARBITRARY_WAIT_A) + wait::fixed(ARBITRARY_WAIT_B);
    assert_eq!(named.next_wait(&retry_state), op.next_wait(&retry_state));

    #[derive(Clone, Copy)]
    struct CustomWait(Duration);
    impl Wait for CustomWait {
        fn next_wait(&mut self, _state: &tenacious::RetryState) -> Duration {
            self.0
        }
    }

    let mut custom = CustomWait(ARBITRARY_WAIT_A).add(wait::fixed(ARBITRARY_WAIT_B));
    assert_eq!(
        custom.next_wait(&retry_state),
        ARBITRARY_WAIT_A.saturating_add(ARBITRARY_WAIT_B)
    );
}

#[cfg(feature = "tokio-sleep")]
#[test]
fn tokio_sleep_helper_is_sleep_compatible() {
    let helper: fn(Duration) -> tokio::time::Sleep = tenacious::sleep::tokio();

    let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    let result: Result<(), tenacious::RetryError<&str>> = block_on(
        policy
            .retry_async(|| async { Ok::<(), &str>(()) })
            .sleep(helper),
    );
    assert_eq!(result, Ok(()));
}

#[cfg(feature = "futures-timer-sleep")]
#[test]
fn futures_timer_sleep_helper_is_sleep_compatible() {
    let helper: fn(Duration) -> futures_timer::Delay = tenacious::sleep::futures_timer();

    let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    let result: Result<(), tenacious::RetryError<&str>> = block_on(
        policy
            .retry_async(|| async { Ok::<(), &str>(()) })
            .sleep(helper),
    );
    assert_eq!(result, Ok(()));
}

#[cfg(feature = "embassy-sleep")]
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
