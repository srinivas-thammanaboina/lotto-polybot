//! Region benchmark harness.
//!
//! Measures feed latencies, internal processing times, and connectivity
//! quality from a candidate host/region. Outputs machine-readable results
//! so region selection can be data-driven.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::telemetry::histograms::{HistogramStats, LatencyHistogram};

// ---------------------------------------------------------------------------
// Benchmark config
// ---------------------------------------------------------------------------

/// Configuration for a benchmark run.
#[derive(Debug, Clone)]
pub struct BenchmarkConfig {
    /// Region tag (e.g. "DO-FRA1", "AWS-eu-west-1").
    pub region: String,
    /// How long to collect samples.
    pub duration: Duration,
    /// Max samples per histogram.
    pub max_samples: usize,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            region: "local".into(),
            duration: Duration::from_secs(60),
            max_samples: 10000,
        }
    }
}

// ---------------------------------------------------------------------------
// Benchmark metrics
// ---------------------------------------------------------------------------

/// Per-feed latency measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedLatency {
    pub source: String,
    pub stats: HistogramStats,
    pub sample_count: usize,
    pub connection_errors: u32,
    pub reconnects: u32,
}

/// Overall benchmark result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub region: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub duration_secs: u64,

    /// Per-feed latency stats.
    pub feed_latencies: Vec<FeedLatency>,

    /// Internal decision latency (signal pipeline).
    pub decision_latency: HistogramStats,

    /// Overall health score (0.0 to 1.0).
    pub health_score: Decimal,

    /// Warnings or issues detected.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Benchmark session
// ---------------------------------------------------------------------------

/// Collects benchmark samples during a measurement window.
pub struct BenchmarkSession {
    config: BenchmarkConfig,
    started_at: DateTime<Utc>,
    feed_histograms: HashMap<String, LatencyHistogram>,
    decision_histogram: LatencyHistogram,
    connection_errors: HashMap<String, u32>,
    reconnects: HashMap<String, u32>,
    warnings: Vec<String>,
}

impl BenchmarkSession {
    pub fn new(config: BenchmarkConfig) -> Self {
        let max_samples = config.max_samples;
        Self {
            config,
            started_at: Utc::now(),
            feed_histograms: HashMap::new(),
            decision_histogram: LatencyHistogram::new("decision", max_samples),
            connection_errors: HashMap::new(),
            reconnects: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    /// Record a feed message latency (time from source to receipt, in microseconds).
    pub fn record_feed_latency(&mut self, source: &str, latency_us: i64) {
        let hist = self
            .feed_histograms
            .entry(source.to_string())
            .or_insert_with(|| LatencyHistogram::new(source, self.config.max_samples));
        hist.record_us(latency_us);
    }

    /// Record internal decision latency (signal pipeline processing time).
    pub fn record_decision_latency(&mut self, latency_us: i64) {
        self.decision_histogram.record_us(latency_us);
    }

    /// Record a connection error for a feed.
    pub fn record_connection_error(&mut self, source: &str) {
        *self
            .connection_errors
            .entry(source.to_string())
            .or_insert(0) += 1;
    }

    /// Record a reconnect for a feed.
    pub fn record_reconnect(&mut self, source: &str) {
        *self.reconnects.entry(source.to_string()).or_insert(0) += 1;
    }

    /// Add a warning.
    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    /// Finalize and produce the benchmark result.
    pub fn finalize(self) -> BenchmarkResult {
        let completed_at = Utc::now();
        let duration = completed_at - self.started_at;

        let feed_latencies: Vec<FeedLatency> = self
            .feed_histograms
            .iter()
            .map(|(source, hist)| {
                let stats = hist.stats();
                FeedLatency {
                    source: source.clone(),
                    stats,
                    sample_count: hist.count(),
                    connection_errors: self.connection_errors.get(source).copied().unwrap_or(0),
                    reconnects: self.reconnects.get(source).copied().unwrap_or(0),
                }
            })
            .collect();

        // Health score: 1.0 minus penalties
        let mut health = Decimal::from(100);

        // Penalty for high latency (p95 > 5ms = 5000us)
        for fl in &feed_latencies {
            if fl.stats.p95_us > 5000 {
                health -= Decimal::from(10);
            }
            if fl.connection_errors > 0 {
                health -= Decimal::from(fl.connection_errors * 5);
            }
            if fl.reconnects > 2 {
                health -= Decimal::from(fl.reconnects * 3);
            }
        }

        let decision_stats = self.decision_histogram.stats();
        if decision_stats.p95_us > 1000 {
            health -= Decimal::from(15);
        }

        let health_score = (health.max(Decimal::ZERO)) / Decimal::from(100);

        let result = BenchmarkResult {
            region: self.config.region,
            started_at: self.started_at,
            completed_at,
            duration_secs: duration.num_seconds() as u64,
            feed_latencies,
            decision_latency: decision_stats,
            health_score,
            warnings: self.warnings,
        };

        info!(
            region = %result.region,
            health = %result.health_score,
            feeds = result.feed_latencies.len(),
            "benchmark_complete"
        );

        result
    }

    /// Get the config.
    pub fn config(&self) -> &BenchmarkConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// Compare benchmark results across regions.
pub fn compare_regions(results: &[BenchmarkResult]) -> Vec<RegionRanking> {
    let mut rankings: Vec<RegionRanking> = results
        .iter()
        .map(|r| {
            let avg_feed_p95 = if r.feed_latencies.is_empty() {
                0
            } else {
                r.feed_latencies.iter().map(|f| f.stats.p95_us).sum::<i64>()
                    / r.feed_latencies.len() as i64
            };

            RegionRanking {
                region: r.region.clone(),
                health_score: r.health_score,
                avg_feed_p95_us: avg_feed_p95,
                decision_p95_us: r.decision_latency.p95_us,
                total_errors: r.feed_latencies.iter().map(|f| f.connection_errors).sum(),
            }
        })
        .collect();

    // Sort by health score descending
    rankings.sort_by(|a, b| b.health_score.cmp(&a.health_score));
    rankings
}

/// Region ranking for comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionRanking {
    pub region: String,
    pub health_score: Decimal,
    pub avg_feed_p95_us: i64,
    pub decision_p95_us: i64,
    pub total_errors: u32,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn session_records_feed_latency() {
        let mut session = BenchmarkSession::new(BenchmarkConfig::default());
        session.record_feed_latency("binance", 500);
        session.record_feed_latency("binance", 600);
        session.record_feed_latency("polymarket", 1000);

        let result = session.finalize();
        assert_eq!(result.feed_latencies.len(), 2);
    }

    #[test]
    fn session_records_decision_latency() {
        let mut session = BenchmarkSession::new(BenchmarkConfig::default());
        session.record_decision_latency(100);
        session.record_decision_latency(200);

        let result = session.finalize();
        assert_eq!(result.decision_latency.count, 2);
    }

    #[test]
    fn health_score_perfect_with_low_latency() {
        let mut session = BenchmarkSession::new(BenchmarkConfig::default());
        for _ in 0..100 {
            session.record_feed_latency("binance", 500);
            session.record_decision_latency(100);
        }

        let result = session.finalize();
        assert_eq!(result.health_score, dec!(1));
    }

    #[test]
    fn health_score_penalized_for_errors() {
        let mut session = BenchmarkSession::new(BenchmarkConfig::default());
        session.record_feed_latency("binance", 500);
        session.record_connection_error("binance");
        session.record_connection_error("binance");

        let result = session.finalize();
        assert!(result.health_score < dec!(1));
    }

    #[test]
    fn health_score_penalized_for_high_latency() {
        let mut session = BenchmarkSession::new(BenchmarkConfig::default());
        for _ in 0..10 {
            session.record_feed_latency("binance", 10000); // 10ms > 5ms threshold
        }
        session.record_decision_latency(100);

        let result = session.finalize();
        assert!(result.health_score < dec!(1));
    }

    #[test]
    fn region_comparison() {
        let r1 = BenchmarkResult {
            region: "FRA1".into(),
            started_at: Utc::now(),
            completed_at: Utc::now(),
            duration_secs: 60,
            feed_latencies: vec![FeedLatency {
                source: "binance".into(),
                stats: HistogramStats {
                    name: "binance".into(),
                    count: 100,
                    min_us: 100,
                    max_us: 2000,
                    mean_us: 500,
                    p50_us: 400,
                    p95_us: 1500,
                    p99_us: 1800,
                },
                sample_count: 100,
                connection_errors: 0,
                reconnects: 0,
            }],
            decision_latency: HistogramStats {
                name: "decision".into(),
                count: 100,
                min_us: 50,
                max_us: 500,
                mean_us: 100,
                p50_us: 80,
                p95_us: 300,
                p99_us: 400,
            },
            health_score: dec!(1.0),
            warnings: Vec::new(),
        };

        let mut r2 = r1.clone();
        r2.region = "AMS3".into();
        r2.health_score = dec!(0.85);

        let rankings = compare_regions(&[r1, r2]);
        assert_eq!(rankings[0].region, "FRA1"); // Higher health score first
        assert_eq!(rankings[1].region, "AMS3");
    }

    #[test]
    fn result_serializable() {
        let mut session = BenchmarkSession::new(BenchmarkConfig {
            region: "test-region".into(),
            ..Default::default()
        });
        session.record_feed_latency("binance", 500);
        session.record_decision_latency(100);

        let result = session.finalize();
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("test-region"));
    }

    #[test]
    fn warnings_collected() {
        let mut session = BenchmarkSession::new(BenchmarkConfig::default());
        session.add_warning("high packet loss detected".into());
        let result = session.finalize();
        assert_eq!(result.warnings.len(), 1);
    }
}
