/// Errors from mesh service operations.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// Serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// A payload field exceeds its maximum allowed size.
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),

    /// An invalid service response status was provided.
    #[error("invalid status: {0} (expected one of: ok, not_found, error)")]
    InvalidStatus(String),

    /// Other service error.
    #[error("{0}")]
    Other(String),
}
