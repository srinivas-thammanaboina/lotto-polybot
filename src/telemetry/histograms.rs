//! Latency histogram tracking.
//!
//! Lightweight in-memory histograms for measuring event processing latencies,
//! feed-to-signal delays, and order round-trip times.

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A simple sliding-window histogram for latency samples.
#[derive(Debug, Clone)]
pub struct LatencyHistogram {
    name: String,
    samples: VecDeque<i64>,
    max_samples: usize,
}

/// Histogram summary statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramStats {
    pub name: String,
    pub count: usize,
    pub min_us: i64,
    pub max_us: i64,
    pub mean_us: i64,
    pub p50_us: i64,
    pub p95_us: i64,
    pub p99_us: i64,
}

impl LatencyHistogram {
    pub fn new(name: &str, max_samples: usize) -> Self {
        Self {
            name: name.to_string(),
            samples: VecDeque::with_capacity(max_samples),
            max_samples,
        }
    }

    /// Record a latency sample in microseconds.
    pub fn record_us(&mut self, latency_us: i64) {
        if self.samples.len() >= self.max_samples {
            self.samples.pop_front();
        }
        self.samples.push_back(latency_us);
    }

    /// Record a latency from a timestamp to now.
    pub fn record_since(&mut self, since: DateTime<Utc>) {
        let elapsed = Utc::now() - since;
        self.record_us(elapsed.num_microseconds().unwrap_or(i64::MAX));
    }

    /// Get summary statistics.
    pub fn stats(&self) -> HistogramStats {
        if self.samples.is_empty() {
            return HistogramStats {
                name: self.name.clone(),
                count: 0,
                min_us: 0,
                max_us: 0,
                mean_us: 0,
                p50_us: 0,
                p95_us: 0,
                p99_us: 0,
            };
        }

        let mut sorted: Vec<i64> = self.samples.iter().copied().collect();
        sorted.sort_unstable();

        let count = sorted.len();
        let sum: i64 = sorted.iter().sum();

        HistogramStats {
            name: self.name.clone(),
            count,
            min_us: sorted[0],
            max_us: sorted[count - 1],
            mean_us: sum / count as i64,
            p50_us: sorted[(count - 1) / 2],
            p95_us: sorted[(count as f64 * 0.95) as usize],
            p99_us: sorted[(count as f64 * 0.99).min((count - 1) as f64) as usize],
        }
    }

    /// Clear all samples.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn count(&self) -> usize {
        self.samples.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_histogram() {
        let hist = LatencyHistogram::new("test", 100);
        let stats = hist.stats();
        assert_eq!(stats.count, 0);
        assert_eq!(stats.min_us, 0);
    }

    #[test]
    fn record_and_stats() {
        let mut hist = LatencyHistogram::new("test", 100);
        for i in 1..=100 {
            hist.record_us(i);
        }
        let stats = hist.stats();
        assert_eq!(stats.count, 100);
        assert_eq!(stats.min_us, 1);
        assert_eq!(stats.max_us, 100);
        assert_eq!(stats.mean_us, 50);
        assert_eq!(stats.p50_us, 50);
        assert!(stats.p95_us >= 95);
        assert!(stats.p99_us >= 99);
    }

    #[test]
    fn sliding_window_evicts() {
        let mut hist = LatencyHistogram::new("test", 5);
        for i in 1..=10 {
            hist.record_us(i);
        }
        assert_eq!(hist.count(), 5);
        let stats = hist.stats();
        assert_eq!(stats.min_us, 6); // oldest 1-5 evicted
        assert_eq!(stats.max_us, 10);
    }

    #[test]
    fn clear_resets() {
        let mut hist = LatencyHistogram::new("test", 100);
        hist.record_us(42);
        hist.clear();
        assert_eq!(hist.count(), 0);
    }

    #[test]
    fn stats_serializable() {
        let mut hist = LatencyHistogram::new("feed_latency", 100);
        hist.record_us(500);
        let stats = hist.stats();
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("feed_latency"));
    }
}
