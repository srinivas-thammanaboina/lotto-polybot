//! Exposure and position limits.
//!
//! Enforces hard limits before order submission. Pending orders are treated
//! as exposure. Separate from advisory telemetry — these are blocking checks.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::RiskConfig;
use crate::domain::market::ContractKey;
use crate::domain::signal::RejectReason;
use crate::execution::fill_state::ExposureSnapshot;

// ---------------------------------------------------------------------------
// Limit check result
// ---------------------------------------------------------------------------

/// Result of a limit check. If violations is empty, the check passed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitCheckResult {
    pub passed: bool,
    pub violations: Vec<LimitViolation>,
}

/// A specific limit violation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitViolation {
    pub limit_name: String,
    pub current: Decimal,
    pub limit: Decimal,
    pub reject_reason: RejectReason,
}

impl std::fmt::Display for LimitViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: current={} limit={}",
            self.limit_name, self.current, self.limit
        )
    }
}

// ---------------------------------------------------------------------------
// Limit engine
// ---------------------------------------------------------------------------

/// Enforces exposure and position limits before order submission.
pub struct LimitEngine;

impl LimitEngine {
    /// Check all limits for a proposed trade.
    ///
    /// `proposed_notional` is the size of the trade being evaluated.
    /// `exposure` is the current aggregate exposure snapshot.
    /// `contract_notional` is the current exposure on this specific contract.
    pub fn check(
        config: &RiskConfig,
        exposure: &ExposureSnapshot,
        contract: &ContractKey,
        contract_notional: Decimal,
        proposed_notional: Decimal,
    ) -> LimitCheckResult {
        let mut violations = Vec::new();

        // Limit 1: Max position per market
        let new_contract_notional = contract_notional + proposed_notional;
        if new_contract_notional > config.max_position_per_market {
            violations.push(LimitViolation {
                limit_name: "max_position_per_market".into(),
                current: new_contract_notional,
                limit: config.max_position_per_market,
                reject_reason: RejectReason::MaxExposureReached,
            });
        }

        // Limit 2: Max concurrent positions
        // Count contracts with filled exposure as positions
        let position_count = exposure.active_contracts as u32;
        // If this is a new contract (no existing exposure), it adds one
        let would_add = if contract_notional.is_zero() { 1 } else { 0 };
        if position_count + would_add > config.max_concurrent_positions {
            violations.push(LimitViolation {
                limit_name: "max_concurrent_positions".into(),
                current: Decimal::from(position_count + would_add),
                limit: Decimal::from(config.max_concurrent_positions),
                reject_reason: RejectReason::MaxExposureReached,
            });
        }

        // Limit 3: Max gross exposure
        let new_gross = exposure.gross_exposure + proposed_notional;
        if new_gross > config.max_gross_exposure {
            violations.push(LimitViolation {
                limit_name: "max_gross_exposure".into(),
                current: new_gross,
                limit: config.max_gross_exposure,
                reject_reason: RejectReason::MaxExposureReached,
            });
        }

        // Limit 4: Max notional per order
        if proposed_notional > config.max_notional_per_order {
            violations.push(LimitViolation {
                limit_name: "max_notional_per_order".into(),
                current: proposed_notional,
                limit: config.max_notional_per_order,
                reject_reason: RejectReason::MaxExposureReached,
            });
        }

        let passed = violations.is_empty();

        if !passed {
            warn!(
                contract = %contract,
                violations = violations.len(),
                "limit_check_failed"
            );
            for v in &violations {
                warn!(violation = %v, "limit_violation");
            }
        } else {
            debug!(
                contract = %contract,
                proposed = %proposed_notional,
                gross_after = %new_gross,
                "limit_check_passed"
            );
        }

        LimitCheckResult { passed, violations }
    }

    /// Quick check: would this trade be within all limits?
    pub fn would_pass(
        config: &RiskConfig,
        exposure: &ExposureSnapshot,
        contract: &ContractKey,
        contract_notional: Decimal,
        proposed_notional: Decimal,
    ) -> bool {
        Self::check(
            config,
            exposure,
            contract,
            contract_notional,
            proposed_notional,
        )
        .passed
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{MarketId, TokenId};
    use rust_decimal_macros::dec;

    fn test_config() -> RiskConfig {
        RiskConfig {
            max_position_per_market: dec!(50),
            max_concurrent_positions: 4,
            max_gross_exposure: dec!(200),
            max_daily_drawdown: dec!(50),
            max_total_drawdown: dec!(100),
            max_consecutive_losses: 5,
            max_notional_per_order: dec!(25),
        }
    }

    fn test_contract() -> ContractKey {
        ContractKey {
            market_id: MarketId("mkt1".into()),
            token_id: TokenId("tok1".into()),
        }
    }

    fn empty_exposure() -> ExposureSnapshot {
        ExposureSnapshot {
            total_pending: Decimal::ZERO,
            total_filled: Decimal::ZERO,
            gross_exposure: Decimal::ZERO,
            active_contracts: 0,
            contracts: Vec::new(),
        }
    }

    #[test]
    fn passes_within_all_limits() {
        let result = LimitEngine::check(
            &test_config(),
            &empty_exposure(),
            &test_contract(),
            Decimal::ZERO,
            dec!(10),
        );
        assert!(result.passed);
        assert!(result.violations.is_empty());
    }

    #[test]
    fn rejects_exceeding_position_per_market() {
        let result = LimitEngine::check(
            &test_config(),
            &empty_exposure(),
            &test_contract(),
            dec!(45), // existing
            dec!(10), // proposed → 55 > 50 limit
        );
        assert!(!result.passed);
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.limit_name == "max_position_per_market")
        );
    }

    #[test]
    fn rejects_exceeding_concurrent_positions() {
        let exposure = ExposureSnapshot {
            total_pending: dec!(40),
            total_filled: dec!(60),
            gross_exposure: dec!(100),
            active_contracts: 4, // Already at limit
            contracts: Vec::new(),
        };
        let result = LimitEngine::check(
            &test_config(),
            &exposure,
            &test_contract(),
            Decimal::ZERO, // New contract
            dec!(10),
        );
        assert!(!result.passed);
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.limit_name == "max_concurrent_positions")
        );
    }

    #[test]
    fn existing_contract_doesnt_add_position_count() {
        let exposure = ExposureSnapshot {
            total_pending: dec!(40),
            total_filled: dec!(60),
            gross_exposure: dec!(100),
            active_contracts: 4,
            contracts: Vec::new(),
        };
        // Existing position — doesn't add to count
        let result = LimitEngine::check(
            &test_config(),
            &exposure,
            &test_contract(),
            dec!(10), // Already has exposure
            dec!(5),
        );
        // Should NOT trigger concurrent positions (4 + 0 <= 4)
        assert!(
            !result
                .violations
                .iter()
                .any(|v| v.limit_name == "max_concurrent_positions")
        );
    }

    #[test]
    fn rejects_exceeding_gross_exposure() {
        let exposure = ExposureSnapshot {
            total_pending: dec!(100),
            total_filled: dec!(90),
            gross_exposure: dec!(190),
            active_contracts: 3,
            contracts: Vec::new(),
        };
        let result = LimitEngine::check(
            &test_config(),
            &exposure,
            &test_contract(),
            Decimal::ZERO,
            dec!(15), // 190 + 15 = 205 > 200
        );
        assert!(!result.passed);
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.limit_name == "max_gross_exposure")
        );
    }

    #[test]
    fn rejects_exceeding_notional_per_order() {
        let result = LimitEngine::check(
            &test_config(),
            &empty_exposure(),
            &test_contract(),
            Decimal::ZERO,
            dec!(30), // > 25 limit
        );
        assert!(!result.passed);
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.limit_name == "max_notional_per_order")
        );
    }

    #[test]
    fn multiple_violations_collected() {
        let exposure = ExposureSnapshot {
            total_pending: dec!(100),
            total_filled: dec!(100),
            gross_exposure: dec!(200),
            active_contracts: 4,
            contracts: Vec::new(),
        };
        let result = LimitEngine::check(
            &test_config(),
            &exposure,
            &test_contract(),
            Decimal::ZERO,
            dec!(30), // Exceeds notional AND gross AND concurrent
        );
        assert!(!result.passed);
        assert!(result.violations.len() >= 2);
    }

    #[test]
    fn would_pass_shortcut() {
        assert!(LimitEngine::would_pass(
            &test_config(),
            &empty_exposure(),
            &test_contract(),
            Decimal::ZERO,
            dec!(10),
        ));

        assert!(!LimitEngine::would_pass(
            &test_config(),
            &empty_exposure(),
            &test_contract(),
            Decimal::ZERO,
            dec!(30),
        ));
    }
}
