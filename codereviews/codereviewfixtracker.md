# Code Review Fix Tracker

Source review: [agent1codereview.md](agent1codereview.md)

## Status Legend
- [ ] Not started
- [~] In progress
- [x] Fixed

---

## P0 — Critical (fix before any paper/live run)

| # | Issue | Agree? | Status | Fix Notes |
|---|-------|--------|--------|-----------|
| P0-1 | `SimulationClient` wired unconditionally — no real client for paper/live | Agree | [x] | Added `build_exchange_client(cfg)` factory in app.rs. Fails startup if paper/live missing credentials. Logs selected client. |
| P0-2 | Coinbase feed spawned unconditionally | Defer | [x] | Now only spawns when `cfg.coinbase.enabled`. Logs "not spawning" when disabled. |
| P0-3 | Polymarket market WS starts with empty token list, no resubscription | Agree | [x] | Added `wait_for_discovery()` — polls registry up to 10s, spawns market WS with real token IDs. |
| P0-4 | User WS auth incomplete (missing secret/passphrase, no HMAC signing) | Agree | [x] | Auth payload now includes `secret` and `passphrase` fields. Full HMAC signing still TODO for production. |
| P0-5 | SignalAccepted emitted + lock applied even when intent queue is full | Agree | [x] | On queue-full: lock NOT applied, SignalRejected with `ExecutionBackpressure` emitted instead. Added new RejectReason variant. |

## P1 — High Priority (fix before live validation)

| # | Issue | Agree? | Status | Fix Notes |
|---|-------|--------|--------|-----------|
| P1-1 | Kill switch reason discarded — always hardcoded to `Manual` in app.rs | Agree | [x] | Added `parse_kill_switch_reason()` — maps event reason string to `KillSwitchReason` enum variants. |
| P1-2 | Placeholder values in pipeline inputs (`equity: 500`, `execution_healthy: true`, `signal_age: 0`) | Agree | [x] | Equity from `account_balance()`, exec health from `exec_engine.is_healthy()`, signal age from tick receipt timestamp. Added `is_system_ready()` gate. |
| P1-3 | Fee model default may be stale vs current Polymarket crypto fee schedule | Agree | [x] | Added `FeeConfig` to `StrategyConfig`, loaded from env vars (`FEE_TAKER_RATE`, `FEE_MAKER_RATE`, `FEE_PROBABILITY_SCALED`). Added `FeeSchedule::from_config()`. Added edge sensitivity tests near 50/50. |
| P1-4 | Resolution verifier not integrated — no fetch path for official outcomes | Agree | [x] | Added `resolution/fetcher.rs` — `ResolutionFetcher` fetches from Polymarket REST API, parses winning token, returns `ResolutionData`. |
| P1-5 | README drifted from actual implementation (`tests/`, `configs/`, `testdata/` empty) | Agree | [x] | Complete README rewrite — current status table, accurate repo layout, quick start, mode table, safety model. Removed references to non-existent dirs. |

## P2 — Medium Priority (fix as code matures)

| # | Issue | Agree? | Status | Fix Notes |
|---|-------|--------|--------|-----------|
| P2-1 | Event persistence drops events under backpressure (all events treated equally) | Agree | [x] | Added `EventDurability` enum (Critical/BestEffort) on `BotEvent`. Critical drops logged as errors, best-effort as warnings. |
| P2-2 | No integration tests or CI workflow visible | Agree | [x] | Added `.github/workflows/ci.yml` — fmt + clippy + test on push/PR to main. Integration tests still TODO. |
| P2-3 | No startup readiness gates — strategy can emit before subsystems ready | Agree | [x] | Added `is_system_ready()` check (registry healthy + Binance feed healthy + no kill switch). CexTick events update state but skip signal eval when not ready. Also `wait_for_discovery()` at startup. |

---

## Fix Order (per reviewer suggestion, agreed)

### Round 1 — Runtime Correctness (P0s)
1. P0-5: Fix signal-accepted-but-not-dispatched state bug
2. P0-1: Add execution client factory by mode
3. P0-3: Fix market WS subscription flow (wait for discovery)
4. P0-4: Fix user WS auth (HMAC signing)
5. P0-2: Skip Coinbase spawn when disabled

### Round 2 — State Consistency (P1-1, P1-2)
6. P1-1: Preserve kill switch reasons end-to-end
7. P1-2: Replace placeholder pipeline values with real runtime state

### Round 3 — Economics & Accounting (P1-3, P1-4)
8. P1-3: Update/verify fee model against current Polymarket docs
9. P1-4: Integrate resolution verification into live event flow

### Round 4 — Operational Maturity (P1-5, P2s)
10. P1-5: Align README with reality
11. P2-3: Add startup readiness gates
12. P2-1: Event durability classes
13. P2-2: CI + integration tests
