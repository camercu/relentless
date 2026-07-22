//! Implementing `Outcome` for a type you own.
//!
//! When an operation returns a type that is not `Result`, implement the
//! `Outcome` trait so the type classifies *itself*. The default retry path then
//! drives it with **no `.decide` at the call site**: a `Return` verdict becomes
//! the `Ok` value, `Abort` becomes `RetryError::Aborted`, and `Retry` keeps
//! polling. This is the zero-annotation path for domain types — reach for
//! `.decide(...)` only when you want per-call-site classification or are working
//! with a `Result`.
//!
//! Run: `cargo run --example custom-outcome`
use core::cell::Cell;
use core::time::Duration;
use relentless::clock::VirtualClock;
use relentless::{Outcome, RetryError, Verdict, retry, stop, wait};

/// A rollout poll: still rolling out, live at some version, or failed.
#[derive(Debug, Clone, PartialEq)]
enum Rollout {
    InProgress,
    Live { version: u32 },
    Failed(&'static str),
}

// Implementing `Outcome` makes `Rollout` classify itself. `Return` is delivered
// to the caller as the `Ok` value; `Abort` as the error payload.
impl Outcome for Rollout {
    type Return = u32; // the live version
    type Abort = &'static str; // the failure reason

    fn classify(self) -> Verdict<u32, &'static str, Rollout> {
        match self {
            Rollout::Live { version } => Verdict::Return(version),
            Rollout::Failed(reason) => Verdict::Abort(reason),
            Rollout::InProgress => Verdict::Retry(self),
        }
    }
}

/// Drives a scripted sequence of poll outcomes so the example is deterministic.
fn scripted(script: &'static [Rollout]) -> impl Fn() -> Rollout {
    let idx = Cell::new(0);
    move || {
        let i = idx.get().min(script.len() - 1);
        idx.set(i + 1);
        script[i].clone()
    }
}

/// Polls until the rollout settles. Note the absence of `.decide` — the default
/// classifier delegates to `Rollout::classify`.
fn await_rollout(poll: impl Fn() -> Rollout) -> Result<u32, RetryError<&'static str, Rollout>> {
    retry(|_| poll())
        .stop(stop::attempts(10))
        .wait(wait::fixed(Duration::from_millis(10)))
        .clock(VirtualClock::new()) // no real waiting in the example
        .call()
}

fn main() {
    // Rolls out for a bit, then goes live at v7.
    let live = await_rollout(scripted(&[
        Rollout::InProgress,
        Rollout::InProgress,
        Rollout::Live { version: 7 },
    ]));
    println!("live path   : {live:?}");
    assert_eq!(live, Ok(7));

    // Fails mid-rollout — aborts immediately with the reason.
    let failed = await_rollout(scripted(&[
        Rollout::InProgress,
        Rollout::Failed("image pull error"),
    ]));
    println!("failed path : {failed:?}");
    assert!(matches!(
        failed,
        Err(RetryError::Aborted {
            last: "image pull error"
        })
    ));

    // Never settles — exhausts the attempt budget, keeping the last outcome.
    let stuck = await_rollout(scripted(&[Rollout::InProgress]));
    println!("stuck path  : {stuck:?}");
    assert!(matches!(
        stuck,
        Err(RetryError::Exhausted {
            last: Rollout::InProgress
        })
    ));
}
