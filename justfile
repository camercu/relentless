set shell := ["bash", "-euo", "pipefail", "-c"]

toml_files := "Cargo.toml deny.toml"
no_std_target := "thumbv7m-none-eabi"
wasm_target := "wasm32-unknown-unknown"
wasm_features := "alloc,gloo-timers-clock"
benchmark_target := "retry_hot_paths"
warnings := "-D warnings"
stable_toolchain := "+stable"

# cargo driver. Defaults to plain `cargo`; set RTK_CARGO="rtk cargo" (see the
# `ci-rtk` target) to route the compile-heavy recipes through rtk for
# token-compressed output. Only used where rtk compresses the subcommand and the
# output is for reading — recipes whose output is consumed (public-api snapshot,
# tool-version parsing) stay on plain cargo.
cargo := env("RTK_CARGO", "cargo")

default:
    @just --list

# ── Formatting ──────────────────────────────────────────────

fmt: fmt-rust fmt-taplo

fmt-rust:
    cargo fmt --all

fmt-taplo:
    taplo fmt {{toml_files}}

fmt-check: fmt-check-rust fmt-check-taplo

fmt-check-rust:
    cargo fmt --all --check

fmt-check-taplo:
    taplo fmt --check {{toml_files}}

# ── Linting ─────────────────────────────────────────────────

lint: fmt-check lint-clippy lint-typos lint-deny

lint-clippy:
    {{cargo}} clippy --all-targets --all-features -- -D warnings

lint-clippy-stable:
    cargo {{stable_toolchain}} clippy --all-targets --all-features

lint-typos:
    typos

lint-deny:
    cargo deny check advisories licenses bans sources

# ── Testing ─────────────────────────────────────────────────

test:
    {{cargo}} nextest run

test-all-features:
    {{cargo}} nextest run --all-features

test-no-default:
    {{cargo}} nextest run --no-default-features

test-alloc:
    {{cargo}} nextest run --no-default-features --features alloc

test-doc:
    RUSTDOCFLAGS="{{warnings}}" {{cargo}} test --doc

test-doc-no-default:
    RUSTDOCFLAGS="{{warnings}}" {{cargo}} test --no-default-features --doc

test-readme:
    {{cargo}} test --features tokio-clock --doc -- readme_doctests

test-examples:
    cargo run --example basic-retry
    cargo run --example hooks-and-stats
    cargo run --example custom-outcome
    cargo run --example sync-cancel
    cargo run --example testing-with-virtual-clock
    cargo run --example async-polling --features tokio-clock
    cargo run --example async-cancel --features tokio-clock

test-tokio-clock:
    {{cargo}} nextest run --test policy_async --test clock_adapters --features tokio-clock

test-futures-timer-clock:
    {{cargo}} nextest run --test policy_async --test clock_adapters --features futures-timer-clock

test-allocation:
    {{cargo}} nextest run --test allocation -j 1

test-stable:
    cargo {{stable_toolchain}} nextest run

# ── Building / checking ─────────────────────────────────────

build:
    {{cargo}} build --all-targets

build-stable:
    cargo {{stable_toolchain}} build --all-targets

check-no-std:
    {{cargo}} build --target {{no_std_target}} --no-default-features

check-wasm:
    cargo check --target {{wasm_target}} --no-default-features --features {{wasm_features}}

check-embassy:
    cargo check --features embassy-clock

check-alloc:
    cargo check --no-default-features --features alloc

check-msrv:
    #!/usr/bin/env bash
    set -euo pipefail
    msrv=$(cargo metadata --format-version 1 --no-deps | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['rust_version'])")
    cargo "+${msrv}" check --quiet

semver-check:
    cargo +stable semver-checks check-release

# Remove run artifacts: build tree, cargo-mutants output, bacon locations.
# Deliberately keeps node_modules (environment, restored by npm ci) and
# proptest-regressions (committed corpus of past test failures).
clean:
    cargo clean
    rm -rf mutants.out mutants.out.old
    rm -f .bacon-locations

# ── Public API surface ──────────────────────────────────────
# cargo-public-api builds rustdoc JSON, which is nightly-only, so these
# recipes require a nightly toolchain (rustup installs one on demand).

# Print the current public API surface (--simplified omits blanket/auto-trait
# impl noise, keeping the snapshot readable and stable across toolchains).
public-api:
    cargo public-api --all-features --simplified

# Regenerate the committed public API snapshot after an intended change.
public-api-bless:
    cargo public-api --all-features --simplified > public-api.txt

# Fail if the public API has drifted from the committed snapshot.
public-api-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo public-api --all-features --simplified | diff -u public-api.txt - \
        || { echo "public API drifted from public-api.txt — review, then run 'just public-api-bless'"; exit 1; }

# ── Documentation ───────────────────────────────────────────

doc:
    RUSTDOCFLAGS="{{warnings}}" {{cargo}} test --doc --all-features
    RUSTDOCFLAGS="{{warnings}}" cargo doc --all-features --no-deps

# ── Benchmarks ──────────────────────────────────────────────

bench:
    cargo bench --bench {{benchmark_target}}

bench-no-run:
    cargo bench --bench {{benchmark_target}} --no-run

# ── Code coverage ──────────────────────────────────────────

coverage:
    cargo llvm-cov --all-features --html
    @echo "HTML report: target/llvm-cov/html/index.html"

coverage-text:
    cargo llvm-cov --all-features

coverage-lcov:
    cargo llvm-cov --all-features --lcov --output-path target/llvm-cov/lcov.info
    @echo "LCOV report: target/llvm-cov/lcov.info"

# ── Mutation testing ───────────────────────────────────────

mutants *args:
    cargo mutants {{args}}

# Mutation-test only code changed since `base` (including uncommitted
# changes): fast gate that new/changed code arrives with killing tests,
# without paying for a full sweep. CI runs this per push/PR.
mutants-diff base="origin/main":
    #!/usr/bin/env bash
    set -euo pipefail
    diff_file=$(mktemp)
    trap 'rm -f "$diff_file"' EXIT
    git diff "$(git merge-base "{{base}}" HEAD)" > "$diff_file"
    cargo mutants --in-diff "$diff_file"

# ── Tool versions ───────────────────────────────────────────

check-tool-versions:
    #!/usr/bin/env bash
    set -euo pipefail
    drift=0
    while read -r name version; do
        case "$name" in
            rust)       actual=$(rustc --version | awk '{print $2}') ;;
            just)       actual=$(just --version | awk '{print $2}') ;;
            cargo-deny) actual=$(cargo-deny --version | awk '{print $2}') ;;
            typos-cli)  actual=$(typos --version | awk '{print $2}') ;;
            taplo-cli)  actual=$(taplo --version | awk '{print $2}') ;;
            cargo-nextest) actual=$(cargo nextest --version | head -1 | awk '{print $2}') ;;
            cargo-semver-checks) actual=$(cargo semver-checks --version | awk '{print $2}') ;;
            cargo-mutants) actual=$(cargo mutants --version | awk '{print $2}') ;;
            cargo-llvm-cov) actual=$(cargo llvm-cov --version | awk '{print $2}') ;;
            cargo-public-api) actual=$(cargo public-api --version | awk '{print $2}') ;;
            *)          continue ;;
        esac
        if [ "$actual" != "$version" ]; then
            printf '  %-12s pinned=%s  actual=%s\n' "$name" "$version" "$actual"
            drift=1
        fi
    done < <(grep -v '^#' .tool-versions | grep -v '^$')
    if [ "$drift" -eq 1 ]; then
        echo "tool versions have drifted from .tool-versions"
        exit 1
    else
        echo "all tool versions match .tool-versions"
    fi

tool-versions-update:
    ./scripts/update-tool-versions.sh

tool-versions-update-check:
    ./scripts/update-tool-versions.sh --check

# ── Setup ───────────────────────────────────────────────────

setup:
    ./scripts/setup-dev.sh

# ── Hooks ───────────────────────────────────────────────────

pre-commit: check-tool-versions fmt-check lint-typos
    cargo check --all-targets --quiet

pre-push:
    RUSTFLAGS="{{warnings}}" RUSTDOCFLAGS="{{warnings}}" just lint-clippy test doc

# ── CI ──────────────────────────────────────────────────────

# Agent-facing CI: same steps as `ci`, but routes the compile-heavy recipes
# (clippy/nextest/test/build) through rtk for token-compressed output. Prefer
# this over `ci` when an agent runs the suite. Same pass/fail semantics.
ci-rtk:
    RTK_CARGO="rtk cargo" just ci

ci:
    just lint
    RUSTFLAGS="{{warnings}}" RUSTDOCFLAGS="{{warnings}}" just \
        test test-all-features test-no-default test-alloc test-doc-no-default doc \
        check-no-std check-wasm check-embassy check-msrv \
        test-readme test-examples \
        test-tokio-clock test-futures-timer-clock test-allocation \
        bench-no-run
    -just coverage-text

ci-stable: build-stable test-stable lint-clippy-stable

# ── Release ─────────────────────────────────────────────────

release:
    npm ci
    npx semantic-release
