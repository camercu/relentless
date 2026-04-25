set shell := ["bash", "-euo", "pipefail", "-c"]

toml_files := "Cargo.toml deny.toml"
no_std_target := "thumbv7m-none-eabi"
wasm_target := "wasm32-unknown-unknown"
wasm_features := "alloc,gloo-timers-sleep"
benchmark_target := "retry_hot_paths"
warnings := "-D warnings"
stable_toolchain := "+stable"

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
    cargo clippy --all-targets --all-features -- -D warnings

lint-clippy-stable:
    cargo {{stable_toolchain}} clippy --all-targets --all-features

lint-typos:
    typos

lint-deny:
    cargo deny check advisories licenses bans sources

# ── Testing ─────────────────────────────────────────────────

test:
    cargo nextest run

test-all-features:
    cargo nextest run --all-features

test-no-default:
    cargo nextest run --no-default-features

test-alloc:
    cargo nextest run --no-default-features --features alloc

test-doc:
    RUSTDOCFLAGS="{{warnings}}" cargo test --doc

test-doc-no-default:
    RUSTDOCFLAGS="{{warnings}}" cargo test --no-default-features --doc

test-readme:
    cargo test --features tokio-sleep --doc -- readme_doctests

test-examples:
    cargo run --example basic-retry
    cargo run --example hooks-and-stats
    cargo run --example sync-cancel
    cargo run --example async-polling --features tokio-sleep
    cargo run --example async-cancel --features tokio-sleep

test-tokio-sleep:
    cargo nextest run --test policy_async --features tokio-sleep

test-futures-timer-sleep:
    cargo nextest run --test policy_async --features futures-timer-sleep

test-allocation:
    cargo nextest run --test allocation -j 1

test-stable:
    cargo {{stable_toolchain}} nextest run

# ── Building / checking ─────────────────────────────────────

build:
    cargo build --all-targets

build-stable:
    cargo {{stable_toolchain}} build --all-targets

check-no-std:
    cargo build --target {{no_std_target}} --no-default-features

check-wasm:
    cargo check --target {{wasm_target}} --no-default-features --features {{wasm_features}}

check-embassy:
    cargo check --features embassy-sleep

check-alloc:
    cargo check --no-default-features --features alloc

check-msrv:
    #!/usr/bin/env bash
    set -euo pipefail
    msrv=$(cargo metadata --format-version 1 --no-deps | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['rust_version'])")
    cargo "+${msrv}" check --quiet

semver-check:
    cargo +stable semver-checks check-release

# ── Documentation ───────────────────────────────────────────

doc:
    RUSTDOCFLAGS="{{warnings}}" cargo test --doc --all-features
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

ci:
    just lint
    RUSTFLAGS="{{warnings}}" RUSTDOCFLAGS="{{warnings}}" just \
        test test-all-features test-no-default test-alloc test-doc-no-default doc \
        check-no-std check-wasm check-embassy check-msrv \
        test-readme test-examples \
        test-tokio-sleep test-futures-timer-sleep test-allocation \
        bench-no-run semver-check
    -just coverage-text

ci-stable: build-stable test-stable lint-clippy-stable

# ── Release ─────────────────────────────────────────────────

release:
    npm ci
    npx semantic-release
