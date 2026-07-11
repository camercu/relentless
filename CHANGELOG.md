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
