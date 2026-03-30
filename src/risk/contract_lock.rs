//! Per-contract lock service.
//!
//! Prevents duplicate entries into the same contract window.
//! Lock state is exposed read-only to signal gates.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use tracing::{debug, info};

use crate::domain::contract::LockState;
use crate::domain::market::ContractKey;

// ---------------------------------------------------------------------------
// Lock record
// ---------------------------------------------------------------------------

/// Full lock record for a contract.
#[derive(Debug, Clone)]
pub struct LockRecord {
    pub contract: ContractKey,
    pub state: LockState,
    /// When the lock was last changed.
    pub changed_at: DateTime<Utc>,
    /// When cooldown expires (if in Cooldown state).
    pub cooldown_until: Option<DateTime<Utc>>,
    /// Market expiry time (for post-expiry buffer).
    pub market_expiry: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Contract lock service
// ---------------------------------------------------------------------------

/// Thread-safe contract lock service.
#[derive(Debug, Clone)]
pub struct ContractLockService {
    locks: Arc<RwLock<HashMap<String, LockRecord>>>,
    /// Post-expiry buffer — keep locked after expiry to avoid re-entry.
    post_expiry_buffer: Duration,
    /// Cooldown duration after a position is closed.
    cooldown_duration: Duration,
}

impl ContractLockService {
    pub fn new(post_expiry_buffer: Duration, cooldown_duration: Duration) -> Self {
        Self {
            locks: Arc::new(RwLock::new(HashMap::new())),
            post_expiry_buffer,
            cooldown_duration,
        }
    }

    /// Get the lock state for a contract. Returns Unlocked if no record exists.
    pub fn lock_state(&self, contract: &ContractKey) -> LockState {
        let key = contract.to_string();
        let locks = self.locks.read();
        match locks.get(&key) {
            None => LockState::Unlocked,
            Some(record) => {
                // Check if cooldown has expired
                if record.state == LockState::Cooldown
                    && let Some(until) = record.cooldown_until
                    && Utc::now() >= until
                {
                    return LockState::Unlocked;
                }
                // Check if post-expiry buffer has passed
                if let Some(expiry) = record.market_expiry {
                    let buffer = chrono::Duration::from_std(self.post_expiry_buffer)
                        .unwrap_or(chrono::Duration::seconds(30));
                    if Utc::now() >= expiry + buffer {
                        return LockState::Unlocked;
                    }
                }
                record.state
            }
        }
    }

    /// Check if a contract is tradeable (Unlocked).
    pub fn is_tradeable(&self, contract: &ContractKey) -> bool {
        self.lock_state(contract) == LockState::Unlocked
    }

    /// Lock a contract (order accepted for execution).
    pub fn lock(&self, contract: &ContractKey, market_expiry: Option<DateTime<Utc>>) {
        let key = contract.to_string();
        let now = Utc::now();
        self.locks.write().insert(
            key,
            LockRecord {
                contract: contract.clone(),
                state: LockState::Locked,
                changed_at: now,
                cooldown_until: None,
                market_expiry,
            },
        );
        info!(contract = %contract, "contract_locked");
    }

    /// Transition a contract to cooldown (position closed).
    pub fn cooldown(&self, contract: &ContractKey) {
        let key = contract.to_string();
        let now = Utc::now();
        let cooldown_until = now
            + chrono::Duration::from_std(self.cooldown_duration)
                .unwrap_or(chrono::Duration::seconds(10));

        let mut locks = self.locks.write();
        if let Some(record) = locks.get_mut(&key) {
            record.state = LockState::Cooldown;
            record.changed_at = now;
            record.cooldown_until = Some(cooldown_until);
            debug!(
                contract = %contract,
                cooldown_until = %cooldown_until,
                "contract_cooldown"
            );
        }
    }

    /// Unlock a contract explicitly (e.g., after reconciliation).
    pub fn unlock(&self, contract: &ContractKey) {
        let key = contract.to_string();
        self.locks.write().remove(&key);
        debug!(contract = %contract, "contract_unlocked");
    }

    /// Get all currently locked contracts.
    pub fn locked_contracts(&self) -> Vec<ContractKey> {
        self.locks
            .read()
            .values()
            .filter(|r| r.state == LockState::Locked)
            .map(|r| r.contract.clone())
            .collect()
    }

    /// Clean up expired locks (cooldowns that have passed, post-expiry buffers).
    pub fn cleanup_expired(&self) {
        let now = Utc::now();
        let buffer = chrono::Duration::from_std(self.post_expiry_buffer)
            .unwrap_or(chrono::Duration::seconds(30));

        let mut locks = self.locks.write();
        locks.retain(|_, record| {
            // Remove if cooldown has expired
            if record.state == LockState::Cooldown
                && let Some(until) = record.cooldown_until
                && now >= until
            {
                return false;
            }
            // Remove if post-expiry buffer has passed
            if let Some(expiry) = record.market_expiry
                && now >= expiry + buffer
            {
                return false;
            }
            true
        });
    }

    /// Count of contracts in each state.
    pub fn state_counts(&self) -> (usize, usize, usize) {
        let locks = self.locks.read();
        let locked = locks
            .values()
            .filter(|r| r.state == LockState::Locked)
            .count();
        let cooldown = locks
            .values()
            .filter(|r| r.state == LockState::Cooldown)
            .count();
        let unlocked = locks
            .values()
            .filter(|r| r.state == LockState::Unlocked)
            .count();
        (locked, cooldown, unlocked)
    }

    /// Clear all locks (for reconciliation reset).
    pub fn clear(&self) {
        self.locks.write().clear();
    }

    /// Rebuild locks from reconciliation data.
    pub fn load_from_reconciliation(&self, contracts: Vec<(ContractKey, Option<DateTime<Utc>>)>) {
        let now = Utc::now();
        let mut locks = self.locks.write();
        for (contract, expiry) in contracts {
            let key = contract.to_string();
            locks.insert(
                key,
                LockRecord {
                    contract,
                    state: LockState::Locked,
                    changed_at: now,
                    cooldown_until: None,
                    market_expiry: expiry,
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{MarketId, TokenId};

    fn test_contract() -> ContractKey {
        ContractKey {
            market_id: MarketId("mkt1".into()),
            token_id: TokenId("tok1".into()),
        }
    }

    fn service() -> ContractLockService {
        ContractLockService::new(Duration::from_secs(30), Duration::from_secs(10))
    }

    #[test]
    fn unknown_contract_is_unlocked() {
        let svc = service();
        assert_eq!(svc.lock_state(&test_contract()), LockState::Unlocked);
        assert!(svc.is_tradeable(&test_contract()));
    }

    #[test]
    fn lock_blocks_trading() {
        let svc = service();
        let c = test_contract();
        svc.lock(&c, None);
        assert_eq!(svc.lock_state(&c), LockState::Locked);
        assert!(!svc.is_tradeable(&c));
    }

    #[test]
    fn cooldown_blocks_trading() {
        let svc = service();
        let c = test_contract();
        svc.lock(&c, None);
        svc.cooldown(&c);
        assert_eq!(svc.lock_state(&c), LockState::Cooldown);
        assert!(!svc.is_tradeable(&c));
    }

    #[test]
    fn explicit_unlock() {
        let svc = service();
        let c = test_contract();
        svc.lock(&c, None);
        svc.unlock(&c);
        assert!(svc.is_tradeable(&c));
    }

    #[test]
    fn locked_contracts_list() {
        let svc = service();
        let c1 = test_contract();
        let c2 = ContractKey {
            market_id: MarketId("mkt2".into()),
            token_id: TokenId("tok2".into()),
        };
        svc.lock(&c1, None);
        svc.lock(&c2, None);
        assert_eq!(svc.locked_contracts().len(), 2);
    }

    #[test]
    fn expired_cooldown_becomes_unlocked() {
        // Use a zero-duration cooldown so it expires immediately
        let svc = ContractLockService::new(Duration::from_secs(30), Duration::from_secs(0));
        let c = test_contract();
        svc.lock(&c, None);
        svc.cooldown(&c);
        // With 0s cooldown, should be unlocked immediately
        assert_eq!(svc.lock_state(&c), LockState::Unlocked);
    }

    #[test]
    fn post_expiry_buffer() {
        // Contract with expiry in the past and zero buffer
        let svc = ContractLockService::new(Duration::from_secs(0), Duration::from_secs(10));
        let c = test_contract();
        let past_expiry = Utc::now() - chrono::Duration::seconds(60);
        svc.lock(&c, Some(past_expiry));
        // Expiry + 0s buffer has passed, so it should be unlocked
        assert_eq!(svc.lock_state(&c), LockState::Unlocked);
    }

    #[test]
    fn cleanup_removes_expired() {
        let svc = ContractLockService::new(Duration::from_secs(0), Duration::from_secs(0));
        let c = test_contract();
        svc.lock(&c, Some(Utc::now() - chrono::Duration::seconds(60)));
        svc.cleanup_expired();
        assert_eq!(svc.state_counts(), (0, 0, 0));
    }

    #[test]
    fn state_counts() {
        let svc = service();
        let c1 = test_contract();
        let c2 = ContractKey {
            market_id: MarketId("mkt2".into()),
            token_id: TokenId("tok2".into()),
        };
        svc.lock(&c1, None);
        svc.lock(&c2, None);
        svc.cooldown(&c2);
        let (locked, cooldown, _) = svc.state_counts();
        assert_eq!(locked, 1);
        assert_eq!(cooldown, 1);
    }

    #[test]
    fn clear_removes_all() {
        let svc = service();
        svc.lock(&test_contract(), None);
        svc.clear();
        assert!(svc.is_tradeable(&test_contract()));
    }

    #[test]
    fn load_from_reconciliation() {
        let svc = service();
        let c = test_contract();
        svc.load_from_reconciliation(vec![(c.clone(), None)]);
        assert_eq!(svc.lock_state(&c), LockState::Locked);
    }
}
