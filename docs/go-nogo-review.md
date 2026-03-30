# Go / No-Go Review Checklist

This review determines whether to scale live trading, refine the strategy,
narrow scope, or stop.

## Evidence Required

All decisions must be based on measured data, not assumptions.

### 1. Net Edge After Fees and Slippage

- [ ] Average net edge (after all costs) is positive
- [ ] Net edge is positive for the most recent 50+ trades
- [ ] Actual slippage is within 2x of modeled slippage
- [ ] Fee model matches Polymarket's current schedule

**Source**: Evaluation report (`ReportBuilder::to_markdown`)

### 2. Latency Competitiveness

- [ ] Binance feed p95 latency < 5ms from chosen region
- [ ] Polymarket book p95 latency < 10ms
- [ ] Signal-to-submit p95 < 50ms
- [ ] Submit-to-ack p95 < 200ms (live only)

**Source**: Benchmark harness results (`BenchmarkResult`)

### 3. Execution Reliability

- [ ] Fill rate > 80% for submitted orders
- [ ] Reject rate from venue < 5%
- [ ] No uncertain order states left unresolved
- [ ] Reconciliation passes cleanly on restart

**Source**: Live validation logs, `OrderTracker` stats

### 4. Drawdown Behavior

- [ ] Max daily drawdown < configured limit (default $50)
- [ ] Max total drawdown < configured limit (default $100)
- [ ] No kill switch triggered by drawdown in last 24h
- [ ] Consecutive loss streak < 5

**Source**: `DrawdownTracker::snapshot()`

### 5. Feed Stability

- [ ] No reconnect storms in last 24h
- [ ] Feed staleness events < 5 in last hour
- [ ] Binance and Polymarket feeds both healthy > 99% of uptime
- [ ] Discovery refresh succeeding consistently

**Source**: `FeedHealthMonitor::snapshot()`, dashboard

### 6. Duration Scope Decision

- [ ] 5m regime: profitable in simulation? Y/N
- [ ] 5m regime: profitable in live validation? Y/N
- [ ] 15m regime: profitable in simulation? Y/N
- [ ] 15m regime: profitable in live validation? Y/N
- [ ] Decision: keep both / 5m only / 15m only / neither

### 7. Operator Confidence

- [ ] Runbooks reviewed and understood
- [ ] Kill switch tested manually
- [ ] Safe mode downgrade tested
- [ ] Monitoring/alerting in place (logs, dashboard)
- [ ] Backup plan if strategy fails (stop, downgrade, or narrow)

## Decision

| Outcome | Criteria |
|---------|----------|
| **GO — Scale** | All checks pass, positive net edge, stable feeds, operator confident |
| **GO — Narrow** | Positive for one regime only, disable the other |
| **REFINE** | Edge is marginal, slippage model needs calibration, retry after fixes |
| **STOP** | Negative net edge, unreliable execution, feed instability, or no edge after costs |

### Decision Record

```
Date: ___________
Decision: [ GO-SCALE / GO-NARROW / REFINE / STOP ]
Confidence: [ HIGH / MEDIUM / LOW ]

Justification:
_______________________________________________

Next steps:
_______________________________________________

Signed: ___________
```
