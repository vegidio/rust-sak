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
# index groups files by path, so `src/crypto`, `src/fetch` and `src/image` each show a
# per-directory subtotal in the combined report. `--by-feature` goes further and compiles +
# tests one feature at a time (`--no-default-features --features <f>`) so each summary
# reflects only that feature's own tests.
#
# `--ignore-filename-regex` drops `src/main.rs` — the scratch `playground` binary has no
# tests, so counting it would understate real coverage.
#
# Note: the `image` feature downloads prebuilt avif/heif/webp static binaries on first build
# (internet required, or set the `*_BINARIES_DIR` env vars for offline builds).
set -euo pipefail

cd "$(dirname "$0")/.."

# The scratch playground binary is not meaningfully coverable; keep it out of every report.
IGNORE_REGEX='src/main\.rs'
FEATURES=(crypto fetch image)

# Ensure the LLVM coverage tooling is available.
if ! cargo llvm-cov --version >/dev/null 2>&1; then
    echo "cargo-llvm-cov not found; installing it (one-time setup)..." >&2
    rustup component add llvm-tools-preview
    cargo install cargo-llvm-cov
fi

case "${1:-}" in
    --lcov)
        mkdir -p target/coverage
        cargo llvm-cov --all-features --ignore-filename-regex "$IGNORE_REGEX" \
            --lcov --output-path target/coverage/lcov.info
        echo "Wrote target/coverage/lcov.info"
        ;;
    --by-feature)
        for feature in "${FEATURES[@]}"; do
            echo
            echo "=== feature: $feature (src/$feature) ==="
            cargo llvm-cov --no-default-features --features "$feature" \
                --ignore-filename-regex "$IGNORE_REGEX" --summary-only
        done
        ;;
    "")
        cargo llvm-cov --all-features --ignore-filename-regex "$IGNORE_REGEX" --html --open
        echo "HTML report written to target/llvm-cov/html/index.html"
        ;;
    *)
        echo "Unknown option: $1" >&2
        echo "Usage: scripts/coverage.sh [--lcov | --by-feature]" >&2
        exit 1
        ;;
esac
