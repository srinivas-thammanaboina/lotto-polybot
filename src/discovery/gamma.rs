use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::config::PolymarketConfig;
use crate::domain::market::{
    Asset, MarketDuration, MarketId, MarketMeta, Outcome, OutcomeMeta, TokenId,
};

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("no supported markets found")]
    NoMarketsFound,

    #[error("parse error: {0}")]
    Parse(String),
}

/// Raw response from Gamma /markets endpoint.
#[derive(Debug, Deserialize)]
struct GammaMarket {
    #[serde(rename = "conditionId")]
    condition_id: Option<String>,
    question: Option<String>,
    slug: Option<String>,
    active: Option<bool>,
    closed: Option<bool>,
    #[serde(rename = "endDateIso")]
    end_date_iso: Option<String>,
    tokens: Option<Vec<GammaToken>>,
}

#[derive(Debug, Deserialize)]
struct GammaToken {
    token_id: Option<String>,
    outcome: Option<String>,
}

/// Discover active BTC/ETH 5m and 15m markets from the Gamma API.
pub async fn discover(
    client: &Client,
    config: &PolymarketConfig,
) -> Result<Vec<MarketMeta>, DiscoveryError> {
    let url = format!("{}/markets", config.rest_base_url);

    // Query for active crypto markets
    let resp = client
        .get(&url)
        .query(&[
            ("active", "true"),
            ("closed", "false"),
            ("limit", "100"),
        ])
        .send()
        .await?
        .error_for_status()?;

    let raw_markets: Vec<GammaMarket> = resp.json().await?;
    debug!(raw_count = raw_markets.len(), "fetched markets from Gamma");

    let mut results = Vec::new();

    for raw in &raw_markets {
        if let Some(meta) = try_parse_market(raw) {
            results.push(meta);
        }
    }

    info!(
        total_raw = raw_markets.len(),
        matched = results.len(),
        "discovery complete"
    );

    if results.is_empty() {
        warn!("discovery found zero supported markets");
    }

    Ok(results)
}

/// Try to parse a raw Gamma market into our domain MarketMeta.
/// Returns None if the market doesn't match our supported scope.
fn try_parse_market(raw: &GammaMarket) -> Option<MarketMeta> {
    let question = raw.question.as_deref().unwrap_or("");
    let slug = raw.slug.as_deref().unwrap_or("");

    // Must be active and not closed
    if raw.active != Some(true) || raw.closed == Some(true) {
        return None;
    }

    // Detect asset and duration from question/slug
    let (asset, duration) = detect_asset_duration(question, slug)?;

    // Parse expiry
    let expiry = raw
        .end_date_iso
        .as_deref()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())?;

    // Skip already-expired markets
    if expiry <= Utc::now() {
        return None;
    }

    // Parse condition_id as market ID
    let market_id = MarketId(raw.condition_id.as_deref()?.to_string());

    // Parse tokens/outcomes
    let outcomes = parse_outcomes(raw)?;
    if outcomes.is_empty() {
        return None;
    }

    Some(MarketMeta {
        market_id,
        asset,
        duration,
        expiry,
        outcomes,
        active: true,
        discovered_at: Utc::now(),
    })
}

/// Detect if a market question/slug matches our supported scope.
fn detect_asset_duration(question: &str, slug: &str) -> Option<(Asset, MarketDuration)> {
    let q_lower = question.to_lowercase();
    let s_lower = slug.to_lowercase();
    let combined = format!("{q_lower} {s_lower}");

    let asset = if combined.contains("bitcoin") || combined.contains("btc") {
        Asset::BTC
    } else if combined.contains("ethereum") || combined.contains("eth") {
        Asset::ETH
    } else {
        return None;
    };

    // Check 15m before 5m — "15min" contains "5min"
    let duration = if combined.contains("15 min")
        || combined.contains("15-min")
        || combined.contains("15min")
    {
        MarketDuration::FifteenMin
    } else if combined.contains("5 min")
        || combined.contains("5-min")
        || combined.contains("5min")
    {
        MarketDuration::FiveMin
    } else {
        return None;
    };

    Some((asset, duration))
}

/// Parse outcome tokens from a raw Gamma market.
fn parse_outcomes(raw: &GammaMarket) -> Option<Vec<OutcomeMeta>> {
    let tokens = raw.tokens.as_ref()?;
    let mut outcomes = Vec::new();

    for token in tokens {
        let token_id = token.token_id.as_deref()?;
        let outcome_str = token.outcome.as_deref().unwrap_or("");

        let outcome = match outcome_str.to_lowercase().as_str() {
            "yes" | "up" => Outcome::Up,
            "no" | "down" => Outcome::Down,
            _ => continue,
        };

        outcomes.push(OutcomeMeta {
            token_id: TokenId(token_id.to_string()),
            outcome,
            label: outcome_str.to_string(),
        });
    }

    if outcomes.is_empty() {
        None
    } else {
        Some(outcomes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_btc_5m() {
        let (asset, dur) =
            detect_asset_duration("Will Bitcoin go up in the next 5 minutes?", "btc-5min-up")
                .unwrap();
        assert_eq!(asset, Asset::BTC);
        assert_eq!(dur, MarketDuration::FiveMin);
    }

    #[test]
    fn detect_eth_15m() {
        let (asset, dur) =
            detect_asset_duration("Will Ethereum go up in the next 15 minutes?", "eth-15min")
                .unwrap();
        assert_eq!(asset, Asset::ETH);
        assert_eq!(dur, MarketDuration::FifteenMin);
    }

    #[test]
    fn reject_unsupported_market() {
        assert!(detect_asset_duration("Will Trump win?", "trump-win").is_none());
    }

    #[test]
    fn reject_unsupported_duration() {
        assert!(
            detect_asset_duration("Will Bitcoin go up in 1 hour?", "btc-1h-up").is_none()
        );
    }
}
