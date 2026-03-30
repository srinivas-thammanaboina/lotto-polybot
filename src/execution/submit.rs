//! Order submission with bounded retry logic and idempotency.
//!
//! Takes `OrderIntent` from the signal pipeline, constructs orders,
//! submits through the client abstraction, and manages initial order state.
//! Never blindly resubmits after uncertain order state.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use parking_lot::RwLock;
use rust_decimal::Decimal;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::config::ExecutionConfig;
use crate::domain::market::ContractKey;
use crate::domain::order::{ClientOrderId, OrderRecord, OrderState, VenueOrderId};
use crate::domain::signal::OrderIntent;
use crate::execution::client::{
    ClientError, ExchangeClient, OrderType, SubmitOrderRequest, SubmitOrderResponse,
};
use crate::types::BotEvent;

// ---------------------------------------------------------------------------
// Order tracker — tracks all orders by client ID for dedup and state
// ---------------------------------------------------------------------------

/// Tracks in-flight and completed orders for idempotency and state management.
#[derive(Debug)]
pub struct OrderTracker {
    orders: Arc<RwLock<HashMap<String, OrderRecord>>>,
    /// Tracks which contracts have pending/open orders (for dedup).
    pending_contracts: Arc<RwLock<HashMap<String, ClientOrderId>>>,
}

impl OrderTracker {
    pub fn new() -> Self {
        Self {
            orders: Arc::new(RwLock::new(HashMap::new())),
            pending_contracts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new order before submission.
    pub fn register(&self, record: OrderRecord) {
        let coid = record.client_order_id.0.clone();
        let contract_key = record.contract.to_string();
        self.orders.write().insert(coid.clone(), record);
        self.pending_contracts
            .write()
            .insert(contract_key, ClientOrderId(coid));
    }

    /// Check if there's already a pending order for this contract.
    pub fn has_pending_order(&self, contract: &ContractKey) -> bool {
        let key = contract.to_string();
        let pending = self.pending_contracts.read();
        if let Some(coid) = pending.get(&key) {
            let orders = self.orders.read();
            if let Some(order) = orders.get(&coid.0) {
                return matches!(
                    order.state,
                    OrderState::Pending | OrderState::Acked | OrderState::PartialFill
                );
            }
        }
        false
    }

    /// Update order state.
    pub fn update_state(
        &self,
        client_order_id: &ClientOrderId,
        new_state: OrderState,
        venue_order_id: Option<VenueOrderId>,
    ) {
        let mut orders = self.orders.write();
        if let Some(order) = orders.get_mut(&client_order_id.0) {
            order.state = new_state;
            order.updated_at = Utc::now();
            if let Some(vid) = venue_order_id {
                order.venue_order_id = Some(vid);
            }

            // Clean up pending contracts on terminal states
            if matches!(
                new_state,
                OrderState::Filled | OrderState::Canceled | OrderState::Rejected
            ) {
                let contract_key = order.contract.to_string();
                drop(orders);
                self.pending_contracts.write().remove(&contract_key);
            }
        }
    }

    /// Update fill info on an order.
    pub fn apply_fill(
        &self,
        client_order_id: &ClientOrderId,
        filled_size: Decimal,
        avg_price: Decimal,
        new_state: OrderState,
    ) {
        let mut orders = self.orders.write();
        if let Some(order) = orders.get_mut(&client_order_id.0) {
            order.filled_size = filled_size;
            order.avg_fill_price = Some(avg_price);
            order.state = new_state;
            order.updated_at = Utc::now();

            if matches!(new_state, OrderState::Filled) {
                let contract_key = order.contract.to_string();
                drop(orders);
                self.pending_contracts.write().remove(&contract_key);
            }
        }
    }

    /// Get a snapshot of an order.
    pub fn get(&self, client_order_id: &ClientOrderId) -> Option<OrderRecord> {
        self.orders.read().get(&client_order_id.0).cloned()
    }

    /// Get all orders in a given state.
    pub fn orders_in_state(&self, state: OrderState) -> Vec<OrderRecord> {
        self.orders
            .read()
            .values()
            .filter(|o| o.state == state)
            .cloned()
            .collect()
    }

    /// Count of orders in active (non-terminal) states.
    pub fn active_count(&self) -> usize {
        self.orders
            .read()
            .values()
            .filter(|o| {
                matches!(
                    o.state,
                    OrderState::Pending
                        | OrderState::Acked
                        | OrderState::PartialFill
                        | OrderState::Retrying
                )
            })
            .count()
    }

    /// Mark an order as uncertain (for reconciliation).
    pub fn mark_uncertain(&self, client_order_id: &ClientOrderId) {
        self.update_state(client_order_id, OrderState::Uncertain, None);
    }

    /// Get all uncertain orders.
    pub fn uncertain_orders(&self) -> Vec<OrderRecord> {
        self.orders_in_state(OrderState::Uncertain)
    }

    /// Load orders from reconciliation (startup recovery).
    pub fn load_from_reconciliation(&self, records: Vec<OrderRecord>) {
        let mut orders = self.orders.write();
        let mut pending = self.pending_contracts.write();
        for record in records {
            let coid = record.client_order_id.0.clone();
            let contract_key = record.contract.to_string();
            if matches!(
                record.state,
                OrderState::Pending | OrderState::Acked | OrderState::PartialFill
            ) {
                pending.insert(contract_key, ClientOrderId(coid.clone()));
            }
            orders.insert(coid, record);
        }
    }
}

impl Default for OrderTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Execution engine
// ---------------------------------------------------------------------------

/// The execution engine: takes OrderIntents, submits through the client,
/// and manages order lifecycle state.
pub struct ExecutionEngine<C: ExchangeClient + ?Sized> {
    client: Arc<C>,
    tracker: Arc<OrderTracker>,
    config: ExecutionConfig,
    event_tx: mpsc::Sender<BotEvent>,
}

impl<C: ExchangeClient + ?Sized> ExecutionEngine<C> {
    pub fn new(
        client: Arc<C>,
        tracker: Arc<OrderTracker>,
        config: ExecutionConfig,
        event_tx: mpsc::Sender<BotEvent>,
    ) -> Self {
        Self {
            client,
            tracker,
            config,
            event_tx,
        }
    }

    /// Process an order intent: validate, construct, submit with retry.
    pub async fn submit_intent(&self, intent: &OrderIntent) -> Result<ClientOrderId, SubmitError> {
        // Check 1: Duplicate prevention
        if self.tracker.has_pending_order(&intent.contract) {
            warn!(
                contract = %intent.contract,
                "duplicate_order_blocked"
            );
            return Err(SubmitError::DuplicateOrder);
        }

        // Check 2: Concurrent order limit
        if self.tracker.active_count() >= self.config.max_concurrent_orders as usize {
            warn!(
                active = self.tracker.active_count(),
                max = self.config.max_concurrent_orders,
                "concurrent_order_limit"
            );
            return Err(SubmitError::ConcurrentLimit);
        }

        // Check 3: Signal freshness
        let signal_age = Utc::now() - intent.signal_timestamp;
        if signal_age
            > chrono::Duration::from_std(self.config.stale_signal_threshold)
                .unwrap_or(chrono::Duration::seconds(1))
        {
            warn!(
                contract = %intent.contract,
                age_ms = signal_age.num_milliseconds(),
                "stale_signal_rejected"
            );
            return Err(SubmitError::StaleSignal);
        }

        // Construct order
        let client_order_id = ClientOrderId::new();
        let now = Utc::now();

        let record = OrderRecord {
            client_order_id: client_order_id.clone(),
            venue_order_id: None,
            contract: intent.contract.clone(),
            side: intent.side,
            price: intent.target_price,
            size: intent.size,
            filled_size: Decimal::ZERO,
            avg_fill_price: None,
            state: OrderState::Pending,
            created_at: now,
            updated_at: now,
            retry_count: 0,
        };

        // Register before submission (fail-safe: if we crash, we know we tried)
        self.tracker.register(record);

        // Submit with bounded retry
        let req = SubmitOrderRequest {
            client_order_id: client_order_id.clone(),
            contract: intent.contract.clone(),
            side: intent.side,
            price: intent.target_price,
            size: intent.size,
            order_type: OrderType::GoodTilCancel,
        };

        let result = self.submit_with_retry(req).await;

        match result {
            Ok(resp) => {
                self.tracker.update_state(
                    &client_order_id,
                    resp.status,
                    Some(resp.venue_order_id.clone()),
                );

                info!(
                    client_order_id = %client_order_id,
                    venue_order_id = %resp.venue_order_id.0,
                    contract = %intent.contract,
                    side = %intent.side,
                    price = %intent.target_price,
                    size = %intent.size,
                    "order_submitted"
                );

                // Emit order ack event
                let _ = self
                    .event_tx
                    .send(BotEvent::OrderAck(crate::types::OrderAck {
                        client_order_id: client_order_id.clone(),
                        venue_order_id: resp.venue_order_id,
                        timestamp: resp.timestamp,
                    }))
                    .await;

                Ok(client_order_id)
            }
            Err(e) => {
                // Mark as uncertain — do NOT mark as rejected because we
                // don't know if the venue received it.
                self.tracker.mark_uncertain(&client_order_id);

                error!(
                    client_order_id = %client_order_id,
                    contract = %intent.contract,
                    error = %e,
                    "order_submit_failed_uncertain"
                );

                Err(SubmitError::ClientError(e.to_string()))
            }
        }
    }

    /// Submit with bounded retry. Only retries on retriable errors.
    async fn submit_with_retry(
        &self,
        req: SubmitOrderRequest,
    ) -> Result<SubmitOrderResponse, ClientError> {
        let mut attempts = 0u32;
        let max_attempts = self.config.max_retry_attempts + 1; // 1 initial + retries

        loop {
            attempts += 1;
            match self.client.submit_order(req.clone()).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let is_retriable =
                        matches!(e, ClientError::RateLimited { .. } | ClientError::Network(_));

                    if !is_retriable || attempts >= max_attempts {
                        return Err(e);
                    }

                    let backoff =
                        Duration::from_millis(self.config.retry_backoff_ms * u64::from(attempts));
                    warn!(
                        attempt = attempts,
                        max = max_attempts,
                        backoff_ms = backoff.as_millis() as u64,
                        error = %e,
                        "order_submit_retry"
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    /// Get the order tracker for external access (fill state, reconciliation).
    pub fn tracker(&self) -> &Arc<OrderTracker> {
        &self.tracker
    }

    /// Check if the execution engine is healthy.
    pub async fn is_healthy(&self) -> bool {
        self.client.is_healthy().await
    }
}

// ---------------------------------------------------------------------------
// Submit errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum SubmitError {
    #[error("duplicate order for contract")]
    DuplicateOrder,
    #[error("concurrent order limit reached")]
    ConcurrentLimit,
    #[error("signal too stale")]
    StaleSignal,
    #[error("client error: {0}")]
    ClientError(String),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{Asset, MarketDuration, MarketId, TokenId};
    use crate::domain::signal::Side;
    use crate::execution::client::SimulationClient;
    use crate::strategy::edge::CostSnapshot;
    use rust_decimal_macros::dec;

    fn test_intent() -> OrderIntent {
        OrderIntent {
            contract: ContractKey {
                market_id: MarketId("mkt1".into()),
                token_id: TokenId("tok1".into()),
            },
            asset: Asset::BTC,
            duration: MarketDuration::FiveMin,
            side: Side::Buy,
            target_price: dec!(0.55),
            size: dec!(10),
            fair_value: dec!(0.65),
            gross_edge: dec!(0.10),
            net_edge: dec!(0.05),
            cost_snapshot: CostSnapshot {
                fee_rate: dec!(0.01),
                entry_fee_usdc: dec!(0.10),
                exit_fee_usdc: dec!(0.10),
                entry_slippage: dec!(0.005),
                exit_slippage: dec!(0.0075),
                latency_decay: dec!(0.0025),
                total_cost_frac: dec!(0.05),
            },
            rationale: "test signal".to_string(),
            model_version: "fv-v1.0".to_string(),
            signal_timestamp: Utc::now(),
        }
    }

    fn test_config() -> ExecutionConfig {
        ExecutionConfig {
            max_retry_attempts: 2,
            retry_backoff_ms: 10,
            stale_signal_threshold: Duration::from_secs(5),
            max_concurrent_orders: 2,
        }
    }

    fn make_engine() -> (
        ExecutionEngine<SimulationClient>,
        mpsc::Receiver<BotEvent>,
        Arc<OrderTracker>,
    ) {
        let client = Arc::new(SimulationClient::default());
        let tracker = Arc::new(OrderTracker::new());
        let (event_tx, event_rx) = mpsc::channel(64);
        let engine = ExecutionEngine::new(client, Arc::clone(&tracker), test_config(), event_tx);
        (engine, event_rx, tracker)
    }

    #[tokio::test]
    async fn submit_intent_succeeds() {
        let (engine, mut rx, tracker) = make_engine();
        let intent = test_intent();
        let coid = engine.submit_intent(&intent).await.unwrap();

        // Order should be tracked
        let order = tracker.get(&coid).unwrap();
        assert_eq!(order.state, OrderState::Filled); // SimulationClient fills immediately
        assert!(order.venue_order_id.is_some());

        // Event should be emitted
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, BotEvent::OrderAck(_)));
    }

    #[tokio::test]
    async fn duplicate_order_blocked() {
        let (_engine, _rx, _tracker) = make_engine();
        let intent = test_intent();

        // First submit should work — but sim client fills immediately,
        // so the pending contract is cleared. Create a partial-fill scenario.
        let client = Arc::new(SimulationClient::new(0.5));
        let tracker = Arc::new(OrderTracker::new());
        let (event_tx, _rx2) = mpsc::channel(64);
        let engine2 = ExecutionEngine::new(client, Arc::clone(&tracker), test_config(), event_tx);

        let _coid = engine2.submit_intent(&intent).await.unwrap();
        // Second submit to same contract should be blocked
        let result = engine2.submit_intent(&intent).await;
        assert!(matches!(result, Err(SubmitError::DuplicateOrder)));
    }

    #[tokio::test]
    async fn concurrent_limit_enforced() {
        let client = Arc::new(SimulationClient::new(0.5)); // Stays Acked, not Filled
        let tracker = Arc::new(OrderTracker::new());
        let (event_tx, _rx) = mpsc::channel(64);
        let config = ExecutionConfig {
            max_concurrent_orders: 1,
            ..test_config()
        };
        let engine = ExecutionEngine::new(client, Arc::clone(&tracker), config, event_tx);

        let intent1 = test_intent();
        let _coid1 = engine.submit_intent(&intent1).await.unwrap();

        let mut intent2 = test_intent();
        intent2.contract.token_id = TokenId("tok2".into());
        let result = engine.submit_intent(&intent2).await;
        assert!(matches!(result, Err(SubmitError::ConcurrentLimit)));
    }

    #[tokio::test]
    async fn stale_signal_rejected() {
        let (engine, _rx, _tracker) = make_engine();
        let mut intent = test_intent();
        intent.signal_timestamp = Utc::now() - chrono::Duration::seconds(60);

        let result = engine.submit_intent(&intent).await;
        assert!(matches!(result, Err(SubmitError::StaleSignal)));
    }

    #[test]
    fn tracker_active_count() {
        let tracker = OrderTracker::new();
        assert_eq!(tracker.active_count(), 0);

        let now = Utc::now();
        let record = OrderRecord {
            client_order_id: ClientOrderId::new(),
            venue_order_id: None,
            contract: ContractKey {
                market_id: MarketId("m".into()),
                token_id: TokenId("t".into()),
            },
            side: Side::Buy,
            price: dec!(0.5),
            size: dec!(10),
            filled_size: Decimal::ZERO,
            avg_fill_price: None,
            state: OrderState::Pending,
            created_at: now,
            updated_at: now,
            retry_count: 0,
        };
        tracker.register(record);
        assert_eq!(tracker.active_count(), 1);
    }

    #[test]
    fn tracker_state_transitions() {
        let tracker = OrderTracker::new();
        let now = Utc::now();
        let coid = ClientOrderId::new();
        let contract = ContractKey {
            market_id: MarketId("m".into()),
            token_id: TokenId("t".into()),
        };

        let record = OrderRecord {
            client_order_id: coid.clone(),
            venue_order_id: None,
            contract: contract.clone(),
            side: Side::Buy,
            price: dec!(0.5),
            size: dec!(10),
            filled_size: Decimal::ZERO,
            avg_fill_price: None,
            state: OrderState::Pending,
            created_at: now,
            updated_at: now,
            retry_count: 0,
        };
        tracker.register(record);

        assert!(tracker.has_pending_order(&contract));

        // Transition to Filled
        tracker.update_state(&coid, OrderState::Filled, Some(VenueOrderId("v1".into())));
        assert!(!tracker.has_pending_order(&contract));

        let order = tracker.get(&coid).unwrap();
        assert_eq!(order.state, OrderState::Filled);
        assert_eq!(order.venue_order_id.unwrap().0, "v1");
    }

    #[test]
    fn uncertain_orders_tracked() {
        let tracker = OrderTracker::new();
        let now = Utc::now();
        let coid = ClientOrderId::new();

        tracker.register(OrderRecord {
            client_order_id: coid.clone(),
            venue_order_id: None,
            contract: ContractKey {
                market_id: MarketId("m".into()),
                token_id: TokenId("t".into()),
            },
            side: Side::Buy,
            price: dec!(0.5),
            size: dec!(10),
            filled_size: Decimal::ZERO,
            avg_fill_price: None,
            state: OrderState::Pending,
            created_at: now,
            updated_at: now,
            retry_count: 0,
        });

        tracker.mark_uncertain(&coid);
        let uncertain = tracker.uncertain_orders();
        assert_eq!(uncertain.len(), 1);
        assert_eq!(uncertain[0].state, OrderState::Uncertain);
    }
}
