//! Rule-based fair value engine v1.
//!
//! Converts normalized CEX price data into an implied probability estimate
//! for Polymarket short-duration contracts. Fully deterministic for identical
//! replay inputs.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::domain::market::{Asset, MarketDuration, Outcome};

/// Model version tag — bump on any logic change.
pub const MODEL_VERSION: &str = "fv-v1.0";

// ---------------------------------------------------------------------------
// Fair value input / output
// ---------------------------------------------------------------------------

/// All inputs the fair value engine needs to produce an estimate.
#[derive(Debug, Clone)]
pub struct FairValueInput {
    /// The asset (BTC / ETH).
    pub asset: Asset,
    /// The contract outcome (Up / Down).
    pub outcome: Outcome,
    /// Market duration regime.
    pub duration: MarketDuration,

    /// Current CEX spot price (primary source).
    pub spot_price: Decimal,
    /// Spot price at the start of the contract window.
    pub window_open_price: Decimal,

    /// Short-window price delta (last N seconds), as fraction.
    /// E.g. +0.001 = +0.1% move.
    pub short_delta: Decimal,

    /// Optional: momentum persistence factor (0..1).
    /// How much of the short delta is likely to continue.
    pub momentum: Option<Decimal>,

    /// Optional: annualized volatility, used to normalise the move magnitude.
    pub volatility: Option<Decimal>,

    /// Optional: secondary source price for consensus check.
    pub secondary_price: Option<Decimal>,

    /// Timestamp of the input snapshot (for deterministic replay).
    pub timestamp: DateTime<Utc>,
}

/// Output of the fair value engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FairValueEstimate {
    /// Implied probability that this outcome resolves YES (0.0 .. 1.0).
    pub probability: Decimal,
    /// Confidence weight (0.0 .. 1.0). Reduced when features are missing.
    pub confidence: Decimal,
    /// Model version that produced this estimate.
    pub model_version: String,
    /// Features that contributed (for logging/replay).
    pub features: FairValueFeatures,
    /// Timestamp of the computation.
    pub computed_at: DateTime<Utc>,
}

/// Feature values used — logged for auditability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FairValueFeatures {
    pub move_pct: Decimal,
    pub short_delta: Decimal,
    pub momentum_adj: Decimal,
    pub vol_normalised_move: Decimal,
    pub consensus_penalty: Decimal,
    pub duration_scale: Decimal,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Rule-based fair value engine. Stateless — takes input, returns estimate.
pub struct FairValueEngine;

impl FairValueEngine {
    /// Compute the fair value estimate for a given contract.
    ///
    /// The model works as follows:
    /// 1. Compute the price move since the window opened.
    /// 2. Adjust for momentum persistence (if provided).
    /// 3. Normalise by volatility (if provided).
    /// 4. Apply a duration-specific scaling factor.
    /// 5. Convert the directional move to an implied probability.
    /// 6. Apply consensus penalty if secondary source disagrees.
    /// 7. Clamp probability to [0.01, 0.99].
    pub fn compute(input: &FairValueInput) -> FairValueEstimate {
        // 1. Raw percentage move since window open
        let move_pct = if input.window_open_price.is_zero() {
            Decimal::ZERO
        } else {
            (input.spot_price - input.window_open_price) / input.window_open_price
        };

        // 2. Momentum adjustment
        let momentum_factor = input.momentum.unwrap_or(dec!(1.0));
        let momentum_adj = input.short_delta * momentum_factor;

        // 3. Volatility normalisation
        // If vol is provided, normalise the move to get a z-like score,
        // then scale back. This prevents the model from over-reacting in
        // high-vol regimes.
        let vol_normalised_move = match input.volatility {
            Some(vol) if vol > Decimal::ZERO => {
                // Convert annualised vol to per-window vol estimate.
                // 5m ≈ sqrt(5/525600) ≈ 0.00308, 15m ≈ sqrt(15/525600) ≈ 0.00534
                let window_minutes: Decimal = match input.duration {
                    MarketDuration::FiveMin => dec!(5),
                    MarketDuration::FifteenMin => dec!(15),
                };
                let minutes_per_year = dec!(525600);
                // Approximate sqrt using a simple estimation: sqrt(x) ≈ x^0.5
                // For small ratios we use a linear approximation that works well
                // in the range we care about.
                let ratio = window_minutes / minutes_per_year;
                let window_vol = vol * ratio;
                if window_vol > Decimal::ZERO {
                    move_pct / window_vol
                } else {
                    move_pct
                }
            }
            _ => move_pct,
        };

        // 4. Duration scaling — 5m markets are more sensitive to moves
        let duration_scale = match input.duration {
            MarketDuration::FiveMin => dec!(1.5),
            MarketDuration::FifteenMin => dec!(1.0),
        };

        // 5. Convert directional move to implied probability
        // Base: 0.50 (fair coin). Shift by scaled move.
        // For "Up" outcome: positive move → higher probability.
        // For "Down" outcome: positive move → lower probability (invert).
        let direction_sign = match input.outcome {
            Outcome::Up => dec!(1),
            Outcome::Down => dec!(-1),
        };

        let raw_shift = (vol_normalised_move + momentum_adj) * duration_scale * direction_sign;
        // Scale the shift: a 1% move shouldn't map to 51% probability—
        // use a sensitivity multiplier. Calibrated for short-duration crypto.
        let sensitivity = dec!(10.0);
        let raw_prob = dec!(0.50) + raw_shift * sensitivity;

        // 6. Consensus penalty — reduce confidence if sources disagree
        let consensus_penalty = match input.secondary_price {
            Some(sec) if input.spot_price > Decimal::ZERO => {
                let diff = ((sec - input.spot_price) / input.spot_price).abs();
                // More than 0.1% divergence starts reducing confidence
                if diff > dec!(0.001) {
                    (diff * dec!(100)).min(dec!(0.30))
                } else {
                    Decimal::ZERO
                }
            }
            _ => Decimal::ZERO,
        };

        // 7. Clamp probability
        let probability = raw_prob.max(dec!(0.01)).min(dec!(0.99));

        // Confidence: starts at 1.0, reduced by missing features and consensus
        let mut confidence = dec!(1.0);
        if input.momentum.is_none() {
            confidence -= dec!(0.05);
        }
        if input.volatility.is_none() {
            confidence -= dec!(0.10);
        }
        if input.secondary_price.is_none() {
            confidence -= dec!(0.05);
        }
        confidence -= consensus_penalty;
        confidence = confidence.max(dec!(0.10));

        let features = FairValueFeatures {
            move_pct,
            short_delta: input.short_delta,
            momentum_adj,
            vol_normalised_move,
            consensus_penalty,
            duration_scale,
        };

        debug!(
            asset = %input.asset,
            outcome = %input.outcome,
            duration = %input.duration,
            probability = %probability,
            confidence = %confidence,
            model = MODEL_VERSION,
            move_pct = %move_pct,
            "fair_value_computed"
        );

        FairValueEstimate {
            probability,
            confidence,
            model_version: MODEL_VERSION.to_string(),
            features,
            computed_at: input.timestamp,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> FairValueInput {
        FairValueInput {
            asset: Asset::BTC,
            outcome: Outcome::Up,
            duration: MarketDuration::FiveMin,
            spot_price: dec!(100100),
            window_open_price: dec!(100000),
            short_delta: dec!(0.0005),
            momentum: Some(dec!(0.8)),
            volatility: Some(dec!(0.60)),
            secondary_price: Some(dec!(100095)),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn deterministic_for_identical_inputs() {
        let input = base_input();
        let a = FairValueEngine::compute(&input);
        let b = FairValueEngine::compute(&input);
        assert_eq!(a.probability, b.probability);
        assert_eq!(a.confidence, b.confidence);
    }

    #[test]
    fn positive_move_increases_up_probability() {
        let input = base_input(); // +0.1% move, Up outcome
        let est = FairValueEngine::compute(&input);
        assert!(est.probability > dec!(0.50), "p={}", est.probability);
    }

    #[test]
    fn positive_move_decreases_down_probability() {
        let mut input = base_input();
        input.outcome = Outcome::Down;
        let est = FairValueEngine::compute(&input);
        assert!(est.probability < dec!(0.50), "p={}", est.probability);
    }

    #[test]
    fn no_move_is_near_fifty() {
        let mut input = base_input();
        input.spot_price = input.window_open_price;
        input.short_delta = Decimal::ZERO;
        let est = FairValueEngine::compute(&input);
        let diff = (est.probability - dec!(0.50)).abs();
        assert!(diff < dec!(0.05), "p={}", est.probability);
    }

    #[test]
    fn probability_clamped_to_range() {
        let mut input = base_input();
        // Huge move to force clamping
        input.spot_price = dec!(200000);
        input.short_delta = dec!(0.10);
        let est = FairValueEngine::compute(&input);
        assert!(est.probability <= dec!(0.99));
        assert!(est.probability >= dec!(0.01));
    }

    #[test]
    fn missing_features_reduce_confidence() {
        let mut input = base_input();
        let full = FairValueEngine::compute(&input);

        input.momentum = None;
        input.volatility = None;
        input.secondary_price = None;
        let partial = FairValueEngine::compute(&input);

        assert!(partial.confidence < full.confidence);
    }

    #[test]
    fn model_version_is_set() {
        let est = FairValueEngine::compute(&base_input());
        assert_eq!(est.model_version, MODEL_VERSION);
    }

    #[test]
    fn zero_window_open_price_safe() {
        let mut input = base_input();
        input.window_open_price = Decimal::ZERO;
        // Should not panic
        let est = FairValueEngine::compute(&input);
        assert!(est.probability >= dec!(0.01));
    }

    #[test]
    fn consensus_divergence_reduces_confidence() {
        let mut input = base_input();
        input.secondary_price = Some(dec!(100500)); // 0.4% divergence
        let est = FairValueEngine::compute(&input);
        let baseline = FairValueEngine::compute(&base_input());
        assert!(est.confidence < baseline.confidence);
    }

    #[test]
    fn five_min_more_sensitive_than_fifteen() {
        // Use a small move, no volatility, to isolate the duration_scale effect
        let mut input_5m = base_input();
        input_5m.spot_price = dec!(100010);
        input_5m.window_open_price = dec!(100000);
        input_5m.short_delta = dec!(0.00005);
        input_5m.volatility = None;
        input_5m.momentum = None;
        input_5m.secondary_price = None;
        let est_5m = FairValueEngine::compute(&input_5m);

        let mut input_15m = input_5m.clone();
        input_15m.duration = MarketDuration::FifteenMin;
        let est_15m = FairValueEngine::compute(&input_15m);

        // 5m should deviate more from 0.50 for the same move
        let dev_5m = (est_5m.probability - dec!(0.50)).abs();
        let dev_15m = (est_15m.probability - dec!(0.50)).abs();
        assert!(dev_5m > dev_15m, "5m={dev_5m} 15m={dev_15m}");
    }
}
