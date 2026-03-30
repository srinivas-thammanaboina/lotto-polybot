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
use crate::domain::market::{ContractKey, MarketId, TokenId};
use crate::domain::order::{ClientOrderId, OrderState, VenueOrderId};
use crate::domain::signal::Side;
use crate::types::{BotEvent, FillEvent, OrderAck, OrderStateChange};

/// Raw user WebSocket message from Polymarket.
#[derive(Debug, Deserialize)]
struct UserMsg {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    order_id: Option<String>,
    client_order_id: Option<String>,
    asset_id: Option<String>,
    market: Option<String>,
    side: Option<String>,
    price: Option<String>,
    size: Option<String>,
    filled_size: Option<String>,
    remaining_size: Option<String>,
    fee: Option<String>,
    reason: Option<String>,
    timestamp: Option<String>,
}

/// Spawn the Polymarket user WebSocket adapter.
/// Receives order acks, fills, cancels, and rejects.
pub fn spawn(
    config: PolymarketConfig,
    event_tx: mpsc::Sender<BotEvent>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut reconnect_attempts: u32 = 0;

        loop {
            if cancel.is_cancelled() {
                break;
            }

            info!(url = %config.user_ws_url, "polymarket-user: connecting");

            match connect_async(&config.user_ws_url).await {
                Ok((ws_stream, _)) => {
                    let (mut write, mut read) = ws_stream.split();

                    // Auth subscription — includes all required credentials.
                    // In production, this should use HMAC signing per Polymarket spec.
                    let auth = serde_json::json!({
                        "type": "auth",
                        "apiKey": config.api_key.as_deref().unwrap_or(""),
                        "secret": config.secret.as_deref().unwrap_or(""),
                        "passphrase": config.passphrase.as_deref().unwrap_or(""),
                    });
                    if let Err(e) = write.send(Message::Text(auth.to_string().into())).await {
                        error!(error = %e, "polymarket-user: auth failed");
                        break;
                    }

                    info!("polymarket-user: connected");
                    reconnect_attempts = 0;

                    loop {
                        tokio::select! {
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        if let Some(event) = parse_user_event(&text)
                                            && event_tx.send(event).await.is_err()
                                        {
                                            debug!("polymarket-user: channel closed");
                                            return;
                                        }
                                    }
                                    Some(Ok(Message::Ping(data))) => {
                                        let _ = write.send(Message::Pong(data)).await;
                                    }
                                    Some(Ok(Message::Close(_))) | None => {
                                        info!("polymarket-user: disconnected");
                                        break;
                                    }
                                    Some(Ok(_)) => {}
                                    Some(Err(e)) => {
                                        warn!(error = %e, "polymarket-user: ws error");
                                        break;
                                    }
                                }
                            }
                            _ = cancel.cancelled() => {
                                info!("polymarket-user: shutdown");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "polymarket-user: connection failed");
                }
            }

            reconnect_attempts += 1;
            let backoff = Duration::from_millis(1000 * u64::from(reconnect_attempts.min(6)));

            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = cancel.cancelled() => return,
            }
        }
    })
}

/// Parse a user WebSocket message into a BotEvent.
fn parse_user_event(text: &str) -> Option<BotEvent> {
    let msg: UserMsg = serde_json::from_str(text).ok()?;
    let msg_type = msg.msg_type.as_deref()?;
    let timestamp = msg
        .timestamp
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(chrono::Utc::now);

    match msg_type {
        "order_ack" | "placement" => {
            let client_order_id = ClientOrderId(msg.client_order_id?);
            let venue_order_id = VenueOrderId(msg.order_id?);
            Some(BotEvent::OrderAck(OrderAck {
                client_order_id,
                venue_order_id,
                timestamp,
            }))
        }
        "fill" | "trade" => {
            let contract = ContractKey {
                market_id: MarketId(msg.market.unwrap_or_default()),
                token_id: TokenId(msg.asset_id?),
            };
            let side = match msg.side.as_deref() {
                Some("BUY" | "buy") => Side::Buy,
                _ => Side::Sell,
            };

            Some(BotEvent::Fill(FillEvent {
                client_order_id: ClientOrderId(msg.client_order_id.unwrap_or_default()),
                venue_order_id: VenueOrderId(msg.order_id.unwrap_or_default()),
                contract,
                side,
                price: Decimal::from_str(msg.price.as_deref()?).ok()?,
                filled_size: Decimal::from_str(msg.filled_size.as_deref().or(msg.size.as_deref())?)
                    .ok()?,
                remaining_size: Decimal::from_str(msg.remaining_size.as_deref().unwrap_or("0"))
                    .unwrap_or_default(),
                fee: Decimal::from_str(msg.fee.as_deref().unwrap_or("0")).unwrap_or_default(),
                timestamp,
            }))
        }
        "canceled" | "rejected" | "expired" => {
            let new_state = match msg_type {
                "canceled" => OrderState::Canceled,
                "rejected" => OrderState::Rejected,
                _ => OrderState::Canceled,
            };
            Some(BotEvent::OrderStateChange(OrderStateChange {
                client_order_id: ClientOrderId(msg.client_order_id.unwrap_or_default()),
                new_state,
                reason: msg.reason,
                timestamp,
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_order_ack() {
        let json = r#"{"type":"order_ack","order_id":"v123","client_order_id":"c456","timestamp":"2024-01-01T00:00:00Z"}"#;
        let event = parse_user_event(json).unwrap();
        match event {
            BotEvent::OrderAck(ack) => {
                assert_eq!(ack.venue_order_id.0, "v123");
                assert_eq!(ack.client_order_id.0, "c456");
            }
            _ => panic!("expected OrderAck"),
        }
    }

    #[test]
    fn parse_fill() {
        let json = r#"{"type":"fill","order_id":"v1","client_order_id":"c1","asset_id":"tok1","side":"BUY","price":"0.55","size":"10","timestamp":"2024-01-01T00:00:00Z"}"#;
        let event = parse_user_event(json).unwrap();
        match event {
            BotEvent::Fill(fill) => {
                assert_eq!(fill.side, Side::Buy);
                assert_eq!(fill.price, Decimal::from_str("0.55").unwrap());
            }
            _ => panic!("expected Fill"),
        }
    }

    #[test]
    fn parse_cancel() {
        let json = r#"{"type":"canceled","client_order_id":"c1","reason":"user","timestamp":"2024-01-01T00:00:00Z"}"#;
        let event = parse_user_event(json).unwrap();
        match event {
            BotEvent::OrderStateChange(change) => {
                assert_eq!(change.new_state, OrderState::Canceled);
            }
            _ => panic!("expected OrderStateChange"),
        }
    }

    #[test]
    fn ignore_unknown_type() {
        let json = r#"{"type":"heartbeat"}"#;
        assert!(parse_user_event(json).is_none());
    }
}
