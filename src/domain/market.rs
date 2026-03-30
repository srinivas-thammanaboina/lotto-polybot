use std::fmt;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Supported crypto assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Asset {
    BTC,
    ETH,
}

impl fmt::Display for Asset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Asset::BTC => write!(f, "BTC"),
            Asset::ETH => write!(f, "ETH"),
        }
    }
}

/// Market duration class — 5m and 15m are separate strategy regimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarketDuration {
    FiveMin,
    FifteenMin,
}

impl fmt::Display for MarketDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MarketDuration::FiveMin => write!(f, "5m"),
            MarketDuration::FifteenMin => write!(f, "15m"),
        }
    }
}

/// Direction of the market outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Outcome {
    Up,
    Down,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Up => write!(f, "UP"),
            Outcome::Down => write!(f, "DOWN"),
        }
    }
}

/// Unique identity for a Polymarket market.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MarketId(pub String);

impl fmt::Display for MarketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identity for a tradeable token within a market.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenId(pub String);

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Composite key identifying a specific contract window.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContractKey {
    pub market_id: MarketId,
    pub token_id: TokenId,
}

impl fmt::Display for ContractKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.market_id, self.token_id)
    }
}

/// Full metadata for a discovered market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketMeta {
    pub market_id: MarketId,
    pub asset: Asset,
    pub duration: MarketDuration,
    pub expiry: DateTime<Utc>,
    pub outcomes: Vec<OutcomeMeta>,
    pub active: bool,
    pub discovered_at: DateTime<Utc>,
}

/// Metadata for one outcome token in a market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeMeta {
    pub token_id: TokenId,
    pub outcome: Outcome,
    pub label: String,
}

/// A single price/probability level.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: Decimal,
    pub size: Decimal,
}

/// Top-of-book snapshot for one side of a market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookSide {
    pub levels: Vec<PriceLevel>,
}

impl BookSide {
    pub fn best(&self) -> Option<PriceLevel> {
        self.levels.first().copied()
    }

    pub fn depth_usdc(&self) -> Decimal {
        self.levels
            .iter()
            .map(|l| l.price * l.size)
            .sum()
    }
}

/// Order book snapshot for a token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookSnapshot {
    pub token_id: TokenId,
    pub bids: BookSide,
    pub asks: BookSide,
    pub timestamp: DateTime<Utc>,
}
