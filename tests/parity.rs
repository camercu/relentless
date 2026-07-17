//! Sync/async differential parity suite.
//!
//! The sync and async execution paths are deliberately duplicated source files
//! wrapping one shared state machine. This suite replaces the manual
//! "audit all four paths for drift" rule with an executable check: every
//! scenario runs through both engines on a shared `VirtualClock` and the full
//! observable traces — result, hook firings (with timing fields), sleeps,
//! stats — must be identical.

#![cfg(feature = "test-util")]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use std::format;
use std::string::String;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use proptest::prelude::{Strategy, any, prop_assert_eq, proptest};
use relentless::clock::VirtualClock;
use relentless::test_util::VirtualClock as AsyncVirtualClock;
use relentless::{RetryError, RetryStats, predicate, retry, retry_async, stop, wait};

const ARBITRARY_ERROR: &str = "boom";

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

/// Everything observable about one execution, rendered comparable.
#[derive(Debug, PartialEq)]
struct Trace {
    result: String,
    hooks: Vec<String>,
    sleeps: Vec<Duration>,
    stats: RetryStats,
}

/// One scripted retry run: per-attempt outcomes plus policy configuration.
#[derive(Clone)]
struct Scenario {
    /// Outcome per attempt; attempts beyond the script repeat its last entry.
    script: Vec<Result<u32, &'static str>>,
    max_attempts: u32,
    wait: WaitKind,
    timeout: Option<Duration>,
    /// `Some(e)` marks error `e` non-retryable (predicate rejection).
    reject_error: Option<&'static str>,
    /// `Some(v)` polls until `Ok(v)` via `.until(...)` instead of `.when(...)`.
    until_ok: Option<u32>,
}

impl Scenario {
    fn new(script: Vec<Result<u32, &'static str>>, max_attempts: u32, wait: WaitKind) -> Self {
        Self {
            script,
            max_attempts,
            wait,
            timeout: None,
            reject_error: None,
            until_ok: None,
        }
    }

    fn outcome_for(&self, attempt: u32) -> Result<u32, &'static str> {
        let index = usize::min(attempt as usize - 1, self.script.len() - 1);
        self.script[index]
    }
}

#[derive(Clone, Copy, Debug)]
enum WaitKind {
    Fixed(Duration),
    Linear(Duration, Duration),
    Exponential(Duration),
    ExponentialCapped(Duration, Duration),
}

impl WaitKind {
    fn build(self) -> Box<dyn relentless::Wait + Send> {
        match self {
            WaitKind::Fixed(dur) => Box::new(wait::fixed(dur)),
            WaitKind::Linear(initial, step) => Box::new(wait::linear(initial, step)),
            WaitKind::Exponential(initial) => Box::new(wait::exponential(initial)),
            WaitKind::ExponentialCapped(initial, cap) => {
                Box::new(relentless::Wait::cap(wait::exponential(initial), cap))
            }
        }
    }
}

type HookLog = Arc<Mutex<Vec<String>>>;

fn log(hooks: &HookLog, entry: String) {
    hooks.lock().expect("hook log poisoned").push(entry);
}

fn run_sync(scenario: &Scenario) -> Trace {
    let clock = VirtualClock::new();
    let hooks: HookLog = Arc::new(Mutex::new(Vec::new()));
    let (before, after, exit) = (Arc::clone(&hooks), Arc::clone(&hooks), Arc::clone(&hooks));

    let scenario_run = scenario.clone();
    let exec = retry(move |state| scenario_run.outcome_for(state.attempt))
        .stop(stop::attempts(scenario.max_attempts))
        .wait(scenario.wait.build())
        .before_attempt(move |state| {
            log(
                &before,
                format!(
                    "before:{}:{:?}:{:?}",
                    state.attempt, state.elapsed, state.previous_delay
                ),
            );
        })
        .after_attempt(move |state| {
            log(
                &after,
                format!(
                    "after:{}:{:?}:{:?}:{:?}",
                    state.attempt, state.elapsed, state.outcome, state.next_delay
                ),
            );
        })
        .on_exit(move |state| {
            log(
                &exit,
                format!(
                    "exit:{}:{:?}:{:?}:{:?}",
                    state.attempt, state.elapsed, state.outcome, state.stop_reason
                ),
            );
        })
        .clock(&clock);

    let (result, stats): (Result<u32, RetryError<u32, &str>>, RetryStats) =
        match (scenario.reject_error, scenario.until_ok) {
            (Some(rejected), None) => exec
                .when(predicate::error(move |e: &&str| *e != rejected))
                .with_stats()
                .call(),
            (None, Some(target)) => exec
                .until(predicate::ok(move |v: &u32| *v == target))
                .with_stats()
                .call(),
            (None, None) => match scenario.timeout {
                Some(dur) => exec.timeout(dur).with_stats().call(),
                None => exec.with_stats().call(),
            },
            (Some(_), Some(_)) => unreachable!("scenarios use either reject or until, not both"),
        };

    Trace {
        result: format!("{result:?}"),
        hooks: hooks.lock().expect("hook log poisoned").clone(),
        sleeps: clock.waits(),
        stats,
    }
}

fn run_async(scenario: &Scenario) -> Trace {
    let clock = AsyncVirtualClock::new();
    let hooks: HookLog = Arc::new(Mutex::new(Vec::new()));
    let (before, after, exit) = (Arc::clone(&hooks), Arc::clone(&hooks), Arc::clone(&hooks));

    let scenario_run = scenario.clone();
    let exec = retry_async(move |state| {
        let outcome = scenario_run.outcome_for(state.attempt);
        core::future::ready(outcome)
    })
    .stop(stop::attempts(scenario.max_attempts))
    .wait(scenario.wait.build())
    .before_attempt(move |state| {
        log(
            &before,
            format!(
                "before:{}:{:?}:{:?}",
                state.attempt, state.elapsed, state.previous_delay
            ),
        );
    })
    .after_attempt(move |state| {
        log(
            &after,
            format!(
                "after:{}:{:?}:{:?}:{:?}",
                state.attempt, state.elapsed, state.outcome, state.next_delay
            ),
        );
    })
    .on_exit(move |state| {
        log(
            &exit,
            format!(
                "exit:{}:{:?}:{:?}:{:?}",
                state.attempt, state.elapsed, state.outcome, state.stop_reason
            ),
        );
    })
    .elapsed_clock_fn(clock.clock())
    .sleep(clock.async_sleep());

    let (result, stats): (Result<u32, RetryError<u32, &str>>, RetryStats) =
        match (scenario.reject_error, scenario.until_ok) {
            (Some(rejected), None) => block_on(
                exec.when(predicate::error(move |e: &&str| *e != rejected))
                    .with_stats()
                    .call(),
            ),
            (None, Some(target)) => block_on(
                exec.until(predicate::ok(move |v: &u32| *v == target))
                    .with_stats()
                    .call(),
            ),
            (None, None) => match scenario.timeout {
                Some(dur) => block_on(exec.timeout(dur).with_stats().call()),
                None => block_on(exec.with_stats().call()),
            },
            (Some(_), Some(_)) => unreachable!("scenarios use either reject or until, not both"),
        };

    Trace {
        result: format!("{result:?}"),
        hooks: hooks.lock().expect("hook log poisoned").clone(),
        sleeps: clock.sleeps(),
        stats,
    }
}

#[track_caller]
fn assert_parity(scenario: &Scenario) {
    let sync_trace = run_sync(scenario);
    let async_trace = run_async(scenario);
    // Field-by-field so a failure names the diverged dimension directly.
    assert_eq!(
        sync_trace.result, async_trace.result,
        "sync/async result diverged"
    );
    assert_eq!(
        sync_trace.hooks, async_trace.hooks,
        "sync/async hook traces diverged"
    );
    assert_eq!(
        sync_trace.sleeps, async_trace.sleeps,
        "sync/async sleep sequences diverged"
    );
    assert_eq!(
        sync_trace.stats, async_trace.stats,
        "sync/async stats diverged"
    );
}

/// GIVEN an operation succeeding on attempt 3 with exponential backoff
/// WHEN it runs through the sync and async engines on virtual clocks
/// THEN the full traces (result, hooks, sleeps, stats) are identical
#[test]
fn parity_success_after_failures() {
    let scenario = Scenario::new(
        vec![Err(ARBITRARY_ERROR), Err(ARBITRARY_ERROR), Ok(7)],
        5,
        WaitKind::Exponential(Duration::from_millis(100)),
    );

    let trace = run_sync(&scenario);
    assert_eq!(trace.sleeps.len(), 2, "guard: scenario must actually sleep");
    assert_parity(&scenario);
}

/// GIVEN an operation that never succeeds and a 3-attempt budget
/// WHEN the stop strategy fires (Exhausted) in both engines
/// THEN the traces are identical
#[test]
fn parity_exhausted_by_attempts() {
    let scenario = Scenario::new(
        vec![Err(ARBITRARY_ERROR)],
        3,
        WaitKind::Linear(Duration::from_millis(10), Duration::from_millis(5)),
    );

    let trace = run_sync(&scenario);
    assert!(
        trace.result.contains("Exhausted"),
        "guard: stop must fire; got {}",
        trace.result
    );
    assert_parity(&scenario);
}

/// GIVEN a single-attempt budget (boundary: no sleeps possible)
/// WHEN the only attempt fails in both engines
/// THEN the traces are identical
#[test]
fn parity_single_attempt() {
    let scenario = Scenario::new(
        vec![Err(ARBITRARY_ERROR)],
        1,
        WaitKind::Fixed(Duration::from_millis(10)),
    );
    assert_parity(&scenario);
}

/// GIVEN a zero-duration wait (boundary: sleeps requested but empty)
/// WHEN retries run in both engines
/// THEN the traces are identical
#[test]
fn parity_zero_wait() {
    let scenario = Scenario::new(
        vec![Err(ARBITRARY_ERROR), Ok(1)],
        3,
        WaitKind::Fixed(Duration::ZERO),
    );
    assert_parity(&scenario);
}

/// GIVEN a predicate rejecting a specific error on attempt 2
/// WHEN both engines terminate as Rejected
/// THEN the traces are identical
#[test]
fn parity_rejected_by_predicate() {
    let mut scenario = Scenario::new(
        vec![Err("transient"), Err(ARBITRARY_ERROR)],
        5,
        WaitKind::Fixed(Duration::from_millis(10)),
    );
    scenario.reject_error = Some(ARBITRARY_ERROR);

    let trace = run_sync(&scenario);
    assert!(
        trace.result.contains("Rejected"),
        "guard: predicate must reject; got {}",
        trace.result
    );
    assert_parity(&scenario);
}

/// GIVEN an `.until(...)` polling run where Ok values stay "pending" until attempt 3
/// WHEN both engines poll to completion
/// THEN the traces are identical
#[test]
fn parity_until_polling() {
    let mut scenario = Scenario::new(
        vec![Ok(1), Ok(2), Ok(7)],
        5,
        WaitKind::ExponentialCapped(Duration::from_millis(100), Duration::from_millis(150)),
    );
    scenario.until_ok = Some(7);

    let trace = run_sync(&scenario);
    assert_eq!(trace.sleeps.len(), 2, "guard: polling must sleep twice");
    assert_parity(&scenario);
}

/// GIVEN a timeout budget driven by the virtual clock, with sleep clamping
/// WHEN both engines exhaust the budget
/// THEN the traces (including the clamped final sleep) are identical
#[test]
fn parity_timeout_clamps_final_sleep() {
    let mut scenario = Scenario::new(
        vec![Err(ARBITRARY_ERROR)],
        10,
        WaitKind::Fixed(Duration::from_millis(100)),
    );
    scenario.timeout = Some(Duration::from_millis(250));

    let trace = run_sync(&scenario);
    assert_eq!(
        trace.sleeps.last(),
        Some(&Duration::from_millis(50)),
        "guard: final sleep must clamp to remaining budget"
    );
    assert_parity(&scenario);
}

proptest! {
    /// Random scripts, budgets, waits, and timeouts: the two engines must
    /// stay trace-identical everywhere, not just at hand-picked points.
    /// Failing cases persist under `proptest-regressions/` for deterministic
    /// replay.
    #[test]
    fn parity_holds_for_random_scenarios(
        script in proptest::collection::vec(
            any::<bool>().prop_map(|ok| if ok { Ok(1_u32) } else { Err(ARBITRARY_ERROR) }),
            1..6,
        ),
        max_attempts in 1..6_u32,
        wait_millis in 0..300_u64,
        wait_kind in 0..4_u8,
        timeout_millis in proptest::option::of(0..500_u64),
    ) {
        let base = Duration::from_millis(wait_millis);
        let wait = match wait_kind {
            0 => WaitKind::Fixed(base),
            1 => WaitKind::Linear(base, Duration::from_millis(wait_millis / 2)),
            2 => WaitKind::Exponential(base),
            _ => WaitKind::ExponentialCapped(base, Duration::from_millis(wait_millis.saturating_mul(2))),
        };
        let mut scenario = Scenario::new(script, max_attempts, wait);
        scenario.timeout = timeout_millis.map(Duration::from_millis);

        let sync_trace = run_sync(&scenario);
        let async_trace = run_async(&scenario);
        prop_assert_eq!(sync_trace, async_trace);
    }
}
