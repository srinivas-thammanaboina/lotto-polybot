//! Evaluation reports for strategy validation.
//!
//! Generates reports from simulation/shadow data to decide whether
//! the strategy deserves live deployment. Outputs JSON/markdown summaries.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use crate::simulation::engine::SimulationStats;
use crate::telemetry::ledger::{LedgerEntry, LedgerSummary};

// ---------------------------------------------------------------------------
// Evaluation report
// ---------------------------------------------------------------------------

/// Top-level evaluation report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationReport {
    pub generated_at: DateTime<Utc>,
    pub data_source: String,
    pub overall: OverallMetrics,
    pub by_duration: HashMap<String, DurationMetrics>,
    pub edge_analysis: EdgeAnalysis,
    pub signal_analysis: SignalAnalysis,
    pub recommendation: Recommendation,
}

/// Overall performance metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverallMetrics {
    pub total_trades: u64,
    pub win_rate: Decimal,
    pub total_pnl: Decimal,
    pub total_fees: Decimal,
    pub net_pnl: Decimal,
    pub avg_pnl_per_trade: Decimal,
    pub max_drawdown: Decimal,
    pub sharpe_estimate: Decimal,
}

/// Per-duration metrics (5m vs 15m).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurationMetrics {
    pub duration: String,
    pub trades: u64,
    pub win_rate: Decimal,
    pub total_pnl: Decimal,
    pub avg_edge: Decimal,
    pub avg_size: Decimal,
}

/// Edge analysis: gross vs net.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeAnalysis {
    pub avg_gross_edge: Decimal,
    pub avg_net_edge: Decimal,
    pub edge_capture_rate: Decimal,
    pub cost_as_pct_of_gross: Decimal,
}

/// Signal pass/fail analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalAnalysis {
    pub total_evaluated: u64,
    pub acceptance_rate: Decimal,
    pub top_reject_reasons: Vec<(String, u64)>,
}

/// Go/no-go recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub go_live: bool,
    pub confidence: String,
    pub reasons: Vec<String>,
}

// ---------------------------------------------------------------------------
// Report builder
// ---------------------------------------------------------------------------

/// Builds evaluation reports from simulation data.
pub struct ReportBuilder;

impl ReportBuilder {
    /// Build a report from a ledger summary and simulation stats.
    pub fn from_simulation(
        summary: &LedgerSummary,
        sim_stats: &SimulationStats,
        entries: &[LedgerEntry],
    ) -> EvaluationReport {
        let overall = Self::compute_overall(summary, entries);
        let by_duration = Self::compute_by_duration(entries);
        let edge_analysis = Self::compute_edge_analysis(entries);
        let signal_analysis = Self::compute_signal_analysis(sim_stats);
        let recommendation = Self::compute_recommendation(&overall, &edge_analysis);

        EvaluationReport {
            generated_at: Utc::now(),
            data_source: format!("simulation ({})", summary.mode),
            overall,
            by_duration,
            edge_analysis,
            signal_analysis,
            recommendation,
        }
    }

    fn compute_overall(summary: &LedgerSummary, entries: &[LedgerEntry]) -> OverallMetrics {
        let total = summary.total_trades;
        let win_rate = if total > 0 {
            Decimal::from(summary.win_count) / Decimal::from(total)
        } else {
            Decimal::ZERO
        };
        let net_pnl = summary.total_realized_pnl - summary.total_fees;
        let avg_pnl = if total > 0 {
            net_pnl / Decimal::from(total)
        } else {
            Decimal::ZERO
        };

        // Simple max drawdown from equity curve
        let mut peak = Decimal::ZERO;
        let mut max_dd = Decimal::ZERO;
        let mut equity = Decimal::ZERO;
        for entry in entries {
            if let Some(pnl) = entry.realized_pnl {
                equity += pnl;
                if equity > peak {
                    peak = equity;
                }
                let dd = peak - equity;
                if dd > max_dd {
                    max_dd = dd;
                }
            }
        }

        // Simple Sharpe estimate: avg / stddev (annualised is not meaningful for short data)
        let pnls: Vec<Decimal> = entries.iter().filter_map(|e| e.realized_pnl).collect();
        let sharpe = Self::simple_sharpe(&pnls);

        OverallMetrics {
            total_trades: total,
            win_rate,
            total_pnl: summary.total_realized_pnl,
            total_fees: summary.total_fees,
            net_pnl,
            avg_pnl_per_trade: avg_pnl,
            max_drawdown: max_dd,
            sharpe_estimate: sharpe,
        }
    }

    fn simple_sharpe(pnls: &[Decimal]) -> Decimal {
        if pnls.len() < 2 {
            return Decimal::ZERO;
        }
        let n = Decimal::from(pnls.len() as u64);
        let sum: Decimal = pnls.iter().sum();
        let mean = sum / n;
        let var_sum: Decimal = pnls.iter().map(|p| (*p - mean) * (*p - mean)).sum();
        let variance = var_sum / (n - dec!(1));
        if variance <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        // Approximate sqrt via Newton's method (3 iterations)
        let mut x = variance;
        for _ in 0..10 {
            if x <= Decimal::ZERO {
                return Decimal::ZERO;
            }
            x = (x + variance / x) / dec!(2);
        }
        let stddev = x;
        if stddev <= Decimal::ZERO {
            Decimal::ZERO
        } else {
            mean / stddev
        }
    }

    fn compute_by_duration(_entries: &[LedgerEntry]) -> HashMap<String, DurationMetrics> {
        // Without duration info on ledger entries, we return an empty map.
        // In a real implementation, entries would carry duration metadata.
        HashMap::new()
    }

    fn compute_edge_analysis(_entries: &[LedgerEntry]) -> EdgeAnalysis {
        // Edge data comes from the intent's cost snapshot, which isn't
        // stored on ledger entries in the current model. Return defaults.
        EdgeAnalysis {
            avg_gross_edge: Decimal::ZERO,
            avg_net_edge: Decimal::ZERO,
            edge_capture_rate: Decimal::ZERO,
            cost_as_pct_of_gross: Decimal::ZERO,
        }
    }

    fn compute_signal_analysis(stats: &SimulationStats) -> SignalAnalysis {
        let total = stats.signals_accepted + stats.signals_rejected;
        let acceptance_rate = if total > 0 {
            Decimal::from(stats.signals_accepted) / Decimal::from(total)
        } else {
            Decimal::ZERO
        };

        SignalAnalysis {
            total_evaluated: total,
            acceptance_rate,
            top_reject_reasons: Vec::new(), // Would need per-reason counters
        }
    }

    fn compute_recommendation(overall: &OverallMetrics, _edge: &EdgeAnalysis) -> Recommendation {
        let mut reasons = Vec::new();
        let mut go = true;

        if overall.total_trades < 20 {
            reasons.push("insufficient sample size (< 20 trades)".into());
            go = false;
        }

        if overall.win_rate < dec!(0.45) {
            reasons.push(format!("low win rate: {}", overall.win_rate));
            go = false;
        }

        if overall.net_pnl <= Decimal::ZERO {
            reasons.push(format!("negative net P&L: {}", overall.net_pnl));
            go = false;
        }

        if overall.sharpe_estimate < dec!(0.5) {
            reasons.push(format!("low Sharpe: {}", overall.sharpe_estimate));
        }

        if overall.max_drawdown > dec!(50) {
            reasons.push(format!("high max drawdown: {}", overall.max_drawdown));
            go = false;
        }

        if go {
            reasons.push("all basic criteria met".into());
        }

        let confidence = if overall.total_trades >= 100 {
            "high"
        } else if overall.total_trades >= 50 {
            "medium"
        } else {
            "low"
        };

        Recommendation {
            go_live: go,
            confidence: confidence.into(),
            reasons,
        }
    }

    /// Render the report as a markdown string.
    pub fn to_markdown(report: &EvaluationReport) -> String {
        let mut out = String::new();
        out.push_str("# Strategy Evaluation Report\n\n");
        out.push_str(&format!(
            "Generated: {}  \nSource: {}\n\n",
            report.generated_at.format("%Y-%m-%d %H:%M UTC"),
            report.data_source
        ));

        out.push_str("## Overall\n\n");
        out.push_str("| Metric | Value |\n|--------|-------|\n");
        out.push_str(&format!("| Trades | {} |\n", report.overall.total_trades));
        out.push_str(&format!("| Win Rate | {} |\n", report.overall.win_rate));
        out.push_str(&format!("| Net P&L | {} |\n", report.overall.net_pnl));
        out.push_str(&format!("| Fees | {} |\n", report.overall.total_fees));
        out.push_str(&format!(
            "| Avg P&L/Trade | {} |\n",
            report.overall.avg_pnl_per_trade
        ));
        out.push_str(&format!(
            "| Max Drawdown | {} |\n",
            report.overall.max_drawdown
        ));
        out.push_str(&format!(
            "| Sharpe | {} |\n\n",
            report.overall.sharpe_estimate
        ));

        out.push_str("## Recommendation\n\n");
        let verdict = if report.recommendation.go_live {
            "GO"
        } else {
            "NO-GO"
        };
        out.push_str(&format!(
            "**{}** (confidence: {})\n\n",
            verdict, report.recommendation.confidence
        ));
        for reason in &report.recommendation.reasons {
            out.push_str(&format!("- {reason}\n"));
        }

        out
    }

    /// Render as JSON.
    pub fn to_json(report: &EvaluationReport) -> String {
        serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".into())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_sim_stats() -> SimulationStats {
        SimulationStats {
            mode: "simulation".into(),
            signals_evaluated: 100,
            signals_accepted: 60,
            signals_rejected: 40,
            fills_simulated: 55,
            fills_missed: 5,
            total_pnl: dec!(25),
            total_fees: dec!(5),
            started_at: Utc::now(),
            events_processed: 5000,
        }
    }

    fn test_summary() -> LedgerSummary {
        LedgerSummary {
            mode: "simulation".into(),
            total_trades: 55,
            open_trades: 5,
            closed_trades: 50,
            verified_trades: 0,
            total_realized_pnl: dec!(25),
            total_official_pnl: Decimal::ZERO,
            total_fees: dec!(5),
            win_count: 30,
            loss_count: 20,
        }
    }

    #[test]
    fn report_generated() {
        let report = ReportBuilder::from_simulation(&test_summary(), &test_sim_stats(), &[]);
        assert!(!report.data_source.is_empty());
        assert_eq!(report.overall.total_trades, 55);
    }

    #[test]
    fn win_rate_computed() {
        let report = ReportBuilder::from_simulation(&test_summary(), &test_sim_stats(), &[]);
        // 30 / 55 ≈ 0.545
        assert!(report.overall.win_rate > dec!(0.5));
    }

    #[test]
    fn acceptance_rate_computed() {
        let report = ReportBuilder::from_simulation(&test_summary(), &test_sim_stats(), &[]);
        // 60 / 100 = 0.6
        assert_eq!(report.signal_analysis.acceptance_rate, dec!(0.6));
    }

    #[test]
    fn recommendation_go_with_enough_data() {
        let report = ReportBuilder::from_simulation(&test_summary(), &test_sim_stats(), &[]);
        // 55 trades, 54% win rate, positive PnL → should be go
        assert!(report.recommendation.go_live);
    }

    #[test]
    fn recommendation_nogo_insufficient_trades() {
        let mut summary = test_summary();
        summary.total_trades = 5;
        summary.win_count = 3;
        summary.loss_count = 2;
        let report = ReportBuilder::from_simulation(&summary, &test_sim_stats(), &[]);
        assert!(!report.recommendation.go_live);
    }

    #[test]
    fn markdown_output() {
        let report = ReportBuilder::from_simulation(&test_summary(), &test_sim_stats(), &[]);
        let md = ReportBuilder::to_markdown(&report);
        assert!(md.contains("Strategy Evaluation Report"));
        assert!(md.contains("Win Rate"));
    }

    #[test]
    fn json_output_valid() {
        let report = ReportBuilder::from_simulation(&test_summary(), &test_sim_stats(), &[]);
        let json = ReportBuilder::to_json(&report);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["overall"]["total_trades"].is_number());
    }

    #[test]
    fn sharpe_with_data() {
        let pnls = vec![dec!(1), dec!(-0.5), dec!(2), dec!(-1), dec!(1.5)];
        let sharpe = ReportBuilder::simple_sharpe(&pnls);
        assert!(sharpe > Decimal::ZERO);
    }

    #[test]
    fn sharpe_empty() {
        assert_eq!(ReportBuilder::simple_sharpe(&[]), Decimal::ZERO);
    }
}
