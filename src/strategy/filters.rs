//! Signal gates: freshness, liquidity, confidence, dedup, and kill-switch checks.
//!
//! Gate logic is separated from fair-value math. Every signal either passes
//! all gates or is rejected with explicit reason codes for logging/analysis.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::MarketRegimeThresholds;
use crate::domain::contract::LockState;
use crate::domain::market::{BookSnapshot, MarketDuration};
use crate::domain::signal::RejectReason;
use crate::strategy::edge::EdgeEstimate;
use crate::strategy::fair_value::FairValueEstimate;

// ---------------------------------------------------------------------------
// Gate context — everything the gate system needs to make a decision
// ---------------------------------------------------------------------------

/// Input context for the gate system.
#[derive(Debug, Clone)]
pub struct GateContext {
    /// Is this a supported market (known asset + duration)?
    pub is_supported_market: bool,

    /// Is the primary CEX feed healthy (connected + non-stale)?
    pub cex_feed_healthy: bool,
    /// Timestamp of the last CEX tick (for staleness check).
    pub last_cex_tick: Option<DateTime<Utc>>,

    /// Polymarket book snapshot for this token.
    pub book: Option<BookSnapshot>,

    /// Fair value estimate from the FV engine.
    pub fair_value: FairValueEstimate,

    /// Edge estimate from the cost model.
    pub edge: EdgeEstimate,

    /// Contract lock state (Unlocked / Locked / Cooldown).
    pub lock_state: LockState,

    /// Is the global kill switch active?
    pub kill_switch_active: bool,

    /// Is the execution layer healthy (no overload, no errors)?
    pub execution_healthy: bool,

    /// Market duration for threshold lookup.
    pub duration: MarketDuration,

    /// Current position count for exposure check.
    pub current_positions: u32,
    /// Max concurrent positions from risk config.
    pub max_concurrent_positions: u32,

    /// Current timestamp for staleness checks.
    pub now: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Gate result
// ---------------------------------------------------------------------------

/// Result of running all signal gates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResult {
    /// Empty if all gates passed.
    pub rejections: Vec<RejectReason>,
    /// True if all gates passed.
    pub passed: bool,
}

impl GateResult {
    fn accept() -> Self {
        Self {
            rejections: Vec::new(),
            passed: true,
        }
    }

    fn reject(reasons: Vec<RejectReason>) -> Self {
        Self {
            rejections: reasons,
            passed: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Gate engine
// ---------------------------------------------------------------------------

/// Runs all signal gates. Returns accept or reject with reasons.
///
/// All gates are evaluated (no short-circuit) so the full rejection set
/// is available for logging and analysis.
pub struct SignalGates;

impl SignalGates {
    pub fn evaluate(ctx: &GateContext, thresholds: &MarketRegimeThresholds) -> GateResult {
        let mut reasons = Vec::new();

        // Gate 1: Supported market
        if !ctx.is_supported_market {
            reasons.push(RejectReason::UnsupportedMarket);
        }

        // Gate 2: Non-stale CEX feed
        if !ctx.cex_feed_healthy {
            reasons.push(RejectReason::StaleFeed);
        } else if let Some(last_tick) = ctx.last_cex_tick {
            let age = ctx.now - last_tick;
            if age
                > chrono::Duration::from_std(thresholds.stale_feed_tolerance)
                    .unwrap_or(chrono::Duration::seconds(5))
            {
                reasons.push(RejectReason::StaleFeed);
            }
        }

        // Gate 3: Non-stale Polymarket book
        match &ctx.book {
            None => reasons.push(RejectReason::StaleBook),
            Some(book) => {
                let book_age = ctx.now - book.timestamp;
                if book_age
                    > chrono::Duration::from_std(thresholds.stale_book_tolerance)
                        .unwrap_or(chrono::Duration::seconds(5))
                {
                    reasons.push(RejectReason::StaleBook);
                }

                // Gate 6: Sufficient tradeable depth
                let depth = book.bids.depth_usdc() + book.asks.depth_usdc();
                if depth < thresholds.min_book_depth_usdc {
                    reasons.push(RejectReason::InsufficientLiquidity);
                }
            }
        }

        // Gate 4: Minimum confidence
        if ctx.fair_value.confidence < thresholds.min_confidence {
            reasons.push(RejectReason::BelowConfidence);
        }

        // Gate 5: Minimum net edge
        if ctx.edge.net_edge < thresholds.min_net_edge {
            reasons.push(RejectReason::BelowEdgeThreshold);
        }

        // Gate 7: No duplicate contract lock
        if ctx.lock_state != LockState::Unlocked {
            reasons.push(RejectReason::ContractLocked);
        }

        // Gate 8: No kill switch active
        if ctx.kill_switch_active {
            reasons.push(RejectReason::KillSwitchActive);
        }

        // Gate 9: Execution health acceptable
        if !ctx.execution_healthy {
            reasons.push(RejectReason::ExecutionUnhealthy);
        }

        // Gate 10: Max exposure (position count)
        if ctx.current_positions >= ctx.max_concurrent_positions {
            reasons.push(RejectReason::MaxExposureReached);
        }

        if reasons.is_empty() {
            debug!(
                confidence = %ctx.fair_value.confidence,
                net_edge = %ctx.edge.net_edge,
                "all_gates_passed"
            );
            GateResult::accept()
        } else {
            warn!(
                reasons = ?reasons,
                confidence = %ctx.fair_value.confidence,
                net_edge = %ctx.edge.net_edge,
                "signal_rejected"
            );
            GateResult::reject(reasons)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{BookSide, PriceLevel, TokenId};
    use crate::strategy::edge::{CostSnapshot, EdgeEstimate};
    use crate::strategy::fair_value::{FairValueEstimate, FairValueFeatures, MODEL_VERSION};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use std::time::Duration;

    fn test_thresholds() -> MarketRegimeThresholds {
        MarketRegimeThresholds {
            min_net_edge: dec!(0.02),
            min_confidence: dec!(0.60),
            min_book_depth_usdc: dec!(100),
            max_hold: Duration::from_secs(240),
            stale_feed_tolerance: Duration::from_secs(2),
            stale_book_tolerance: Duration::from_secs(3),
            cooldown: Duration::from_secs(10),
        }
    }

    fn test_fv() -> FairValueEstimate {
        FairValueEstimate {
            probability: dec!(0.65),
            confidence: dec!(0.85),
            model_version: MODEL_VERSION.to_string(),
            features: FairValueFeatures {
                move_pct: dec!(0.001),
                short_delta: dec!(0.0005),
                momentum_adj: dec!(0.0004),
                vol_normalised_move: dec!(0.001),
                consensus_penalty: Decimal::ZERO,
                duration_scale: dec!(1.5),
            },
            computed_at: Utc::now(),
        }
    }

    fn test_edge() -> EdgeEstimate {
        EdgeEstimate {
            gross_edge: dec!(0.10),
            net_edge: dec!(0.05),
            costs: CostSnapshot {
                fee_rate: dec!(0.01),
                entry_fee_usdc: dec!(0.10),
                exit_fee_usdc: dec!(0.10),
                entry_slippage: dec!(0.005),
                exit_slippage: dec!(0.0075),
                latency_decay: dec!(0.0025),
                total_cost_frac: dec!(0.05),
            },
            is_profitable: true,
        }
    }

    fn test_book() -> BookSnapshot {
        BookSnapshot {
            token_id: TokenId("tok1".into()),
            bids: BookSide {
                levels: vec![PriceLevel {
                    price: dec!(0.55),
                    size: dec!(200),
                }],
            },
            asks: BookSide {
                levels: vec![PriceLevel {
                    price: dec!(0.56),
                    size: dec!(200),
                }],
            },
            timestamp: Utc::now(),
        }
    }

    fn passing_context() -> GateContext {
        GateContext {
            is_supported_market: true,
            cex_feed_healthy: true,
            last_cex_tick: Some(Utc::now()),
            book: Some(test_book()),
            fair_value: test_fv(),
            edge: test_edge(),
            lock_state: LockState::Unlocked,
            kill_switch_active: false,
            execution_healthy: true,
            duration: MarketDuration::FiveMin,
            current_positions: 0,
            max_concurrent_positions: 4,
            now: Utc::now(),
        }
    }

    #[test]
    fn all_gates_pass() {
        let result = SignalGates::evaluate(&passing_context(), &test_thresholds());
        assert!(result.passed);
        assert!(result.rejections.is_empty());
    }

    #[test]
    fn unsupported_market_rejected() {
        let mut ctx = passing_context();
        ctx.is_supported_market = false;
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::UnsupportedMarket))
        );
    }

    #[test]
    fn stale_feed_rejected() {
        let mut ctx = passing_context();
        ctx.cex_feed_healthy = false;
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::StaleFeed))
        );
    }

    #[test]
    fn stale_book_rejected() {
        let mut ctx = passing_context();
        ctx.book = None;
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::StaleBook))
        );
    }

    #[test]
    fn below_confidence_rejected() {
        let mut ctx = passing_context();
        ctx.fair_value.confidence = dec!(0.30);
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::BelowConfidence))
        );
    }

    #[test]
    fn below_edge_rejected() {
        let mut ctx = passing_context();
        ctx.edge.net_edge = dec!(0.01);
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::BelowEdgeThreshold))
        );
    }

    #[test]
    fn insufficient_liquidity_rejected() {
        let mut ctx = passing_context();
        let mut book = test_book();
        book.bids.levels = vec![PriceLevel {
            price: dec!(0.55),
            size: dec!(10),
        }];
        book.asks.levels = vec![PriceLevel {
            price: dec!(0.56),
            size: dec!(10),
        }];
        ctx.book = Some(book);
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::InsufficientLiquidity))
        );
    }

    #[test]
    fn contract_locked_rejected() {
        let mut ctx = passing_context();
        ctx.lock_state = LockState::Locked;
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::ContractLocked))
        );
    }

    #[test]
    fn kill_switch_rejected() {
        let mut ctx = passing_context();
        ctx.kill_switch_active = true;
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::KillSwitchActive))
        );
    }

    #[test]
    fn execution_unhealthy_rejected() {
        let mut ctx = passing_context();
        ctx.execution_healthy = false;
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::ExecutionUnhealthy))
        );
    }

    #[test]
    fn max_exposure_rejected() {
        let mut ctx = passing_context();
        ctx.current_positions = 4;
        ctx.max_concurrent_positions = 4;
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::MaxExposureReached))
        );
    }

    #[test]
    fn multiple_rejections_collected() {
        let mut ctx = passing_context();
        ctx.is_supported_market = false;
        ctx.cex_feed_healthy = false;
        ctx.kill_switch_active = true;
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(result.rejections.len() >= 3);
    }

    #[test]
    fn cooldown_state_is_rejected() {
        let mut ctx = passing_context();
        ctx.lock_state = LockState::Cooldown;
        let result = SignalGates::evaluate(&ctx, &test_thresholds());
        assert!(!result.passed);
        assert!(
            result
                .rejections
                .iter()
                .any(|r| matches!(r, RejectReason::ContractLocked))
        );
    }
}
