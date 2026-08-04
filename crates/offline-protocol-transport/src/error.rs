//! Transport layer errors.

use thiserror::Error;

/// Result type alias using the transport Error type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in the transport layer.
///
/// `Display` (the `#[error(...)]` text) is human-facing and free to change.
/// [`Error::code`] is the stable, machine-readable contract — downstream
/// telemetry classifies on the code, never on the `Display` wording.
// Adding a variant to a public error enum is a breaking change without
// this attribute; downstream crates must carry a wildcard arm.
#[non_exhaustive]
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    /// Transport is not available or supported.
    #[error("Transport not available: {0}")]
    TransportNotAvailable(String),

    /// Recipient is not reachable via this transport.
    /// Unlike `TransportNotAvailable`, the transport itself is healthy --
    /// only this specific peer cannot be reached through it.
    #[error("Peer not reachable: {0}")]
    PeerNotReachable(String),

    /// Failed to send message.
    #[error("Send failed: {0}")]
    SendFailed(String),

    /// Failed to receive message.
    #[error("Receive failed: {0}")]
    ReceiveFailed(String),

    /// Transport configuration error.
    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    /// Serialization/deserialization error.
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Core error propagated from offline-protocol-core.
    #[error("Core error: {0}")]
    Core(#[from] offline_protocol_core::Error),

    /// Message exceeds the configured maximum size.
    #[error("Message too large: {0} bytes exceeds limit of {1} bytes")]
    MessageTooLarge(usize, usize),

    /// Cryptographic operation failed (key derivation, signing).
    #[error("Crypto error: {0}")]
    CryptoError(String),

    /// Generic error.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Returns a stable, non-localized machine-readable error code.
    ///
    /// Unlike [`Display`](std::fmt::Display), this value is a contract:
    /// downstream consumers (e.g. telemetry classifiers) switch on it instead
    /// of substring-matching human-readable error text. Keep these strings
    /// stable — changing one reclassifies every event that carries it.
    ///
    /// The match is intentionally exhaustive (no wildcard arm) so that adding
    /// a new variant fails to compile until it is assigned a code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::TransportNotAvailable(_) => "TRANSPORT_NOT_AVAILABLE",
            Self::PeerNotReachable(_) => "PEER_NOT_REACHABLE",
            Self::SendFailed(_) => "SEND_FAILED",
            Self::ReceiveFailed(_) => "RECEIVE_FAILED",
            Self::ConfigurationError(_) => "CONFIGURATION_ERROR",
            Self::SerializationError(_) => "SERIALIZATION_ERROR",
            Self::Core(_) => "CORE_ERROR",
            Self::MessageTooLarge(_, _) => "MESSAGE_TOO_LARGE",
            Self::CryptoError(_) => "CRYPTO_ERROR",
            Self::Other(_) => "OTHER",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn transport_error_code_values_are_stable() {
        assert_eq!(
            Error::TransportNotAvailable("ble".into()).code(),
            "TRANSPORT_NOT_AVAILABLE"
        );
        assert_eq!(
            Error::PeerNotReachable("peer".into()).code(),
            "PEER_NOT_REACHABLE"
        );
        assert_eq!(Error::SendFailed("boom".into()).code(), "SEND_FAILED");
        assert_eq!(Error::ReceiveFailed("boom".into()).code(), "RECEIVE_FAILED");
        assert_eq!(
            Error::ConfigurationError("bad".into()).code(),
            "CONFIGURATION_ERROR"
        );
        assert_eq!(
            Error::SerializationError("bad".into()).code(),
            "SERIALIZATION_ERROR"
        );
        assert_eq!(
            Error::Core(offline_protocol_core::Error::Other("x".into())).code(),
            "CORE_ERROR"
        );
        assert_eq!(
            Error::MessageTooLarge(2048, 1024).code(),
            "MESSAGE_TOO_LARGE"
        );
        assert_eq!(Error::CryptoError("kdf".into()).code(), "CRYPTO_ERROR");
        assert_eq!(Error::Other("opaque".into()).code(), "OTHER");
    }

    #[test]
    fn transport_error_code_is_independent_of_display() {
        // The code must not be derived from the Display text.
        let err = Error::SendFailed("transient radio failure".into());
        assert_eq!(err.code(), "SEND_FAILED");
        assert!(err.to_string().contains("transient radio failure"));
    }
}
