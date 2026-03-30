//! Order signing logic.
//!
//! Abstracts the signing path so it can be swapped between real CLOB signing
//! (live mode) and a no-op pass-through (simulation/paper mode).

use thiserror::Error;

// ---------------------------------------------------------------------------
// Signer errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SignerError {
    #[error("missing signing credentials")]
    MissingCredentials,
    #[error("signing failed: {0}")]
    SigningFailed(String),
}

// ---------------------------------------------------------------------------
// Signer trait
// ---------------------------------------------------------------------------

/// Abstracts the order signing path.
pub trait OrderSigner: Send + Sync {
    /// Sign the order payload and return the signature string.
    fn sign(&self, payload: &[u8]) -> Result<String, SignerError>;
}

// ---------------------------------------------------------------------------
// No-op signer for simulation
// ---------------------------------------------------------------------------

/// Pass-through signer for simulation/paper mode.
pub struct NoOpSigner;

impl OrderSigner for NoOpSigner {
    fn sign(&self, _payload: &[u8]) -> Result<String, SignerError> {
        Ok("sim-signature".to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_signer_returns_sim_signature() {
        let signer = NoOpSigner;
        let sig = signer.sign(b"test payload").unwrap();
        assert_eq!(sig, "sim-signature");
    }
}
