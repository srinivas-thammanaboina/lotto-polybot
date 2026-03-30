use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::CoinbaseConfig;
use crate::feeds::health::{ConnectionState, FeedHealthMonitor};
use crate::feeds::normalization::parse_asset;
use crate::types::{BotEvent, CexTick, FeedSource, ReceiptTimestamp};

// V1 role: backup/failover only.
// Coinbase does NOT add latency to the happy path.
// This adapter is capable of normalization and health tracking
// even when not on the critical path.

/// Raw Coinbase match message.
#[derive(Debug, Deserialize)]
struct CoinbaseMatch {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    product_id: Option<String>,
    price: Option<String>,
    size: Option<String>,
    time: Option<String>,
}

/// Spawn the Coinbase WebSocket adapter task.
pub fn spawn(
    config: CoinbaseConfig,
    event_tx: mpsc::Sender<BotEvent>,
    health: FeedHealthMonitor,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if !config.enabled {
            info!("coinbase: disabled in config, skipping");
            cancel.cancelled().await;
            return;
        }

        let mut reconnect_attempts: u32 = 0;

        loop {
            if cancel.is_cancelled() {
                break;
            }

            health.set_state(FeedSource::Coinbase, ConnectionState::Connecting);
            info!(url = %config.ws_url, "coinbase: connecting");

            match connect_async(&config.ws_url).await {
                Ok((ws_stream, _)) => {
                    let (mut write, mut read) = ws_stream.split();

                    // Subscribe to BTC and ETH matches
                    let subscribe = json!({
                        "type": "subscribe",
                        "channels": [
                            {
                                "name": "matches",
                                "product_ids": ["BTC-USD", "ETH-USD"]
                            }
                        ]
                    });

                    if let Err(e) = write
                        .send(Message::Text(subscribe.to_string().into()))
                        .await
                    {
                        error!(error = %e, "coinbase: subscribe failed");
                        break;
                    }

                    info!("coinbase: connected and subscribed");
                    health.set_state(FeedSource::Coinbase, ConnectionState::Connected);
                    reconnect_attempts = 0;

                    loop {
                        tokio::select! {
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        let receipt = ReceiptTimestamp::now();
                                        health.record_message(FeedSource::Coinbase);

                                        if let Some(event) = parse_match(&text, receipt)
                                            && event_tx.send(event).await.is_err()
                                        {
                                            debug!("coinbase: event channel closed");
                                            return;
                                        }
                                    }
                                    Some(Ok(Message::Ping(data))) => {
                                        if write.send(Message::Pong(data)).await.is_err() {
                                            warn!("coinbase: pong failed");
                                            break;
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) => {
                                        info!("coinbase: server closed");
                                        break;
                                    }
                                    Some(Ok(_)) => {}
                                    Some(Err(e)) => {
                                        warn!(error = %e, "coinbase: ws error");
                                        break;
                                    }
                                    None => {
                                        info!("coinbase: stream ended");
                                        break;
                                    }
                                }
                            }
                            _ = cancel.cancelled() => {
                                info!("coinbase: shutdown requested");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "coinbase: connection failed");
                }
            }

            reconnect_attempts += 1;
            health.set_state(FeedSource::Coinbase, ConnectionState::Reconnecting);
            let backoff = Duration::from_millis(
                config.reconnect_backoff_ms * u64::from(reconnect_attempts.min(6)),
            );
            info!(attempt = reconnect_attempts, "coinbase: reconnecting");

            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = cancel.cancelled() => return,
            }
        }

        health.set_state(FeedSource::Coinbase, ConnectionState::Disconnected);
    })
}

/// Parse a Coinbase match message into a BotEvent.
fn parse_match(text: &str, receipt: ReceiptTimestamp) -> Option<BotEvent> {
    let msg: CoinbaseMatch = serde_json::from_str(text).ok()?;

    if msg.msg_type.as_deref() != Some("match") && msg.msg_type.as_deref() != Some("last_match") {
        return None;
    }

    let product = msg.product_id.as_deref()?;
    let asset = parse_asset(product)?;
    let price = Decimal::from_str(msg.price.as_deref()?).ok()?;
    let quantity = Decimal::from_str(msg.size.as_deref()?).ok()?;
    let source_timestamp = msg
        .time
        .as_deref()
        .and_then(|t| t.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(Utc::now);

    Some(BotEvent::CexTick(CexTick {
        source: FeedSource::Coinbase,
        asset,
        price,
        quantity,
        source_timestamp,
        receipt_timestamp: receipt,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::Asset;

    #[test]
    fn parse_valid_match() {
        let json = r#"{"type":"match","product_id":"BTC-USD","price":"67500.00","size":"0.01","time":"2024-01-01T00:00:00.000Z"}"#;
        let event = parse_match(json, ReceiptTimestamp::now()).unwrap();
        match event {
            BotEvent::CexTick(tick) => {
                assert_eq!(tick.asset, Asset::BTC);
                assert_eq!(tick.source, FeedSource::Coinbase);
            }
            _ => panic!("expected CexTick"),
        }
    }

    #[test]
    fn ignore_non_match_type() {
        let json = r#"{"type":"subscriptions","channels":[]}"#;
        assert!(parse_match(json, ReceiptTimestamp::now()).is_none());
    }
}
