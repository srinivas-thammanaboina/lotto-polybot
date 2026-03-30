use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::ContractKey;
use super::signal::Side;

/// A completed trade record for accounting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    pub contract: ContractKey,
    pub side: Side,
    pub entry_price: Decimal,
    pub exit_price: Option<Decimal>,
    pub size: Decimal,
    pub fees_paid: Decimal,
    pub realized_pnl: Option<Decimal>,
    pub entry_slippage: Decimal,
    pub exit_slippage: Option<Decimal>,
    pub opened_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// Resolution outcome from Polymarket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionOutcome {
    Yes,
    No,
    Unknown,
}

/// Verified final outcome for a contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedOutcome {
    pub contract: ContractKey,
    pub outcome: ResolutionOutcome,
    pub payout_price: Decimal,
    pub realized_pnl: Decimal,
    pub verified_at: DateTime<Utc>,
}
