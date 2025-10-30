//! Protocol errors.

use thiserror::Error;

/// Result type alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Protocol errors.
#[derive(Debug, Error)]
pub enum Error {
    /// Protocol not started.
    #[error("Protocol not started")]
    NotStarted,

    /// Protocol already started.
    #[error("Protocol already started")]
    AlreadyStarted,

    /// Invalid configuration.
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),

    /// Core error.
    #[error("Core error: {0}")]
    Core(#[from] offline_protocol_core::Error),

    /// Transport error.
    #[error("Transport error: {0}")]
    Transport(#[from] offline_protocol_transport::Error),

    /// Router error.
    #[error("Router error: {0}")]
    Router(#[from] offline_protocol_router::Error),

    /// Reliability error.
    #[error("Reliability error: {0}")]
    Reliability(#[from] offline_protocol_reliability::Error),

    /// Generic error.
    #[error("{0}")]
    Other(String),
}
