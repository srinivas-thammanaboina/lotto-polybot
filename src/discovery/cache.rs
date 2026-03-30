use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::PolymarketConfig;
use crate::domain::contract::{ContractEntry, LockState};
use crate::domain::market::{Asset, ContractKey, MarketDuration, MarketId, MarketMeta};

use super::gamma;

/// Thread-safe contract registry backed by a refreshing discovery loop.
#[derive(Debug, Clone)]
pub struct ContractRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

#[derive(Debug)]
struct RegistryInner {
    entries: HashMap<ContractKey, ContractEntry>,
    markets: HashMap<MarketId, MarketMeta>,
    last_refresh: Option<chrono::DateTime<Utc>>,
    consecutive_failures: u32,
    healthy: bool,
}

/// Health snapshot for external queries.
#[derive(Debug, Clone)]
pub struct RegistryHealth {
    pub total_contracts: usize,
    pub active_contracts: usize,
    pub healthy: bool,
    pub consecutive_failures: u32,
    pub last_refresh: Option<chrono::DateTime<Utc>>,
}

impl ContractRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RegistryInner {
                entries: HashMap::new(),
                markets: HashMap::new(),
                last_refresh: None,
                consecutive_failures: 0,
                healthy: false,
            })),
        }
    }

    /// Look up a contract entry by key.
    pub fn get(&self, key: &ContractKey) -> Option<ContractEntry> {
        self.inner.read().entries.get(key).cloned()
    }

    /// Get all active, tradeable contracts.
    pub fn active_contracts(&self) -> Vec<ContractEntry> {
        self.inner
            .read()
            .entries
            .values()
            .filter(|e| e.is_tradeable())
            .cloned()
            .collect()
    }

    /// Get all contracts for a given asset and duration.
    pub fn contracts_for(&self, asset: Asset, duration: MarketDuration) -> Vec<ContractEntry> {
        self.inner
            .read()
            .entries
            .values()
            .filter(|e| e.asset == asset && e.duration == duration && !e.is_expired())
            .cloned()
            .collect()
    }

    /// Update lock state for a contract.
    pub fn set_lock(&self, key: &ContractKey, state: LockState) -> bool {
        let mut inner = self.inner.write();
        if let Some(entry) = inner.entries.get_mut(key) {
            entry.lock_state = state;
            entry.lock_changed_at = Some(Utc::now());
            true
        } else {
            false
        }
    }

    /// Check if the registry is healthy enough for trading.
    pub fn is_healthy(&self) -> bool {
        self.inner.read().healthy
    }

    /// Get a health snapshot.
    pub fn health(&self) -> RegistryHealth {
        let inner = self.inner.read();
        RegistryHealth {
            total_contracts: inner.entries.len(),
            active_contracts: inner.entries.values().filter(|e| e.is_tradeable()).count(),
            healthy: inner.healthy,
            consecutive_failures: inner.consecutive_failures,
            last_refresh: inner.last_refresh,
        }
    }

    /// Apply discovered markets to the registry.
    /// Preserves lock state for contracts that already exist.
    fn apply_discovery(&self, markets: Vec<MarketMeta>) {
        let mut inner = self.inner.write();
        let now = Utc::now();

        // Mark expired contracts
        for entry in inner.entries.values_mut() {
            if entry.is_expired() && entry.lock_state != LockState::Locked {
                entry.lock_state = LockState::Cooldown;
            }
        }

        // Upsert discovered markets
        for meta in &markets {
            inner.markets.insert(meta.market_id.clone(), meta.clone());

            for outcome in &meta.outcomes {
                let key = ContractKey {
                    market_id: meta.market_id.clone(),
                    token_id: outcome.token_id.clone(),
                };

                inner
                    .entries
                    .entry(key.clone())
                    .or_insert_with(|| ContractEntry {
                        key,
                        market_id: meta.market_id.clone(),
                        token_id: outcome.token_id.clone(),
                        asset: meta.asset,
                        duration: meta.duration,
                        outcome: outcome.outcome,
                        expiry: meta.expiry,
                        lock_state: LockState::Unlocked,
                        lock_changed_at: None,
                    });
            }
        }

        inner.last_refresh = Some(now);
        inner.consecutive_failures = 0;
        inner.healthy = true;
    }

    /// Record a discovery failure.
    fn record_failure(&self) {
        let mut inner = self.inner.write();
        inner.consecutive_failures += 1;
        if inner.consecutive_failures >= 3 {
            inner.healthy = false;
        }
    }

    /// Spawn the background refresh loop.
    pub fn spawn_refresh(
        self,
        config: PolymarketConfig,
        client: Client,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let refresh_interval = config.discovery_refresh;

        tokio::spawn(async move {
            // Initial discovery
            match gamma::discover(&client, &config).await {
                Ok(markets) => {
                    info!(count = markets.len(), "initial discovery complete");
                    self.apply_discovery(markets);
                }
                Err(e) => {
                    error!(error = %e, "initial discovery failed");
                    self.record_failure();
                }
            }

            let mut interval = tokio::time::interval(refresh_interval);
            interval.tick().await; // consume the first immediate tick

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match gamma::discover(&client, &config).await {
                            Ok(markets) => {
                                let health = self.health();
                                info!(
                                    count = markets.len(),
                                    active = health.active_contracts,
                                    "discovery refresh complete"
                                );
                                self.apply_discovery(markets);
                            }
                            Err(e) => {
                                warn!(error = %e, "discovery refresh failed");
                                self.record_failure();
                                let health = self.health();
                                if !health.healthy {
                                    error!(
                                        failures = health.consecutive_failures,
                                        "discovery unhealthy — trading blocked"
                                    );
                                }
                            }
                        }
                    }
                    _ = cancel.cancelled() => {
                        info!("discovery refresh loop shutting down");
                        break;
                    }
                }
            }
        })
    }
}

impl Default for ContractRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{Outcome, OutcomeMeta, TokenId};

    fn make_meta(id: &str, asset: Asset, dur: MarketDuration) -> MarketMeta {
        MarketMeta {
            market_id: MarketId(id.to_string()),
            asset,
            duration: dur,
            expiry: Utc::now() + chrono::Duration::minutes(10),
            outcomes: vec![
                OutcomeMeta {
                    token_id: TokenId(format!("{id}-up")),
                    outcome: Outcome::Up,
                    label: "Up".into(),
                },
                OutcomeMeta {
                    token_id: TokenId(format!("{id}-down")),
                    outcome: Outcome::Down,
                    label: "Down".into(),
                },
            ],
            active: true,
            discovered_at: Utc::now(),
        }
    }

    #[test]
    fn registry_starts_unhealthy() {
        let reg = ContractRegistry::new();
        assert!(!reg.is_healthy());
        assert!(reg.active_contracts().is_empty());
    }

    #[test]
    fn apply_discovery_populates_registry() {
        let reg = ContractRegistry::new();
        let markets = vec![
            make_meta("m1", Asset::BTC, MarketDuration::FiveMin),
            make_meta("m2", Asset::ETH, MarketDuration::FifteenMin),
        ];
        reg.apply_discovery(markets);

        assert!(reg.is_healthy());
        assert_eq!(reg.active_contracts().len(), 4); // 2 markets × 2 tokens
    }

    #[test]
    fn filter_by_asset_duration() {
        let reg = ContractRegistry::new();
        reg.apply_discovery(vec![
            make_meta("m1", Asset::BTC, MarketDuration::FiveMin),
            make_meta("m2", Asset::ETH, MarketDuration::FifteenMin),
        ]);

        let btc_5m = reg.contracts_for(Asset::BTC, MarketDuration::FiveMin);
        assert_eq!(btc_5m.len(), 2); // up + down tokens

        let eth_5m = reg.contracts_for(Asset::ETH, MarketDuration::FiveMin);
        assert!(eth_5m.is_empty());
    }

    #[test]
    fn lock_state_management() {
        let reg = ContractRegistry::new();
        reg.apply_discovery(vec![make_meta("m1", Asset::BTC, MarketDuration::FiveMin)]);

        let key = ContractKey {
            market_id: MarketId("m1".into()),
            token_id: TokenId("m1-up".into()),
        };

        // Initially unlocked
        let entry = reg.get(&key).unwrap();
        assert_eq!(entry.lock_state, LockState::Unlocked);
        assert!(entry.is_tradeable());

        // Lock it
        assert!(reg.set_lock(&key, LockState::Locked));
        let entry = reg.get(&key).unwrap();
        assert_eq!(entry.lock_state, LockState::Locked);
        assert!(!entry.is_tradeable());

        // Unlock
        assert!(reg.set_lock(&key, LockState::Unlocked));
        assert!(reg.get(&key).unwrap().is_tradeable());
    }

    #[test]
    fn consecutive_failures_degrade_health() {
        let reg = ContractRegistry::new();
        reg.apply_discovery(vec![make_meta("m1", Asset::BTC, MarketDuration::FiveMin)]);
        assert!(reg.is_healthy());

        reg.record_failure();
        reg.record_failure();
        assert!(reg.is_healthy()); // still healthy after 2

        reg.record_failure();
        assert!(!reg.is_healthy()); // unhealthy after 3
    }

    #[test]
    fn discovery_resets_failure_count() {
        let reg = ContractRegistry::new();
        reg.record_failure();
        reg.record_failure();
        reg.record_failure();
        assert!(!reg.is_healthy());

        reg.apply_discovery(vec![make_meta("m1", Asset::BTC, MarketDuration::FiveMin)]);
        assert!(reg.is_healthy());
        assert_eq!(reg.health().consecutive_failures, 0);
    }
}
