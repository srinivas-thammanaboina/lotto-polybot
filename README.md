# Polymarket Latency Arbitrage Bot

Rust/Tokio trading system for short-duration Polymarket crypto markets, designed for low-latency signal processing, deterministic replay, simulation, paper trading, and tightly gated live deployment.

## Goal

Build an event-driven bot that:
- monitors short-duration BTC/ETH Polymarket markets
- ingests external crypto price feeds in real time
- computes fair value versus Polymarket tradable prices
- trades only when expected edge remains positive after fees, slippage, and latency decay
- prioritizes safety, observability, and replayability over hype metrics

This project is intentionally **not** based on social-media win-rate claims.

## Initial Strategy Scope

V1 focuses on:
- BTC 5-minute up/down markets
- BTC 15-minute up/down markets
- ETH 5-minute up/down markets
- ETH 15-minute up/down markets
- latency arbitrage mode
- oracle-aware hooks in architecture, but not necessarily enabled in first live iteration

Out of scope for initial build:
- copy trading
- general prediction markets
- news-driven trading
- production market making
- multi-account routing
- Kubernetes / microservice sprawl

## Core Design Principles

- **Rust/Tokio in the hot path**
- **WebSocket-first**, not polling-first
- **Net edge only**: all decisions after fees, slippage, and latency decay
- **Fail closed** on stale data, duplicate exposure, or uncertain order state
- **Replay-first engineering**: every important decision should be reproducible offline
- **Simulation before live**
- **Tiny live rollout after simulation and paper shadowing**

## High-Level Architecture

### Main runtime modules

- `market_discovery`
  - discovers active BTC/ETH 5m and 15m markets
  - caches contract metadata

- `feed_cex`
  - Binance WebSocket adapter
  - Coinbase WebSocket adapter, initially backup-only

- `feed_polymarket`
  - market WebSocket
  - user WebSocket
  - RTDS WebSocket

- `fair_value_engine`
  - computes implied fair probability
  - estimates gross edge and net edge

- `signal_engine`
  - validates thresholds, staleness, liquidity, and confidence

- `execution_engine`
  - builds, signs, submits, tracks, and reconciles orders

- `risk_engine`
  - position sizing
  - exposure controls
  - drawdown controls
  - contract lock / dedupe logic

- `kill_switch`
  - top-level trading stop for safety events

- `resolution_verifier`
  - validates official market outcome after expiry using Polymarket’s own resolved market state

- `telemetry_engine`
  - structured logs
  - latency histograms
  - fill statistics
  - risk-state changes

- `replay_engine`
  - deterministic playback of captured sessions

## Deployment Model

### Initial primary region

Start with one primary execution node in **Frankfurt** for low operational friction and strong EU connectivity.

### Benchmark regions

Benchmark before locking in production region:
- Frankfurt
- Amsterdam
- Dublin

### Warm standby

Initial standby mode should be one of:
- benchmark-only passive probes, or
- manual active-passive failover

Avoid automated leader election in v1.

## Latency Philosophy

The realistic target is:
- **sub-millisecond internal engine**
- not sub-millisecond full internet round trip

The bot must measure continuously:
- source timestamp to bot receipt
- internal decision latency
- order submission latency
- submit-to-ack
- ack-to-fill
- expected versus realized slippage

## Safety Model

The bot must never assume:
- feed freshness
- order success
- fill completeness
- stable fee regime
- identical live and simulation execution behavior

The bot must stop trading on:
- stale data regime
- repeated execution anomalies
- excessive drawdown
- repeated disconnects
- unresolved order-state anomalies
- manual operator kill

## Expected Repository Layout

```text
poly-latency-bot/
├── README.md
├── requirements.md
├── plan.md
├── Cargo.toml
├── .env.example
├── configs/
│   ├── base.toml
│   ├── markets.toml
│   ├── risk.toml
│   └── regions.toml
├── src/
│   ├── main.rs
│   ├── app.rs
│   ├── config/
│   ├── types/
│   ├── market_discovery/
│   ├── feed_cex/
│   ├── feed_polymarket/
│   ├── fair_value_engine/
│   ├── signal_engine/
│   ├── execution_engine/
│   ├── risk_engine/
│   ├── kill_switch/
│   ├── resolution_verifier/
│   ├── telemetry_engine/
│   ├── replay_engine/
│   └── utils/
├── benches/
├── tests/
├── testdata/
└── scripts/
```

## Modes

The system must support:
- `dry_run`
- `simulation`
- `paper`
- `live`

Default must never be `live`.

## Polymarket-Specific Notes

- Use Polymarket market and user WebSockets for live market/order state
- Use RTDS as an additional reference stream and cross-check path
- Use Polymarket’s official resolved market data for final outcome verification
- Do not verify final P&L from Binance or Coinbase spot snapshots
- Treat fee handling as dynamic, not hardcoded

## Recommended Build Order

1. Rust/Tokio skeleton
2. config + logging + types
3. Polymarket market discovery
4. Binance feed adapter
5. Polymarket market WebSocket adapter
6. fair value engine
7. signal gates
8. simulation mode
9. replay engine
10. execution engine
11. risk engine + kill switch
12. resolution verifier
13. paper shadow mode
14. tiny live rollout

## What Success Looks Like

Early success is:
- correct market discovery
- correct feed ingestion
- deterministic replay
- accurate net-edge accounting
- safe order controls
- strong telemetry
- stable simulation and paper shadow runs

Early success is not:
- huge trade counts
- screenshot P&L
- unsupported win-rate claims

## Related Docs

- [requirements.md](./requirements.md)
- [plan.md](./plan.md)
