//! Polling where an outcome can be *pending*, *succeeded*, or *terminally
//! failed* — the realistic shape of "wait for a deployment / job / migration."
//!
//! The key modeling rule: a retry predicate returns a single bool, and stopping
//! on an `Ok` always resolves to **success**. There is no way to make an
//! `Ok(_)` terminate as a failure. So a *terminal failure* must live in the
//! `Err` channel, and the fail-fast condition is combined (with `|`) *inside*
//! `.until(...)`:
//!
//! ```text
//! .until( ok(is_done) | error(is_fatal) )
//! ```
//!
//! `.until(p)` retries until `p` accepts, i.e. it retries on `!p`. So `p` is the
//! set of *terminal* outcomes:
//!
//! | outcome        | `p` accepts? | result                         |
//! |----------------|--------------|--------------------------------|
//! | `Ok(Done)`     | yes (ok)     | success `Ok(Done)`             |
//! | `Ok(Pending)`  | no           | retry                          |
//! | `Err(Transient)` | no         | retry                          |
//! | `Err(Fatal)`   | yes (error)  | `RetryError::Rejected{Fatal}`  |
//!
//! Run: `cargo run --example polling-terminal-failure`
use core::cell::Cell;
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{RetryError, predicate, stop, wait};

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

impl JobError {
    fn is_fatal(&self) -> bool {
        matches!(self, JobError::Fatal(_))
    }
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
) -> Result<JobStatus, RetryError<JobStatus, JobError>> {
    relentless::retry(|_| poller())
        .until(
            predicate::ok(|s: &JobStatus| *s == JobStatus::Done)
                | predicate::error(JobError::is_fatal),
        )
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
        Err(RetryError::Rejected {
            last: JobError::Fatal(_)
        })
    ));

    // Case 3: never settles — exhausts the attempt budget.
    let stuck = poll_until_done(scripted_poller(&[Ok(JobStatus::Pending)]));
    println!("stuck path : {stuck:?}");
    assert!(matches!(stuck, Err(RetryError::Exhausted { .. })));
}
