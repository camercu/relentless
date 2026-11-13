//! Every type that stands in a public signature or `where` bound must be
//! nameable from the crate root.
//!
//! A `pub` item in a private module compiles clean, satisfies `missing_docs`,
//! and still renders as a dead token on the crate's own entry-point pages —
//! the consumer who tries to write it down gets `not found in crate`. Nothing
//! in the build catches that, so these tests do: each one is a shape a
//! consumer writes when they outgrow a single inline `retry(..)` call, and it
//! fails to compile if a name stops being reachable.

mod common;

use common::{ARBITRARY_ATTEMPTS, ARBITRARY_DELAY, ARBITRARY_VALUE, ReadyClock, block_on};
use core::future::Future;

use relentless::clock::VirtualClock;
use relentless::stop::StopAfterAttempts;
use relentless::wait::{WaitExponential, WaitFixed};
use relentless::{
    Async, AsyncClock, AsyncRun, Blocking, Decide, DefaultClassifier, DropStats, Operation, Retry,
    RetryError, RetryPolicy, RetryState,
};

const SUCCESS_VALUE: u32 = 7;

/// Sharing a retry shape by returning a preconfigured builder, clock included.
/// Spelling the return type out is unavoidable, so every parameter in it must
/// be nameable.
type Preconfigured<F> =
    Retry<Blocking, F, DefaultClassifier, StopAfterAttempts, WaitExponential, VirtualClock>;

fn preconfigured<F>(op: F) -> Preconfigured<F>
where
    F: FnMut(RetryState) -> Result<u32, &'static str>,
{
    relentless::retry(op).clock(VirtualClock::new())
}

#[test]
fn a_helper_can_return_a_preconfigured_builder() {
    assert_eq!(
        preconfigured(|_| Ok(SUCCESS_VALUE)).call(),
        Ok(SUCCESS_VALUE)
    );
}

/// Code generic over *any* configured builder needs the bounds `.call()`
/// carries, which names the operation trait.
#[test]
fn code_can_be_generic_over_a_configured_builder() {
    fn drive<F, C, S, W, O>(
        builder: Retry<Blocking, F, C, S, W, VirtualClock>,
    ) -> Result<C::R, RetryError<C::A, O>>
    where
        F: Operation<Blocking, Output = O>,
        C: Decide<O>,
        S: relentless::stop::Stop,
        W: relentless::wait::Wait,
    {
        builder.call()
    }

    assert_eq!(
        drive(preconfigured(|_| Ok(SUCCESS_VALUE))),
        Ok(SUCCESS_VALUE)
    );
}

/// Awaiting a generic async builder binds the outcome only, as the blocking
/// one does: no bound names the future the operation returns.
#[test]
fn code_can_await_a_configured_async_builder_generically() {
    async fn drive<F, C, S, W, O>(
        builder: Retry<Async, F, C, S, W, ReadyClock>,
    ) -> Result<C::R, RetryError<C::A, O>>
    where
        F: Operation<Async, Output = O>,
        C: Decide<O>,
        S: relentless::stop::Stop,
        W: relentless::wait::Wait,
    {
        builder.timeout(ARBITRARY_DELAY).call().await
    }

    let builder =
        relentless::retry_async(|_| async { Ok::<u32, &str>(SUCCESS_VALUE) }).clock(ReadyClock);
    assert_eq!(block_on(drive(builder)), Ok(SUCCESS_VALUE));
}

/// Generic async code that hands the retry future to a multithreaded executor
/// requires `Send` through `Call: Send`, without naming the future type.
#[test]
fn generic_async_code_can_require_a_send_retry_future() {
    fn spawnable<F, C, S, W, Cl, O>(
        builder: Retry<Async, F, C, S, W, Cl>,
    ) -> impl Future<Output = Result<C::R, RetryError<C::A, O>>> + Send
    where
        F: Operation<Async, Output = O, Call: Send> + Send,
        C: Decide<O> + Send,
        S: relentless::stop::Stop + Send,
        W: relentless::wait::Wait + Send,
        Cl: AsyncClock<Wait: Send> + Send,
    {
        builder.call()
    }

    let builder =
        relentless::retry_async(|_| async { Ok::<u32, &str>(SUCCESS_VALUE) }).clock(ReadyClock);
    assert_eq!(block_on(spawnable(builder)), Ok(SUCCESS_VALUE));
}

/// An in-flight retry stored in a struct field, as a hand-written future or
/// a connection pool would hold it. The field restates only the bounds the
/// future type itself declares.
struct InFlight<F, C, S, W, Cl>
where
    F: Operation<Async>,
    Cl: AsyncClock,
{
    // Spelling the full type out is what this test checks.
    #[allow(clippy::type_complexity)]
    future: DropStats<AsyncRun<F, C, S, W, Cl>>,
}

#[test]
fn a_struct_field_can_hold_the_async_retry_future() {
    let in_flight = InFlight {
        future: relentless::retry_async(|_| async { Ok::<u32, &str>(SUCCESS_VALUE) })
            .clock(ReadyClock)
            .call(),
    };
    assert_eq!(block_on(in_flight.future), Ok(SUCCESS_VALUE));
}

/// The builder a policy hands back is nameable as `Retry<Blocking, ..>`.
#[test]
fn sync_retry_type_is_nameable_from_crate_root() {
    let policy = RetryPolicy::new().stop(relentless::stop::attempts(1));
    let retry = policy
        .retry(|_| Ok::<_, &str>(ARBITRARY_VALUE))
        .clock(VirtualClock::new());
    let typed: Retry<Blocking, _, _, _, _, _> = retry;
    assert_eq!(typed.call(), Ok(ARBITRARY_VALUE));
}

/// The builder a policy hands back to the async engine is nameable as
/// `Retry<Async, ..>`.
#[test]
fn async_retry_type_is_nameable_from_crate_root() {
    #[allow(clippy::type_complexity, clippy::needless_pass_by_value)]
    fn assert_nameable<F, C, S, W, Cl>(retry: Retry<Async, F, C, S, W, Cl>) {
        let _ = retry;
    }

    let clock = VirtualClock::new();
    let policy = RetryPolicy::new().stop(relentless::stop::attempts(1));
    assert_nameable(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(ARBITRARY_VALUE) })
            .clock(&clock),
    );
    let result: relentless::RetryResult<i32, &str> = block_on(
        policy
            .retry_async(|_| async { Ok::<i32, &str>(ARBITRARY_VALUE) })
            .clock(&clock)
            .call(),
    );
    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

/// The setters are written once for both modes, so a helper generic over the
/// mode configures a blocking and an async builder alike.
#[test]
fn a_helper_generic_over_the_mode_configures_either_builder() {
    fn bounded<M, F, C, S, W, Cl>(
        builder: Retry<M, F, C, S, W, Cl>,
    ) -> Retry<M, F, C, StopAfterAttempts, WaitFixed, Cl> {
        builder
            .stop(relentless::stop::attempts(ARBITRARY_ATTEMPTS))
            .wait(relentless::wait::fixed(ARBITRARY_DELAY))
    }

    let (_, blocking) = bounded(relentless::retry(|_| Err::<u32, &str>("down")))
        .clock(VirtualClock::new())
        .call_with_stats();
    let (_, asynchronous) = block_on(
        bounded(relentless::retry_async(|_| async {
            Err::<u32, &str>("down")
        }))
        .clock(ReadyClock)
        .call_with_stats(),
    );

    assert_eq!(
        (blocking.attempts, asynchronous.attempts),
        (ARBITRARY_ATTEMPTS, ARBITRARY_ATTEMPTS)
    );
}
