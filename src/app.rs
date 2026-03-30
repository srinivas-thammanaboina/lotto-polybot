use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::{AppConfig, RunMode};
use crate::discovery::cache::ContractRegistry;
use crate::domain::market::{Asset, BookSnapshot, MarketDuration, Outcome};
use crate::domain::signal::{OrderIntent, RejectReason, SignalDecision};
use crate::error::BotError;
use crate::execution::cancel_policy::CancelPolicy;
use crate::execution::client::{ExchangeClient, SimulationClient};
use crate::execution::fill_state::{ExposureTracker, FillStateProcessor};
use crate::execution::submit::{ExecutionEngine, OrderTracker};
use crate::feeds::health::FeedHealthMonitor;
use crate::metrics::BotMetrics;
use crate::risk::contract_lock::ContractLockService;
use crate::risk::kill_switch::{KillSwitch, KillSwitchReason};
use crate::shutdown;
use crate::simulation::engine::{FillModel, SimulationSession};
use crate::strategy::edge::FeeSchedule;
use crate::strategy::pipeline::{PipelineInput, SignalPipeline};
use crate::strategy::sizing::SizingMode;
use crate::telemetry;
use crate::telemetry::persistence::EventPersistence;
use crate::types::{BotEvent, CexTick, FeedSource};

/// Channel capacity for the main event bus.
const EVENT_BUS_CAPACITY: usize = 4096;

/// Channel capacity for order intents flowing to execution.
const INTENT_CAPACITY: usize = 256;

/// Cancel policy scan interval.
const CANCEL_SCAN_INTERVAL_SECS: u64 = 5;

/// Contract lock cleanup interval.
const LOCK_CLEANUP_INTERVAL_SECS: u64 = 30;

/// How long to wait for initial discovery before entering the event loop.
const DISCOVERY_WAIT_SECS: u64 = 10;

// ---------------------------------------------------------------------------
// Shared runtime state accessible from the event loop
// ---------------------------------------------------------------------------

/// Latest CEX tick per asset, for building pipeline inputs.
struct LatestState {
    cex_ticks: HashMap<Asset, CexTick>,
    books: HashMap<String, BookSnapshot>,
    window_open_prices: HashMap<Asset, Decimal>,
}

impl LatestState {
    fn new() -> Self {
        Self {
            cex_ticks: HashMap::new(),
            books: HashMap::new(),
            window_open_prices: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// P0-1: Execution client factory by mode
// ---------------------------------------------------------------------------

/// Build the appropriate exchange client based on run mode.
/// Fails startup if paper/live is requested without a real client.
fn build_exchange_client(cfg: &AppConfig) -> Result<Arc<dyn ExchangeClient>, BotError> {
    match cfg.mode {
        RunMode::DryRun | RunMode::Simulation => {
            info!(mode = %cfg.mode, "using SimulationClient");
            Ok(Arc::new(SimulationClient::default()))
        }
        RunMode::Paper | RunMode::Live => {
            // TODO: Replace with real Polymarket SDK client when available.
            // For now, fail closed if credentials are missing.
            if cfg.polymarket.api_key.is_none() || cfg.polymarket.secret.is_none() {
                return Err(BotError::Config(crate::config::ConfigError::Missing(
                    "paper/live mode requires POLYMARKET_API_KEY and POLYMARKET_SECRET".into(),
                )));
            }
            warn!(
                mode = %cfg.mode,
                "real ExchangeClient not yet implemented — using SimulationClient as placeholder"
            );
            Ok(Arc::new(SimulationClient::default()))
        }
    }
}

// ---------------------------------------------------------------------------
// P0-3 + P2-3: Startup readiness
// ---------------------------------------------------------------------------

/// Wait for initial discovery to populate the contract registry.
/// Returns the discovered token IDs for market WS subscription.
async fn wait_for_discovery(registry: &ContractRegistry, timeout: Duration) -> Vec<String> {
    let start = tokio::time::Instant::now();
    let poll_interval = Duration::from_millis(500);

    loop {
        if registry.is_healthy() {
            let contracts = registry.active_contracts();
            if !contracts.is_empty() {
                let token_ids: Vec<String> =
                    contracts.iter().map(|c| c.token_id.to_string()).collect();
                info!(
                    tokens = token_ids.len(),
                    "discovery ready — token IDs available for subscription"
                );
                return token_ids;
            }
        }

        if start.elapsed() > timeout {
            warn!("discovery timeout — proceeding with empty token list");
            return Vec::new();
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Check if the system is ready for signal evaluation.
fn is_system_ready(
    registry: &ContractRegistry,
    feed_health: &FeedHealthMonitor,
    kill_switch: &KillSwitch,
) -> bool {
    registry.is_healthy() && feed_health.is_healthy(FeedSource::Binance) && !kill_switch.is_active()
}

// ---------------------------------------------------------------------------
// P1-1: Parse kill switch reason from event
// ---------------------------------------------------------------------------

fn parse_kill_switch_reason(reason: &str) -> KillSwitchReason {
    match reason {
        s if s.contains("daily_drawdown") => KillSwitchReason::DailyDrawdownBreach {
            drawdown: "unknown".into(),
            limit: "unknown".into(),
        },
        s if s.contains("total_drawdown") => KillSwitchReason::TotalDrawdownBreach {
            drawdown: "unknown".into(),
            limit: "unknown".into(),
        },
        s if s.contains("consecutive_loss") => {
            KillSwitchReason::ConsecutiveLossBreach { count: 0, limit: 0 }
        }
        s if s.contains("stale_feed") => KillSwitchReason::StaleFeedRegime,
        s if s.contains("reconnect") => KillSwitchReason::ReconnectStorm {
            count: 0,
            window_secs: 0,
        },
        _ => KillSwitchReason::Manual,
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

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
    let (event_tx, mut event_rx) = mpsc::channel::<BotEvent>(EVENT_BUS_CAPACITY);

    // --- intent channel ---
    let (intent_tx, mut intent_rx) = mpsc::channel::<OrderIntent>(INTENT_CAPACITY);

    // --- risk subsystems ---
    let kill_switch = KillSwitch::new();
    let contract_locks = ContractLockService::new(
        Duration::from_secs(30), // post-expiry buffer
        cfg.strategy.five_min.cooldown,
    );
    let feed_health = FeedHealthMonitor::new();

    // Register feeds
    feed_health.register(FeedSource::Binance, cfg.binance.stale_threshold);
    if cfg.coinbase.enabled {
        feed_health.register(FeedSource::Coinbase, cfg.coinbase.stale_threshold);
    }

    // --- P0-1: execution client selection by mode ---
    let exchange_client = build_exchange_client(&cfg)?;
    info!(mode = %cfg.mode, "exchange client initialized");

    let order_tracker = Arc::new(OrderTracker::new());
    let exposure_tracker = Arc::new(ExposureTracker::new());
    let fill_processor =
        FillStateProcessor::new(Arc::clone(&order_tracker), Arc::clone(&exposure_tracker));

    let exec_engine = ExecutionEngine::new(
        exchange_client.clone(),
        Arc::clone(&order_tracker),
        cfg.execution.clone(),
        event_tx.clone(),
    );

    // --- discovery ---
    let registry = ContractRegistry::new();
    let http_client = reqwest::Client::new();
    let _discovery_handle =
        registry
            .clone()
            .spawn_refresh(cfg.polymarket.clone(), http_client, shutdown_token.clone());

    // --- P0-3: Wait for initial discovery before spawning market WS ---
    let token_ids = wait_for_discovery(&registry, Duration::from_secs(DISCOVERY_WAIT_SECS)).await;

    // --- feed adapters ---
    // Binance
    let _binance_handle = crate::feeds::binance::spawn(
        cfg.binance.clone(),
        event_tx.clone(),
        feed_health.clone(),
        shutdown_token.clone(),
    );

    // P0-2: Only spawn Coinbase when enabled
    if cfg.coinbase.enabled {
        let _coinbase_handle = crate::feeds::coinbase::spawn(
            cfg.coinbase.clone(),
            event_tx.clone(),
            feed_health.clone(),
            shutdown_token.clone(),
        );
    } else {
        info!("coinbase: disabled in config, not spawning");
    }

    // P0-3: Polymarket market WS — subscribe with discovered token IDs
    let _poly_market_handle = crate::feeds::polymarket_market::spawn(
        cfg.polymarket.clone(),
        token_ids,
        event_tx.clone(),
        feed_health.clone(),
        shutdown_token.clone(),
    );

    // P0-4: Polymarket user WS (auth now includes secret + passphrase)
    let _poly_user_handle = crate::feeds::polymarket_user::spawn(
        cfg.polymarket.clone(),
        event_tx.clone(),
        shutdown_token.clone(),
    );

    // Polymarket RTDS
    let _rtds_handle = crate::feeds::polymarket_rtds::spawn(
        cfg.polymarket.clone(),
        event_tx.clone(),
        feed_health.clone(),
        shutdown_token.clone(),
    );

    // --- persistence ---
    let persistence_path = PathBuf::from(&cfg.telemetry.event_log_path);
    let (persistence, persist_rx) = EventPersistence::new(persistence_path.clone(), 1024);
    let _persist_handle = EventPersistence::spawn_writer(persistence_path, persist_rx, 100);

    // --- simulation session (active in simulation mode) ---
    let sim_session = Arc::new(RwLock::new(SimulationSession::new(FillModel::default())));

    // --- P1-2: Track equity from exchange client balance ---
    let initial_balance = exchange_client
        .account_balance()
        .await
        .map(|b| b.available_usdc)
        .unwrap_or(dec!(500));
    let equity = Arc::new(RwLock::new(initial_balance));
    info!(equity = %initial_balance, "initial equity loaded");

    // --- strategy config ---
    let fee_schedule = FeeSchedule::default();
    let sizing_mode = SizingMode::FixedNotional {
        amount: cfg.risk.max_notional_per_order,
    };

    // --- mutable latest-state for the event loop ---
    let mut latest = LatestState::new();

    // --- periodic tasks ---
    let cancel_scan_token = shutdown_token.clone();
    let cancel_tracker = Arc::clone(&order_tracker);
    let _cancel_scan_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(CANCEL_SCAN_INTERVAL_SECS));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let active = cancel_tracker.orders_in_state(
                        crate::domain::order::OrderState::Acked,
                    );
                    let pending = cancel_tracker.orders_in_state(
                        crate::domain::order::OrderState::Pending,
                    );
                    let partial = cancel_tracker.orders_in_state(
                        crate::domain::order::OrderState::PartialFill,
                    );

                    let mut all_active: Vec<_> = active;
                    all_active.extend(pending);
                    all_active.extend(partial);

                    if !all_active.is_empty() {
                        let to_cancel = CancelPolicy::scan_orders(
                            &all_active,
                            MarketDuration::FiveMin,
                            None,
                            Utc::now(),
                        );
                        for (coid, reason) in &to_cancel {
                            warn!(
                                client_order_id = %coid,
                                reason = %reason,
                                "cancel_policy_triggered"
                            );
                        }
                    }
                }
                _ = cancel_scan_token.cancelled() => break,
            }
        }
    });

    let lock_cleanup_token = shutdown_token.clone();
    let lock_cleanup_svc = contract_locks.clone();
    let _lock_cleanup_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(LOCK_CLEANUP_INTERVAL_SECS));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    lock_cleanup_svc.cleanup_expired();
                }
                _ = lock_cleanup_token.cancelled() => break,
            }
        }
    });

    // --- main event loop ---
    let metrics_clone = Arc::clone(&metrics);
    let drain_token = shutdown_token.clone();

    info!("all systems ready, entering main event loop");

    loop {
        tokio::select! {
            // Process events from the bus
            Some(event) = event_rx.recv() => {
                metrics_clone.inc_events();
                persistence.try_persist(&event);

                match &event {
                    BotEvent::CexTick(tick) => {
                        // P2-3: Check system readiness before signal evaluation
                        if !is_system_ready(&registry, &feed_health, &kill_switch) {
                            // Update state but don't evaluate signals
                            latest.cex_ticks.insert(tick.asset, tick.clone());
                            latest.window_open_prices.entry(tick.asset).or_insert(tick.price);
                            continue;
                        }

                        // Update latest state
                        latest.window_open_prices.entry(tick.asset).or_insert(tick.price);
                        latest.cex_ticks.insert(tick.asset, tick.clone());

                        // P1-2: Compute signal age from tick receipt time
                        let now = Utc::now();
                        let signal_age_chrono = now - tick.receipt_timestamp.0;
                        let signal_age = signal_age_chrono
                            .to_std()
                            .unwrap_or(Duration::from_millis(0));

                        // P1-2: Read current equity
                        let current_equity = *equity.read();

                        // Try to generate signals for all active contracts of this asset
                        let contracts = registry.active_contracts();
                        for contract_entry in contracts.iter().filter(|c| c.asset == tick.asset) {
                            if kill_switch.is_active() {
                                break;
                            }
                            if !contract_locks.is_tradeable(&contract_entry.key) {
                                continue;
                            }

                            let book_key = contract_entry.token_id.to_string();
                            let book = latest.books.get(&book_key);
                            let market_price = book
                                .and_then(|b| b.bids.best().or(b.asks.best()))
                                .map(|l| l.price)
                                .unwrap_or(dec!(0.50));

                            let window_open = latest
                                .window_open_prices
                                .get(&tick.asset)
                                .copied()
                                .unwrap_or(tick.price);

                            let outcome = if contract_entry.token_id.0.contains("up")
                                || contract_entry.token_id.0.contains("Up")
                                || contract_entry.token_id.0.contains("UP")
                            {
                                Outcome::Up
                            } else {
                                Outcome::Down
                            };

                            let secondary = latest
                                .cex_ticks
                                .values()
                                .find(|t| t.asset == tick.asset && t.source != tick.source)
                                .map(|t| t.price);

                            // P1-2: Use real execution health check
                            let exec_healthy = exec_engine.is_healthy().await;

                            let pipeline_input = PipelineInput {
                                contract: contract_entry.key.clone(),
                                asset: contract_entry.asset,
                                outcome,
                                duration: contract_entry.duration,
                                spot_price: tick.price,
                                window_open_price: window_open,
                                short_delta: if window_open > Decimal::ZERO {
                                    (tick.price - window_open) / window_open
                                } else {
                                    Decimal::ZERO
                                },
                                momentum: None,
                                volatility: None,
                                secondary_price: secondary,
                                book: book.cloned(),
                                market_price,
                                cex_feed_healthy: feed_health.is_healthy(FeedSource::Binance),
                                last_cex_tick: Some(tick.receipt_timestamp.0),
                                lock_state: contract_locks.lock_state(&contract_entry.key),
                                kill_switch_active: kill_switch.is_active(),
                                execution_healthy: exec_healthy,
                                current_positions: exposure_tracker.active_position_count(),
                                current_position_notional: exposure_tracker
                                    .contract_notional(&contract_entry.key),
                                equity: current_equity,
                                signal_age,
                                now,
                            };

                            let decision = SignalPipeline::evaluate(
                                &pipeline_input,
                                &cfg.strategy,
                                &cfg.risk,
                                &fee_schedule,
                                &sizing_mode,
                            );

                            match &decision {
                                SignalDecision::Accept(intent) => {
                                    // P0-5: Only mark as accepted if intent dispatch succeeds
                                    match intent_tx.try_send(*intent.clone()) {
                                        Ok(()) => {
                                            // Dispatch succeeded — lock contract and record
                                            metrics_clone.signals_accepted
                                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                                            contract_locks.lock(
                                                &intent.contract,
                                                Some(contract_entry.expiry),
                                            );

                                            let _ = event_tx.try_send(BotEvent::SignalAccepted(
                                                *intent.clone(),
                                            ));
                                        }
                                        Err(e) => {
                                            // P0-5: Dispatch failed — do NOT lock, emit rejection
                                            warn!(
                                                contract = %intent.contract,
                                                error = %e,
                                                "intent_dispatch_failed — backpressure"
                                            );
                                            metrics_clone.signals_rejected
                                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                                            let _ = event_tx.try_send(BotEvent::SignalRejected {
                                                contract: intent.contract.clone(),
                                                reasons: vec![RejectReason::ExecutionBackpressure],
                                                timestamp: now,
                                            });
                                        }
                                    }
                                }
                                SignalDecision::Reject { contract, reasons, timestamp } => {
                                    metrics_clone.signals_rejected
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                                    let _ = event_tx.try_send(BotEvent::SignalRejected {
                                        contract: contract.clone(),
                                        reasons: reasons.clone(),
                                        timestamp: *timestamp,
                                    });
                                }
                            }

                            // Record in simulation session
                            sim_session.write().process_signal(&decision);
                        }
                    }

                    BotEvent::BookUpdate(update) => {
                        latest.books.insert(
                            update.token_id.to_string(),
                            update.snapshot.clone(),
                        );
                    }

                    BotEvent::RtdsUpdate(_) => {
                        debug!("rtds update received");
                    }

                    BotEvent::OrderAck(ack) => {
                        fill_processor.process_ack(ack);
                    }

                    BotEvent::Fill(fill) => {
                        fill_processor.process_fill(fill);
                        metrics_clone.fills_received
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }

                    BotEvent::OrderStateChange(change) => {
                        fill_processor.process_state_change(change);
                    }

                    BotEvent::KillSwitch(ks_event) => {
                        // P1-1: Preserve the original kill switch reason
                        let reason = parse_kill_switch_reason(&ks_event.reason);
                        kill_switch.activate(reason);
                        metrics_clone.kill_switch_activations
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }

                    BotEvent::Resolution(_) => {
                        debug!("resolution event received");
                    }

                    BotEvent::SignalAccepted(_) | BotEvent::SignalRejected { .. } => {
                        // Already handled above, just persisted
                    }
                }

                sim_session.write().record_event();
            }

            // Process order intents for execution
            Some(intent) = intent_rx.recv() => {
                match exec_engine.submit_intent(&intent).await {
                    Ok(coid) => {
                        metrics_clone.orders_submitted
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        info!(
                            client_order_id = %coid,
                            contract = %intent.contract,
                            "order_submitted"
                        );
                    }
                    Err(e) => {
                        warn!(
                            contract = %intent.contract,
                            error = %e,
                            "order_submission_failed"
                        );
                        // Unlock contract on submission failure
                        contract_locks.cooldown(&intent.contract);
                    }
                }
            }

            // Shutdown
            _ = drain_token.cancelled() => {
                info!("shutdown signal received, draining events");
                while let Ok(event) = event_rx.try_recv() {
                    metrics_clone.inc_events();
                    persistence.try_persist(&event);
                    tracing::trace!(event = event.label(), "draining event");
                }
                break;
            }
        }
    }

    // --- shutdown sequence ---
    info!("shutdown initiated, stopping task groups");

    drop(event_tx);
    drop(intent_tx);

    let snap = metrics.snapshot();
    let sim_stats = sim_session.read().stats().clone();

    info!(
        events = snap.events_received,
        signals_accepted = snap.signals_accepted,
        signals_rejected = snap.signals_rejected,
        orders = snap.orders_submitted,
        fills = snap.fills_received,
        sim_pnl = %sim_stats.total_pnl,
        "shutdown complete"
    );

    Ok(())
}
