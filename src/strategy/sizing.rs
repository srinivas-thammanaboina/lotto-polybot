//! Position sizing: fixed notional, percent-of-equity, and capped fractional Kelly.
//!
//! Pure uncapped Kelly is forbidden. All sizing respects exposure and
//! liquidity limits. Reasons for size reduction are logged.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Sizing mode
// ---------------------------------------------------------------------------

/// Sizing strategy selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SizingMode {
    /// Fixed USDC notional per trade.
    FixedNotional { amount: Decimal },
    /// Percentage of current equity.
    PercentOfEquity { pct: Decimal },
    /// Fractional Kelly with a hard cap (fraction must be < 1.0).
    CappedKelly {
        /// Kelly fraction (e.g. 0.25 = quarter Kelly).
        fraction: Decimal,
        /// Hard maximum notional regardless of Kelly output.
        max_notional: Decimal,
    },
}

// ---------------------------------------------------------------------------
// Sizing input / output
// ---------------------------------------------------------------------------

/// Input for the sizing engine.
#[derive(Debug, Clone)]
pub struct SizingInput {
    /// Sizing mode to use.
    pub mode: SizingMode,
    /// Current equity (bankroll) in USDC.
    pub equity: Decimal,
    /// Net edge from the edge calculator (as a fraction, e.g. 0.05).
    pub net_edge: Decimal,
    /// Fair value probability (used for Kelly computation).
    pub fair_value_prob: Decimal,
    /// Market price / probability we'd trade at.
    pub market_price: Decimal,
    /// Max notional per order from risk config.
    pub max_notional_per_order: Decimal,
    /// Max position size per market from risk config.
    pub max_position_per_market: Decimal,
    /// Current position notional in this market (for remaining capacity).
    pub current_position_notional: Decimal,
    /// Available book depth on our side (USDC).
    pub book_depth_usdc: Decimal,
}

/// Output of the sizing engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SizingOutput {
    /// Final position size in USDC.
    pub notional: Decimal,
    /// Raw size before clips.
    pub raw_notional: Decimal,
    /// Reasons the size was reduced (empty if no clipping).
    pub clips: Vec<SizeClipReason>,
}

/// Why the size was reduced from the raw computation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SizeClipReason {
    MaxNotionalPerOrder,
    MaxPositionPerMarket,
    LiquidityLimit,
    MinimumSize,
    EquityConstraint,
}

impl std::fmt::Display for SizeClipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SizeClipReason::MaxNotionalPerOrder => "max_notional_per_order",
            SizeClipReason::MaxPositionPerMarket => "max_position_per_market",
            SizeClipReason::LiquidityLimit => "liquidity_limit",
            SizeClipReason::MinimumSize => "minimum_size",
            SizeClipReason::EquityConstraint => "equity_constraint",
        };
        write!(f, "{s}")
    }
}

/// Minimum trade size in USDC.
const MIN_NOTIONAL: Decimal = dec!(1);

/// Maximum fraction of book depth we'll consume (to limit market impact).
const MAX_DEPTH_FRACTION: Decimal = dec!(0.25);

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

pub struct SizingEngine;

impl SizingEngine {
    /// Compute the position size for a trade candidate.
    pub fn compute(input: &SizingInput) -> SizingOutput {
        // Step 1: Raw size from the sizing mode
        let raw = match &input.mode {
            SizingMode::FixedNotional { amount } => *amount,
            SizingMode::PercentOfEquity { pct } => input.equity * *pct,
            SizingMode::CappedKelly {
                fraction,
                max_notional,
            } => {
                let kelly_size = Self::kelly_notional(
                    input.equity,
                    input.fair_value_prob,
                    input.market_price,
                    *fraction,
                );
                kelly_size.min(*max_notional)
            }
        };

        let mut size = raw;
        let mut clips = Vec::new();

        // Step 2: Clip by max notional per order
        if size > input.max_notional_per_order {
            size = input.max_notional_per_order;
            clips.push(SizeClipReason::MaxNotionalPerOrder);
        }

        // Step 3: Clip by remaining market capacity
        let remaining = input.max_position_per_market - input.current_position_notional;
        let remaining = remaining.max(Decimal::ZERO);
        if size > remaining {
            size = remaining;
            clips.push(SizeClipReason::MaxPositionPerMarket);
        }

        // Step 4: Clip by liquidity (don't consume more than 25% of book depth)
        let liquidity_limit = input.book_depth_usdc * MAX_DEPTH_FRACTION;
        if size > liquidity_limit {
            size = liquidity_limit;
            clips.push(SizeClipReason::LiquidityLimit);
        }

        // Step 5: Clip by equity (never risk more than 50% of equity in one trade)
        let equity_limit = input.equity * dec!(0.50);
        if size > equity_limit {
            size = equity_limit;
            clips.push(SizeClipReason::EquityConstraint);
        }

        // Step 6: Floor at minimum or zero
        if size < MIN_NOTIONAL {
            if size > Decimal::ZERO {
                clips.push(SizeClipReason::MinimumSize);
            }
            size = Decimal::ZERO;
        }

        if !clips.is_empty() {
            warn!(
                raw = %raw,
                final_size = %size,
                clips = ?clips,
                "size_clipped"
            );
        } else {
            debug!(notional = %size, "size_computed");
        }

        SizingOutput {
            notional: size,
            raw_notional: raw,
            clips,
        }
    }

    /// Fractional Kelly criterion.
    ///
    /// Kelly formula for binary outcomes:
    ///   f* = (p * b - q) / b
    /// where:
    ///   p = estimated probability of winning
    ///   q = 1 - p
    ///   b = odds (payout ratio) = (1 / market_price) - 1
    ///
    /// We then multiply by the kelly fraction (e.g. 0.25 for quarter-Kelly)
    /// and by equity to get the notional.
    fn kelly_notional(
        equity: Decimal,
        fair_value_prob: Decimal,
        market_price: Decimal,
        fraction: Decimal,
    ) -> Decimal {
        if market_price <= Decimal::ZERO || market_price >= dec!(1) {
            return Decimal::ZERO;
        }

        let p = fair_value_prob;
        let q = dec!(1) - p;
        let b = (dec!(1) / market_price) - dec!(1);

        if b <= Decimal::ZERO {
            return Decimal::ZERO;
        }

        let kelly_f = (p * b - q) / b;

        // Never bet on negative edge
        if kelly_f <= Decimal::ZERO {
            return Decimal::ZERO;
        }

        // Apply fraction cap (e.g. quarter-Kelly)
        let capped_f = kelly_f * fraction;

        (equity * capped_f).max(Decimal::ZERO)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> SizingInput {
        SizingInput {
            mode: SizingMode::FixedNotional { amount: dec!(10) },
            equity: dec!(500),
            net_edge: dec!(0.05),
            fair_value_prob: dec!(0.65),
            market_price: dec!(0.55),
            max_notional_per_order: dec!(25),
            max_position_per_market: dec!(50),
            current_position_notional: Decimal::ZERO,
            book_depth_usdc: dec!(200),
        }
    }

    #[test]
    fn fixed_notional_returns_amount() {
        let out = SizingEngine::compute(&base_input());
        assert_eq!(out.notional, dec!(10));
        assert!(out.clips.is_empty());
    }

    #[test]
    fn percent_of_equity() {
        let mut input = base_input();
        input.mode = SizingMode::PercentOfEquity { pct: dec!(0.02) };
        let out = SizingEngine::compute(&input);
        assert_eq!(out.notional, dec!(10)); // 2% of 500
    }

    #[test]
    fn capped_kelly_positive_edge() {
        let mut input = base_input();
        input.mode = SizingMode::CappedKelly {
            fraction: dec!(0.25),
            max_notional: dec!(50),
        };
        let out = SizingEngine::compute(&input);
        assert!(out.notional > Decimal::ZERO);
        assert!(out.notional <= dec!(50));
    }

    #[test]
    fn capped_kelly_no_edge_returns_zero() {
        let mut input = base_input();
        input.fair_value_prob = dec!(0.40); // Below market price, negative edge
        input.market_price = dec!(0.55);
        input.mode = SizingMode::CappedKelly {
            fraction: dec!(0.25),
            max_notional: dec!(50),
        };
        let out = SizingEngine::compute(&input);
        assert_eq!(out.notional, Decimal::ZERO);
    }

    #[test]
    fn clipped_by_max_notional() {
        let mut input = base_input();
        input.mode = SizingMode::FixedNotional { amount: dec!(100) };
        input.max_notional_per_order = dec!(25);
        let out = SizingEngine::compute(&input);
        assert_eq!(out.notional, dec!(25));
        assert!(
            out.clips
                .iter()
                .any(|c| matches!(c, SizeClipReason::MaxNotionalPerOrder))
        );
    }

    #[test]
    fn clipped_by_market_position_limit() {
        let mut input = base_input();
        input.mode = SizingMode::FixedNotional { amount: dec!(20) };
        input.max_position_per_market = dec!(50);
        input.current_position_notional = dec!(45);
        let out = SizingEngine::compute(&input);
        assert_eq!(out.notional, dec!(5));
        assert!(
            out.clips
                .iter()
                .any(|c| matches!(c, SizeClipReason::MaxPositionPerMarket))
        );
    }

    #[test]
    fn clipped_by_liquidity() {
        let mut input = base_input();
        input.mode = SizingMode::FixedNotional { amount: dec!(100) };
        input.max_notional_per_order = dec!(200);
        input.book_depth_usdc = dec!(100); // 25% = 25
        let out = SizingEngine::compute(&input);
        assert_eq!(out.notional, dec!(25));
        assert!(
            out.clips
                .iter()
                .any(|c| matches!(c, SizeClipReason::LiquidityLimit))
        );
    }

    #[test]
    fn below_minimum_returns_zero() {
        let mut input = base_input();
        input.mode = SizingMode::FixedNotional { amount: dec!(0.50) };
        let out = SizingEngine::compute(&input);
        assert_eq!(out.notional, Decimal::ZERO);
        assert!(
            out.clips
                .iter()
                .any(|c| matches!(c, SizeClipReason::MinimumSize))
        );
    }

    #[test]
    fn equity_constraint_clips() {
        let mut input = base_input();
        input.mode = SizingMode::FixedNotional { amount: dec!(300) };
        input.max_notional_per_order = dec!(500);
        input.equity = dec!(500);
        // 50% of 500 = 250
        let out = SizingEngine::compute(&input);
        assert!(out.notional <= dec!(250));
    }

    #[test]
    fn kelly_fraction_caps_full_kelly() {
        let mut input = base_input();
        let full_kelly = SizingMode::CappedKelly {
            fraction: dec!(1.0),
            max_notional: dec!(1000),
        };
        let quarter_kelly = SizingMode::CappedKelly {
            fraction: dec!(0.25),
            max_notional: dec!(1000),
        };

        input.mode = full_kelly;
        let full_out = SizingEngine::compute(&input);

        input.mode = quarter_kelly;
        let quarter_out = SizingEngine::compute(&input);

        assert!(quarter_out.raw_notional < full_out.raw_notional);
    }

    #[test]
    fn zero_market_price_returns_zero() {
        let mut input = base_input();
        input.market_price = Decimal::ZERO;
        input.mode = SizingMode::CappedKelly {
            fraction: dec!(0.25),
            max_notional: dec!(50),
        };
        let out = SizingEngine::compute(&input);
        assert_eq!(out.notional, Decimal::ZERO);
    }

    #[test]
    fn full_position_returns_zero() {
        let mut input = base_input();
        input.current_position_notional = dec!(50);
        input.max_position_per_market = dec!(50);
        let out = SizingEngine::compute(&input);
        assert_eq!(out.notional, Decimal::ZERO);
    }
}
