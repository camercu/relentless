use core::fmt;

use crate::compat::Duration;
use crate::state::RetryState;

use super::Wait;

#[cfg(any(target_has_atomic = "ptr", target_has_atomic = "64"))]
use core::sync::atomic::Ordering;

#[cfg(target_has_atomic = "ptr")]
use core::sync::atomic::AtomicUsize;

#[cfg(target_has_atomic = "64")]
use core::sync::atomic::AtomicU64;

#[cfg(not(target_has_atomic = "64"))]
use core::cell::Cell;

const DEFAULT_JITTER_SEED: u64 = 0x5A5A_5A5A_5A5A_5A5A;

/// SplitMix64 state increment (the "golden gamma").
const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

/// Monotonic jitter nonce counter used to decorrelate independent policies.
#[cfg(target_has_atomic = "ptr")]
static JITTER_NONCE_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// Fast, non-cryptographic PRNG for jitter decorrelation.
///
/// `SplitMix64` has excellent avalanche properties and is widely used for
/// seeding other PRNGs (e.g., in Java's `SplittableRandom`). It is more
/// than sufficient for retry jitter where the goal is decorrelation, not
/// cryptographic security.
///
/// The state advances by a fixed increment (`GAMMA`) per draw, so on targets
/// with 64-bit atomics it is stored in an `AtomicU64` and advanced with a
/// single `fetch_add` — lock-free, `Sync`, and sequentially identical to the
/// single-threaded algorithm. Concurrent callers interleave one stream, each
/// drawing a distinct state. Targets without 64-bit atomics fall back to
/// `Cell`, making jittered strategies `!Sync` there.
struct SplitMix64 {
    #[cfg(target_has_atomic = "64")]
    state: AtomicU64,
    #[cfg(not(target_has_atomic = "64"))]
    state: Cell<u64>,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed.into() }
    }

    /// Advances the state by `GAMMA` and returns the new value.
    #[cfg(target_has_atomic = "64")]
    fn advance(&self) -> u64 {
        self.state
            .fetch_add(GAMMA, Ordering::Relaxed)
            .wrapping_add(GAMMA)
    }

    /// Advances the state by `GAMMA` and returns the new value.
    #[cfg(not(target_has_atomic = "64"))]
    fn advance(&self) -> u64 {
        let next = self.state.get().wrapping_add(GAMMA);
        self.state.set(next);
        next
    }

    fn next_u64(&self) -> u64 {
        let mut z = self.advance();
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Returns a value in `[0, max]`.
    fn next_bounded(&self, max: u64) -> u64 {
        if max == u64::MAX {
            return self.next_u64();
        }
        // Simple modulo — bias is negligible for jitter ranges (nanoseconds).
        let range = max + 1;
        self.next_u64() % range
    }

    #[cfg(target_has_atomic = "64")]
    fn current_state(&self) -> u64 {
        self.state.load(Ordering::Relaxed)
    }

    #[cfg(not(target_has_atomic = "64"))]
    fn current_state(&self) -> u64 {
        self.state.get()
    }
}

impl Clone for SplitMix64 {
    fn clone(&self) -> Self {
        Self::new(self.current_state())
    }
}

impl fmt::Debug for SplitMix64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitMix64").finish_non_exhaustive()
    }
}

fn seeded_rng(seed: u64, nonce: u64) -> SplitMix64 {
    SplitMix64::new(seed ^ nonce)
}

/// Derives the instance nonce from an explicit seed so that `with_seed` alone
/// fully pins the jitter sequence.
fn derive_nonce(seed: u64) -> u64 {
    SplitMix64::new(seed).next_u64()
}

/// Selects which jitter formula `Jittered<W>` applies to the base duration.
#[derive(Debug, Clone, Copy)]
enum JitterKind {
    /// `base + random(0, max_jitter)`
    Additive(Duration),
    /// `random(0, base)`
    Full,
    /// `base/2 + random(0, base/2)`
    Equal,
    /// `random(base, previous_delay * 3)` — AWS decorrelated jitter. The floor
    /// is the inner strategy's output; the upper bound feeds back the previous
    /// (post-clamp) delay from [`RetryState::previous_delay`].
    Decorrelated,
}

/// Applies jitter to an inner wait strategy's output.
///
/// Created by calling [`.jitter(max)`](Wait::jitter),
/// [`.full_jitter()`](Wait::full_jitter), or
/// [`.equal_jitter()`](Wait::equal_jitter) on any wait strategy.
///
/// Jitter uses a fast PRNG intended for retry backoff behavior, not for
/// cryptographic use. Cloning a `Jittered` strategy produces a decorrelated
/// copy — the clone will generate a different jitter sequence.
///
/// The default PRNG seed is fixed (there is no entropy source in `no_std`), so
/// without [`with_seed`](Self::with_seed) the jitter sequence is **deterministic
/// across process restarts** (instances within a run are still decorrelated by a
/// per-instance nonce). Call `with_seed` with a runtime-sourced value if you
/// need run-to-run variation.
///
/// The PRNG state is a single atomic on targets with 64-bit atomics, so a
/// jittered strategy — and any policy containing one — is `Send + Sync` and
/// shareable across threads by reference; concurrent retry loops interleave
/// draws from one stream. Targets without 64-bit atomics fall back to a
/// `Cell`-based PRNG, which is `!Sync`.
///
/// # Examples
///
/// ```
/// use relentless::{RetryState, Wait, wait};
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
/// use relentless::{RetryState, Wait, wait};
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
/// use relentless::{RetryState, Wait, wait};
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
    rng: SplitMix64,
}

impl<W> Jittered<W> {
    fn new(inner: W, kind: JitterKind) -> Self {
        let nonce = next_jitter_nonce();
        Self {
            inner,
            kind,
            seed: DEFAULT_JITTER_SEED,
            nonce,
            rng: seeded_rng(DEFAULT_JITTER_SEED, nonce),
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

    pub(super) fn decorrelated(inner: W) -> Self {
        Self::new(inner, JitterKind::Decorrelated)
    }

    /// Sets an explicit PRNG seed for reproducible jitter sequences.
    ///
    /// The seed alone fully pins the sequence: the instance-decorrelation
    /// nonce is re-derived from the seed, replacing any prior nonce (default
    /// or explicit). Two instances given the same seed produce identical
    /// sequences. To decorrelate same-seed instances, call
    /// [`with_nonce`](Self::with_nonce) *after* `with_seed`.
    ///
    /// Cloning still decorrelates: a clone receives a fresh nonce and
    /// diverges from the seeded original.
    #[must_use]
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self.nonce = derive_nonce(seed);
        self.rng = seeded_rng(seed, self.nonce);
        self
    }

    /// Overrides the instance-decorrelation nonce.
    ///
    /// By default, each `Jittered` instance (including clones) receives a
    /// unique nonce so independent retry loops produce different jitter
    /// sequences. Set an explicit nonce to decorrelate instances that share
    /// a seed while keeping each stream deterministic. Call it after
    /// [`with_seed`](Self::with_seed), which resets the nonce.
    #[must_use]
    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self.rng = seeded_rng(self.seed, nonce);
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
            rng: seeded_rng(self.seed, nonce),
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
            JitterKind::Decorrelated => {
                // On the first attempt there is no previous delay, so the upper
                // bound falls back to the floor (`base`) before tripling,
                // yielding `random(base, base * 3)`.
                let lower = base;
                let upper = state.previous_delay.unwrap_or(lower).saturating_mul(3);
                random_duration_in(lower, upper, &self.rng)
            }
        }
    }
}

/// Produces a decorrelated jitter strategy: each delay is random between `base`
/// and three times the previous (post-clamp) delay.
///
/// This is the "Decorrelated Jitter" strategy from the [AWS Architecture Blog](https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/).
/// The feedback — the previous delay — is read from
/// [`RetryState::previous_delay`](crate::RetryState::previous_delay), so the
/// strategy carries no per-attempt state of its own (only its PRNG) and is
/// freely shareable across reused policies. On the first attempt there is no
/// previous delay, so the result is `random(base, base * 3)`.
///
/// Compose with [`.cap(max)`](Wait::cap) to bound the maximum delay, and use
/// [`Jittered::with_seed`]/[`Jittered::with_nonce`] for deterministic output.
///
/// To build a *custom* feedback strategy, implement [`Wait`] and read
/// `state.previous_delay` directly; bring your own PRNG (e.g. a small
/// `SplitMix64`) via interior mutability if you need randomness.
///
/// # Examples
///
/// ```
/// use relentless::{RetryState, Wait, wait};
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
#[must_use]
pub fn decorrelated_jitter(base: Duration) -> Jittered<super::WaitFixed> {
    Jittered::decorrelated(super::fixed(base))
}

/// Generates a random jitter duration in `[0, max_jitter]`.
fn random_jitter_duration(max_jitter: Duration, rng: &SplitMix64) -> Duration {
    random_duration_in(Duration::ZERO, max_jitter, rng)
}

/// Returns a uniformly random `Duration` in `[lower, upper]`, or `lower` when
/// `upper <= lower`. The nanosecond range is clamped to `u64::MAX`.
fn random_duration_in(lower: Duration, upper: Duration, rng: &SplitMix64) -> Duration {
    const MAX_RANGE_NANOS: u128 = u64::MAX as u128;
    let range_nanos = upper.as_nanos().saturating_sub(lower.as_nanos());
    if range_nanos == 0 {
        return lower;
    }
    let max_nanos = range_nanos.min(MAX_RANGE_NANOS) as u64;
    let random = rng.next_bounded(max_nanos);
    lower.saturating_add(Duration::from_nanos(random))
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
