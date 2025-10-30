//! Reliability layer errors.

use thiserror::Error;

/// Result type alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Reliability layer errors.
#[derive(Debug, Error)]
pub enum Error {
    /// ACK timeout.
    #[error("ACK timeout for message {0}")]
    AckTimeout(String),

    /// Maximum retries exceeded.
    #[error("Maximum retries exceeded")]
    MaxRetriesExceeded,

    /// Core error.
    #[error("Core error: {0}")]
    Core(#[from] offline_protocol_core::Error),

    /// Generic error.
    #[error("{0}")]
    Other(String),
}
