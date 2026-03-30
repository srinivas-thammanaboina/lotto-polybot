//! Resolution data fetcher.
//!
//! Fetches official resolved market outcomes from Polymarket REST API.
//! This is the authoritative source for final P&L — not CEX prices.

use rust_decimal::Decimal;
use serde::Deserialize;
use thiserror::Error;
use tracing::info;

use crate::domain::market::{MarketId, TokenId};
use crate::resolution::verifier::ResolutionData;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ResolutionFetchError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("market not yet resolved: {0}")]
    NotResolved(String),
}

// ---------------------------------------------------------------------------
// Raw API response shape
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MarketResponse {
    condition_id: Option<String>,
    question: Option<String>,
    resolved: Option<bool>,
    winning_outcome: Option<String>,
    tokens: Option<Vec<TokenResponse>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TokenResponse {
    token_id: Option<String>,
    outcome: Option<String>,
    winner: Option<bool>,
}

// ---------------------------------------------------------------------------
// Fetcher
// ---------------------------------------------------------------------------

/// Fetches resolution data from Polymarket REST API.
pub struct ResolutionFetcher {
    client: reqwest::Client,
    base_url: String,
}

impl ResolutionFetcher {
    pub fn new(client: reqwest::Client, base_url: &str) -> Self {
        Self {
            client,
            base_url: base_url.to_string(),
        }
    }

    /// Fetch resolution data for a market.
    pub async fn fetch(
        &self,
        market_id: &MarketId,
    ) -> Result<ResolutionData, ResolutionFetchError> {
        let url = format!("{}/markets/{}", self.base_url, market_id);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ResolutionFetchError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ResolutionFetchError::Http(format!(
                "status {}",
                resp.status()
            )));
        }

        let body: MarketResponse = resp
            .json()
            .await
            .map_err(|e| ResolutionFetchError::Parse(e.to_string()))?;

        if !body.resolved.unwrap_or(false) {
            return Err(ResolutionFetchError::NotResolved(market_id.to_string()));
        }

        // Find the winning token
        let winning_token = body.tokens.as_ref().and_then(|tokens| {
            tokens
                .iter()
                .find(|t| t.winner.unwrap_or(false))
                .and_then(|t| t.token_id.as_ref())
                .map(|id| TokenId(id.clone()))
        });

        let payout = if winning_token.is_some() {
            Decimal::ONE
        } else {
            Decimal::ZERO
        };

        let data = ResolutionData {
            market_id: market_id.clone(),
            winning_token,
            resolved_at: chrono::Utc::now(),
            payout_price: payout,
        };

        info!(
            market_id = %market_id,
            winning_token = ?data.winning_token,
            "resolution_fetched"
        );

        Ok(data)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetcher_constructs() {
        let client = reqwest::Client::new();
        let fetcher = ResolutionFetcher::new(client, "https://clob.polymarket.com");
        assert_eq!(fetcher.base_url, "https://clob.polymarket.com");
    }
}
