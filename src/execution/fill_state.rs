//! Fill state tracking and exposure feedback.
//!
//! Connects venue WebSocket events (acks, fills, cancels, rejects) back into
//! execution and risk state. Updates exposure on every lifecycle event so
//! signal gates can see pending/open exposure.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::domain::market::ContractKey;
use crate::domain::order::OrderState;
use crate::domain::signal::Side;
use crate::execution::submit::OrderTracker;
use crate::types::{FillEvent, OrderAck, OrderStateChange};

// ---------------------------------------------------------------------------
// Exposure tracking
// ---------------------------------------------------------------------------

/// Per-contract exposure record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractExposure {
    pub contract: ContractKey,
    pub side: Side,
    /// Notional pending (orders submitted but not filled).
    pub pending_notional: Decimal,
    /// Notional filled (open position).
    pub filled_notional: Decimal,
    /// Total fees paid on this contract.
    pub fees_paid: Decimal,
    /// Average fill price.
    pub avg_fill_price: Decimal,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}

/// Aggregate exposure snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExposureSnapshot {
    /// Total pending notional across all contracts.
    pub total_pending: Decimal,
    /// Total filled notional across all contracts.
    pub total_filled: Decimal,
    /// Gross exposure = pending + filled.
    pub gross_exposure: Decimal,
    /// Number of contracts with active exposure.
    pub active_contracts: usize,
    /// Per-contract breakdown.
    pub contracts: Vec<ContractExposure>,
}

/// Thread-safe exposure tracker.
#[derive(Debug, Clone)]
pub struct ExposureTracker {
    exposures: Arc<RwLock<HashMap<String, ContractExposure>>>,
}

impl ExposureTracker {
    pub fn new() -> Self {
        Self {
            exposures: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a new pending order.
    pub fn add_pending(&self, contract: &ContractKey, side: Side, notional: Decimal) {
        let key = contract.to_string();
        let mut map = self.exposures.write();
        let entry = map.entry(key).or_insert_with(|| ContractExposure {
            contract: contract.clone(),
            side,
            pending_notional: Decimal::ZERO,
            filled_notional: Decimal::ZERO,
            fees_paid: Decimal::ZERO,
            avg_fill_price: Decimal::ZERO,
            updated_at: Utc::now(),
        });
        entry.pending_notional += notional;
        entry.updated_at = Utc::now();

        debug!(
            contract = %contract,
            pending = %entry.pending_notional,
            "exposure_pending_added"
        );
    }

    /// Remove pending exposure (order cancelled/rejected).
    pub fn remove_pending(&self, contract: &ContractKey, notional: Decimal) {
        let key = contract.to_string();
        let mut map = self.exposures.write();
        if let Some(entry) = map.get_mut(&key) {
            entry.pending_notional = (entry.pending_notional - notional).max(Decimal::ZERO);
            entry.updated_at = Utc::now();

            debug!(
                contract = %contract,
                pending = %entry.pending_notional,
                "exposure_pending_removed"
            );

            // Clean up if no exposure remains
            if entry.pending_notional.is_zero() && entry.filled_notional.is_zero() {
                map.remove(&key);
            }
        }
    }

    /// Apply a fill event: move from pending to filled exposure.
    pub fn apply_fill(
        &self,
        contract: &ContractKey,
        filled_size: Decimal,
        price: Decimal,
        fee: Decimal,
    ) {
        let key = contract.to_string();
        let mut map = self.exposures.write();
        if let Some(entry) = map.get_mut(&key) {
            let fill_notional = filled_size * price;
            entry.pending_notional = (entry.pending_notional - fill_notional).max(Decimal::ZERO);

            // Weighted average fill price
            let old_filled = entry.filled_notional;
            let new_filled = old_filled + fill_notional;
            if new_filled > Decimal::ZERO {
                entry.avg_fill_price =
                    (entry.avg_fill_price * old_filled + price * fill_notional) / new_filled;
            }
            entry.filled_notional = new_filled;
            entry.fees_paid += fee;
            entry.updated_at = Utc::now();

            info!(
                contract = %contract,
                filled_size = %filled_size,
                price = %price,
                total_filled = %entry.filled_notional,
                "exposure_fill_applied"
            );
        }
    }

    /// Close exposure for a contract (full fill or position exit).
    pub fn close_position(&self, contract: &ContractKey) {
        let key = contract.to_string();
        self.exposures.write().remove(&key);
        debug!(contract = %contract, "exposure_closed");
    }

    /// Get current exposure for a specific contract.
    pub fn contract_exposure(&self, contract: &ContractKey) -> Option<ContractExposure> {
        self.exposures.read().get(&contract.to_string()).cloned()
    }

    /// Get the current notional exposure for a contract (pending + filled).
    pub fn contract_notional(&self, contract: &ContractKey) -> Decimal {
        self.exposures
            .read()
            .get(&contract.to_string())
            .map(|e| e.pending_notional + e.filled_notional)
            .unwrap_or(Decimal::ZERO)
    }

    /// Get aggregate exposure snapshot.
    pub fn snapshot(&self) -> ExposureSnapshot {
        let map = self.exposures.read();
        let contracts: Vec<ContractExposure> = map.values().cloned().collect();
        let total_pending: Decimal = contracts.iter().map(|c| c.pending_notional).sum();
        let total_filled: Decimal = contracts.iter().map(|c| c.filled_notional).sum();

        ExposureSnapshot {
            total_pending,
            total_filled,
            gross_exposure: total_pending + total_filled,
            active_contracts: contracts.len(),
            contracts,
        }
    }

    /// Count of contracts with any active exposure.
    pub fn active_position_count(&self) -> u32 {
        self.exposures
            .read()
            .values()
            .filter(|e| e.filled_notional > Decimal::ZERO)
            .count() as u32
    }

    /// Clear all exposure (for reconciliation reset).
    pub fn clear(&self) {
        self.exposures.write().clear();
    }

    /// Load exposure from reconciliation data.
    pub fn load_from_reconciliation(&self, exposures: Vec<ContractExposure>) {
        let mut map = self.exposures.write();
        for exp in exposures {
            map.insert(exp.contract.to_string(), exp);
        }
    }
}

impl Default for ExposureTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Fill state processor — processes venue events and updates tracker + exposure
// ---------------------------------------------------------------------------

/// Processes venue lifecycle events and updates order tracker + exposure.
pub struct FillStateProcessor {
    tracker: Arc<OrderTracker>,
    exposure: Arc<ExposureTracker>,
}

impl FillStateProcessor {
    pub fn new(tracker: Arc<OrderTracker>, exposure: Arc<ExposureTracker>) -> Self {
        Self { tracker, exposure }
    }

    /// Process an order acknowledgment.
    pub fn process_ack(&self, ack: &OrderAck) {
        self.tracker.update_state(
            &ack.client_order_id,
            OrderState::Acked,
            Some(ack.venue_order_id.clone()),
        );
        debug!(
            client_order_id = %ack.client_order_id,
            venue_order_id = %ack.venue_order_id.0,
            "fill_state_ack"
        );
    }

    /// Process a fill event (partial or full).
    pub fn process_fill(&self, fill: &FillEvent) {
        let new_state = if fill.remaining_size.is_zero() {
            OrderState::Filled
        } else {
            OrderState::PartialFill
        };

        // Update order tracker
        self.tracker.apply_fill(
            &fill.client_order_id,
            fill.filled_size,
            fill.price,
            new_state,
        );

        // Update exposure
        self.exposure
            .apply_fill(&fill.contract, fill.filled_size, fill.price, fill.fee);

        info!(
            client_order_id = %fill.client_order_id,
            contract = %fill.contract,
            filled = %fill.filled_size,
            remaining = %fill.remaining_size,
            price = %fill.price,
            fee = %fill.fee,
            state = %new_state,
            "fill_state_fill"
        );
    }

    /// Process an order state change (cancel, reject, etc).
    pub fn process_state_change(&self, change: &OrderStateChange) {
        self.tracker
            .update_state(&change.client_order_id, change.new_state, None);

        // On cancel/reject, remove pending exposure
        if matches!(
            change.new_state,
            OrderState::Canceled | OrderState::Rejected
        ) && let Some(order) = self.tracker.get(&change.client_order_id)
        {
            let pending_notional = (order.size - order.filled_size) * order.price;
            self.exposure
                .remove_pending(&order.contract, pending_notional);
        }

        info!(
            client_order_id = %change.client_order_id,
            new_state = %change.new_state,
            reason = change.reason.as_deref().unwrap_or("none"),
            "fill_state_change"
        );
    }

    /// Get the exposure tracker for external access.
    pub fn exposure(&self) -> &Arc<ExposureTracker> {
        &self.exposure
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{MarketId, TokenId};
    use crate::domain::order::{ClientOrderId, VenueOrderId};
    use rust_decimal_macros::dec;

    fn test_contract() -> ContractKey {
        ContractKey {
            market_id: MarketId("mkt1".into()),
            token_id: TokenId("tok1".into()),
        }
    }

    #[test]
    fn exposure_add_and_remove_pending() {
        let tracker = ExposureTracker::new();
        let contract = test_contract();

        tracker.add_pending(&contract, Side::Buy, dec!(10));
        assert_eq!(tracker.contract_notional(&contract), dec!(10));

        tracker.remove_pending(&contract, dec!(10));
        assert_eq!(tracker.contract_notional(&contract), Decimal::ZERO);
    }

    #[test]
    fn exposure_apply_fill() {
        let tracker = ExposureTracker::new();
        let contract = test_contract();

        tracker.add_pending(&contract, Side::Buy, dec!(10));
        tracker.apply_fill(&contract, dec!(20), dec!(0.50), dec!(0.10));

        let exp = tracker.contract_exposure(&contract).unwrap();
        assert_eq!(exp.filled_notional, dec!(10)); // 20 * 0.50 = 10
        assert_eq!(exp.fees_paid, dec!(0.10));
    }

    #[test]
    fn exposure_close_position() {
        let tracker = ExposureTracker::new();
        let contract = test_contract();

        tracker.add_pending(&contract, Side::Buy, dec!(10));
        tracker.close_position(&contract);
        assert!(tracker.contract_exposure(&contract).is_none());
    }

    #[test]
    fn exposure_snapshot() {
        let tracker = ExposureTracker::new();
        let c1 = test_contract();
        let c2 = ContractKey {
            market_id: MarketId("mkt2".into()),
            token_id: TokenId("tok2".into()),
        };

        tracker.add_pending(&c1, Side::Buy, dec!(10));
        tracker.add_pending(&c2, Side::Sell, dec!(20));

        let snap = tracker.snapshot();
        assert_eq!(snap.total_pending, dec!(30));
        assert_eq!(snap.active_contracts, 2);
        assert_eq!(snap.gross_exposure, dec!(30));
    }

    #[test]
    fn active_position_count() {
        let tracker = ExposureTracker::new();
        let contract = test_contract();

        // Only pending, not filled
        tracker.add_pending(&contract, Side::Buy, dec!(10));
        assert_eq!(tracker.active_position_count(), 0);

        // Apply fill
        tracker.apply_fill(&contract, dec!(20), dec!(0.50), Decimal::ZERO);
        assert_eq!(tracker.active_position_count(), 1);
    }

    #[test]
    fn fill_processor_ack() {
        let order_tracker = Arc::new(OrderTracker::new());
        let exposure = Arc::new(ExposureTracker::new());
        let processor = FillStateProcessor::new(Arc::clone(&order_tracker), Arc::clone(&exposure));

        let coid = ClientOrderId::new();
        let now = Utc::now();

        // Register an order first
        order_tracker.register(crate::domain::order::OrderRecord {
            client_order_id: coid.clone(),
            venue_order_id: None,
            contract: test_contract(),
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

        processor.process_ack(&OrderAck {
            client_order_id: coid.clone(),
            venue_order_id: VenueOrderId("v1".into()),
            timestamp: now,
        });

        let order = order_tracker.get(&coid).unwrap();
        assert_eq!(order.state, OrderState::Acked);
    }

    #[test]
    fn fill_processor_fill() {
        let order_tracker = Arc::new(OrderTracker::new());
        let exposure = Arc::new(ExposureTracker::new());
        let processor = FillStateProcessor::new(Arc::clone(&order_tracker), Arc::clone(&exposure));

        let coid = ClientOrderId::new();
        let contract = test_contract();
        let now = Utc::now();

        order_tracker.register(crate::domain::order::OrderRecord {
            client_order_id: coid.clone(),
            venue_order_id: Some(VenueOrderId("v1".into())),
            contract: contract.clone(),
            side: Side::Buy,
            price: dec!(0.55),
            size: dec!(10),
            filled_size: Decimal::ZERO,
            avg_fill_price: None,
            state: OrderState::Acked,
            created_at: now,
            updated_at: now,
            retry_count: 0,
        });

        exposure.add_pending(&contract, Side::Buy, dec!(5.5));

        processor.process_fill(&FillEvent {
            client_order_id: coid.clone(),
            venue_order_id: VenueOrderId("v1".into()),
            contract: contract.clone(),
            side: Side::Buy,
            price: dec!(0.55),
            filled_size: dec!(10),
            remaining_size: Decimal::ZERO,
            fee: dec!(0.05),
            timestamp: now,
        });

        let order = order_tracker.get(&coid).unwrap();
        assert_eq!(order.state, OrderState::Filled);

        let exp = exposure.contract_exposure(&contract).unwrap();
        assert!(exp.filled_notional > Decimal::ZERO);
        assert_eq!(exp.fees_paid, dec!(0.05));
    }

    #[test]
    fn fill_processor_cancel() {
        let order_tracker = Arc::new(OrderTracker::new());
        let exposure = Arc::new(ExposureTracker::new());
        let processor = FillStateProcessor::new(Arc::clone(&order_tracker), Arc::clone(&exposure));

        let coid = ClientOrderId::new();
        let contract = test_contract();
        let now = Utc::now();

        order_tracker.register(crate::domain::order::OrderRecord {
            client_order_id: coid.clone(),
            venue_order_id: Some(VenueOrderId("v1".into())),
            contract: contract.clone(),
            side: Side::Buy,
            price: dec!(0.55),
            size: dec!(10),
            filled_size: Decimal::ZERO,
            avg_fill_price: None,
            state: OrderState::Acked,
            created_at: now,
            updated_at: now,
            retry_count: 0,
        });

        exposure.add_pending(&contract, Side::Buy, dec!(5.5));

        processor.process_state_change(&OrderStateChange {
            client_order_id: coid.clone(),
            new_state: OrderState::Canceled,
            reason: Some("user_cancel".into()),
            timestamp: now,
        });

        let order = order_tracker.get(&coid).unwrap();
        assert_eq!(order.state, OrderState::Canceled);
    }
}
