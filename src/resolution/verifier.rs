//! Post-expiry outcome verification from Polymarket resolved data.
//!
//! Verifies final P&L against Polymarket's official market resolution,
//! not Binance/Coinbase proxy prices. Feeds results into accounting
//! and drawdown logic.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::domain::ledger::{ResolutionOutcome, VerifiedOutcome};
use crate::domain::market::{ContractKey, MarketId, TokenId};
use crate::domain::signal::Side;

// ---------------------------------------------------------------------------
// Resolution data from Polymarket
// ---------------------------------------------------------------------------

/// Raw resolution data fetched from Polymarket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionData {
    pub market_id: MarketId,
    pub winning_token: Option<TokenId>,
    pub resolved_at: DateTime<Utc>,
    /// Payout price: 1.0 for winning token, 0.0 for losing.
    pub payout_price: Decimal,
}

// ---------------------------------------------------------------------------
// Verification input
// ---------------------------------------------------------------------------

/// What we need to verify a position's final P&L.
#[derive(Debug, Clone)]
pub struct VerificationInput {
    pub contract: ContractKey,
    pub side: Side,
    pub entry_price: Decimal,
    pub size: Decimal,
    pub fees_paid: Decimal,
}

// ---------------------------------------------------------------------------
// Verification result
// ---------------------------------------------------------------------------

/// Result of verifying a position against official resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub contract: ContractKey,
    pub outcome: ResolutionOutcome,
    pub payout_price: Decimal,
    pub realized_pnl: Decimal,
    pub entry_price: Decimal,
    pub size: Decimal,
    pub fees_paid: Decimal,
    pub verified_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Resolution verifier
// ---------------------------------------------------------------------------

/// Verifies position outcomes against Polymarket's official resolution.
pub struct ResolutionVerifier;

impl ResolutionVerifier {
    /// Verify a position against resolution data.
    ///
    /// For a Buy position on a winning token:
    ///   P&L = (payout_price - entry_price) * size - fees
    ///
    /// For a Buy position on a losing token:
    ///   P&L = (0 - entry_price) * size - fees
    ///
    /// Sell positions are the inverse.
    pub fn verify(input: &VerificationInput, resolution: &ResolutionData) -> VerificationResult {
        let outcome = match &resolution.winning_token {
            Some(winner) if winner == &input.contract.token_id => ResolutionOutcome::Yes,
            Some(_) => ResolutionOutcome::No,
            None => ResolutionOutcome::Unknown,
        };

        let payout = match outcome {
            ResolutionOutcome::Yes => dec!(1.0),
            ResolutionOutcome::No => dec!(0.0),
            ResolutionOutcome::Unknown => resolution.payout_price,
        };

        let realized_pnl = match input.side {
            Side::Buy => (payout - input.entry_price) * input.size - input.fees_paid,
            Side::Sell => (input.entry_price - payout) * input.size - input.fees_paid,
        };

        let result = VerificationResult {
            contract: input.contract.clone(),
            outcome,
            payout_price: payout,
            realized_pnl,
            entry_price: input.entry_price,
            size: input.size,
            fees_paid: input.fees_paid,
            verified_at: Utc::now(),
        };

        info!(
            contract = %result.contract,
            outcome = ?result.outcome,
            pnl = %result.realized_pnl,
            "resolution_verified"
        );

        result
    }

    /// Convert a verification result into a VerifiedOutcome domain object.
    pub fn to_verified_outcome(result: &VerificationResult) -> VerifiedOutcome {
        VerifiedOutcome {
            contract: result.contract.clone(),
            outcome: result.outcome,
            payout_price: result.payout_price,
            realized_pnl: result.realized_pnl,
            verified_at: result.verified_at,
        }
    }

    /// Batch verify multiple positions.
    pub fn verify_batch(
        positions: &[VerificationInput],
        resolution: &ResolutionData,
    ) -> Vec<VerificationResult> {
        positions
            .iter()
            .map(|input| Self::verify(input, resolution))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_contract() -> ContractKey {
        ContractKey {
            market_id: MarketId("mkt1".into()),
            token_id: TokenId("tok-up".into()),
        }
    }

    fn winning_resolution() -> ResolutionData {
        ResolutionData {
            market_id: MarketId("mkt1".into()),
            winning_token: Some(TokenId("tok-up".into())),
            resolved_at: Utc::now(),
            payout_price: dec!(1.0),
        }
    }

    fn losing_resolution() -> ResolutionData {
        ResolutionData {
            market_id: MarketId("mkt1".into()),
            winning_token: Some(TokenId("tok-down".into())),
            resolved_at: Utc::now(),
            payout_price: dec!(0.0),
        }
    }

    #[test]
    fn buy_winning_token_profit() {
        let input = VerificationInput {
            contract: test_contract(),
            side: Side::Buy,
            entry_price: dec!(0.55),
            size: dec!(10),
            fees_paid: dec!(0.10),
        };
        let result = ResolutionVerifier::verify(&input, &winning_resolution());
        assert_eq!(result.outcome, ResolutionOutcome::Yes);
        assert_eq!(result.payout_price, dec!(1.0));
        // P&L = (1.0 - 0.55) * 10 - 0.10 = 4.50 - 0.10 = 4.40
        assert_eq!(result.realized_pnl, dec!(4.40));
    }

    #[test]
    fn buy_losing_token_loss() {
        let input = VerificationInput {
            contract: test_contract(),
            side: Side::Buy,
            entry_price: dec!(0.55),
            size: dec!(10),
            fees_paid: dec!(0.10),
        };
        let result = ResolutionVerifier::verify(&input, &losing_resolution());
        assert_eq!(result.outcome, ResolutionOutcome::No);
        // P&L = (0.0 - 0.55) * 10 - 0.10 = -5.50 - 0.10 = -5.60
        assert_eq!(result.realized_pnl, dec!(-5.60));
    }

    #[test]
    fn sell_winning_token_loss() {
        let input = VerificationInput {
            contract: test_contract(),
            side: Side::Sell,
            entry_price: dec!(0.55),
            size: dec!(10),
            fees_paid: dec!(0.10),
        };
        let result = ResolutionVerifier::verify(&input, &winning_resolution());
        // P&L = (0.55 - 1.0) * 10 - 0.10 = -4.50 - 0.10 = -4.60
        assert_eq!(result.realized_pnl, dec!(-4.60));
    }

    #[test]
    fn sell_losing_token_profit() {
        let input = VerificationInput {
            contract: test_contract(),
            side: Side::Sell,
            entry_price: dec!(0.55),
            size: dec!(10),
            fees_paid: dec!(0.10),
        };
        let result = ResolutionVerifier::verify(&input, &losing_resolution());
        // P&L = (0.55 - 0.0) * 10 - 0.10 = 5.50 - 0.10 = 5.40
        assert_eq!(result.realized_pnl, dec!(5.40));
    }

    #[test]
    fn unknown_resolution() {
        let resolution = ResolutionData {
            market_id: MarketId("mkt1".into()),
            winning_token: None,
            resolved_at: Utc::now(),
            payout_price: dec!(0.50),
        };
        let input = VerificationInput {
            contract: test_contract(),
            side: Side::Buy,
            entry_price: dec!(0.55),
            size: dec!(10),
            fees_paid: dec!(0.10),
        };
        let result = ResolutionVerifier::verify(&input, &resolution);
        assert_eq!(result.outcome, ResolutionOutcome::Unknown);
    }

    #[test]
    fn batch_verify() {
        let positions = vec![
            VerificationInput {
                contract: test_contract(),
                side: Side::Buy,
                entry_price: dec!(0.55),
                size: dec!(10),
                fees_paid: dec!(0.10),
            },
            VerificationInput {
                contract: ContractKey {
                    market_id: MarketId("mkt1".into()),
                    token_id: TokenId("tok-down".into()),
                },
                side: Side::Buy,
                entry_price: dec!(0.45),
                size: dec!(10),
                fees_paid: dec!(0.10),
            },
        ];
        let results = ResolutionVerifier::verify_batch(&positions, &winning_resolution());
        assert_eq!(results.len(), 2);
        assert!(results[0].realized_pnl > Decimal::ZERO); // bought winner
        assert!(results[1].realized_pnl < Decimal::ZERO); // bought loser
    }

    #[test]
    fn to_verified_outcome_converts() {
        let input = VerificationInput {
            contract: test_contract(),
            side: Side::Buy,
            entry_price: dec!(0.55),
            size: dec!(10),
            fees_paid: dec!(0.10),
        };
        let result = ResolutionVerifier::verify(&input, &winning_resolution());
        let outcome = ResolutionVerifier::to_verified_outcome(&result);
        assert_eq!(outcome.outcome, ResolutionOutcome::Yes);
        assert_eq!(outcome.realized_pnl, result.realized_pnl);
    }
}
