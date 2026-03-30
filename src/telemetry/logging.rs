use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

use crate::config::TelemetryConfig;

/// Initialize the global tracing subscriber.
/// Must be called once at startup before any tracing macros fire.
pub fn init(cfg: &TelemetryConfig) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.log_level));

    if cfg.log_json {
        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json().with_target(true));
        subscriber.init();
    } else {
        let subscriber = tracing_subscriber::registry().with(filter).with(
            fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_file(false),
        );
        subscriber.init();
    }
}
