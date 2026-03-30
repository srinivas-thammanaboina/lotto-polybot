use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;

use crate::types::FeedSource;

/// Connection state for a feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

/// Per-feed health record.
#[derive(Debug, Clone)]
pub struct FeedStatus {
    pub source: FeedSource,
    pub state: ConnectionState,
    pub last_message_at: Option<DateTime<Utc>>,
    pub reconnect_count: u32,
    pub parse_errors: u64,
    pub stale_threshold: Duration,
}

impl FeedStatus {
    pub fn new(source: FeedSource, stale_threshold: Duration) -> Self {
        Self {
            source,
            state: ConnectionState::Disconnected,
            last_message_at: None,
            reconnect_count: 0,
            parse_errors: 0,
            stale_threshold,
        }
    }

    /// Is the feed data stale?
    pub fn is_stale(&self) -> bool {
        match self.last_message_at {
            None => true,
            Some(ts) => {
                let age = Utc::now() - ts;
                age > chrono::Duration::from_std(self.stale_threshold)
                    .unwrap_or(chrono::Duration::seconds(5))
            }
        }
    }

    /// Is the feed connected and not stale?
    pub fn is_healthy(&self) -> bool {
        self.state == ConnectionState::Connected && !self.is_stale()
    }
}

/// Aggregate health snapshot.
#[derive(Debug, Clone)]
pub struct FeedHealthSnapshot {
    pub feeds: Vec<FeedStatus>,
}

impl FeedHealthSnapshot {
    /// Are all required feeds healthy?
    pub fn all_healthy(&self) -> bool {
        self.feeds.iter().all(|f| f.is_healthy())
    }

    /// Is a specific source healthy?
    pub fn source_healthy(&self, source: FeedSource) -> bool {
        self.feeds
            .iter()
            .find(|f| f.source == source)
            .map(|f| f.is_healthy())
            .unwrap_or(false)
    }
}

/// Thread-safe feed health monitor.
#[derive(Debug, Clone)]
pub struct FeedHealthMonitor {
    inner: Arc<RwLock<HashMap<FeedSource, FeedStatus>>>,
}

impl FeedHealthMonitor {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a feed with its staleness threshold.
    pub fn register(&self, source: FeedSource, stale_threshold: Duration) {
        self.inner
            .write()
            .insert(source, FeedStatus::new(source, stale_threshold));
    }

    /// Update connection state.
    pub fn set_state(&self, source: FeedSource, state: ConnectionState) {
        if let Some(status) = self.inner.write().get_mut(&source) {
            status.state = state;
            if state == ConnectionState::Reconnecting {
                status.reconnect_count += 1;
            }
        }
    }

    /// Record a message received.
    pub fn record_message(&self, source: FeedSource) {
        if let Some(status) = self.inner.write().get_mut(&source) {
            status.last_message_at = Some(Utc::now());
        }
    }

    /// Record a parse error.
    pub fn record_parse_error(&self, source: FeedSource) {
        if let Some(status) = self.inner.write().get_mut(&source) {
            status.parse_errors += 1;
        }
    }

    /// Check if a specific source is healthy.
    pub fn is_healthy(&self, source: FeedSource) -> bool {
        self.inner
            .read()
            .get(&source)
            .map(|s| s.is_healthy())
            .unwrap_or(false)
    }

    /// Check if a specific source is stale.
    pub fn is_stale(&self, source: FeedSource) -> bool {
        self.inner
            .read()
            .get(&source)
            .map(|s| s.is_stale())
            .unwrap_or(true)
    }

    /// Get a full health snapshot.
    pub fn snapshot(&self) -> FeedHealthSnapshot {
        let inner = self.inner.read();
        FeedHealthSnapshot {
            feeds: inner.values().cloned().collect(),
        }
    }
}

impl Default for FeedHealthMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_feed_is_stale_and_unhealthy() {
        let monitor = FeedHealthMonitor::new();
        monitor.register(FeedSource::Binance, Duration::from_secs(3));

        assert!(monitor.is_stale(FeedSource::Binance));
        assert!(!monitor.is_healthy(FeedSource::Binance));
    }

    #[test]
    fn connected_with_recent_message_is_healthy() {
        let monitor = FeedHealthMonitor::new();
        monitor.register(FeedSource::Binance, Duration::from_secs(3));
        monitor.set_state(FeedSource::Binance, ConnectionState::Connected);
        monitor.record_message(FeedSource::Binance);

        assert!(!monitor.is_stale(FeedSource::Binance));
        assert!(monitor.is_healthy(FeedSource::Binance));
    }

    #[test]
    fn disconnected_is_unhealthy() {
        let monitor = FeedHealthMonitor::new();
        monitor.register(FeedSource::Binance, Duration::from_secs(3));
        monitor.record_message(FeedSource::Binance);
        // Still disconnected
        assert!(!monitor.is_healthy(FeedSource::Binance));
    }

    #[test]
    fn unregistered_source_is_unhealthy() {
        let monitor = FeedHealthMonitor::new();
        assert!(!monitor.is_healthy(FeedSource::Coinbase));
        assert!(monitor.is_stale(FeedSource::Coinbase));
    }

    #[test]
    fn reconnect_increments_count() {
        let monitor = FeedHealthMonitor::new();
        monitor.register(FeedSource::Binance, Duration::from_secs(3));
        monitor.set_state(FeedSource::Binance, ConnectionState::Reconnecting);
        monitor.set_state(FeedSource::Binance, ConnectionState::Reconnecting);

        let snap = monitor.snapshot();
        let binance = snap
            .feeds
            .iter()
            .find(|f| f.source == FeedSource::Binance)
            .unwrap();
        assert_eq!(binance.reconnect_count, 2);
    }
}
