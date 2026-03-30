//! Ledger and accounting view.
//!
//! Records trade lifecycle events and produces auditable accounting.
//! Simulation, paper, and live ledgers are distinguishable by mode tag.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::config::RunMode;
use crate::domain::ledger::{ResolutionOutcome, VerifiedOutcome};
use crate::domain::market::ContractKey;
use crate::domain::signal::Side;

// ---------------------------------------------------------------------------
// Ledger entry
// ---------------------------------------------------------------------------

/// A single trade in the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: u64,
    pub mode: String,
    pub contract: ContractKey,
    pub side: Side,
    pub entry_price: Decimal,
    pub exit_price: Option<Decimal>,
    pub size: Decimal,
    pub fees_paid: Decimal,
    pub realized_pnl: Option<Decimal>,
    pub official_outcome: Option<ResolutionOutcome>,
    pub official_pnl: Option<Decimal>,
    pub opened_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub verified_at: Option<DateTime<Utc>>,
}

impl LedgerEntry {
    pub fn is_open(&self) -> bool {
        self.closed_at.is_none()
    }

    pub fn is_verified(&self) -> bool {
        self.official_outcome.is_some()
    }
}

// ---------------------------------------------------------------------------
// Ledger summary
// ---------------------------------------------------------------------------

/// Aggregate accounting summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerSummary {
    pub mode: String,
    pub total_trades: u64,
    pub open_trades: u64,
    pub closed_trades: u64,
    pub verified_trades: u64,
    pub total_realized_pnl: Decimal,
    pub total_official_pnl: Decimal,
    pub total_fees: Decimal,
    pub win_count: u64,
    pub loss_count: u64,
}

// ---------------------------------------------------------------------------
// Ledger
// ---------------------------------------------------------------------------

/// In-memory accounting ledger. Mode-tagged to prevent mixing sim/live data.
pub struct Ledger {
    mode: RunMode,
    entries: Vec<LedgerEntry>,
    /// Index: contract key → entry IDs for fast lookup.
    by_contract: HashMap<String, Vec<u64>>,
    next_id: u64,
}

impl Ledger {
    pub fn new(mode: RunMode) -> Self {
        Self {
            mode,
            entries: Vec::new(),
            by_contract: HashMap::new(),
            next_id: 1,
        }
    }

    /// Record a new trade entry (position opened).
    pub fn record_entry(
        &mut self,
        contract: ContractKey,
        side: Side,
        entry_price: Decimal,
        size: Decimal,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let entry = LedgerEntry {
            id,
            mode: self.mode.to_string(),
            contract: contract.clone(),
            side,
            entry_price,
            exit_price: None,
            size,
            fees_paid: Decimal::ZERO,
            realized_pnl: None,
            official_outcome: None,
            official_pnl: None,
            opened_at: Utc::now(),
            closed_at: None,
            verified_at: None,
        };

        let key = contract.to_string();
        self.by_contract.entry(key).or_default().push(id);
        self.entries.push(entry);

        debug!(id = id, contract = %contract, "ledger_entry_opened");
        id
    }

    /// Record a trade exit (position closed).
    pub fn record_exit(&mut self, id: u64, exit_price: Decimal, fees: Decimal) -> Option<Decimal> {
        let entry = self.entries.iter_mut().find(|e| e.id == id)?;

        entry.exit_price = Some(exit_price);
        entry.fees_paid += fees;
        entry.closed_at = Some(Utc::now());

        let pnl = match entry.side {
            Side::Buy => (exit_price - entry.entry_price) * entry.size - entry.fees_paid,
            Side::Sell => (entry.entry_price - exit_price) * entry.size - entry.fees_paid,
        };
        entry.realized_pnl = Some(pnl);

        info!(id = id, pnl = %pnl, "ledger_entry_closed");
        Some(pnl)
    }

    /// Add fees to an existing entry.
    pub fn add_fees(&mut self, id: u64, fees: Decimal) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.fees_paid += fees;
        }
    }

    /// Record an official resolution outcome.
    pub fn record_resolution(&mut self, id: u64, outcome: &VerifiedOutcome) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.official_outcome = Some(outcome.outcome);
            entry.official_pnl = Some(outcome.realized_pnl);
            entry.verified_at = Some(outcome.verified_at);

            info!(
                id = id,
                outcome = ?outcome.outcome,
                official_pnl = %outcome.realized_pnl,
                "ledger_resolution_recorded"
            );
        }
    }

    /// Get open entries for a contract.
    pub fn open_entries_for(&self, contract: &ContractKey) -> Vec<&LedgerEntry> {
        let key = contract.to_string();
        self.by_contract
            .get(&key)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.entries.iter().find(|e| e.id == *id && e.is_open()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get a summary of the ledger.
    pub fn summary(&self) -> LedgerSummary {
        let total = self.entries.len() as u64;
        let open = self.entries.iter().filter(|e| e.is_open()).count() as u64;
        let closed = total - open;
        let verified = self.entries.iter().filter(|e| e.is_verified()).count() as u64;

        let total_realized: Decimal = self.entries.iter().filter_map(|e| e.realized_pnl).sum();
        let total_official: Decimal = self.entries.iter().filter_map(|e| e.official_pnl).sum();
        let total_fees: Decimal = self.entries.iter().map(|e| e.fees_paid).sum();

        let win_count = self
            .entries
            .iter()
            .filter(|e| e.realized_pnl.map(|p| p > Decimal::ZERO).unwrap_or(false))
            .count() as u64;
        let loss_count = self
            .entries
            .iter()
            .filter(|e| e.realized_pnl.map(|p| p < Decimal::ZERO).unwrap_or(false))
            .count() as u64;

        LedgerSummary {
            mode: self.mode.to_string(),
            total_trades: total,
            open_trades: open,
            closed_trades: closed,
            verified_trades: verified,
            total_realized_pnl: total_realized,
            total_official_pnl: total_official,
            total_fees,
            win_count,
            loss_count,
        }
    }

    /// Get all entries (for persistence/export).
    pub fn entries(&self) -> &[LedgerEntry] {
        &self.entries
    }

    /// Get the mode this ledger is tracking.
    pub fn mode(&self) -> RunMode {
        self.mode
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{MarketId, TokenId};
    use rust_decimal_macros::dec;

    fn test_contract() -> ContractKey {
        ContractKey {
            market_id: MarketId("mkt1".into()),
            token_id: TokenId("tok1".into()),
        }
    }

    #[test]
    fn record_entry_and_exit() {
        let mut ledger = Ledger::new(RunMode::Simulation);
        let id = ledger.record_entry(test_contract(), Side::Buy, dec!(0.55), dec!(10));
        assert_eq!(ledger.entries().len(), 1);
        assert!(ledger.entries()[0].is_open());

        let pnl = ledger.record_exit(id, dec!(0.65), dec!(0.10)).unwrap();
        // (0.65 - 0.55) * 10 - 0.10 = 1.00 - 0.10 = 0.90
        assert_eq!(pnl, dec!(0.90));
        assert!(!ledger.entries()[0].is_open());
    }

    #[test]
    fn record_sell_entry_and_exit() {
        let mut ledger = Ledger::new(RunMode::Simulation);
        let id = ledger.record_entry(test_contract(), Side::Sell, dec!(0.55), dec!(10));

        let pnl = ledger.record_exit(id, dec!(0.45), dec!(0.10)).unwrap();
        // (0.55 - 0.45) * 10 - 0.10 = 1.00 - 0.10 = 0.90
        assert_eq!(pnl, dec!(0.90));
    }

    #[test]
    fn add_fees() {
        let mut ledger = Ledger::new(RunMode::Simulation);
        let id = ledger.record_entry(test_contract(), Side::Buy, dec!(0.55), dec!(10));
        ledger.add_fees(id, dec!(0.05));
        assert_eq!(ledger.entries()[0].fees_paid, dec!(0.05));
    }

    #[test]
    fn record_resolution() {
        let mut ledger = Ledger::new(RunMode::Simulation);
        let id = ledger.record_entry(test_contract(), Side::Buy, dec!(0.55), dec!(10));

        let outcome = VerifiedOutcome {
            contract: test_contract(),
            outcome: ResolutionOutcome::Yes,
            payout_price: dec!(1.0),
            realized_pnl: dec!(4.40),
            verified_at: Utc::now(),
        };
        ledger.record_resolution(id, &outcome);

        assert!(ledger.entries()[0].is_verified());
        assert_eq!(ledger.entries()[0].official_pnl, Some(dec!(4.40)));
    }

    #[test]
    fn summary_counts() {
        let mut ledger = Ledger::new(RunMode::Simulation);
        let id1 = ledger.record_entry(test_contract(), Side::Buy, dec!(0.55), dec!(10));
        let _id2 = ledger.record_entry(test_contract(), Side::Buy, dec!(0.60), dec!(5));
        ledger.record_exit(id1, dec!(0.70), dec!(0.10));

        let summary = ledger.summary();
        assert_eq!(summary.total_trades, 2);
        assert_eq!(summary.open_trades, 1);
        assert_eq!(summary.closed_trades, 1);
        assert_eq!(summary.win_count, 1);
        assert_eq!(summary.mode, "simulation");
    }

    #[test]
    fn open_entries_for_contract() {
        let mut ledger = Ledger::new(RunMode::Simulation);
        let id1 = ledger.record_entry(test_contract(), Side::Buy, dec!(0.55), dec!(10));
        let _id2 = ledger.record_entry(test_contract(), Side::Buy, dec!(0.60), dec!(5));
        ledger.record_exit(id1, dec!(0.70), dec!(0.10));

        let open = ledger.open_entries_for(&test_contract());
        assert_eq!(open.len(), 1);
    }

    #[test]
    fn mode_tagged() {
        let ledger = Ledger::new(RunMode::Paper);
        assert_eq!(ledger.mode(), RunMode::Paper);
    }

    #[test]
    fn summary_serializable() {
        let ledger = Ledger::new(RunMode::Simulation);
        let summary = ledger.summary();
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("simulation"));
    }
}
