//! Every type that stands in a public signature or `where` bound must be
//! nameable from the crate root.
//!
//! A `pub` item in a private module compiles clean, satisfies `missing_docs`,
//! and still renders as a dead token on the crate's own entry-point pages —
//! the consumer who tries to write it down gets `not found in crate`. Nothing
//! in the build catches that, so these tests do: each one is a shape a
//! consumer writes when they outgrow a single inline `.retry()` call, and it
//! fails to compile if a name stops being reachable.

use core::time::Duration;

use relentless::clock::VirtualClock;
use relentless::stop::StopAfterAttempts;
use relentless::wait::WaitExponential;
use relentless::{
    AsyncRetry, AsyncRetryOp, AttemptHook, BeforeAttemptHook, Decide, DefaultClassifier, ExitHook,
    HookChain, Retry, RetryError, RetryOp, RetryState, StatelessOp,
};

const ARBITRARY_ATTEMPTS: u32 = 2;
const SUCCESS_VALUE: u32 = 7;

/// Sharing a retry shape by returning a preconfigured builder — the
/// alternative to `RetryPolicy` when the hooks matter. Spelling the return
/// type out is unavoidable, so every parameter in it must be nameable.
type Preconfigured<F> = Retry<
    StatelessOp<F>,
    DefaultClassifier,
    StopAfterAttempts,
    WaitExponential,
    VirtualClock,
    HookChain<(), fn(&RetryState)>,
    (),
    (),
>;

fn preconfigured<F>(op: F) -> Preconfigured<F>
where
    F: FnMut() -> Result<u32, &'static str>,
{
    fn note(_state: &RetryState) {}

    relentless::RetryExt::retry(op)
        .before_attempt(note as fn(&RetryState))
        .clock(VirtualClock::new())
}

#[test]
fn a_helper_can_return_a_preconfigured_builder() {
    assert_eq!(
        preconfigured(|| Ok(SUCCESS_VALUE)).call(),
        Ok(SUCCESS_VALUE)
    );
}

/// Code generic over *any* configured builder needs the bounds `.call()`
/// carries, which names the operation trait and all three hook traits.
#[test]
fn code_can_be_generic_over_a_configured_builder() {
    fn drive<F, C, S, W, BA, AA, OX, O>(
        builder: Retry<F, C, S, W, VirtualClock, BA, AA, OX>,
    ) -> Result<C::R, RetryError<C::A, O>>
    where
        F: RetryOp<Output = O>,
        C: Decide<O>,
        S: relentless::stop::Stop,
        W: relentless::wait::Wait,
        BA: BeforeAttemptHook,
        AA: AttemptHook<O>,
        OX: ExitHook<C::R, C::A, O>,
    {
        builder.call()
    }

    assert_eq!(
        drive(preconfigured(|| Ok(SUCCESS_VALUE))),
        Ok(SUCCESS_VALUE)
    );
}

/// The async operation trait is the same story on the async builder.
#[test]
fn code_can_be_generic_over_a_configured_async_builder() {
    fn accepts_async_op<F, C, S, W, BA, AA, OX, O>(
        builder: AsyncRetry<F, C, S, W, VirtualClock, BA, AA, OX>,
    ) -> AsyncRetry<F, C, S, W, VirtualClock, BA, AA, OX>
    where
        F: AsyncRetryOp<Output = O>,
    {
        builder.timeout(Duration::from_secs(1))
    }

    let _builder = accepts_async_op(
        relentless::retry_async(|_| async { Ok::<u32, &str>(SUCCESS_VALUE) })
            .stop(relentless::stop::attempts(ARBITRARY_ATTEMPTS))
            .clock(VirtualClock::new()),
    );
}
