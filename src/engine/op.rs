//! The operation abstraction shared by the blocking and async builders.
//!
//! A builder stores its operation behind [`Operation`]: `retry(|state| ...)`
//! takes an `FnMut(RetryState) -> O`, and `retry_async` its async twin. The op-anchored classifier methods bound
//! `F: Operation<M, Output = O>`, which — through the blanket impls — still
//! pins `O` to the closure's output, so inline classifiers keep inferring with
//! no annotations.

use crate::state::RetryState;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

/// Mode marker: the execution blocks the calling thread while it waits.
///
/// Chosen by the entry point: [`retry`](crate::retry) and
/// [`RetryPolicy::retry`](crate::RetryPolicy::retry) start a `Blocking` execution.
/// Never constructed; it only selects which `Operation` and clock a builder
/// needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Blocking {}

/// Mode marker: the execution is a future that awaits its waits.
///
/// Chosen by the entry point: [`retry_async`](crate::retry_async) and
/// [`RetryPolicy::retry_async`](crate::RetryPolicy::retry_async) start an
/// `Async` execution. Never constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Async {}

// The items here are `pub` because public signatures mention them, and in a
// private module so no other crate can name them: that is what seals
// `Operation` and `Attempt`.
#[allow(unreachable_pub)]
mod sealed {
    use super::{Async, Attempt, Blocking};
    use crate::state::RetryState;
    use core::future::Future;

    /// Restricts [`Attempt`] to its two blanket impls.
    pub trait Sealed<M> {}
    impl<T> Sealed<Blocking> for T {}
    impl<F: Future> Sealed<Async> for F {}

    /// Restricts [`Operation`](super::Operation) to closures: the entry points
    /// accept nothing else.
    pub trait SealedOp<M> {}
    impl<M, R: Attempt<M>, F: FnMut(RetryState) -> R> SealedOp<M> for F {}

    /// Names the token `poll_attempt` takes in each mode.
    pub trait Mode {
        type Token;
    }
    // Uninhabited: no blocking attempt can ever be polled.
    impl Mode for Blocking {
        type Token = core::convert::Infallible;
    }
    // Polling an async attempt is polling its future, so anyone may do it.
    impl Mode for Async {
        type Token = ();
    }
}

/// What one call of an [`Operation`] yields in mode `M`: the outcome itself in
/// [`Blocking`] mode, or a future of it in [`Async`] mode.
///
/// Sealed: every type implements `Attempt<Blocking>` with itself as `Output`,
/// and every future implements `Attempt<Async>` with the future's output. It
/// appears in bounds only as the item bound on [`Operation::Call`]; consumer
/// code never names it.
///
/// No other crate can add a mode or an implementation:
///
/// ```compile_fail
/// use core::pin::Pin;
/// use core::task::{Context, Poll};
/// use relentless::{Async, Attempt};
///
/// struct NotAFuture;
///
/// impl Attempt<Async> for NotAFuture {
///     type Output = ();
///     fn poll_attempt(self: Pin<&mut Self>, _: &mut Context<'_>, _: ()) -> Poll<()> {
///         Poll::Ready(())
///     }
/// }
/// ```
pub trait Attempt<M>: sealed::Sealed<M> {
    /// The outcome the attempt produces.
    type Output;

    // The `M: Mode` bound sits on the method, not the trait, so code generic
    // over the mode can name `Attempt<M>` (and `Operation<M>`) without it.
    #[doc(hidden)]
    fn poll_attempt(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        token: <M as sealed::Mode>::Token,
    ) -> Poll<Self::Output>
    where
        M: sealed::Mode;
}

impl<T> Attempt<Blocking> for T {
    type Output = T;

    fn poll_attempt(
        self: Pin<&mut T>,
        _cx: &mut Context<'_>,
        token: core::convert::Infallible,
    ) -> Poll<T> {
        match token {}
    }
}

impl<F: Future> Attempt<Async> for F {
    type Output = F::Output;

    fn poll_attempt(self: Pin<&mut F>, cx: &mut Context<'_>, _token: ()) -> Poll<F::Output> {
        self.poll(cx)
    }
}

/// The operation a builder retries, in mode `M` ([`Blocking`] or [`Async`]).
///
/// Implemented for `FnMut(RetryState) -> R` closures; sealed, so those are the
/// only implementors. Name it in the bounds of code
/// generic over a configured builder:
///
/// - `F: Operation<Blocking, Output = O>` to run a blocking builder;
/// - `F: Operation<Async, Output = O>` to await an async one.
///
/// To require a `Send` retry future, add `Call: Send` and bound every other
/// builder parameter `Send` as well — the operation itself, the classifier,
/// the stop and wait strategies, and the clock with its wait
/// future: `F: Operation<Async, Output = O, Call: Send> + Send,
/// C: Decide<O> + Send, Cl: AsyncClock<Wait: Send> + Send`, and so on.
///
/// A struct field that stores the async retry future repeats the future's own
/// bounds, `F: Operation<Async>` and `Cl: AsyncClock`.
///
/// A closure that returns a future implements both modes, which is why the
/// entry point chooses the mode rather than inferring it.
///
/// # Examples
///
/// A helper that finishes any configured blocking builder:
///
/// ```
/// use relentless::clock::VirtualClock;
/// use relentless::stop::Stop;
/// use relentless::wait::Wait;
/// use relentless::{Blocking, Decide, Operation, Retry, RetryError, SyncClock, retry};
///
/// fn finish<F, C, S, W, Cl, O>(
///     builder: Retry<Blocking, F, C, S, W, Cl>,
/// ) -> Result<C::R, RetryError<C::A, O>>
/// where
///     F: Operation<Blocking, Output = O>,
///     C: Decide<O>,
///     S: Stop,
///     W: Wait,
///     Cl: SyncClock,
/// {
///     builder.call()
/// }
///
/// let builder = retry(|_| Ok::<u32, &str>(1)).clock(VirtualClock::new());
/// assert_eq!(finish(builder), Ok(1));
/// ```
///
/// Every entry point takes a closure, so no other implementation could run;
/// the trait is sealed to say so at compile time:
///
/// ```compile_fail
/// use relentless::{Blocking, Operation, RetryState};
///
/// struct Ping;
///
/// impl Operation<Blocking> for Ping {
///     type Output = Result<(), ()>;
///     type Call = Result<(), ()>;
///     fn call_op(&mut self, _state: RetryState) -> Self::Call {
///         Ok(())
///     }
/// }
/// ```
pub trait Operation<M>: sealed::SealedOp<M> {
    /// The outcome an attempt produces.
    ///
    /// An associated type, so `F: Operation<M>` determines the outcome as
    /// `FnMut(..) -> O` does; classifier closures infer their parameter from it.
    type Output;
    /// What running the operation once returns: the outcome itself in
    /// `Blocking` mode, a future of it in `Async` mode.
    type Call: Attempt<M, Output = Self::Output>;
    /// Runs one attempt, given the current retry state.
    fn call_op(&mut self, state: RetryState) -> Self::Call;
}

impl<M, R, F> Operation<M> for F
where
    F: FnMut(RetryState) -> R,
    R: Attempt<M>,
{
    type Output = R::Output;
    type Call = R;
    fn call_op(&mut self, state: RetryState) -> R {
        (self)(state)
    }
}

/// Turns a blocking call result into its outcome.
///
/// An identity function the compiler needs: the blocking driver holds an
/// `F::Call` and must hand on an `F::Output`. With `A` unbounded here the
/// blanket impl normalizes the return type to `A`; at the call site the item
/// bound on [`Operation::Call`] normalizes it to `F::Output`. Assigning the
/// call result directly is rejected as a type mismatch.
pub(crate) fn ready_outcome<A>(attempt: A) -> <A as Attempt<Blocking>>::Output {
    attempt
}
