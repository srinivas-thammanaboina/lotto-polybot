#!/usr/bin/env bash
# Run the bot in shadow/paper mode.
# Real feeds, signal decisions logged, no real orders.
set -euo pipefail

export BOT_MODE=paper
export RUST_LOG="${RUST_LOG:-info}"
export LOG_JSON="${LOG_JSON:-true}"

echo "Starting poly-latency-bot in SHADOW/PAPER mode"
echo "  BOT_MODE=$BOT_MODE"
echo "  RUST_LOG=$RUST_LOG"
echo ""

cargo run --release
