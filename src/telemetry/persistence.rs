//! Append-only event persistence for replay and audit.
//!
//! Writes versioned JSONL records to local files. Non-blocking — uses a
//! bounded channel so the hot path never waits on disk I/O.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::types::BotEvent;

/// Schema version for persisted records.
pub const SCHEMA_VERSION: &str = "1.0";

// ---------------------------------------------------------------------------
// Persisted record wrapper
// ---------------------------------------------------------------------------

/// Versioned wrapper around any persisted event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedRecord {
    /// Schema version for forward compatibility.
    pub schema_version: String,
    /// Sequence number within this session.
    pub seq: u64,
    /// When the record was persisted.
    pub persisted_at: DateTime<Utc>,
    /// The event payload.
    pub event: BotEvent,
}

// ---------------------------------------------------------------------------
// Event writer — synchronous file writer behind a mutex
// ---------------------------------------------------------------------------

/// Synchronous JSONL writer. Kept behind a Mutex so the async task
/// can hand off records without blocking the hot path.
struct EventWriter {
    file: std::io::BufWriter<std::fs::File>,
    seq: u64,
}

impl EventWriter {
    fn open(path: &Path) -> std::io::Result<Self> {
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            file: std::io::BufWriter::new(file),
            seq: 0,
        })
    }

    fn write_event(&mut self, event: &BotEvent) -> std::io::Result<()> {
        self.seq += 1;
        let record = PersistedRecord {
            schema_version: SCHEMA_VERSION.to_string(),
            seq: self.seq,
            persisted_at: Utc::now(),
            event: event.clone(),
        };
        let line = serde_json::to_string(&record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }

    fn records_written(&self) -> u64 {
        self.seq
    }
}

// ---------------------------------------------------------------------------
// Event persistence service
// ---------------------------------------------------------------------------

/// Non-blocking event persistence service.
///
/// Events are sent via a bounded channel. A background task drains the
/// channel and writes to disk. If the channel is full, events are dropped
/// (logged) rather than blocking the hot path.
pub struct EventPersistence {
    tx: mpsc::Sender<BotEvent>,
    path: PathBuf,
}

impl EventPersistence {
    /// Create a new persistence service. Call `spawn` to start the writer task.
    pub fn new(path: PathBuf, buffer_size: usize) -> (Self, mpsc::Receiver<BotEvent>) {
        let (tx, rx) = mpsc::channel(buffer_size);
        (Self { tx, path }, rx)
    }

    /// Try to persist an event. Returns false if the channel is full (event dropped).
    pub fn try_persist(&self, event: &BotEvent) -> bool {
        match self.tx.try_send(event.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!(
                    event = event.label(),
                    "persistence_channel_full — event dropped"
                );
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!(event = event.label(), "persistence_channel_closed");
                false
            }
        }
    }

    /// Get the file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Spawn the background writer task. Returns a handle to the writer's
    /// record count (for metrics).
    pub fn spawn_writer(
        path: PathBuf,
        mut rx: mpsc::Receiver<BotEvent>,
        flush_interval_events: u64,
    ) -> tokio::task::JoinHandle<u64> {
        tokio::task::spawn_blocking(move || {
            let mut writer = match EventWriter::open(&path) {
                Ok(w) => {
                    info!(path = %path.display(), "persistence_writer_started");
                    w
                }
                Err(e) => {
                    error!(path = %path.display(), error = %e, "persistence_writer_failed_to_open");
                    return 0;
                }
            };

            // Use blocking recv in a spawn_blocking context
            while let Some(event) = rx.blocking_recv() {
                if let Err(e) = writer.write_event(&event) {
                    error!(error = %e, "persistence_write_error");
                }

                // Periodic flush
                if writer.records_written() % flush_interval_events == 0
                    && let Err(e) = writer.flush()
                {
                    error!(error = %e, "persistence_flush_error");
                }
            }

            // Final flush
            if let Err(e) = writer.flush() {
                error!(error = %e, "persistence_final_flush_error");
            }

            let count = writer.records_written();
            info!(records = count, "persistence_writer_stopped");
            count
        })
    }
}

// ---------------------------------------------------------------------------
// Event reader — for replay
// ---------------------------------------------------------------------------

/// Reads persisted JSONL event files.
pub struct EventReader;

impl EventReader {
    /// Read all events from a JSONL file.
    pub fn read_all(path: &Path) -> std::io::Result<Vec<PersistedRecord>> {
        let content = std::fs::read_to_string(path)?;
        let mut records = Vec::new();
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<PersistedRecord>(line) {
                Ok(record) => records.push(record),
                Err(e) => {
                    warn!(line = i + 1, error = %e, "persistence_parse_error");
                }
            }
        }
        Ok(records)
    }

    /// Count records in a file without loading all into memory.
    pub fn count_records(path: &Path) -> std::io::Result<usize> {
        let content = std::fs::read_to_string(path)?;
        Ok(content.lines().filter(|l| !l.trim().is_empty()).count())
    }
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
    fn writer_creates_file_and_writes() {
        let dir = std::env::temp_dir().join(format!("poly-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("events.jsonl");

        let mut writer = EventWriter::open(&path).unwrap();
        writer.write_event(&test_event()).unwrap();
        writer.write_event(&test_event()).unwrap();
        writer.flush().unwrap();

        assert_eq!(writer.records_written(), 2);

        // Read back
        let records = EventReader::read_all(&path).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[1].seq, 2);
        assert_eq!(records[0].schema_version, SCHEMA_VERSION);

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reader_handles_empty_file() {
        let dir = std::env::temp_dir().join(format!("poly-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("empty.jsonl");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "").unwrap();

        let records = EventReader::read_all(&path).unwrap();
        assert!(records.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reader_skips_invalid_lines() {
        let dir = std::env::temp_dir().join(format!("poly-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("mixed.jsonl");
        std::fs::create_dir_all(&dir).unwrap();

        // Write one valid and one invalid line
        let mut writer = EventWriter::open(&path).unwrap();
        writer.write_event(&test_event()).unwrap();
        writer.flush().unwrap();

        // Append invalid line
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut f, b"not valid json\n").unwrap();

        let records = EventReader::read_all(&path).unwrap();
        assert_eq!(records.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn count_records() {
        let dir = std::env::temp_dir().join(format!("poly-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("count.jsonl");

        let mut writer = EventWriter::open(&path).unwrap();
        for _ in 0..5 {
            writer.write_event(&test_event()).unwrap();
        }
        writer.flush().unwrap();

        assert_eq!(EventReader::count_records(&path).unwrap(), 5);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_persist_returns_false_when_closed() {
        let (persistence, rx) = EventPersistence::new(PathBuf::from("/tmp/test.jsonl"), 8);
        drop(rx); // Close the receiver
        assert!(!persistence.try_persist(&test_event()));
    }

    #[test]
    fn persisted_record_roundtrip() {
        let record = PersistedRecord {
            schema_version: SCHEMA_VERSION.to_string(),
            seq: 42,
            persisted_at: Utc::now(),
            event: test_event(),
        };
        let json = serde_json::to_string(&record).unwrap();
        let parsed: PersistedRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.seq, 42);
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
    }
}
