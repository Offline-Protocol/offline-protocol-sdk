//! MLS lifecycle observability primitives.

use offline_protocol_mls::MlsError;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;

const DEFAULT_RATE_LIMIT_WINDOW_MS: i64 = 1_000;
const DEFAULT_RATE_LIMIT_MAX_EVENTS: u32 = 10;
const MAX_RATE_LIMIT_KEYS: usize = 4096;

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
            MlsError::InvalidMessage(_) | MlsError::Deserialization(_) | MlsError::Decryption(_) => {
                Self::InvalidCiphertext
            }
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

#[derive(Debug, Clone)]
struct EventWindow {
    window_start_ms: i64,
    count: u32,
}

/// Best-effort fixed-window limiter to reduce high-volume failure event floods.
#[derive(Debug)]
pub struct MlsEventRateLimiter {
    max_events_per_window: u32,
    window_ms: i64,
    windows: Mutex<HashMap<String, EventWindow>>,
}

impl Default for MlsEventRateLimiter {
    fn default() -> Self {
        Self {
            max_events_per_window: DEFAULT_RATE_LIMIT_MAX_EVENTS,
            window_ms: DEFAULT_RATE_LIMIT_WINDOW_MS,
            windows: Mutex::new(HashMap::new()),
        }
    }
}

impl MlsEventRateLimiter {
    /// Returns true when an event should be emitted for the current window.
    pub fn allow(&self, key: &str, now_ms: i64) -> bool {
        let Ok(mut windows) = self.windows.lock() else {
            return true;
        };

        if windows.len() >= MAX_RATE_LIMIT_KEYS && !windows.contains_key(key) {
            windows.retain(|_, window| {
                now_ms.saturating_sub(window.window_start_ms) <= DEFAULT_RATE_LIMIT_WINDOW_MS
            });
            if windows.len() >= MAX_RATE_LIMIT_KEYS {
                windows.clear();
            }
        }

        let window = windows.entry(key.to_string()).or_insert(EventWindow {
            window_start_ms: now_ms,
            count: 0,
        });

        if now_ms.saturating_sub(window.window_start_ms) >= self.window_ms {
            window.window_start_ms = now_ms;
            window.count = 0;
        }

        if window.count >= self.max_events_per_window {
            return false;
        }

        window.count = window.count.saturating_add(1);
        true
    }

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
                self.allow(&key, now_ms)
            }
            MlsLifecycleEvent::SessionMissing { peer_id, .. } => {
                let key = format!("session_missing:{}", peer_id.as_deref().unwrap_or("none"));
                self.allow(&key, now_ms)
            }
            _ => true,
        }
    }
}

/// Returns current wall-clock time in Unix milliseconds.
pub fn timestamp_now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Generates a stable opaque identifier for telemetry.
pub fn opaque_id(raw: &str, secret: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret);
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(32);
    for byte in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
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
    fn opaque_id_is_stable_for_same_input() {
        let secret = b"secret";
        let first = opaque_id("peer:alice", secret);
        let second = opaque_id("peer:alice", secret);
        assert_eq!(first, second);
    }

    #[test]
    fn opaque_id_changes_for_different_secret() {
        let first = opaque_id("peer:alice", b"secret-a");
        let second = opaque_id("peer:alice", b"secret-b");
        assert_ne!(first, second);
    }

    #[test]
    fn limiter_blocks_after_budget() {
        let limiter = MlsEventRateLimiter::default();
        let now = 1000_i64;
        let key = "k";
        for _ in 0..DEFAULT_RATE_LIMIT_MAX_EVENTS {
            assert!(limiter.allow(key, now));
        }
        assert!(!limiter.allow(key, now));
        assert!(limiter.allow(key, now + DEFAULT_RATE_LIMIT_WINDOW_MS));
    }
}
