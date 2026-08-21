//! Errors raised by the sealed layer.

use alloc::string::String;
use thiserror::Error;

/// Result type alias for sealed-layer operations.
pub type Result<T> = core::result::Result<T, SealedError>;

/// Errors raised by the envelope codec, address derivation and the canonical
/// signing-payload construction.
///
/// Every variant's `Display` string is byte-identical to the
/// `offline_protocol_mls::MlsError` variant it maps onto, because these types
/// moved out of that enum and their rendered text reaches logs, telemetry and
/// two test suites that assert on substrings of it.
///
/// # Why this enum is not `#[non_exhaustive]`
///
/// Its sibling `MlsError` is, and the difference is deliberate.
/// `From<SealedError> for MlsError` in the MLS crate must map every variant to
/// the one it replaced; `#[non_exhaustive]` would force that impl to carry a
/// wildcard arm, and a wildcard arm is exactly how a new variant added here
/// silently starts rendering as some other error's text. Leaving the enum
/// exhaustive makes adding a variant a compile error in the MLS crate until
/// someone maps it, which is the point. Adding a variant is a breaking change
/// for external consumers, and the workspace already moves every crate's
/// version together.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SealedError {
    /// Failed to serialize data.
    #[error("Serialization failed: {0}")]
    Serialization(String),

    /// Failed to deserialize data.
    #[error("Deserialization failed: {0}")]
    Deserialization(String),

    /// Group id failed storage-key validation (empty, oversized, or
    /// containing path-traversal / storage-hostile characters).
    #[error("Invalid group id: {0}")]
    InvalidGroupId(String),

    /// Invalid public key format.
    #[error("Invalid public key: {0}")]
    InvalidPublicKey(String),

    /// A field of a canonical signing payload does not fit its `u32` length
    /// prefix.
    ///
    /// Carried as its own variant rather than folded into
    /// [`SealedError::Serialization`] so that both callers keep the exact
    /// message they raised before this construction moved here: the engine
    /// renders it bare into `Error::Other`, and the MLS crate wraps it in
    /// `MlsError::Serialization`, which prepends its own prefix.
    #[error("Field too large for canonical payload length prefix: {0} bytes")]
    FieldTooLarge(usize),
}
