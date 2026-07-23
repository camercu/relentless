//! [`RetryPolicy`]: reusable retry configuration.
//!
//! A policy captures the stop strategy, wait strategy, and classifier once, then
//! is reused across operations via [`RetryPolicy::retry`] and
//! [`RetryPolicy::retry_async`], which borrow the policy by `&self`. An optional
//! wall-clock [`timeout`](RetryPolicy::timeout) is also captured and seeds each
//! built retry. Remaining per-call concerns (hooks, clock, stats) live on the
//! returned builder, not on the policy.

use crate::clock::SystemClock;
#[cfg(feature = "alloc")]
use crate::compat::Box;
use crate::compat::Duration;
use crate::decision::{ClosureClassifier, DefaultClassifier, Until, When};
use crate::engine::{AsyncRetry, Retry};
use crate::state::RetryState;
#[cfg(feature = "alloc")]
use crate::stop::Stop;
use crate::stop::{self, StopAfterAttempts};
#[cfg(feature = "alloc")]
use crate::wait::Wait;
use crate::wait::{self, WaitExponential};
use core::future::Future;

const DEFAULT_MAX_ATTEMPTS: u32 = 3;
const DEFAULT_INITIAL_WAIT: Duration = Duration::from_millis(100);

/// Reusable retry configuration: a stop strategy, a wait strategy, and a
/// classifier.
///
/// Construct with [`RetryPolicy::new`] (safe defaults: `attempts(3)`,
/// `exponential(100ms)`, retry on any `Err`), customize with `.stop`/`.wait`/
/// `.when`/`.until`/`.decide`, then reuse across operations:
///
/// ```
/// use relentless::clock::VirtualClock;
/// use relentless::{RetryPolicy, stop, wait};
/// use core::time::Duration;
///
/// let policy = RetryPolicy::new()
///     .stop(stop::attempts(3))
///     .wait(wait::fixed(Duration::from_millis(5)));
///
/// let a = policy.retry(|_| Ok::<_, &str>("a")).clock(VirtualClock::new()).call();
/// let b = policy.retry(|_| Ok::<_, &str>("b")).clock(VirtualClock::new()).call();
/// assert_eq!(a.unwrap(), "a");
/// assert_eq!(b.unwrap(), "b");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy<S = StopAfterAttempts, W = WaitExponential, C = DefaultClassifier> {
    stop: S,
    wait: W,
    classifier: C,
    timeout: Option<Duration>,
}

impl RetryPolicy<StopAfterAttempts, WaitExponential, DefaultClassifier> {
    /// Creates a policy with safe defaults: `attempts(3)`, `exponential(100ms)`,
    /// and the default classifier (retry on any `Err`).
    #[must_use]
    pub fn new() -> Self {
        RetryPolicy {
            stop: stop::attempts(DEFAULT_MAX_ATTEMPTS),
            wait: wait::exponential(DEFAULT_INITIAL_WAIT),
            classifier: DefaultClassifier,
            timeout: None,
        }
    }
}

impl Default for RetryPolicy<StopAfterAttempts, WaitExponential, DefaultClassifier> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, W, C> RetryPolicy<S, W, C> {
    /// Sets the stop strategy.
    #[must_use]
    pub fn stop<NewStop>(self, stop: NewStop) -> RetryPolicy<NewStop, W, C> {
        RetryPolicy {
            stop,
            wait: self.wait,
            classifier: self.classifier,
            timeout: self.timeout,
        }
    }

    /// Sets the wait strategy used between attempts.
    #[must_use]
    pub fn wait<NewWait>(self, wait: NewWait) -> RetryPolicy<S, NewWait, C> {
        RetryPolicy {
            stop: self.stop,
            wait,
            classifier: self.classifier,
            timeout: self.timeout,
        }
    }

    /// Sets a wall-clock deadline for the whole retry execution, seeding every
    /// [`retry`](Self::retry)/[`retry_async`](Self::retry_async) built from this
    /// policy. A builder [`.timeout()`](crate::Retry::timeout) replaces it for
    /// that call (it is not combined). See [`Retry::timeout`](crate::Retry::timeout)
    /// for the deadline's semantics.
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    /// Retries while `predicate` wants to; otherwise accepts (an `Err` aborts
    /// with the bare error). Replaces the classifier slot.
    #[must_use]
    pub fn when<P>(self, predicate: P) -> RetryPolicy<S, W, When<P>> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            classifier: When::new(predicate),
            timeout: self.timeout,
        }
    }

    /// Retries *until* `predicate` is satisfied, then accepts. Replaces the
    /// classifier slot; natural for polling (`.until(ok(is_ready))`).
    #[must_use]
    pub fn until<P>(self, predicate: P) -> RetryPolicy<S, W, Until<P>> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            classifier: Until::new(predicate),
            timeout: self.timeout,
        }
    }

    /// Installs a classifier closure, replacing the classifier slot.
    ///
    /// Policy-first construction has no operation to anchor inference on, so the
    /// closure's parameter needs a type annotation (unlike the op-first
    /// [`Retry::decide`](crate::Retry::decide)).
    #[must_use]
    pub fn decide<NewC>(self, classifier: NewC) -> RetryPolicy<S, W, ClosureClassifier<NewC>> {
        RetryPolicy {
            stop: self.stop,
            wait: self.wait,
            classifier: ClosureClassifier(classifier),
            timeout: self.timeout,
        }
    }

    /// Erases the stop and wait strategies behind `Send` trait objects, leaving
    /// the classifier intact.
    ///
    /// Produces one nameable type,
    /// `RetryPolicy<Box<dyn Stop + Send>, Box<dyn Wait + Send>, C>`, suitable for
    /// a struct field. The classifier is deliberately not erased: the default
    /// classifier works for every outcome type, so a default-classifier boxed
    /// policy stays reusable across operations with different `Ok`/`Err` types.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn boxed(
        self,
    ) -> RetryPolicy<Box<dyn Stop + Send + 'static>, Box<dyn Wait + Send + 'static>, C>
    where
        S: Stop + Send + 'static,
        W: Wait + Send + 'static,
    {
        RetryPolicy {
            stop: Box::new(self.stop),
            wait: Box::new(self.wait),
            classifier: self.classifier,
            timeout: self.timeout,
        }
    }

    /// Like [`boxed`](Self::boxed) but without `Send` bounds, for policies that
    /// do not cross thread boundaries.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn boxed_local(self) -> RetryPolicy<Box<dyn Stop + 'static>, Box<dyn Wait + 'static>, C>
    where
        S: Stop + 'static,
        W: Wait + 'static,
    {
        RetryPolicy {
            stop: Box::new(self.stop),
            wait: Box::new(self.wait),
            classifier: self.classifier,
            timeout: self.timeout,
        }
    }

    /// Creates a synchronous retry for `op`, borrowing this policy's parts so it
    /// stays reusable.
    #[allow(clippy::type_complexity)]
    pub fn retry<F, O>(&self, op: F) -> Retry<F, &C, &S, &W, SystemClock, (), (), ()>
    where
        F: FnMut(RetryState) -> O,
    {
        Retry::from_parts(op, &self.classifier, &self.stop, &self.wait, self.timeout)
    }

    /// Creates an asynchronous retry for `op`, borrowing this policy's parts.
    #[allow(clippy::type_complexity)]
    pub fn retry_async<F, Fut, O>(
        &self,
        op: F,
    ) -> AsyncRetry<F, &C, &S, &W, SystemClock, (), (), ()>
    where
        F: FnMut(RetryState) -> Fut,
        Fut: Future<Output = O>,
    {
        AsyncRetry::from_parts(op, &self.classifier, &self.stop, &self.wait, self.timeout)
    }
}
