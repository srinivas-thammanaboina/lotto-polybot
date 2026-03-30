//! Append-only event capture for replay.
//!
//! Thin wrapper around telemetry persistence, specifically for recording
//! replay-ready sessions with metadata.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::telemetry::persistence::{EventPersistence, EventReader, PersistedRecord};
use crate::types::BotEvent;

/// Metadata for a recorded session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub config_version: String,
    pub mode: String,
    pub event_count: u64,
}

/// Records a session to a JSONL file with metadata sidecar.
pub struct SessionRecorder {
    persistence: EventPersistence,
    meta: SessionMeta,
    meta_path: PathBuf,
}

impl SessionRecorder {
    pub fn new(
        session_dir: &Path,
        session_id: &str,
        config_version: &str,
        mode: &str,
        buffer_size: usize,
    ) -> std::io::Result<(Self, tokio::sync::mpsc::Receiver<BotEvent>)> {
        std::fs::create_dir_all(session_dir)?;

        let events_path = session_dir.join(format!("{session_id}.jsonl"));
        let meta_path = session_dir.join(format!("{session_id}.meta.json"));

        let (persistence, rx) = EventPersistence::new(events_path, buffer_size);

        let meta = SessionMeta {
            session_id: session_id.to_string(),
            started_at: Utc::now(),
            ended_at: None,
            config_version: config_version.to_string(),
            mode: mode.to_string(),
            event_count: 0,
        };

        Ok((
            Self {
                persistence,
                meta,
                meta_path,
            },
            rx,
        ))
    }

    /// Record an event. Non-blocking.
    pub fn record(&mut self, event: &BotEvent) -> bool {
        let ok = self.persistence.try_persist(event);
        if ok {
            self.meta.event_count += 1;
        }
        ok
    }

    /// Finalize the session and write metadata.
    pub fn finalize(&mut self) -> std::io::Result<()> {
        self.meta.ended_at = Some(Utc::now());
        let json = serde_json::to_string_pretty(&self.meta)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&self.meta_path, json)?;
        info!(
            session_id = %self.meta.session_id,
            events = self.meta.event_count,
            "session_recording_finalized"
        );
        Ok(())
    }

    pub fn meta(&self) -> &SessionMeta {
        &self.meta
    }
}

/// Load session metadata from a sidecar file.
pub fn load_session_meta(path: &Path) -> std::io::Result<SessionMeta> {
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Load all events from a session recording.
pub fn load_session_events(path: &Path) -> std::io::Result<Vec<PersistedRecord>> {
    EventReader::read_all(path)
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
    fn session_recorder_records_and_finalizes() {
        let dir = std::env::temp_dir().join(format!("poly-session-{}", uuid::Uuid::new_v4()));
        let (mut recorder, _rx) =
            SessionRecorder::new(&dir, "test-session", "v1.0", "simulation", 64).unwrap();

        assert!(recorder.record(&test_event()));
        assert!(recorder.record(&test_event()));
        assert_eq!(recorder.meta().event_count, 2);

        recorder.finalize().unwrap();
        assert!(recorder.meta().ended_at.is_some());

        // Load metadata back
        let meta_path = dir.join("test-session.meta.json");
        let loaded = load_session_meta(&meta_path).unwrap();
        assert_eq!(loaded.session_id, "test-session");
        assert_eq!(loaded.event_count, 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_meta_roundtrip() {
        let meta = SessionMeta {
            session_id: "s1".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            config_version: "v1.0".into(),
            mode: "simulation".into(),
            event_count: 42,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: SessionMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_count, 42);
    }
}
