set shell := ["bash", "-euo", "pipefail", "-c"]

toml_files := "Cargo.toml deny.toml"
no_std_target := "thumbv7m-none-eabi"
wasm_target := "wasm32-unknown-unknown"
wasm_features := "alloc,gloo-timers-sleep"
benchmark_target := "retry_hot_paths"
warning_flags := "-D warnings"
stable_toolchain := "+stable"

default:
    @just --list

fmt:
    just fmt-rust
    just fmt-taplo

fmt-rust:
    cargo fmt --all

fmt-taplo:
    taplo fmt {{toml_files}}

fmt-check:
    just fmt-check-rust
    just fmt-check-taplo

fmt-check-rust:
    cargo fmt --all --check

fmt-check-taplo:
    taplo fmt --check {{toml_files}}

lint: lint-clippy lint-taplo lint-typos lint-deny

lint-clippy:
    cargo clippy --all-targets --all-features -- -D warnings

lint-clippy-stable:
    cargo {{stable_toolchain}} clippy --all-targets --all-features

lint-taplo: fmt-check-taplo

lint-typos:
    typos

lint-deny:
    cargo deny check advisories licenses bans sources

test:
    cargo test --all-targets

test-strict:
    RUSTFLAGS="{{warning_flags}}" cargo test --all-targets

test-stable:
    cargo {{stable_toolchain}} test --all-targets

test-no-default:
    cargo test --no-default-features --tests

test-doc-no-default:
    RUSTDOCFLAGS="-D warnings" cargo test --no-default-features --doc

test-doc-no-default-strict:
    RUSTFLAGS="{{warning_flags}}" RUSTDOCFLAGS="-D warnings" cargo test --no-default-features --doc

test-no-default-strict:
    RUSTFLAGS="{{warning_flags}}" cargo test --no-default-features

build:
    cargo build --all-targets

build-strict:
    RUSTFLAGS="{{warning_flags}}" cargo build --all-targets

build-stable:
    cargo {{stable_toolchain}} build --all-targets

doc:
    RUSTDOCFLAGS="-D warnings" cargo test --doc
    RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps

doc-strict:
    RUSTFLAGS="{{warning_flags}}" RUSTDOCFLAGS="-D warnings" cargo test --doc
    RUSTFLAGS="{{warning_flags}}" RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps

check-no-std:
    cargo build --target {{no_std_target}} --no-default-features

check-no-std-strict:
    RUSTFLAGS="{{warning_flags}}" cargo build --target {{no_std_target}} --no-default-features

check-wasm:
    cargo check --target {{wasm_target}} --no-default-features --features {{wasm_features}}

check-wasm-strict:
    RUSTFLAGS="{{warning_flags}}" cargo check --target {{wasm_target}} --no-default-features --features {{wasm_features}}

test-readme:
    cargo test --features tokio-sleep --doc -- readme_doctests

test-readme-strict:
    RUSTFLAGS="{{warning_flags}}" cargo test --features tokio-sleep --doc -- readme_doctests

test-examples:
    cargo run --example basic-retry
    cargo run --example hooks-and-stats
    cargo run --example async-polling --features tokio-sleep

test-examples-strict:
    RUSTFLAGS="{{warning_flags}}" cargo run --example basic-retry
    RUSTFLAGS="{{warning_flags}}" cargo run --example hooks-and-stats
    RUSTFLAGS="{{warning_flags}}" cargo run --example async-polling --features tokio-sleep

bench-no-run:
    cargo bench --bench {{benchmark_target}} --no-run

bench-no-run-strict:
    RUSTFLAGS="{{warning_flags}}" cargo bench --bench {{benchmark_target}} --no-run

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

pre-commit: fmt-check lint-typos

pre-push: lint-clippy test-strict doc-strict

ci: fmt-check lint test-strict test-no-default-strict test-doc-no-default-strict doc-strict check-no-std-strict check-wasm-strict test-readme-strict test-examples-strict bench-no-run-strict

ci-stable: build-stable test-stable lint-clippy-stable
