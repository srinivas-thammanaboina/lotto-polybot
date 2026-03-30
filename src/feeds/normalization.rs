use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::domain::market::{Asset, TokenId};
use crate::types::{FeedSource, ReceiptTimestamp};

/// Normalized trade tick from any CEX source.
#[derive(Debug, Clone)]
pub struct NormalizedTick {
    pub source: FeedSource,
    pub asset: Asset,
    pub price: Decimal,
    pub quantity: Decimal,
    pub source_timestamp: DateTime<Utc>,
    pub receipt_timestamp: ReceiptTimestamp,
}

/// Normalized book level from Polymarket.
#[derive(Debug, Clone)]
pub struct NormalizedBookLevel {
    pub price: Decimal,
    pub size: Decimal,
}

/// Normalized book update from Polymarket.
#[derive(Debug, Clone)]
pub struct NormalizedBookUpdate {
    pub token_id: TokenId,
    pub bids: Vec<NormalizedBookLevel>,
    pub asks: Vec<NormalizedBookLevel>,
    pub timestamp: DateTime<Utc>,
    pub receipt_timestamp: ReceiptTimestamp,
}

/// Parse an asset string from exchange symbols.
pub fn parse_asset(symbol: &str) -> Option<Asset> {
    let s = symbol.to_uppercase();
    if s.contains("BTC") {
        Some(Asset::BTC)
    } else if s.contains("ETH") {
        Some(Asset::ETH)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_assets() {
        assert_eq!(parse_asset("BTCUSDT"), Some(Asset::BTC));
        assert_eq!(parse_asset("ETHUSDT"), Some(Asset::ETH));
        assert_eq!(parse_asset("btcusd"), Some(Asset::BTC));
        assert_eq!(parse_asset("DOGEUSDT"), None);
    }
}
