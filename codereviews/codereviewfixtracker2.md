# Code Review Fix Tracker — Round 2

Source review: [agent1codereview2.md](agent1codereview2.md)

## Status Legend
- [ ] Not started
- [~] In progress
- [x] Fixed
- [=] Already fixed in Round 1

---

## P0 — Critical

| # | Issue | Agree? | Status | Comment |
|---|-------|--------|--------|---------|
| R2-1 | Pending exposure not added before submit/fill lifecycle | **Agree** | [x] | `add_pending()` called before `submit_intent()`. `remove_pending()` on failure. |
| R2-2 | Market WS needs full token-refresh/resubscribe wiring | **Agree** | [~] | wait_for_discovery provides initial tokens. Full mid-run resubscribe still TODO. |
| R2-3 | Outcome direction inferred from token-id string matching | **Agree** | [x] | Added `outcome: Outcome` field to `ContractEntry`. Populated from `OutcomeMeta` during discovery. |
| R2-4 | Resolution runtime handling not connected to ledger/drawdown | **Agree** | [x] | Resolution events now: release contract locks, fetch from Polymarket API, verify P&L, update ledger + drawdown + equity. |

## P1 — High Priority

| # | Issue | Agree? | Status | Comment |
|---|-------|--------|--------|---------|
| R2-5 | `app.rs` too large / god orchestrator | **Agree** | [ ] | Valid concern. Deferred — P1 refactor. File is 707 lines with clear sections. |
| R2-6 | Kill switch reasons collapsed to Manual | **Defer** | [=] | Already fixed in Round 1 — `parse_kill_switch_reason()` maps event reason to enum. Reviewer may not have seen latest commit. |
| R2-7 | CEX health hardcodes Binance, no backup fallback | **Agree** | [x] | CEX health now checks Binance OR (Coinbase if enabled). Both `is_system_ready()` and pipeline input updated. |
| R2-8 | Sizing uses placeholder equity | **Defer** | [=] | Already fixed in Round 1 — equity loaded from `account_balance()` at startup, tracked in `Arc<RwLock<Decimal>>`. |
| R2-9 | `execution_healthy` hardcoded true | **Defer** | [=] | Already fixed in Round 1 — now calls `exec_engine.is_healthy().await`. |

## P2 — Medium Priority

| # | Issue | Agree? | Status | Comment |
|---|-------|--------|--------|---------|
| R2-10 | README status claims should be backed by CI | **Defer** | [=] | CI already added in Round 1 (`.github/workflows/ci.yml`). Badge not yet added. |

---

## Fix Order

### Round 1 — Correctness (P0s)
1. R2-1: Wire pending exposure before submit
2. R2-3: Replace token-id string outcome with registry metadata
3. R2-4: Wire resolution events into ledger + drawdown + lock release
4. R2-2: Token subscription refresh (enhance existing wait_for_discovery)

### Round 2 — Runtime Safety (P1s)
5. R2-7: CEX health checks primary OR backup
6. R2-5: Split app.rs into runtime modules

---

## Items Already Fixed in Round 1
- R2-6 (kill switch reasons) → `parse_kill_switch_reason()` in app.rs
- R2-8 (placeholder equity) → `account_balance()` + `Arc<RwLock<Decimal>>`
- R2-9 (execution_healthy) → `exec_engine.is_healthy().await`
- R2-10 (CI) → `.github/workflows/ci.yml`
