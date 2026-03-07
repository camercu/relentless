use crate::compat::Duration;
use crate::state::RetryState;

use super::Wait;

#[cfg(all(feature = "jitter", target_has_atomic = "ptr"))]
use core::sync::atomic::{AtomicUsize, Ordering};

use rand::{Rng, SeedableRng, rngs::SmallRng};

/// Fixed seed used by jitter-enabled wait strategies.
const DEFAULT_JITTER_SEED: [u8; 32] = [0x5A; 32];

#[cfg(feature = "serde")]
const fn default_jitter_seed() -> [u8; 32] {
    DEFAULT_JITTER_SEED
}

/// Monotonic jitter nonce counter used to decorrelate independent policies.
#[cfg(target_has_atomic = "ptr")]
static JITTER_NONCE_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// A wrapper that adds uniformly distributed jitter in `[0, max_jitter]` to
/// the inner strategy output.
///
/// Enabled with the `jitter` feature and created by calling `.jitter(max)` on
/// any wait strategy.
///
/// Jitter uses a fast PRNG intended for retry backoff behavior, not for
/// cryptographic use.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "jitter")]
/// # {
/// use tenacious::{RetryState, Wait, WaitExt, wait};
/// use core::time::Duration;
///
/// let mut strategy = wait::fixed(Duration::from_millis(50))
///     .jitter(Duration::from_millis(10));
/// let state = RetryState {
///     attempt: 1,
///     elapsed: None,
///     next_delay: Duration::ZERO,
///     total_wait: Duration::ZERO,
/// };
///
/// let next = strategy.next_wait(&state);
/// assert!(next >= Duration::from_millis(50));
/// assert!(next <= Duration::from_millis(60));
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct WaitJitter<W> {
    inner: W,
    max_jitter: Duration,
    seed: [u8; 32],
    nonce: u64,
    rng: SmallRng,
}

impl<W> WaitJitter<W> {
    pub(super) fn new(inner: W, max_jitter: Duration) -> Self {
        let seed = DEFAULT_JITTER_SEED;
        Self {
            inner,
            max_jitter,
            seed,
            nonce: next_jitter_nonce(),
            rng: SmallRng::from_seed(seed),
        }
    }

    /// Sets an explicit PRNG seed for reproducible jitter when paired with
    /// [`with_nonce`](Self::with_nonce).
    #[must_use]
    pub fn with_seed(mut self, seed: [u8; 32]) -> Self {
        self.seed = seed;
        self.rng = SmallRng::from_seed(seed);
        self
    }

    /// Sets an explicit nonce offset used to decorrelate policy instances.
    #[must_use]
    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self
    }
}

impl<W: Wait> Wait for WaitJitter<W> {
    fn next_wait(&mut self, state: &RetryState) -> Duration {
        let base = self.inner.next_wait(state);
        let jitter = random_jitter_duration(self.max_jitter, &mut self.rng, self.nonce);
        base.saturating_add(jitter)
    }

    fn reset(&mut self) {
        self.inner.reset();
        self.nonce = self.nonce.wrapping_add(1);
        self.rng = SmallRng::from_seed(self.seed);
    }
}

#[cfg(feature = "serde")]
impl<W> serde::Serialize for WaitJitter<W>
where
    W: serde::Serialize,
{
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("WaitJitter", 4)?;
        state.serialize_field("inner", &self.inner)?;
        state.serialize_field("max_jitter", &self.max_jitter)?;
        state.serialize_field("seed", &self.seed)?;
        state.serialize_field("nonce", &self.nonce)?;
        state.end()
    }
}

#[cfg(feature = "serde")]
impl<'de, W> serde::Deserialize<'de> for WaitJitter<W>
where
    W: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct SerializedWaitJitter<W> {
            inner: W,
            max_jitter: Duration,
            #[serde(default = "default_jitter_seed")]
            seed: [u8; 32],
            #[serde(default = "next_jitter_nonce")]
            nonce: u64,
        }

        let serialized = SerializedWaitJitter::deserialize(deserializer)?;
        Ok(Self {
            inner: serialized.inner,
            max_jitter: serialized.max_jitter,
            seed: serialized.seed,
            nonce: serialized.nonce,
            rng: SmallRng::from_seed(serialized.seed),
        })
    }
}

/// Generates a random jitter duration in `[0, max_jitter]`.
fn random_jitter_duration(max_jitter: Duration, rng: &mut SmallRng, nonce: u64) -> Duration {
    if max_jitter.is_zero() {
        return Duration::ZERO;
    }

    const MAX_JITTER_NANOS: u128 = u64::MAX as u128;
    let upper = max_jitter.as_nanos().min(MAX_JITTER_NANOS) as u64;
    let random = rng.gen_range(0..=upper);
    let offset = nonce;
    let adjusted = if upper == u64::MAX {
        random.wrapping_add(offset)
    } else {
        let modulus = upper + 1;
        (random + (offset % modulus)) % modulus
    };

    Duration::from_nanos(adjusted)
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
