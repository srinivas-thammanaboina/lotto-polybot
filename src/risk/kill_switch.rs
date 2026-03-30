//! Top-level kill switch for emergency halt.
//!
//! Sits above strategy and execution. When active, blocks all new trade entry.
//! Can be triggered manually or automatically by risk conditions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Kill switch trigger reason
// ---------------------------------------------------------------------------

/// Why the kill switch was activated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KillSwitchReason {
    /// Operator manually triggered.
    Manual,
    /// Daily drawdown limit breached.
    DailyDrawdownBreach { drawdown: String, limit: String },
    /// Total drawdown limit breached.
    TotalDrawdownBreach { drawdown: String, limit: String },
    /// Consecutive loss limit breached.
    ConsecutiveLossBreach { count: u32, limit: u32 },
    /// All required feeds are stale.
    StaleFeedRegime,
    /// Abnormal latency detected.
    AbnormalLatency { avg_ms: u64 },
    /// Repeated execution failures.
    ExecutionFailures { count: u32, window_secs: u64 },
    /// Unresolved order-state anomaly.
    OrderStateAnomaly { uncertain_count: usize },
    /// Reconnect storm (too many reconnects in short window).
    ReconnectStorm { count: u32, window_secs: u64 },
}

impl std::fmt::Display for KillSwitchReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KillSwitchReason::Manual => write!(f, "manual"),
            KillSwitchReason::DailyDrawdownBreach { drawdown, limit } => {
                write!(f, "daily_dd_breach({drawdown}/{limit})")
            }
            KillSwitchReason::TotalDrawdownBreach { drawdown, limit } => {
                write!(f, "total_dd_breach({drawdown}/{limit})")
            }
            KillSwitchReason::ConsecutiveLossBreach { count, limit } => {
                write!(f, "consec_loss_breach({count}/{limit})")
            }
            KillSwitchReason::StaleFeedRegime => write!(f, "stale_feed_regime"),
            KillSwitchReason::AbnormalLatency { avg_ms } => {
                write!(f, "abnormal_latency({avg_ms}ms)")
            }
            KillSwitchReason::ExecutionFailures { count, window_secs } => {
                write!(f, "exec_failures({count} in {window_secs}s)")
            }
            KillSwitchReason::OrderStateAnomaly { uncertain_count } => {
                write!(f, "order_anomaly({uncertain_count} uncertain)")
            }
            KillSwitchReason::ReconnectStorm { count, window_secs } => {
                write!(f, "reconnect_storm({count} in {window_secs}s)")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Kill switch activation record
// ---------------------------------------------------------------------------

/// Record of a kill switch activation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KillSwitchActivation {
    pub reason: KillSwitchReason,
    pub activated_at: DateTime<Utc>,
    pub deactivated_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Kill switch
// ---------------------------------------------------------------------------

/// Top-level kill switch. Thread-safe, lock-free for the hot-path check.
#[derive(Debug)]
pub struct KillSwitch {
    /// Fast atomic check — no lock needed on hot path.
    active: Arc<AtomicBool>,
    /// Full state with reason and history (behind lock for writes only).
    state: Arc<RwLock<KillSwitchState>>,
}

#[derive(Debug)]
struct KillSwitchState {
    current: Option<KillSwitchActivation>,
    history: Vec<KillSwitchActivation>,
}

impl KillSwitch {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            state: Arc::new(RwLock::new(KillSwitchState {
                current: None,
                history: Vec::new(),
            })),
        }
    }

    /// Check if the kill switch is active (lock-free, safe for hot path).
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Activate the kill switch with a reason.
    pub fn activate(&self, reason: KillSwitchReason) {
        let was_active = self.active.swap(true, Ordering::Release);

        let activation = KillSwitchActivation {
            reason: reason.clone(),
            activated_at: Utc::now(),
            deactivated_at: None,
        };

        let mut state = self.state.write();
        state.current = Some(activation.clone());
        state.history.push(activation);

        if !was_active {
            error!(
                reason = %reason,
                "KILL_SWITCH_ACTIVATED — all trading halted"
            );
        } else {
            warn!(
                reason = %reason,
                "kill_switch_re_triggered (already active)"
            );
        }
    }

    /// Deactivate the kill switch (operator action).
    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
        let mut state = self.state.write();
        if let Some(ref mut current) = state.current {
            current.deactivated_at = Some(Utc::now());
        }
        state.current = None;
        info!("kill_switch_deactivated");
    }

    /// Get the current activation reason (if active).
    pub fn current_reason(&self) -> Option<KillSwitchReason> {
        self.state.read().current.as_ref().map(|a| a.reason.clone())
    }

    /// Get the activation history.
    pub fn history(&self) -> Vec<KillSwitchActivation> {
        self.state.read().history.clone()
    }

    /// Total number of activations.
    pub fn activation_count(&self) -> usize {
        self.state.read().history.len()
    }
}

impl Default for KillSwitch {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for KillSwitch {
    fn clone(&self) -> Self {
        Self {
            active: Arc::clone(&self.active),
            state: Arc::clone(&self.state),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_inactive() {
        let ks = KillSwitch::new();
        assert!(!ks.is_active());
        assert!(ks.current_reason().is_none());
    }

    #[test]
    fn activate_blocks_trading() {
        let ks = KillSwitch::new();
        ks.activate(KillSwitchReason::Manual);
        assert!(ks.is_active());
        assert!(ks.current_reason().is_some());
    }

    #[test]
    fn deactivate_resumes_trading() {
        let ks = KillSwitch::new();
        ks.activate(KillSwitchReason::Manual);
        ks.deactivate();
        assert!(!ks.is_active());
        assert!(ks.current_reason().is_none());
    }

    #[test]
    fn history_tracks_activations() {
        let ks = KillSwitch::new();
        ks.activate(KillSwitchReason::Manual);
        ks.deactivate();
        ks.activate(KillSwitchReason::StaleFeedRegime);
        assert_eq!(ks.activation_count(), 2);
        assert_eq!(ks.history().len(), 2);
    }

    #[test]
    fn re_trigger_while_active() {
        let ks = KillSwitch::new();
        ks.activate(KillSwitchReason::Manual);
        ks.activate(KillSwitchReason::StaleFeedRegime);
        assert!(ks.is_active());
        assert_eq!(ks.activation_count(), 2);
    }

    #[test]
    fn clone_shares_state() {
        let ks1 = KillSwitch::new();
        let ks2 = ks1.clone();
        ks1.activate(KillSwitchReason::Manual);
        assert!(ks2.is_active());
    }

    #[test]
    fn daily_drawdown_reason_display() {
        let reason = KillSwitchReason::DailyDrawdownBreach {
            drawdown: "55.0".into(),
            limit: "50.0".into(),
        };
        assert_eq!(reason.to_string(), "daily_dd_breach(55.0/50.0)");
    }

    #[test]
    fn consecutive_loss_reason_display() {
        let reason = KillSwitchReason::ConsecutiveLossBreach { count: 6, limit: 5 };
        assert_eq!(reason.to_string(), "consec_loss_breach(6/5)");
    }
}
