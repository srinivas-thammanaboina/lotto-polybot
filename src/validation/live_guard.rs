//! Live validation guard.
//!
//! Enforces tight safeguards for tiny-size live order testing.
//! Purpose: measure execution truth (latency, slippage, fill behavior),
//! not optimize P&L.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::RiskConfig;
use crate::domain::market::{Asset, MarketDuration};

// ---------------------------------------------------------------------------
// Live validation config — tighter than normal risk limits
// ---------------------------------------------------------------------------

/// Configuration for tiny-size live validation.
/// These override normal risk config to enforce minimal exposure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveValidationConfig {
    /// Maximum notional per order (tiny).
    pub max_notional: Decimal,
    /// Maximum concurrent positions.
    pub max_concurrent: u32,
    /// Maximum gross exposure.
    pub max_gross_exposure: Decimal,
    /// Allowed assets (empty = all).
    pub allowed_assets: Vec<Asset>,
    /// Allowed durations (empty = all).
    pub allowed_durations: Vec<MarketDuration>,
    /// Maximum daily loss before automatic kill switch.
    pub max_daily_loss: Decimal,
    /// Maximum consecutive losses before kill switch.
    pub max_consecutive_losses: u32,
    /// Whether operator must be actively supervising.
    pub require_operator_ack: bool,
}

impl Default for LiveValidationConfig {
    fn default() -> Self {
        Self {
            max_notional: dec!(5),
            max_concurrent: 1,
            max_gross_exposure: dec!(10),
            allowed_assets: vec![Asset::BTC],
            allowed_durations: vec![MarketDuration::FiveMin],
            max_daily_loss: dec!(10),
            max_consecutive_losses: 3,
            require_operator_ack: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Live validation guard
// ---------------------------------------------------------------------------

/// Pre-submission guard for live validation mode.
/// Rejects anything that exceeds the tight validation limits.
pub struct LiveGuard {
    config: LiveValidationConfig,
    operator_acked: bool,
}

/// Why the live guard rejected a trade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LiveGuardReject {
    OperatorNotAcked,
    NotionalTooLarge { requested: Decimal, limit: Decimal },
    AssetNotAllowed { asset: Asset },
    DurationNotAllowed { duration: MarketDuration },
    ExposureLimitExceeded { current: Decimal, limit: Decimal },
    ConcurrentLimitExceeded { current: u32, limit: u32 },
}

impl std::fmt::Display for LiveGuardReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveGuardReject::OperatorNotAcked => write!(f, "operator_not_acked"),
            LiveGuardReject::NotionalTooLarge { requested, limit } => {
                write!(f, "notional_too_large({requested}/{limit})")
            }
            LiveGuardReject::AssetNotAllowed { asset } => {
                write!(f, "asset_not_allowed({asset})")
            }
            LiveGuardReject::DurationNotAllowed { duration } => {
                write!(f, "duration_not_allowed({duration})")
            }
            LiveGuardReject::ExposureLimitExceeded { current, limit } => {
                write!(f, "exposure_exceeded({current}/{limit})")
            }
            LiveGuardReject::ConcurrentLimitExceeded { current, limit } => {
                write!(f, "concurrent_exceeded({current}/{limit})")
            }
        }
    }
}

impl LiveGuard {
    pub fn new(config: LiveValidationConfig) -> Self {
        let needs_ack = config.require_operator_ack;
        Self {
            config,
            operator_acked: !needs_ack,
        }
    }

    /// Operator acknowledges they are supervising.
    pub fn operator_ack(&mut self) {
        self.operator_acked = true;
        info!("live_guard: operator acknowledged supervision");
    }

    /// Check if a proposed trade passes the live validation guard.
    pub fn check(
        &self,
        asset: Asset,
        duration: MarketDuration,
        notional: Decimal,
        current_exposure: Decimal,
        current_positions: u32,
    ) -> Result<(), LiveGuardReject> {
        // Check operator ack
        if !self.operator_acked {
            warn!("live_guard: operator not acked");
            return Err(LiveGuardReject::OperatorNotAcked);
        }

        // Check asset allowed
        if !self.config.allowed_assets.is_empty() && !self.config.allowed_assets.contains(&asset) {
            warn!(asset = %asset, "live_guard: asset not allowed");
            return Err(LiveGuardReject::AssetNotAllowed { asset });
        }

        // Check duration allowed
        if !self.config.allowed_durations.is_empty()
            && !self.config.allowed_durations.contains(&duration)
        {
            warn!(duration = %duration, "live_guard: duration not allowed");
            return Err(LiveGuardReject::DurationNotAllowed { duration });
        }

        // Check notional
        if notional > self.config.max_notional {
            warn!(
                notional = %notional,
                limit = %self.config.max_notional,
                "live_guard: notional too large"
            );
            return Err(LiveGuardReject::NotionalTooLarge {
                requested: notional,
                limit: self.config.max_notional,
            });
        }

        // Check exposure
        let new_exposure = current_exposure + notional;
        if new_exposure > self.config.max_gross_exposure {
            return Err(LiveGuardReject::ExposureLimitExceeded {
                current: new_exposure,
                limit: self.config.max_gross_exposure,
            });
        }

        // Check concurrent positions
        if current_positions >= self.config.max_concurrent {
            return Err(LiveGuardReject::ConcurrentLimitExceeded {
                current: current_positions,
                limit: self.config.max_concurrent,
            });
        }

        Ok(())
    }

    /// Apply live validation overrides to a risk config.
    /// Returns a new risk config with the tighter limits.
    pub fn apply_overrides(&self, base: &RiskConfig) -> RiskConfig {
        RiskConfig {
            max_notional_per_order: self.config.max_notional.min(base.max_notional_per_order),
            max_concurrent_positions: self
                .config
                .max_concurrent
                .min(base.max_concurrent_positions),
            max_gross_exposure: self.config.max_gross_exposure.min(base.max_gross_exposure),
            max_daily_drawdown: self.config.max_daily_loss.min(base.max_daily_drawdown),
            max_consecutive_losses: self
                .config
                .max_consecutive_losses
                .min(base.max_consecutive_losses),
            // Keep base values for these
            max_position_per_market: self.config.max_notional.min(base.max_position_per_market),
            max_total_drawdown: base.max_total_drawdown,
        }
    }

    pub fn config(&self) -> &LiveValidationConfig {
        &self.config
    }

    pub fn is_operator_acked(&self) -> bool {
        self.operator_acked
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_guard() -> LiveGuard {
        let mut guard = LiveGuard::new(LiveValidationConfig::default());
        guard.operator_ack();
        guard
    }

    #[test]
    fn passes_within_limits() {
        let guard = default_guard();
        assert!(
            guard
                .check(Asset::BTC, MarketDuration::FiveMin, dec!(3), dec!(0), 0)
                .is_ok()
        );
    }

    #[test]
    fn rejects_without_operator_ack() {
        let guard = LiveGuard::new(LiveValidationConfig::default());
        let result = guard.check(Asset::BTC, MarketDuration::FiveMin, dec!(3), dec!(0), 0);
        assert!(matches!(result, Err(LiveGuardReject::OperatorNotAcked)));
    }

    #[test]
    fn rejects_large_notional() {
        let guard = default_guard();
        let result = guard.check(Asset::BTC, MarketDuration::FiveMin, dec!(10), dec!(0), 0);
        assert!(matches!(
            result,
            Err(LiveGuardReject::NotionalTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_disallowed_asset() {
        let guard = default_guard(); // Only BTC allowed by default
        let result = guard.check(Asset::ETH, MarketDuration::FiveMin, dec!(3), dec!(0), 0);
        assert!(matches!(
            result,
            Err(LiveGuardReject::AssetNotAllowed { .. })
        ));
    }

    #[test]
    fn rejects_disallowed_duration() {
        let guard = default_guard(); // Only 5m allowed by default
        let result = guard.check(Asset::BTC, MarketDuration::FifteenMin, dec!(3), dec!(0), 0);
        assert!(matches!(
            result,
            Err(LiveGuardReject::DurationNotAllowed { .. })
        ));
    }

    #[test]
    fn rejects_exposure_exceeded() {
        let guard = default_guard(); // max exposure = 10
        let result = guard.check(Asset::BTC, MarketDuration::FiveMin, dec!(5), dec!(8), 0);
        assert!(matches!(
            result,
            Err(LiveGuardReject::ExposureLimitExceeded { .. })
        ));
    }

    #[test]
    fn rejects_concurrent_exceeded() {
        let guard = default_guard(); // max concurrent = 1
        let result = guard.check(Asset::BTC, MarketDuration::FiveMin, dec!(3), dec!(0), 1);
        assert!(matches!(
            result,
            Err(LiveGuardReject::ConcurrentLimitExceeded { .. })
        ));
    }

    #[test]
    fn apply_overrides_uses_tighter_limits() {
        let guard = default_guard();
        let base = RiskConfig {
            max_notional_per_order: dec!(25),
            max_concurrent_positions: 4,
            max_gross_exposure: dec!(200),
            max_daily_drawdown: dec!(50),
            max_total_drawdown: dec!(100),
            max_consecutive_losses: 5,
            max_position_per_market: dec!(50),
        };
        let overridden = guard.apply_overrides(&base);
        assert_eq!(overridden.max_notional_per_order, dec!(5));
        assert_eq!(overridden.max_concurrent_positions, 1);
        assert_eq!(overridden.max_gross_exposure, dec!(10));
    }

    #[test]
    fn allow_all_assets_when_empty() {
        let config = LiveValidationConfig {
            allowed_assets: Vec::new(), // empty = all allowed
            ..LiveValidationConfig::default()
        };
        let mut guard = LiveGuard::new(config);
        guard.operator_ack();
        assert!(
            guard
                .check(Asset::ETH, MarketDuration::FiveMin, dec!(3), dec!(0), 0)
                .is_ok()
        );
    }

    #[test]
    fn reject_reason_display() {
        let reason = LiveGuardReject::NotionalTooLarge {
            requested: dec!(10),
            limit: dec!(5),
        };
        assert_eq!(reason.to_string(), "notional_too_large(10/5)");
    }
}
