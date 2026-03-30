use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::ContractKey;
use super::signal::Side;

/// An open position in a contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub contract: ContractKey,
    pub side: Side,
    pub size: Decimal,
    pub avg_entry_price: Decimal,
    pub unrealized_pnl: Decimal,
    pub opened_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Position {
    pub fn notional(&self) -> Decimal {
        self.size * self.avg_entry_price
    }
}
