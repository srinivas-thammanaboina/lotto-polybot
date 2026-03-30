# agent1codereview.md — Code Review for `lotto-polybot`

## Scope

Repository reviewed: `https://github.com/srinivas-thammanaboina/lotto-polybot`

Review type: **static code review** of the public repository.
I **did not** run the code locally, compile it, or execute integration tests. All comments below are based on the repository structure and the code visible in GitHub.

---

## Overall verdict

This is a **promising architecture-level implementation** with good module separation and many of the right primitives for a Rust/Tokio trading system, but it is **not production-ready yet**.

The strongest parts are the project structure, the event-driven shape, the separation of discovery/feeds/strategy/execution/risk, and the presence of simulation/replay-minded components.

The biggest problems are **runtime wiring gaps** and a few **correctness-critical implementation mismatches** that would prevent the bot from behaving as intended outside simulation.

In short:
- **Architecture direction:** good
- **Engineering maturity:** decent early-stage foundation
- **Production readiness:** not yet
- **Main blockers:** execution wiring, user-stream auth, market subscription flow, signal/execution state coupling

---

## What is already good

### 1. Good high-level module structure
The repository is organized in a way that matches the intended system design pretty well:
- `discovery`
- `feeds`
- `strategy`
- `execution`
- `risk`
- `resolution`
- `simulation`
- `telemetry`
- `validation`

That is the right direction for keeping domain logic, transport logic, and safety logic separate.

### 2. Execution client abstraction exists
`src/execution/client.rs` has an `ExchangeClient` trait abstraction, which is exactly the right shape if you want to compare the official Polymarket Rust SDK with other implementations later without rewriting strategy logic.

### 3. Kill switch design is structurally strong
The kill-switch module is one of the better parts of the codebase. It has:
- explicit reasons
- active/inactive state
- history tracking
- fast-path atomic access

That is the correct control-plane pattern.

### 4. Feed adapters are separated instead of buried inside app logic
Binance, Coinbase, Polymarket market WS, Polymarket user WS, and RTDS are isolated into their own modules. That is a solid design choice and makes later benchmarking/refactoring easier.

### 5. Discovery and metadata are not hardcoded into the hot path
Discovery/cache logic appears to be isolated from strategy/execution, which is the right pattern for this class of bot.

### 6. There is visible effort toward replay/simulation and telemetry
The codebase shows a healthy bias toward observability and offline analysis, which is important for this kind of system.

---

## Critical issues (P0)

These should be fixed before any serious paper/live run.

### P0-1 — `SimulationClient` is wired unconditionally in `app.rs`

**Problem**

The application currently appears to construct `SimulationClient::default()` and pass it into `ExecutionEngine::new(...)` unconditionally in `src/app.rs`.

That means even if config says `paper` or `live`, the current app wiring still routes execution through the simulation client.

**Why this matters**

This is a correctness blocker. It means non-simulation modes are not actually wired to a real client implementation, so mode separation is not trustworthy.

**Observed in**
- `src/app.rs` around the execution-engine construction path

**Review comment**

Mode selection should happen at app wiring time and choose the concrete execution client explicitly:
- simulation -> `SimulationClient`
- paper/live -> real Polymarket client implementation

**Recommendation**
- Introduce a `build_exchange_client(config)` factory in app startup.
- Fail startup if `paper/live` is requested but no real client is configured.
- Log the selected execution client at startup.

---

### P0-2 — Coinbase feed task is spawned even when Coinbase is meant to be optional/backup

**Problem**

In `src/app.rs`, Coinbase is spawned unconditionally. I also did not see a visible `enabled` guard inside the Coinbase adapter itself.

**Why this matters**

Your design docs say Coinbase should have a clearly defined role, likely backup-only in v1. Spawning it unconditionally adds unnecessary complexity and can create confusion around whether it is part of the happy path, backup path, or signal consensus path.

**Observed in**
- `src/app.rs` feed startup section
- `src/feeds/coinbase.rs`

**Review comment**

Right now the runtime behavior and intended architecture are out of sync.

**Recommendation**
- Either do not spawn Coinbase when disabled, or
- spawn it only under a clearly named config mode such as:
  - `backup_only`
  - `confirmation`
  - `disabled`
- Make the v1 role explicit and narrow.

---

### P0-3 — Polymarket market WS appears to start with an empty token list

**Problem**

`src/app.rs` appears to start the Polymarket market WebSocket adapter with `Vec::new()` for token IDs, with a comment that token IDs will be populated after first discovery.

However, I did not find a clear later subscription-update path in the app wiring.

The market WS adapter appears to subscribe based on the token list provided at connect time.

**Why this matters**

If the adapter starts with zero token IDs and never resubscribes, then the bot will not receive the order-book/market updates it needs for trading.

This is a major correctness issue because the strategy cannot safely trade without live Polymarket book state.

**Observed in**
- `src/app.rs` market WS startup path
- `src/feeds/polymarket_market.rs`

**Recommendation**
- Do not start the market WS with an empty subscription set.
- Resolve supported markets first, then subscribe.
- If markets refresh dynamically, add an explicit resubscription/update mechanism.
- Block strategy start until market subscriptions are confirmed.

---

### P0-4 — Polymarket user WebSocket auth flow does not look correct

**Problem**

The current user WebSocket auth payload appears to send only something like:

```json
{"type":"auth","apiKey":"..."}
```

I did not see `secret` or `passphrase` included, and I did not see a subscription/auth shape matching Polymarket’s documented user-channel auth flow.

**Why this matters**

The user stream is the execution truth path for:
- acks
- fills
- partial fills
- cancels
- rejects

If auth is incomplete or malformed, the bot may believe it has order-state visibility when it does not.

**Observed in**
- `src/feeds/polymarket_user.rs`

**External reference**
- Polymarket docs: user channel auth requires API key, secret, and passphrase
- Polymarket docs also describe the user WS/auth request shape

**Recommendation**
- Rework user WS auth to match the official Polymarket spec exactly.
- Add an explicit authenticated/ready state before execution is allowed.
- If user WS auth fails, execution should fail closed.

---

### P0-5 — Accepted signals can be marked as accepted even when execution queue is full

**Problem**

In `src/app.rs`, when a signal is accepted:
1. metrics are incremented
2. contract lock is applied
3. `intent_tx.try_send(...)` is attempted
4. if the channel is full, the code logs the problem
5. but the signal still appears to be emitted/persisted as `SignalAccepted`

**Why this matters**

This creates a state-consistency bug:
- strategy thinks signal was accepted
- lock may remain active
- telemetry may record acceptance
- but execution never received the intent

That can distort audit logs and interfere with contract locks/cooldowns.

**Observed in**
- `src/app.rs` accepted-signal path around intent queue submission

**Recommendation**
- If the execution queue is full, treat it as a rejection or backpressure failure.
- Do **not** emit `SignalAccepted` unless the intent has actually been handed off successfully.
- Release or downgrade the contract lock when handoff fails.
- Emit a specific event such as:
  - `SignalRejected(Reason::ExecutionBackpressure)`
  - or `IntentDispatchFailed`

---

## High-priority issues (P1)

These are not quite as fatal as the P0 items, but they matter before any live validation.

### P1-1 — Kill-switch reason is being discarded in app wiring

**Problem**

The event model includes a kill-switch event with a reason string / structured reason, but in `app.rs` the kill switch appears to be activated as `KillSwitchReason::Manual` regardless of the actual source event.

**Why this matters**

That destroys auditability and makes postmortems much harder. A kill switch triggered by stale data, drawdown, or execution failure should not be flattened into “manual”.

**Observed in**
- `src/app.rs`
- `src/types.rs`
- `src/risk/kill_switch.rs`

**Recommendation**
- Preserve the original kill-switch cause through the app event pipeline.
- Convert external event reason -> internal `KillSwitchReason` enum without losing information.
- Persist the exact trigger reason in the kill-switch history.

---

### P1-2 — Several strategy inputs are still placeholders/hardcoded in app wiring

**Problem**

The signal path still appears to use placeholder values such as:
- `execution_healthy: true`
- `equity: dec!(500)`
- `signal_age: Duration::from_millis(0)`

These are clearly TODO placeholders, but they are in a safety-critical path.

**Why this matters**

This means gating and sizing are not yet based on real runtime truth.
For this kind of bot, fake values in safety fields are more dangerous than missing features because they can make the system look operationally complete when it is not.

**Observed in**
- `src/app.rs` where pipeline inputs are assembled before gating/signal evaluation

**Recommendation**
- Replace placeholders with real sources before any non-simulation evaluation:
  - execution health from execution engine / user stream health
  - equity from balance/ledger/reconciliation state
  - signal age from actual event timestamps
- If the value is unknown, fail closed rather than assuming safe defaults.

---

### P1-3 — Fee model default is stale relative to current Polymarket crypto fee structure

**Problem**

`src/strategy/edge.rs` appears to use a baked default fee model that implies about **1.00% effective fee at 50% probability** (from `taker_rate: 0.02 * min(p, 1-p)`).

Current Polymarket crypto fee docs are higher than that at 50% probability and also document an upcoming changed fee structure.

**Why this matters**

If the fee model is understated, the bot can label trades as profitable when they are not.

**Observed in**
- `src/strategy/edge.rs`

**External reference**
- Official Polymarket fees documentation for crypto markets

**Recommendation**
- Do not rely on this baked default for production logic.
- Make fee schedules explicitly configurable and versioned.
- Add tests for:
  - current live schedule
  - future scheduled schedule
  - edge sensitivity around 50/50 markets
- Consider fetching or centrally updating the fee schedule instead of embedding assumptions.

---

### P1-4 — Resolution verifier exists as logic, but authoritative market-outcome integration is still incomplete

**Problem**

The resolution verifier logic itself looks useful, but I did not see clear evidence that it is integrated to fetch and verify against authoritative Polymarket resolved market data in the end-to-end flow.

**Why this matters**

A verifier that works only on manually supplied data structures is not enough. The full system still needs a trusted path that fetches official resolved outcome state and feeds that into accounting/drawdown.

**Observed in**
- `src/resolution/verifier.rs`
- app-level flow did not clearly show full authoritative-resolution integration

**Recommendation**
- Add a dedicated resolution fetch path.
- Make end-of-market verification part of the production event flow.
- Feed verified outcomes into ledger, loss streak, drawdown, and postmortem reports.

---

### P1-5 — README / repository state appears to be drifting from actual implementation

**Problem**

The README describes a broader/cleaner repository layout than what is actually visible in the root tree. For example, the README references directories such as `tests/`, `configs/`, and `testdata/`, but those were not visible in the repository root tree I reviewed.

**Why this matters**

This increases onboarding friction and makes it harder to tell what is implemented versus planned.

**Observed in**
- repo root tree
- `README.md` project layout section

**Recommendation**
- Align README with the real repository shape.
- If certain pieces are still planned, mark them explicitly as planned.
- Add a short “current status” section listing what is already built and what is still placeholder/TODO.

---

## Medium-priority issues (P2)

These do not block progress immediately, but they should be cleaned up as the code matures.

### P2-1 — Event persistence appears to drop events under backpressure

**Problem**

`EventPersistence::try_persist` appears to use a non-blocking channel send and drop events when the queue is full.

**Why this matters**

This may be acceptable for low-priority telemetry, but it is risky for audit-critical event classes such as:
- order lifecycle
- kill switch activation
- fills
- official resolution outcomes

**Recommendation**
- Define event durability classes:
  - critical -> must not drop silently
  - best-effort -> may drop under pressure
- Route critical events through a stronger persistence path.
- Track drop counts by event class.

---

### P2-2 — More explicit benchmark/test evidence is needed in the repo surface

**Problem**

The root tree I reviewed did not visibly show a top-level `tests/` directory or visible CI workflow files.

**Why this matters**

For a bot like this, visible automated validation matters almost as much as architecture.

**Recommendation**
- Add visible integration tests for:
  - discovery
  - feed normalization
  - signal rejection reasons
  - execution handoff
  - contract lock behavior
  - kill-switch activation
- Add CI to run fmt/clippy/test.
- Add replay fixtures as repository assets where feasible.

---

### P2-3 — Startup readiness gates should be stricter

**Problem**

The system shape suggests multiple asynchronous subsystems, but it is not yet obvious that startup waits for a minimum “ready” state before enabling strategy.

**Why this matters**

The bot should not emit trade intents unless all required readiness conditions are met.

**Recommendation**
Add a formal readiness gate requiring at least:
- discovery ready
- market subscriptions ready
- user stream ready/authenticated
- required feed health green
- contract registry populated
- execution client healthy
- no active kill switch

---

## Suggested fix order

If I were fixing this repo, I would do it in this order:

### 1. Fix runtime correctness first
- wire real execution client selection by mode
- fix user WS auth
- fix market subscription flow so token IDs are real and active
- ensure strategy only starts when required streams are ready

### 2. Fix state consistency
- repair accepted-signal vs queue-full behavior
- preserve kill-switch reasons end to end
- replace placeholder pipeline values with real runtime state or fail closed

### 3. Fix economics and accounting
- update fee model
- integrate authoritative resolution verification into the live flow
- verify ledger/drawdown are driven by official outcomes

### 4. Tighten operational maturity
- align README with reality
- add visible integration tests / CI
- classify telemetry durability by event criticality

---

## Suggested summary for the repository owner

If I had to summarize the review in one sentence:

> The repo has the **right architectural shape**, but there are still several **last-mile correctness gaps** between “well-designed simulation-first foundation” and “safe execution system.”

And the two most important fixes are:
1. **make non-simulation execution real and explicit**, and
2. **make feed/execution state transitions trustworthy before any trade can be emitted**.

---

## Final recommendation

I would **not** call this build production-ready yet.

I **would** call it a strong early implementation that is worth continuing, because the structure is good and the hard parts that remain are mostly **integration correctness and operational hardening**, not a total redesign.

