use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::{Asset, ContractKey, MarketDuration};
use crate::strategy::edge::CostSnapshot;

/// Trade side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

/// Why a signal was rejected by the gate checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RejectReason {
    StaleFeed,
    StaleBook,
    InsufficientLiquidity,
    BelowEdgeThreshold,
    BelowConfidence,
    ContractLocked,
    KillSwitchActive,
    ExecutionUnhealthy,
    UnsupportedMarket,
    MaxExposureReached,
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RejectReason::StaleFeed => "stale_feed",
            RejectReason::StaleBook => "stale_book",
            RejectReason::InsufficientLiquidity => "insufficient_liquidity",
            RejectReason::BelowEdgeThreshold => "below_edge",
            RejectReason::BelowConfidence => "below_confidence",
            RejectReason::ContractLocked => "contract_locked",
            RejectReason::KillSwitchActive => "kill_switch",
            RejectReason::ExecutionUnhealthy => "execution_unhealthy",
            RejectReason::UnsupportedMarket => "unsupported_market",
            RejectReason::MaxExposureReached => "max_exposure",
        };
        write!(f, "{s}")
    }
}

/// The result of running signal gates on a trade candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignalDecision {
    Accept(Box<OrderIntent>),
    Reject {
        contract: ContractKey,
        reasons: Vec<RejectReason>,
        timestamp: DateTime<Utc>,
    },
}

/// A qualified order intent ready for the execution engine.
///
/// This is the contract between strategy and execution. It carries enough
/// context for replay and debugging without requiring strategy logic in
/// the execution layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderIntent {
    pub contract: ContractKey,
    pub asset: Asset,
    pub duration: MarketDuration,
    pub side: Side,
    pub target_price: Decimal,
    pub size: Decimal,
    pub fair_value: Decimal,
    pub gross_edge: Decimal,
    pub net_edge: Decimal,
    pub cost_snapshot: CostSnapshot,
    pub rationale: String,
    pub model_version: String,
    pub signal_timestamp: DateTime<Utc>,
}
