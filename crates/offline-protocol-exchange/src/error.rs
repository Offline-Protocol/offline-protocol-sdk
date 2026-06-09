//! Error types for the capability exchange.

use thiserror::Error;

/// Errors produced by the capability exchange layer.
#[derive(Debug, Error)]
pub enum ExchangeError {
    /// The listing envelope embedded in a service descriptor is malformed.
    #[error("Invalid listing envelope: {0}")]
    InvalidEnvelope(String),

    /// The listing envelope carries an unsupported version.
    #[error("Unsupported listing envelope version: {0}")]
    UnsupportedEnvelopeVersion(u16),

    /// A listing failed validation before publish.
    #[error("Invalid listing: {0}")]
    InvalidListing(String),

    /// Signing failed (identity key unavailable or signer error).
    #[error("Signing failed: {0}")]
    SigningFailed(String),

    /// Signature verification failed structurally (not merely "invalid signature").
    #[error("Verification failed: {0}")]
    VerificationFailed(String),

    /// A priced operation requires an established encrypted session.
    #[error("Encrypted session required: {0}")]
    EncryptionRequired(String),

    /// The listing is unknown to the local exchange (not discovered or not verified).
    #[error("Unknown listing: {0}")]
    UnknownListing(String),

    /// The listing's attestation could not be verified, so a paid operation is refused.
    #[error("Listing attestation not verified: {0}")]
    AttestationNotVerified(String),

    /// The prepaid balance cannot cover the operation.
    #[error("Insufficient balance: need {needed} {currency} minor units, available {available}")]
    InsufficientBalance {
        /// Currency identifier.
        currency: String,
        /// Minor units required for the hold.
        needed: u64,
        /// Minor units currently available.
        available: u64,
    },

    /// A monetary computation overflowed.
    #[error("Amount overflow: {0}")]
    AmountOverflow(String),

    /// A usage receipt failed validation.
    #[error("Invalid receipt: {0}")]
    InvalidReceipt(String),

    /// The referenced invocation is unknown (no pending hold / billing entry).
    #[error("Unknown invocation: {0}")]
    UnknownInvocation(String),

    /// An adapter artifact failed integrity verification.
    #[error("Artifact verification failed: {0}")]
    ArtifactVerificationFailed(String),

    /// Serialization or deserialization failure.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Settlement backend failure.
    #[error("Settlement error: {0}")]
    Settlement(String),
}

/// Convenience result alias for exchange operations.
pub type ExchangeResult<T> = Result<T, ExchangeError>;
