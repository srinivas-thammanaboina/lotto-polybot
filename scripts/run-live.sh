#!/usr/bin/env bash
# Run the bot in LIVE mode — REAL ORDERS.
# Requires explicit credentials and operator confirmation.
set -euo pipefail

# Safety check: require explicit confirmation
if [[ "${CONFIRM_LIVE:-}" != "yes" ]]; then
    echo "ERROR: Live mode requires explicit confirmation."
    echo "  Set CONFIRM_LIVE=yes to proceed."
    echo ""
    echo "  Example: CONFIRM_LIVE=yes ./scripts/run-live.sh"
    exit 1
fi

# Require credentials
: "${POLYMARKET_API_KEY:?POLYMARKET_API_KEY is required for live mode}"
: "${POLYMARKET_SECRET:?POLYMARKET_SECRET is required for live mode}"
: "${POLYMARKET_PASSPHRASE:?POLYMARKET_PASSPHRASE is required for live mode}"

export BOT_MODE=live
export RUST_LOG="${RUST_LOG:-info}"
export LOG_JSON=true

echo "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
echo "!!  STARTING IN LIVE MODE — REAL ORDERS WILL  !!"
echo "!!  BE SUBMITTED TO POLYMARKET                !!"
echo "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
echo ""
echo "  REGION_TAG=${REGION_TAG:-local}"
echo "  RISK_MAX_NOTIONAL_ORDER=${RISK_MAX_NOTIONAL_ORDER:-25}"
echo "  RISK_MAX_GROSS_EXPOSURE=${RISK_MAX_GROSS_EXPOSURE:-200}"
echo ""

cargo run --release
