//! MLS lifecycle event emission for observability.
//!
//! Two fan-outs share this module:
//!
//! 1. The legacy [`MlsEventEmitter`] registered via
//!    [`OfflineProtocol::set_mls_event_emitter`] — preserved for backward
//!    compatibility with apps that wired observability before the unified
//!    telemetry sink existed.
//! 2. The unified [`crate::telemetry::TelemetrySink`] registered via
//!    [`OfflineProtocol::install_telemetry_sink`] — receives every lifecycle
//!    event as a [`TelemetryRecord::Mls`].
//!
//! Both fan-outs share one gate: [`crate::telemetry::MlsVerbosity`] from the
//! installed [`crate::telemetry::TelemetryConfig`]. Setting verbosity to
//! `Off` suppresses both. When no telemetry sink has been installed,
//! verbosity defaults to `Lifecycle` — the same always-on behavior the old
//! `mls-observability` Cargo feature used to gate.
//!
//! [`MlsEventEmitter`]: crate::mls_observability::MlsEventEmitter
//! [`OfflineProtocol::set_mls_event_emitter`]: super::OfflineProtocol::set_mls_event_emitter
//! [`OfflineProtocol::install_telemetry_sink`]: super::OfflineProtocol::install_telemetry_sink

use super::OfflineProtocol;
use crate::mls_observability::{
    timestamp_now_ms, DecryptionFailureKind, MlsErrorCategory, MlsLifecycleEvent,
    MlsOperationContext,
};
use crate::telemetry::{dispatch_record, MlsVerbosity, TelemetryRecord};

impl OfflineProtocol {
    /// Returns the current scrubber used by MLS emit sites.
    ///
    /// When a telemetry sink has been installed, this is the scrubber derived
    /// from the installed [`crate::telemetry::TelemetryConfig`]. Otherwise it
    /// is the pre-install scrubber constructed at
    /// [`OfflineProtocol::new`], which shares its fallback secret with any
    /// later-installed scrubber — so opaque identifiers stay consistent for
    /// legacy-emitter consumers across the install boundary.
    fn current_scrubber(&self) -> &crate::telemetry::Scrubber {
        self.telemetry
            .as_ref()
            .map(|ctx| &ctx.scrubber)
            .unwrap_or(&self.telemetry_scrubber)
    }

    /// Returns the effective MLS lifecycle verbosity tier.
    ///
    /// Defaults to [`MlsVerbosity::Lifecycle`] when no telemetry sink has
    /// been installed, matching the always-on legacy-emitter behavior that
    /// the retired `mls-observability` Cargo feature used to gate.
    pub(super) fn mls_verbosity(&self) -> MlsVerbosity {
        self.telemetry
            .as_ref()
            .map(|ctx| ctx.config.mls_verbosity())
            .unwrap_or(MlsVerbosity::Lifecycle)
    }

    /// Returns whether the installed sink opted out of MLS event rate
    /// limiting.
    ///
    /// Defaults to `false` (rate limiting active) when no telemetry sink has
    /// been installed, preserving the always-limited behavior the legacy
    /// emitter path has always had.
    fn mls_sampling_bypass(&self) -> bool {
        self.telemetry
            .as_ref()
            .map(|ctx| ctx.config.mls_sampling_bypass())
            .unwrap_or(false)
    }

    /// Derives the opaque session identifier used to correlate MLS lifecycle
    /// events. The raw seed (`peer=<peer_id>|group=<group_id>`) couples two
    /// identifiers, so it is hashed unconditionally via
    /// [`crate::telemetry::Scrubber::hash_always`] — disabling `scrub_ids`
    /// does not disable obfuscation of derived correlation tokens.
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
        self.current_scrubber().hash_always(&seed)
    }

    /// Single choke point for MLS lifecycle emission. Both the legacy
    /// [`crate::mls_observability::MlsEventEmitter`] and the installed
    /// [`crate::telemetry::TelemetrySink`] are reached from here.
    pub(super) fn emit_mls_lifecycle_event(&self, event: MlsLifecycleEvent) {
        // Runtime replacement for the retired `mls-observability` Cargo
        // feature. Verbosity defaults to `Lifecycle` when no sink is
        // installed, matching today's always-on-when-feature-was-on
        // behavior for the legacy emitter path.
        if matches!(self.mls_verbosity(), MlsVerbosity::Off) {
            return;
        }
        // Telemetry-grade sinks can opt out of the fixed-window limiter so
        // aggregate counts are not clipped to the per-window ceiling. When
        // bypass is on we skip `should_emit` entirely — its window counter is
        // left untouched, so toggling bypass off later resumes clean limiting.
        if !self.mls_sampling_bypass() && !self.mls_event_rate_limiter.should_emit(&event) {
            return;
        }
        // Legacy emitter consumes a value-typed event (its existing
        // signature), so we clone when a sink is also installed. The clone
        // is cheap — `MlsLifecycleEvent` is small and lives on the stack.
        if let Some(ctx) = &self.telemetry {
            self.mls_event_emitter.emit(event.clone());
            dispatch_record(&ctx.sink, &TelemetryRecord::Mls(event));
        } else {
            self.mls_event_emitter.emit(event);
        }
    }

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

    pub(super) fn emit_mls_encryption_used(&self, recipient: &str) {
        let scrubber = self.current_scrubber();
        let peer_id = scrubber.hash_id(recipient).into_owned();
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::EncryptionUsed {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(Some(recipient), None),
            group_id: None,
            peer_id: Some(peer_id),
            context: MlsOperationContext::Send,
            error_category: None,
        });
    }

    pub(super) fn emit_mls_session_missing(
        &self,
        peer_id: Option<&str>,
        group_id: Option<&str>,
        context: MlsOperationContext,
        error_category: MlsErrorCategory,
    ) {
        let scrubber = self.current_scrubber();
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::SessionMissing {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(peer_id, group_id),
            group_id: group_id.map(|id| scrubber.hash_id(id).into_owned()),
            peer_id: peer_id.map(|id| scrubber.hash_id(id).into_owned()),
            context,
            error_category: Some(error_category),
        });
    }

    pub(super) fn emit_mls_decryption_failed(
        &self,
        sender_id: &str,
        group_id: Option<&str>,
        kind: DecryptionFailureKind,
        context: MlsOperationContext,
    ) {
        let scrubber = self.current_scrubber();
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::DecryptionFailed {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(Some(sender_id), group_id),
            group_id: group_id.map(|id| scrubber.hash_id(id).into_owned()),
            peer_id: Some(scrubber.hash_id(sender_id).into_owned()),
            context,
            error_category: Some(kind.error_category()),
            failure_kind: kind,
        });
    }

    pub(super) fn emit_mls_session_ready(
        &self,
        peer_id: &str,
        group_id: &str,
        context: MlsOperationContext,
    ) {
        let scrubber = self.current_scrubber();
        self.emit_mls_lifecycle_event(MlsLifecycleEvent::SessionReady {
            timestamp_ms: timestamp_now_ms(),
            session_id: self.session_id_for_observability(Some(peer_id), Some(group_id)),
            group_id: Some(scrubber.hash_id(group_id).into_owned()),
            peer_id: Some(scrubber.hash_id(peer_id).into_owned()),
            context,
            error_category: None,
        });
    }
}
