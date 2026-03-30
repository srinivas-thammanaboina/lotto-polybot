//! Terminal dashboard for runtime observability.
//!
//! Reads from in-memory state and produces a text snapshot.
//! Dashboard failure does not affect the core bot.

use std::fmt::Write;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Dashboard state — assembled from various subsystem snapshots
// ---------------------------------------------------------------------------

/// Feed health entry for dashboard display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardFeedStatus {
    pub name: String,
    pub healthy: bool,
    pub last_message: Option<DateTime<Utc>>,
    pub reconnects: u32,
}

/// Position entry for dashboard display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardPosition {
    pub contract: String,
    pub side: String,
    pub size: Decimal,
    pub entry_price: Decimal,
    pub unrealized_pnl: Decimal,
}

/// Recent decision for dashboard display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardDecision {
    pub timestamp: DateTime<Utc>,
    pub contract: String,
    pub decision: String,
    pub detail: String,
}

/// Complete dashboard state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardState {
    pub mode: String,
    pub region: String,
    pub uptime_secs: i64,

    // Feed health
    pub feeds: Vec<DashboardFeedStatus>,

    // Positions and orders
    pub active_positions: Vec<DashboardPosition>,
    pub open_order_count: u32,

    // P&L
    pub daily_pnl: Decimal,
    pub session_pnl: Decimal,
    pub daily_drawdown: Decimal,

    // Risk
    pub kill_switch_active: bool,
    pub kill_switch_reason: Option<String>,
    pub consecutive_losses: u32,
    pub gross_exposure: Decimal,

    // Metrics
    pub events_received: u64,
    pub signals_accepted: u64,
    pub signals_rejected: u64,
    pub orders_submitted: u64,
    pub fills_received: u64,

    // Recent decisions
    pub recent_decisions: Vec<DashboardDecision>,

    pub snapshot_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Dashboard renderer
// ---------------------------------------------------------------------------

/// Renders the dashboard state as a terminal-friendly text block.
pub struct DashboardRenderer;

impl DashboardRenderer {
    /// Render the dashboard state to a string.
    pub fn render(state: &DashboardState) -> String {
        let mut out = String::with_capacity(2048);

        // Header
        let _ = writeln!(
            out,
            "╔══════════════════════════════════════════════════════════╗"
        );
        let _ = writeln!(
            out,
            "║  poly-latency-bot  │  mode: {:<10} │  region: {:<6} ║",
            state.mode, state.region
        );
        let _ = writeln!(
            out,
            "║  uptime: {}s  │  {}",
            state.uptime_secs,
            state.snapshot_at.format("%H:%M:%S UTC")
        );
        let _ = writeln!(
            out,
            "╠══════════════════════════════════════════════════════════╣"
        );

        // Kill switch
        if state.kill_switch_active {
            let _ = writeln!(
                out,
                "║  !! KILL SWITCH ACTIVE: {} !!",
                state.kill_switch_reason.as_deref().unwrap_or("unknown")
            );
            let _ = writeln!(
                out,
                "╠══════════════════════════════════════════════════════════╣"
            );
        }

        // Feeds
        let _ = writeln!(out, "║  FEEDS:");
        for feed in &state.feeds {
            let status = if feed.healthy { "OK" } else { "!!" };
            let age = feed
                .last_message
                .map(|t| {
                    let ago = Utc::now() - t;
                    format!("{}ms ago", ago.num_milliseconds())
                })
                .unwrap_or_else(|| "never".into());
            let _ = writeln!(
                out,
                "║    [{status}] {:<16} last: {:<14} reconnects: {}",
                feed.name, age, feed.reconnects
            );
        }

        // P&L
        let _ = writeln!(
            out,
            "╠══════════════════════════════════════════════════════════╣"
        );
        let _ = writeln!(
            out,
            "║  P&L: daily={:<10} session={:<10} DD={}",
            state.daily_pnl, state.session_pnl, state.daily_drawdown
        );
        let _ = writeln!(
            out,
            "║  exposure={:<10} consec_losses={} orders_open={}",
            state.gross_exposure, state.consecutive_losses, state.open_order_count
        );

        // Positions
        if !state.active_positions.is_empty() {
            let _ = writeln!(
                out,
                "╠══════════════════════════════════════════════════════════╣"
            );
            let _ = writeln!(out, "║  POSITIONS:");
            for pos in &state.active_positions {
                let _ = writeln!(
                    out,
                    "║    {} {} size={} entry={} upnl={}",
                    pos.contract, pos.side, pos.size, pos.entry_price, pos.unrealized_pnl
                );
            }
        }

        // Metrics
        let _ = writeln!(
            out,
            "╠══════════════════════════════════════════════════════════╣"
        );
        let _ = writeln!(
            out,
            "║  events={} accepted={} rejected={} orders={} fills={}",
            state.events_received,
            state.signals_accepted,
            state.signals_rejected,
            state.orders_submitted,
            state.fills_received
        );

        // Recent decisions
        if !state.recent_decisions.is_empty() {
            let _ = writeln!(
                out,
                "╠══════════════════════════════════════════════════════════╣"
            );
            let _ = writeln!(out, "║  RECENT:");
            for d in state.recent_decisions.iter().rev().take(5) {
                let _ = writeln!(
                    out,
                    "║    {} {} {} {}",
                    d.timestamp.format("%H:%M:%S"),
                    d.contract,
                    d.decision,
                    d.detail
                );
            }
        }

        let _ = writeln!(
            out,
            "╚══════════════════════════════════════════════════════════╝"
        );

        out
    }

    /// Render as JSON (for programmatic consumers).
    pub fn render_json(state: &DashboardState) -> String {
        serde_json::to_string_pretty(state).unwrap_or_else(|_| "{}".to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn test_state() -> DashboardState {
        DashboardState {
            mode: "simulation".into(),
            region: "local".into(),
            uptime_secs: 120,
            feeds: vec![
                DashboardFeedStatus {
                    name: "binance".into(),
                    healthy: true,
                    last_message: Some(Utc::now()),
                    reconnects: 0,
                },
                DashboardFeedStatus {
                    name: "polymarket".into(),
                    healthy: false,
                    last_message: None,
                    reconnects: 3,
                },
            ],
            active_positions: vec![DashboardPosition {
                contract: "BTC-5m-UP".into(),
                side: "BUY".into(),
                size: dec!(10),
                entry_price: dec!(0.55),
                unrealized_pnl: dec!(0.50),
            }],
            open_order_count: 1,
            daily_pnl: dec!(5.50),
            session_pnl: dec!(12.30),
            daily_drawdown: dec!(2.00),
            kill_switch_active: false,
            kill_switch_reason: None,
            consecutive_losses: 0,
            gross_exposure: dec!(15.00),
            events_received: 5000,
            signals_accepted: 10,
            signals_rejected: 45,
            orders_submitted: 10,
            fills_received: 8,
            recent_decisions: vec![DashboardDecision {
                timestamp: Utc::now(),
                contract: "BTC-5m-UP".into(),
                decision: "ACCEPT".into(),
                detail: "edge=0.05".into(),
            }],
            snapshot_at: Utc::now(),
        }
    }

    #[test]
    fn render_produces_output() {
        let output = DashboardRenderer::render(&test_state());
        assert!(output.contains("poly-latency-bot"));
        assert!(output.contains("simulation"));
        assert!(output.contains("binance"));
        assert!(output.contains("BTC-5m-UP"));
    }

    #[test]
    fn render_shows_kill_switch_when_active() {
        let mut state = test_state();
        state.kill_switch_active = true;
        state.kill_switch_reason = Some("daily_dd_breach".into());
        let output = DashboardRenderer::render(&state);
        assert!(output.contains("KILL SWITCH ACTIVE"));
        assert!(output.contains("daily_dd_breach"));
    }

    #[test]
    fn render_json_is_valid() {
        let json = DashboardRenderer::render_json(&test_state());
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["mode"], "simulation");
    }

    #[test]
    fn empty_positions_no_section() {
        let mut state = test_state();
        state.active_positions.clear();
        let output = DashboardRenderer::render(&state);
        assert!(!output.contains("POSITIONS:"));
    }
}
