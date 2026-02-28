use core::time::Duration;
use std::hint::black_box;
use std::time::Instant;
use tenacious::{RetryPolicy, stop, wait};

const WARMUP_ITERS: u32 = 2_000;
const BENCH_ITERS: u32 = 50_000;
const MAX_ATTEMPTS: u32 = 3;
const SUCCESS_VALUE: i32 = 42;
const ERROR_VALUE: &str = "fail";
const FIXED_WAIT: Duration = Duration::from_millis(1);

fn instant_sleep(_dur: Duration) {}

fn run_case(name: &str, mut case: impl FnMut()) {
    for _ in 0..WARMUP_ITERS {
        case();
    }

    let start = Instant::now();
    for _ in 0..BENCH_ITERS {
        case();
    }
    let elapsed = start.elapsed();
    let nanos_per_iter = elapsed.as_nanos() / u128::from(BENCH_ITERS);

    println!("{name:36} total={elapsed:?} ns/iter={nanos_per_iter}");
}

fn sync_success_first_attempt() {
    let mut policy = RetryPolicy::new().stop(stop::attempts(1));
    let result = policy
        .retry(|| Ok::<i32, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();
    black_box(result).expect("success path benchmark must succeed");
}

fn sync_retry_until_success() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));
    let mut attempts = 0_u32;

    let result = policy
        .retry(|| {
            attempts = attempts.saturating_add(1);
            if attempts < MAX_ATTEMPTS {
                Err::<i32, &str>(ERROR_VALUE)
            } else {
                Ok(SUCCESS_VALUE)
            }
        })
        .sleep(instant_sleep)
        .call();
    black_box(result).expect("retry benchmark must eventually succeed");
}

fn sync_retry_exhausted_with_wait() {
    let mut policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(FIXED_WAIT));
    let result = policy
        .retry(|| Err::<i32, &str>(ERROR_VALUE))
        .sleep(instant_sleep)
        .call();
    let _ = black_box(result);
}

fn main() {
    println!("tenacious micro-benchmarks (deterministic in-process):");
    run_case("sync_success_first_attempt", sync_success_first_attempt);
    run_case("sync_retry_until_success", sync_retry_until_success);
    run_case(
        "sync_retry_exhausted_with_wait",
        sync_retry_exhausted_with_wait,
    );
}
