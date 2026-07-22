//! Polling where an outcome can be *pending*, *succeeded*, or *terminally
//! failed* — the realistic shape of "wait for a deployment / job / migration."
//!
//! A classifier (`.decide`) sorts each poll outcome into a three-way `Verdict`,
//! so pending, success, and terminal failure are each modeled directly — no
//! encoding tricks, and the `Result` variant does not dictate the outcome:
//!
//! | outcome          | verdict          | result                       |
//! |------------------|------------------|------------------------------|
//! | `Ok(Done)`       | `Return(Done)`   | success `Ok(Done)`           |
//! | `Ok(Pending)`    | `Retry`          | retry                        |
//! | `Err(Transient)` | `Retry`          | retry                        |
//! | `Err(Fatal)`     | `Abort(Fatal)`   | `RetryError::Aborted{Fatal}` |
//!
//! Run: `cargo run --example polling-terminal-failure`
use core::cell::Cell;
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{RetryError, Verdict, stop, wait};

#[derive(Debug, Clone, PartialEq)]
enum JobStatus {
    Pending,
    Done,
}

#[derive(Debug, Clone, PartialEq)]
enum JobError {
    /// Control-plane blip — retry.
    Transient,
    /// Job rolled back / crashed — terminal, stop immediately.
    Fatal(&'static str),
}

/// Drives a scripted sequence of poll outcomes so the example is deterministic.
fn scripted_poller(
    script: &'static [Result<JobStatus, JobError>],
) -> impl Fn() -> Result<JobStatus, JobError> {
    let idx = Cell::new(0);
    move || {
        let i = idx.get().min(script.len() - 1);
        idx.set(i + 1);
        script[i].clone()
    }
}

fn poll_until_done(
    poller: impl Fn() -> Result<JobStatus, JobError>,
) -> Result<JobStatus, RetryError<JobError, Result<JobStatus, JobError>>> {
    relentless::retry(|_| poller())
        .decide(|outcome| match outcome {
            Ok(JobStatus::Done) => Verdict::Return(JobStatus::Done),
            Err(JobError::Fatal(msg)) => Verdict::Abort(JobError::Fatal(msg)),
            // Still pending or a transient blip: keep polling.
            Ok(JobStatus::Pending) | Err(JobError::Transient) => Verdict::Retry(outcome),
        })
        .wait(wait::fixed(Duration::from_millis(10)))
        .stop(stop::attempts(10))
        .clock(VirtualClock::new()) // no real waiting in the example
        .call()
}

fn main() {
    // Case 1: blips and stays pending, then succeeds.
    let happy = poll_until_done(scripted_poller(&[
        Err(JobError::Transient),
        Ok(JobStatus::Pending),
        Ok(JobStatus::Pending),
        Ok(JobStatus::Done),
    ]));
    println!("happy path : {happy:?}");
    assert_eq!(happy, Ok(JobStatus::Done));

    // Case 2: fails fatally — stops immediately, does NOT exhaust retries.
    let fatal = poll_until_done(scripted_poller(&[
        Ok(JobStatus::Pending),
        Err(JobError::Fatal("rolled back")),
        Ok(JobStatus::Pending), // never reached
    ]));
    println!("fatal path : {fatal:?}");
    assert!(matches!(
        fatal,
        Err(RetryError::Aborted {
            last: JobError::Fatal(_)
        })
    ));

    // Case 3: never settles — exhausts the attempt budget.
    let stuck = poll_until_done(scripted_poller(&[Ok(JobStatus::Pending)]));
    println!("stuck path : {stuck:?}");
    assert!(matches!(stuck, Err(RetryError::Exhausted { .. })));
}
