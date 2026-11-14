# Changelog

All notable changes to this project are documented in this file. Entries
from 0.19.0 back were written by semantic-release; later ones by
cocogitto.

- - -
## [0.19.0](https://github.com/camercu/relentless/compare/v0.18.5...v0.19.0) (2026-11-14)

### ⚠ BREAKING CHANGES

* **clock:** on wasm32-unknown-unknown, TokioClock::new, Default, Clock
and AsyncClock are gone. Such code panicked on first use; use GlooClock in
the browser.
* **clock:** on wasm32-unknown-unknown, FuturesTimerClock::new,
Default, Clock and AsyncClock are gone. Such code panicked on first use;
use GlooClock in the browser.
* **engine:** AsyncRetry is removed. Code that names the builder
type gains the mode as its first parameter:

| Before                         | After                                |
|--------------------------------|--------------------------------------|
| Retry<F, C, S, W, Cl>          | Retry<Blocking, F, C, S, W, Cl>      |
| AsyncRetry<F, C, S, W, Cl>     | Retry<Async, F, C, S, W, Cl>         |

Chained call sites are unaffected.
* **engine:** Retry::with_stats, AsyncRetry::with_stats,
RetryWithStats and AsyncRetryWithStats are removed. Replace
.with_stats().call() with .call_with_stats(), and
.with_stats().call().await with .call_with_stats().await.
* **engine:** RetryExt, AsyncRetryExt and StatelessOp are removed,
along with their prelude entries. Replace (|| op()).retry() with
retry(|_| op()) and f.retry() with retry(|_| f()). Async forms use
retry_async the same way.
* **classifier:** the predicate module is removed and .when/.until take
plain closures. Migrate:
- .when(predicate::error(f)) -> .when(f)
- .until(predicate::ok(f)) -> .until(f)
- .when(predicate::any_error()) -> drop it (the default) or .when(|_| true)
- .when(predicate::ok(f)), .until(predicate::error(f)), and any
  predicate::result(f) -> .decide with a match returning Verdict
Policy-first .when/.until closures need a parameter type, as
policy-first .decide does.
* **stop,wait:** StopAny::new, StopAll::new, WaitCombine::new and
WaitChain::new are no longer public. Write a.or(b) or a | b, a.and(b)
or a & b, a.add(b) or a + b, and a.chain(b, after) instead.
* **engine:** the hook setters and types are gone, and builders
take five type parameters (F, C, S, W, Cl) instead of eight. Move
before_attempt code to the top of the operation. Move after_attempt
code into the operation, after the attempt and before returning its
outcome. Replace on_exit with a match on the value .call() returns,
plus .with_stats() for the attempt count and stop reason. To log each
delay before the sleep, wrap the wait strategy in a type implementing
Wait. Drop BA, AA and OX, and their hook-trait bounds, from any type
or helper that names a builder.
* **clock:** on wasm32-unknown-unknown, a sync retry that relied on
the default SystemClock no longer compiles. It panicked at runtime
before. Pass a clock with .clock(...), e.g. VirtualClock for a loop
that needs no timer.
* **clock:** VirtualClock::waits() is removed. Wrap the clock to
keep its waits: let clock = Recording::new(VirtualClock::new()), pass
&clock as before, and read clock.waits(). Call .advance() through
clock.inner(). Recording needs the alloc feature, as waits() did.
* **decision:** Decide is sealed. It was exported doc(hidden) in 0.18,
and an impl outside the crate compiled but could never be installed, so
no working code relied on one. Delete any such impl and classify with
.decide, .when or .until instead.
* **engine:** RetryOp and AsyncRetryOp are removed. Bound
operations as Operation<Blocking, Output = O> or Operation<Async,
Output = O>; Operation is sealed and cannot be implemented outside
the crate. Generic async code that needs a Send retry future adds
Call: Send, as in Operation<Async, Output = O, Call: Send> + Send,
and bounds the other builder parameters Send. AsyncRun drops its Fut
and O parameters and requires F: Operation<Async>, so a struct field
storing it repeats that bound.

### Features

* **classifier:** take plain closures in .when and .until ([e6df0f1](https://github.com/camercu/relentless/commit/e6df0f13825d679d6a485dc9d5b76efad2bc1207))
* **clock:** add Recording to record the waits any clock performs ([078d1b4](https://github.com/camercu/relentless/commit/078d1b4af4249a753de7e317e4b9ae6e5691bc52))
* **clock:** make VirtualClock a stateless no-sleep clock ([8f2a95b](https://github.com/camercu/relentless/commit/8f2a95b91714630dd0bd0b966bbe4f980f7029ee))
* **engine:** remove the closure extension traits ([5c17593](https://github.com/camercu/relentless/commit/5c1759391cab89e177f3a8c209b8367d0203cf79))
* **engine:** remove the observer hooks ([b61a4b2](https://github.com/camercu/relentless/commit/b61a4b2ebb3defc8758226b85a16aa8c8a845534))
* **engine:** replace with_stats wrappers with call_with_stats ([e528031](https://github.com/camercu/relentless/commit/e528031e9ce6644871f3aeda85250efacdf747d7))
* **stop,wait:** build leaf strategies in const items ([1219c45](https://github.com/camercu/relentless/commit/1219c45e30693070881fdda5bd49a8f31bd82ff8))

### Bug Fixes

* **async:** mark AsyncRun and DropStats must_use ([d473c97](https://github.com/camercu/relentless/commit/d473c977e09ef0036997b6031f7412f081991942))
* **clock:** refuse FuturesTimerClock on wasm32-unknown-unknown ([4e17e5a](https://github.com/camercu/relentless/commit/4e17e5a57135e8991e5d66779126b67b267f1b76))
* **clock:** refuse SystemClock as a clock on wasm32-unknown-unknown ([4685a59](https://github.com/camercu/relentless/commit/4685a5900e03762321840ea01bf3d9c8868ccd55))
* **clock:** refuse TokioClock on wasm32-unknown-unknown ([07409cc](https://github.com/camercu/relentless/commit/07409cce1c30b4071ac53306cb743d43ee0c9cdf))
* **decision:** seal Decide as the docs already claimed ([e7e25e9](https://github.com/camercu/relentless/commit/e7e25e997793bec4e53e8b5e04a8401713bab162))
* **error:** keep Display and Error for any abort type on Result outcomes ([d24aab0](https://github.com/camercu/relentless/commit/d24aab09143dddc196ca4fffb737053316f7c7b0))
* **wait:** stop reading the wall clock for jitter on wasm32 ([a4bbdff](https://github.com/camercu/relentless/commit/a4bbdff66f2b7943f6a65ce4dc81156acd21bc4d))

### Code Refactoring

* **engine:** merge Retry and AsyncRetry into one builder ([c952efc](https://github.com/camercu/relentless/commit/c952efca0cf114f9cddbb3df4fc215533361ce3b))
* **engine:** replace RetryOp and AsyncRetryOp with Operation<M> ([9aa429c](https://github.com/camercu/relentless/commit/9aa429c9af445ac7de8f3024772cf67b308e7c94))
* **stop,wait:** hide the combinator constructors ([37bdc5e](https://github.com/camercu/relentless/commit/37bdc5e77ebb060d2a199165657210fcd3b13a5e))

## [0.18.5](https://github.com/camercu/relentless/compare/v0.18.4...v0.18.5) (2026-11-08)

### Bug Fixes

* **just:** keep git hook variables away from recipe tools ([e1e72c4](https://github.com/camercu/relentless/commit/e1e72c4c0f58c6a6cc269bdf570c5b9dd413bc32))
* **just:** lint only the Markdown files git sees ([904fd4f](https://github.com/camercu/relentless/commit/904fd4fea24efa74dbc262452a30f6858d23c05c))

## [0.18.4](https://github.com/camercu/relentless/compare/v0.18.3...v0.18.4) (2026-10-25)

### Bug Fixes

* **deps:** update rustls-webpki and webpki-root-certs to fix RUSTSEC-2026-0382 and RUSTSEC-2026-0383 ([856bffe](https://github.com/camercu/relentless/commit/856bffeb1916c1b6a5e489d4013a89ca3fd871a0))

## [0.18.3](https://github.com/camercu/relentless/compare/v0.18.2...v0.18.3) (2026-10-14)

### Bug Fixes

* **deps:** bump rustls to 0.23.46 for the key-share advisory ([6dbbda3](https://github.com/camercu/relentless/commit/6dbbda3921d89c47eacc59f8f2a59a7db843b9c0))

## [0.18.2](https://github.com/camercu/relentless/compare/v0.18.1...v0.18.2) (2026-09-15)

### Upgrading from 0.18.1

No library code changed. This release updates npm packages used by the release tooling. Nothing to do.

### Bug Fixes

* **deps:** patch npm-bundled vulns (undici, tar, ip-address, brace-expansion) ([38af0b7](https://github.com/camercu/relentless/commit/38af0b7c15dfcccc22a800f75417b45ac7c4bb93))

## [0.18.1](https://github.com/camercu/relentless/compare/v0.18.0...v0.18.1) (2026-09-15)

### Upgrading from 0.18.0

No library code changed. This release updates `rustls`, which the crate's tests and examples pull in through the `reqwest` dev-dependency. Nothing to do.

### Bug Fixes

* **deps:** update rustls to 0.23.45 for RUSTSEC-2026-0285 ([d583d20](https://github.com/camercu/relentless/commit/d583d2073292866889351d66b62bf956a5b29702))

## [0.18.0](https://github.com/camercu/relentless/compare/v0.17.0...v0.18.0) (2026-09-09)

### Upgrading from 0.17

* **`.cap(max).jitter(j)` changed behavior.** Delays can now reach `max + j`; in 0.17 they were clamped to `max`. To keep the old bound, write `.jitter(j).cap(max)`. Code that names the type now gets `Jittered<WaitCapped<W>>` instead of `WaitCapped<Jittered<W>>`.
* **Async retries with a zero delay now yield between attempts.** Each attempt takes one poll instead of the whole loop running inside a single poll, so `select!`, `tokio::time::timeout` and cancellation now take effect. Code that awaits the future to completion needs no change.
* **A clock that moves backwards no longer refunds the time budget.** `.timeout()` and `stop::elapsed` count from the highest reading seen. Debug builds panic if a custom `Clock` goes backwards.
* **More types are nameable.** `StatelessOp`, `HookChain`, `RetryOp`, `AsyncRetryOp`, `BeforeAttemptHook`, `AttemptHook` and `ExitHook` are exported from the crate root, so builder types can be written out in signatures.
* The breaking-change list below also mentions `Wait::max_delay`, `Wait::imposed_cap` and a public `StatelessOp` field. All three were added and removed during this release; 0.17 had none of them.

### ⚠ BREAKING CHANGES

* **wait:** .cap(max).jitter(j) now yields delays up to
max + j rather than being clamped to max. Code relying on the
previous normalization should write .jitter(j).cap(max), which is
unchanged. Wait::imposed_cap is removed; it was added earlier in this
unreleased series.
* **wait:** Wait::max_delay is now Wait::imposed_cap. It was
introduced earlier in this unreleased series and has never shipped.
* **api:** relentless::StatelessOp no longer exposes its
field. It was only reachable at all from the re-export added earlier
in this same unreleased series.
* **async:** an async retry with a zero inter-attempt delay now
takes one poll per attempt rather than completing within a single
poll. No API changed; code that drives the future to completion
is unaffected.
* **wait:** the inherent WaitCapped::jitter is gone, so
.cap(max).jitter(j) now has type Jittered<WaitCapped<W>> rather than
WaitCapped<Jittered<W>>. Code that spells the type out must be updated;
behavior is identical.

### Features

* **api:** hide the StatelessOp field and derive the standard traits ([9130ae7](https://github.com/camercu/relentless/commit/9130ae787b90924f2d9227e728da1557d24fe4f0))
* **api:** make the builder's type-state plumbing nameable ([b57c5bd](https://github.com/camercu/relentless/commit/b57c5bd4ae573550566adddc3c08fa1fd953fa70))
* **wait:** carry the cap guarantee on the trait, not on name resolution ([0f0f69a](https://github.com/camercu/relentless/commit/0f0f69aa2ae01c3b14a33429febba3b6e2324aa4))
* **wait:** compose cap and jitter in the order written ([b842de4](https://github.com/camercu/relentless/commit/b842de4d63618991415ceb0d49c58c06666d3eb2))

### Bug Fixes

* **async:** yield cooperatively instead of spinning on a zero delay ([ee2ad4f](https://github.com/camercu/relentless/commit/ee2ad4fd53806320a8d1136fd3ad7b44cb14ab63))
* **bench:** stop the retry-loop benchmark folding to a constant ([595771a](https://github.com/camercu/relentless/commit/595771a06e5b0ce1580a842a7d8970f179d891d5))
* **ci:** run semver-checks on the pinned toolchain ([9392737](https://github.com/camercu/relentless/commit/93927374e34c4238eb304c294bb3837a3a71b321))
* **deps:** bump h2 to 0.4.19 for RUSTSEC-2026-0258 ([8a187a7](https://github.com/camercu/relentless/commit/8a187a7e75cf4556c2576003ed22a125ccaa1ab8))
* **docs:** retract the .fuse() remedy for the re-poll panic ([7252f50](https://github.com/camercu/relentless/commit/7252f50419988e8e7dd9b8a30302005e782a1a64))
* **engine:** keep elapsed time non-decreasing across clock reads ([adc01a1](https://github.com/camercu/relentless/commit/adc01a1a2b4e65ffa183b212688ac8c68fabebeb))
* **engine:** stop Elapsed being copyable ([3c77eef](https://github.com/camercu/relentless/commit/3c77eef27c7cc6cce2cf2c35a699e1d39fbe0f0e))
* **examples:** make the two silent examples show their result ([8f744c0](https://github.com/camercu/relentless/commit/8f744c05cfa7f8b015741a7ebd1b9d1e76c4146a))
* **wait:** propagate the delay ceiling through chain and sum ([c68aa94](https://github.com/camercu/relentless/commit/c68aa94da282aab6f2fbb1b06ba5f80cfc31125c))
* **wait:** stop carrying a branch cap outward through composites ([dd75dbf](https://github.com/camercu/relentless/commit/dd75dbf66b48d2d5bd51f4bb1fcbed899f112952))

### Code Refactoring

* **wait:** rename Wait::max_delay to imposed_cap ([0a75ddf](https://github.com/camercu/relentless/commit/0a75ddfd7368808f29d5b47d627a808f89a56e03))

## [0.17.0](https://github.com/camercu/relentless/compare/v0.16.0...v0.17.0) (2026-07-25)

### Features

* **decision:** classify ControlFlow outcomes as Break-returns/Continue-retries ([e772976](https://github.com/camercu/relentless/commit/e7729762f51b47e7941341127b1c43532c7b1dbf))
* **error:** give the ControlFlow error shape Display and Error impls ([0b52a11](https://github.com/camercu/relentless/commit/0b52a11b9d87b1dc42f69836fb62feaec0da84d6))

## [0.16.0](https://github.com/camercu/relentless/compare/v0.15.0...v0.16.0) (2026-07-25)

### Features

* **decision:** classify Option outcomes as None-retries/Some-returns ([694a7a3](https://github.com/camercu/relentless/commit/694a7a3f4cf6b554abfdc37ea1eed9d75beb9695))
* **error:** give the Option error shape Display and Error impls ([9b5545a](https://github.com/camercu/relentless/commit/9b5545a591959da588fd91edf6d42ed3501a86ba))
* **error:** make last()/into_last() available for all outcome shapes ([edb046a](https://github.com/camercu/relentless/commit/edb046ab5a775fde05a4082b92f06aa58f15f0aa))

## [0.15.0](https://github.com/camercu/relentless/compare/v0.14.0...v0.15.0) (2026-07-24)


### Features

* **policy:** let RetryPolicy carry a wall-clock timeout ([ebfe9a1](https://github.com/camercu/relentless/commit/ebfe9a1620e0e66c6b7f839bc039faf249c7cb0e))

## [0.14.0](https://github.com/camercu/relentless/compare/v0.13.0...v0.14.0) (2026-07-23)


### ⚠ BREAKING CHANGES

* **engine:** make AttemptState::new crate-private
* **predicate:** drop the composition algebra, keep the factories (ADR-6 #1b)
* **api:** trim now-redundant public surface (ADR-6)
* **engine:** re-found the engine on a paired-decision classifier (ADR-6)
* **state:** elapsed/total_elapsed are Duration instead of
Option<Duration>; with_elapsed takes Duration.
* **policy:** async builders configure time via
.clock(impl AsyncClock) instead of .sleep(impl Sleeper);
relentless::sleep, Sleeper, NoSyncSleep, NoAsyncSleep,
test_util, and the test-util feature are removed; features
tokio-sleep/embassy-sleep/gloo-timers-sleep/futures-timer-sleep
are renamed to
tokio-clock/embassy-clock/gloo-timers-clock/futures-timer-clock.
* **policy:** sync builders configure time via .clock(impl SyncClock)
instead of .sleep(...)/.elapsed_clock(...)/.elapsed_clock_fn(...).

* **api:** trim now-redundant public surface (ADR-6) ([b1a1b58](https://github.com/camercu/relentless/commit/b1a1b586e259d7f380c84005e62bf23575208c0e))
* **engine:** make AttemptState::new crate-private ([61561d1](https://github.com/camercu/relentless/commit/61561d1eb3af4c03619d044aae1bde1c08af31ad))
* **predicate:** drop the composition algebra, keep the factories (ADR-6 [#1b](https://github.com/camercu/relentless/issues/1b)) ([267d2e5](https://github.com/camercu/relentless/commit/267d2e5ca57ae92641f1074ae18b37901ee5e2d5)), closes [#1](https://github.com/camercu/relentless/issues/1)


### Features

* **clock:** accept capturing closures as GlooClock now-source ([a51e59f](https://github.com/camercu/relentless/commit/a51e59f4962d11316769d6f1900cfe11d435ae29))
* **clock:** add runtime AsyncClock adapters ([68ad00c](https://github.com/camercu/relentless/commit/68ad00c8e062a1c86601777e8fd91ce3dfb3e2a9))
* **clock:** add unified clock abstraction (ADR-0005) ([dc47719](https://github.com/camercu/relentless/commit/dc47719598ba15641cc02e3f47786125796b4d54))
* **clock:** blanket-implement AsyncClock for shared references ([73d7033](https://github.com/camercu/relentless/commit/73d7033162ec6d581dd02acc677cad6fd82a85da))
* **clock:** point missing-clock compile errors at .clock(...) ([96ed282](https://github.com/camercu/relentless/commit/96ed282854c10abd38135ebe697191e42038257c))
* **engine:** add .decide with paired Decision/Verdict enums (ADR-6 S2) ([917c0d9](https://github.com/camercu/relentless/commit/917c0d9736bcce1cc1a2433faddd6707e1c15404))
* **engine:** add .when/.until predicate classifiers (ADR-6 S3) ([fe818b7](https://github.com/camercu/relentless/commit/fe818b756095afccc6ea757257e911cf663871ae))
* **engine:** add before/after/on_exit hooks and Exit view (ADR-6 S5-hooks) ([b265149](https://github.com/camercu/relentless/commit/b26514923bbae53340fe30d3463a8939d43ab310))
* **engine:** add classifier-driven retry skeleton (ADR-6 S1) ([faa71d1](https://github.com/camercu/relentless/commit/faa71d13b0549e26b4d8c8b7691186f748d306aa))
* **engine:** add Result-shaped RetryError helpers (ADR-6 S8a) ([8335afa](https://github.com/camercu/relentless/commit/8335afad2df80c378f84239c21e8905549453fc3))
* **engine:** add RetryExt/AsyncRetryExt closure entry points (ADR-6 S7) ([6a45dbe](https://github.com/camercu/relentless/commit/6a45dbef683ee545eb803b8a2a027516058dc932))
* **engine:** add the async classifier engine (ADR-6 S6) ([4cdcea2](https://github.com/camercu/relentless/commit/4cdcea225a7d44b1c840b56397ae2dfe1cbfdc74))
* **engine:** add with_stats and timeout (ADR-6 S5-stats) ([8d908ff](https://github.com/camercu/relentless/commit/8d908fff35aafacd9caaacf6e9b62f8ff06ed756))
* **engine:** implement Stop/Wait/Decide for shared references (ADR-6 S8b) ([41b80b7](https://github.com/camercu/relentless/commit/41b80b789e2fcb325b4b90fb696324d64fd33bdd))
* **engine:** re-found the engine on a paired-decision classifier (ADR-6) ([53e80b9](https://github.com/camercu/relentless/commit/53e80b906d6a1ae5e4e56f23b73a68b2d61a22ad))
* **policy:** replace sync sleep/elapsed-clock seams with .clock() ([088818f](https://github.com/camercu/relentless/commit/088818ff6476187519429219c75345277b82960f))
* **policy:** unify async engine onto AsyncClock and retire the sleep seams ([20c6a03](https://github.com/camercu/relentless/commit/20c6a03aa710c44c6385a84da7de484498e51d5d))
* **state:** make elapsed time non-optional ([49883f4](https://github.com/camercu/relentless/commit/49883f4a5425757441d1cfb93593df93c4ea8202))


### Bug Fixes

* **clock:** clamp GlooClock waits to the i32 setTimeout ceiling ([7bbb0d4](https://github.com/camercu/relentless/commit/7bbb0d4164de6ae32ecf78275da312af317367d6))
* **clock:** require futures-timer 3.0.4 so saturated waits cannot panic ([c42301b](https://github.com/camercu/relentless/commit/c42301be7a342f27f28099598237e26ef59ead0c))
* **clock:** saturate GlooClock waits; align SPEC loop order and stale names ([e919de1](https://github.com/camercu/relentless/commit/e919de13b062c54ace4e50737fb9bcd1c8c93e59))

## [0.13.0](https://github.com/camercu/relentless/compare/v0.12.0...v0.13.0) (2026-07-16)


### ⚠ BREAKING CHANGES

* **async:** AsyncRetryExec, AsyncRetryExecWithStats, and the
async type aliases lose the Fut and SleepFut type parameters; Fut
moved to a generic on .call(). Chained-builder usage is unaffected;
only explicit type annotations need updating.
* **hooks:** without alloc, hook setters now produce HookChain<(),
Hook> slot types instead of bare Hook; previously-rejected multi-hook
no-alloc code now compiles.
* **state:** RetryState::new, AttemptState::new, and
ExitState::new are removed. Use RetryState::for_attempt(n),
AttemptState::for_attempt(n, &outcome), ExitState::for_attempt(n,
&outcome, reason), with optional fields set via with_elapsed /
with_next_delay / with_previous_delay.

* **async:** split builder from retry state machine ([cfa65a2](https://github.com/camercu/relentless/commit/cfa65a2b6d2dbd58f67e5f68d085e1feaea0d574))


### Features

* **hooks:** multiple hooks per point no longer require alloc ([b1341d2](https://github.com/camercu/relentless/commit/b1341d2b2e3be8158fc1d519a00636ee240fca33))
* **state:** construct state types via for_attempt + with_* setters ([111052d](https://github.com/camercu/relentless/commit/111052d923da6589a78d0972fca00118631a4b1e))


### Bug Fixes

* **sync:** bound .sleep() by SyncSleep so misuse fails at the call site ([91e5c59](https://github.com/camercu/relentless/commit/91e5c59959bbcec7cb70b2276c07214aab16bf80))
* **sync:** restore clippy compliance on stats-returning call ([a45500c](https://github.com/camercu/relentless/commit/a45500c8d13fbde2ab2a78eab2529550cf9d767f))

## [0.12.0](https://github.com/camercu/relentless/compare/v0.11.1...v0.12.0) (2026-07-11)


### Features

* **test-util:** add VirtualClock deterministic test clock ([07890bf](https://github.com/camercu/relentless/commit/07890bf21faec177232a9e6cddd5e963de47fd2b))


### Bug Fixes

* **hooks:** pass previous_delay to before_attempt state ([f50972d](https://github.com/camercu/relentless/commit/f50972d223e14a9293782ce19120792d15b2a528))

## [0.11.1](https://github.com/camercu/relentless/compare/v0.11.0...v0.11.1) (2026-07-09)


### Bug Fixes

* **policy:** capture elapsed baseline at execution start ([ed28f35](https://github.com/camercu/relentless/commit/ed28f352ab0af1976868f1639409d97ebc0cf900))
* **sleep:** saturate embassy conversion without overflow panic ([9528013](https://github.com/camercu/relentless/commit/952801384e92eaaf36b6bf719076431fcf8594dc))
* **wait:** keep zero-initial exponential at zero past f64 overflow ([9f9f1a3](https://github.com/camercu/relentless/commit/9f9f1a3bc96b274c9a88eac47af702008fef17b4))

## [0.11.0](https://github.com/camercu/relentless/compare/v0.10.0...v0.11.0) (2026-07-02)


### Features

* **wait:** make jittered strategies Sync via atomic PRNG state ([6ccb31c](https://github.com/camercu/relentless/commit/6ccb31c2f9c7973860050f129746aff2775de0e5))


### Bug Fixes

* **wait:** with_seed alone pins the jitter sequence ([903781b](https://github.com/camercu/relentless/commit/903781b6f5cb9563e5e6c578e6399bfeb05d38cd))

## [0.10.0](https://github.com/camercu/relentless/compare/v0.9.0...v0.10.0) (2026-06-22)


### ⚠ BREAKING CHANGES

* **error:** `RetryError` is now `#[non_exhaustive]`; exhaustive matches on
it must add a wildcard `_` arm.
* **async:** async retry builders no longer implement `Future`. Replace
`builder.await` with `builder.call().await` (and `with_stats().await` with
`with_stats().call().await`).
* **wait:** the `WaitDecorrelatedJitter` type is removed;
`wait::decorrelated_jitter` now returns `Jittered<WaitFixed>`. `RetryState`
gains a `previous_delay` field (it is `#[non_exhaustive]`, so this is additive
for matches but affects exhaustive struct literals).
* **stats:** `StopReason::Accepted` is removed; use `Succeeded` or
`Rejected`. `StopReason` is now `#[non_exhaustive]`, so exhaustive matches
require a wildcard arm. `Display` now emits "succeeded"/"rejected" instead of
"accepted".
* **policy:** `.boxed()`/`.boxed_local()` no longer take `<T, E>` type
arguments and no longer box the predicate; the returned type's third parameter
is now the original predicate `P` instead of `Box<dyn Predicate<T, E>>`.
* **policy:** SyncRetry, SyncRetryBuilder, AsyncRetry, and
AsyncRetryBuilder (and their WithStats variants) are now type aliases
over SyncRetryExec/AsyncRetryExec. Debug output prints the engine type
name (e.g. "SyncRetryExec") rather than the alias name.

* **error:** mark RetryError #[non_exhaustive] ([db4bc6e](https://github.com/camercu/relentless/commit/db4bc6ecbfb849566bc110775e2441ef5f0eedbd))
* **policy:** boxed() erases stop+wait only, predicate stays generic ([4e4b59e](https://github.com/camercu/relentless/commit/4e4b59e32bd2de9e03005987cf3ba98f6e9e0b02))
* **policy:** collapse sync/async retry wrappers into one engine each ([96b4c1f](https://github.com/camercu/relentless/commit/96b4c1f5b84967ae1f352c059576dd55db7b20a7))


### Features

* **api:** re-export NoSyncSleep/NoAsyncSleep at the crate root ([5da7d3e](https://github.com/camercu/relentless/commit/5da7d3e9a9004ee2eb62ce924ad2a446e73cb31d))
* **async:** terminate async retries with .call(), mirroring sync ([2ceeceb](https://github.com/camercu/relentless/commit/2ceeceba962de8888b90719bedc19037e231742e))
* **prelude:** add prelude module re-exporting DSL traits ([4db8b21](https://github.com/camercu/relentless/commit/4db8b21f826a8a4ebf2f4a8413c081652e08536c))
* **stats:** split StopReason::Accepted into Succeeded and Rejected ([6481aed](https://github.com/camercu/relentless/commit/6481aedc6d2277641f95c7f44cb91ae05f389aa9))
* **wait:** feedback jitter via RetryState.previous_delay; fold decorrelated ([76f24f9](https://github.com/camercu/relentless/commit/76f24f960e86eb31fde9c1c7c9da54703fd8a7a7))


### Performance Improvements

* **time:** skip Instant::now() when custom clock configured ([36e0c94](https://github.com/camercu/relentless/commit/36e0c944497830cda5b6aa442ce15c0d6bae30da))

## [0.9.0](https://github.com/camercu/relentless/compare/v0.8.0...v0.9.0) (2026-04-25)


### Features

* add code coverage targets with cargo-llvm-cov ([e3bc875](https://github.com/camercu/relentless/commit/e3bc8759da1b59c4e72e8a85cc07496f85826f42))


### Bug Fixes

* **test:** remove target_os gate from embassy sleep test ([6d6b150](https://github.com/camercu/relentless/commit/6d6b150a415b413ce4fce333836c369c348468b4))

## [0.8.0](https://github.com/camercu/relentless/compare/v0.7.2...v0.8.0) (2026-04-16)


### Features

* add tool-versions-update and tool-versions-update-check recipes ([88d2eb2](https://github.com/camercu/relentless/commit/88d2eb2d3de0181a6a13e661704be6ae623db87a))
* update rust-toolchain.toml and install toolchain in update script ([9c7b615](https://github.com/camercu/relentless/commit/9c7b615e7de56ffd5a71539f1b26ed4124aa870f))


### Bug Fixes

* **ci:** run semver-checks with stable toolchain ([f07c471](https://github.com/camercu/relentless/commit/f07c4713d4684f5794a9bce190f558d93f410db9))
* resolve unchecked Duration subtraction clippy lint ([ed41f40](https://github.com/camercu/relentless/commit/ed41f40ced5ee142d1fbda54317ff2d4e3ee7550))

## [0.7.2](https://github.com/camercu/relentless/compare/v0.7.1...v0.7.2) (2026-04-12)


### Bug Fixes

* **docs:** add missing sleep call in hooks-and-stats doctest ([92bf9cd](https://github.com/camercu/relentless/commit/92bf9cd0916939ee9dbbcbbd5ecc0b851ddd3a08))

## [0.7.1](https://github.com/camercu/relentless/compare/v0.7.0...v0.7.1) (2026-04-05)


### Bug Fixes

* add required-features to sync-cancel example ([0244edc](https://github.com/camercu/relentless/commit/0244edc18693d78b23537312d3c95f6be98cc0c9))

## [0.7.0](https://github.com/camercu/relentless/compare/v0.6.0...v0.7.0) (2026-04-04)


### ⚠ BREAKING CHANGES

* **crate:** crate name changed

* **crate:** rename crate from 'tenacious' to 'relentless' ([98d16e6](https://github.com/camercu/relentless/commit/98d16e697ccdde8028448e16f095cac49f932884))

# [0.6.0](https://github.com/camercu/relentless/compare/v0.5.0...v0.6.0) (2026-04-01)


### Bug Fixes

* **ci:** pass explicit shell.nix to nix-shell in release workflow ([56f91c3](https://github.com/camercu/relentless/commit/56f91c34df9f31d034f5478049def64134d4eebc))
* **ci:** skip cargo registry token verification in semantic-release ([6d5237b](https://github.com/camercu/relentless/commit/6d5237b190f9971508a69101583dd9c3b6417661))


### Features

* **examples:** add async-cancel example showing timeout and select! cancellation ([3e7fc44](https://github.com/camercu/relentless/commit/3e7fc44acba715ad814d559ab8033ac0022e5124))
* **execution:** debug_assert when timeout is set without an elapsed clock ([2aebde5](https://github.com/camercu/relentless/commit/2aebde51600c47138ce99a43fcf05f0e99820dee))
* **policy:** add RetryPolicy::boxed_local() for type erasure without Send ([35f007f](https://github.com/camercu/relentless/commit/35f007f2d69df91d8d4149a83e8af320f6cc0006))

# 0.5.0

### Breaking

- **Consolidated jitter wrapper types.** `WaitJitter<W>`, `WaitFullJitter<W>`,
  and `WaitEqualJitter<W>` have been merged into a single `Jittered<W>` type.
  The `.jitter()`, `.full_jitter()`, and `.equal_jitter()` methods on the `Wait`
  trait now all return `Jittered<Self>`. Code that names these types explicitly
  must be updated; code that only uses the builder methods is unaffected.

# 0.4.0

### Breaking

- **Jitter no longer requires a feature flag.** The `jitter` feature has been
  removed. All jitter strategies (`.jitter()`, `.full_jitter()`,
  `.equal_jitter()`, `wait::decorrelated_jitter()`) are now always available
  with zero additional dependencies.
- **`with_seed()` now accepts `u64` instead of `[u8; 32]`.** This simplifies
  seeding and matches the underlying PRNG state size.
- **Cloning a jitter strategy now decorrelates the clone.** Previously, clones
  shared identical PRNG state and produced the same jitter sequence, creating
  a thundering herd among clones. Clones now get a fresh PRNG stream
  automatically.

### Removed

- Removed `rand` dependency. Jitter now uses an inline SplitMix64 PRNG.

# 0.3.1

### Fixed

- Removed redundant doc links and strengthened pre-push hook.
- CI fixes: updated action SHAs, switched to version tags, opted into
  Node.js 24, removed stale `serde` feature reference.

### Changed

- CI now reads tool versions from `.tool-versions` instead of hardcoding them.
- CI replaced nix-shell with direct tool installation for faster runs.
- Reorganized test files by logical concern.
- Aligned public API surface with spec.
- Refactored examples for readability: extracted named operations, replaced
  manual async executor with tokio, used `.until()` for polling example.
- Added `test-examples` target to justfile and wired it into CI.

# 0.3.0

### Breaking

- Aligned public exports with SPEC: tightened re-exports and module visibility.

# 0.2.0

### Breaking

- **Removed cancellation infrastructure.** `cancel()`, `CancelToken`, cancel
  futures, and all cancellation-related APIs have been removed.
- **Removed `poll_until` / `poll_until_async`.** Use `.until(predicate::ok(...))`
  on a `RetryPolicy` instead.
- **Removed serde support.** The `serde` feature, `Serialize`/`Deserialize`
  impls on strategies and stats, and the `serde` dependency have been removed.
- **`RetryExt` / `AsyncRetryExt` now require `FnMut()` closures** (no
  `RetryState` parameter). Use `retry()` / `retry_async()` free functions or
  `RetryPolicy` methods for state-aware operations.

### Added

- **`.until()` predicate.** `RetryPolicy::until(p)` and
  `predicate::until(p)` negate a predicate, reading naturally for polling:
  `.until(predicate::ok(|s| s.is_ready()))`.

# 0.1.0

Initial release.

### Added

- Core retry engine with sync and async support.
- `RetryPolicy<S, W, P>` for reusable, composable retry configuration.
- `retry()` and `retry_async()` free functions for one-off retries with
  sensible defaults (3 attempts, exponential backoff, retry on any error).
- `RetryExt` and `AsyncRetryExt` extension traits for calling `.retry()` /
  `.retry_async()` directly on closures and function pointers.
- **Stop strategies:** `stop::attempts`, `stop::elapsed`, `stop::never` with
  `|` (either) and `&` (both) composition operators.
- **Wait strategies:** `wait::fixed`, `wait::linear`, `wait::exponential` with
  `+` (sum) composition, `.cap()`, and `.chain()`.
- **Jitter strategies** (behind `jitter` feature): `.jitter()`, `.full_jitter()`,
  `.equal_jitter()`, `wait::decorrelated_jitter()` with `.with_seed()` and
  `.with_nonce()` for reproducibility.
- **Predicates:** `predicate::any_error`, `predicate::error`,
  `predicate::ok`, `predicate::result` with `|` (or) and `&` (and) composition.
- **Hooks:** `.before_attempt()`, `.after_attempt()`, `.on_exit()` lifecycle
  hooks on execution builders. Multiple hooks per point supported with `alloc`.
- **Stats:** `.with_stats()` returns `(Result, RetryStats)` with attempt count,
  total wait, total elapsed, and stop reason.
- **Error types:** `RetryError<T, E>` with `Exhausted` and `Rejected` variants,
  `RetryResult<T, E>` alias.
- **Runtime sleep adapters:** `sleep::tokio()`, `sleep::embassy()`,
  `sleep::gloo()`, `sleep::futures_timer()` behind feature flags.
- **Custom elapsed clocks** via closure or `Instant`-based tracking.
- **Zero-alloc async retry** — async retry works without `alloc`.
- Works across `std`, `no_std` (with `alloc`), and `wasm32` targets.
- MSRV: Rust 1.85.
