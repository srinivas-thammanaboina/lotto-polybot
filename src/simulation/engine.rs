//! Simulation mode engine.
//!
//! Runs the full strategy pipeline (fair value → edge → gates → sizing → intent)
//! with simulated fills. Never touches real endpoints. Telemetry mirrors
//! the live schema so sim and live data are comparable.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::config::RunMode;
use crate::domain::market::ContractKey;
use crate::domain::signal::{OrderIntent, Side, SignalDecision};
use crate::telemetry::ledger::Ledger;

// ---------------------------------------------------------------------------
// Simulated fill model
// ---------------------------------------------------------------------------

/// How the simulation fills orders.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum FillModel {
    /// Instant fill at the target price (optimistic).
    InstantFill,
    /// Fill at target price + slippage (from cost snapshot).
    #[default]
    WithSlippage,
    /// Fill with configurable probability.
    Probabilistic { fill_rate: f64 },
}

/// Result of a simulated fill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatedFill {
    pub contract: ContractKey,
    pub side: Side,
    pub fill_price: Decimal,
    pub size: Decimal,
    pub fee: Decimal,
    pub filled: bool,
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Fill simulator
// ---------------------------------------------------------------------------

/// Simulates fills for order intents.
pub struct FillSimulator {
    model: FillModel,
}

impl FillSimulator {
    pub fn new(model: FillModel) -> Self {
        Self { model }
    }

    /// Simulate filling an order intent.
    pub fn simulate_fill(&self, intent: &OrderIntent) -> SimulatedFill {
        let now = Utc::now();

        match &self.model {
            FillModel::InstantFill => SimulatedFill {
                contract: intent.contract.clone(),
                side: intent.side,
                fill_price: intent.target_price,
                size: intent.size,
                fee: intent.cost_snapshot.entry_fee_usdc,
                filled: true,
                timestamp: now,
            },
            FillModel::WithSlippage => {
                let slippage = intent.cost_snapshot.entry_slippage;
                let fill_price = match intent.side {
                    Side::Buy => intent.target_price + slippage,
                    Side::Sell => intent.target_price - slippage,
                };
                SimulatedFill {
                    contract: intent.contract.clone(),
                    side: intent.side,
                    fill_price,
                    size: intent.size,
                    fee: intent.cost_snapshot.entry_fee_usdc,
                    filled: true,
                    timestamp: now,
                }
            }
            FillModel::Probabilistic { fill_rate } => {
                // Deterministic "random" based on timestamp nanos for reproducibility
                let nanos = now.timestamp_subsec_nanos() as f64;
                let pseudo_random = (nanos % 1000.0) / 1000.0;
                let filled = pseudo_random < *fill_rate;

                SimulatedFill {
                    contract: intent.contract.clone(),
                    side: intent.side,
                    fill_price: intent.target_price,
                    size: if filled { intent.size } else { Decimal::ZERO },
                    fee: if filled {
                        intent.cost_snapshot.entry_fee_usdc
                    } else {
                        Decimal::ZERO
                    },
                    filled,
                    timestamp: now,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Simulation session
// ---------------------------------------------------------------------------

/// Tracks simulation session state and results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationStats {
    pub mode: String,
    pub signals_evaluated: u64,
    pub signals_accepted: u64,
    pub signals_rejected: u64,
    pub fills_simulated: u64,
    pub fills_missed: u64,
    pub total_pnl: Decimal,
    pub total_fees: Decimal,
    pub started_at: DateTime<Utc>,
    pub events_processed: u64,
}

/// A simulation session that processes events and tracks results.
pub struct SimulationSession {
    stats: SimulationStats,
    fill_simulator: FillSimulator,
    ledger: Ledger,
}

impl SimulationSession {
    pub fn new(fill_model: FillModel) -> Self {
        Self {
            stats: SimulationStats {
                mode: "simulation".into(),
                signals_evaluated: 0,
                signals_accepted: 0,
                signals_rejected: 0,
                fills_simulated: 0,
                fills_missed: 0,
                total_pnl: Decimal::ZERO,
                total_fees: Decimal::ZERO,
                started_at: Utc::now(),
                events_processed: 0,
            },
            fill_simulator: FillSimulator::new(fill_model),
            ledger: Ledger::new(RunMode::Simulation),
        }
    }

    /// Process a signal decision from the pipeline.
    pub fn process_signal(&mut self, decision: &SignalDecision) {
        self.stats.signals_evaluated += 1;

        match decision {
            SignalDecision::Accept(intent) => {
                self.stats.signals_accepted += 1;

                // Simulate fill
                let fill = self.fill_simulator.simulate_fill(intent);

                if fill.filled {
                    self.stats.fills_simulated += 1;
                    self.stats.total_fees += fill.fee;

                    // Record in ledger
                    let entry_id = self.ledger.record_entry(
                        fill.contract.clone(),
                        fill.side,
                        fill.fill_price,
                        fill.size,
                    );
                    self.ledger.add_fees(entry_id, fill.fee);

                    info!(
                        contract = %fill.contract,
                        side = %fill.side,
                        price = %fill.fill_price,
                        size = %fill.size,
                        fee = %fill.fee,
                        "sim_fill"
                    );
                } else {
                    self.stats.fills_missed += 1;
                    debug!(contract = %intent.contract, "sim_fill_missed");
                }
            }
            SignalDecision::Reject {
                contract, reasons, ..
            } => {
                self.stats.signals_rejected += 1;
                debug!(
                    contract = %contract,
                    reasons = ?reasons,
                    "sim_signal_rejected"
                );
            }
        }
    }

    /// Record an event processed (for metrics).
    pub fn record_event(&mut self) {
        self.stats.events_processed += 1;
    }

    /// Simulate closing a position at a given exit price.
    pub fn close_position(
        &mut self,
        ledger_id: u64,
        exit_price: Decimal,
        exit_fee: Decimal,
    ) -> Option<Decimal> {
        let pnl = self.ledger.record_exit(ledger_id, exit_price, exit_fee)?;
        self.stats.total_pnl += pnl;
        self.stats.total_fees += exit_fee;
        info!(id = ledger_id, pnl = %pnl, "sim_position_closed");
        Some(pnl)
    }

    /// Get current simulation stats.
    pub fn stats(&self) -> &SimulationStats {
        &self.stats
    }

    /// Get the simulation ledger.
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// Get a mutable reference to the ledger (for resolution recording).
    pub fn ledger_mut(&mut self) -> &mut Ledger {
        &mut self.ledger
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
    use rust_decimal_macros::dec;

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

    #[test]
    fn instant_fill_at_target_price() {
        let sim = FillSimulator::new(FillModel::InstantFill);
        let intent = test_intent();
        let fill = sim.simulate_fill(&intent);
        assert!(fill.filled);
        assert_eq!(fill.fill_price, dec!(0.55));
        assert_eq!(fill.size, dec!(10));
    }

    #[test]
    fn slippage_fill_adjusts_price() {
        let sim = FillSimulator::new(FillModel::WithSlippage);
        let intent = test_intent();
        let fill = sim.simulate_fill(&intent);
        assert!(fill.filled);
        // Buy: price + slippage = 0.55 + 0.005 = 0.555
        assert_eq!(fill.fill_price, dec!(0.555));
    }

    #[test]
    fn sell_slippage_reduces_price() {
        let sim = FillSimulator::new(FillModel::WithSlippage);
        let mut intent = test_intent();
        intent.side = Side::Sell;
        let fill = sim.simulate_fill(&intent);
        // Sell: price - slippage = 0.55 - 0.005 = 0.545
        assert_eq!(fill.fill_price, dec!(0.545));
    }

    #[test]
    fn session_tracks_accepted_signals() {
        let mut session = SimulationSession::new(FillModel::InstantFill);
        let decision = SignalDecision::Accept(Box::new(test_intent()));
        session.process_signal(&decision);

        assert_eq!(session.stats().signals_evaluated, 1);
        assert_eq!(session.stats().signals_accepted, 1);
        assert_eq!(session.stats().fills_simulated, 1);
    }

    #[test]
    fn session_tracks_rejected_signals() {
        let mut session = SimulationSession::new(FillModel::InstantFill);
        let decision = SignalDecision::Reject {
            contract: ContractKey {
                market_id: MarketId("mkt1".into()),
                token_id: TokenId("tok1".into()),
            },
            reasons: vec![RejectReason::StaleFeed],
            timestamp: Utc::now(),
        };
        session.process_signal(&decision);

        assert_eq!(session.stats().signals_evaluated, 1);
        assert_eq!(session.stats().signals_rejected, 1);
    }

    #[test]
    fn session_records_in_ledger() {
        let mut session = SimulationSession::new(FillModel::InstantFill);
        let decision = SignalDecision::Accept(Box::new(test_intent()));
        session.process_signal(&decision);

        let summary = session.ledger().summary();
        assert_eq!(summary.total_trades, 1);
        assert_eq!(summary.open_trades, 1);
        assert_eq!(summary.mode, "simulation");
    }

    #[test]
    fn session_close_position() {
        let mut session = SimulationSession::new(FillModel::InstantFill);
        let decision = SignalDecision::Accept(Box::new(test_intent()));
        session.process_signal(&decision);

        let pnl = session.close_position(1, dec!(0.65), dec!(0.10)).unwrap();
        assert!(pnl > Decimal::ZERO);
        assert!(session.stats().total_pnl > Decimal::ZERO);
    }

    #[test]
    fn session_stats_mode() {
        let session = SimulationSession::new(FillModel::default());
        assert_eq!(session.stats().mode, "simulation");
    }

    #[test]
    fn event_counter() {
        let mut session = SimulationSession::new(FillModel::default());
        session.record_event();
        session.record_event();
        assert_eq!(session.stats().events_processed, 2);
    }
}
