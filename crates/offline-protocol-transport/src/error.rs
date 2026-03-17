//! Transport layer errors.

use thiserror::Error;

/// Result type alias using the transport Error type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in the transport layer.
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

    /// Generic error.
    #[error("{0}")]
    Other(String),
}
