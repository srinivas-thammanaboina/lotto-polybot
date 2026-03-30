//! Signal pipeline: ties fair value → edge → gates → sizing → OrderIntent.
//!
//! Strategy code never talks directly to the CLOB client. It emits
//! `OrderIntent` objects as the contract between strategy and execution.

use std::time::Duration;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tracing::{info, warn};

use crate::config::{RiskConfig, StrategyConfig};
use crate::domain::contract::LockState;
use crate::domain::market::{Asset, BookSnapshot, ContractKey, MarketDuration, Outcome};
use crate::domain::signal::{OrderIntent, RejectReason, Side, SignalDecision};
use crate::strategy::edge::{EdgeCalculator, EdgeInput, FeeSchedule};
use crate::strategy::fair_value::{FairValueEngine, FairValueInput, MODEL_VERSION};
use crate::strategy::filters::{GateContext, SignalGates};
use crate::strategy::sizing::{SizingEngine, SizingInput, SizingMode};

// ---------------------------------------------------------------------------
// Pipeline input — everything the pipeline needs from external state
// ---------------------------------------------------------------------------

/// Snapshot of the world at signal evaluation time.
#[derive(Debug, Clone)]
pub struct PipelineInput {
    // Market identity
    pub contract: ContractKey,
    pub asset: Asset,
    pub outcome: Outcome,
    pub duration: MarketDuration,

    // CEX data
    pub spot_price: Decimal,
    pub window_open_price: Decimal,
    pub short_delta: Decimal,
    pub momentum: Option<Decimal>,
    pub volatility: Option<Decimal>,
    pub secondary_price: Option<Decimal>,

    // Polymarket book
    pub book: Option<BookSnapshot>,
    /// Current best price on the side we'd trade (the market probability).
    pub market_price: Decimal,

    // Health / state
    pub cex_feed_healthy: bool,
    pub last_cex_tick: Option<DateTime<Utc>>,
    pub lock_state: LockState,
    pub kill_switch_active: bool,
    pub execution_healthy: bool,
    pub current_positions: u32,
    pub current_position_notional: Decimal,

    // Account
    pub equity: Decimal,

    // Timing
    pub signal_age: Duration,
    pub now: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// The signal pipeline. Stateless — takes a world snapshot and config,
/// returns a `SignalDecision`.
pub struct SignalPipeline;

impl SignalPipeline {
    /// Evaluate a trade candidate through the full pipeline.
    pub fn evaluate(
        input: &PipelineInput,
        strategy_cfg: &StrategyConfig,
        risk_cfg: &RiskConfig,
        fee_schedule: &FeeSchedule,
        sizing_mode: &SizingMode,
    ) -> SignalDecision {
        let thresholds = match input.duration {
            MarketDuration::FiveMin => &strategy_cfg.five_min,
            MarketDuration::FifteenMin => &strategy_cfg.fifteen_min,
        };

        // Step 1: Fair value
        let fv_input = FairValueInput {
            asset: input.asset,
            outcome: input.outcome,
            duration: input.duration,
            spot_price: input.spot_price,
            window_open_price: input.window_open_price,
            short_delta: input.short_delta,
            momentum: input.momentum,
            volatility: input.volatility,
            secondary_price: input.secondary_price,
            timestamp: input.now,
        };
        let fv = FairValueEngine::compute(&fv_input);

        // Step 2: Edge / cost model
        let book_depth = input
            .book
            .as_ref()
            .map(|b| b.bids.depth_usdc() + b.asks.depth_usdc())
            .unwrap_or(Decimal::ZERO);

        let edge_input = EdgeInput {
            fair_value_prob: fv.probability,
            market_price: input.market_price,
            notional: risk_cfg.max_notional_per_order, // Use max for cost estimation
            book_depth_usdc: book_depth,
            duration: input.duration,
            fees: fee_schedule.clone(),
            latency_decay_buffer: strategy_cfg.latency_decay_buffer,
            signal_age: input.signal_age,
            timestamp: input.now,
        };
        let edge = EdgeCalculator::compute(&edge_input);

        // Step 3: Signal gates
        let gate_ctx = GateContext {
            is_supported_market: matches!(input.asset, Asset::BTC | Asset::ETH),
            cex_feed_healthy: input.cex_feed_healthy,
            last_cex_tick: input.last_cex_tick,
            book: input.book.clone(),
            fair_value: fv.clone(),
            edge: edge.clone(),
            lock_state: input.lock_state,
            kill_switch_active: input.kill_switch_active,
            execution_healthy: input.execution_healthy,
            duration: input.duration,
            current_positions: input.current_positions,
            max_concurrent_positions: risk_cfg.max_concurrent_positions,
            now: input.now,
        };
        let gate_result = SignalGates::evaluate(&gate_ctx, thresholds);

        if !gate_result.passed {
            warn!(
                contract = %input.contract,
                reasons = ?gate_result.rejections,
                "signal_pipeline_rejected"
            );
            return SignalDecision::Reject {
                contract: input.contract.clone(),
                reasons: gate_result.rejections,
                timestamp: input.now,
            };
        }

        // Step 4: Sizing
        let sizing_input = SizingInput {
            mode: sizing_mode.clone(),
            equity: input.equity,
            net_edge: edge.net_edge,
            fair_value_prob: fv.probability,
            market_price: input.market_price,
            max_notional_per_order: risk_cfg.max_notional_per_order,
            max_position_per_market: risk_cfg.max_position_per_market,
            current_position_notional: input.current_position_notional,
            book_depth_usdc: book_depth,
        };
        let sizing = SizingEngine::compute(&sizing_input);

        if sizing.notional.is_zero() {
            warn!(
                contract = %input.contract,
                "signal_pipeline_rejected_zero_size"
            );
            return SignalDecision::Reject {
                contract: input.contract.clone(),
                reasons: vec![RejectReason::InsufficientLiquidity],
                timestamp: input.now,
            };
        }

        // Step 5: Determine side
        let side = if fv.probability > input.market_price {
            Side::Buy
        } else {
            Side::Sell
        };

        let rationale = format!(
            "fv={:.4} mkt={:.4} edge={:.4} net={:.4} conf={:.2}",
            fv.probability, input.market_price, edge.gross_edge, edge.net_edge, fv.confidence,
        );

        let intent = OrderIntent {
            contract: input.contract.clone(),
            asset: input.asset,
            duration: input.duration,
            side,
            target_price: input.market_price,
            size: sizing.notional,
            fair_value: fv.probability,
            gross_edge: edge.gross_edge,
            net_edge: edge.net_edge,
            cost_snapshot: edge.costs,
            rationale,
            model_version: MODEL_VERSION.to_string(),
            signal_timestamp: input.now,
        };

        info!(
            contract = %intent.contract,
            side = %intent.side,
            size = %intent.size,
            net_edge = %intent.net_edge,
            model = %intent.model_version,
            "signal_pipeline_accepted"
        );

        SignalDecision::Accept(Box::new(intent))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MarketRegimeThresholds;
    use crate::domain::market::{BookSide, MarketId, PriceLevel, TokenId};
    use rust_decimal_macros::dec;

    fn test_strategy_cfg() -> StrategyConfig {
        StrategyConfig {
            five_min: MarketRegimeThresholds {
                min_net_edge: dec!(0.02),
                min_confidence: dec!(0.60),
                min_book_depth_usdc: dec!(50),
                max_hold: Duration::from_secs(240),
                stale_feed_tolerance: Duration::from_secs(2),
                stale_book_tolerance: Duration::from_secs(3),
                cooldown: Duration::from_secs(10),
            },
            fifteen_min: MarketRegimeThresholds {
                min_net_edge: dec!(0.015),
                min_confidence: dec!(0.55),
                min_book_depth_usdc: dec!(50),
                max_hold: Duration::from_secs(780),
                stale_feed_tolerance: Duration::from_secs(3),
                stale_book_tolerance: Duration::from_secs(5),
                cooldown: Duration::from_secs(15),
            },
            latency_decay_buffer: Duration::from_millis(200),
        }
    }

    fn test_risk_cfg() -> RiskConfig {
        RiskConfig {
            max_position_per_market: dec!(50),
            max_concurrent_positions: 4,
            max_gross_exposure: dec!(200),
            max_daily_drawdown: dec!(50),
            max_total_drawdown: dec!(100),
            max_consecutive_losses: 5,
            max_notional_per_order: dec!(25),
        }
    }

    fn test_book() -> BookSnapshot {
        BookSnapshot {
            token_id: TokenId("tok1".into()),
            bids: BookSide {
                levels: vec![PriceLevel {
                    price: dec!(0.50),
                    size: dec!(200),
                }],
            },
            asks: BookSide {
                levels: vec![PriceLevel {
                    price: dec!(0.51),
                    size: dec!(200),
                }],
            },
            timestamp: Utc::now(),
        }
    }

    fn accepting_input() -> PipelineInput {
        PipelineInput {
            contract: ContractKey {
                market_id: MarketId("mkt1".into()),
                token_id: TokenId("tok1".into()),
            },
            asset: Asset::BTC,
            outcome: Outcome::Up,
            duration: MarketDuration::FiveMin,
            spot_price: dec!(100200),
            window_open_price: dec!(100000),
            short_delta: dec!(0.001),
            momentum: Some(dec!(0.8)),
            volatility: Some(dec!(0.60)),
            secondary_price: Some(dec!(100195)),
            book: Some(test_book()),
            market_price: dec!(0.50),
            cex_feed_healthy: true,
            last_cex_tick: Some(Utc::now()),
            lock_state: LockState::Unlocked,
            kill_switch_active: false,
            execution_healthy: true,
            current_positions: 0,
            current_position_notional: Decimal::ZERO,
            equity: dec!(500),
            signal_age: Duration::from_millis(50),
            now: Utc::now(),
        }
    }

    #[test]
    fn pipeline_accepts_good_signal() {
        let input = accepting_input();
        let decision = SignalPipeline::evaluate(
            &input,
            &test_strategy_cfg(),
            &test_risk_cfg(),
            &FeeSchedule::default(),
            &SizingMode::FixedNotional { amount: dec!(10) },
        );
        match decision {
            SignalDecision::Accept(intent) => {
                assert!(intent.size > Decimal::ZERO);
                assert!(intent.net_edge > Decimal::ZERO);
                assert_eq!(intent.model_version, MODEL_VERSION);
                assert!(!intent.rationale.is_empty());
                assert!(intent.cost_snapshot.total_cost_frac > Decimal::ZERO);
            }
            SignalDecision::Reject { reasons, .. } => {
                panic!("expected accept, got reject: {reasons:?}");
            }
        }
    }

    #[test]
    fn pipeline_rejects_when_kill_switch_active() {
        let mut input = accepting_input();
        input.kill_switch_active = true;
        let decision = SignalPipeline::evaluate(
            &input,
            &test_strategy_cfg(),
            &test_risk_cfg(),
            &FeeSchedule::default(),
            &SizingMode::FixedNotional { amount: dec!(10) },
        );
        assert!(matches!(decision, SignalDecision::Reject { .. }));
    }

    #[test]
    fn pipeline_rejects_stale_feed() {
        let mut input = accepting_input();
        input.cex_feed_healthy = false;
        let decision = SignalPipeline::evaluate(
            &input,
            &test_strategy_cfg(),
            &test_risk_cfg(),
            &FeeSchedule::default(),
            &SizingMode::FixedNotional { amount: dec!(10) },
        );
        assert!(matches!(decision, SignalDecision::Reject { .. }));
    }

    #[test]
    fn pipeline_rejects_locked_contract() {
        let mut input = accepting_input();
        input.lock_state = LockState::Locked;
        let decision = SignalPipeline::evaluate(
            &input,
            &test_strategy_cfg(),
            &test_risk_cfg(),
            &FeeSchedule::default(),
            &SizingMode::FixedNotional { amount: dec!(10) },
        );
        assert!(matches!(decision, SignalDecision::Reject { .. }));
    }

    #[test]
    fn pipeline_buys_when_fv_above_market() {
        let input = accepting_input(); // FV should be > 0.50 (market_price)
        let decision = SignalPipeline::evaluate(
            &input,
            &test_strategy_cfg(),
            &test_risk_cfg(),
            &FeeSchedule::default(),
            &SizingMode::FixedNotional { amount: dec!(10) },
        );
        if let SignalDecision::Accept(intent) = decision {
            assert_eq!(intent.side, Side::Buy);
        }
    }

    #[test]
    fn intent_carries_cost_snapshot() {
        let input = accepting_input();
        let decision = SignalPipeline::evaluate(
            &input,
            &test_strategy_cfg(),
            &test_risk_cfg(),
            &FeeSchedule::default(),
            &SizingMode::FixedNotional { amount: dec!(10) },
        );
        if let SignalDecision::Accept(intent) = decision {
            assert!(intent.cost_snapshot.fee_rate >= Decimal::ZERO);
            assert!(intent.cost_snapshot.entry_slippage >= Decimal::ZERO);
        }
    }

    #[test]
    fn pipeline_works_with_fifteen_min() {
        let mut input = accepting_input();
        input.duration = MarketDuration::FifteenMin;
        let decision = SignalPipeline::evaluate(
            &input,
            &test_strategy_cfg(),
            &test_risk_cfg(),
            &FeeSchedule::default(),
            &SizingMode::FixedNotional { amount: dec!(10) },
        );
        // Should still accept with a big enough move
        assert!(matches!(decision, SignalDecision::Accept(_)));
    }

    #[test]
    fn pipeline_rejects_no_book() {
        let mut input = accepting_input();
        input.book = None;
        let decision = SignalPipeline::evaluate(
            &input,
            &test_strategy_cfg(),
            &test_risk_cfg(),
            &FeeSchedule::default(),
            &SizingMode::FixedNotional { amount: dec!(10) },
        );
        assert!(matches!(decision, SignalDecision::Reject { .. }));
    }
}
