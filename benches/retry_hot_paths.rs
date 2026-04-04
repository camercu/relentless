use core::time::Duration;
use relentless::{RetryPolicy, stop, wait};
use std::env;
use std::hint::black_box;
use std::time::Instant;

const WARMUP_ITERS: u32 = 2_000;
const BENCH_ITERS: u32 = 50_000;
const MAX_ATTEMPTS: u32 = 3;
const SUCCESS_VALUE: i32 = 42;
const ERROR_VALUE: &str = "fail";
const FIXED_WAIT: Duration = Duration::from_millis(1);
const BENCHMARK_KIND_SUFFIX: &str = ": benchmark";

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
    let policy = RetryPolicy::new().stop(stop::attempts(1));
    let result = policy
        .retry(|_| Ok::<i32, &str>(SUCCESS_VALUE))
        .sleep(instant_sleep)
        .call();
    black_box(result).expect("success path benchmark must succeed");
}

fn sync_retry_until_success() {
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(Duration::ZERO));
    let mut attempts = 0_u32;

    let result = policy
        .retry(|_| {
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
    let policy = RetryPolicy::new()
        .stop(stop::attempts(MAX_ATTEMPTS))
        .wait(wait::fixed(FIXED_WAIT));
    let result = policy
        .retry(|_| Err::<i32, &str>(ERROR_VALUE))
        .sleep(instant_sleep)
        .call();
    let _ = black_box(result);
}

type BenchCase = fn();

const BENCH_CASES: &[(&str, BenchCase)] = &[
    ("sync_success_first_attempt", sync_success_first_attempt),
    ("sync_retry_until_success", sync_retry_until_success),
    (
        "sync_retry_exhausted_with_wait",
        sync_retry_exhausted_with_wait,
    ),
];

fn list_cases_for_nextest() {
    for (name, _) in BENCH_CASES {
        println!("{name}{BENCHMARK_KIND_SUFFIX}");
    }
}

fn run_named_case(name: &str) -> Result<(), &'static str> {
    for (case_name, case) in BENCH_CASES {
        if *case_name == name {
            run_case(case_name, *case);
            return Ok(());
        }
    }
    Err("unknown benchmark case")
}

fn run_all_cases() {
    println!("relentless micro-benchmarks (deterministic in-process):");
    for (name, case) in BENCH_CASES {
        run_case(name, *case);
    }
}

enum RunMode {
    List,
    Named(String),
    All,
}

fn parse_mode_from_args() -> RunMode {
    let mut args = env::args().skip(1);
    let mut exact_case: Option<String> = None;
    let mut is_list_mode = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => {
                is_list_mode = true;
            }
            "--exact" => {
                exact_case = args.next();
            }
            _ => {}
        }
    }

    if is_list_mode {
        RunMode::List
    } else if let Some(case_name) = exact_case {
        RunMode::Named(case_name)
    } else {
        RunMode::All
    }
}

fn main() {
    match parse_mode_from_args() {
        RunMode::List => {
            list_cases_for_nextest();
        }
        RunMode::All => {
            run_all_cases();
        }
        RunMode::Named(case_name) => {
            if let Err(error) = run_named_case(&case_name) {
                eprintln!("{error}: {case_name}");
                std::process::exit(1);
            }
        }
    }
}
