use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::PolymarketConfig;
use crate::feeds::health::{ConnectionState, FeedHealthMonitor};
use crate::feeds::normalization::parse_asset;
use crate::types::{BotEvent, FeedSource, ReceiptTimestamp, RtdsUpdate};

// RTDS role in v1:
// - Cross-check / verification data source
// - NOT the sole source of truth for latency-arb entry
// - Diagnostics for source divergence

/// Raw RTDS price message.
#[derive(Debug, Deserialize)]
struct RtdsMsg {
    asset: Option<String>,
    price: Option<String>,
    timestamp: Option<String>,
}

/// Spawn the RTDS WebSocket adapter.
pub fn spawn(
    config: PolymarketConfig,
    event_tx: mpsc::Sender<BotEvent>,
    health: FeedHealthMonitor,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut reconnect_attempts: u32 = 0;

        loop {
            if cancel.is_cancelled() {
                break;
            }

            health.set_state(FeedSource::PolymarketRtds, ConnectionState::Connecting);
            info!(url = %config.rtds_ws_url, "rtds: connecting");

            match connect_async(&config.rtds_ws_url).await {
                Ok((ws_stream, _)) => {
                    let (mut write, mut read) = ws_stream.split();

                    info!("rtds: connected");
                    health.set_state(FeedSource::PolymarketRtds, ConnectionState::Connected);
                    reconnect_attempts = 0;

                    loop {
                        tokio::select! {
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        let receipt = ReceiptTimestamp::now();
                                        health.record_message(FeedSource::PolymarketRtds);

                                        if let Some(event) = parse_rtds(&text, receipt)
                                            && event_tx.send(event).await.is_err()
                                        {
                                            debug!("rtds: channel closed");
                                            return;
                                        }
                                    }
                                    Some(Ok(Message::Ping(data))) => {
                                        let _ = write.send(Message::Pong(data)).await;
                                    }
                                    Some(Ok(Message::Close(_))) | None => {
                                        info!("rtds: disconnected");
                                        break;
                                    }
                                    Some(Ok(_)) => {}
                                    Some(Err(e)) => {
                                        warn!(error = %e, "rtds: ws error");
                                        break;
                                    }
                                }
                            }
                            _ = cancel.cancelled() => {
                                info!("rtds: shutdown");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "rtds: connection failed");
                }
            }

            reconnect_attempts += 1;
            health.set_state(FeedSource::PolymarketRtds, ConnectionState::Reconnecting);
            let backoff = Duration::from_millis(1000 * u64::from(reconnect_attempts.min(6)));

            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = cancel.cancelled() => return,
            }
        }

        health.set_state(FeedSource::PolymarketRtds, ConnectionState::Disconnected);
    })
}

/// Parse an RTDS message into a BotEvent.
fn parse_rtds(text: &str, receipt: ReceiptTimestamp) -> Option<BotEvent> {
    let msg: RtdsMsg = serde_json::from_str(text).ok()?;
    let asset = parse_asset(msg.asset.as_deref()?)?;
    let price = Decimal::from_str(msg.price.as_deref()?).ok()?;
    let source_timestamp: DateTime<Utc> = msg
        .timestamp
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(Utc::now);

    Some(BotEvent::RtdsUpdate(RtdsUpdate {
        asset,
        price,
        source_timestamp,
        receipt_timestamp: receipt,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::Asset;

    #[test]
    fn parse_valid_rtds() {
        let json = r#"{"asset":"BTC","price":"67500.00","timestamp":"2024-01-01T00:00:00Z"}"#;
        let event = parse_rtds(json, ReceiptTimestamp::now()).unwrap();
        match event {
            BotEvent::RtdsUpdate(update) => {
                assert_eq!(update.asset, Asset::BTC);
                assert_eq!(update.price, Decimal::from_str("67500.00").unwrap());
            }
            _ => panic!("expected RtdsUpdate"),
        }
    }

    #[test]
    fn reject_unsupported_asset() {
        let json = r#"{"asset":"DOGE","price":"0.15"}"#;
        assert!(parse_rtds(json, ReceiptTimestamp::now()).is_none());
    }

    #[test]
    fn reject_missing_price() {
        let json = r#"{"asset":"BTC"}"#;
        assert!(parse_rtds(json, ReceiptTimestamp::now()).is_none());
    }
}
