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

use crate::config::BinanceConfig;
use crate::feeds::health::{ConnectionState, FeedHealthMonitor};
use crate::feeds::normalization::parse_asset;
use crate::types::{BotEvent, CexTick, FeedSource, ReceiptTimestamp};

/// Raw Binance trade stream message.
#[derive(Debug, Deserialize)]
struct BinanceTrade {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "q")]
    quantity: String,
    #[serde(rename = "T")]
    trade_time: i64,
}

/// Spawn the Binance WebSocket adapter task.
/// Connects to combined stream for BTC and ETH trade feeds.
pub fn spawn(
    config: BinanceConfig,
    event_tx: mpsc::Sender<BotEvent>,
    health: FeedHealthMonitor,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let streams = "btcusdt@trade/ethusdt@trade";
        let url = format!("{}/ws/{}", config.ws_url, streams);
        let mut reconnect_attempts: u32 = 0;

        loop {
            if cancel.is_cancelled() {
                break;
            }

            health.set_state(FeedSource::Binance, ConnectionState::Connecting);
            info!(url = %url, "binance: connecting");

            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    info!("binance: connected");
                    health.set_state(FeedSource::Binance, ConnectionState::Connected);
                    reconnect_attempts = 0;

                    let (mut _write, mut read) = ws_stream.split();

                    loop {
                        tokio::select! {
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        let receipt = ReceiptTimestamp::now();
                                        health.record_message(FeedSource::Binance);

                                        match parse_trade(&text, receipt) {
                                            Some(event) => {
                                                if event_tx.send(event).await.is_err() {
                                                    debug!("binance: event channel closed");
                                                    return;
                                                }
                                            }
                                            None => {
                                                health.record_parse_error(FeedSource::Binance);
                                            }
                                        }
                                    }
                                    Some(Ok(Message::Ping(data))) => {
                                        if _write.send(Message::Pong(data)).await.is_err() {
                                            warn!("binance: failed to send pong");
                                            break;
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) => {
                                        info!("binance: server closed connection");
                                        break;
                                    }
                                    Some(Ok(_)) => {} // ignore binary, pong, etc.
                                    Some(Err(e)) => {
                                        warn!(error = %e, "binance: ws error");
                                        break;
                                    }
                                    None => {
                                        info!("binance: stream ended");
                                        break;
                                    }
                                }
                            }
                            _ = cancel.cancelled() => {
                                info!("binance: shutdown requested");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "binance: connection failed");
                }
            }

            // Reconnect logic
            reconnect_attempts += 1;
            if reconnect_attempts > config.max_reconnect_attempts {
                error!(
                    attempts = reconnect_attempts,
                    "binance: max reconnect attempts reached"
                );
                break;
            }

            health.set_state(FeedSource::Binance, ConnectionState::Reconnecting);
            let backoff = Duration::from_millis(
                config.reconnect_backoff_ms * u64::from(reconnect_attempts.min(6)),
            );
            info!(
                attempt = reconnect_attempts,
                backoff_ms = backoff.as_millis() as u64,
                "binance: reconnecting"
            );

            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = cancel.cancelled() => {
                    info!("binance: shutdown during reconnect backoff");
                    return;
                }
            }
        }

        health.set_state(FeedSource::Binance, ConnectionState::Disconnected);
    })
}

/// Parse a raw Binance trade message into a BotEvent.
fn parse_trade(text: &str, receipt: ReceiptTimestamp) -> Option<BotEvent> {
    let trade: BinanceTrade = serde_json::from_str(text).ok()?;
    let asset = parse_asset(&trade.symbol)?;
    let price = Decimal::from_str(&trade.price).ok()?;
    let quantity = Decimal::from_str(&trade.quantity).ok()?;
    let source_timestamp = DateTime::<Utc>::from_timestamp_millis(trade.trade_time)?;

    Some(BotEvent::CexTick(CexTick {
        source: FeedSource::Binance,
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
    fn parse_valid_btc_trade() {
        let json = r#"{"e":"trade","E":1234567890123,"s":"BTCUSDT","t":123,"p":"67500.50","q":"0.001","T":1234567890100,"m":false}"#;
        let receipt = ReceiptTimestamp::now();
        let event = parse_trade(json, receipt).unwrap();

        match event {
            BotEvent::CexTick(tick) => {
                assert_eq!(tick.asset, Asset::BTC);
                assert_eq!(tick.source, FeedSource::Binance);
                assert_eq!(tick.price, Decimal::from_str("67500.50").unwrap());
            }
            _ => panic!("expected CexTick"),
        }
    }

    #[test]
    fn parse_valid_eth_trade() {
        let json = r#"{"s":"ETHUSDT","p":"3200.00","q":"0.5","T":1234567890100}"#;
        let receipt = ReceiptTimestamp::now();
        let event = parse_trade(json, receipt).unwrap();

        match event {
            BotEvent::CexTick(tick) => {
                assert_eq!(tick.asset, Asset::ETH);
            }
            _ => panic!("expected CexTick"),
        }
    }

    #[test]
    fn reject_unsupported_symbol() {
        let json = r#"{"s":"DOGEUSDT","p":"0.15","q":"100","T":1234567890100}"#;
        assert!(parse_trade(json, ReceiptTimestamp::now()).is_none());
    }

    #[test]
    fn reject_invalid_json() {
        assert!(parse_trade("not json", ReceiptTimestamp::now()).is_none());
    }
}
