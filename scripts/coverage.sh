#!/usr/bin/env bash
#
# Generate a code-coverage report for rust-sak using cargo-llvm-cov.
#
# Usage:
#   scripts/coverage.sh              # build an HTML report (all features) and open it
#   scripts/coverage.sh --lcov       # emit target/coverage/lcov.info instead (for tooling/CI)
#   scripts/coverage.sh --by-feature # per-feature summary: each module gated behind its own
#                                    # Cargo feature, tested in isolation
#
# rust-sak has no `default` feature set, so the full report uses `--all-features`. The HTML
# index groups files by path, so `src/crypto`, `src/fetch`, `src/fs`, `src/image`, `src/memo`, `src/o11y`
# and `src/sysinfo` each show a per-directory subtotal in the combined report. `--by-feature` goes further and compiles +
# tests one feature at a time (`--no-default-features --features <f>`) so each summary
# reflects only that feature's own tests.
#
# Note: the `sysinfo` feature needs no toolchain beyond a Rust compiler, but its GPU coverage is
# inherently partial off-Windows: the DXGI backend only compiles there, so only its parsers are
# exercised elsewhere. The `fs` feature compiles xz from vendored C sources on first build, so a C compiler
# is required. The `image` feature downloads prebuilt avif/heif/webp static binaries on first build
# (internet required, or set the `*_BINARIES_DIR` env vars for offline builds). The `memo` feature is listed with
# `memo-async` so the per-feature run also covers the async method, which `memo` alone compiles out.
#
# The workspace's other member, the `o11y-macros` proc-macro crate in `macros/`, is deliberately left out of every
# run here: a proc macro executes while the *consumer* compiles, so its line coverage measures nothing a test can
# influence and would only drag the headline number down. Its behaviour is covered through the `o11y` tests that
# use `#[instrument]`.
#
# Note also that the default (no-argument) run goes through `cargo test`, which puts the whole suite in one process
# rather than one process per test the way `cargo nextest` does. The `o11y` tests that touch its process globals
# serialise on a shared lock for exactly that reason.
set -euo pipefail

cd "$(dirname "$0")/.."

FEATURES=(crypto fetch fs image image-raw memo,memo-async o11y sysinfo)

# Ensure the LLVM coverage tooling is available.
if ! cargo llvm-cov --version >/dev/null 2>&1; then
    echo "cargo-llvm-cov not found; installing it (one-time setup)..." >&2
    rustup component add llvm-tools-preview
    cargo install cargo-llvm-cov
fi

case "${1:-}" in
    --lcov)
        mkdir -p target/coverage
        cargo llvm-cov --all-features --lcov --output-path target/coverage/lcov.info
        echo "Wrote target/coverage/lcov.info"
        ;;
    --by-feature)
        for feature in "${FEATURES[@]}"; do
            echo
            # `memo,memo-async` and `image-raw` both cover the directory their first word names.
            dir="${feature%%,*}"
            echo "=== feature: $feature (src/${dir%-raw}) ==="
            cargo llvm-cov --no-default-features --features "$feature" --summary-only
        done
        ;;
    "")
        cargo llvm-cov --all-features --html --open
        echo "HTML report written to target/llvm-cov/html/index.html"
        ;;
    *)
        echo "Unknown option: $1" >&2
        echo "Usage: scripts/coverage.sh [--lcov | --by-feature]" >&2
        exit 1
        ;;
esac
