//! Protocol errors.

use thiserror::Error;

/// Result type alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Per-peer establishment state: where we are in getting to "can send" with a peer.
///
/// Used both as the value returned from `get_establishment_state` and as the payload
/// of `Error::SessionNotReady`, so callers can show "Establishing…" and retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EstablishmentState {
    /// No key package for this peer (memory nor storage).
    NoKeyPackage,
    /// Key package stored; no MLS session yet.
    HaveKeyPackage,
    /// Session created, welcome sent/queued; not confirmed.
    SessionPending,
    /// Can send/receive encrypted messages.
    SessionConfirmed,
}

impl EstablishmentState {
    /// Stable string for logging and FFI.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoKeyPackage => "NoKeyPackage",
            Self::HaveKeyPackage => "HaveKeyPackage",
            Self::SessionPending => "SessionPending",
            Self::SessionConfirmed => "SessionConfirmed",
        }
    }
}

/// Stable, machine-readable error classes used for session readiness decisions.
///
/// This enum is the single SDK boundary that maps heterogeneous upstream error
/// types into deterministic categories for protocol control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStateError {
    /// Session exists but cannot process application traffic yet.
    SessionNotReady,
    /// Session/group state does not exist locally.
    GroupNotFound,
    /// MLS stack is not initialized.
    NotInitialized,
    /// Underlying transport failure.
    TransportFailure,
    /// Cryptographic operation failure.
    CryptoFailure,
    /// Error that does not match a known class.
    Unknown,
}

impl SessionStateError {
    /// Returns a stable, non-localized machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::SessionNotReady => "SESSION_NOT_READY",
            Self::GroupNotFound => "GROUP_NOT_FOUND",
            Self::NotInitialized => "NOT_INITIALIZED",
            Self::TransportFailure => "TRANSPORT_FAILURE",
            Self::CryptoFailure => "CRYPTO_FAILURE",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// Classifies protocol-level errors for session state control flow.
    pub fn classify(error: &Error) -> Self {
        match error {
            Error::SessionNotReady(_) => Self::SessionNotReady,
            Error::MlsNotInitialized => Self::NotInitialized,
            Error::Transport(_) => Self::TransportFailure,
            Error::EncryptFailed(_) => Self::CryptoFailure,
            Error::Mls(inner) => Self::from(inner),
            _ => Self::Unknown,
        }
    }

    /// Maps session state class into welcome lifecycle reason taxonomy.
    pub fn to_welcome_reason_code(&self) -> crate::events::WelcomeReasonCode {
        match self {
            Self::TransportFailure => crate::events::WelcomeReasonCode::TransportUnavailable,
            Self::SessionNotReady
            | Self::GroupNotFound
            | Self::NotInitialized
            | Self::CryptoFailure
            | Self::Unknown => crate::events::WelcomeReasonCode::InternalError,
        }
    }
}

impl From<&offline_protocol_mls::MlsError> for SessionStateError {
    fn from(error: &offline_protocol_mls::MlsError) -> Self {
        match error {
            offline_protocol_mls::MlsError::GroupNotFound(_)
            | offline_protocol_mls::MlsError::SessionNotFound(_) => Self::GroupNotFound,
            offline_protocol_mls::MlsError::NotInitialized => Self::NotInitialized,
            offline_protocol_mls::MlsError::Storage(_)
            | offline_protocol_mls::MlsError::OpenMls(_)
            | offline_protocol_mls::MlsError::Deserialization(_)
            | offline_protocol_mls::MlsError::Serialization(_)
            | offline_protocol_mls::MlsError::InvalidMessage(_)
            | offline_protocol_mls::MlsError::Encryption(_)
            | offline_protocol_mls::MlsError::Decryption(_)
            | offline_protocol_mls::MlsError::CryptoGeneration(_)
            | offline_protocol_mls::MlsError::Signing(_)
            | offline_protocol_mls::MlsError::VerificationFailed(_)
            | offline_protocol_mls::MlsError::InvalidPublicKey(_) => Self::CryptoFailure,
            _ => Self::Unknown,
        }
    }
}

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

    /// MLS error.
    #[error("MLS error: {0}")]
    Mls(#[from] offline_protocol_mls::MlsError),

    /// MLS not initialized.
    #[error("MLS encryption not initialized")]
    MlsNotInitialized,

    /// No key package available for recipient.
    #[error("No key package available for recipient: {0}")]
    NoKeyPackage(String),

    /// Session not ready; establishment in progress. Caller can retry or show "Establishing…".
    #[error("Session not ready: {0:?}")]
    SessionNotReady(EstablishmentState),

    /// Outbound message encryption failed.
    #[error("Failed to encrypt message: {0}")]
    EncryptFailed(String),

    /// Service error.
    #[error("Service error: {0}")]
    Service(#[from] offline_protocol_services::ServiceError),

    /// Capability exchange error.
    #[error("Exchange error: {0}")]
    Exchange(#[from] offline_protocol_exchange::ExchangeError),

    /// Serialization error.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Operation rejected because the target user is blocked.
    #[error("User is blocked: {0}")]
    UserBlocked(String),

    /// Generic error.
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::{Error, SessionStateError};
    use offline_protocol_mls::MlsError;
    use offline_protocol_transport::Error as TransportError;

    #[test]
    fn session_state_code_values_are_stable() {
        assert_eq!(
            SessionStateError::SessionNotReady.code(),
            "SESSION_NOT_READY"
        );
        assert_eq!(SessionStateError::GroupNotFound.code(), "GROUP_NOT_FOUND");
        assert_eq!(SessionStateError::NotInitialized.code(), "NOT_INITIALIZED");
        assert_eq!(
            SessionStateError::TransportFailure.code(),
            "TRANSPORT_FAILURE"
        );
        assert_eq!(SessionStateError::CryptoFailure.code(), "CRYPTO_FAILURE");
        assert_eq!(SessionStateError::Unknown.code(), "UNKNOWN");
    }

    #[test]
    fn classify_maps_mls_group_not_found() {
        let classified = SessionStateError::from(&MlsError::GroupNotFound("g1".to_string()));
        assert_eq!(classified, SessionStateError::GroupNotFound);
    }

    #[test]
    fn classify_maps_not_initialized() {
        let classified = SessionStateError::from(&MlsError::NotInitialized);
        assert_eq!(classified, SessionStateError::NotInitialized);
    }

    #[test]
    fn classify_maps_crypto_failures() {
        let classified = SessionStateError::from(&MlsError::Decryption("failed".to_string()));
        assert_eq!(classified, SessionStateError::CryptoFailure);
    }

    #[test]
    fn classify_unknown_fallback_is_safe() {
        let classified = SessionStateError::classify(&Error::Other("opaque".to_string()));
        assert_eq!(classified, SessionStateError::Unknown);
    }

    #[test]
    fn welcome_reason_mapping_uses_typed_transport_failure() {
        let classified = SessionStateError::classify(&Error::Transport(
            TransportError::SendFailed("send failed".to_string()),
        ));
        assert_eq!(
            classified.to_welcome_reason_code(),
            crate::events::WelcomeReasonCode::TransportUnavailable
        );
    }

    #[test]
    fn welcome_reason_mapping_unknown_defaults_internal_error() {
        let classified = SessionStateError::classify(&Error::Other("opaque".to_string()));
        assert_eq!(
            classified.to_welcome_reason_code(),
            crate::events::WelcomeReasonCode::InternalError
        );
    }
}
