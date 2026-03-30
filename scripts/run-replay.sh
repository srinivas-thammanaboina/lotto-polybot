#!/usr/bin/env bash
# Run replay against a captured session.
# Usage: ./scripts/run-replay.sh <session-file.jsonl>
set -euo pipefail

if [[ $# -lt 1 ]]; then
    echo "Usage: $0 <session-file.jsonl>"
    exit 1
fi

export BOT_MODE=dry_run
export RUST_LOG="${RUST_LOG:-info}"
export REPLAY_FILE="$1"

echo "Starting replay"
echo "  REPLAY_FILE=$REPLAY_FILE"
echo ""

cargo run --release
