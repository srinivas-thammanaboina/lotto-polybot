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
use crate::domain::market::{BookSide, BookSnapshot, PriceLevel, TokenId};
use crate::feeds::health::FeedHealthMonitor;
use crate::types::{BotEvent, BookUpdate, ReceiptTimestamp};

/// Raw Polymarket book message.
#[derive(Debug, Deserialize)]
struct PolyBookMsg {
    asset_id: Option<String>,
    bids: Option<Vec<RawLevel>>,
    asks: Option<Vec<RawLevel>>,
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawLevel {
    price: Option<String>,
    size: Option<String>,
}

/// Spawn the Polymarket market WebSocket adapter.
/// Maintains top-of-book and depth for subscribed tokens.
pub fn spawn(
    config: PolymarketConfig,
    token_ids: Vec<String>,
    event_tx: mpsc::Sender<BotEvent>,
    _health: FeedHealthMonitor,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    // Polymarket book staleness is tracked by timestamp checks in the
    // signal engine. The health monitor is accepted for future use.

    tokio::spawn(async move {
        let mut reconnect_attempts: u32 = 0;

        loop {
            if cancel.is_cancelled() {
                break;
            }

            info!(url = %config.market_ws_url, "polymarket-market: connecting");

            match connect_async(&config.market_ws_url).await {
                Ok((ws_stream, _)) => {
                    let (mut write, mut read) = ws_stream.split();

                    // Subscribe to token IDs
                    for token_id in &token_ids {
                        let sub = serde_json::json!({
                            "type": "market",
                            "assets_ids": [token_id],
                        });
                        if let Err(e) = write.send(Message::Text(sub.to_string().into())).await {
                            error!(error = %e, "polymarket-market: subscribe failed");
                            break;
                        }
                    }

                    info!(
                        tokens = token_ids.len(),
                        "polymarket-market: connected and subscribed"
                    );
                    reconnect_attempts = 0;

                    loop {
                        tokio::select! {
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        let receipt = ReceiptTimestamp::now();
                                        if let Some(event) = parse_book_update(&text, receipt)
                                            && event_tx.send(event).await.is_err()
                                        {
                                            debug!("polymarket-market: channel closed");
                                            return;
                                        }
                                    }
                                    Some(Ok(Message::Ping(data))) => {
                                        let _ = write.send(Message::Pong(data)).await;
                                    }
                                    Some(Ok(Message::Close(_))) | None => {
                                        info!("polymarket-market: disconnected");
                                        break;
                                    }
                                    Some(Ok(_)) => {}
                                    Some(Err(e)) => {
                                        warn!(error = %e, "polymarket-market: ws error");
                                        break;
                                    }
                                }
                            }
                            _ = cancel.cancelled() => {
                                info!("polymarket-market: shutdown");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "polymarket-market: connection failed");
                }
            }

            reconnect_attempts += 1;
            let backoff = Duration::from_millis(1000 * u64::from(reconnect_attempts.min(6)));
            info!(attempt = reconnect_attempts, "polymarket-market: reconnecting");

            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = cancel.cancelled() => return,
            }
        }
    })
}

/// Parse a raw book message into a BotEvent.
fn parse_book_update(text: &str, receipt: ReceiptTimestamp) -> Option<BotEvent> {
    let msg: PolyBookMsg = serde_json::from_str(text).ok()?;
    let token_id = TokenId(msg.asset_id?);
    let timestamp = msg
        .timestamp
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(chrono::Utc::now);

    let bids = parse_levels(msg.bids.as_deref().unwrap_or(&[]));
    let asks = parse_levels(msg.asks.as_deref().unwrap_or(&[]));

    Some(BotEvent::BookUpdate(BookUpdate {
        token_id: token_id.clone(),
        snapshot: BookSnapshot {
            token_id,
            bids: BookSide { levels: bids },
            asks: BookSide { levels: asks },
            timestamp,
        },
        receipt_timestamp: receipt,
    }))
}

fn parse_levels(raw: &[RawLevel]) -> Vec<PriceLevel> {
    raw.iter()
        .filter_map(|l| {
            let price = Decimal::from_str(l.price.as_deref()?).ok()?;
            let size = Decimal::from_str(l.size.as_deref()?).ok()?;
            Some(PriceLevel { price, size })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_book() {
        let json = r#"{
            "asset_id": "token-123",
            "bids": [{"price": "0.55", "size": "100"}],
            "asks": [{"price": "0.60", "size": "50"}],
            "timestamp": "2024-01-01T00:00:00Z"
        }"#;
        let event = parse_book_update(json, ReceiptTimestamp::now()).unwrap();
        match event {
            BotEvent::BookUpdate(update) => {
                assert_eq!(update.token_id.0, "token-123");
                assert_eq!(update.snapshot.bids.levels.len(), 1);
                assert_eq!(update.snapshot.asks.levels.len(), 1);
            }
            _ => panic!("expected BookUpdate"),
        }
    }

    #[test]
    fn parse_empty_book() {
        let json = r#"{"asset_id": "token-456"}"#;
        let event = parse_book_update(json, ReceiptTimestamp::now()).unwrap();
        match event {
            BotEvent::BookUpdate(update) => {
                assert!(update.snapshot.bids.levels.is_empty());
                assert!(update.snapshot.asks.levels.is_empty());
            }
            _ => panic!("expected BookUpdate"),
        }
    }
}
