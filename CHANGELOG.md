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
