//! MLS lifecycle observability primitives.

use crate::telemetry::CategorySampler;
use offline_protocol_mls::MlsError;
use serde::Serialize;

/// Operation context for MLS lifecycle events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MlsOperationContext {
    /// Outbound send path.
    Send,
    /// Inbound receive path.
    Receive,
    /// MLS manager initialization path.
    Initialize,
    /// Session lookup or resolution path.
    SessionLookup,
    /// Welcome processing path.
    Welcome,
}

/// Typed error category for MLS lifecycle events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MlsErrorCategory {
    /// Session or group state is missing.
    SessionStateMissing,
    /// MLS manager is not initialized.
    NotInitialized,
    /// Ciphertext/message shape is invalid.
    InvalidCiphertext,
    /// Sender identity or signature verification failed.
    IdentityMismatch,
    /// Crypto engine operation failed.
    CryptoFailure,
    /// Unknown or uncategorized failure.
    Unknown,
}

/// Fine-grained decryption failure classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecryptionFailureKind {
    /// Session or group state was not found.
    SessionNotFound,
    /// MLS manager is not initialized.
    NotInitialized,
    /// Message bytes cannot be decrypted/parsed as valid MLS ciphertext.
    InvalidCiphertext,
    /// Signature/identity verification failed.
    IdentityMismatch,
    /// Other cryptographic failure.
    CryptoFailure,
    /// Fallback class for unsupported variants.
    Unknown,
}

impl DecryptionFailureKind {
    /// Classifies an MLS error into a decryption failure kind.
    pub fn from_mls_error(error: &MlsError) -> Self {
        match error {
            MlsError::SessionNotFound(_) | MlsError::GroupNotFound(_) => Self::SessionNotFound,
            MlsError::NotInitialized => Self::NotInitialized,
            MlsError::InvalidMessage(_)
            | MlsError::Deserialization(_)
            | MlsError::Decryption(_) => Self::InvalidCiphertext,
            MlsError::VerificationFailed(_) | MlsError::InvalidPublicKey(_) => {
                Self::IdentityMismatch
            }
            MlsError::Encryption(_)
            | MlsError::OpenMls(_)
            | MlsError::Signing(_)
            | MlsError::Storage(_)
            | MlsError::CryptoGeneration(_) => Self::CryptoFailure,
            _ => Self::Unknown,
        }
    }

    /// Maps a decryption failure kind to its broader telemetry category.
    pub fn error_category(self) -> MlsErrorCategory {
        match self {
            Self::SessionNotFound => MlsErrorCategory::SessionStateMissing,
            Self::NotInitialized => MlsErrorCategory::NotInitialized,
            Self::InvalidCiphertext => MlsErrorCategory::InvalidCiphertext,
            Self::IdentityMismatch => MlsErrorCategory::IdentityMismatch,
            Self::CryptoFailure => MlsErrorCategory::CryptoFailure,
            Self::Unknown => MlsErrorCategory::Unknown,
        }
    }
}

/// Typed MLS lifecycle events for operational observability.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MlsLifecycleEvent {
    /// MLS subsystem has been initialized and is ready for use.
    Initialized {
        timestamp_ms: i64,
        session_id: String,
        group_id: Option<String>,
        peer_id: Option<String>,
        context: MlsOperationContext,
        error_category: Option<MlsErrorCategory>,
    },
    /// Encryption path was successfully used.
    EncryptionUsed {
        timestamp_ms: i64,
        session_id: String,
        group_id: Option<String>,
        peer_id: Option<String>,
        context: MlsOperationContext,
        error_category: Option<MlsErrorCategory>,
    },
    /// Decryption failed with a typed failure class.
    DecryptionFailed {
        timestamp_ms: i64,
        session_id: String,
        group_id: Option<String>,
        peer_id: Option<String>,
        context: MlsOperationContext,
        error_category: Option<MlsErrorCategory>,
        failure_kind: DecryptionFailureKind,
    },
    /// Session lookup failed or session/group state is missing.
    SessionMissing {
        timestamp_ms: i64,
        session_id: String,
        group_id: Option<String>,
        peer_id: Option<String>,
        context: MlsOperationContext,
        error_category: Option<MlsErrorCategory>,
    },
    /// Session lifecycle has reached a ready/usable state.
    SessionReady {
        timestamp_ms: i64,
        session_id: String,
        group_id: Option<String>,
        peer_id: Option<String>,
        context: MlsOperationContext,
        error_category: Option<MlsErrorCategory>,
    },
}

impl MlsLifecycleEvent {
    /// Returns the stable `snake.dot.case` telemetry name for this event.
    ///
    /// Names are compatible with OpenTelemetry naming conventions and are
    /// considered stable across minor versions of the SDK.
    pub fn telemetry_name(&self) -> &'static str {
        match self {
            Self::Initialized { .. } => "mls.initialized",
            Self::EncryptionUsed { .. } => "mls.encryption_used",
            Self::DecryptionFailed { .. } => "mls.decryption_failed",
            Self::SessionMissing { .. } => "mls.session_missing",
            Self::SessionReady { .. } => "mls.session_ready",
        }
    }
}

/// Sink abstraction for MLS lifecycle events.
pub trait MlsEventEmitter: Send + Sync {
    /// Emits a structured MLS lifecycle event.
    fn emit(&self, event: MlsLifecycleEvent);
}

/// Default no-op emitter used when observability is not configured.
#[derive(Debug, Default)]
pub struct NoopMlsEventEmitter;

impl MlsEventEmitter for NoopMlsEventEmitter {
    fn emit(&self, _event: MlsLifecycleEvent) {}
}

/// Best-effort fixed-window limiter to reduce high-volume failure event floods.
///
/// Thin wrapper around [`CategorySampler`] that maps MLS lifecycle events to
/// rate-limit keys.
#[derive(Debug, Default)]
pub struct MlsEventRateLimiter {
    sampler: CategorySampler,
}

impl MlsEventRateLimiter {
    /// Returns true if the event passes rate limiting.
    pub fn should_emit(&self, event: &MlsLifecycleEvent) -> bool {
        let now_ms = timestamp_now_ms();
        match event {
            MlsLifecycleEvent::DecryptionFailed {
                peer_id,
                failure_kind,
                ..
            } => {
                let key = format!(
                    "decryption_failed:{}:{:?}",
                    peer_id.as_deref().unwrap_or("none"),
                    failure_kind
                );
                self.sampler.allow(&key, now_ms)
            }
            MlsLifecycleEvent::SessionMissing { peer_id, .. } => {
                let key = format!("session_missing:{}", peer_id.as_deref().unwrap_or("none"));
                self.sampler.allow(&key, now_ms)
            }
            _ => true,
        }
    }
}

/// Returns current wall-clock time in Unix milliseconds.
pub fn timestamp_now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decryption_failure_classification_is_typed() {
        assert_eq!(
            DecryptionFailureKind::from_mls_error(&MlsError::SessionNotFound("p".to_string())),
            DecryptionFailureKind::SessionNotFound
        );
        assert_eq!(
            DecryptionFailureKind::from_mls_error(&MlsError::NotInitialized),
            DecryptionFailureKind::NotInitialized
        );
        assert_eq!(
            DecryptionFailureKind::from_mls_error(&MlsError::InvalidMessage("bad".to_string())),
            DecryptionFailureKind::InvalidCiphertext
        );
        assert_eq!(
            DecryptionFailureKind::from_mls_error(&MlsError::VerificationFailed("sig".to_string())),
            DecryptionFailureKind::IdentityMismatch
        );
    }

    #[test]
    fn limiter_suppresses_flood() {
        let limiter = MlsEventRateLimiter::default();
        let base_ts = 1000_i64;
        // First 10 DecryptionFailed events for the same peer+kind pass.
        for i in 0..10 {
            let event = MlsLifecycleEvent::DecryptionFailed {
                timestamp_ms: base_ts + i,
                session_id: "s".to_string(),
                group_id: None,
                peer_id: Some("peer-a".to_string()),
                context: MlsOperationContext::Receive,
                error_category: None,
                failure_kind: DecryptionFailureKind::InvalidCiphertext,
            };
            assert!(limiter.should_emit(&event));
        }
        // 11th is suppressed.
        let event = MlsLifecycleEvent::DecryptionFailed {
            timestamp_ms: base_ts + 10,
            session_id: "s".to_string(),
            group_id: None,
            peer_id: Some("peer-a".to_string()),
            context: MlsOperationContext::Receive,
            error_category: None,
            failure_kind: DecryptionFailureKind::InvalidCiphertext,
        };
        assert!(!limiter.should_emit(&event));
    }
}
