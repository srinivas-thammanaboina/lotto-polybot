//! Live vs shadow vs simulation comparison.
//!
//! Highlights where live reality differs from estimated behavior.
//! Focus on truth discovery, not vanity metrics.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Execution sample — one data point from any mode
// ---------------------------------------------------------------------------

/// A single execution observation from any mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSample {
    pub mode: String,
    pub timestamp: DateTime<Utc>,
    pub contract: String,
    pub side: String,

    /// Expected fill price (from signal intent).
    pub expected_price: Decimal,
    /// Actual fill price (or simulated fill price).
    pub actual_price: Option<Decimal>,

    /// Expected slippage from cost model.
    pub expected_slippage: Decimal,
    /// Actual slippage (actual_price - expected_price).
    pub actual_slippage: Option<Decimal>,

    /// Signal-to-fill latency in microseconds.
    pub fill_latency_us: Option<i64>,

    /// Net edge at signal time.
    pub expected_net_edge: Decimal,
    /// Realized P&L (after fill + resolution).
    pub realized_pnl: Option<Decimal>,

    /// Whether the fill happened at all.
    pub filled: bool,
}

// ---------------------------------------------------------------------------
// Comparison result
// ---------------------------------------------------------------------------

/// Side-by-side comparison of two modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeComparison {
    pub mode_a: String,
    pub mode_b: String,
    pub generated_at: DateTime<Utc>,

    pub sample_count_a: usize,
    pub sample_count_b: usize,

    /// Slippage comparison.
    pub slippage: SlippageComparison,
    /// Fill rate comparison.
    pub fill_rates: FillRateComparison,
    /// Edge comparison.
    pub edge: EdgeComparison,
    /// Latency comparison.
    pub latency: LatencyComparison,
    /// Key mismatches found.
    pub mismatches: Vec<Mismatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlippageComparison {
    pub avg_expected: Decimal,
    pub avg_actual_a: Decimal,
    pub avg_actual_b: Decimal,
    pub slippage_underestimate_pct: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillRateComparison {
    pub fill_rate_a: Decimal,
    pub fill_rate_b: Decimal,
    pub fill_rate_delta: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeComparison {
    pub avg_expected_edge_a: Decimal,
    pub avg_expected_edge_b: Decimal,
    pub avg_realized_pnl_a: Decimal,
    pub avg_realized_pnl_b: Decimal,
    pub edge_capture_a: Decimal,
    pub edge_capture_b: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyComparison {
    pub avg_latency_a_us: i64,
    pub avg_latency_b_us: i64,
    pub p95_latency_a_us: i64,
    pub p95_latency_b_us: i64,
}

/// A specific mismatch between modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mismatch {
    pub category: String,
    pub description: String,
    pub severity: MismatchSeverity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MismatchSeverity {
    Info,
    Warning,
    Critical,
}

// ---------------------------------------------------------------------------
// Comparison builder
// ---------------------------------------------------------------------------

pub struct ComparisonBuilder;

impl ComparisonBuilder {
    /// Compare two sets of execution samples from different modes.
    pub fn compare(
        mode_a: &str,
        samples_a: &[ExecutionSample],
        mode_b: &str,
        samples_b: &[ExecutionSample],
    ) -> ModeComparison {
        let slippage = Self::compare_slippage(samples_a, samples_b);
        let fill_rates = Self::compare_fill_rates(samples_a, samples_b);
        let edge = Self::compare_edge(samples_a, samples_b);
        let latency = Self::compare_latency(samples_a, samples_b);
        let mismatches = Self::detect_mismatches(&slippage, &fill_rates, &edge, &latency);

        ModeComparison {
            mode_a: mode_a.to_string(),
            mode_b: mode_b.to_string(),
            generated_at: Utc::now(),
            sample_count_a: samples_a.len(),
            sample_count_b: samples_b.len(),
            slippage,
            fill_rates,
            edge,
            latency,
            mismatches,
        }
    }

    fn avg_decimal(values: &[Decimal]) -> Decimal {
        if values.is_empty() {
            return Decimal::ZERO;
        }
        let sum: Decimal = values.iter().sum();
        sum / Decimal::from(values.len() as u64)
    }

    fn compare_slippage(a: &[ExecutionSample], b: &[ExecutionSample]) -> SlippageComparison {
        let expected: Vec<Decimal> = a
            .iter()
            .chain(b.iter())
            .map(|s| s.expected_slippage)
            .collect();
        let actual_a: Vec<Decimal> = a.iter().filter_map(|s| s.actual_slippage).collect();
        let actual_b: Vec<Decimal> = b.iter().filter_map(|s| s.actual_slippage).collect();

        let avg_exp = Self::avg_decimal(&expected);
        let avg_a = Self::avg_decimal(&actual_a);
        let avg_b = Self::avg_decimal(&actual_b);

        let underestimate = if avg_exp > Decimal::ZERO {
            let worst_actual = avg_a.max(avg_b);
            if worst_actual > avg_exp {
                ((worst_actual - avg_exp) / avg_exp) * dec!(100)
            } else {
                Decimal::ZERO
            }
        } else {
            Decimal::ZERO
        };

        SlippageComparison {
            avg_expected: avg_exp,
            avg_actual_a: avg_a,
            avg_actual_b: avg_b,
            slippage_underestimate_pct: underestimate,
        }
    }

    fn compare_fill_rates(a: &[ExecutionSample], b: &[ExecutionSample]) -> FillRateComparison {
        let rate_a = if a.is_empty() {
            Decimal::ZERO
        } else {
            Decimal::from(a.iter().filter(|s| s.filled).count() as u64)
                / Decimal::from(a.len() as u64)
        };
        let rate_b = if b.is_empty() {
            Decimal::ZERO
        } else {
            Decimal::from(b.iter().filter(|s| s.filled).count() as u64)
                / Decimal::from(b.len() as u64)
        };

        FillRateComparison {
            fill_rate_a: rate_a,
            fill_rate_b: rate_b,
            fill_rate_delta: (rate_a - rate_b).abs(),
        }
    }

    fn compare_edge(a: &[ExecutionSample], b: &[ExecutionSample]) -> EdgeComparison {
        let edge_a: Vec<Decimal> = a.iter().map(|s| s.expected_net_edge).collect();
        let edge_b: Vec<Decimal> = b.iter().map(|s| s.expected_net_edge).collect();
        let pnl_a: Vec<Decimal> = a.iter().filter_map(|s| s.realized_pnl).collect();
        let pnl_b: Vec<Decimal> = b.iter().filter_map(|s| s.realized_pnl).collect();

        let avg_edge_a = Self::avg_decimal(&edge_a);
        let avg_edge_b = Self::avg_decimal(&edge_b);
        let avg_pnl_a = Self::avg_decimal(&pnl_a);
        let avg_pnl_b = Self::avg_decimal(&pnl_b);

        let capture_a = if avg_edge_a > Decimal::ZERO {
            avg_pnl_a / avg_edge_a
        } else {
            Decimal::ZERO
        };
        let capture_b = if avg_edge_b > Decimal::ZERO {
            avg_pnl_b / avg_edge_b
        } else {
            Decimal::ZERO
        };

        EdgeComparison {
            avg_expected_edge_a: avg_edge_a,
            avg_expected_edge_b: avg_edge_b,
            avg_realized_pnl_a: avg_pnl_a,
            avg_realized_pnl_b: avg_pnl_b,
            edge_capture_a: capture_a,
            edge_capture_b: capture_b,
        }
    }

    fn compare_latency(a: &[ExecutionSample], b: &[ExecutionSample]) -> LatencyComparison {
        let lat_a: Vec<i64> = a.iter().filter_map(|s| s.fill_latency_us).collect();
        let lat_b: Vec<i64> = b.iter().filter_map(|s| s.fill_latency_us).collect();

        let avg_a = if lat_a.is_empty() {
            0
        } else {
            lat_a.iter().sum::<i64>() / lat_a.len() as i64
        };
        let avg_b = if lat_b.is_empty() {
            0
        } else {
            lat_b.iter().sum::<i64>() / lat_b.len() as i64
        };

        let p95_a = Self::percentile_i64(&lat_a, 0.95);
        let p95_b = Self::percentile_i64(&lat_b, 0.95);

        LatencyComparison {
            avg_latency_a_us: avg_a,
            avg_latency_b_us: avg_b,
            p95_latency_a_us: p95_a,
            p95_latency_b_us: p95_b,
        }
    }

    fn percentile_i64(values: &[i64], pct: f64) -> i64 {
        if values.is_empty() {
            return 0;
        }
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let idx = ((sorted.len() as f64 * pct) as usize).min(sorted.len() - 1);
        sorted[idx]
    }

    fn detect_mismatches(
        slippage: &SlippageComparison,
        fill_rates: &FillRateComparison,
        edge: &EdgeComparison,
        _latency: &LatencyComparison,
    ) -> Vec<Mismatch> {
        let mut mismatches = Vec::new();

        // Slippage underestimate
        if slippage.slippage_underestimate_pct > dec!(50) {
            mismatches.push(Mismatch {
                category: "slippage".into(),
                description: format!(
                    "actual slippage {}% higher than modeled",
                    slippage.slippage_underestimate_pct
                ),
                severity: MismatchSeverity::Critical,
            });
        } else if slippage.slippage_underestimate_pct > dec!(20) {
            mismatches.push(Mismatch {
                category: "slippage".into(),
                description: format!(
                    "actual slippage {}% higher than modeled",
                    slippage.slippage_underestimate_pct
                ),
                severity: MismatchSeverity::Warning,
            });
        }

        // Fill rate divergence
        if fill_rates.fill_rate_delta > dec!(0.20) {
            mismatches.push(Mismatch {
                category: "fill_rate".into(),
                description: format!(
                    "fill rate divergence: {} vs {}",
                    fill_rates.fill_rate_a, fill_rates.fill_rate_b
                ),
                severity: MismatchSeverity::Warning,
            });
        }

        // Negative edge capture
        if edge.edge_capture_a < Decimal::ZERO || edge.edge_capture_b < Decimal::ZERO {
            mismatches.push(Mismatch {
                category: "edge".into(),
                description: "negative edge capture — strategy losing money".into(),
                severity: MismatchSeverity::Critical,
            });
        }

        mismatches
    }

    /// Render comparison as markdown.
    pub fn to_markdown(cmp: &ModeComparison) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "# {} vs {} Comparison\n\n",
            cmp.mode_a, cmp.mode_b
        ));
        out.push_str(&format!(
            "Generated: {}\n\n",
            cmp.generated_at.format("%Y-%m-%d %H:%M UTC")
        ));

        out.push_str("## Fill Rates\n\n");
        out.push_str(&format!(
            "| Mode | Fill Rate |\n|------|----------|\n| {} | {} |\n| {} | {} |\n\n",
            cmp.mode_a, cmp.fill_rates.fill_rate_a, cmp.mode_b, cmp.fill_rates.fill_rate_b
        ));

        out.push_str("## Slippage\n\n");
        out.push_str(&format!(
            "- Expected: {}\n- {} actual: {}\n- {} actual: {}\n- Underestimate: {}%\n\n",
            cmp.slippage.avg_expected,
            cmp.mode_a,
            cmp.slippage.avg_actual_a,
            cmp.mode_b,
            cmp.slippage.avg_actual_b,
            cmp.slippage.slippage_underestimate_pct
        ));

        out.push_str("## Edge Capture\n\n");
        out.push_str(
            "| Mode | Avg Edge | Avg PnL | Capture |\n|------|----------|---------|--------|\n",
        );
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            cmp.mode_a,
            cmp.edge.avg_expected_edge_a,
            cmp.edge.avg_realized_pnl_a,
            cmp.edge.edge_capture_a
        ));
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n\n",
            cmp.mode_b,
            cmp.edge.avg_expected_edge_b,
            cmp.edge.avg_realized_pnl_b,
            cmp.edge.edge_capture_b
        ));

        if !cmp.mismatches.is_empty() {
            out.push_str("## Mismatches\n\n");
            for m in &cmp.mismatches {
                let icon = match m.severity {
                    MismatchSeverity::Critical => "!!",
                    MismatchSeverity::Warning => "!",
                    MismatchSeverity::Info => "i",
                };
                out.push_str(&format!(
                    "- [{}] **{}**: {}\n",
                    icon, m.category, m.description
                ));
            }
        }

        out
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sim_sample(edge: Decimal, pnl: Decimal) -> ExecutionSample {
        ExecutionSample {
            mode: "simulation".into(),
            timestamp: Utc::now(),
            contract: "BTC-5m".into(),
            side: "BUY".into(),
            expected_price: dec!(0.55),
            actual_price: Some(dec!(0.555)),
            expected_slippage: dec!(0.005),
            actual_slippage: Some(dec!(0.005)),
            fill_latency_us: Some(100),
            expected_net_edge: edge,
            realized_pnl: Some(pnl),
            filled: true,
        }
    }

    fn live_sample(edge: Decimal, pnl: Decimal, slippage: Decimal) -> ExecutionSample {
        ExecutionSample {
            mode: "live".into(),
            timestamp: Utc::now(),
            contract: "BTC-5m".into(),
            side: "BUY".into(),
            expected_price: dec!(0.55),
            actual_price: Some(dec!(0.55) + slippage),
            expected_slippage: dec!(0.005),
            actual_slippage: Some(slippage),
            fill_latency_us: Some(500),
            expected_net_edge: edge,
            realized_pnl: Some(pnl),
            filled: true,
        }
    }

    #[test]
    fn compare_similar_modes() {
        let sim = vec![sim_sample(dec!(0.05), dec!(0.03))];
        let live = vec![live_sample(dec!(0.05), dec!(0.02), dec!(0.006))];

        let cmp = ComparisonBuilder::compare("simulation", &sim, "live", &live);
        assert_eq!(cmp.sample_count_a, 1);
        assert_eq!(cmp.sample_count_b, 1);
    }

    #[test]
    fn fill_rate_comparison() {
        let a = vec![
            sim_sample(dec!(0.05), dec!(0.03)),
            ExecutionSample {
                filled: false,
                ..sim_sample(dec!(0.05), dec!(0.03))
            },
        ];
        let b = vec![live_sample(dec!(0.05), dec!(0.02), dec!(0.005))];

        let cmp = ComparisonBuilder::compare("sim", &a, "live", &b);
        assert_eq!(cmp.fill_rates.fill_rate_a, dec!(0.5)); // 1/2
        assert_eq!(cmp.fill_rates.fill_rate_b, dec!(1)); // 1/1
    }

    #[test]
    fn detects_negative_edge_mismatch() {
        let sim = vec![sim_sample(dec!(0.05), dec!(-0.10))]; // Negative PnL
        let live = vec![live_sample(dec!(0.05), dec!(-0.08), dec!(0.005))];

        let cmp = ComparisonBuilder::compare("sim", &sim, "live", &live);
        assert!(cmp.mismatches.iter().any(|m| m.category == "edge"));
    }

    #[test]
    fn markdown_output() {
        let sim = vec![sim_sample(dec!(0.05), dec!(0.03))];
        let live = vec![live_sample(dec!(0.05), dec!(0.02), dec!(0.006))];

        let cmp = ComparisonBuilder::compare("simulation", &sim, "live", &live);
        let md = ComparisonBuilder::to_markdown(&cmp);
        assert!(md.contains("simulation vs live"));
        assert!(md.contains("Fill Rates"));
    }

    #[test]
    fn empty_samples_no_panic() {
        let cmp = ComparisonBuilder::compare("sim", &[], "live", &[]);
        assert_eq!(cmp.sample_count_a, 0);
        assert_eq!(cmp.sample_count_b, 0);
    }

    #[test]
    fn comparison_serializable() {
        let cmp = ComparisonBuilder::compare(
            "sim",
            &[sim_sample(dec!(0.05), dec!(0.03))],
            "live",
            &[live_sample(dec!(0.05), dec!(0.02), dec!(0.005))],
        );
        let json = serde_json::to_string(&cmp).unwrap();
        assert!(json.contains("sim"));
    }
}
