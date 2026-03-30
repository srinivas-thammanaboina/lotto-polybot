# development.md — Polymarket Latency Arbitrage Bot Development Plan

## 1. Objective

Build a low-latency, event-driven Rust/Tokio trading system for Polymarket short-duration crypto markets.

Initial scope:
- BTC 5-minute up/down markets
- BTC 15-minute up/down markets
- ETH 5-minute up/down markets
- ETH 15-minute up/down markets

Strategy scope for the first implementation:
- latency arbitrage mode
- oracle-aware hooks designed in the architecture but not fully optimized until later phases

Primary build principles:
- simulation first
- replayable by design
- fail closed
- measure everything
- keep the hot path small
- prefer correctness and observability over premature optimization

---

## 2. Delivery Philosophy

### 2.1 Build order
The system must be developed in this order:
1. repository and standards
2. runtime skeleton
3. data ingestion
4. deterministic simulation
5. strategy logic
6. execution plumbing
7. risk controls
8. telemetry and replay
9. deployment and live shadowing
10. tiny live validation

### 2.2 Non-negotiable constraints
- No live trading before replay and simulation are stable.
- No polling-driven strategy logic in the hot path.
- No hardcoded fallback market in production code.
- No financial logic built on floats when fixed precision is required.
- No blocking remote writes in the hot path.
- No duplicate order submission without explicit idempotency rules.

### 2.3 Delivery artifacts
By the end of the program, the repo should contain:
- `README.md`
- `requirements.md`
- `plan.md`
- `development.md`
- `.env.example`
- Rust workspace or single crate with clear module boundaries
- deterministic replay fixtures
- simulation mode
- paper/live mode separation
- deployment notes
- benchmark notes
- runbooks

---

## 3. Target Repository Shape

```text
poly-latency-bot/
  Cargo.toml
  Cargo.lock
  README.md
  requirements.md
  plan.md
  development.md
  .env.example
  rust-toolchain.toml                # optional
  src/
    main.rs
    lib.rs
    app.rs
    config.rs
    error.rs
    types.rs
    shutdown.rs
    metrics.rs
    domain/
      mod.rs
      market.rs
      contract.rs
      signal.rs
      order.rs
      position.rs
      ledger.rs
    discovery/
      mod.rs
      gamma.rs
      cache.rs
    feeds/
      mod.rs
      binance.rs
      coinbase.rs
      polymarket_market.rs
      polymarket_user.rs
      polymarket_rtds.rs
      normalization.rs
    strategy/
      mod.rs
      fair_value.rs
      edge.rs
      filters.rs
      sizing.rs
    execution/
      mod.rs
      client.rs
      intents.rs
      signer.rs
      submit.rs
      reconciliation.rs
      fill_state.rs
    risk/
      mod.rs
      limits.rs
      kill_switch.rs
      contract_lock.rs
      drawdown.rs
    telemetry/
      mod.rs
      logging.rs
      persistence.rs
      histograms.rs
      dashboard.rs
    replay/
      mod.rs
      recorder.rs
      runner.rs
      scenarios.rs
    resolution/
      mod.rs
      verifier.rs
  tests/
    integration_discovery.rs
    integration_feeds.rs
    integration_simulation.rs
    integration_execution.rs
  benches/
    parse_latency.rs
    hot_path.rs
  scripts/
    run_local.sh
    replay_session.sh
    shadow_mode.sh
```

---

## 4. Phase Plan

# Phase 0 — Project Foundation and Standards

## Goal
Create a clean, repeatable Rust/Tokio project foundation so every later feature lands in the right place.

---

### Task 0.1 — Create repository skeleton

**Title**  
As a developer, I want a clean Rust project structure so later work is modular and maintainable.

**Description**  
Create the initial repository structure using either a single crate or a workspace if you know multiple crates will be needed. Keep `main.rs` thin and place reusable logic in `lib.rs` and modules. Add the folder layout shown above or a close equivalent. Create placeholder modules for discovery, feeds, strategy, execution, risk, telemetry, replay, and resolution. Add `.gitignore`, `.env.example`, and a short bootstrap README section.

Development instructions:
- Prefer a single crate first unless there is a strong reason for a workspace.
- Add placeholder files with minimal compile-ready stubs.
- Ensure the project builds successfully before any feature logic is added.

**Dependencies**  
None.

**Acceptance Criteria**
- `cargo check` passes.
- The project contains all top-level folders/modules needed for future phases.
- `main.rs` contains only startup wiring, not business logic.
- `.env.example` exists.
- No placeholder logic causes compile errors.

---

### Task 0.2 — Add baseline dependencies and toolchain standards

**Title**  
As a developer, I want a consistent dependency baseline so the team and any agent can build safely.

**Description**  
Add core crates needed for the architecture:
- `tokio`
- `serde`, `serde_json`
- `tracing`, `tracing-subscriber`
- `thiserror`
- `reqwest`
- `tokio-tungstenite`
- `futures-util`
- `dotenvy`
- `rust_decimal`
- `clap` if CLI entrypoints are needed
- `tokio-util` for cancellation/utilities
- `uuid` if needed for order IDs

Do not add crates that are not justified yet. Add dev dependencies for tests and benchmarks only when needed.

Development instructions:
- Keep versions current and compatible.
- Avoid experimental/nightly-only crates.
- Document why any performance-specific crate is added.
- Do not add alternative HTTP/WS clients unless benchmarking requires it.

**Dependencies**  
Task 0.1

**Acceptance Criteria**
- `Cargo.toml` is clean and documented.
- Dependency list supports the planned architecture.
- No redundant crate overlaps exist without reason.
- `cargo check` and `cargo test` still pass.

---

### Task 0.3 — Add formatting, linting, and quality gates

**Title**  
As a developer, I want automated quality gates so the codebase stays consistent as it grows.

**Description**  
Set up formatting and linting standards. Add instructions to run:
- `cargo fmt`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`

Optionally add GitHub Actions or local scripts, but keep the first version simple. The goal is to make quality checks unavoidable.

Development instructions:
- Resolve lint issues instead of suppressing them unless suppression is justified.
- Keep warning-free builds as the default quality bar.

**Dependencies**  
Task 0.2

**Acceptance Criteria**
- Formatting command works.
- Clippy passes with no warnings or only justified exceptions.
- Test command works.
- Quality commands are documented in README.

---

### Task 0.4 — Add structured config loading

**Title**  
As a developer, I want validated config loading so the service fails fast on bad runtime settings.

**Description**  
Create a typed configuration layer that reads:
- environment variables
- optional local `.env`
- optional config file if needed later

Config domains should include:
- endpoints
- credentials
- mode flags
- timeouts
- reconnect policy
- strategy thresholds
- risk limits
- telemetry settings
- storage paths
- deployment tags
- latency thresholds

Development instructions:
- Validate required fields at startup.
- Separate simulation defaults from live settings.
- Never silently use production-looking defaults for live credentials.
- Add helper methods for mode checks and validation.

**Dependencies**  
Task 0.1, Task 0.2

**Acceptance Criteria**
- Startup fails with clear messages when required config is missing.
- All config is represented with typed Rust structs.
- `.env.example` documents all required variables.
- Simulation and live modes are distinguishable in config.

---

### Task 0.5 — Add tracing and startup/shutdown framework

**Title**  
As a developer, I want structured logs and clean lifecycle handling so the bot is observable from day one.

**Description**  
Add `tracing`-based logging and graceful shutdown support. Create startup logs that show mode, region tag, enabled feeds, and core thresholds. Add shutdown handling for SIGINT/SIGTERM. Add a cancellation token or equivalent pattern.

Development instructions:
- Use structured fields instead of string-heavy logs.
- Avoid `println!` except in quick local tests.
- Log startup config summary without exposing secrets.
- Log clean shutdown and task termination status.

**Dependencies**  
Task 0.2, Task 0.4

**Acceptance Criteria**
- Logs are structured and readable.
- Shutdown stops tasks cleanly.
- Startup logs show safe runtime summary.
- No secrets are printed.

---

# Phase 1 — Core Runtime and Internal Contracts

## Goal
Establish the internal data types, event bus, and orchestration model that every component will use.

---

### Task 1.1 — Define domain types

**Title**  
As a developer, I want strong domain types so market, signal, order, and position logic is safe and explicit.

**Description**  
Define domain structs/enums for:
- asset
- market kind
- contract metadata
- token IDs
- price/probability
- depth levels
- signal
- order intent
- order state
- position state
- risk state
- resolution state
- latency samples
- feed health

Development instructions:
- Use `rust_decimal` or integer fixed units for money/price-sensitive values.
- Avoid free-form strings for domain concepts that can be typed.
- Keep transport payloads separate from domain models.

**Dependencies**  
Phase 0 complete

**Acceptance Criteria**
- Core domain types compile and are reusable across modules.
- No raw-string-heavy domain logic exists where typed enums should be used.
- Monetary/probability values avoid unsafe float use in business logic.

---

### Task 1.2 — Define internal event model

**Title**  
As a developer, I want a unified internal event schema so all feeds and engines can communicate consistently.

**Description**  
Create normalized internal events for:
- external trade tick
- external book update
- Polymarket market update
- Polymarket user/order update
- RTDS update
- signal generated
- order intent
- order ack
- fill event
- risk event
- kill switch event
- resolution event

Development instructions:
- Timestamp on receipt immediately.
- Preserve original source metadata where useful.
- Use one normalized schema and map transport-specific payloads into it.

**Dependencies**  
Task 1.1

**Acceptance Criteria**
- Internal events are typed and serializable where needed.
- Every later module can use these events without transport-specific coupling.
- Receipt timestamp is part of the model.

---

### Task 1.3 — Build task orchestration shell

**Title**  
As a developer, I want a predictable async orchestration shell so the service can run multiple adapters cleanly.

**Description**  
Create the main application orchestration pattern using Tokio tasks and channels. Separate these task groups:
- discovery
- feed adapters
- signal engine
- execution engine
- risk engine
- telemetry
- replay/simulation control

Development instructions:
- Prefer channels over shared mutable global state.
- Use bounded channels where appropriate.
- Design for clear ownership.
- Add cancellation propagation.

**Dependencies**  
Task 1.2, Task 0.5

**Acceptance Criteria**
- App starts and runs task groups without real logic yet.
- Tasks can be shut down cleanly.
- Channel ownership is clear.
- No giant shared mutable singleton is required.

---

# Phase 2 — Market Discovery and Metadata Cache

## Goal
Discover supported Polymarket contracts, validate scope, and maintain trusted metadata.

---

### Task 2.1 — Implement market discovery adapter

**Title**  
As a developer, I want automated market discovery so the bot can identify active BTC/ETH 5m and 15m contracts.

**Description**  
Build a discovery adapter against the relevant Polymarket/Gamma endpoints. Discover only supported contracts and filter out everything else. Track:
- market ID
- token IDs
- asset
- duration
- expiry
- outcome labels
- tradability status
- relevant pricing metadata

Development instructions:
- Discovery must fail closed if the endpoint is unavailable.
- Do not hardcode a fallback live market.
- Cache discovery results locally in memory with refresh rules.
- Keep discovery out of the hot path.

**Dependencies**  
Phase 1 complete

**Acceptance Criteria**
- Supported BTC/ETH 5m and 15m markets are discovered automatically.
- Unsupported markets are rejected.
- No hardcoded fallback market is used in production mode.
- Discovery results are cached and refreshable.

---

### Task 2.2 — Build contract registry and metadata cache

**Title**  
As a developer, I want a contract registry so feed, strategy, and execution logic all reference the same market truth.

**Description**  
Create an in-memory registry keyed by contract/token identity. This registry should store:
- metadata
- active/inactive status
- expiry
- cached fee-related settings if available
- lock state
- latest resolution state if closed

Development instructions:
- Separate registry updates from signal logic.
- Make reads fast.
- Keep writes controlled and auditable.

**Dependencies**  
Task 2.1

**Acceptance Criteria**
- Registry can answer market lookups quickly.
- Contract identity is stable across modules.
- Closed/expired markets are marked appropriately.
- Registry updates do not require hot-path REST calls.

---

### Task 2.3 — Add discovery health and fail-closed behavior

**Title**  
As an operator, I want discovery failures to stop unsafe trading so the bot never trades against stale or unknown market metadata.

**Description**  
Add health rules to discovery:
- missing markets
- duplicate markets
- unexpected token changes
- expired-but-still-active registry entries
- repeated fetch failures

Development instructions:
- Discovery errors should degrade safely.
- Signal generation must be blocked when registry health is unsafe for relevant markets.
- Log every discovery-health transition.

**Dependencies**  
Task 2.2

**Acceptance Criteria**
- Unsafe discovery state blocks trading for affected markets.
- Errors are logged clearly.
- The system never substitutes a guessed market when discovery is unhealthy.

---

# Phase 3 — Live Feed Ingestion and Normalization

## Goal
Consume all required market data in real time and normalize it into internal events.

---

### Task 3.1 — Build Binance WebSocket adapter

**Title**  
As a developer, I want a Binance adapter so the bot receives primary external price movement in real time.

**Description**  
Implement a resilient Binance WebSocket adapter. It should:
- connect to configured streams
- parse payloads into transport structs
- normalize into internal events
- timestamp on receipt
- detect stale connections
- reconnect with backoff

Development instructions:
- Separate socket read loop from downstream processing.
- Add heartbeat/staleness detection.
- Keep parsing efficient but not over-optimized yet.
- Preserve source timestamps when available.

**Dependencies**  
Phase 2 complete, Task 1.2

**Acceptance Criteria**
- Adapter connects and emits normalized events.
- Reconnect logic works after disconnect.
- Stale connection detection exists.
- Receipt timestamps are captured.
- Parsing errors do not crash the whole service.

---

### Task 3.2 — Build Coinbase adapter with v1 role defined

**Title**  
As a developer, I want a Coinbase adapter with a single defined purpose so the architecture stays simple in v1.

**Description**  
Implement Coinbase support, but commit to one v1 role:
- recommended v1 role: backup/failover only

In v1, do not make Coinbase a confirmation gate unless you intentionally decide to. The adapter should be capable of normalization and health tracking even if it is not on the happy path.

Development instructions:
- Make the role explicit in config and code comments.
- Do not mix backup and confirmation semantics in one ambiguous switch.
- Add future extension points without enabling them by default.

**Dependencies**  
Task 3.1

**Acceptance Criteria**
- Coinbase adapter works.
- Its v1 role is explicit and documented.
- The happy path remains simple.
- Failover behavior can be tested.

---

### Task 3.3 — Build Polymarket market WebSocket adapter

**Title**  
As a developer, I want Polymarket market updates so I can track tradable order book state in real time.

**Description**  
Implement the Polymarket market data WebSocket adapter. Maintain:
- top of book
- relevant depth levels
- market update timestamps
- health/staleness state

Development instructions:
- Separate raw payload structs from normalized book state.
- Keep a bounded in-memory book representation.
- Reconnect with backoff.
- Emit explicit staleness events when the book becomes unsafe.

**Dependencies**  
Task 2.2, Task 1.2

**Acceptance Criteria**
- Top-of-book and needed depth are maintained.
- Adapter reconnects safely.
- Staleness can be detected.
- Normalized market events are emitted reliably.

---

### Task 3.4 — Build Polymarket user WebSocket adapter

**Title**  
As a developer, I want user order/fill updates so execution and exposure logic can react to real order lifecycle events.

**Description**  
Implement the authenticated user WebSocket adapter for:
- order acknowledgment
- order updates
- fills
- partial fills
- cancels
- rejects

Development instructions:
- Treat this as part of the execution truth path.
- Preserve event ordering as much as possible.
- Make downstream exposure updates event-driven.

**Dependencies**  
Task 3.3, authentication/config setup from Phase 0

**Acceptance Criteria**
- User events are received and normalized.
- Execution engine can consume them.
- Partial fills and cancels are represented explicitly.
- Disconnects and reauth failures are handled safely.

---

### Task 3.5 — Build Polymarket RTDS adapter

**Title**  
As a developer, I want RTDS support so I can cross-check Polymarket-provided crypto reference streams.

**Description**  
Implement the RTDS adapter and make its role explicit:
- cross-check against direct Binance feed
- optional oracle-aware input via Chainlink relayed path
- diagnostics for source divergence

Development instructions:
- Do not replace direct Binance input with RTDS in v1.
- Use RTDS as a supporting signal/verification path first.
- Keep its metrics separate from direct feed metrics.

**Dependencies**  
Task 3.3

**Acceptance Criteria**
- RTDS stream is ingested and normalized.
- RTDS role is clearly documented.
- Cross-check metrics can be generated.
- RTDS failure does not break direct-feed ingestion.

---

### Task 3.6 — Add feed health monitor

**Title**  
As an operator, I want feed health status so the bot can block trading when its market view is stale or degraded.

**Description**  
Create a health monitor that tracks:
- connected/disconnected
- last message age
- reconnect count
- parse failures
- clock drift indicators if available
- stream-specific staleness

Development instructions:
- Feed health should publish into risk/signal gates.
- Stale feed state should be visible in telemetry and dashboard.
- Make thresholds configurable per feed.

**Dependencies**  
Tasks 3.1 to 3.5

**Acceptance Criteria**
- Feed health is computed continuously.
- Strategy can query feed health before signaling.
- Stale or disconnected feeds can block trading safely.

---

# Phase 4 — Fair Value, Edge Model, and Signal Engine

## Goal
Convert normalized market data into tradeable, net-of-cost signals.

---

### Task 4.1 — Build fair value engine v1

**Title**  
As a strategist, I want a rule-based fair value engine so the bot can estimate implied probability from external market movement.

**Description**  
Create a first rule-based fair value model using:
- short-window price delta
- optional momentum persistence
- optional volatility adjustment
- optional consensus check between sources

Development instructions:
- Keep v1 simple and fully interpretable.
- Version the model.
- Log every feature used for a signal.
- Do not use ML in the initial build.

**Dependencies**  
Phase 3 complete

**Acceptance Criteria**
- Fair value can be computed for supported contracts.
- Model version and inputs are logged.
- The implementation is deterministic for identical replay input.

---

### Task 4.2 — Build dynamic fee and slippage model

**Title**  
As a strategist, I want a net-edge model so the bot only trades when expected edge survives fees and slippage.

**Description**  
Create a cost model that includes:
- Polymarket fee schedule
- price/probability sensitivity
- expected entry slippage
- expected exit slippage
- latency decay buffer

Development instructions:
- Never use a fixed gross-edge-only rule in production logic.
- Keep the fee model pluggable so schedule changes do not require large refactors.
- Separate measured slippage from estimated slippage.

**Dependencies**  
Task 4.1, metadata from Phase 2

**Acceptance Criteria**
- Net edge can be computed for each candidate trade.
- Fee and slippage assumptions are explicit.
- Strategy can reject trades that fail net-edge rules.

---

### Task 4.3 — Build signal gates

**Title**  
As a strategist, I want signal gates so only safe and qualified opportunities become order intents.

**Description**  
Implement gates for:
- supported market
- non-stale feed state
- non-stale Polymarket book
- minimum confidence
- minimum net edge
- sufficient tradable depth
- no duplicate contract lock
- no kill switch active
- execution health acceptable

Development instructions:
- Keep gate reasons explicit and loggable.
- Generate “signal rejected” events with reasons.
- Separate gate logic from fair-value math.

**Dependencies**  
Task 4.2, feed health from Phase 3

**Acceptance Criteria**
- Signals either pass or fail with explicit reason codes.
- Rejected opportunities are logged for later analysis.
- No order intent is generated when a critical gate fails.

---

### Task 4.4 — Build sizing engine with safe Kelly support

**Title**  
As a risk-aware strategist, I want controlled position sizing so profitable signals are sized without unsafe exposure.

**Description**  
Implement sizing modes:
- fixed notional
- percent of equity
- capped fractional Kelly

Development instructions:
- Pure uncapped Kelly is forbidden.
- Size must also respect exposure and liquidity limits.
- Log the reason when sizing is reduced or clipped.

**Dependencies**  
Task 4.3, risk scaffolding from later phase can be stubbed initially

**Acceptance Criteria**
- Sizing can be computed deterministically.
- Kelly mode is capped.
- Position size is clipped by safety limits when necessary.

---

### Task 4.5 — Emit order intents instead of direct orders

**Title**  
As a developer, I want the signal layer to emit order intents instead of trading directly so execution remains isolated and testable.

**Description**  
Signal logic should output an `OrderIntent` object containing:
- market identity
- side
- target price logic
- size
- rationale
- cost model snapshot
- signal timestamp
- model version

Development instructions:
- Do not let strategy code talk directly to the CLOB client.
- Treat order intents as the contract between strategy and execution.
- Include enough context for replay/debug.

**Dependencies**  
Tasks 4.1 to 4.4

**Acceptance Criteria**
- Strategy emits structured order intents.
- Execution layer can consume intents without additional strategy logic.
- Intents carry sufficient metadata for replay and debugging.

---

# Phase 5 — Execution Engine and State Reconciliation

## Goal
Submit orders safely, track order lifecycle, and maintain accurate exposure.

---

### Task 5.1 — Build Polymarket client abstraction layer

**Title**  
As a developer, I want a client abstraction so the bot can benchmark or swap the official Rust SDK and alternative implementations cleanly.

**Description**  
Create an execution client interface that hides:
- auth/signing path
- order submission
- cancellation
- status queries
- fee lookups if needed
- user-stream correlation

Development instructions:
- Default to the official Rust SDK path unless measured results justify another client.
- Keep the abstraction narrow.
- Do not couple business logic to one client implementation.

**Dependencies**  
Phase 4 complete

**Acceptance Criteria**
- Execution layer depends on a client trait/interface, not one concrete implementation.
- At least one concrete implementation works.
- Client swap is possible without large strategy changes.

---

### Task 5.2 — Build execution engine with idempotent order submission

**Title**  
As a trader, I want order submission to be safe and idempotent so retries and uncertain states do not create duplicate exposure.

**Description**  
Build the execution engine that takes `OrderIntent` and performs:
- order construction
- signing/auth path
- submission
- initial order-state registration
- retry only where safe
- explicit uncertain-state handling

Development instructions:
- Generate idempotent client order IDs.
- Never blindly resubmit after uncertain order state.
- Keep submit path minimal and instrumented.

**Dependencies**  
Task 5.1

**Acceptance Criteria**
- Order intents can be submitted through the client abstraction.
- Submission produces structured order lifecycle state.
- Duplicate submissions are prevented by policy.

---

### Task 5.3 — Build fill-state feedback loop

**Title**  
As a risk engine, I want real order lifecycle feedback so new order intents account for open exposure and partial fills.

**Description**  
Connect user WebSocket events back into execution and risk state. Track:
- pending
- acked
- partial fill
- filled
- canceled
- rejected
- retrying if you model it explicitly

Development instructions:
- Update exposure on every lifecycle event.
- Feed this back into gating so the bot does not double-enter.
- Keep state transitions explicit and logged.

**Dependencies**  
Task 5.2, Task 3.4

**Acceptance Criteria**
- Fill/order states update exposure in near real time.
- Risk/signal gates can see pending/open exposure.
- Duplicate entry from stale state is prevented.

---

### Task 5.4 — Build startup reconciliation

**Title**  
As an operator, I want startup reconciliation so the bot restarts safely and understands existing orders and positions.

**Description**  
On startup:
- fetch relevant open orders
- fetch or reconstruct current positions
- reconcile with local ledger
- rebuild contract locks
- restore risk/exposure state
- mark anomalies for operator review

Development instructions:
- Never assume a clean slate after restart.
- Reconciliation should fail closed if account state cannot be trusted.
- Log all mismatches explicitly.

**Dependencies**  
Task 5.2, persistence scaffolding can be partial

**Acceptance Criteria**
- Restart can rebuild operational state.
- Unknown open orders/positions are detected.
- Trading remains blocked if reconciliation cannot establish safe state.

---

### Task 5.5 — Add bounded cancellation and replace rules

**Title**  
As a trader, I want safe cancel/replace behavior so stale or unfilled intents do not linger indefinitely.

**Description**  
Create policy for:
- max order age
- stale-signal cancellation
- cancel/replace conditions
- no-fill timeout
- partial-fill timeout
- expiry-near cancellation behavior

Development instructions:
- Tie policy to market duration regime.
- Avoid aggressive churn that could create self-inflicted API pressure.
- Log why every cancel/replace happened.

**Dependencies**  
Task 5.3

**Acceptance Criteria**
- Order timeouts and stale behavior are explicit.
- Cancel/replace actions are bounded and traceable.
- Strategy does not rely on forgotten or hanging orders.

---

# Phase 6 — Risk Engine, Kill Switch, and Contract Locking

## Goal
Prevent avoidable blowups and enforce operational safety.

---

### Task 6.1 — Build contract lock service

**Title**  
As a risk engine, I want per-contract locks so the bot does not open multiple overlapping positions in the same contract window.

**Description**  
Implement contract locking keyed by market/token identity. The lock should activate when:
- an order intent is accepted for execution
- an order is pending/open
- a position remains open

Lock release rules:
- position fully closed, or
- expiry plus configured post-expiry buffer

Development instructions:
- Keep the contract lock in a dedicated module, not buried in execution logic.
- Expose read-only lock state to signal gates.

**Dependencies**  
Phase 5 complete

**Acceptance Criteria**
- Duplicate entries into the same contract window are blocked.
- Lock lifecycle is explicit and testable.
- Post-expiry buffer works.

---

### Task 6.2 — Build exposure and limit engine

**Title**  
As a risk engine, I want strict exposure controls so size and concurrency remain within safe boundaries.

**Description**  
Implement:
- max position per market
- max concurrent positions
- max gross exposure
- max asset-linked exposure
- max notional per time bucket

Development instructions:
- Enforce limits before order submission.
- Treat pending orders as exposure where appropriate.
- Separate hard limits from advisory telemetry.

**Dependencies**  
Task 6.1

**Acceptance Criteria**
- Exposure limits are enforced consistently.
- Pending/open/fill states update exposure correctly.
- Signals exceeding limits are rejected with reason codes.

---

### Task 6.3 — Build top-level kill switch

**Title**  
As an operator, I want a top-level kill switch so the entire execution flow can be halted immediately when the system becomes unsafe.

**Description**  
Implement a kill switch as a top-level wrapper around signal and execution flow.

Trigger conditions should include:
- manual kill switch
- daily drawdown breach
- total drawdown breach
- consecutive-loss breach
- stale-data regime
- abnormal latency regime
- repeated execution failures
- unresolved order-state anomaly
- reconnect storm

Development instructions:
- The kill switch must sit above strategy and execution.
- It must block new trade entry immediately.
- Its state must be visible in telemetry/dashboard.

**Dependencies**  
Task 6.2

**Acceptance Criteria**
- Kill switch can be triggered manually and automatically.
- When active, new trading is blocked.
- Trigger reason is recorded and visible.

---

### Task 6.4 — Build drawdown and loss-streak tracker

**Title**  
As a risk engine, I want drawdown and loss-sequence tracking so degraded performance causes early intervention.

**Description**  
Track:
- daily drawdown
- session drawdown
- total drawdown
- consecutive losses
- adverse slippage bursts

Development instructions:
- Track these from official P&L events, not guessed proxy values.
- Make thresholds configurable.
- Connect to the kill switch.

**Dependencies**  
Task 6.3, accounting state from later phases may be partially stubbed initially

**Acceptance Criteria**
- Loss and drawdown metrics are computed continuously.
- Threshold breach can trigger a kill switch.
- Values are available for dashboard and logs.

---

# Phase 7 — Telemetry, Persistence, Replay, and Resolution

## Goal
Make every decision inspectable, replayable, and accountable to official outcomes.

---

### Task 7.1 — Build append-only telemetry persistence

**Title**  
As a developer, I want append-only local persistence so every signal, order, and fill can be replayed later.

**Description**  
Persist:
- raw inbound events
- normalized events
- feature snapshots
- signal decisions
- rejected-trade reasons
- order intents
- order lifecycle events
- fills
- fees
- risk-state transitions
- kill-switch events

Development instructions:
- Use local append-only files or another lightweight write path.
- Do not block the hot path on remote DB writes.
- Schema must be versioned.

**Dependencies**  
Phase 6 complete

**Acceptance Criteria**
- Key operational events are persisted.
- Hot path does not depend on remote storage.
- Stored records are versioned and parseable.

---

### Task 7.2 — Build replay engine

**Title**  
As a developer, I want deterministic replay so I can reproduce signals and regressions from captured live sessions.

**Description**  
Create a replay runner that can:
- load captured event streams
- preserve ordering
- preserve timestamps or scaled time
- rerun fair value and signal logic
- compare output against expected results

Development instructions:
- Replay should not depend on live network access.
- Make config version part of replay output.
- Allow replay in accelerated and real-time modes.

**Dependencies**  
Task 7.1

**Acceptance Criteria**
- A captured session can be replayed offline.
- Replay produces deterministic signal output for the same input and config.
- Regression scenarios can be built from replay fixtures.

---

### Task 7.3 — Build terminal dashboard and metrics view

**Title**  
As an operator, I want a terminal dashboard so I can understand health, exposure, and latency without digging through raw logs.

**Description**  
Add a first dashboard showing:
- mode
- region tag
- feed health
- active positions
- open orders
- latency samples
- P&L
- kill-switch state
- last N decisions
- reconnect/error counts

Development instructions:
- Keep v1 simple and terminal-based.
- Read from in-memory state and telemetry channels.
- Do not make dashboard logic part of hot path.

**Dependencies**  
Task 7.1

**Acceptance Criteria**
- Dashboard runs locally.
- It shows the most important runtime state.
- Dashboard failure does not kill the core bot.

---

### Task 7.4 — Build resolution verifier

**Title**  
As a risk and accounting system, I want a resolution verifier so final P&L is checked against Polymarket’s official resolved market outcome.

**Description**  
After contract closure, verify final outcome using Polymarket’s own official resolved market data and outcome state.

Development instructions:
- Do not verify final win/loss from Binance spot or arbitrary local snapshots.
- Keep resolution verification separate from live mark-to-market.
- Feed official results into accounting and drawdown logic.

**Dependencies**  
Task 2.2, telemetry persistence from Task 7.1

**Acceptance Criteria**
- Closed contracts can be verified against official Polymarket outcome data.
- Final P&L is not based on Binance/Coinbase proxy price.
- Resolution events are persisted.

---

### Task 7.5 — Build ledger and accounting view

**Title**  
As an operator, I want accurate accounting so live results, simulated results, and official outcomes remain consistent.

**Description**  
Create a ledger view that records:
- entry decision
- entry fills
- exits
- fees
- realized P&L
- unrealized P&L
- final official outcome
- drawdown-impacting events

Development instructions:
- Keep simulation, paper, and live ledgers distinguishable.
- Make accounting auditable from event history.

**Dependencies**  
Task 7.4

**Acceptance Criteria**
- Ledger state can be reconstructed from persisted events.
- Simulation/live data is not mixed.
- Official outcome is recorded for closed contracts.

---

# Phase 8 — Simulation, Shadow Mode, and Strategy Validation

## Goal
Validate strategy behavior before exposing real capital.

---

### Task 8.1 — Build simulation mode

**Title**  
As a developer, I want simulation mode so the full strategy path can run without real order submission.

**Description**  
Simulation mode should:
- use live or replayed market data
- run full fair-value logic
- run full signal gates
- run sizing
- generate order intents
- simulate fills under configurable rules
- compute simulated P&L

Development instructions:
- Simulation telemetry should mirror live telemetry schema.
- Mode should be explicit everywhere.
- Never allow simulated orders to touch real endpoints.

**Dependencies**  
Phase 7 complete

**Acceptance Criteria**
- Full bot can run in simulation without submitting real orders.
- Simulated fills and P&L are recorded.
- Simulation mode is clearly visible in logs/dashboard.

---

### Task 8.2 — Build paper/live shadow mode

**Title**  
As an operator, I want shadow mode so the system can observe live opportunities and execution paths before risking meaningful capital.

**Description**  
Create a shadow mode where:
- real data flows
- real signal logic runs
- order intents are generated
- optional order-forming code runs
- no real submission occurs, or live submission occurs only on tiny configured test paths

Development instructions:
- Keep shadow mode separate from full simulation.
- Preserve latency measurements for internal processing.
- Clearly tag all shadow-mode records.

**Dependencies**  
Task 8.1

**Acceptance Criteria**
- Shadow mode can run against live feeds.
- No unintended live orders are possible.
- Shadow decisions can be compared to later live decisions.

---

### Task 8.3 — Build evaluation reports

**Title**  
As a strategist, I want evaluation outputs so I can decide whether the strategy deserves live deployment.

**Description**  
Generate reports for:
- gross edge vs net edge
- fill assumptions vs actuals
- signal pass/fail reasons
- 5m vs 15m performance
- Binance-only vs Binance+Coinbase comparisons
- repricing exit vs hold-to-resolution comparisons

Development instructions:
- Start with CSV/JSON and markdown summaries.
- Keep analysis scripts outside the hot path.
- Make results reproducible from replay/simulation data.

**Dependencies**  
Task 8.2

**Acceptance Criteria**
- Reports can be generated from simulation/shadow data.
- Results help answer go/no-go questions for live rollout.
- 5m and 15m regimes can be compared separately.

---

# Phase 9 — Deployment, Region Benchmarking, and Production Hardening

## Goal
Prepare the system for controlled, measured live operation.

---

### Task 9.1 — Build region benchmark harness

**Title**  
As an operator, I want a benchmark harness so host region is chosen from measurement, not assumption.

**Description**  
Build a small benchmark mode that measures from candidate hosts:
- Binance message age
- Coinbase message age if enabled
- Polymarket message age
- internal decision latency
- submit-to-ack
- ack-to-fill if live validation is enabled
- reconnect quality
- packet-loss-like degradation signals

Suggested candidate regions:
- DO FRA1
- DO AMS3
- AWS eu-west-1
- other providers only if needed

Development instructions:
- Keep benchmark mode safe.
- Allow collection without live trading when possible.
- Output machine-readable results.

**Dependencies**  
Phase 8 complete

**Acceptance Criteria**
- Benchmark results can be captured by host/region.
- Region selection can be justified with data.
- Results are exportable.

---

### Task 9.2 — Add deployment profiles and run scripts

**Title**  
As an operator, I want clear deployment profiles so local, staging, and production-like runs are repeatable.

**Description**  
Create scripts or documented commands for:
- local simulation
- replay
- shadow mode
- benchmark mode
- production-like startup

Development instructions:
- Keep env handling explicit.
- Separate local dev credentials from production credentials.
- Make one-command startup possible for the common flows.

**Dependencies**  
Task 9.1

**Acceptance Criteria**
- Common run modes are documented and runnable.
- Operators can start the correct mode without guessing.
- Startup commands are reproducible.

---

### Task 9.3 — Define standby model

**Title**  
As an operator, I want a clear standby model so failover behavior is safe and does not create duplicate order submission.

**Description**  
Choose one v1 model and document it explicitly:
- benchmark-only standby, or
- manual active-passive standby

Recommended v1:
- manual active-passive or benchmark-only
- no automated leader election yet

Development instructions:
- If standby can ever submit orders, contract-lock semantics must be considered carefully.
- Keep v1 simple.

**Dependencies**  
Task 9.2

**Acceptance Criteria**
- Standby model is explicitly documented.
- There is no ambiguous multi-writer trading setup.
- Operators know how failover works.

---

### Task 9.4 — Add production health checks and runbooks

**Title**  
As an operator, I want operational runbooks so incidents can be handled quickly and consistently.

**Description**  
Add runbooks for:
- feed disconnect
- high latency
- repeated rejects
- kill switch triggered
- reconciliation failure
- stale market discovery
- restart procedure
- log collection
- safe mode downgrade

Development instructions:
- Write short, actionable runbooks.
- Keep them in the repo.
- Include mode verification steps.

**Dependencies**  
Task 9.3

**Acceptance Criteria**
- Common incident cases have documented operator actions.
- Restart and recovery procedures are written.
- Live safety actions are clear.

---

# Phase 10 — Tiny Live Validation and Controlled Ramp

## Goal
Validate real execution characteristics with the smallest reasonable risk.

---

### Task 10.1 — Enable tiny-size live order testing

**Title**  
As a trader, I want minimal-size live validation so real execution latency and slippage can be measured safely.

**Description**  
After simulation and shadow mode pass, enable tiny-size live orders with tight safeguards:
- small notional only
- limited market scope
- low concurrency
- active operator supervision
- strict kill-switch thresholds

Development instructions:
- Keep this phase explicitly separate from strategy scaling.
- The purpose is to measure execution truth, not to optimize P&L.
- Log every live decision and result in detail.

**Dependencies**  
Phase 9 complete

**Acceptance Criteria**
- Minimal-size live orders can be placed safely.
- Execution latency and fill behavior are measured.
- Risk guardrails remain active and visible.

---

### Task 10.2 — Compare live vs shadow vs simulation

**Title**  
As a strategist, I want side-by-side comparison so I can see where live reality differs from estimated behavior.

**Description**  
Build comparison outputs for:
- expected vs actual fill timing
- expected vs actual slippage
- simulation vs live net edge
- shadow opportunities vs live realized opportunities
- rejected order reasons vs modeled assumptions

Development instructions:
- Focus on truth discovery, not vanity metrics.
- Highlight where live assumptions break down.

**Dependencies**  
Task 10.1

**Acceptance Criteria**
- Live and non-live results can be compared clearly.
- Key mismatch categories are visible.
- Go/no-go decision can be made from evidence.

---

### Task 10.3 — Final go/no-go review

**Title**  
As an operator and strategist, I want a final readiness review so production expansion happens only if the strategy is still economically valid.

**Description**  
Review:
- net edge after fees/slippage
- latency competitiveness
- execution reliability
- drawdown behavior
- feed stability
- operator confidence
- whether 5m and/or 15m should remain in scope

Development instructions:
- This review should be evidence-based.
- It must be acceptable to conclude that the strategy is not worth further deployment.
- Document the decision and next steps.

**Dependencies**  
Task 10.2

**Acceptance Criteria**
- A written go/no-go decision exists.
- Decision is based on measured evidence.
- Next steps are clear: scale, refine, narrow scope, or stop.

---

## 5. Suggested Delivery Sequence by Sprint

### Sprint A
- Phase 0
- Phase 1
- Phase 2

### Sprint B
- Phase 3
- Phase 4

### Sprint C
- Phase 5
- Phase 6

### Sprint D
- Phase 7
- Phase 8

### Sprint E
- Phase 9
- Phase 10

---

## 6. Minimum Viable Milestones

### Milestone 1 — Local compile-ready foundation
Complete through Phase 2.

### Milestone 2 — Live data ingestion
Complete through Phase 3.

### Milestone 3 — Offline signal generation
Complete through Phase 4.

### Milestone 4 — Safe simulated bot
Complete through Phase 8.

### Milestone 5 — Production-ready engineering shell
Complete through Phase 9.

### Milestone 6 — Real execution truth
Complete through Phase 10.

---

## 7. Done Definition for the Full Program

The program is complete only when:
- the system can discover supported markets
- live feed ingestion works reliably
- fair value and net edge are computed in real time
- order intents are generated safely
- execution is idempotent and reconciled
- contract locks prevent duplicate positions
- risk and kill-switch rules are enforced
- every important event is logged and replayable
- official Polymarket outcome verification exists
- simulation and shadow mode are stable
- region selection is benchmark-driven
- tiny-size live validation has been measured
- a written go/no-go review exists

---

## 8. Notes for Any Coding Agent

If an agent is given this file:
- follow phases in order
- do not skip simulation/replay
- do not implement live shortcuts first
- do not collapse signal and execution into one module
- do not hardcode fallback markets
- do not treat paper/simulation latency as live execution truth
- keep all major behaviors testable and observable
