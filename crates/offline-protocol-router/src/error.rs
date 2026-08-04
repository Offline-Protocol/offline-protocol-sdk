//! Router layer errors.

use thiserror::Error;

/// Result type alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Router errors.
// Adding a variant to a public error enum is a breaking change without
// this attribute; downstream crates must carry a wildcard arm.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum Error {
    /// No suitable transport available.
    #[error("No transport available")]
    NoTransportAvailable,

    /// No suitable relay found.
    #[error("No relay available")]
    NoRelayAvailable,

    /// Transport error.
    #[error("Transport error: {0}")]
    Transport(#[from] offline_protocol_transport::Error),

    /// Core error.
    #[error("Core error: {0}")]
    Core(#[from] offline_protocol_core::Error),

    /// Generic error.
    #[error("{0}")]
    Other(String),
}
