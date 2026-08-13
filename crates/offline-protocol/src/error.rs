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
// Adding a variant to a public error enum is a breaking change without
// this attribute; downstream crates must carry a wildcard arm.
#[non_exhaustive]
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
    /// Session exists but is out of sync with the sender's epoch (the two sides
    /// disagree on the MLS epoch).
    ///
    /// What separates this from [`Self::CryptoFailure`] is the **re-key**, not
    /// the ACK: under `crypto_recovery_enabled` both classes withhold the
    /// delivery ACK so the sender's resend can still deliver, but only this one
    /// additionally re-establishes the 1:1 session. The split is deliberate —
    /// re-keying on an AEAD/authentication failure, which any injected frame can
    /// produce, would be a re-key-storm vector.
    SessionDesync,
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
            Self::SessionDesync => "SESSION_DESYNC",
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
            Error::GroupNotFound(_) => Self::GroupNotFound,
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
            | Self::SessionDesync
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
            offline_protocol_mls::MlsError::SessionDesync(_) => Self::SessionDesync,
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
            // Not a session-state condition: commit enforcement is a group
            // policy, and 1:1 sessions are exempt from it. It can only reach
            // this classifier when a *group* commit ciphertext arrives wrapped
            // in an `__MLS_ENC__` envelope, where `Unknown` is the disposition
            // we want — drop and ACK, never queue (the refusal is permanent,
            // so the same frame can never become decryptable) and never re-key
            // (the session is healthy).
            offline_protocol_mls::MlsError::CommitNotAuthorized { .. } => Self::Unknown,
            // Not a session-state condition either: the envelope named a slot
            // that is not the claimed sender's, which is a security rejection,
            // not a readiness problem. `Unknown` is the disposition we want on
            // any path that reaches this classifier — never queue (the frame
            // can never become legitimate) and never re-key (there is no
            // evidence our session with the claimed sender is unhealthy). The
            // 1:1 text path intercepts this variant before classification so it
            // can withhold the ACK; see `handle_encrypted_message`.
            offline_protocol_mls::MlsError::SessionIdentityMismatch { .. } => Self::Unknown,
            // Not a session-state condition either, and `Unknown` is the
            // disposition both need on any path that reaches this classifier
            // (a group commit or message wrapped in an `__MLS_ENC__`
            // envelope): never queue — a leaf whose key does not hash to its
            // own credential can never start doing so, and an unsupported
            // sender role never becomes supported — and never re-key, since
            // neither says anything about the health of our session with the
            // claimed sender. The group path intercepts both before
            // classification so it can withhold the ACK; see
            // `decrypt_group_application`.
            offline_protocol_mls::MlsError::LeafAddressMismatch { .. }
            | offline_protocol_mls::MlsError::UnsupportedSender { .. } => Self::Unknown,
            _ => Self::Unknown,
        }
    }
}

/// Protocol errors.
// Adding a variant to a public error enum is a breaking change without
// this attribute; downstream crates must carry a wildcard arm.
#[non_exhaustive]
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

    /// Serialization error.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Operation rejected because the target user is blocked.
    #[error("User is blocked: {0}")]
    UserBlocked(String),

    /// Too many concurrent media transfers to the same recipient. Encrypted
    /// transfers share the recipient's session ratchet, whose out-of-order
    /// tolerance bounds how many chunks may be in flight at once.
    #[error(
        "Too many concurrent media transfers to {0}; retry after an active transfer completes"
    )]
    MediaTransferLimit(String),

    /// Group does not exist locally.
    #[error("Group not found: {0}")]
    GroupNotFound(String),

    /// Operation requires a group role or permission the caller does not hold.
    #[error("{0}")]
    PermissionDenied(String),

    /// Operation rejected by the current group or protocol state (e.g.
    /// last-admin constraints, member limits, expired key packages).
    #[error("{0}")]
    InvalidState(String),

    /// A caller-supplied argument failed validation.
    #[error("{0}")]
    InvalidArgument(String),

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
        assert_eq!(SessionStateError::SessionDesync.code(), "SESSION_DESYNC");
        assert_eq!(SessionStateError::Unknown.code(), "UNKNOWN");
    }

    #[test]
    fn classify_maps_mls_group_not_found() {
        let classified = SessionStateError::from(&MlsError::GroupNotFound("g1".to_string()));
        assert_eq!(classified, SessionStateError::GroupNotFound);
    }

    #[test]
    fn classify_maps_protocol_group_not_found() {
        // The protocol-level variant must land in the same class as the
        // MLS-layer one — one condition, one category.
        let classified = SessionStateError::classify(&Error::GroupNotFound("g1".to_string()));
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
    fn classify_maps_session_desync_distinct_from_crypto_failure() {
        // An epoch desync is recoverable and must land in its own class, NOT
        // CryptoFailure — otherwise the receive path would drop-and-ACK it as a
        // permanent failure instead of re-keying.
        let desync = SessionStateError::from(&MlsError::SessionDesync("wrong epoch".to_string()));
        assert_eq!(desync, SessionStateError::SessionDesync);
        // The genuine-failure sibling stays CryptoFailure.
        let crypto = SessionStateError::from(&MlsError::Decryption("aead".to_string()));
        assert_eq!(crypto, SessionStateError::CryptoFailure);
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
