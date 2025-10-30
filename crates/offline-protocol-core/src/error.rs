//! Error types for the offline protocol

use thiserror::Error;

/// Result type for offline protocol operations
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in the offline protocol
#[derive(Error, Debug, Clone)]
pub enum Error {
    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error: {0}")]
    Deserialization(String),

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("Message too large: {0} bytes (max: {1})")]
    MessageTooLarge(usize, usize),

    #[error("Invalid device ID: {0}")]
    InvalidDeviceId(String),

    #[error("Invalid user ID: {0}")]
    InvalidUserId(String),

    #[error("TTL exceeded")]
    TtlExceeded,

    #[error("Unknown message type: {0}")]
    UnknownMessageType(u8),
}

impl From<rmp_serde::encode::Error> for Error {
    fn from(err: rmp_serde::encode::Error) -> Self {
        Error::Serialization(err.to_string())
    }
}

impl From<rmp_serde::decode::Error> for Error {
    fn from(err: rmp_serde::decode::Error) -> Self {
        Error::Deserialization(err.to_string())
    }
}

