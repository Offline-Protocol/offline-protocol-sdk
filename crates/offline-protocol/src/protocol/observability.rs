//! MLS lifecycle event emission for observability.

use super::OfflineProtocol;
#[cfg(feature = "mls-observability")]
use crate::mls_observability::{opaque_id, timestamp_now_ms, MlsLifecycleEvent};
use crate::mls_observability::{DecryptionFailureKind, MlsErrorCategory, MlsOperationContext};

impl OfflineProtocol {
    #[cfg(feature = "mls-observability")]
    pub(super) fn session_id_for_observability(
        &self,
        peer_id: Option<&str>,
        group_id: Option<&str>,
    ) -> String {
        let seed = format!(
            "peer={}|group={}",
            peer_id.unwrap_or("none"),
            group_id.unwrap_or("none")
        );
        opaque_id(&seed, &self.mls_observability_secret)
    }

    #[cfg(feature = "mls-observability")]
    pub(super) fn emit_mls_lifecycle_event(&self, event: MlsLifecycleEvent) {
        if self.mls_event_rate_limiter.should_emit(&event) {
            self.mls_event_emitter.emit(event);
        }
    }

    #[cfg(feature = "mls-observability")]
    pub(super) fn emit_mls_initialized(&self) {
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::Initialized {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(None, None),
            group_id: None,
            peer_id: None,
            context: MlsOperationContext::Initialize,
            error_category: None,
        });
    }

    #[cfg(not(feature = "mls-observability"))]
    pub(super) fn emit_mls_initialized(&self) {}

    #[cfg(feature = "mls-observability")]
    pub(super) fn emit_mls_encryption_used(&self, recipient: &str) {
        let peer_id = opaque_id(recipient, &self.mls_observability_secret);
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::EncryptionUsed {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(Some(recipient), None),
            group_id: None,
            peer_id: Some(peer_id),
            context: MlsOperationContext::Send,
            error_category: None,
        });
    }

    #[cfg(not(feature = "mls-observability"))]
    pub(super) fn emit_mls_encryption_used(&self, _recipient: &str) {}

    #[cfg(feature = "mls-observability")]
    pub(super) fn emit_mls_session_missing(
        &self,
        peer_id: Option<&str>,
        group_id: Option<&str>,
        context: MlsOperationContext,
        error_category: MlsErrorCategory,
    ) {
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::SessionMissing {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(peer_id, group_id),
            group_id: group_id.map(|id| opaque_id(id, &self.mls_observability_secret)),
            peer_id: peer_id.map(|id| opaque_id(id, &self.mls_observability_secret)),
            context,
            error_category: Some(error_category),
        });
    }

    #[cfg(not(feature = "mls-observability"))]
    pub(super) fn emit_mls_session_missing(
        &self,
        _peer_id: Option<&str>,
        _group_id: Option<&str>,
        _context: MlsOperationContext,
        _error_category: MlsErrorCategory,
    ) {
    }

    #[cfg(feature = "mls-observability")]
    pub(super) fn emit_mls_decryption_failed(
        &self,
        sender_id: &str,
        group_id: Option<&str>,
        kind: DecryptionFailureKind,
        context: MlsOperationContext,
    ) {
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::DecryptionFailed {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(Some(sender_id), group_id),
            group_id: group_id.map(|id| opaque_id(id, &self.mls_observability_secret)),
            peer_id: Some(opaque_id(sender_id, &self.mls_observability_secret)),
            context,
            error_category: Some(kind.error_category()),
            failure_kind: kind,
        });
    }

    #[cfg(not(feature = "mls-observability"))]
    pub(super) fn emit_mls_decryption_failed(
        &self,
        _sender_id: &str,
        _group_id: Option<&str>,
        _kind: DecryptionFailureKind,
        _context: MlsOperationContext,
    ) {
    }

    #[cfg(feature = "mls-observability")]
    pub(super) fn emit_mls_session_ready(
        &self,
        peer_id: &str,
        group_id: &str,
        context: MlsOperationContext,
    ) {
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::SessionReady {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(Some(peer_id), Some(group_id)),
            group_id: Some(opaque_id(group_id, &self.mls_observability_secret)),
            peer_id: Some(opaque_id(peer_id, &self.mls_observability_secret)),
            context,
            error_category: None,
        });
    }

    #[cfg(not(feature = "mls-observability"))]
    pub(super) fn emit_mls_session_ready(
        &self,
        _peer_id: &str,
        _group_id: &str,
        _context: MlsOperationContext,
    ) {
    }
}
