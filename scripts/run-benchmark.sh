#!/usr/bin/env bash
# Run the region benchmark harness.
# Collects feed latency, decision latency, and connectivity quality.
set -euo pipefail

export BOT_MODE=dry_run
export RUST_LOG="${RUST_LOG:-info}"
export REGION_TAG="${REGION_TAG:-local}"
export BENCHMARK_DURATION_SECS="${BENCHMARK_DURATION_SECS:-60}"

echo "Starting benchmark harness"
echo "  REGION_TAG=$REGION_TAG"
echo "  BENCHMARK_DURATION_SECS=$BENCHMARK_DURATION_SECS"
echo ""

cargo run --release
