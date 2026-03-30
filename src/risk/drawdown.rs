//! Drawdown tracking: daily, session, total, consecutive loss.
//!
//! Tracks from official P&L events. Thresholds are configurable.
//! Connects to the kill switch when breached.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::config::RiskConfig;
use crate::risk::kill_switch::{KillSwitch, KillSwitchReason};

// ---------------------------------------------------------------------------
// Drawdown state
// ---------------------------------------------------------------------------

/// Comprehensive drawdown and loss tracking state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawdownState {
    /// Starting equity for the session.
    pub session_start_equity: Decimal,
    /// Starting equity for the current day.
    pub daily_start_equity: Decimal,
    /// Current equity.
    pub current_equity: Decimal,
    /// High-water mark (peak equity since session start).
    pub high_water_mark: Decimal,

    /// Total realized P&L for the session.
    pub session_pnl: Decimal,
    /// Total realized P&L for the current day.
    pub daily_pnl: Decimal,

    /// Current consecutive loss count.
    pub consecutive_losses: u32,
    /// Peak consecutive losses in this session.
    pub max_consecutive_losses: u32,

    /// Total number of trades.
    pub total_trades: u32,
    /// Total winning trades.
    pub winning_trades: u32,
    /// Total losing trades.
    pub losing_trades: u32,

    /// Total adverse slippage.
    pub total_adverse_slippage: Decimal,
    /// Adverse slippage events in the current window.
    pub recent_adverse_slippage_count: u32,

    /// When the daily tracking was last reset.
    pub daily_reset_at: DateTime<Utc>,
}

/// Snapshot of drawdown metrics for dashboard/logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawdownSnapshot {
    pub daily_drawdown: Decimal,
    pub session_drawdown: Decimal,
    pub total_drawdown: Decimal,
    pub consecutive_losses: u32,
    pub win_rate: Decimal,
    pub daily_pnl: Decimal,
    pub session_pnl: Decimal,
}

// ---------------------------------------------------------------------------
// Drawdown tracker
// ---------------------------------------------------------------------------

/// Tracks drawdown and loss metrics. Updates on every P&L event.
pub struct DrawdownTracker {
    state: DrawdownState,
    config: RiskConfig,
    kill_switch: KillSwitch,
}

impl DrawdownTracker {
    pub fn new(starting_equity: Decimal, config: RiskConfig, kill_switch: KillSwitch) -> Self {
        let now = Utc::now();
        Self {
            state: DrawdownState {
                session_start_equity: starting_equity,
                daily_start_equity: starting_equity,
                current_equity: starting_equity,
                high_water_mark: starting_equity,
                session_pnl: Decimal::ZERO,
                daily_pnl: Decimal::ZERO,
                consecutive_losses: 0,
                max_consecutive_losses: 0,
                total_trades: 0,
                winning_trades: 0,
                losing_trades: 0,
                total_adverse_slippage: Decimal::ZERO,
                recent_adverse_slippage_count: 0,
                daily_reset_at: now,
            },
            config,
            kill_switch,
        }
    }

    /// Record a trade result (realized P&L after fees).
    pub fn record_trade(&mut self, pnl: Decimal, adverse_slippage: Option<Decimal>) {
        self.state.total_trades += 1;
        self.state.session_pnl += pnl;
        self.state.daily_pnl += pnl;
        self.state.current_equity += pnl;

        // Update high-water mark
        if self.state.current_equity > self.state.high_water_mark {
            self.state.high_water_mark = self.state.current_equity;
        }

        // Win/loss tracking
        if pnl > Decimal::ZERO {
            self.state.winning_trades += 1;
            self.state.consecutive_losses = 0;
        } else if pnl < Decimal::ZERO {
            self.state.losing_trades += 1;
            self.state.consecutive_losses += 1;
            if self.state.consecutive_losses > self.state.max_consecutive_losses {
                self.state.max_consecutive_losses = self.state.consecutive_losses;
            }
        }
        // pnl == 0 is a scratch, doesn't affect streak

        // Adverse slippage tracking
        if let Some(slip) = adverse_slippage
            && slip > Decimal::ZERO
        {
            self.state.total_adverse_slippage += slip;
            self.state.recent_adverse_slippage_count += 1;
        }

        debug!(
            pnl = %pnl,
            equity = %self.state.current_equity,
            daily_pnl = %self.state.daily_pnl,
            consec_losses = self.state.consecutive_losses,
            "drawdown_trade_recorded"
        );

        // Check thresholds
        self.check_thresholds();
    }

    /// Check all drawdown thresholds and trigger kill switch if breached.
    fn check_thresholds(&self) {
        // Daily drawdown
        let daily_dd = self.daily_drawdown();
        if daily_dd >= self.config.max_daily_drawdown {
            warn!(
                daily_dd = %daily_dd,
                limit = %self.config.max_daily_drawdown,
                "daily_drawdown_breach"
            );
            self.kill_switch
                .activate(KillSwitchReason::DailyDrawdownBreach {
                    drawdown: daily_dd.to_string(),
                    limit: self.config.max_daily_drawdown.to_string(),
                });
        }

        // Total drawdown (from high-water mark)
        let total_dd = self.total_drawdown();
        if total_dd >= self.config.max_total_drawdown {
            warn!(
                total_dd = %total_dd,
                limit = %self.config.max_total_drawdown,
                "total_drawdown_breach"
            );
            self.kill_switch
                .activate(KillSwitchReason::TotalDrawdownBreach {
                    drawdown: total_dd.to_string(),
                    limit: self.config.max_total_drawdown.to_string(),
                });
        }

        // Consecutive losses
        if self.state.consecutive_losses >= self.config.max_consecutive_losses {
            warn!(
                count = self.state.consecutive_losses,
                limit = self.config.max_consecutive_losses,
                "consecutive_loss_breach"
            );
            self.kill_switch
                .activate(KillSwitchReason::ConsecutiveLossBreach {
                    count: self.state.consecutive_losses,
                    limit: self.config.max_consecutive_losses,
                });
        }
    }

    /// Daily drawdown: how much we've lost from the daily start.
    pub fn daily_drawdown(&self) -> Decimal {
        (self.state.daily_start_equity - self.state.current_equity).max(Decimal::ZERO)
    }

    /// Session drawdown: how much we've lost from the session start.
    pub fn session_drawdown(&self) -> Decimal {
        (self.state.session_start_equity - self.state.current_equity).max(Decimal::ZERO)
    }

    /// Total drawdown: from high-water mark.
    pub fn total_drawdown(&self) -> Decimal {
        (self.state.high_water_mark - self.state.current_equity).max(Decimal::ZERO)
    }

    /// Win rate as a decimal (0.0 to 1.0).
    pub fn win_rate(&self) -> Decimal {
        if self.state.total_trades == 0 {
            return Decimal::ZERO;
        }
        Decimal::from(self.state.winning_trades) / Decimal::from(self.state.total_trades)
    }

    /// Get a snapshot of current drawdown metrics.
    pub fn snapshot(&self) -> DrawdownSnapshot {
        DrawdownSnapshot {
            daily_drawdown: self.daily_drawdown(),
            session_drawdown: self.session_drawdown(),
            total_drawdown: self.total_drawdown(),
            consecutive_losses: self.state.consecutive_losses,
            win_rate: self.win_rate(),
            daily_pnl: self.state.daily_pnl,
            session_pnl: self.state.session_pnl,
        }
    }

    /// Reset daily tracking (call at start of new day).
    pub fn reset_daily(&mut self) {
        info!(
            daily_pnl = %self.state.daily_pnl,
            "drawdown_daily_reset"
        );
        self.state.daily_start_equity = self.state.current_equity;
        self.state.daily_pnl = Decimal::ZERO;
        self.state.daily_reset_at = Utc::now();
        self.state.recent_adverse_slippage_count = 0;
    }

    /// Get full state (for persistence/telemetry).
    pub fn state(&self) -> &DrawdownState {
        &self.state
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn test_config() -> RiskConfig {
        RiskConfig {
            max_position_per_market: dec!(50),
            max_concurrent_positions: 4,
            max_gross_exposure: dec!(200),
            max_daily_drawdown: dec!(50),
            max_total_drawdown: dec!(100),
            max_consecutive_losses: 3,
            max_notional_per_order: dec!(25),
        }
    }

    fn make_tracker() -> (DrawdownTracker, KillSwitch) {
        let ks = KillSwitch::new();
        let tracker = DrawdownTracker::new(dec!(1000), test_config(), ks.clone());
        (tracker, ks)
    }

    #[test]
    fn starts_with_zero_drawdown() {
        let (tracker, _) = make_tracker();
        assert_eq!(tracker.daily_drawdown(), Decimal::ZERO);
        assert_eq!(tracker.session_drawdown(), Decimal::ZERO);
        assert_eq!(tracker.total_drawdown(), Decimal::ZERO);
    }

    #[test]
    fn winning_trade_no_drawdown() {
        let (mut tracker, _) = make_tracker();
        tracker.record_trade(dec!(10), None);
        assert_eq!(tracker.daily_drawdown(), Decimal::ZERO);
        assert_eq!(tracker.state.winning_trades, 1);
        assert_eq!(tracker.state.consecutive_losses, 0);
    }

    #[test]
    fn losing_trade_increases_drawdown() {
        let (mut tracker, _) = make_tracker();
        tracker.record_trade(dec!(-20), None);
        assert_eq!(tracker.daily_drawdown(), dec!(20));
        assert_eq!(tracker.session_drawdown(), dec!(20));
        assert_eq!(tracker.state.losing_trades, 1);
        assert_eq!(tracker.state.consecutive_losses, 1);
    }

    #[test]
    fn consecutive_losses_tracked() {
        let (mut tracker, _) = make_tracker();
        tracker.record_trade(dec!(-5), None);
        tracker.record_trade(dec!(-5), None);
        assert_eq!(tracker.state.consecutive_losses, 2);

        // Win resets streak
        tracker.record_trade(dec!(10), None);
        assert_eq!(tracker.state.consecutive_losses, 0);
    }

    #[test]
    fn daily_drawdown_triggers_kill_switch() {
        let (mut tracker, ks) = make_tracker();
        // Max daily DD is 50
        tracker.record_trade(dec!(-55), None);
        assert!(ks.is_active());
    }

    #[test]
    fn total_drawdown_triggers_kill_switch() {
        let (mut tracker, ks) = make_tracker();
        // Max total DD is 100
        tracker.record_trade(dec!(-105), None);
        assert!(ks.is_active());
    }

    #[test]
    fn consecutive_loss_triggers_kill_switch() {
        let (mut tracker, ks) = make_tracker();
        // Max consecutive is 3
        tracker.record_trade(dec!(-1), None);
        tracker.record_trade(dec!(-1), None);
        assert!(!ks.is_active());
        tracker.record_trade(dec!(-1), None);
        assert!(ks.is_active());
    }

    #[test]
    fn high_water_mark_tracks_peak() {
        let (mut tracker, _) = make_tracker();
        tracker.record_trade(dec!(50), None); // equity = 1050
        tracker.record_trade(dec!(-20), None); // equity = 1030
        assert_eq!(tracker.state.high_water_mark, dec!(1050));
        assert_eq!(tracker.total_drawdown(), dec!(20));
    }

    #[test]
    fn win_rate_calculation() {
        let (mut tracker, _) = make_tracker();
        tracker.record_trade(dec!(10), None);
        tracker.record_trade(dec!(-5), None);
        tracker.record_trade(dec!(10), None);
        tracker.record_trade(dec!(-5), None);
        assert_eq!(tracker.win_rate(), dec!(0.5));
    }

    #[test]
    fn daily_reset() {
        let (mut tracker, _) = make_tracker();
        tracker.record_trade(dec!(-20), None);
        assert_eq!(tracker.daily_drawdown(), dec!(20));

        tracker.reset_daily();
        assert_eq!(tracker.daily_drawdown(), Decimal::ZERO);
        assert_eq!(tracker.state.daily_pnl, Decimal::ZERO);
    }

    #[test]
    fn snapshot() {
        let (mut tracker, _) = make_tracker();
        tracker.record_trade(dec!(10), None);
        tracker.record_trade(dec!(-5), None);

        let snap = tracker.snapshot();
        assert_eq!(snap.daily_pnl, dec!(5));
        assert_eq!(snap.session_pnl, dec!(5));
        assert_eq!(snap.consecutive_losses, 1);
    }

    #[test]
    fn adverse_slippage_tracked() {
        let (mut tracker, _) = make_tracker();
        tracker.record_trade(dec!(-5), Some(dec!(0.02)));
        assert_eq!(tracker.state.total_adverse_slippage, dec!(0.02));
        assert_eq!(tracker.state.recent_adverse_slippage_count, 1);
    }

    #[test]
    fn scratch_trade_no_streak_change() {
        let (mut tracker, _) = make_tracker();
        tracker.record_trade(dec!(-5), None);
        assert_eq!(tracker.state.consecutive_losses, 1);
        tracker.record_trade(Decimal::ZERO, None); // scratch
        assert_eq!(tracker.state.consecutive_losses, 1); // unchanged
    }
}
