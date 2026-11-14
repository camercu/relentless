# Contributing

This guide explains how to work on `relentless` with the same toolchain and
checks used by the repository. Use it when you set up a new clone, install Git
hooks, or want to understand which commands are part of the normal development
loop.

`README.md` is the project overview and quick-start guide.
[docs/SPEC.md](./docs/SPEC.md) is the normative behavior and public-API
contract. Keep behavior changes aligned with the spec, and keep workflow and
CI guidance in this document rather than in the spec.

## Development environment

`relentless` uses a pinned Rust toolchain and a pinned Nix shell so contributors
run the same checks with the same tool versions.

- [rust-toolchain.toml](./rust-toolchain.toml) pins Rust `1.94.1`, `clippy`,
  `rustfmt`, and the required cross-compilation targets.
- [shell.nix](./shell.nix) provides `just`, `pre-commit`, `taplo`, `typos`,
  `cargo-deny`, `cargo-semver-checks`, `cargo-mutants`, `cargo-llvm-cov`, and
  `cocogitto` (`cog`).

To enter the pinned shell, run:

```bash
nix-shell
```

## First-time setup

Run the setup script once after you clone the repository. It installs the Git
hooks through the pinned shell and leaves your repository ready for normal
development.

```bash
./scripts/setup-dev.sh
```

The script installs:

- a `pre-commit` hook for formatting, typo, and compilation checks
- a `pre-push` hook for the commit check, clippy, and tests
- a `commit-msg` hook that checks the message against Conventional Commits

## Daily workflow

Use `just` for day-to-day development. The commands are split between a fast
local loop and stricter CI-oriented gates.

For normal local work, the most useful commands are:

- `just build`
- `just test`
- `just fmt`
- `just lint`
- `just pre-commit`
- `just pre-push`

For the full repository gate, run:

```bash
just ci
```

`just ci` is the canonical project gate. It runs the pinned-toolchain checks
that GitHub Actions enforces.

## Stable checks

The repository also provides an advisory latest-stable lane. Use it to see how
the code behaves on the newest stable Rust without changing the supported
toolchain policy.

```bash
just ci-stable
```

`just ci-stable` is informational. It helps you spot new warnings or behavior
changes early, but the pinned `just ci` gate remains the source of truth.

## Semver and MSRV checks

`just semver-check` uses `cargo-semver-checks` to verify that changes do not
accidentally break the public API. The release workflow runs it after the
version bump and before anything is pushed, so an unintended break stops the
release; run it manually any time to check early. It is deliberately **not**
part of `just ci`: `cargo-semver-checks` compares against the published
version, but the version bump happens only at release time, so an intentional
breaking change would otherwise fail the gate and block the release.
`just check-msrv` compiles the crate
with the declared minimum supported Rust version and runs as part of `just ci`.

`just mutants` runs a full `cargo-mutants` sweep — run it periodically to find
test coverage gaps. `just mutants-diff [base]` mutation-tests only code changed
since `base` (default `origin/main`), and CI runs it on every push and pull
request as a fast gate that changed code arrives with killing tests.
`.cargo/mutants.toml` enables the host-testable clock-adapter features so
feature-gated code is covered too. When reading results, note that mutants in
`cfg`-gated code for other targets (the wasm-only `gloo` adapter, the
non-atomic `SplitMix64::advance` fallback) cannot be killed on the host.
Infinite-loop mutants (e.g. `should_stop -> false`) count as caught, not as
timeouts: cargo-mutants runs the suite under nextest (`.cargo/mutants.toml`),
whose slow-timeout (`.config/nextest.toml`) kills a hung test and fails the
run before cargo-mutants' own timeout can fire.

## Code coverage

`just coverage` generates an HTML coverage report using `cargo-llvm-cov`.
`just coverage-text` prints a summary table to stdout. `just coverage-lcov`
produces an LCOV file for tooling integration.

Coverage runs as an infallible step at the end of `just ci` — it reports
results but does not fail the build.

## Toolchain policy

`relentless` supports Rust `1.85.0` as its minimum supported Rust version
(MSRV). That matches the crate metadata and the Rust 2024 edition used by the
project.

You can use a newer local Rust toolchain if you want, but you must keep the
repository compatible with the pinned toolchain unless you intentionally raise
MSRV.

## Git hooks

The Git hooks are designed to give you quick feedback before a commit or push.
They run through `nix-shell`, so they use the pinned versions of the developer
tools.

The hooks currently do this:

- `pre-commit`: `just pre-commit`
- `pre-push`: `just pre-push`, which starts with `just check-commits`
- `commit-msg`: `cog verify`

jj fires no Git hooks, so jj users run `just pre-commit` and `just pre-push`
themselves before pushing.

If you need to reinstall the hooks, run:

```bash
./scripts/setup-dev.sh
```

## Releases

Releases are automatic. After CI passes on a push to `main`, the release
workflow runs `just release`, which uses cocogitto to pick the next version
from the Conventional Commits since the last tag:

- `fix`, `perf`, and `revert` bump the patch version.
- `feat` bumps the minor version.
- Before 1.0.0, a breaking change (`!` or a `BREAKING CHANGE:` footer) bumps
  the minor version, whatever its type.
- Other types (`docs`, `test`, `refactor`, `build`, `ci`, `chore`, `style`)
  release nothing. Use `ci:` for a CI fix, not `fix(ci):`, which would cut a
  patch release.
- `git revert` and `jj backout` write `Revert "…"` and `Back out "…"`
  subjects, which fail the commit check. Reword them as `revert: …`.

`just release` writes the new version into `Cargo.toml` and `Cargo.lock`,
prepends the entry to `CHANGELOG.md`, commits, and tags. The workflow then
runs `just semver-check`, pushes the commit and tag, publishes the crate
(`scripts/publish-crate.sh`), and creates the GitHub Release
(`scripts/publish-release.sh`). `just test-release` exercises the version
choice and the bump commit in a throwaway clone and runs as part of
`just ci`.

To retry a release that failed after its tag was pushed, run the Release
workflow by hand with that tag. Each publish step skips what already exists.

The workflow never reaches 1.0, because a breaking change before 1.0.0 bumps
the minor version. Cut 1.0 by hand:

1. Rewrite the cases in `scripts/test-release.sh` for a 1.x version. They
   assume 0.x and fail `just ci` otherwise.
1. With `main` checked out (under jj, `jj new main`, then
   `git checkout -B main`, since cocogitto bumps only on a branch named
   `main`), run `cog bump --version 1.0.0`, then
   `just semver-check`.
1. Push the commit and the `v1.0.0` tag.
1. Run the Release workflow by hand (Actions → Release → Run workflow) with
   the tag `v1.0.0`.
