//! Deterministic replay runner.
//!
//! Loads captured event streams and reruns fair value + signal logic
//! without live network access. Supports accelerated and real-time modes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::telemetry::persistence::{EventReader, PersistedRecord};
use crate::types::BotEvent;

// ---------------------------------------------------------------------------
// Replay mode
// ---------------------------------------------------------------------------

/// How to handle timing during replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaySpeed {
    /// Process events as fast as possible.
    Accelerated,
    /// Preserve original inter-event timing.
    RealTime,
    /// Scale timing by a factor (e.g., 2.0 = 2x speed).
    Scaled(u32),
}

// ---------------------------------------------------------------------------
// Replay config
// ---------------------------------------------------------------------------

/// Configuration for a replay run.
#[derive(Debug, Clone)]
pub struct ReplayConfig {
    /// Path to the JSONL event file.
    pub events_path: std::path::PathBuf,
    /// Replay speed.
    pub speed: ReplaySpeed,
    /// Config version to tag the replay output.
    pub config_version: String,
    /// Optional: only replay events within this time range.
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Replay result
// ---------------------------------------------------------------------------

/// Summary of a replay run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    pub events_loaded: usize,
    pub events_processed: usize,
    pub events_skipped: usize,
    pub config_version: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub duration_ms: i64,
}

// ---------------------------------------------------------------------------
// Replay runner
// ---------------------------------------------------------------------------

/// Deterministic replay runner. Loads events and feeds them through a
/// user-provided handler function.
pub struct ReplayRunner;

impl ReplayRunner {
    /// Load events from a file, applying optional time filters.
    pub fn load_events(config: &ReplayConfig) -> std::io::Result<Vec<PersistedRecord>> {
        let mut records = EventReader::read_all(&config.events_path)?;

        // Apply time filters
        if let Some(start) = config.start_time {
            records.retain(|r| r.persisted_at >= start);
        }
        if let Some(end) = config.end_time {
            records.retain(|r| r.persisted_at <= end);
        }

        // Sort by sequence number for deterministic ordering
        records.sort_by_key(|r| r.seq);

        info!(
            events = records.len(),
            path = %config.events_path.display(),
            "replay_events_loaded"
        );

        Ok(records)
    }

    /// Run replay synchronously, calling the handler for each event.
    /// Returns a summary of the run.
    pub fn run_sync<F>(
        config: &ReplayConfig,
        records: &[PersistedRecord],
        mut handler: F,
    ) -> ReplayResult
    where
        F: FnMut(&BotEvent, u64) -> bool,
    {
        let started_at = Utc::now();
        let total = records.len();
        let mut processed = 0usize;
        let mut skipped = 0usize;

        let mut prev_time: Option<DateTime<Utc>> = None;

        for record in records {
            // Inter-event delay for real-time / scaled modes
            if config.speed != ReplaySpeed::Accelerated
                && let Some(prev) = prev_time
            {
                let gap = record.persisted_at - prev;
                let delay = match config.speed {
                    ReplaySpeed::RealTime => gap,
                    ReplaySpeed::Scaled(factor) => {
                        if factor > 0 {
                            gap / factor as i32
                        } else {
                            chrono::Duration::zero()
                        }
                    }
                    ReplaySpeed::Accelerated => chrono::Duration::zero(),
                };
                if delay > chrono::Duration::zero()
                    && let Ok(std_delay) = delay.to_std()
                {
                    std::thread::sleep(std_delay);
                }
            }
            prev_time = Some(record.persisted_at);

            let should_continue = handler(&record.event, record.seq);
            if should_continue {
                processed += 1;
            } else {
                skipped += 1;
            }

            debug!(
                seq = record.seq,
                event = record.event.label(),
                "replay_event"
            );
        }

        let completed_at = Utc::now();
        let duration = completed_at - started_at;

        info!(
            total = total,
            processed = processed,
            skipped = skipped,
            duration_ms = duration.num_milliseconds(),
            "replay_complete"
        );

        ReplayResult {
            events_loaded: total,
            events_processed: processed,
            events_skipped: skipped,
            config_version: config.config_version.clone(),
            started_at,
            completed_at,
            duration_ms: duration.num_milliseconds(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::Asset;
    use crate::telemetry::persistence::{PersistedRecord, SCHEMA_VERSION};
    use crate::types::{CexTick, FeedSource, ReceiptTimestamp};
    use rust_decimal_macros::dec;

    fn test_records(n: usize) -> Vec<PersistedRecord> {
        (0..n)
            .map(|i| PersistedRecord {
                schema_version: SCHEMA_VERSION.to_string(),
                seq: i as u64 + 1,
                persisted_at: Utc::now() + chrono::Duration::milliseconds(i as i64 * 10),
                event: BotEvent::CexTick(CexTick {
                    source: FeedSource::Binance,
                    asset: Asset::BTC,
                    price: dec!(100000),
                    quantity: dec!(0.5),
                    source_timestamp: Utc::now(),
                    receipt_timestamp: ReceiptTimestamp::now(),
                }),
            })
            .collect()
    }

    #[test]
    fn replay_processes_all_events() {
        let records = test_records(10);
        let config = ReplayConfig {
            events_path: std::path::PathBuf::from("test.jsonl"),
            speed: ReplaySpeed::Accelerated,
            config_version: "v1.0".into(),
            start_time: None,
            end_time: None,
        };

        let result = ReplayRunner::run_sync(&config, &records, |_event, _seq| true);
        assert_eq!(result.events_loaded, 10);
        assert_eq!(result.events_processed, 10);
        assert_eq!(result.events_skipped, 0);
    }

    #[test]
    fn replay_handler_can_skip() {
        let records = test_records(5);
        let config = ReplayConfig {
            events_path: std::path::PathBuf::from("test.jsonl"),
            speed: ReplaySpeed::Accelerated,
            config_version: "v1.0".into(),
            start_time: None,
            end_time: None,
        };

        let result = ReplayRunner::run_sync(&config, &records, |_event, seq| seq % 2 == 0);
        assert_eq!(result.events_processed, 2); // seq 2 and 4
        assert_eq!(result.events_skipped, 3);
    }

    #[test]
    fn replay_result_has_metadata() {
        let records = test_records(3);
        let config = ReplayConfig {
            events_path: std::path::PathBuf::from("test.jsonl"),
            speed: ReplaySpeed::Accelerated,
            config_version: "v2.0".into(),
            start_time: None,
            end_time: None,
        };

        let result = ReplayRunner::run_sync(&config, &records, |_, _| true);
        assert_eq!(result.config_version, "v2.0");
        assert!(result.duration_ms >= 0);
    }

    #[test]
    fn replay_result_serializable() {
        let result = ReplayResult {
            events_loaded: 100,
            events_processed: 95,
            events_skipped: 5,
            config_version: "v1.0".into(),
            started_at: Utc::now(),
            completed_at: Utc::now(),
            duration_ms: 500,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ReplayResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.events_loaded, 100);
    }
}
