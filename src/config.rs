use std::fmt;
use std::time::Duration;

use rust_decimal::Decimal;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required config: {0}")]
    Missing(String),

    #[error("invalid config value for {key}: {reason}")]
    Invalid { key: String, reason: String },
}

// ---------------------------------------------------------------------------
// RunMode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    DryRun,
    Simulation,
    Paper,
    Live,
}

impl fmt::Display for RunMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunMode::DryRun => write!(f, "dry_run"),
            RunMode::Simulation => write!(f, "simulation"),
            RunMode::Paper => write!(f, "paper"),
            RunMode::Live => write!(f, "live"),
        }
    }
}

impl RunMode {
    fn parse(s: &str) -> Result<Self, ConfigError> {
        match s.to_lowercase().as_str() {
            "dry_run" | "dryrun" => Ok(RunMode::DryRun),
            "simulation" | "sim" => Ok(RunMode::Simulation),
            "paper" => Ok(RunMode::Paper),
            "live" => Ok(RunMode::Live),
            other => Err(ConfigError::Invalid {
                key: "BOT_MODE".into(),
                reason: format!("unknown mode '{other}', expected: dry_run|simulation|paper|live"),
            }),
        }
    }

    pub fn is_live(self) -> bool {
        self == RunMode::Live
    }

    pub fn allows_real_orders(self) -> bool {
        matches!(self, RunMode::Paper | RunMode::Live)
    }
}

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub mode: RunMode,
    pub region_tag: String,

    pub polymarket: PolymarketConfig,
    pub binance: BinanceConfig,
    pub coinbase: CoinbaseConfig,

    pub strategy: StrategyConfig,
    pub risk: RiskConfig,
    pub execution: ExecutionConfig,
    pub telemetry: TelemetryConfig,
}

// ---------------------------------------------------------------------------
// Polymarket
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PolymarketConfig {
    pub api_key: Option<String>,
    pub secret: Option<String>,
    pub passphrase: Option<String>,
    pub market_ws_url: String,
    pub user_ws_url: String,
    pub rtds_ws_url: String,
    pub rest_base_url: String,
    pub discovery_refresh: Duration,
}

// ---------------------------------------------------------------------------
// CEX feeds
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BinanceConfig {
    pub ws_url: String,
    pub reconnect_backoff_ms: u64,
    pub max_reconnect_attempts: u32,
    pub stale_threshold: Duration,
}

#[derive(Debug, Clone)]
pub struct CoinbaseConfig {
    pub enabled: bool,
    pub ws_url: String,
    pub reconnect_backoff_ms: u64,
    pub stale_threshold: Duration,
}

// ---------------------------------------------------------------------------
// Strategy thresholds (separate for 5m and 15m)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MarketRegimeThresholds {
    pub min_net_edge: Decimal,
    pub min_confidence: Decimal,
    pub min_book_depth_usdc: Decimal,
    pub max_hold: Duration,
    pub stale_feed_tolerance: Duration,
    pub stale_book_tolerance: Duration,
    pub cooldown: Duration,
}

#[derive(Debug, Clone)]
pub struct StrategyConfig {
    pub five_min: MarketRegimeThresholds,
    pub fifteen_min: MarketRegimeThresholds,
    pub latency_decay_buffer: Duration,
}

// ---------------------------------------------------------------------------
// Risk
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RiskConfig {
    pub max_position_per_market: Decimal,
    pub max_concurrent_positions: u32,
    pub max_gross_exposure: Decimal,
    pub max_daily_drawdown: Decimal,
    pub max_total_drawdown: Decimal,
    pub max_consecutive_losses: u32,
    pub max_notional_per_order: Decimal,
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    pub max_retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub stale_signal_threshold: Duration,
    pub max_concurrent_orders: u32,
}

// ---------------------------------------------------------------------------
// Telemetry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub log_level: String,
    pub log_json: bool,
    pub event_log_path: String,
}

// ---------------------------------------------------------------------------
// Loading from environment
// ---------------------------------------------------------------------------

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let _ = dotenvy::dotenv(); // optional .env file

        let mode = RunMode::parse(&env_or("BOT_MODE", "simulation"))?;

        // Live mode requires explicit credentials
        if mode.is_live() {
            require_env("POLYMARKET_API_KEY")?;
            require_env("POLYMARKET_SECRET")?;
            require_env("POLYMARKET_PASSPHRASE")?;
        }

        Ok(AppConfig {
            mode,
            region_tag: env_or("REGION_TAG", "local"),

            polymarket: PolymarketConfig {
                api_key: opt_env("POLYMARKET_API_KEY"),
                secret: opt_env("POLYMARKET_SECRET"),
                passphrase: opt_env("POLYMARKET_PASSPHRASE"),
                market_ws_url: env_or(
                    "POLYMARKET_MARKET_WS_URL",
                    "wss://ws-subscriptions-clob.polymarket.com/ws/market",
                ),
                user_ws_url: env_or(
                    "POLYMARKET_USER_WS_URL",
                    "wss://ws-subscriptions-clob.polymarket.com/ws/user",
                ),
                rtds_ws_url: env_or(
                    "POLYMARKET_RTDS_WS_URL",
                    "wss://ws-subscriptions-clob.polymarket.com/ws/rtds",
                ),
                rest_base_url: env_or("POLYMARKET_REST_URL", "https://clob.polymarket.com"),
                discovery_refresh: Duration::from_secs(env_parse("DISCOVERY_REFRESH_SECS", 30)?),
            },

            binance: BinanceConfig {
                ws_url: env_or("BINANCE_WS_URL", "wss://stream.binance.com:9443"),
                reconnect_backoff_ms: env_parse("BINANCE_RECONNECT_BACKOFF_MS", 1000)?,
                max_reconnect_attempts: env_parse("BINANCE_MAX_RECONNECT", 10)?,
                stale_threshold: Duration::from_millis(env_parse("BINANCE_STALE_MS", 3000)?),
            },

            coinbase: CoinbaseConfig {
                enabled: env_parse("COINBASE_ENABLED", false)?,
                ws_url: env_or("COINBASE_WS_URL", "wss://ws-feed.exchange.coinbase.com"),
                reconnect_backoff_ms: env_parse("COINBASE_RECONNECT_BACKOFF_MS", 1000)?,
                stale_threshold: Duration::from_millis(env_parse("COINBASE_STALE_MS", 5000)?),
            },

            strategy: StrategyConfig {
                five_min: MarketRegimeThresholds {
                    min_net_edge: dec_or("STRATEGY_5M_MIN_NET_EDGE", "0.02")?,
                    min_confidence: dec_or("STRATEGY_5M_MIN_CONFIDENCE", "0.60")?,
                    min_book_depth_usdc: dec_or("STRATEGY_5M_MIN_DEPTH_USDC", "100")?,
                    max_hold: Duration::from_secs(env_parse("STRATEGY_5M_MAX_HOLD_SECS", 240)?),
                    stale_feed_tolerance: Duration::from_millis(env_parse(
                        "STRATEGY_5M_STALE_FEED_MS",
                        2000,
                    )?),
                    stale_book_tolerance: Duration::from_millis(env_parse(
                        "STRATEGY_5M_STALE_BOOK_MS",
                        3000,
                    )?),
                    cooldown: Duration::from_secs(env_parse("STRATEGY_5M_COOLDOWN_SECS", 10)?),
                },
                fifteen_min: MarketRegimeThresholds {
                    min_net_edge: dec_or("STRATEGY_15M_MIN_NET_EDGE", "0.015")?,
                    min_confidence: dec_or("STRATEGY_15M_MIN_CONFIDENCE", "0.55")?,
                    min_book_depth_usdc: dec_or("STRATEGY_15M_MIN_DEPTH_USDC", "100")?,
                    max_hold: Duration::from_secs(env_parse("STRATEGY_15M_MAX_HOLD_SECS", 780)?),
                    stale_feed_tolerance: Duration::from_millis(env_parse(
                        "STRATEGY_15M_STALE_FEED_MS",
                        3000,
                    )?),
                    stale_book_tolerance: Duration::from_millis(env_parse(
                        "STRATEGY_15M_STALE_BOOK_MS",
                        5000,
                    )?),
                    cooldown: Duration::from_secs(env_parse("STRATEGY_15M_COOLDOWN_SECS", 15)?),
                },
                latency_decay_buffer: Duration::from_millis(env_parse(
                    "STRATEGY_LATENCY_DECAY_MS",
                    200,
                )?),
            },

            risk: RiskConfig {
                max_position_per_market: dec_or("RISK_MAX_POS_PER_MARKET", "50")?,
                max_concurrent_positions: env_parse("RISK_MAX_CONCURRENT_POS", 4)?,
                max_gross_exposure: dec_or("RISK_MAX_GROSS_EXPOSURE", "200")?,
                max_daily_drawdown: dec_or("RISK_MAX_DAILY_DD", "50")?,
                max_total_drawdown: dec_or("RISK_MAX_TOTAL_DD", "100")?,
                max_consecutive_losses: env_parse("RISK_MAX_CONSEC_LOSSES", 5)?,
                max_notional_per_order: dec_or("RISK_MAX_NOTIONAL_ORDER", "25")?,
            },

            execution: ExecutionConfig {
                max_retry_attempts: env_parse("EXEC_MAX_RETRIES", 2)?,
                retry_backoff_ms: env_parse("EXEC_RETRY_BACKOFF_MS", 500)?,
                stale_signal_threshold: Duration::from_millis(env_parse(
                    "EXEC_STALE_SIGNAL_MS",
                    1000,
                )?),
                max_concurrent_orders: env_parse("EXEC_MAX_CONCURRENT_ORDERS", 2)?,
            },

            telemetry: TelemetryConfig {
                log_level: env_or("RUST_LOG", "info"),
                log_json: env_parse("LOG_JSON", false)?,
                event_log_path: env_or("EVENT_LOG_PATH", "data/events.jsonl"),
            },
        })
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.mode.is_live() {
            if self.polymarket.api_key.is_none() {
                return Err(ConfigError::Missing(
                    "POLYMARKET_API_KEY for live mode".into(),
                ));
            }
            if self.risk.max_notional_per_order > Decimal::from(100) {
                return Err(ConfigError::Invalid {
                    key: "RISK_MAX_NOTIONAL_ORDER".into(),
                    reason: "live mode cap must be <= 100 USDC for initial rollout".into(),
                });
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Env helpers
// ---------------------------------------------------------------------------

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn opt_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn require_env(key: &str) -> Result<String, ConfigError> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ConfigError::Missing(key.into()))
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> Result<T, ConfigError>
where
    T::Err: fmt::Display,
{
    match std::env::var(key) {
        Ok(val) if !val.is_empty() => val.parse::<T>().map_err(|e| ConfigError::Invalid {
            key: key.into(),
            reason: e.to_string(),
        }),
        _ => Ok(default),
    }
}

fn dec_or(key: &str, default: &str) -> Result<Decimal, ConfigError> {
    let raw = env_or(key, default);
    raw.parse::<Decimal>().map_err(|e| ConfigError::Invalid {
        key: key.into(),
        reason: e.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_run_modes() {
        assert_eq!(RunMode::parse("simulation").unwrap(), RunMode::Simulation);
        assert_eq!(RunMode::parse("sim").unwrap(), RunMode::Simulation);
        assert_eq!(RunMode::parse("dry_run").unwrap(), RunMode::DryRun);
        assert_eq!(RunMode::parse("dryrun").unwrap(), RunMode::DryRun);
        assert_eq!(RunMode::parse("paper").unwrap(), RunMode::Paper);
        assert_eq!(RunMode::parse("live").unwrap(), RunMode::Live);
        assert!(RunMode::parse("unknown").is_err());
    }

    #[test]
    fn default_config_is_simulation() {
        // No env vars set — should default to simulation
        let cfg = AppConfig::from_env().unwrap();
        assert_eq!(cfg.mode, RunMode::Simulation);
        assert!(!cfg.mode.is_live());
    }

    #[test]
    fn live_mode_is_flagged_correctly() {
        assert!(RunMode::Live.is_live());
        assert!(RunMode::Live.allows_real_orders());
        assert!(RunMode::Paper.allows_real_orders());
        assert!(!RunMode::Simulation.is_live());
        assert!(!RunMode::Simulation.allows_real_orders());
        assert!(!RunMode::DryRun.allows_real_orders());
    }

    #[test]
    fn risk_defaults_are_conservative() {
        let cfg = AppConfig::from_env().unwrap();
        assert!(cfg.risk.max_notional_per_order <= Decimal::from(100));
        assert!(cfg.risk.max_concurrent_positions <= 10);
        assert!(cfg.risk.max_consecutive_losses <= 10);
    }
}
