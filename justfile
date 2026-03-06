set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

lint:
    cargo clippy --all-targets --all-features -- -D warnings

test:
    cargo test --all-targets

test-nextest:
    cargo nextest run --all-targets

test-no-default:
    cargo test --no-default-features

build:
    cargo build --all-targets

build-all-features:
    cargo build --all-targets --all-features

run-example example:
    cargo run --example {{example}}

doc:
    RUSTDOCFLAGS="-D warnings" cargo test --doc
    RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps

deny:
    cargo deny check advisories licenses bans sources

hack:
    cargo hack check --feature-powerset --optional-deps --no-dev-deps

typos:
    typos

taplo-fmt:
    taplo fmt

taplo-check:
    taplo fmt --check

targets:
    rustup target add thumbv7m-none-eabi wasm32-unknown-unknown

check-no-std:
    cargo build --target thumbv7m-none-eabi --no-default-features

check-wasm:
    cargo check --target wasm32-unknown-unknown --no-default-features --features alloc,gloo-timers-sleep

bench-no-run:
    cargo bench --bench retry_hot_paths --no-run

ci:
    just fmt-check
    just lint
    just test
    just test-no-default
    just deny
    just doc
    just check-wasm
