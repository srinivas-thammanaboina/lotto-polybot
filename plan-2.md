# Polymarket Latency Arbitrage Bot — Implementation Plan

## 1. Objective

Deliver a safe, measurable, replayable Rust/Tokio trading system for Polymarket short-duration BTC/ETH crypto markets.

The plan prioritizes:
- local development first
- deterministic replay before live trading
- simulation before paper shadowing
- tiny-size live rollout only after hard safety gates pass

---

## 2. Delivery Strategy

We will build this system in phased slices.
Each phase must leave the repo in a runnable, testable state.

### Success rules for every phase

- code compiles cleanly
- config is explicit
- logs are structured
- tests are added where possible
- no hidden behavior
- no hardcoded production fallback
- new modules are replay/test friendly

---

## 3. Phase Plan

## Phase 0 — Project Foundation

### Goals

Create the initial Rust/Tokio repo skeleton and development ergonomics.

### Deliverables

- Cargo workspace or single crate foundation
- `.env.example`
- config loading
- structured logging
- error handling conventions
- shared types module
- feature flags for modes
- benchmark scaffold
- testdata folder

### Output

A clean `cargo run` project with startup banner, config parse, and health output.

### Exit criteria

- app boots locally
- config validation works
- basic logging works
- CI can run fmt/lint/test skeleton

---

## Phase 1 — Market Discovery + Static Metadata

### Goals

Discover relevant Polymarket BTC/ETH 5m and 15m markets and maintain metadata cache.

### Deliverables

- Polymarket discovery adapter
- active market filtering
- market metadata cache
- no-trade fail-closed behavior if discovery fails
- market identity normalization

### Output

CLI/service can list currently eligible markets for the configured scope.

### Exit criteria

- discovers expected markets
- never falls back to hardcoded markets
- cache refresh logic works

---

## Phase 2 — External Feed Ingestion

### Goals

Bring in Binance as primary price feed and Coinbase as backup-only feed.

### Deliverables

- Binance WebSocket adapter
- Coinbase WebSocket adapter
- normalized event schema
- immediate receipt timestamping
- reconnect logic with backoff and jitter
- feed health metrics

### Output

Service can continuously ingest and normalize external price events.

### Exit criteria

- reconnect works
- stale-feed detection works
- event timestamps and health metrics are logged

---

## Phase 3 — Polymarket Feed Ingestion

### Goals

Ingest Polymarket market WebSocket, user WebSocket, and RTDS stream.

### Deliverables

- market WebSocket adapter
- user WebSocket adapter
- RTDS adapter
- normalized Polymarket event schema
- local book state representation
- order lifecycle event handling

### Output

Service maintains live market state and receives user/order updates.

### Exit criteria

- book updates apply correctly
- user/order events are captured
- RTDS is available for cross-checking

---

## Phase 4 — Fair Value Engine

### Goals

Create the first rule-based fair-value engine.

### Deliverables

- short-window price delta model
- simple momentum persistence logic
- optional volatility adjustment
- gross edge computation
- net edge computation after costs
- versioned strategy config

### Output

Service can compute fair value and net edge for each eligible market.

### Exit criteria

- fair value is deterministic for same input stream
- costs are included in net edge
- output is logged with model version

---

## Phase 5 — Signal Engine + Contract Locking

### Goals

Gate signals aggressively before they become order intents.

### Deliverables

- supported-market checks
- freshness checks
- liquidity checks
- confidence checks
- edge threshold checks
- contract dedupe lock
- cool-down logic
- intent object generation

### Output

Service emits order intents only when all preconditions pass.

### Exit criteria

- duplicate contract entries blocked
- stale data blocks signals
- insufficient liquidity blocks signals

---

## Phase 6 — Simulation Mode

### Goals

Run the whole strategy without sending real orders.

### Deliverables

- simulation fill model
- simulated P&L
- same telemetry schema as live
- mode separation
- recent-decision logging

### Output

The bot can run end-to-end on live feeds in simulation mode.

### Exit criteria

- no real submissions occur
- signals, fills, and P&L are visible
- telemetry matches expected schema

---

## Phase 7 — Replay Engine

### Goals

Support deterministic offline replay from captured event streams.

### Deliverables

- append-only event capture format
- replay runner
- deterministic clock model for replay
- scenario fixtures

### Output

Captured sessions can be replayed locally for debugging and validation.

### Exit criteria

- same inputs produce same decisions
- replay can reproduce key scenarios
- replay logs are useful for debugging

---

## Phase 8 — Execution Engine

### Goals

Add real order path through the Polymarket client layer.

### Deliverables

- internal client abstraction
- official SDK integration first
- alternative client benchmark harness if needed
- order building
- order signing
- order submission
- bounded retry logic
- uncertain-order-state handling
- cancel/replace handling

### Output

Orders can be built, submitted, and tracked through the execution engine.

### Exit criteria

- no direct strategy-to-order submission
- idempotent order IDs implemented
- retries are bounded and safe

---

## Phase 9 — Risk Engine + Kill Switch

### Goals

Make the bot safe enough to survive bad data and bad conditions.

### Deliverables

- position sizing modes
- exposure limits
- daily drawdown logic
- total drawdown logic
- consecutive-loss breaker
- stale-data regime detection
- latency regime detection
- top-level kill switch

### Output

The bot can refuse or stop trading under unsafe conditions.

### Exit criteria

- kill switch can halt execution path
- risk checks run before all submissions
- loss and drawdown counters work

---

## Phase 10 — Fill-State Feedback + Reconciliation

### Goals

Close the loop between execution, exposure, and startup state.

### Deliverables

- fill-state tracking
- open exposure feedback into gating
- startup reconciliation
- reconnect reconciliation
- unresolved-order handling

### Output

The bot has correct awareness of current orders and exposure.

### Exit criteria

- no double-positioning on reconnect
- startup fails closed if state is unclear
- exposure reflects pending and filled orders

---

## Phase 11 — Resolution Verifier

### Goals

Audit final contract outcomes from Polymarket’s own resolved market state.

### Deliverables

- post-expiry verifier
- final realized P&L computation
- audited outcome records
- loss-counter integration

### Output

The bot can verify final contract outcomes without relying on CEX spot.

### Exit criteria

- official outcome records are persisted
- realized P&L matches verified contract result

---

## Phase 12 — Paper Shadow Mode

### Goals

Run with real feeds and production-like orchestration while keeping risk tightly bounded.

### Deliverables

- paper mode plumbing
- paper/live separation
- mode dashboards
- shadow run reporting

### Output

The system can operate in paper or equivalent shadow mode with production-like telemetry.

### Exit criteria

- mode separation is unambiguous
- telemetry is stable
- operator controls are clear

---

## Phase 13 — Region Benchmarking and Deployment Readiness

### Goals

Choose the initial production region and deployment model using measurement, not assumption.

### Deliverables

- benchmark scripts
- Frankfurt, Amsterdam, Dublin measurements
- submit-to-ack measurements where possible
- reconnect stability comparison
- deployment runbook

### Output

Region decision backed by real metrics.

### Exit criteria

- candidate regions measured
- ops runbook written
- failover mode selected

---

## Phase 14 — Tiny Live Rollout

### Goals

Validate actual execution characteristics with minimal risk.

### Deliverables

- very small size live config
- live health checklist
- operator rollback procedure
- incident logging

### Output

First live run with strict caps and full telemetry.

### Exit criteria

- live run completes safely
- actual execution stats are recorded
- no unresolved state anomalies remain

---

## 4. Initial Technical Decisions

### Runtime
- Rust + Tokio

### External feeds
- Binance primary
- Coinbase backup-only for v1

### Polymarket interfaces
- market WS
- user WS
- RTDS
- official outcome verification from resolved market data

### Deployment
- start in Frankfurt
- benchmark Amsterdam and Dublin
- keep one-node primary first
- standby is benchmark-only or manual active-passive in v1

---

## 5. Risk Management Approach During Build

### Non-negotiable safeguards

- no live as default
- no hardcoded market fallback
- no blind resubmission after uncertain state
- no contract duplicate entries
- no CEX-price-based settlement verification
- no production rollout before replay and simulation gates pass

---

## 6. Benchmark and Quality Checklist

### Required repo quality

- rustfmt
- clippy
- unit tests
- integration hooks where practical
- docs/examples
- benchmark harness for hot path

### Required performance checks

- parse latency
- normalize latency
- fair-value latency
- signal latency
- intent build latency
- order submission path latency

---

## 7. Deliverables Summary

At the end of the initial program, the repo should contain:
- production-oriented Rust/Tokio architecture
- simulation mode
- replay engine
- safe execution path
- top-level kill switch
- startup reconciliation
- resolution verifier
- benchmark-backed deployment recommendation
- live rollout checklist

---

## 8. Final Definition of Done

The system is ready for controlled live use only when:
- supported markets are discovered correctly
- feeds ingest continuously
- replay is deterministic
- net edge is computed after costs
- signal gating blocks unsafe trades
- reconciliation works
- official outcome verification works
- risk controls work
- small-size live testing shows acceptable execution behavior
