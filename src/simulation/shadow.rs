//! Paper/live shadow mode.
//!
//! Runs against real feeds, generates real signal decisions, but does NOT
//! submit real orders. All decisions are tagged as shadow for comparison
//! with later live runs.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::domain::market::ContractKey;
use crate::domain::signal::{OrderIntent, Side, SignalDecision};

// ---------------------------------------------------------------------------
// Shadow decision record
// ---------------------------------------------------------------------------

/// A shadow-mode decision record for later comparison with live.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowDecision {
    pub timestamp: DateTime<Utc>,
    pub contract: ContractKey,
    pub accepted: bool,
    /// The intent that would have been submitted (if accepted).
    pub intent: Option<ShadowIntent>,
    /// Why rejected (if rejected).
    pub reject_reasons: Vec<String>,
    /// Internal processing latency (signal evaluation time).
    pub processing_latency_us: i64,
}

/// Shadow-mode intent snapshot (no real submission).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowIntent {
    pub side: Side,
    pub target_price: Decimal,
    pub size: Decimal,
    pub fair_value: Decimal,
    pub gross_edge: Decimal,
    pub net_edge: Decimal,
    pub model_version: String,
}

impl From<&OrderIntent> for ShadowIntent {
    fn from(intent: &OrderIntent) -> Self {
        Self {
            side: intent.side,
            target_price: intent.target_price,
            size: intent.size,
            fair_value: intent.fair_value,
            gross_edge: intent.gross_edge,
            net_edge: intent.net_edge,
            model_version: intent.model_version.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Shadow session
// ---------------------------------------------------------------------------

/// Shadow session stats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowStats {
    pub mode: String,
    pub decisions_recorded: u64,
    pub would_accept: u64,
    pub would_reject: u64,
    pub total_hypothetical_notional: Decimal,
    pub avg_net_edge: Decimal,
    pub started_at: DateTime<Utc>,
}

/// Shadow mode session. Records what the bot *would* do without doing it.
pub struct ShadowSession {
    decisions: Vec<ShadowDecision>,
    stats: ShadowStats,
}

impl ShadowSession {
    pub fn new() -> Self {
        Self {
            decisions: Vec::new(),
            stats: ShadowStats {
                mode: "shadow".into(),
                decisions_recorded: 0,
                would_accept: 0,
                would_reject: 0,
                total_hypothetical_notional: Decimal::ZERO,
                avg_net_edge: Decimal::ZERO,
                started_at: Utc::now(),
            },
        }
    }

    /// Record a signal decision with its processing latency.
    pub fn record_decision(&mut self, decision: &SignalDecision, processing_latency_us: i64) {
        let now = Utc::now();
        self.stats.decisions_recorded += 1;

        let shadow = match decision {
            SignalDecision::Accept(intent) => {
                self.stats.would_accept += 1;
                self.stats.total_hypothetical_notional += intent.size;

                // Running average of net edge
                let n = Decimal::from(self.stats.would_accept);
                self.stats.avg_net_edge =
                    self.stats.avg_net_edge * (n - dec!(1)) / n + intent.net_edge / n;

                info!(
                    contract = %intent.contract,
                    side = %intent.side,
                    net_edge = %intent.net_edge,
                    size = %intent.size,
                    "shadow_would_accept"
                );

                ShadowDecision {
                    timestamp: now,
                    contract: intent.contract.clone(),
                    accepted: true,
                    intent: Some(ShadowIntent::from(intent.as_ref())),
                    reject_reasons: Vec::new(),
                    processing_latency_us,
                }
            }
            SignalDecision::Reject {
                contract, reasons, ..
            } => {
                self.stats.would_reject += 1;

                debug!(
                    contract = %contract,
                    reasons = ?reasons,
                    "shadow_would_reject"
                );

                ShadowDecision {
                    timestamp: now,
                    contract: contract.clone(),
                    accepted: false,
                    intent: None,
                    reject_reasons: reasons.iter().map(|r| r.to_string()).collect(),
                    processing_latency_us,
                }
            }
        };

        self.decisions.push(shadow);
    }

    /// Get current stats.
    pub fn stats(&self) -> &ShadowStats {
        &self.stats
    }

    /// Get all recorded decisions.
    pub fn decisions(&self) -> &[ShadowDecision] {
        &self.decisions
    }

    /// Export decisions as JSON for comparison with live runs.
    pub fn export_json(&self) -> String {
        serde_json::to_string_pretty(&self.decisions).unwrap_or_else(|_| "[]".into())
    }

    /// Get the acceptance rate.
    pub fn acceptance_rate(&self) -> Decimal {
        if self.stats.decisions_recorded == 0 {
            return Decimal::ZERO;
        }
        Decimal::from(self.stats.would_accept) / Decimal::from(self.stats.decisions_recorded)
    }
}

impl Default for ShadowSession {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{Asset, MarketDuration, MarketId, TokenId};
    use crate::domain::signal::RejectReason;
    use crate::strategy::edge::CostSnapshot;

    fn test_intent() -> OrderIntent {
        OrderIntent {
            contract: ContractKey {
                market_id: MarketId("mkt1".into()),
                token_id: TokenId("tok1".into()),
            },
            asset: Asset::BTC,
            duration: MarketDuration::FiveMin,
            side: Side::Buy,
            target_price: dec!(0.55),
            size: dec!(10),
            fair_value: dec!(0.65),
            gross_edge: dec!(0.10),
            net_edge: dec!(0.05),
            cost_snapshot: CostSnapshot {
                fee_rate: dec!(0.01),
                entry_fee_usdc: dec!(0.10),
                exit_fee_usdc: dec!(0.10),
                entry_slippage: dec!(0.005),
                exit_slippage: dec!(0.0075),
                latency_decay: dec!(0.0025),
                total_cost_frac: dec!(0.05),
            },
            rationale: "test".into(),
            model_version: "fv-v1.0".into(),
            signal_timestamp: Utc::now(),
        }
    }

    fn test_accept() -> SignalDecision {
        SignalDecision::Accept(Box::new(test_intent()))
    }

    fn test_reject() -> SignalDecision {
        SignalDecision::Reject {
            contract: ContractKey {
                market_id: MarketId("mkt1".into()),
                token_id: TokenId("tok1".into()),
            },
            reasons: vec![RejectReason::StaleFeed, RejectReason::BelowEdgeThreshold],
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn records_accept() {
        let mut session = ShadowSession::new();
        session.record_decision(&test_accept(), 100);

        assert_eq!(session.stats().decisions_recorded, 1);
        assert_eq!(session.stats().would_accept, 1);
        assert_eq!(session.decisions().len(), 1);
        assert!(session.decisions()[0].accepted);
        assert!(session.decisions()[0].intent.is_some());
    }

    #[test]
    fn records_reject() {
        let mut session = ShadowSession::new();
        session.record_decision(&test_reject(), 50);

        assert_eq!(session.stats().would_reject, 1);
        assert!(!session.decisions()[0].accepted);
        assert_eq!(session.decisions()[0].reject_reasons.len(), 2);
    }

    #[test]
    fn acceptance_rate() {
        let mut session = ShadowSession::new();
        session.record_decision(&test_accept(), 100);
        session.record_decision(&test_reject(), 50);
        session.record_decision(&test_accept(), 100);

        // 2/3 accepted
        let rate = session.acceptance_rate();
        assert!(rate > dec!(0.6) && rate < dec!(0.7));
    }

    #[test]
    fn hypothetical_notional_tracked() {
        let mut session = ShadowSession::new();
        session.record_decision(&test_accept(), 100);
        session.record_decision(&test_accept(), 100);

        assert_eq!(session.stats().total_hypothetical_notional, dec!(20));
    }

    #[test]
    fn export_json_valid() {
        let mut session = ShadowSession::new();
        session.record_decision(&test_accept(), 100);
        let json = session.export_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 1);
    }

    #[test]
    fn shadow_mode_tag() {
        let session = ShadowSession::new();
        assert_eq!(session.stats().mode, "shadow");
    }

    #[test]
    fn processing_latency_recorded() {
        let mut session = ShadowSession::new();
        session.record_decision(&test_accept(), 250);
        assert_eq!(session.decisions()[0].processing_latency_us, 250);
    }
}
