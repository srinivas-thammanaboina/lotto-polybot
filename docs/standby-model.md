# Standby Model — v1

## Model: Manual Active-Passive

Only one instance may submit orders at any time. There is **no automated
leader election** in v1.

### Rules

1. **Single writer**: Only the active instance may have `BOT_MODE=live` or
   `BOT_MODE=paper` with real order submission enabled.

2. **Standby runs benchmark-only**: The standby instance runs in `dry_run`
   or `simulation` mode. It collects latency/feed data but never submits
   orders.

3. **Manual failover**: If the active instance fails, the operator must:
   - Confirm the active instance is stopped (check process, check open orders
     via `scripts/run-benchmark.sh` or manual API query).
   - Run reconciliation on the standby to pick up any open orders/positions.
   - Switch the standby to active mode.

4. **No dual-write**: Two instances with `BOT_MODE=live` pointed at the same
   Polymarket account is **forbidden**. Contract locks are local to each
   instance and do not synchronize across processes.

### Future (v2+)

- Shared contract lock store (Redis / DynamoDB)
- Heartbeat-based leader election
- Automatic failover with fencing tokens

### Verification Checklist

Before promoting standby to active:

- [ ] Confirm previous active is fully stopped
- [ ] Check open orders via Polymarket API
- [ ] Run reconciliation (`Reconciler::reconcile()`)
- [ ] Verify kill switch is inactive
- [ ] Confirm feed health is green
- [ ] Switch mode and start
