//! Every type that stands in a public signature or `where` bound must be
//! nameable from the crate root.
//!
//! A `pub` item in a private module compiles clean, satisfies `missing_docs`,
//! and still renders as a dead token on the crate's own entry-point pages —
//! the consumer who tries to write it down gets `not found in crate`. Nothing
//! in the build catches that, so these tests do: each one is a shape a
//! consumer writes when they outgrow a single inline `.retry()` call, and it
//! fails to compile if a name stops being reachable.

mod common;

use common::{ARBITRARY_DELAY, ARBITRARY_VALUE, ReadyClock, block_on};
use core::future::Future;

use relentless::clock::VirtualClock;
use relentless::stop::StopAfterAttempts;
use relentless::wait::WaitExponential;
use relentless::{
    Async, AsyncClock, AsyncRetry, AsyncRetryExt, AsyncRun, Blocking, Decide, DefaultClassifier,
    DropStats, Operation, Retry, RetryError, RetryExt, RetryPolicy, RetryStats, StatelessOp,
};

const SUCCESS_VALUE: u32 = 7;

/// Sharing a retry shape by returning a preconfigured builder, clock included.
/// Spelling the return type out is unavoidable, so every parameter in it must
/// be nameable.
type Preconfigured<F> =
    Retry<StatelessOp<F>, DefaultClassifier, StopAfterAttempts, WaitExponential, VirtualClock>;

fn preconfigured<F>(op: F) -> Preconfigured<F>
where
    F: FnMut() -> Result<u32, &'static str>,
{
    relentless::RetryExt::retry(op).clock(VirtualClock::new())
}

#[test]
fn a_helper_can_return_a_preconfigured_builder() {
    assert_eq!(
        preconfigured(|| Ok(SUCCESS_VALUE)).call(),
        Ok(SUCCESS_VALUE)
    );
}

/// Code generic over *any* configured builder needs the bounds `.call()`
/// carries, which names the operation trait.
#[test]
fn code_can_be_generic_over_a_configured_builder() {
    fn drive<F, C, S, W, O>(
        builder: Retry<F, C, S, W, VirtualClock>,
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
        drive(preconfigured(|| Ok(SUCCESS_VALUE))),
        Ok(SUCCESS_VALUE)
    );
}

/// Awaiting a generic async builder binds the outcome only, as the blocking
/// one does: no bound names the future the operation returns.
#[test]
fn code_can_await_a_configured_async_builder_generically() {
    async fn drive<F, C, S, W, O>(
        builder: AsyncRetry<F, C, S, W, ReadyClock>,
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
        builder: AsyncRetry<F, C, S, W, Cl>,
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

/// The builder a policy hands back is nameable as `Retry<..>`.
#[test]
fn sync_retry_type_is_nameable_from_crate_root() {
    let policy = RetryPolicy::new().stop(relentless::stop::attempts(1));
    let retry = policy
        .retry(|_| Ok::<_, &str>(ARBITRARY_VALUE))
        .clock(VirtualClock::new());
    let typed: Retry<_, _, _, _, _> = retry;
    assert_eq!(typed.call(), Ok(ARBITRARY_VALUE));
}

/// The builder a policy hands back to the async engine is nameable as
/// `AsyncRetry<..>`.
#[test]
fn async_retry_type_is_nameable_from_crate_root() {
    #[allow(clippy::type_complexity, clippy::needless_pass_by_value)]
    fn assert_nameable<F, C, S, W, Cl>(retry: AsyncRetry<F, C, S, W, Cl>) {
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

#[allow(clippy::unnecessary_wraps)]
fn do_work() -> Result<i32, &'static str> {
    Ok(ARBITRARY_VALUE)
}

fn do_async_work() -> core::future::Ready<Result<i32, &'static str>> {
    core::future::ready(Ok(ARBITRARY_VALUE))
}

#[test]
fn default_sync_retry_builder_alias_is_nameable() {
    type SyncWorkFn = fn() -> Result<i32, &'static str>;

    let typed = (do_work as SyncWorkFn).retry().clock(VirtualClock::new());
    assert_eq!(typed.call(), Ok(ARBITRARY_VALUE));
}

#[test]
fn default_sync_retry_builder_with_stats_alias_is_nameable() {
    type SyncWorkFn = fn() -> Result<i32, &'static str>;

    let typed = (do_work as SyncWorkFn)
        .retry()
        .clock(VirtualClock::new())
        .with_stats();
    let (result, stats): (relentless::RetryResult<i32, &str>, RetryStats) = typed.call();
    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(stats.attempts, 1);
}

#[test]
fn default_async_retry_builder_alias_is_nameable() {
    type AsyncWorkFn = fn() -> core::future::Ready<Result<i32, &'static str>>;

    let clock = VirtualClock::new();
    let typed = (do_async_work as AsyncWorkFn).retry_async();
    let result: relentless::RetryResult<i32, &str> = block_on(typed.clock(&clock).call());
    assert_eq!(result, Ok(ARBITRARY_VALUE));
}

#[test]
fn default_async_retry_builder_with_stats_alias_is_nameable() {
    type AsyncWorkFn = fn() -> core::future::Ready<Result<i32, &'static str>>;

    let clock = VirtualClock::new();
    let typed = (do_async_work as AsyncWorkFn)
        .retry_async()
        .clock(&clock)
        .with_stats();
    let (result, stats): (relentless::RetryResult<i32, &str>, RetryStats) = block_on(typed.call());
    assert_eq!(result, Ok(ARBITRARY_VALUE));
    assert_eq!(stats.attempts, 1);
}
