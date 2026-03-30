use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::domain::market::{Asset, BookSnapshot, ContractKey, MarketId, TokenId};
use crate::domain::order::{ClientOrderId, OrderState, VenueOrderId};
use crate::domain::signal::{OrderIntent, RejectReason, Side};

/// Timestamp captured immediately on receipt of an inbound message.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ReceiptTimestamp(pub DateTime<Utc>);

impl ReceiptTimestamp {
    pub fn now() -> Self {
        Self(Utc::now())
    }
}

// ---------------------------------------------------------------------------
// Feed events — normalized from transport-specific payloads
// ---------------------------------------------------------------------------

/// External CEX trade tick (Binance / Coinbase).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CexTick {
    pub source: FeedSource,
    pub asset: Asset,
    pub price: Decimal,
    pub quantity: Decimal,
    pub source_timestamp: DateTime<Utc>,
    pub receipt_timestamp: ReceiptTimestamp,
}

/// Which external feed produced the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeedSource {
    Binance,
    Coinbase,
    PolymarketRtds,
}

impl std::fmt::Display for FeedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedSource::Binance => write!(f, "binance"),
            FeedSource::Coinbase => write!(f, "coinbase"),
            FeedSource::PolymarketRtds => write!(f, "rtds"),
        }
    }
}

/// Polymarket order book update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookUpdate {
    pub token_id: TokenId,
    pub snapshot: BookSnapshot,
    pub receipt_timestamp: ReceiptTimestamp,
}

/// Polymarket RTDS reference price update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RtdsUpdate {
    pub asset: Asset,
    pub price: Decimal,
    pub source_timestamp: DateTime<Utc>,
    pub receipt_timestamp: ReceiptTimestamp,
}

// ---------------------------------------------------------------------------
// Execution events — order lifecycle from venue
// ---------------------------------------------------------------------------

/// Order acknowledgment from venue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderAck {
    pub client_order_id: ClientOrderId,
    pub venue_order_id: VenueOrderId,
    pub timestamp: DateTime<Utc>,
}

/// Fill event (partial or full).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillEvent {
    pub client_order_id: ClientOrderId,
    pub venue_order_id: VenueOrderId,
    pub contract: ContractKey,
    pub side: Side,
    pub price: Decimal,
    pub filled_size: Decimal,
    pub remaining_size: Decimal,
    pub fee: Decimal,
    pub timestamp: DateTime<Utc>,
}

/// Order state change from venue (cancel, reject, etc).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderStateChange {
    pub client_order_id: ClientOrderId,
    pub new_state: OrderState,
    pub reason: Option<String>,
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Risk events
// ---------------------------------------------------------------------------

/// Kill switch activation event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KillSwitchEvent {
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Resolution events
// ---------------------------------------------------------------------------

/// Market resolution event from Polymarket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionEvent {
    pub market_id: MarketId,
    pub winning_token: Option<TokenId>,
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Unified internal event envelope
// ---------------------------------------------------------------------------

/// All internal events flow through this enum.
/// This is the contract between producers (feeds, execution) and consumers
/// (strategy, risk, telemetry, replay).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BotEvent {
    // Feed events
    CexTick(CexTick),
    BookUpdate(BookUpdate),
    RtdsUpdate(RtdsUpdate),

    // Signal events
    SignalAccepted(OrderIntent),
    SignalRejected {
        contract: ContractKey,
        reasons: Vec<RejectReason>,
        timestamp: DateTime<Utc>,
    },

    // Execution events
    OrderAck(OrderAck),
    Fill(FillEvent),
    OrderStateChange(OrderStateChange),

    // Risk events
    KillSwitch(KillSwitchEvent),

    // Resolution events
    Resolution(ResolutionEvent),
}

/// Event durability class.
/// Critical events must not be silently dropped by persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDurability {
    /// Must persist — audit-critical (fills, kill switch, resolutions, order lifecycle).
    Critical,
    /// Best-effort — can be dropped under backpressure (ticks, book updates).
    BestEffort,
}

impl BotEvent {
    /// Returns a short label for logging/metrics.
    pub fn label(&self) -> &'static str {
        match self {
            BotEvent::CexTick(_) => "cex_tick",
            BotEvent::BookUpdate(_) => "book_update",
            BotEvent::RtdsUpdate(_) => "rtds_update",
            BotEvent::SignalAccepted(_) => "signal_accepted",
            BotEvent::SignalRejected { .. } => "signal_rejected",
            BotEvent::OrderAck(_) => "order_ack",
            BotEvent::Fill(_) => "fill",
            BotEvent::OrderStateChange(_) => "order_state_change",
            BotEvent::KillSwitch(_) => "kill_switch",
            BotEvent::Resolution(_) => "resolution",
        }
    }

    /// Event durability class — critical events must not be silently dropped.
    pub fn durability(&self) -> EventDurability {
        match self {
            BotEvent::CexTick(_) | BotEvent::BookUpdate(_) | BotEvent::RtdsUpdate(_) => {
                EventDurability::BestEffort
            }
            _ => EventDurability::Critical,
        }
    }
}
