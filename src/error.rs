use thiserror::Error;

use crate::config::ConfigError;

/// Top-level application error type.
#[derive(Debug, Error)]
pub enum BotError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("feed error: {0}")]
    Feed(String),

    #[error("execution error: {0}")]
    Execution(String),

    #[error("risk error: {0}")]
    Risk(String),

    #[error("discovery error: {0}")]
    Discovery(String),

    #[error("internal error: {0}")]
    Internal(String),
}
