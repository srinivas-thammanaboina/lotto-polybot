//! Order intent validation and construction from signals.
//!
//! Bridges the strategy layer's `OrderIntent` to the execution layer's
//! `SubmitOrderRequest`. Validates intent freshness and constructs the
//! order with appropriate type based on market duration regime.

use std::time::Duration;

use chrono::Utc;
use tracing::debug;

use crate::domain::market::MarketDuration;
use crate::domain::order::ClientOrderId;
use crate::domain::signal::OrderIntent;
use crate::execution::client::{OrderType, SubmitOrderRequest};

/// Determines the appropriate order type based on market duration and urgency.
///
/// - 5m markets use FOK (fill-or-kill) for speed — we can't wait.
/// - 15m markets use GTC (good-til-cancel) for better fill probability.
pub fn order_type_for_duration(duration: MarketDuration) -> OrderType {
    match duration {
        MarketDuration::FiveMin => OrderType::FillOrKill,
        MarketDuration::FifteenMin => OrderType::GoodTilCancel,
    }
}

/// Validate an order intent is still fresh and valid for submission.
pub fn validate_intent(intent: &OrderIntent, stale_threshold: Duration) -> Result<(), String> {
    let age = Utc::now() - intent.signal_timestamp;
    let threshold =
        chrono::Duration::from_std(stale_threshold).unwrap_or(chrono::Duration::seconds(1));

    if age > threshold {
        return Err(format!(
            "intent stale: age={}ms threshold={}ms",
            age.num_milliseconds(),
            threshold.num_milliseconds()
        ));
    }

    if intent.size.is_zero() {
        return Err("zero size".to_string());
    }

    if intent.net_edge.is_sign_negative() {
        return Err("negative net edge".to_string());
    }

    Ok(())
}

/// Construct a submit request from a validated order intent.
pub fn intent_to_request(intent: &OrderIntent) -> SubmitOrderRequest {
    let order_type = order_type_for_duration(intent.duration);

    debug!(
        contract = %intent.contract,
        side = %intent.side,
        price = %intent.target_price,
        size = %intent.size,
        order_type = %order_type,
        "intent_to_request"
    );

    SubmitOrderRequest {
        client_order_id: ClientOrderId::new(),
        contract: intent.contract.clone(),
        side: intent.side,
        price: intent.target_price,
        size: intent.size,
        order_type,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{Asset, ContractKey, MarketId, TokenId};
    use crate::domain::signal::Side;
    use crate::strategy::edge::CostSnapshot;
    use rust_decimal::Decimal;
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
            rationale: "test".to_string(),
            model_version: "fv-v1.0".to_string(),
            signal_timestamp: Utc::now(),
        }
    }

    #[test]
    fn five_min_uses_fok() {
        assert_eq!(
            order_type_for_duration(MarketDuration::FiveMin),
            OrderType::FillOrKill
        );
    }

    #[test]
    fn fifteen_min_uses_gtc() {
        assert_eq!(
            order_type_for_duration(MarketDuration::FifteenMin),
            OrderType::GoodTilCancel
        );
    }

    #[test]
    fn validate_fresh_intent() {
        let intent = test_intent();
        assert!(validate_intent(&intent, Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn validate_stale_intent() {
        let mut intent = test_intent();
        intent.signal_timestamp = Utc::now() - chrono::Duration::seconds(60);
        assert!(validate_intent(&intent, Duration::from_secs(5)).is_err());
    }

    #[test]
    fn validate_zero_size() {
        let mut intent = test_intent();
        intent.size = Decimal::ZERO;
        assert!(validate_intent(&intent, Duration::from_secs(5)).is_err());
    }

    #[test]
    fn validate_negative_edge() {
        let mut intent = test_intent();
        intent.net_edge = dec!(-0.01);
        assert!(validate_intent(&intent, Duration::from_secs(5)).is_err());
    }

    #[test]
    fn intent_to_request_sets_fields() {
        let intent = test_intent();
        let req = intent_to_request(&intent);
        assert_eq!(req.contract, intent.contract);
        assert_eq!(req.side, intent.side);
        assert_eq!(req.price, intent.target_price);
        assert_eq!(req.size, intent.size);
        assert_eq!(req.order_type, OrderType::FillOrKill);
    }
}
