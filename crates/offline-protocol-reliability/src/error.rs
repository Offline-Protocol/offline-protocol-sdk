//! Reliability layer errors.

use thiserror::Error;

/// Result type alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Reliability layer errors.
// Adding a variant to a public error enum is a breaking change without
// this attribute; downstream crates must carry a wildcard arm.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum Error {
    /// ACK timeout.
    #[error("ACK timeout for message {0}")]
    AckTimeout(String),

    /// Core error.
    #[error("Core error: {0}")]
    Core(#[from] offline_protocol_core::Error),

    /// Generic error.
    #[error("{0}")]
    Other(String),
}
