//! The outcome-classification layer.
//!
//! The engine asks a *classifier* to sort each outcome into a three-way
//! [`Verdict`]: return it to the caller, retry it, or abort with a projected
//! payload. The retry decision is therefore independent of `Result<T, E>`
//! semantics. A sought-after `Err`, a non-`Result` poll enum, or a search state
//! can each drive the loop directly. `.when` (a test on the error) and `.until`
//! (a test on the `Ok` value) are shortcuts that build a classifier for the
//! common `Result` cases. [`some_or_abort`] is a ready-made classifier for a
//! `Result<Option<T>, E>` poll.
//!
//! This module is the vocabulary; the engine that consumes it lives in
//! [`crate::engine`].

mod classifier;
mod outcome;
mod verdict;

pub use classifier::{ClosureClassifier, Decide, DefaultClassifier, Until, When, some_or_abort};
pub use outcome::Outcome;
pub use verdict::{Decision, IntoDecision, Verdict};

// `pub` items in a crate-visible module: public bounds mention them, and no
// other crate can name them, which seals `Decide` and `IntoDecision`.
#[allow(unreachable_pub)]
pub(crate) mod sealed {
    pub trait Sealed {}

    /// Restricts [`Decide`](super::Decide) to the crate's own classifiers.
    pub trait SealedDecide {}
}
