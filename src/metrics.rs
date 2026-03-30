use std::sync::atomic::{AtomicU64, Ordering};

/// Lightweight atomic counters for runtime metrics.
/// No external metrics crate needed yet — just atomics for now.
pub struct BotMetrics {
    pub events_received: AtomicU64,
    pub signals_accepted: AtomicU64,
    pub signals_rejected: AtomicU64,
    pub orders_submitted: AtomicU64,
    pub fills_received: AtomicU64,
    pub kill_switch_activations: AtomicU64,
}

impl Default for BotMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl BotMetrics {
    pub const fn new() -> Self {
        Self {
            events_received: AtomicU64::new(0),
            signals_accepted: AtomicU64::new(0),
            signals_rejected: AtomicU64::new(0),
            orders_submitted: AtomicU64::new(0),
            fills_received: AtomicU64::new(0),
            kill_switch_activations: AtomicU64::new(0),
        }
    }

    pub fn inc_events(&self) {
        self.events_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            events_received: self.events_received.load(Ordering::Relaxed),
            signals_accepted: self.signals_accepted.load(Ordering::Relaxed),
            signals_rejected: self.signals_rejected.load(Ordering::Relaxed),
            orders_submitted: self.orders_submitted.load(Ordering::Relaxed),
            fills_received: self.fills_received.load(Ordering::Relaxed),
            kill_switch_activations: self.kill_switch_activations.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub events_received: u64,
    pub signals_accepted: u64,
    pub signals_rejected: u64,
    pub orders_submitted: u64,
    pub fills_received: u64,
    pub kill_switch_activations: u64,
}
