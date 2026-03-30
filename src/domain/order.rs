use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::market::ContractKey;
use super::signal::Side;

/// Unique client-side order ID for idempotency.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientOrderId(pub String);

impl ClientOrderId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for ClientOrderId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ClientOrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Venue-assigned order ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VenueOrderId(pub String);

/// Order lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderState {
    Pending,
    Acked,
    PartialFill,
    Filled,
    CancelPending,
    Canceled,
    Rejected,
    Retrying,
    Uncertain,
}

impl std::fmt::Display for OrderState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            OrderState::Pending => "pending",
            OrderState::Acked => "acked",
            OrderState::PartialFill => "partial_fill",
            OrderState::Filled => "filled",
            OrderState::CancelPending => "cancel_pending",
            OrderState::Canceled => "canceled",
            OrderState::Rejected => "rejected",
            OrderState::Retrying => "retrying",
            OrderState::Uncertain => "uncertain",
        };
        write!(f, "{s}")
    }
}

/// Full order record tracked by the execution engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRecord {
    pub client_order_id: ClientOrderId,
    pub venue_order_id: Option<VenueOrderId>,
    pub contract: ContractKey,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub filled_size: Decimal,
    pub avg_fill_price: Option<Decimal>,
    pub state: OrderState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub retry_count: u32,
}
