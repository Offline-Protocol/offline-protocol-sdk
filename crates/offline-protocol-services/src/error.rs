/// Errors from mesh service operations.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// Serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Other service error.
    #[error("{0}")]
    Other(String),
}
