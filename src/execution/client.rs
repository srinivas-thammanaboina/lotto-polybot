//! Polymarket client abstraction layer.
//!
//! The execution layer depends on the `ExchangeClient` trait, not a concrete
//! implementation. This allows benchmarking, swapping SDK versions, or using
//! a simulation client without changing strategy/execution logic.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::market::ContractKey;
use crate::domain::order::{ClientOrderId, OrderState, VenueOrderId};
use crate::domain::signal::Side;

// ---------------------------------------------------------------------------
// Client errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("order submission failed: {0}")]
    Submit(String),

    #[error("cancellation failed: {0}")]
    Cancel(String),

    #[error("query failed: {0}")]
    Query(String),

    #[error("rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    #[error("network error: {0}")]
    Network(String),

    #[error("unknown/unexpected error: {0}")]
    Unknown(String),
}

// ---------------------------------------------------------------------------
// Order request / response types
// ---------------------------------------------------------------------------

/// Order type on Polymarket CLOB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    /// Good-til-cancelled limit order.
    GoodTilCancel,
    /// Fill-or-kill.
    FillOrKill,
    /// Immediate-or-cancel (partial fill allowed).
    ImmediateOrCancel,
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderType::GoodTilCancel => write!(f, "GTC"),
            OrderType::FillOrKill => write!(f, "FOK"),
            OrderType::ImmediateOrCancel => write!(f, "IOC"),
        }
    }
}

/// Request to submit a new order.
#[derive(Debug, Clone)]
pub struct SubmitOrderRequest {
    pub client_order_id: ClientOrderId,
    pub contract: ContractKey,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub order_type: OrderType,
}

/// Response from a successful order submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitOrderResponse {
    pub client_order_id: ClientOrderId,
    pub venue_order_id: VenueOrderId,
    pub status: OrderState,
    pub timestamp: DateTime<Utc>,
}

/// Request to cancel an order.
#[derive(Debug, Clone)]
pub struct CancelOrderRequest {
    pub client_order_id: ClientOrderId,
    pub venue_order_id: Option<VenueOrderId>,
}

/// Response from a cancellation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelOrderResponse {
    pub client_order_id: ClientOrderId,
    pub cancelled: bool,
    pub timestamp: DateTime<Utc>,
}

/// Open order query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenOrderInfo {
    pub client_order_id: ClientOrderId,
    pub venue_order_id: VenueOrderId,
    pub contract: ContractKey,
    pub side: Side,
    pub price: Decimal,
    pub original_size: Decimal,
    pub remaining_size: Decimal,
    pub state: OrderState,
    pub created_at: DateTime<Utc>,
}

/// Account balance info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountBalance {
    pub available_usdc: Decimal,
    pub locked_usdc: Decimal,
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Client trait
// ---------------------------------------------------------------------------

/// Abstraction over the Polymarket CLOB client.
///
/// Implementations include:
/// - `SimulationClient` — for backtesting/simulation (returns synthetic fills)
/// - Future: `SdkClient` — wrapping the official Polymarket Rust SDK
#[async_trait]
pub trait ExchangeClient: Send + Sync {
    /// Submit a new order.
    async fn submit_order(
        &self,
        req: SubmitOrderRequest,
    ) -> Result<SubmitOrderResponse, ClientError>;

    /// Cancel an existing order.
    async fn cancel_order(
        &self,
        req: CancelOrderRequest,
    ) -> Result<CancelOrderResponse, ClientError>;

    /// Query open orders.
    async fn open_orders(&self) -> Result<Vec<OpenOrderInfo>, ClientError>;

    /// Query account balance.
    async fn account_balance(&self) -> Result<AccountBalance, ClientError>;

    /// Check if the client is connected and authenticated.
    async fn is_healthy(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Simulation client — concrete implementation for non-live modes
// ---------------------------------------------------------------------------

/// Simulation client that returns synthetic responses.
/// Used in simulation/paper/dry-run modes.
pub struct SimulationClient {
    fill_probability: f64,
}

impl SimulationClient {
    pub fn new(fill_probability: f64) -> Self {
        Self {
            fill_probability: fill_probability.clamp(0.0, 1.0),
        }
    }
}

impl Default for SimulationClient {
    fn default() -> Self {
        Self::new(1.0) // 100% fill rate in simulation
    }
}

#[async_trait]
impl ExchangeClient for SimulationClient {
    async fn submit_order(
        &self,
        req: SubmitOrderRequest,
    ) -> Result<SubmitOrderResponse, ClientError> {
        let now = Utc::now();
        // In simulation, orders are immediately acked
        // Fill probability determines if we get Filled vs Pending
        let status = if self.fill_probability >= 1.0 {
            OrderState::Filled
        } else {
            OrderState::Acked
        };

        Ok(SubmitOrderResponse {
            client_order_id: req.client_order_id,
            venue_order_id: VenueOrderId(format!("sim-{}", uuid::Uuid::new_v4())),
            status,
            timestamp: now,
        })
    }

    async fn cancel_order(
        &self,
        req: CancelOrderRequest,
    ) -> Result<CancelOrderResponse, ClientError> {
        Ok(CancelOrderResponse {
            client_order_id: req.client_order_id,
            cancelled: true,
            timestamp: Utc::now(),
        })
    }

    async fn open_orders(&self) -> Result<Vec<OpenOrderInfo>, ClientError> {
        // Simulation has no persistent orders
        Ok(Vec::new())
    }

    async fn account_balance(&self) -> Result<AccountBalance, ClientError> {
        Ok(AccountBalance {
            available_usdc: Decimal::from(10000),
            locked_usdc: Decimal::ZERO,
            timestamp: Utc::now(),
        })
    }

    async fn is_healthy(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{MarketId, TokenId};

    fn test_contract() -> ContractKey {
        ContractKey {
            market_id: MarketId("mkt-1".into()),
            token_id: TokenId("tok-1".into()),
        }
    }

    fn test_submit_req() -> SubmitOrderRequest {
        SubmitOrderRequest {
            client_order_id: ClientOrderId::new(),
            contract: test_contract(),
            side: Side::Buy,
            price: Decimal::from(55) / Decimal::from(100),
            size: Decimal::from(10),
            order_type: OrderType::GoodTilCancel,
        }
    }

    #[tokio::test]
    async fn simulation_submit_returns_ack() {
        let client = SimulationClient::default();
        let resp = client.submit_order(test_submit_req()).await.unwrap();
        assert_eq!(resp.status, OrderState::Filled);
        assert!(resp.venue_order_id.0.starts_with("sim-"));
    }

    #[tokio::test]
    async fn simulation_cancel_succeeds() {
        let client = SimulationClient::default();
        let resp = client
            .cancel_order(CancelOrderRequest {
                client_order_id: ClientOrderId::new(),
                venue_order_id: None,
            })
            .await
            .unwrap();
        assert!(resp.cancelled);
    }

    #[tokio::test]
    async fn simulation_open_orders_empty() {
        let client = SimulationClient::default();
        let orders = client.open_orders().await.unwrap();
        assert!(orders.is_empty());
    }

    #[tokio::test]
    async fn simulation_balance_available() {
        let client = SimulationClient::default();
        let bal = client.account_balance().await.unwrap();
        assert!(bal.available_usdc > Decimal::ZERO);
    }

    #[tokio::test]
    async fn simulation_is_healthy() {
        let client = SimulationClient::default();
        assert!(client.is_healthy().await);
    }

    #[tokio::test]
    async fn partial_fill_probability() {
        let client = SimulationClient::new(0.5);
        let resp = client.submit_order(test_submit_req()).await.unwrap();
        // With fill_probability < 1.0, status should be Acked (not immediately filled)
        assert_eq!(resp.status, OrderState::Acked);
    }
}
