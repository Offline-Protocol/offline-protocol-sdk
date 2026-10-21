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

    /// The pending-ACK table is full and nothing in it may be evicted for
    /// this message.
    ///
    /// Typed rather than a string so a caller can tell "this frame cannot be
    /// tracked right now" (defer it) from a genuine fault. A caller that
    /// treats it as fatal after the frame has been transmitted reports a
    /// failure for a message that is already on the wire.
    #[error("Pending ACK table is full ({max} entries)")]
    AckCapacity {
        /// The configured ceiling.
        max: usize,
    },

    /// Core error.
    #[error("Core error: {0}")]
    Core(#[from] offline_protocol_core::Error),

    /// Generic error.
    #[error("{0}")]
    Other(String),
}
