# Operational Runbooks

## 1. Feed Disconnect

**Symptom**: Feed health shows `Disconnected` or `Reconnecting`.

**Actions**:
1. Check tracing logs for the specific feed (grep `feed_source`).
2. If Binance: check <https://www.binance.com/en/support> for outages.
3. If Polymarket: check their status page and WS URL config.
4. The bot auto-reconnects with exponential backoff. Monitor reconnect count.
5. If reconnects exceed 10 in 5 minutes, the kill switch may trigger
   (`ReconnectStorm`). Investigate upstream before deactivating.

**Resolution**: Usually self-healing. If persistent, restart the bot.

---

## 2. High Latency

**Symptom**: Dashboard shows elevated feed or decision latencies.

**Actions**:
1. Check benchmark histograms (p95, p99).
2. Compare with region benchmark baseline.
3. If network latency: check host provider status, consider region switch.
4. If decision latency: check CPU load, event bus backlog.
5. Kill switch triggers on `AbnormalLatency` if thresholds are breached.

**Resolution**: If transient, wait. If persistent, consider region migration.

---

## 3. Repeated Order Rejects

**Symptom**: High `signals_rejected` count or repeated `OrderState::Rejected`.

**Actions**:
1. Check reject reasons in logs (filter `signal_rejected`, `order_submit_failed`).
2. Common causes:
   - `BelowEdgeThreshold`: Market conditions may have changed.
   - `InsufficientLiquidity`: Thin books, reduce size or wait.
   - `ContractLocked`: Expected if position is open.
   - `ExecutionUnhealthy`: Check client connectivity.
3. If rejects are from the venue (not gate rejects): check API key validity,
   rate limits, balance.

**Resolution**: Depends on cause. Venue rejects need credential/balance check.

---

## 4. Kill Switch Triggered

**Symptom**: `KILL_SWITCH_ACTIVATED` in logs. No new trades.

**Actions**:
1. Check the trigger reason: `kill_switch_reason` in logs.
2. Common reasons and responses:
   - `DailyDrawdownBreach`: Review P&L. Wait for daily reset or manual deactivate.
   - `ConsecutiveLossBreach`: Review last N trades for pattern.
   - `StaleFeedRegime`: Check all feed connections.
   - `ExecutionFailures`: Check client/network health.
   - `Manual`: Operator triggered. Review why.
3. To deactivate: operator must call `KillSwitch::deactivate()` (via admin
   endpoint or restart with fresh state after confirming safety).

**Resolution**: Fix root cause, then deactivate. Never deactivate blindly.

---

## 5. Reconciliation Failure

**Symptom**: `reconciliation_unsafe` in logs at startup. Trading blocked.

**Actions**:
1. Check for unknown open orders on the venue.
2. Review `reconciliation_unknown_order` log entries.
3. Manually verify order status via Polymarket API.
4. If orders are from a previous session:
   - Cancel stale orders manually.
   - Restart and reconcile again.
5. If orders are from another instance: **stop the other instance first**
   (see standby model).

**Resolution**: Clear unknown orders, restart. Never force-start with unknown state.

---

## 6. Stale Market Discovery

**Symptom**: No new contracts found, or contracts appear expired.

**Actions**:
1. Check discovery refresh logs (`discovery_refresh`).
2. Verify Polymarket REST API is reachable.
3. Check `DISCOVERY_REFRESH_SECS` config (default 30s).
4. If 3 consecutive failures: discovery health degrades, trading is blocked.

**Resolution**: Usually transient API issues. Bot auto-recovers on next success.

---

## 7. Restart Procedure

**Safe restart steps**:

1. Send SIGINT (Ctrl-C) for graceful shutdown.
2. Wait for `shutdown complete` log message.
3. Verify no open orders remain (or accept reconciliation will handle them).
4. Start the bot with the appropriate run script.
5. Confirm reconciliation passes at startup.
6. Confirm feed health is green.
7. Confirm kill switch is inactive.

**Hard restart** (if graceful fails):

1. Send SIGTERM or SIGKILL.
2. On next start, reconciliation will detect any orphaned orders.
3. Review and resolve any `uncertain` orders.

---

## 8. Log Collection

**Local logs**: Written to stdout/stderr. Redirect to file:
```bash
./scripts/run-sim.sh 2>&1 | tee -a logs/bot.log
```

**JSON logs**: Set `LOG_JSON=true` for structured output:
```bash
LOG_JSON=true ./scripts/run-sim.sh 2>&1 >> logs/bot.jsonl
```

**Event persistence**: Events written to `data/events.jsonl` (configurable
via `EVENT_LOG_PATH`).

---

## 9. Safe Mode Downgrade

If live mode encounters issues, downgrade without full restart:

1. Trigger kill switch manually (`KillSwitch::activate(Manual)`).
2. All new trading stops immediately.
3. Existing orders remain (monitor for fills/cancels).
4. Investigate the issue.
5. If safe: deactivate kill switch to resume.
6. If not safe: stop the bot, restart in simulation mode.

**Mode hierarchy** (safest to riskiest):
`dry_run` → `simulation` → `paper` → `live`
