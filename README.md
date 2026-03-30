# Polymarket Latency Arbitrage Bot

Rust/Tokio trading system for short-duration Polymarket crypto markets, designed for low-latency signal processing, deterministic replay, simulation, paper trading, and tightly gated live deployment.

## Current Status

**All 10 development phases are implemented.** 260+ unit tests, clippy clean.

| Phase | Status | Description |
|-------|--------|-------------|
| 0-1 | Done | Project foundation, domain types, event bus |
| 2 | Done | Market discovery + contract registry |
| 3 | Done | Feed adapters (Binance, Coinbase, Polymarket WS) |
| 4 | Done | Fair value engine, edge model, signal gates, sizing |
| 5 | Done | Execution engine, fill state, reconciliation |
| 6 | Done | Risk engine, kill switch, contract locking, drawdown |
| 7 | Done | Telemetry, persistence, replay, resolution verifier |
| 8 | Done | Simulation, shadow mode, evaluation reports |
| 9 | Done | Benchmarks, deployment scripts, runbooks |
| 10 | Done | Live validation guard, comparison tooling, go/no-go |

**Next steps:** Run simulation against live feeds, shadow mode validation, then go/no-go review.

## Goal

Build an event-driven bot that:
- monitors short-duration BTC/ETH Polymarket markets
- ingests external crypto price feeds in real time
- computes fair value versus Polymarket tradable prices
- trades only when expected edge remains positive after fees, slippage, and latency decay
- prioritizes safety, observability, and replayability over hype metrics

## Core Design Principles

- **Rust/Tokio in the hot path**
- **WebSocket-first**, not polling-first
- **Net edge only**: all decisions after fees, slippage, and latency decay
- **Fail closed** on stale data, duplicate exposure, or uncertain order state
- **Replay-first engineering**: every important decision should be reproducible offline
- **Simulation before live**
- **Tiny live rollout after simulation and paper shadowing**

## Repository Layout

```text
poly-latency-bot/
├── README.md
├── CLAUDE.md                  # Quality commands and rules
├── Cargo.toml
├── development.md             # Full 10-phase development plan
├── requirements-2.md          # Detailed requirements
├── plan-2.md                  # Architecture plan
├── skill.md                   # Domain knowledge reference
├── src/
│   ├── main.rs                # Thin entry point
│   ├── app.rs                 # Main event loop wiring
│   ├── config.rs              # Typed config from env vars
│   ├── types.rs               # BotEvent enum, CexTick, etc.
│   ├── metrics.rs             # Atomic counters
│   ├── error.rs               # Top-level error types
│   ├── shutdown.rs            # Graceful SIGINT/SIGTERM
│   ├── discovery/             # Gamma API, ContractRegistry
│   ├── domain/                # Market, Order, Signal, Position types
│   ├── feeds/                 # Binance, Coinbase, Polymarket WS adapters
│   ├── strategy/              # Fair value, edge, gates, sizing, pipeline
│   ├── execution/             # Client abstraction, submit, fill state, reconciliation
│   ├── risk/                  # Kill switch, contract locks, limits, drawdown
│   ├── telemetry/             # Persistence, dashboard, histograms, ledger
│   ├── replay/                # Recorder, runner, scenario fixtures
│   ├── resolution/            # Outcome verifier, resolution fetcher
│   ├── simulation/            # Sim engine, shadow mode, evaluation reports
│   ├── benchmark/             # Region benchmark harness
│   └── validation/            # Live guard, sim-vs-live comparison
├── scripts/
│   ├── check.sh               # cargo fmt + clippy + test
│   ├── run-sim.sh             # Simulation mode
│   ├── run-shadow.sh          # Shadow/paper mode
│   ├── run-benchmark.sh       # Region benchmark
│   ├── run-replay.sh          # Replay from session file
│   └── run-live.sh            # Live mode (requires CONFIRM_LIVE=yes)
├── docs/
│   ├── standby-model.md       # Manual active-passive v1
│   ├── runbooks.md            # 9 operational runbooks
│   └── go-nogo-review.md      # Go/no-go checklist
├── codereviews/               # Code review findings + fix tracker
└── benches/                   # Criterion benchmarks
```

## Quick Start

```bash
# Run all quality checks
bash scripts/check.sh

# Start in simulation mode (default, safe)
./scripts/run-sim.sh

# Run shadow mode against live feeds
./scripts/run-shadow.sh

# Run region benchmark
REGION_TAG=local ./scripts/run-benchmark.sh
```

## Modes

| Mode | Orders | Feeds | Description |
|------|--------|-------|-------------|
| `dry_run` | None | None | Config validation only |
| `simulation` | Simulated | Live | Full pipeline, synthetic fills |
| `paper` | Logged only | Live | Real signals, no submission |
| `live` | Real | Live | Requires `CONFIRM_LIVE=yes` + credentials |

Default is always `simulation`.

## Safety Model

The bot stops trading on:
- stale data regime
- repeated execution anomalies
- daily/total drawdown breach
- consecutive loss breach
- repeated disconnects (reconnect storm)
- unresolved order-state anomalies
- manual operator kill switch

## Related Docs

- [development.md](./development.md) — Full 10-phase build plan
- [requirements-2.md](./requirements-2.md) — Detailed requirements
- [plan-2.md](./plan-2.md) — Architecture plan
- [docs/runbooks.md](./docs/runbooks.md) — Operational runbooks
- [docs/standby-model.md](./docs/standby-model.md) — Standby model
- [docs/go-nogo-review.md](./docs/go-nogo-review.md) — Go/no-go checklist
