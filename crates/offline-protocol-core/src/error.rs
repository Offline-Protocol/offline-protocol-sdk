//! Error types for the Offline Protocol SDK.

use thiserror::Error;

/// Result type alias using the core Error type.
pub type Result<T> = std::result::Result<T, Error>;

/// Core errors that can occur in the Offline Protocol SDK.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    /// Invalid message format or content.
    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    /// Invalid user ID format.
    #[error("Invalid user ID: {0}")]
    InvalidUserId(String),

    /// Invalid app ID format.
    #[error("Invalid app ID: {0}")]
    InvalidAppId(String),

    /// Invalid TTL value (must be > 0).
    #[error("Invalid TTL: {0}")]
    InvalidTTL(u8),

    /// Invalid hop count value.
    #[error("Invalid hop count: {0}")]
    InvalidHopCount(u8),

    /// Message serialization failed.
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Message deserialization failed.
    #[error("Deserialization error: {0}")]
    DeserializationError(String),

    /// Generic error with custom message.
    #[error("{0}")]
    Other(String),
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::SerializationError(err.to_string())
    }
}
