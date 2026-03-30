#!/usr/bin/env bash
# Run the bot in simulation mode (default, safe).
# Uses live feeds but no real order submission.
set -euo pipefail

export BOT_MODE=simulation
export RUST_LOG="${RUST_LOG:-info}"
export LOG_JSON="${LOG_JSON:-false}"

echo "Starting poly-latency-bot in SIMULATION mode"
echo "  RUST_LOG=$RUST_LOG"
echo "  LOG_JSON=$LOG_JSON"
echo ""

cargo run --release
