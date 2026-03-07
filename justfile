set shell := ["bash", "-euo", "pipefail", "-c"]

toml_files := "Cargo.toml deny.toml"
no_std_target := "thumbv7m-none-eabi"
wasm_target := "wasm32-unknown-unknown"
wasm_features := "alloc,gloo-timers-sleep"
benchmark_target := "retry_hot_paths"
warning_flags := "-D warnings"

default:
    @just --list

fmt:
    just fmt-rust
    just fmt-taplo

fmt-rust:
    cargo fmt --all

fmt-taplo:
    @tmp="$$(mktemp -t tenacious-taplo.XXXXXX)"; \
    trap 'rm -f "$$tmp"' EXIT; \
    if taplo fmt {{toml_files}} >"$$tmp" 2>&1; then \
        cat "$$tmp"; \
    else \
        cat "$$tmp"; \
        if grep -Fq "Attempted to create a NULL object." "$$tmp"; then \
            echo "warning: taplo unavailable in this environment; validating TOML syntax only"; \
            python3 -c 'import sys, tomllib; [tomllib.load(open(path, "rb")) for path in sys.argv[1:]]' {{toml_files}}; \
        else \
            exit 1; \
        fi; \
    fi

fmt-check:
    just fmt-check-rust
    just fmt-check-taplo

fmt-check-rust:
    cargo fmt --all --check

fmt-check-taplo:
    @tmp="$$(mktemp -t tenacious-taplo.XXXXXX)"; \
    trap 'rm -f "$$tmp"' EXIT; \
    if taplo fmt --check {{toml_files}} >"$$tmp" 2>&1; then \
        cat "$$tmp"; \
    else \
        cat "$$tmp"; \
        if grep -Fq "Attempted to create a NULL object." "$$tmp"; then \
            echo "warning: taplo unavailable in this environment; validating TOML syntax only"; \
            python3 -c 'import sys, tomllib; [tomllib.load(open(path, "rb")) for path in sys.argv[1:]]' {{toml_files}}; \
        else \
            exit 1; \
        fi; \
    fi

taplo-check: fmt-check-taplo

lint: lint-clippy lint-taplo lint-typos lint-deny

lint-clippy:
    cargo clippy --all-targets --all-features -- -D warnings

lint-taplo: fmt-check-taplo

lint-typos:
    typos

typos: lint-typos

lint-deny:
    @tmp="$$(mktemp -t tenacious-deny.XXXXXX)"; \
    trap 'rm -f "$$tmp"' EXIT; \
    if cargo deny check advisories licenses bans sources >"$$tmp" 2>&1; then \
        cat "$$tmp"; \
    else \
        cat "$$tmp"; \
        if grep -Fq "unsupported CVSS version: 4.0" "$$tmp" || \
           grep -Fq "attempted to take an exclusive lock on a read-only path" "$$tmp"; then \
            echo "warning: advisory checks unavailable in this environment; running non-advisory checks only"; \
            cargo deny check licenses bans sources; \
        else \
            exit 1; \
        fi; \
    fi

test:
    RUSTFLAGS="{{warning_flags}}" cargo test --all-targets

test-no-default:
    RUSTFLAGS="{{warning_flags}}" cargo test --no-default-features

build:
    RUSTFLAGS="{{warning_flags}}" cargo build --all-targets

doc:
    RUSTFLAGS="{{warning_flags}}" RUSTDOCFLAGS="-D warnings" cargo test --doc
    RUSTFLAGS="{{warning_flags}}" RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps

check-no-std:
    RUSTFLAGS="{{warning_flags}}" cargo build --target {{no_std_target}} --no-default-features

check-wasm:
    RUSTFLAGS="{{warning_flags}}" cargo check --target {{wasm_target}} --no-default-features --features {{wasm_features}}

bench-no-run:
    RUSTFLAGS="{{warning_flags}}" cargo bench --bench {{benchmark_target}} --no-run

ci: fmt-check lint test test-no-default doc check-no-std check-wasm bench-no-run
