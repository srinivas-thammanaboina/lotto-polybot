//! Bounded cancellation and replace rules.
//!
//! Ensures stale or unfilled intents do not linger. Cancel/replace policy
//! is tied to market duration regime. Every cancel/replace action is logged
//! with a reason.

use std::time::Duration;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::domain::market::MarketDuration;
use crate::domain::order::{ClientOrderId, OrderRecord, OrderState};

// ---------------------------------------------------------------------------
// Cancel reason
// ---------------------------------------------------------------------------

/// Why an order is being cancelled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CancelReason {
    /// Order exceeded max age without filling.
    MaxAge { age_ms: i64, limit_ms: i64 },
    /// The signal that generated this order is now stale.
    StaleSignal { age_ms: i64 },
    /// Order has been partially filled but timed out.
    PartialFillTimeout { filled_pct: Decimal, age_ms: i64 },
    /// Market is approaching expiry — close out orders.
    NearExpiry { time_to_expiry_ms: i64 },
    /// Kill switch activated.
    KillSwitch,
    /// Manual operator cancel.
    Operator,
}

impl std::fmt::Display for CancelReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CancelReason::MaxAge { age_ms, limit_ms } => {
                write!(f, "max_age({age_ms}ms/{limit_ms}ms)")
            }
            CancelReason::StaleSignal { age_ms } => write!(f, "stale_signal({age_ms}ms)"),
            CancelReason::PartialFillTimeout { filled_pct, age_ms } => {
                write!(f, "partial_timeout(filled={filled_pct}% age={age_ms}ms)")
            }
            CancelReason::NearExpiry { time_to_expiry_ms } => {
                write!(f, "near_expiry({time_to_expiry_ms}ms)")
            }
            CancelReason::KillSwitch => write!(f, "kill_switch"),
            CancelReason::Operator => write!(f, "operator"),
        }
    }
}

// ---------------------------------------------------------------------------
// Cancel policy config (per duration regime)
// ---------------------------------------------------------------------------

/// Cancel policy thresholds for a market duration regime.
#[derive(Debug, Clone)]
pub struct CancelPolicyConfig {
    /// Max time an unfilled order can remain open.
    pub max_order_age: Duration,
    /// Max time for a partially filled order before cancelling remainder.
    pub partial_fill_timeout: Duration,
    /// Time before market expiry to cancel all open orders.
    pub expiry_buffer: Duration,
    /// Max time since the original signal before marking as stale.
    pub stale_signal_threshold: Duration,
}

impl CancelPolicyConfig {
    /// Default policy for 5m markets — tighter timeouts.
    pub fn five_min() -> Self {
        Self {
            max_order_age: Duration::from_secs(30),
            partial_fill_timeout: Duration::from_secs(20),
            expiry_buffer: Duration::from_secs(60),
            stale_signal_threshold: Duration::from_secs(5),
        }
    }

    /// Default policy for 15m markets — more relaxed.
    pub fn fifteen_min() -> Self {
        Self {
            max_order_age: Duration::from_secs(90),
            partial_fill_timeout: Duration::from_secs(60),
            expiry_buffer: Duration::from_secs(120),
            stale_signal_threshold: Duration::from_secs(10),
        }
    }

    /// Get the appropriate config for a duration.
    pub fn for_duration(duration: MarketDuration) -> Self {
        match duration {
            MarketDuration::FiveMin => Self::five_min(),
            MarketDuration::FifteenMin => Self::fifteen_min(),
        }
    }
}

// ---------------------------------------------------------------------------
// Cancel decision
// ---------------------------------------------------------------------------

/// Result of evaluating cancel policy for an order.
#[derive(Debug, Clone)]
pub enum CancelDecision {
    /// Keep the order open.
    Keep,
    /// Cancel the order with a reason.
    Cancel(CancelReason),
}

// ---------------------------------------------------------------------------
// Cancel policy engine
// ---------------------------------------------------------------------------

/// Evaluates whether open orders should be cancelled.
pub struct CancelPolicy;

impl CancelPolicy {
    /// Evaluate whether a single order should be cancelled.
    pub fn evaluate(
        order: &OrderRecord,
        config: &CancelPolicyConfig,
        market_expiry: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> CancelDecision {
        // Only evaluate active orders
        if !matches!(
            order.state,
            OrderState::Pending | OrderState::Acked | OrderState::PartialFill
        ) {
            return CancelDecision::Keep;
        }

        let order_age = now - order.created_at;
        let age_ms = order_age.num_milliseconds();

        // Check 1: Kill switch is handled externally, not here

        // Check 2: Near expiry
        if let Some(expiry) = market_expiry {
            let time_to_expiry = expiry - now;
            let buffer = chrono::Duration::from_std(config.expiry_buffer)
                .unwrap_or(chrono::Duration::seconds(60));
            if time_to_expiry < buffer {
                info!(
                    client_order_id = %order.client_order_id,
                    time_to_expiry_ms = time_to_expiry.num_milliseconds(),
                    "cancel_near_expiry"
                );
                return CancelDecision::Cancel(CancelReason::NearExpiry {
                    time_to_expiry_ms: time_to_expiry.num_milliseconds(),
                });
            }
        }

        // Check 3: Partial fill timeout
        if order.state == OrderState::PartialFill {
            let partial_limit = chrono::Duration::from_std(config.partial_fill_timeout)
                .unwrap_or(chrono::Duration::seconds(20));
            if order_age > partial_limit {
                let filled_pct = if order.size > Decimal::ZERO {
                    (order.filled_size / order.size) * Decimal::from(100)
                } else {
                    Decimal::ZERO
                };
                warn!(
                    client_order_id = %order.client_order_id,
                    filled_pct = %filled_pct,
                    age_ms = age_ms,
                    "cancel_partial_timeout"
                );
                return CancelDecision::Cancel(CancelReason::PartialFillTimeout {
                    filled_pct,
                    age_ms,
                });
            }
            // Don't apply max_age to partial fills — use partial_fill_timeout instead
            return CancelDecision::Keep;
        }

        // Check 4: Max order age (unfilled)
        let max_age = chrono::Duration::from_std(config.max_order_age)
            .unwrap_or(chrono::Duration::seconds(30));
        if order_age > max_age {
            warn!(
                client_order_id = %order.client_order_id,
                age_ms = age_ms,
                limit_ms = max_age.num_milliseconds(),
                "cancel_max_age"
            );
            return CancelDecision::Cancel(CancelReason::MaxAge {
                age_ms,
                limit_ms: max_age.num_milliseconds(),
            });
        }

        debug!(
            client_order_id = %order.client_order_id,
            age_ms = age_ms,
            "cancel_policy_keep"
        );
        CancelDecision::Keep
    }

    /// Scan all active orders and return those that should be cancelled.
    pub fn scan_orders(
        orders: &[OrderRecord],
        duration: MarketDuration,
        market_expiry: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Vec<(ClientOrderId, CancelReason)> {
        let config = CancelPolicyConfig::for_duration(duration);
        let mut to_cancel = Vec::new();

        for order in orders {
            if let CancelDecision::Cancel(reason) =
                Self::evaluate(order, &config, market_expiry, now)
            {
                to_cancel.push((order.client_order_id.clone(), reason));
            }
        }

        if !to_cancel.is_empty() {
            info!(
                count = to_cancel.len(),
                duration = %duration,
                "cancel_policy_scan_results"
            );
        }

        to_cancel
    }

    /// Emergency cancel all — for kill switch activation.
    pub fn cancel_all(orders: &[OrderRecord]) -> Vec<(ClientOrderId, CancelReason)> {
        orders
            .iter()
            .filter(|o| {
                matches!(
                    o.state,
                    OrderState::Pending | OrderState::Acked | OrderState::PartialFill
                )
            })
            .map(|o| (o.client_order_id.clone(), CancelReason::KillSwitch))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{ContractKey, MarketId, TokenId};
    use crate::domain::order::VenueOrderId;
    use crate::domain::signal::Side;
    use rust_decimal_macros::dec;

    fn test_order(state: OrderState, age_secs: i64) -> OrderRecord {
        let now = Utc::now();
        OrderRecord {
            client_order_id: ClientOrderId::new(),
            venue_order_id: Some(VenueOrderId("v1".into())),
            contract: ContractKey {
                market_id: MarketId("mkt1".into()),
                token_id: TokenId("tok1".into()),
            },
            side: Side::Buy,
            price: dec!(0.55),
            size: dec!(10),
            filled_size: if state == OrderState::PartialFill {
                dec!(3)
            } else {
                Decimal::ZERO
            },
            avg_fill_price: None,
            state,
            created_at: now - chrono::Duration::seconds(age_secs),
            updated_at: now,
            retry_count: 0,
        }
    }

    #[test]
    fn keep_fresh_order() {
        let order = test_order(OrderState::Acked, 5);
        let config = CancelPolicyConfig::five_min();
        let decision = CancelPolicy::evaluate(&order, &config, None, Utc::now());
        assert!(matches!(decision, CancelDecision::Keep));
    }

    #[test]
    fn cancel_old_unfilled_order() {
        let order = test_order(OrderState::Acked, 60); // 60s > 30s limit
        let config = CancelPolicyConfig::five_min();
        let decision = CancelPolicy::evaluate(&order, &config, None, Utc::now());
        assert!(matches!(
            decision,
            CancelDecision::Cancel(CancelReason::MaxAge { .. })
        ));
    }

    #[test]
    fn cancel_partial_fill_timeout() {
        let order = test_order(OrderState::PartialFill, 30); // 30s > 20s limit
        let config = CancelPolicyConfig::five_min();
        let decision = CancelPolicy::evaluate(&order, &config, None, Utc::now());
        assert!(matches!(
            decision,
            CancelDecision::Cancel(CancelReason::PartialFillTimeout { .. })
        ));
    }

    #[test]
    fn cancel_near_expiry() {
        let order = test_order(OrderState::Acked, 5);
        let config = CancelPolicyConfig::five_min();
        let expiry = Utc::now() + chrono::Duration::seconds(30); // 30s < 60s buffer
        let decision = CancelPolicy::evaluate(&order, &config, Some(expiry), Utc::now());
        assert!(matches!(
            decision,
            CancelDecision::Cancel(CancelReason::NearExpiry { .. })
        ));
    }

    #[test]
    fn keep_if_expiry_far_away() {
        let order = test_order(OrderState::Acked, 5);
        let config = CancelPolicyConfig::five_min();
        let expiry = Utc::now() + chrono::Duration::seconds(300); // 5 min away
        let decision = CancelPolicy::evaluate(&order, &config, Some(expiry), Utc::now());
        assert!(matches!(decision, CancelDecision::Keep));
    }

    #[test]
    fn skip_terminal_orders() {
        let order = test_order(OrderState::Filled, 60);
        let config = CancelPolicyConfig::five_min();
        let decision = CancelPolicy::evaluate(&order, &config, None, Utc::now());
        assert!(matches!(decision, CancelDecision::Keep));
    }

    #[test]
    fn fifteen_min_more_relaxed() {
        let order = test_order(OrderState::Acked, 60); // 60s
        let config_5m = CancelPolicyConfig::five_min();
        let config_15m = CancelPolicyConfig::fifteen_min();

        let decision_5m = CancelPolicy::evaluate(&order, &config_5m, None, Utc::now());
        let decision_15m = CancelPolicy::evaluate(&order, &config_15m, None, Utc::now());

        // 5m should cancel at 60s (> 30s limit), 15m should keep (< 90s limit)
        assert!(matches!(decision_5m, CancelDecision::Cancel(_)));
        assert!(matches!(decision_15m, CancelDecision::Keep));
    }

    #[test]
    fn scan_orders_finds_cancellable() {
        let orders = vec![
            test_order(OrderState::Acked, 5),   // fresh — keep
            test_order(OrderState::Acked, 60),  // stale — cancel
            test_order(OrderState::Filled, 60), // terminal — skip
        ];

        let to_cancel =
            CancelPolicy::scan_orders(&orders, MarketDuration::FiveMin, None, Utc::now());
        assert_eq!(to_cancel.len(), 1);
    }

    #[test]
    fn cancel_all_for_kill_switch() {
        let orders = vec![
            test_order(OrderState::Acked, 5),
            test_order(OrderState::PartialFill, 10),
            test_order(OrderState::Filled, 20),
        ];

        let to_cancel = CancelPolicy::cancel_all(&orders);
        assert_eq!(to_cancel.len(), 2); // Filled is excluded
        assert!(
            to_cancel
                .iter()
                .all(|(_, r)| matches!(r, CancelReason::KillSwitch))
        );
    }

    #[test]
    fn cancel_reason_display() {
        let reason = CancelReason::MaxAge {
            age_ms: 60000,
            limit_ms: 30000,
        };
        assert_eq!(reason.to_string(), "max_age(60000ms/30000ms)");
    }
}
