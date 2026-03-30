use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::market::{ContractKey, MarketDuration, MarketId, TokenId, Asset};

/// Contract lock state — prevents duplicate entries into the same window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LockState {
    /// No position, available for entry.
    Unlocked,
    /// Position open or order pending.
    Locked,
    /// Position closed, in cooldown before re-entry.
    Cooldown,
}

/// Registry entry for a single contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractEntry {
    pub key: ContractKey,
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub asset: Asset,
    pub duration: MarketDuration,
    pub expiry: DateTime<Utc>,
    pub lock_state: LockState,
    pub lock_changed_at: Option<DateTime<Utc>>,
}

impl ContractEntry {
    pub fn is_tradeable(&self) -> bool {
        self.lock_state == LockState::Unlocked && self.expiry > Utc::now()
    }

    pub fn is_expired(&self) -> bool {
        self.expiry <= Utc::now()
    }
}
