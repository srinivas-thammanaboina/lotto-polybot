# Code Review Fix Tracker (Consolidated)

Source reviews:
- [agent1codereview.md](agent1codereview.md) — Round 1
- [agent1codereview2.md](agent1codereview2.md) — Round 2

## Status Legend
- [ ] Not started
- [~] In progress
- [x] Fixed

---

## P0 — Critical (fix before any paper/live run)

| # | Source | Issue | Agree? | Status | Fix Notes |
|---|--------|-------|--------|--------|-----------|
| P0-1 | R1 | `SimulationClient` wired unconditionally — no real client for paper/live | Agree | [x] | Added `build_exchange_client(cfg)` factory. Fails startup if paper/live missing credentials. |
| P0-2 | R1 | Coinbase feed spawned unconditionally | Defer | [x] | Now only spawns when `cfg.coinbase.enabled`. Adapter also self-disables internally. |
| P0-3 | R1+R2 | Polymarket market WS starts with empty token list | Agree | [~] | `wait_for_discovery()` provides initial tokens. Full mid-run resubscribe on refresh still TODO. |
| P0-4 | R1 | User WS auth incomplete (missing secret/passphrase) | Agree | [x] | Auth payload now includes `secret` + `passphrase`. Full HMAC signing still TODO for production. |
| P0-5 | R1 | SignalAccepted emitted + lock applied even when intent queue full | Agree | [x] | On queue-full: lock NOT applied, `SignalRejected(ExecutionBackpressure)` emitted. |
| P0-6 | R2 | Pending exposure not added before submit/fill lifecycle | Agree | [x] | `add_pending()` before `submit_intent()`. `remove_pending()` on failure. |
| P0-7 | R2 | Outcome direction inferred from token-id string matching | Agree | [x] | Added `outcome: Outcome` to `ContractEntry`. Populated from `OutcomeMeta` during discovery. |
| P0-8 | R2 | Resolution handling not connected to ledger/drawdown/locks | Agree | [x] | Resolution events now: release locks, fetch from Polymarket API, verify P&L, update ledger + drawdown + equity. |

## P1 — High Priority (fix before live validation)

| # | Source | Issue | Agree? | Status | Fix Notes |
|---|--------|-------|--------|--------|-----------|
| P1-1 | R1+R2 | Kill switch reason discarded — hardcoded `Manual` | Agree | [x] | `parse_kill_switch_reason()` maps event reason to `KillSwitchReason` enum. |
| P1-2 | R1+R2 | Placeholder pipeline values (equity, exec health, signal age) | Agree | [x] | Equity from `account_balance()`, exec health from `is_healthy()`, signal age from receipt timestamp. |
| P1-3 | R1 | Fee model default may be stale | Agree | [x] | `FeeConfig` in `StrategyConfig`, loaded from env vars. `FeeSchedule::from_config()`. Edge sensitivity tests added. |
| P1-4 | R1+R2 | Resolution verifier not integrated into live flow | Agree | [x] | `ResolutionFetcher` added. Wired into event loop: fetch → verify → ledger → drawdown → equity. |
| P1-5 | R1 | README drifted from actual implementation | Agree | [x] | Complete rewrite — status table, accurate layout, quick start, modes. |
| P1-6 | R2 | `app.rs` too large / god orchestrator | Agree | [ ] | Valid. Deferred P1 refactor. 707 lines with clear sections. |
| P1-7 | R2 | CEX health hardcodes Binance, no backup fallback | Agree | [x] | Now checks Binance OR (Coinbase if enabled). Both `is_system_ready()` and pipeline input updated. |

## P2 — Medium Priority (fix as code matures)

| # | Source | Issue | Agree? | Status | Fix Notes |
|---|--------|-------|--------|--------|-----------|
| P2-1 | R1 | Event persistence drops all events equally under backpressure | Agree | [x] | `EventDurability` enum (Critical/BestEffort). Critical drops → error log. |
| P2-2 | R1+R2 | No integration tests or CI | Agree | [x] | `.github/workflows/ci.yml` — fmt + clippy + test. Integration tests still TODO. |
| P2-3 | R1 | No startup readiness gates | Agree | [x] | `is_system_ready()` gate + `wait_for_discovery()` at startup. |

---

## Summary

| Severity | Total | Fixed | In Progress | Not Started |
|----------|-------|-------|-------------|-------------|
| P0 | 8 | 7 | 1 (resubscribe) | 0 |
| P1 | 7 | 6 | 0 | 1 (app.rs split) |
| P2 | 3 | 3 | 0 | 0 |
| **Total** | **18** | **16** | **1** | **1** |

## Remaining Work

1. **P0-3** (in progress): Mid-run token resubscription when discovery refreshes new markets
2. **P1-6** (not started): Split `app.rs` into smaller runtime coordinator modules
3. **P0-4** (partial): Full HMAC signing for Polymarket user WS auth (currently sends credentials, not signed)
4. **P2-2** (partial): Integration tests (CI exists, tests still TODO)
