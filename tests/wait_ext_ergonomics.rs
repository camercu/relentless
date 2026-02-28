//! Ergonomics tests for `WaitExt` on custom wait strategies.

use core::time::Duration;
use tenacious::{Wait, WaitExt, wait};

const ARBITRARY_BASE_WAIT: Duration = Duration::from_millis(10);
const ARBITRARY_INCREMENT_WAIT: Duration = Duration::from_millis(5);
const ARBITRARY_CAP_WAIT: Duration = Duration::from_millis(22);
const ARBITRARY_FALLBACK_WAIT: Duration = Duration::from_millis(40);
const CHAIN_SWITCH_ATTEMPT: u32 = 2;

#[cfg(feature = "jitter")]
const ARBITRARY_JITTER_WAIT: Duration = Duration::from_millis(7);

#[derive(Clone, Copy)]
struct StepWait {
    base: Duration,
    increment: Duration,
}

impl Wait for StepWait {
    fn next_wait(&mut self, state: &tenacious::RetryState) -> Duration {
        let step = self
            .increment
            .checked_mul(state.attempt.saturating_sub(1))
            .unwrap_or(Duration::MAX);
        self.base.saturating_add(step)
    }
}

fn state(attempt: u32) -> tenacious::RetryState {
    tenacious::RetryState {
        attempt,
        elapsed: None,
        next_delay: Duration::ZERO,
        total_wait: Duration::ZERO,
    }
}

#[test]
fn custom_wait_supports_cap_and_chain_via_wait_ext() {
    let mut strategy = StepWait {
        base: ARBITRARY_BASE_WAIT,
        increment: ARBITRARY_INCREMENT_WAIT,
    }
    .cap(ARBITRARY_CAP_WAIT)
    .chain(wait::fixed(ARBITRARY_FALLBACK_WAIT), CHAIN_SWITCH_ATTEMPT);

    assert_eq!(
        strategy.next_wait(&state(1)),
        ARBITRARY_BASE_WAIT.min(ARBITRARY_CAP_WAIT)
    );
    assert_eq!(
        strategy.next_wait(&state(CHAIN_SWITCH_ATTEMPT + 1)),
        ARBITRARY_FALLBACK_WAIT
    );
}

#[cfg(feature = "jitter")]
#[test]
fn custom_wait_supports_jitter_via_wait_ext() {
    let mut strategy = StepWait {
        base: ARBITRARY_BASE_WAIT,
        increment: ARBITRARY_INCREMENT_WAIT,
    }
    .jitter(ARBITRARY_JITTER_WAIT);

    let baseline = ARBITRARY_BASE_WAIT.saturating_add(ARBITRARY_INCREMENT_WAIT);
    let upper = baseline.saturating_add(ARBITRARY_JITTER_WAIT);
    let wait = strategy.next_wait(&state(2));

    assert!(wait >= baseline);
    assert!(wait <= upper);
}
