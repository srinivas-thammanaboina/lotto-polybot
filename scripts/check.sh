#!/usr/bin/env bash
set -euo pipefail

echo "==> formatting"
cargo fmt -- --check

echo "==> linting"
cargo clippy --all-targets --all-features -- -D warnings

echo "==> tests"
cargo test

echo "==> all checks passed"
