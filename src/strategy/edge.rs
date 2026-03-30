//! Net edge computation after fees, slippage, and latency decay.
//!
//! The cost model is pluggable: fee schedules and slippage estimators are
//! injected rather than hardcoded, so schedule changes require only config
//! updates.

use std::time::Duration;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::domain::market::MarketDuration;

// ---------------------------------------------------------------------------
// Fee model
// ---------------------------------------------------------------------------

/// Polymarket fee schedule. Pluggable so schedule changes don't require
/// large refactors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeSchedule {
    /// Maker fee rate (e.g. 0.00 for current Polymarket zero-maker-fee).
    pub maker_rate: Decimal,
    /// Taker fee rate (e.g. 0.02 = 2%).
    pub taker_rate: Decimal,
    /// Whether the fee scales with probability (Polymarket's actual model:
    /// fee = taker_rate * min(p, 1-p), so fee is lower near 0 and 1).
    pub probability_scaled: bool,
}

impl Default for FeeSchedule {
    fn default() -> Self {
        Self {
            maker_rate: dec!(0.00),
            taker_rate: dec!(0.02),
            probability_scaled: true,
        }
    }
}

impl FeeSchedule {
    /// Compute the effective fee rate for a trade at a given probability.
    pub fn effective_rate(&self, probability: Decimal) -> Decimal {
        if self.probability_scaled {
            // Polymarket model: fee = taker_rate * min(p, 1-p)
            let p_min = probability.min(dec!(1) - probability);
            self.taker_rate * p_min
        } else {
            self.taker_rate
        }
    }

    /// Compute the total fee in USDC for a given notional and probability.
    pub fn fee_usdc(&self, notional: Decimal, probability: Decimal) -> Decimal {
        notional * self.effective_rate(probability)
    }
}

// ---------------------------------------------------------------------------
// Slippage model
// ---------------------------------------------------------------------------

/// Slippage estimates for entry and exit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlippageEstimate {
    /// Expected entry slippage as a fraction of price (e.g. 0.005 = 0.5%).
    pub entry: Decimal,
    /// Expected exit slippage as a fraction of price.
    pub exit: Decimal,
}

impl Default for SlippageEstimate {
    fn default() -> Self {
        Self {
            entry: dec!(0.005),
            exit: dec!(0.01),
        }
    }
}

impl SlippageEstimate {
    /// Estimate slippage from order book depth.
    /// More depth → less slippage.
    pub fn from_book_depth(order_size: Decimal, available_depth: Decimal) -> Self {
        let entry = if available_depth > Decimal::ZERO {
            // Slippage increases as order size approaches available depth
            let fill_ratio = order_size / available_depth;
            (fill_ratio * dec!(0.02)).min(dec!(0.05))
        } else {
            dec!(0.05) // Max slippage when no depth
        };

        // Exit slippage is typically worse (thinner market, urgency)
        let exit = (entry * dec!(1.5)).min(dec!(0.05));

        Self { entry, exit }
    }

    /// Total round-trip slippage cost as a fraction.
    pub fn round_trip(&self) -> Decimal {
        self.entry + self.exit
    }
}

// ---------------------------------------------------------------------------
// Cost model snapshot
// ---------------------------------------------------------------------------

/// Complete cost model snapshot for a single trade candidate.
/// Attached to order intents for replay/debug.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSnapshot {
    pub fee_rate: Decimal,
    pub entry_fee_usdc: Decimal,
    pub exit_fee_usdc: Decimal,
    pub entry_slippage: Decimal,
    pub exit_slippage: Decimal,
    pub latency_decay: Decimal,
    pub total_cost_frac: Decimal,
}

// ---------------------------------------------------------------------------
// Edge computation
// ---------------------------------------------------------------------------

/// Input to the edge calculator.
#[derive(Debug, Clone)]
pub struct EdgeInput {
    /// Fair value probability from the FV engine.
    pub fair_value_prob: Decimal,
    /// Current best market price (as probability 0..1).
    pub market_price: Decimal,
    /// Notional size in USDC (for fee computation).
    pub notional: Decimal,
    /// Available book depth in USDC on the side we're trading.
    pub book_depth_usdc: Decimal,
    /// Market duration (affects latency decay).
    pub duration: MarketDuration,
    /// Fee schedule to use.
    pub fees: FeeSchedule,
    /// Latency decay buffer from config.
    pub latency_decay_buffer: Duration,
    /// Signal age (time since fair value was computed).
    pub signal_age: Duration,
    /// Timestamp of computation.
    pub timestamp: DateTime<Utc>,
}

/// Output of the edge calculator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeEstimate {
    /// Gross edge: |fair_value - market_price|.
    pub gross_edge: Decimal,
    /// Net edge after all costs.
    pub net_edge: Decimal,
    /// Full cost breakdown.
    pub costs: CostSnapshot,
    /// Whether this trade is profitable after costs.
    pub is_profitable: bool,
}

/// Stateless edge calculator.
pub struct EdgeCalculator;

impl EdgeCalculator {
    /// Compute net edge for a trade candidate.
    pub fn compute(input: &EdgeInput) -> EdgeEstimate {
        // Gross edge: absolute divergence between fair value and market
        let gross_edge = (input.fair_value_prob - input.market_price).abs();

        // Fee computation (entry + exit)
        let fee_rate = input.fees.effective_rate(input.market_price);
        let entry_fee = input.fees.fee_usdc(input.notional, input.market_price);
        // Exit at fair value probability
        let exit_fee = input.fees.fee_usdc(input.notional, input.fair_value_prob);
        let total_fees_frac = if input.notional > Decimal::ZERO {
            (entry_fee + exit_fee) / input.notional
        } else {
            Decimal::ZERO
        };

        // Slippage
        let slippage = SlippageEstimate::from_book_depth(input.notional, input.book_depth_usdc);

        // Latency decay: the edge decays as time passes.
        // Model as a linear penalty based on signal age relative to buffer.
        let buffer_ms = input.latency_decay_buffer.as_millis() as u64;
        let age_ms = input.signal_age.as_millis() as u64;
        let latency_decay = if buffer_ms > 0 {
            let decay_ratio = Decimal::from(age_ms) / Decimal::from(buffer_ms);
            // Decay penalty scales with how much of the buffer is consumed
            (decay_ratio * dec!(0.01)).min(gross_edge)
        } else {
            Decimal::ZERO
        };

        // Total cost as fraction of probability space
        let total_cost = total_fees_frac + slippage.round_trip() + latency_decay;

        // Net edge
        let net_edge = gross_edge - total_cost;
        let is_profitable = net_edge > Decimal::ZERO;

        let costs = CostSnapshot {
            fee_rate,
            entry_fee_usdc: entry_fee,
            exit_fee_usdc: exit_fee,
            entry_slippage: slippage.entry,
            exit_slippage: slippage.exit,
            latency_decay,
            total_cost_frac: total_cost,
        };

        debug!(
            gross_edge = %gross_edge,
            net_edge = %net_edge,
            total_cost = %total_cost,
            fee_rate = %fee_rate,
            entry_slip = %slippage.entry,
            exit_slip = %slippage.exit,
            latency_decay = %latency_decay,
            profitable = is_profitable,
            "edge_computed"
        );

        EdgeEstimate {
            gross_edge,
            net_edge,
            costs,
            is_profitable,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> EdgeInput {
        EdgeInput {
            fair_value_prob: dec!(0.65),
            market_price: dec!(0.55),
            notional: dec!(10),
            book_depth_usdc: dec!(200),
            duration: MarketDuration::FiveMin,
            fees: FeeSchedule::default(),
            latency_decay_buffer: Duration::from_millis(200),
            signal_age: Duration::from_millis(50),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn gross_edge_is_correct() {
        let input = base_input();
        let est = EdgeCalculator::compute(&input);
        assert_eq!(est.gross_edge, dec!(0.10));
    }

    #[test]
    fn net_edge_less_than_gross() {
        let est = EdgeCalculator::compute(&base_input());
        assert!(est.net_edge < est.gross_edge);
    }

    #[test]
    fn profitable_when_edge_survives_costs() {
        let est = EdgeCalculator::compute(&base_input());
        // 10% gross edge on small size should survive costs
        assert!(est.is_profitable);
        assert!(est.net_edge > Decimal::ZERO);
    }

    #[test]
    fn tiny_edge_is_unprofitable() {
        let mut input = base_input();
        input.fair_value_prob = dec!(0.551); // 0.1% gross edge
        input.market_price = dec!(0.550);
        let est = EdgeCalculator::compute(&input);
        assert!(!est.is_profitable);
    }

    #[test]
    fn fee_schedule_probability_scaling() {
        let fees = FeeSchedule::default();
        // At p=0.50: rate = 0.02 * 0.50 = 0.01
        assert_eq!(fees.effective_rate(dec!(0.50)), dec!(0.0100));
        // At p=0.10: rate = 0.02 * 0.10 = 0.002
        assert_eq!(fees.effective_rate(dec!(0.10)), dec!(0.002));
        // At p=0.90: rate = 0.02 * 0.10 = 0.002 (symmetric)
        assert_eq!(fees.effective_rate(dec!(0.90)), dec!(0.002));
    }

    #[test]
    fn flat_fee_schedule() {
        let fees = FeeSchedule {
            maker_rate: dec!(0.00),
            taker_rate: dec!(0.02),
            probability_scaled: false,
        };
        assert_eq!(fees.effective_rate(dec!(0.50)), dec!(0.02));
        assert_eq!(fees.effective_rate(dec!(0.10)), dec!(0.02));
    }

    #[test]
    fn slippage_increases_with_fill_ratio() {
        let small = SlippageEstimate::from_book_depth(dec!(5), dec!(200));
        let large = SlippageEstimate::from_book_depth(dec!(100), dec!(200));
        assert!(large.entry > small.entry);
    }

    #[test]
    fn exit_slippage_worse_than_entry() {
        let slip = SlippageEstimate::from_book_depth(dec!(10), dec!(200));
        assert!(slip.exit > slip.entry);
    }

    #[test]
    fn zero_depth_gives_max_slippage() {
        let slip = SlippageEstimate::from_book_depth(dec!(10), Decimal::ZERO);
        assert_eq!(slip.entry, dec!(0.05));
    }

    #[test]
    fn latency_decay_increases_with_age() {
        let mut input = base_input();
        input.signal_age = Duration::from_millis(10);
        let fresh = EdgeCalculator::compute(&input);

        input.signal_age = Duration::from_millis(180);
        let stale = EdgeCalculator::compute(&input);

        assert!(stale.costs.latency_decay > fresh.costs.latency_decay);
        assert!(stale.net_edge < fresh.net_edge);
    }

    #[test]
    fn cost_snapshot_is_populated() {
        let est = EdgeCalculator::compute(&base_input());
        assert!(est.costs.entry_fee_usdc >= Decimal::ZERO);
        assert!(est.costs.exit_fee_usdc >= Decimal::ZERO);
        assert!(est.costs.entry_slippage >= Decimal::ZERO);
        assert!(est.costs.exit_slippage >= Decimal::ZERO);
        assert!(est.costs.total_cost_frac > Decimal::ZERO);
    }
}
