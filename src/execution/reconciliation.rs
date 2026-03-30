//! Startup and reconnect state reconciliation.
//!
//! On startup, fetches open orders and reconstructs exposure state.
//! Fails closed if account state cannot be trusted. Logs all mismatches.

use std::sync::Arc;

use chrono::Utc;
use rust_decimal::Decimal;
use thiserror::Error;
use tracing::{error, info, warn};

use crate::domain::order::OrderRecord;
use crate::execution::client::{ExchangeClient, OpenOrderInfo};
use crate::execution::fill_state::{ContractExposure, ExposureTracker};
use crate::execution::submit::OrderTracker;

// ---------------------------------------------------------------------------
// Reconciliation errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ReconciliationError {
    #[error("failed to fetch open orders: {0}")]
    FetchFailed(String),

    #[error("failed to fetch account balance: {0}")]
    BalanceFailed(String),

    #[error("unrecognised open orders found: {count} orders")]
    UnrecognisedOrders { count: usize },

    #[error("state inconsistency: {0}")]
    Inconsistency(String),
}

// ---------------------------------------------------------------------------
// Reconciliation result
// ---------------------------------------------------------------------------

/// Result of the reconciliation process.
#[derive(Debug, Clone)]
pub struct ReconciliationResult {
    /// Open orders found on the venue.
    pub open_orders: Vec<OpenOrderInfo>,
    /// Orders that matched local state.
    pub matched_orders: usize,
    /// Orders that were unknown locally (anomalies).
    pub unknown_orders: usize,
    /// Available balance in USDC.
    pub available_usdc: Decimal,
    /// Whether trading is safe to proceed.
    pub safe_to_trade: bool,
    /// Warnings generated during reconciliation.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Reconciler
// ---------------------------------------------------------------------------

/// Performs startup reconciliation to ensure the bot understands existing state.
pub struct Reconciler<C: ExchangeClient> {
    client: Arc<C>,
    order_tracker: Arc<OrderTracker>,
    exposure_tracker: Arc<ExposureTracker>,
}

impl<C: ExchangeClient> Reconciler<C> {
    pub fn new(
        client: Arc<C>,
        order_tracker: Arc<OrderTracker>,
        exposure_tracker: Arc<ExposureTracker>,
    ) -> Self {
        Self {
            client,
            order_tracker,
            exposure_tracker,
        }
    }

    /// Run full startup reconciliation.
    ///
    /// 1. Fetch open orders from venue
    /// 2. Rebuild order tracker state
    /// 3. Rebuild exposure state
    /// 4. Verify account balance
    /// 5. Determine if trading is safe
    pub async fn reconcile(&self) -> Result<ReconciliationResult, ReconciliationError> {
        info!("reconciliation_starting");

        let mut warnings = Vec::new();

        // Step 1: Fetch open orders
        let open_orders = self
            .client
            .open_orders()
            .await
            .map_err(|e| ReconciliationError::FetchFailed(e.to_string()))?;

        info!(
            open_orders = open_orders.len(),
            "reconciliation_orders_fetched"
        );

        // Step 2: Rebuild order tracker from open orders
        let mut matched = 0usize;
        let mut unknown = 0usize;
        let mut order_records = Vec::new();
        let mut exposures = Vec::new();

        for order_info in &open_orders {
            // Check if we know about this order
            let known = self.order_tracker.get(&order_info.client_order_id);

            match known {
                Some(existing) => {
                    // Known order — verify state is consistent
                    if existing.state != order_info.state {
                        warn!(
                            client_order_id = %order_info.client_order_id,
                            local_state = %existing.state,
                            venue_state = %order_info.state,
                            "reconciliation_state_mismatch"
                        );
                        warnings.push(format!(
                            "order {} state mismatch: local={} venue={}",
                            order_info.client_order_id, existing.state, order_info.state
                        ));
                    }
                    matched += 1;
                }
                None => {
                    // Unknown order — could be from a previous session
                    warn!(
                        client_order_id = %order_info.client_order_id,
                        venue_order_id = %order_info.venue_order_id.0,
                        contract = %order_info.contract,
                        "reconciliation_unknown_order"
                    );
                    warnings.push(format!(
                        "unknown order on venue: {} (contract: {})",
                        order_info.client_order_id, order_info.contract
                    ));
                    unknown += 1;
                }
            }

            // Build order record for tracker
            let filled_size = order_info.original_size - order_info.remaining_size;
            order_records.push(OrderRecord {
                client_order_id: order_info.client_order_id.clone(),
                venue_order_id: Some(order_info.venue_order_id.clone()),
                contract: order_info.contract.clone(),
                side: order_info.side,
                price: order_info.price,
                size: order_info.original_size,
                filled_size,
                avg_fill_price: if filled_size > Decimal::ZERO {
                    Some(order_info.price)
                } else {
                    None
                },
                state: order_info.state,
                created_at: order_info.created_at,
                updated_at: Utc::now(),
                retry_count: 0,
            });

            // Build exposure from open orders
            let pending_notional = order_info.remaining_size * order_info.price;
            let filled_notional = filled_size * order_info.price;
            if pending_notional > Decimal::ZERO || filled_notional > Decimal::ZERO {
                exposures.push(ContractExposure {
                    contract: order_info.contract.clone(),
                    side: order_info.side,
                    pending_notional,
                    filled_notional,
                    fees_paid: Decimal::ZERO, // Fees not available from open order query
                    avg_fill_price: order_info.price,
                    updated_at: Utc::now(),
                });
            }
        }

        // Load reconciled state
        self.order_tracker.load_from_reconciliation(order_records);
        self.exposure_tracker.load_from_reconciliation(exposures);

        // Step 3: Verify account balance
        let balance = self
            .client
            .account_balance()
            .await
            .map_err(|e| ReconciliationError::BalanceFailed(e.to_string()))?;

        info!(
            available = %balance.available_usdc,
            locked = %balance.locked_usdc,
            "reconciliation_balance"
        );

        if balance.available_usdc <= Decimal::ZERO {
            warnings.push("zero available balance".to_string());
        }

        // Step 4: Determine safety
        // Trading is safe if there are no unknown orders.
        // Unknown orders are logged but don't necessarily block trading
        // (operator can review). We block only if count is high enough
        // to suggest state corruption.
        let safe_to_trade = unknown == 0;

        if !safe_to_trade {
            error!(
                unknown_orders = unknown,
                "reconciliation_unsafe — unknown orders found, trading blocked"
            );
        }

        let result = ReconciliationResult {
            open_orders,
            matched_orders: matched,
            unknown_orders: unknown,
            available_usdc: balance.available_usdc,
            safe_to_trade,
            warnings,
        };

        info!(
            matched = result.matched_orders,
            unknown = result.unknown_orders,
            safe = result.safe_to_trade,
            balance = %result.available_usdc,
            "reconciliation_complete"
        );

        Ok(result)
    }

    /// Mark all uncertain orders for operator review.
    /// Returns the count of uncertain orders found.
    pub fn resolve_uncertain(&self) -> usize {
        let uncertain = self.order_tracker.uncertain_orders();
        let count = uncertain.len();
        if count > 0 {
            warn!(count = count, "reconciliation_uncertain_orders");
            for order in &uncertain {
                warn!(
                    client_order_id = %order.client_order_id,
                    contract = %order.contract,
                    "uncertain_order_needs_review"
                );
            }
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{MarketId, TokenId};
    use crate::domain::order::{ClientOrderId, OrderState};
    use crate::domain::signal::Side;
    use crate::execution::client::SimulationClient;
    use rust_decimal_macros::dec;

    #[tokio::test]
    async fn reconcile_empty_state() {
        let client = Arc::new(SimulationClient::default());
        let tracker = Arc::new(OrderTracker::new());
        let exposure = Arc::new(ExposureTracker::new());
        let reconciler = Reconciler::new(client, tracker, exposure);

        let result = reconciler.reconcile().await.unwrap();
        assert!(result.safe_to_trade);
        assert!(result.open_orders.is_empty());
        assert_eq!(result.unknown_orders, 0);
        assert!(result.available_usdc > Decimal::ZERO);
    }

    #[tokio::test]
    async fn reconcile_detects_balance() {
        let client = Arc::new(SimulationClient::default());
        let tracker = Arc::new(OrderTracker::new());
        let exposure = Arc::new(ExposureTracker::new());
        let reconciler = Reconciler::new(client, tracker, exposure);

        let result = reconciler.reconcile().await.unwrap();
        assert_eq!(result.available_usdc, dec!(10000));
    }

    #[test]
    fn resolve_uncertain_empty() {
        let client = Arc::new(SimulationClient::default());
        let tracker = Arc::new(OrderTracker::new());
        let exposure = Arc::new(ExposureTracker::new());
        let reconciler = Reconciler::new(client, tracker, exposure);

        assert_eq!(reconciler.resolve_uncertain(), 0);
    }

    #[test]
    fn resolve_uncertain_finds_orders() {
        let client = Arc::new(SimulationClient::default());
        let tracker = Arc::new(OrderTracker::new());
        let exposure = Arc::new(ExposureTracker::new());
        let reconciler = Reconciler::new(client, Arc::clone(&tracker), exposure);

        // Add an order and mark it uncertain
        let coid = ClientOrderId::new();
        tracker.register(OrderRecord {
            client_order_id: coid.clone(),
            venue_order_id: None,
            contract: crate::domain::market::ContractKey {
                market_id: MarketId("m".into()),
                token_id: TokenId("t".into()),
            },
            side: Side::Buy,
            price: dec!(0.5),
            size: dec!(10),
            filled_size: Decimal::ZERO,
            avg_fill_price: None,
            state: OrderState::Pending,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            retry_count: 0,
        });
        tracker.mark_uncertain(&coid);

        assert_eq!(reconciler.resolve_uncertain(), 1);
    }
}
