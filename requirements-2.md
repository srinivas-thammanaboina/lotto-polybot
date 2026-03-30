# Polymarket Latency Arbitrage Bot — Requirements

## 1. Purpose

Build a low-latency, event-driven trading system for Polymarket short-duration crypto markets that detects and trades temporary divergence between external crypto price movement and Polymarket tradable prices.

The system is for:
- research
- deterministic replay
- simulation
- paper trading
- tightly gated live deployment after safety validation

This system must be engineered around correctness, observability, and risk containment.

---

## 2. Product Scope

### 2.1 In-scope markets

Supported initial markets:
- BTC 5-minute up/down
- BTC 15-minute up/down
- ETH 5-minute up/down
- ETH 15-minute up/down

### 2.2 In-scope strategy modes

The architecture must support:
- latency arbitrage mode
- oracle-aware mode

Only one mode needs to be activated in the first live iteration.

### 2.3 External reference feeds

The bot shall support:
- Binance real-time feed adapter
- Coinbase real-time feed adapter
- optional Chainlink-aware path for oracle-aware logic and verification

### 2.4 Out of scope

The following are excluded from the initial version:
- copy trading
- news-driven trading
- general non-crypto prediction markets
- production market making
- multi-wallet routing
- mobile application
- Kubernetes / service mesh / distributed leader election

---

## 3. Runtime and Architecture

### 3.1 Runtime choice

The hot path shall be implemented in Rust with Tokio.

Hot path includes:
- websocket ingestion
- message parsing
- event normalization
- fair-value calculation
- signal generation
- order intent construction
- order submission
- order state tracking

Python may be used only for:
- notebooks
- research scripts
- offline analytics
- reporting
- utility tooling

### 3.2 Processing model

The hot path must be websocket-first and event-driven.
Polling is forbidden in the hot path.

### 3.3 Core modules

The system shall include:
- `market_discovery`
- `feed_cex`
- `feed_polymarket`
- `fair_value_engine`
- `signal_engine`
- `execution_engine`
- `risk_engine`
- `kill_switch`
- `resolution_verifier`
- `telemetry_engine`
- `replay_engine`

### 3.4 Client abstraction

The system shall isolate the Polymarket client behind an internal adapter interface so the team can benchmark:
- official Rust SDK path
- alternative client-layer implementations

No strategy code shall depend directly on a single SDK implementation.

### 3.5 Hot-path memory discipline

The hot path should aim for minimal steady-state allocation.
Requirements:
- reusable buffers where practical
- cached market metadata
- bounded book depth retention
- no remote blocking writes in hot path
- no per-trade metadata fetches that can be cached

---

## 4. Market and Contract Handling

### 4.1 Separate 5m and 15m strategy profiles

5-minute and 15-minute markets must be treated as distinct regimes with separate:
- thresholds
- sizing
- max-hold time
- stale-data tolerances
- liquidity minimums
- latency tolerances

### 4.2 Contract identity and deduplication

The bot must maintain an active contract registry keyed by market and token identity.

Before placing any order, the bot must check:
- whether a position already exists in that contract
- whether an order is already pending
- whether the contract is in a cool-down state

Duplicate entries into the same contract window must be blocked by default.

### 4.3 Contract lock lifecycle

A contract lock remains active until:
- the position is fully closed, or
- expiry plus configurable post-expiry buffer passes

### 4.4 Discovery behavior

Market discovery must fail closed.
The system must never silently fall back to a hardcoded market if discovery fails.

---

## 5. Feed Requirements

### 5.1 CEX feed requirements

The bot shall support:
- Binance as primary feed
- Coinbase as backup-only in v1

Coinbase must not add latency to the happy path in v1.

### 5.2 Polymarket feed requirements

The bot shall support:
- market WebSocket for order book / market state
- user WebSocket for order and trade lifecycle
- RTDS WebSocket for crypto reference stream cross-checking

### 5.3 RTDS role clarity

RTDS must be treated as:
- a verification / cross-check data source
- not the sole source of external truth for latency-arb entry in v1

### 5.4 Feed freshness

All inbound feed messages must be timestamped immediately on receipt.
The system must maintain freshness metrics and stale-feed alarms.

---

## 6. Fair Value and Signal Rules

### 6.1 Fair value model

The initial fair value model shall be rule-based, versioned, and logged.

It may include:
- short-window price delta
- momentum persistence
- volatility adjustment
- source-consensus check
- oracle-aware adjustment hooks

Machine learning is out of scope in the initial build.

### 6.2 Net-edge only

Signal generation must use net expected edge, not gross divergence.

Net edge must include:
- current fee model
- expected entry slippage
- expected exit slippage
- latency decay buffer
- spread and liquidity constraints

### 6.3 Dynamic fee model

The fee model must not be hardcoded to one static assumption.
It must support:
- dynamic price/probability-based fees
- market-category-specific fee rules
- future fee-schedule overrides through config
- simulation/replay override support

### 6.4 Signal confirmation

A trade candidate must pass all of:
- supported market
- supported regime
- non-stale external feed
- non-stale Polymarket book
- fair value confidence above threshold
- net edge above threshold
- sufficient liquidity
- no duplicate contract lock
- no kill switch active
- acceptable execution health

### 6.5 Signal and execution separation

Signal generation must output an order intent.
Signal logic must not submit orders directly.
All order submission shall flow through the execution engine.

---

## 7. Execution Requirements

### 7.1 Supported modes

The bot shall support:
- `dry_run`
- `simulation`
- `paper`
- `live`

Default mode must be `simulation` or `paper`, never `live`.

### 7.2 Live trading guards

Live mode must require:
- live credentials present
- live mode explicitly enabled
- environment confirmation enabled
- risk profile approved
- system-health checks passing

### 7.3 Order controls

The execution engine must support:
- idempotent client order IDs
- stale-signal rejection
- duplicate-order prevention
- bounded retry logic
- safe cancel/replace
- no blind resubmission after uncertain order state
- max order size caps
- per-market exposure caps
- max concurrent orders

### 7.4 Fill state feedback loop

Execution state must feed back into risk and gating.
Tracked states must include:
- ack
- partial fill
- full fill
- cancel pending
- canceled
- rejected
- retrying
- uncertain

New intents must consider current open exposure and pending order state.

### 7.5 Startup reconciliation

On startup or reconnect, the bot must reconcile:
- open orders
- open positions
- unresolved prior order intents
- current contract locks

No trading may begin until reconciliation succeeds or the bot fails closed.

---

## 8. Risk and Safety Requirements

### 8.1 Position sizing

Supported sizing modes:
- fixed notional
- percent-of-equity
- capped fractional Kelly
- max-notional cap
- max-per-market cap

Pure uncapped Kelly is forbidden.

### 8.2 Exposure controls

The system shall enforce:
- max position per market
- max concurrent positions
- max gross exposure
- max net BTC-linked exposure
- max net ETH-linked exposure
- max notional per time bucket

### 8.3 Loss controls

The system shall enforce:
- max daily drawdown
- max session drawdown
- max total drawdown
- max consecutive losses
- max losing trades per interval
- max adverse slippage per interval

### 8.4 Kill switch

The kill switch must be top-level and able to halt the entire order path.

It must support triggers including:
- manual kill
- daily drawdown breach
- total drawdown breach
- consecutive-loss breach
- repeated execution failure
- stale-data regime
- abnormal latency regime
- unresolved order-state anomaly
- repeated disconnect storm

### 8.5 Conservative defaults

Initial live defaults must be tighter than any assumptions derived from public anecdotes or social posts.

---

## 9. Resolution and Accounting

### 9.1 Official outcome verification only

Realized P&L and final outcome must be verified from Polymarket’s own resolved market state and official outcome data.

The system must not use:
- Binance spot snapshots
- Coinbase spot snapshots
- arbitrary local prices

as a substitute for final contract resolution.

### 9.2 Resolution verifier

A dedicated resolution verifier must run after contract expiry and final resolution.
It must:
- fetch official resolved outcome data
- compute final realized P&L
- update loss counters and risk metrics
- persist audited final outcome records

### 9.3 Mark-to-market

During contract life, the system may use Polymarket tradable prices for mark-to-market and active risk control.

### 9.4 Accounting records

The system must persist:
- entry decisions
- entry fills
- exit decisions
- exit fills
- fees paid
- slippage estimates
- realized P&L
- unrealized P&L
- official final outcome

---

## 10. Latency Requirements

### 10.1 Latency philosophy

The primary target is a sub-millisecond internal engine, not a sub-millisecond full internet round trip.

### 10.2 Internal latency budgets

Target on benchmark host:
- inbound normalization p99 < 250 microseconds
- fair value + signal p99 < 500 microseconds
- order intent construction p99 < 250 microseconds
- total internal hot path p99 < 1 millisecond

### 10.3 Required measured latencies

The system must continuously measure:
- source timestamp to bot receipt
- Polymarket timestamp to bot receipt
- internal decision time
- order-submit time
- submit-to-ack
- ack-to-fill
- decision-to-fill
- expected versus realized slippage

### 10.4 Stale-data gating

A trade must be blocked if:
- external feed age exceeds threshold
- Polymarket order-book age exceeds threshold
- clock skew exceeds threshold
- recent execution latency exceeds threshold
- reconnect is in progress
- confidence falls below regime minimum

---

## 11. Rate Limits and External Dependencies

### 11.1 WebSocket priority

Live data ingestion must prefer WebSocket over REST wherever possible.

### 11.2 Rate-limit handling

The bot must implement explicit handling for:
- market discovery APIs
- Polymarket public endpoints
- Polymarket CLOB endpoints
- external REST fallbacks

### 11.3 Backoff behavior

The bot shall implement:
- exponential backoff
- jitter
- sliding-window request budgeting
- per-endpoint rate accounting
- fail-closed behavior when request pressure becomes unsafe

### 11.4 Polygon-side dependency model

The architecture must include Polygon-side dependency support for:
- balance checks
- approval checks
- optional oracle reads
- relayer-aware operational flows

This dependency must not be assumed to be in the per-order hot path.

---

## 12. Persistence and Replay

### 12.1 Hot-path persistence

The hot path must not block on remote database writes.

### 12.2 Persisted records

Persist:
- raw inbound events
- normalized events
- features used for each signal
- rejected-trade reasons
- order lifecycle events
- fill events
- fee values
- risk-state transitions
- kill-switch events
- final market outcome

### 12.3 Replay guarantees

Replay mode must:
- preserve event ordering
- preserve timestamps
- preserve decision reproducibility
- produce deterministic signal output for the same input stream and config version

---

## 13. Observability Requirements

The system shall expose:
- current mode
- connectivity state
- feed health
- active positions
- open orders
- realized P&L
- unrealized P&L
- latency histograms
- slippage stats
- fill ratio
- reject counts
- stale-data counts
- circuit-breaker state
- per-market performance
- recent decisions
- replay compatibility status

A terminal dashboard is sufficient in the first release.

---

## 14. Infrastructure Requirements

### 14.1 Initial topology

Initial deployment shall be:
- one primary execution node
- optional one warm standby
- no Kubernetes
- no service mesh
- no remote dependency in hot path except venue and data connections

### 14.2 Benchmark-first region selection

Candidate regions must be benchmarked before live deployment.
Initial benchmark set:
- Frankfurt
- Amsterdam
- Dublin

### 14.3 Host class

Development may use cloud VMs.
Serious live testing should use:
- dedicated CPU or bare metal where practical
- synchronized clock
- minimal background services
- local append-only logs
- local health monitoring

### 14.4 Standby model definition

V1 standby must be explicitly one of:
- benchmark-only passive nodes, or
- manual active-passive failover

Automated leader-election failover is deferred.

### 14.5 No discovery fallback trading

If discovery, reconciliation, or venue eligibility checks fail, the bot must fail closed and not submit orders.

---

## 15. Testing Requirements

### 15.1 Must pass before live mode

- unit tests
- parser tests
- deterministic replay tests
- stale-feed tests
- disconnect/reconnect tests
- duplicate-order tests
- uncertain-order-state tests
- partial-fill tests
- no-fill tests
- latency budget tests
- risk limit tests
- kill-switch tests
- simulation soak run
- paper-trading shadow run

### 15.2 Replay scenario packs

Replay suites must include:
- normal repricing capture
- false signal
- sudden reversal
- partial fill
- no liquidity
- stale book
- exchange disconnect
- Polymarket disconnect
- burst volatility
- near-expiry chaos
- fee-regime change
- duplicate contract signal burst

### 15.3 Benchmark gates

The repo must include benchmark coverage for:
- parse/decode throughput
- order-book apply latency
- signal compute latency
- order intent build latency
- client-layer comparison where practical

---

## 16. Engineering Quality Gates

The repository must include:
- formatting enforcement
- linting
- unit and integration test support
- docs/examples
- benchmark smoke checks in CI where practical

---

## 17. Acceptance Criteria

The initial build is acceptable only if it can:
1. discover supported BTC/ETH 5m and 15m markets
2. ingest external and Polymarket feeds continuously
3. compute fair value and net edge in real time
4. block stale, duplicate, or unsafe trades
5. run end-to-end in simulation mode
6. replay saved sessions deterministically
7. enforce all risk limits
8. record sufficient telemetry to debug every decision
9. keep 5m and 15m strategy profiles separate
10. verify official market outcomes correctly
11. reconcile state correctly on startup and reconnect

---

## 18. Discovery Questions Before Scaling

The system must explicitly validate:
1. whether live net edge survives current fee regime
2. whether 5m markets are economically tradable for this infra
3. whether 15m markets are economically tradable for this infra
4. whether Coinbase adds measurable value beyond backup role
5. whether repricing exits outperform hold-to-resolution
6. whether current region choice is competitive
7. whether latency arb or oracle-aware mode is superior for this setup
8. whether realized slippage destroys expected edge
9. whether the opportunity window is still large enough after costs
10. whether the strategy remains worth production deployment at all
