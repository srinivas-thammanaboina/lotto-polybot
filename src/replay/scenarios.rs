//! Replay scenario fixture management.
//!
//! Allows building test scenarios from event sequences for regression testing.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::telemetry::persistence::{PersistedRecord, SCHEMA_VERSION};
use crate::types::BotEvent;

/// A replay scenario: a named sequence of events with expected outcomes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayScenario {
    pub name: String,
    pub description: String,
    pub events: Vec<PersistedRecord>,
    pub expected_signals_accepted: u64,
    pub expected_signals_rejected: u64,
}

/// Builder for constructing replay scenarios programmatically.
pub struct ScenarioBuilder {
    name: String,
    description: String,
    events: Vec<BotEvent>,
    expected_accepted: u64,
    expected_rejected: u64,
}

impl ScenarioBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: String::new(),
            events: Vec::new(),
            expected_accepted: 0,
            expected_rejected: 0,
        }
    }

    pub fn description(mut self, desc: &str) -> Self {
        self.description = desc.to_string();
        self
    }

    pub fn event(mut self, event: BotEvent) -> Self {
        self.events.push(event);
        self
    }

    pub fn events(mut self, events: Vec<BotEvent>) -> Self {
        self.events.extend(events);
        self
    }

    pub fn expect_accepted(mut self, count: u64) -> Self {
        self.expected_accepted = count;
        self
    }

    pub fn expect_rejected(mut self, count: u64) -> Self {
        self.expected_rejected = count;
        self
    }

    pub fn build(self) -> ReplayScenario {
        let now = Utc::now();
        let records = self
            .events
            .into_iter()
            .enumerate()
            .map(|(i, event)| PersistedRecord {
                schema_version: SCHEMA_VERSION.to_string(),
                seq: i as u64 + 1,
                persisted_at: now + chrono::Duration::milliseconds(i as i64 * 10),
                event,
            })
            .collect();

        ReplayScenario {
            name: self.name,
            description: self.description,
            events: records,
            expected_signals_accepted: self.expected_accepted,
            expected_signals_rejected: self.expected_rejected,
        }
    }
}

/// Save a scenario to a JSON file.
pub fn save_scenario(scenario: &ReplayScenario, path: &std::path::Path) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(scenario)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

/// Load a scenario from a JSON file.
pub fn load_scenario(path: &std::path::Path) -> std::io::Result<ReplayScenario> {
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::Asset;
    use crate::types::{CexTick, FeedSource, ReceiptTimestamp};
    use rust_decimal_macros::dec;

    fn test_event() -> BotEvent {
        BotEvent::CexTick(CexTick {
            source: FeedSource::Binance,
            asset: Asset::BTC,
            price: dec!(100000),
            quantity: dec!(0.5),
            source_timestamp: Utc::now(),
            receipt_timestamp: ReceiptTimestamp::now(),
        })
    }

    #[test]
    fn builder_creates_scenario() {
        let scenario = ScenarioBuilder::new("test-scenario")
            .description("A test scenario")
            .event(test_event())
            .event(test_event())
            .expect_accepted(1)
            .expect_rejected(1)
            .build();

        assert_eq!(scenario.name, "test-scenario");
        assert_eq!(scenario.events.len(), 2);
        assert_eq!(scenario.expected_signals_accepted, 1);
        assert_eq!(scenario.expected_signals_rejected, 1);
    }

    #[test]
    fn scenario_roundtrip() {
        let scenario = ScenarioBuilder::new("roundtrip")
            .event(test_event())
            .build();

        let dir = std::env::temp_dir().join(format!("poly-scenario-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scenario.json");

        save_scenario(&scenario, &path).unwrap();
        let loaded = load_scenario(&path).unwrap();

        assert_eq!(loaded.name, "roundtrip");
        assert_eq!(loaded.events.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn events_have_sequential_seqs() {
        let scenario = ScenarioBuilder::new("seq")
            .event(test_event())
            .event(test_event())
            .event(test_event())
            .build();

        let seqs: Vec<u64> = scenario.events.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
    }
}
