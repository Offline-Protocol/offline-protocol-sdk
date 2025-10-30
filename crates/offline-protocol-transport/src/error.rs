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

    /// Failed to send message.
    #[error("Send failed: {0}")]
    SendFailed(String),

    /// Failed to receive message.
    #[error("Receive failed: {0}")]
    ReceiveFailed(String),

    /// Transport configuration error.
    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    /// Core error propagated from offline-protocol-core.
    #[error("Core error: {0}")]
    Core(#[from] offline_protocol_core::Error),

    /// Generic error.
    #[error("{0}")]
    Other(String),
}
