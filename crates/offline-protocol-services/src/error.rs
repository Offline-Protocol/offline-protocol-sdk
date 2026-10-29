/// Errors from mesh service operations.
// Adding a variant to a public error enum is a breaking change without
// this attribute; downstream crates must carry a wildcard arm.
#[non_exhaustive]
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

    /// An argument is outside what the call accepts.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// No subscription with this id exists on this side.
    #[error("unknown subscription: {0}")]
    UnknownSubscription(String),

    /// The peer does not support what was asked of it.
    #[error("unsupported by peer: {0}")]
    Unsupported(String),

    /// Other service error.
    #[error("{0}")]
    Other(String),
}
