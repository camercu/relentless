use core::cell::RefCell;
use core::fmt;

use crate::compat::Duration;
use crate::state::RetryState;

use super::Wait;

#[cfg(target_has_atomic = "ptr")]
use core::sync::atomic::{AtomicUsize, Ordering};

/// Default seed for jitter PRNGs.
const DEFAULT_JITTER_SEED: u64 = 0x5A5A_5A5A_5A5A_5A5A;

/// Monotonic jitter nonce counter used to decorrelate independent policies.
#[cfg(target_has_atomic = "ptr")]
static JITTER_NONCE_COUNTER: AtomicUsize = AtomicUsize::new(1);

// ---------------------------------------------------------------------------
// SplitMix64 — inline PRNG
// ---------------------------------------------------------------------------

/// Fast, non-cryptographic PRNG for jitter decorrelation.
///
/// SplitMix64 has excellent avalanche properties and is widely used for
/// seeding other PRNGs (e.g., in Java's `SplittableRandom`). It is more
/// than sufficient for retry jitter where the goal is decorrelation, not
/// cryptographic security.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Returns a value in `[0, max]`.
    fn next_bounded(&mut self, max: u64) -> u64 {
        if max == u64::MAX {
            return self.next_u64();
        }
        // Simple modulo — bias is negligible for jitter ranges (nanoseconds).
        let range = max + 1;
        self.next_u64() % range
    }
}

impl Clone for SplitMix64 {
    fn clone(&self) -> Self {
        Self { state: self.state }
    }
}

impl fmt::Debug for SplitMix64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitMix64").finish_non_exhaustive()
    }
}

/// Creates an RNG seeded from the combination of a base seed and a nonce.
fn seeded_rng(seed: u64, nonce: u64) -> SplitMix64 {
    SplitMix64::new(seed ^ nonce)
}

// ---------------------------------------------------------------------------
// JitterKind — internal enum selecting the jitter computation
// ---------------------------------------------------------------------------

/// Internal jitter mode selector.
#[derive(Debug, Clone, Copy)]
enum JitterKind {
    /// `base + random(0, max_jitter)`
    Additive(Duration),
    /// `random(0, base)`
    Full,
    /// `base/2 + random(0, base/2)`
    Equal,
}

// ---------------------------------------------------------------------------
// Jittered<W> — unified jitter wrapper
// ---------------------------------------------------------------------------

/// A wrapper that applies jitter to an inner wait strategy's output.
///
/// Created by calling [`.jitter(max)`](Wait::jitter),
/// [`.full_jitter()`](Wait::full_jitter), or
/// [`.equal_jitter()`](Wait::equal_jitter) on any wait strategy.
///
/// Jitter uses a fast PRNG intended for retry backoff behavior, not for
/// cryptographic use. Cloning a `Jittered` strategy produces a decorrelated
/// copy — the clone will generate a different jitter sequence.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryState, Wait, wait};
/// use core::time::Duration;
///
/// // Additive jitter: base + random(0, max_jitter)
/// let strategy = wait::fixed(Duration::from_millis(50))
///     .jitter(Duration::from_millis(10));
/// let state = RetryState::new(1, None);
///
/// let next = strategy.next_wait(&state);
/// assert!(next >= Duration::from_millis(50));
/// assert!(next <= Duration::from_millis(60));
/// ```
///
/// ```
/// use tenacious::{RetryState, Wait, wait};
/// use core::time::Duration;
///
/// // Full jitter: random(0, base)
/// let strategy = wait::fixed(Duration::from_millis(100))
///     .full_jitter();
/// let state = RetryState::new(1, None);
///
/// let next = strategy.next_wait(&state);
/// assert!(next <= Duration::from_millis(100));
/// ```
///
/// ```
/// use tenacious::{RetryState, Wait, wait};
/// use core::time::Duration;
///
/// // Equal jitter: base/2 + random(0, base/2)
/// let strategy = wait::fixed(Duration::from_millis(100))
///     .equal_jitter();
/// let state = RetryState::new(1, None);
///
/// let next = strategy.next_wait(&state);
/// assert!(next >= Duration::from_millis(50));
/// assert!(next <= Duration::from_millis(100));
/// ```
#[derive(Debug)]
pub struct Jittered<W> {
    inner: W,
    kind: JitterKind,
    seed: u64,
    nonce: u64,
    rng: RefCell<SplitMix64>,
}

impl<W> Jittered<W> {
    fn new(inner: W, kind: JitterKind) -> Self {
        let nonce = next_jitter_nonce();
        Self {
            inner,
            kind,
            seed: DEFAULT_JITTER_SEED,
            nonce,
            rng: RefCell::new(seeded_rng(DEFAULT_JITTER_SEED, nonce)),
        }
    }

    pub(super) fn additive(inner: W, max_jitter: Duration) -> Self {
        Self::new(inner, JitterKind::Additive(max_jitter))
    }

    pub(super) fn full(inner: W) -> Self {
        Self::new(inner, JitterKind::Full)
    }

    pub(super) fn equal(inner: W) -> Self {
        Self::new(inner, JitterKind::Equal)
    }

    /// Sets an explicit PRNG seed for reproducible jitter when paired with
    /// [`with_nonce`](Self::with_nonce).
    #[must_use]
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self.rng = RefCell::new(seeded_rng(seed, self.nonce));
        self
    }

    /// Sets an explicit nonce used to decorrelate policy instances.
    #[must_use]
    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self.rng = RefCell::new(seeded_rng(self.seed, nonce));
        self
    }
}

impl<W: Clone> Clone for Jittered<W> {
    fn clone(&self) -> Self {
        let nonce = next_jitter_nonce();
        Self {
            inner: self.inner.clone(),
            kind: self.kind,
            seed: self.seed,
            nonce,
            rng: RefCell::new(seeded_rng(self.seed, nonce)),
        }
    }
}

impl<W: Wait> Wait for Jittered<W> {
    fn next_wait(&self, state: &RetryState) -> Duration {
        let base = self.inner.next_wait(state);
        match self.kind {
            JitterKind::Additive(max_jitter) => {
                let jitter = random_jitter_duration(max_jitter, &self.rng);
                base.saturating_add(jitter)
            }
            JitterKind::Full => random_jitter_duration(base, &self.rng),
            JitterKind::Equal => {
                let half = base / 2;
                let jitter = random_jitter_duration(half, &self.rng);
                half.saturating_add(jitter)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// WaitDecorrelatedJitter — decorrelated jitter: random(base, last_sleep * 3)
// ---------------------------------------------------------------------------

/// A standalone jitter strategy where each delay is random between `base` and
/// three times the previous delay.
///
/// This is the "Decorrelated Jitter" strategy from the [AWS Architecture Blog](https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/).
/// State is tracked via interior mutability (`Cell<Duration>`), consistent
/// with the `&self` model.
///
/// On the first attempt, `last_sleep` is `base`. Decorrelated jitter composes
/// with `.cap(max)` to bound the maximum delay.
///
/// Because decorrelated jitter is stateful, each concurrent or sequential
/// retry loop should use its own clone. Cloning produces a decorrelated
/// copy with a fresh PRNG stream and snapshots the current `last_sleep`
/// value.
///
/// # Examples
///
/// ```
/// use tenacious::{RetryState, Wait, wait};
/// use core::time::Duration;
///
/// let strategy = wait::decorrelated_jitter(Duration::from_millis(100))
///     .cap(Duration::from_secs(5));
/// let state = RetryState::new(1, None);
///
/// let next = strategy.next_wait(&state);
/// assert!(next >= Duration::from_millis(100));
/// assert!(next <= Duration::from_millis(300));
/// ```
#[derive(Debug)]
pub struct WaitDecorrelatedJitter {
    base: Duration,
    last_sleep: core::cell::Cell<Duration>,
    seed: u64,
    nonce: u64,
    rng: RefCell<SplitMix64>,
}

/// Produces a decorrelated jitter strategy: `random(base, last_sleep * 3)`.
///
/// On the first attempt, `last_sleep` is `base`.
#[must_use]
pub fn decorrelated_jitter(base: Duration) -> WaitDecorrelatedJitter {
    let nonce = next_jitter_nonce();
    WaitDecorrelatedJitter {
        base,
        last_sleep: core::cell::Cell::new(base),
        seed: DEFAULT_JITTER_SEED,
        nonce,
        rng: RefCell::new(seeded_rng(DEFAULT_JITTER_SEED, nonce)),
    }
}

impl WaitDecorrelatedJitter {
    /// Sets an explicit PRNG seed for reproducible jitter.
    #[must_use]
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self.rng = RefCell::new(seeded_rng(seed, self.nonce));
        self
    }

    /// Sets an explicit nonce used to decorrelate policy instances.
    #[must_use]
    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self.rng = RefCell::new(seeded_rng(self.seed, nonce));
        self
    }
}

impl Clone for WaitDecorrelatedJitter {
    fn clone(&self) -> Self {
        let nonce = next_jitter_nonce();
        Self {
            base: self.base,
            last_sleep: self.last_sleep.clone(),
            seed: self.seed,
            nonce,
            rng: RefCell::new(seeded_rng(self.seed, nonce)),
        }
    }
}

impl Wait for WaitDecorrelatedJitter {
    fn next_wait(&self, _state: &RetryState) -> Duration {
        let last = self.last_sleep.get();
        let upper = last.saturating_mul(3);
        let lower = self.base;

        // Generate random duration in [lower, upper]
        let delay = if upper <= lower {
            lower
        } else {
            let range_nanos = upper.as_nanos().saturating_sub(lower.as_nanos());
            if range_nanos == 0 {
                lower
            } else {
                let max_nanos = range_nanos.min(u64::MAX as u128) as u64;
                let random = self.rng.borrow_mut().next_bounded(max_nanos);
                lower.saturating_add(Duration::from_nanos(random))
            }
        };

        self.last_sleep.set(delay);
        delay
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Generates a random jitter duration in `[0, max_jitter]`.
fn random_jitter_duration(max_jitter: Duration, rng: &RefCell<SplitMix64>) -> Duration {
    if max_jitter.is_zero() {
        return Duration::ZERO;
    }

    const MAX_JITTER_NANOS: u128 = u64::MAX as u128;
    let upper = max_jitter.as_nanos().min(MAX_JITTER_NANOS) as u64;
    let random = rng.borrow_mut().next_bounded(upper);

    Duration::from_nanos(random)
}

#[cfg(target_has_atomic = "ptr")]
fn next_jitter_nonce() -> u64 {
    let counter = JITTER_NONCE_COUNTER.fetch_add(1, Ordering::Relaxed) as u64;

    #[cfg(feature = "std")]
    {
        use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(StdDuration::ZERO);
        counter ^ (now.as_nanos() as u64)
    }

    #[cfg(not(feature = "std"))]
    {
        counter
    }
}

#[cfg(not(target_has_atomic = "ptr"))]
fn next_jitter_nonce() -> u64 {
    1
}
