# Contributing

This guide explains how to work on `tenacious` with the same toolchain and
checks used by the repository. Use it when you set up a new clone, install Git
hooks, or want to understand which commands are part of the normal development
loop.

`README.md` is the project overview and quick-start guide.
[docs/SPEC.md](./docs/SPEC.md) is the normative behavior and public-API
contract. Keep behavior changes aligned with the spec, and keep workflow and
CI guidance in this document rather than in the spec.

## Development environment

`tenacious` uses a pinned Rust toolchain and a pinned Nix shell so contributors
run the same checks with the same tool versions.

- [rust-toolchain.toml](./rust-toolchain.toml) pins Rust `1.85.0`, `clippy`,
  `rustfmt`, and the required cross-compilation targets.
- [shell.nix](./shell.nix) provides `just`, `pre-commit`, `taplo`, `typos`,
  and `cargo-deny`.

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

- a `pre-commit` hook for fast formatting and typo checks
- a `pre-push` hook for clippy and test coverage

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

## Toolchain policy

`tenacious` supports Rust `1.85.0` as its minimum supported Rust version
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
- `pre-push`: `just pre-push`

If you need to reinstall the hooks, run:

```bash
./scripts/setup-dev.sh
```
