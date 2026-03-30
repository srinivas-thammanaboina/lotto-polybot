use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::error::BotError;
use crate::metrics::BotMetrics;
use crate::shutdown;
use crate::telemetry;
use crate::types::BotEvent;

/// Channel capacity for the main event bus.
const EVENT_BUS_CAPACITY: usize = 4096;

/// Channel capacity for order intents flowing to execution.
const INTENT_CAPACITY: usize = 256;

/// Top-level application entry point.
/// Wires config, logging, shutdown, channels, and all task groups.
pub async fn run() -> Result<(), BotError> {
    let cfg = AppConfig::from_env()?;
    cfg.validate()?;

    // --- logging ---
    telemetry::logging::init(&cfg.telemetry);

    info!(
        mode = %cfg.mode,
        region = %cfg.region_tag,
        coinbase_enabled = cfg.coinbase.enabled,
        "poly-latency-bot starting"
    );

    // --- shared state ---
    let metrics = Arc::new(BotMetrics::new());
    let shutdown_token = CancellationToken::new();
    shutdown::install(shutdown_token.clone());

    // --- event bus ---
    // All feed adapters and execution events publish here.
    // Telemetry, replay recorder, and risk engine consume from here.
    let (event_tx, mut event_rx) = mpsc::channel::<BotEvent>(EVENT_BUS_CAPACITY);

    // --- intent channel ---
    // Signal engine publishes order intents; execution engine consumes.
    let (_intent_tx, _intent_rx) =
        mpsc::channel::<crate::domain::signal::OrderIntent>(INTENT_CAPACITY);

    // --- task groups ---
    // Each phase will add spawns here. For now just the event drain loop.

    let metrics_clone = Arc::clone(&metrics);
    let drain_token = shutdown_token.clone();
    let drain_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(event) = event_rx.recv() => {
                    metrics_clone.inc_events();
                    // Future phases: route to strategy, risk, telemetry, replay
                    tracing::trace!(event = event.label(), "event received");
                }
                _ = drain_token.cancelled() => {
                    // Drain remaining events before exiting
                    while let Ok(event) = event_rx.try_recv() {
                        metrics_clone.inc_events();
                        tracing::trace!(event = event.label(), "draining event");
                    }
                    break;
                }
            }
        }
    });

    // --- ready ---
    info!("all systems ready, waiting for shutdown signal (ctrl-c)");
    shutdown_token.cancelled().await;

    // --- shutdown sequence ---
    info!("shutdown initiated, stopping task groups");

    // Drop the sender side so receivers can drain and exit
    drop(event_tx);

    // Wait for the drain loop to finish
    if let Err(e) = drain_handle.await {
        warn!(error = %e, "event drain task panicked");
    }

    let snap = metrics.snapshot();
    info!(
        events = snap.events_received,
        signals_accepted = snap.signals_accepted,
        signals_rejected = snap.signals_rejected,
        orders = snap.orders_submitted,
        fills = snap.fills_received,
        "shutdown complete"
    );

    Ok(())
}
