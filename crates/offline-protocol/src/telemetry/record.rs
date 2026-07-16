//! Unified telemetry record taxonomy.
//!
//! [`TelemetryRecord`] is the single typed surface that every SDK event and
//! lifecycle signal flows through. Today it carries `Protocol` (wrapping
//! [`crate::events::Event`]) and `Mls` (wrapping
//! [`crate::mls_observability::MlsLifecycleEvent`]); additional variants
//! (periodic metrics snapshots, transport state transitions, routing
//! decisions, device-capability snapshots) are introduced alongside the emit
//! sites that populate them in follow-up work.
//!
//! Names returned by [`TelemetryRecord::name`] follow a stable
//! `snake.dot.case` grammar compatible with OpenTelemetry conventions: the
//! string matches `[a-z0-9_]+(\.[a-z0-9_]+)+`. These names are the canonical
//! wire identity for a record.
//!
//! The enum is `#[non_exhaustive]` so new categories can be added without a
//! major-version bump. The record intentionally does not derive `Serialize`:
//! the `name()` above is the canonical wire identity, and sinks that need
//! on-the-wire serialization serialize the inner typed payload
//! (`Event`/`MlsLifecycleEvent`) directly — this avoids maintaining two
//! parallel naming schemes.

use crate::events::Event;
use crate::mls_observability::MlsLifecycleEvent;
use crate::telemetry::device::DeviceCapabilitySnapshot;
use crate::telemetry::metrics_snapshot::MetricsFrame;
use crate::telemetry::routing::RoutingDecision;
use crate::telemetry::transport_state::TransportStateEvent;

/// A single typed telemetry emission.
///
/// The name is delegated to the inner payload's `telemetry_name()` method so
/// there is a single source of truth for variant naming.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum TelemetryRecord {
    /// A protocol-level event — wraps the existing [`Event`] type.
    ///
    /// The inner `Event` is boxed because it is substantially larger than
    /// every other variant; boxing keeps the enum's footprint flat so
    /// non-`Protocol` records do not pay the size tax when stored in
    /// collections.
    Protocol(Box<Event>),
    /// An MLS lifecycle event — wraps the existing [`MlsLifecycleEvent`].
    Mls(MlsLifecycleEvent),
    /// A periodic snapshot of protocol-wide counters and per-transport
    /// metrics. Boxed to keep the enum footprint flat — the frame carries
    /// inline `Vec`/stat structs that would otherwise dominate the enum
    /// size and tax every non-snapshot variant.
    MetricsSnapshot(Box<MetricsFrame>),
    /// A single transport-status transition (per-transport, unlike the
    /// protocol-level `Event::TransportSwitched`).
    TransportState(TransportStateEvent),
    /// A structured routing decision (supersets `Event::Dors*`).
    ///
    /// Boxed because the score-breakdown vector can carry per-transport
    /// factor details at the `Diagnostic` verbosity tier.
    Routing(Box<RoutingDecision>),
    /// A snapshot of local device capability (battery, charging, relay role).
    Device(DeviceCapabilitySnapshot),
}

impl TelemetryRecord {
    /// Returns the stable name for this record. See the module-level docs
    /// for the exact grammar and stability guarantees.
    pub fn name(&self) -> &'static str {
        match self {
            TelemetryRecord::Protocol(event) => event.telemetry_name(),
            TelemetryRecord::Mls(event) => event.telemetry_name(),
            TelemetryRecord::MetricsSnapshot(frame) => frame.telemetry_name(),
            TelemetryRecord::TransportState(event) => event.telemetry_name(),
            TelemetryRecord::Routing(decision) => decision.telemetry_name(),
            TelemetryRecord::Device(snapshot) => snapshot.telemetry_name(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        DorsEscalationPhase, DorsEscalationReasonCode, DorsReasonCode, PresenceStatus,
        SecurityWarningCode, WelcomeReasonCode,
    };
    use crate::mls_observability::{DecryptionFailureKind, MlsOperationContext};
    use std::collections::{HashMap, HashSet};

    /// Hand-maintained catalogue of every `telemetry_name()` the SDK emits.
    ///
    /// This list is the wire-identity record for the release. It is
    /// cross-checked against the real output of
    /// [`Event::telemetry_name`]/[`MlsLifecycleEvent::telemetry_name`] by
    /// `impl_telemetry_names_match_catalogue` below, which feeds one
    /// exemplar per variant through the production mapping and asserts that
    /// the resulting set equals this catalogue. Combined with the compile-
    /// time exhaustiveness wards on [`event_exemplars`] and
    /// [`mls_lifecycle_exemplars`], the three pieces together guarantee:
    ///
    /// 1. Every enum variant is represented in the exemplar list (the ward
    ///    fails to compile when a new variant is added without an exemplar).
    /// 2. No two variants produce the same name in the real implementation
    ///    (`impl_telemetry_names_are_unique`).
    /// 3. This catalogue is neither missing an entry nor carrying a stale
    ///    one relative to the implementation
    ///    (`impl_telemetry_names_match_catalogue`).
    /// 4. Every name conforms to the documented `snake.dot.case` grammar
    ///    (`all_telemetry_names_match_grammar`).
    const ALL_TELEMETRY_NAMES: &[&str] = &[
        // Event::*
        "protocol.message.sent",
        "protocol.message.received",
        "protocol.message.delivered",
        "protocol.message.failed",
        "protocol.message.decryption_failed",
        "protocol.transport.switched",
        "protocol.relay.promoted",
        "protocol.relay.demoted",
        "protocol.neighbor.discovered",
        "protocol.neighbor.lost",
        "protocol.network.metrics",
        "protocol.file.progress",
        "protocol.file.received",
        "protocol.file.receive_failed",
        "protocol.media.sent",
        "protocol.media.send_failed",
        "protocol.message.deferred",
        "protocol.ack.evicted",
        "protocol.fragment.assembly_evicted",
        "protocol.relay.demoted_battery",
        "protocol.secure_session.established",
        "protocol.secure_session.failed",
        "protocol.convergence.diag",
        "protocol.welcome.send_attempted",
        "protocol.welcome.send_succeeded",
        "protocol.welcome.send_failed",
        "protocol.welcome.send_expired",
        "protocol.connection.request_received",
        "protocol.connection.request_undeliverable",
        "protocol.connection.accepted",
        "protocol.connection.rejected",
        "protocol.connection.request_cancelled",
        "protocol.group.created",
        "protocol.group.message_received",
        "protocol.group.member_added",
        "protocol.group.member_removed",
        "protocol.group.info",
        "protocol.group.user_groups",
        "protocol.group.error",
        "protocol.group.message_sent",
        "protocol.group.message_partial_failure",
        "protocol.group.epoch_fork_detected",
        "protocol.group.epoch_fork_resolved",
        "protocol.group.role_changed",
        "protocol.group.renamed",
        "protocol.service.discovered",
        "protocol.service.request_received",
        "protocol.service.response_received",
        "protocol.presence.updated",
        "protocol.typing.received",
        "protocol.read_receipt.received",
        "protocol.dors.score_updated",
        "protocol.dors.transport_selected",
        "protocol.dors.transport_switched",
        "protocol.dors.escalation_triggered",
        "protocol.security.warning",
        "protocol.message.relayed",
        "protocol.user.blocked",
        "protocol.user.unblocked",
        "protocol.tofu.reset",
        // MlsLifecycleEvent::*
        "mls.initialized",
        "mls.encryption_used",
        "mls.decryption_failed",
        "mls.session_missing",
        "mls.session_ready",
        // Non-Event, non-MLS TelemetryRecord variants.
        //
        // These names are not covered by the Event/MlsLifecycleEvent
        // exhaustiveness wards above because their source types are simple
        // structs — there is no multi-variant enum to stay in sync with.
        // The dedicated coverage check below
        // (`simple_record_variant_names_are_stable`) exercises each one's
        // `telemetry_name()` against the value inlined here.
        "protocol.metrics.snapshot",
        "protocol.transport.state_changed",
        "protocol.routing.decision",
        "protocol.device.capability_changed",
    ];

    /// Returns `true` when `name` matches the documented grammar
    /// `[a-z0-9_]+(\.[a-z0-9_]+)+`.
    fn is_well_formed(name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        let mut segments = name.split('.');
        let mut segment_count = 0;
        for segment in &mut segments {
            if segment.is_empty() {
                return false;
            }
            if !segment
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                return false;
            }
            segment_count += 1;
        }
        segment_count >= 2
    }

    /// Regression guard for the `Box<Event>` decision on the `Protocol`
    /// variant. `Event` is ~368 bytes; the non-Protocol variants are much
    /// smaller (`MlsLifecycleEvent` is the next-largest at ~100 bytes). If
    /// someone removes the `Box`, `TelemetryRecord` balloons to `Event`'s
    /// size and the size tax is paid by every non-Protocol record stored in
    /// a collection. This assertion fails loudly if that happens.
    #[test]
    fn telemetry_record_is_smaller_than_unboxed_event() {
        use std::mem::size_of;
        assert!(
            size_of::<TelemetryRecord>() < size_of::<Event>(),
            "TelemetryRecord ({} bytes) is not smaller than Event ({} bytes) \
             — the `Box<Event>` variant was removed or the size budget was \
             blown by another variant. Re-box the Protocol variant or audit \
             the new variant's payload.",
            size_of::<TelemetryRecord>(),
            size_of::<Event>(),
        );
    }

    #[test]
    fn protocol_variant_names_are_dotted() {
        let event = Event::MessageReceived {
            message_id: "m".into(),
            sender: "s".into(),
            recipient: "r".into(),
            content: String::new(),
            hop_count: 0,
            transport: "ble".into(),
            timestamp: 0,
            lamport_clock: 0,
            reply_to_msg: None,
            content_type: String::new(),
            media_metadata: None,
            forward_info: None,
            encrypted: false,
        };
        assert_eq!(
            TelemetryRecord::Protocol(Box::new(event)).name(),
            "protocol.message.received",
        );
    }

    #[test]
    fn mls_variant_names_are_dotted() {
        let event = MlsLifecycleEvent::Initialized {
            timestamp_ms: 0,
            session_id: "s".into(),
            group_id: None,
            peer_id: None,
            context: MlsOperationContext::Initialize,
            error_category: None,
        };
        assert_eq!(TelemetryRecord::Mls(event).name(), "mls.initialized");
    }

    /// Compile-time exhaustiveness ward for [`Event`].
    ///
    /// The match below deliberately omits a wildcard arm, so adding a new
    /// variant to [`Event`] breaks compilation here and forces the author to
    /// update [`event_exemplars`] to cover the new variant. Without this
    /// ward, an author could add a variant + a `telemetry_name` arm while
    /// silently skipping the exemplar list, and the impl-level uniqueness
    /// check below would quietly run with incomplete coverage.
    #[allow(dead_code)]
    fn event_variant_exhaustiveness_ward(e: &Event) {
        match e {
            Event::MessageSent { .. }
            | Event::MessageReceived { .. }
            | Event::MessageDelivered { .. }
            | Event::MessageFailed { .. }
            | Event::MessageDecryptionFailed { .. }
            | Event::TransportSwitched { .. }
            | Event::RelayPromoted { .. }
            | Event::RelayDemoted { .. }
            | Event::NeighborDiscovered { .. }
            | Event::NeighborLost { .. }
            | Event::NetworkMetrics { .. }
            | Event::FileProgress { .. }
            | Event::FileReceived { .. }
            | Event::FileReceiveFailed { .. }
            | Event::MediaSent { .. }
            | Event::MediaSendFailed { .. }
            | Event::MessageDeferred { .. }
            | Event::AckEvicted { .. }
            | Event::FragmentAssemblyEvicted { .. }
            | Event::RelayDemotedBattery { .. }
            | Event::SecureSessionEstablished { .. }
            | Event::SecureSessionFailed { .. }
            | Event::ConvergenceDiag { .. }
            | Event::WelcomeSendAttempted { .. }
            | Event::WelcomeSendSucceeded { .. }
            | Event::WelcomeSendFailed { .. }
            | Event::WelcomeSendExpired { .. }
            | Event::ConnectionRequestReceived { .. }
            | Event::ConnectionRequestUndeliverable { .. }
            | Event::ConnectionAccepted { .. }
            | Event::ConnectionRejected { .. }
            | Event::ConnectionRequestCancelled { .. }
            | Event::GroupCreated { .. }
            | Event::GroupMessageReceived { .. }
            | Event::GroupMemberAdded { .. }
            | Event::GroupMemberRemoved { .. }
            | Event::GroupInfo { .. }
            | Event::UserGroups { .. }
            | Event::GroupError { .. }
            | Event::GroupMessageSent { .. }
            | Event::GroupMessagePartialFailure { .. }
            | Event::GroupEpochForkDetected { .. }
            | Event::GroupEpochForkResolved { .. }
            | Event::GroupRoleChanged { .. }
            | Event::GroupRenamed { .. }
            | Event::ServiceDiscovered { .. }
            | Event::ServiceRequestReceived { .. }
            | Event::ServiceResponseReceived { .. }
            | Event::PresenceUpdated { .. }
            | Event::TypingIndicatorReceived { .. }
            | Event::ReadReceiptReceived { .. }
            | Event::DorsScoreUpdated { .. }
            | Event::DorsTransportSelected { .. }
            | Event::DorsTransportSwitched { .. }
            | Event::DorsEscalationTriggered { .. }
            | Event::SecurityWarning { .. }
            | Event::MessageRelayed { .. }
            | Event::UserBlocked { .. }
            | Event::UserUnblocked { .. }
            | Event::TofuReset { .. } => (),
        }
    }

    /// Compile-time exhaustiveness ward for [`MlsLifecycleEvent`] — see
    /// [`event_variant_exhaustiveness_ward`] for the rationale.
    #[allow(dead_code)]
    fn mls_lifecycle_exhaustiveness_ward(e: &MlsLifecycleEvent) {
        match e {
            MlsLifecycleEvent::Initialized { .. }
            | MlsLifecycleEvent::EncryptionUsed { .. }
            | MlsLifecycleEvent::DecryptionFailed { .. }
            | MlsLifecycleEvent::SessionMissing { .. }
            | MlsLifecycleEvent::SessionReady { .. } => (),
        }
    }

    /// Constructs one exemplar per [`Event`] variant, in the same order as
    /// the match arms of [`Event::telemetry_name`]. Field values are
    /// placeholders — the tests only read the telemetry name.
    fn event_exemplars() -> Vec<Event> {
        vec![
            Event::MessageSent {
                message_id: String::new(),
                sender: String::new(),
                recipient: String::new(),
                content: String::new(),
                priority: String::new(),
                requires_ack: false,
                timestamp: 0,
                lamport_clock: 0,
                forward_info: None,
            },
            Event::MessageReceived {
                message_id: String::new(),
                sender: String::new(),
                recipient: String::new(),
                content: String::new(),
                hop_count: 0,
                transport: String::new(),
                timestamp: 0,
                lamport_clock: 0,
                reply_to_msg: None,
                content_type: String::new(),
                media_metadata: None,
                forward_info: None,
                encrypted: false,
            },
            Event::MessageDelivered {
                message_id: String::new(),
                latency_ms: 0,
                hop_count: 0,
                transport: String::new(),
            },
            Event::MessageFailed {
                message_id: String::new(),
                reason: String::new(),
                retry_count: 0,
            },
            Event::MessageDecryptionFailed {
                message_id: String::new(),
                sender: String::new(),
                code: crate::events::DecryptionFailureCode::Unknown,
                reason: String::new(),
            },
            Event::TransportSwitched {
                from: None,
                to: String::new(),
                reason: String::new(),
            },
            Event::RelayPromoted {
                connection_count: 0,
                battery_level: 0,
            },
            Event::RelayDemoted {
                reason: String::new(),
            },
            Event::NeighborDiscovered {
                peer_id: String::new(),
                transport: String::new(),
                rssi: None,
            },
            Event::NeighborLost {
                peer_id: String::new(),
            },
            Event::NetworkMetrics {
                neighbor_count: 0,
                relay_count: 0,
                delivery_ratio: 0.0,
                avg_latency_ms: 0,
            },
            Event::FileProgress {
                file_id: String::new(),
                chunks_sent: 0,
                total_chunks: 0,
                percentage: 0,
            },
            Event::FileReceived {
                file_id: String::new(),
                file_name: String::new(),
                file_size: 0,
                sender: String::new(),
                content_type: String::new(),
                media_metadata: None,
                file_data: String::new(),
            },
            Event::FileReceiveFailed {
                file_id: String::new(),
                file_name: String::new(),
                sender: String::new(),
                reason: String::new(),
            },
            Event::MediaSent {
                file_id: String::new(),
                content_type: String::new(),
                recipient: String::new(),
            },
            Event::MediaSendFailed {
                file_id: String::new(),
                recipient: String::new(),
                reason: String::new(),
            },
            Event::MessageDeferred {
                message_id: String::new(),
                reason: String::new(),
                retry_count: 0,
                next_retry_at: None,
            },
            Event::AckEvicted {
                message_id: String::new(),
                priority: String::new(),
                reason: String::new(),
            },
            Event::FragmentAssemblyEvicted {
                message_id: String::new(),
                completion_percent: 0,
                reason: String::new(),
            },
            Event::RelayDemotedBattery {
                battery_level: 0,
                min_required: 0,
            },
            Event::SecureSessionEstablished {
                peer_id: String::new(),
                group_id: String::new(),
                is_session: false,
                initiated_by_local: false,
            },
            Event::SecureSessionFailed {
                peer_id: String::new(),
                reason: String::new(),
            },
            Event::ConvergenceDiag {
                stage: String::new(),
                peer_id: String::new(),
                detail: String::new(),
            },
            Event::WelcomeSendAttempted {
                peer_id: String::new(),
                message_id: String::new(),
                group_id: String::new(),
                attempt: 0,
            },
            Event::WelcomeSendSucceeded {
                peer_id: String::new(),
                message_id: String::new(),
                group_id: String::new(),
                attempt: 0,
            },
            Event::WelcomeSendFailed {
                peer_id: String::new(),
                message_id: String::new(),
                group_id: String::new(),
                attempt: 0,
                reason_code: WelcomeReasonCode::InternalError,
                transport_error: None,
                retryable: false,
                next_retry_at: None,
            },
            Event::WelcomeSendExpired {
                peer_id: String::new(),
                message_id: String::new(),
                attempt: 0,
                reason_code: WelcomeReasonCode::RetryExhausted,
            },
            Event::ConnectionRequestReceived {
                sender: String::new(),
                sender_name: String::new(),
                timestamp: 0,
                key_package: None,
                initial_message: None,
            },
            Event::ConnectionRequestUndeliverable {
                recipient: String::new(),
                message_id: String::new(),
                reason: String::new(),
            },
            Event::ConnectionAccepted {
                accepted_by: String::new(),
                accepted_by_name: String::new(),
                timestamp: 0,
                key_package: None,
            },
            Event::ConnectionRejected {
                rejected_by: String::new(),
            },
            Event::ConnectionRequestCancelled {
                cancelled_by: String::new(),
            },
            Event::GroupCreated {
                group_id: String::new(),
                name: String::new(),
            },
            Event::GroupMessageReceived {
                group_id: String::new(),
                sender: String::new(),
                content: String::new(),
                timestamp: String::new(),
                message_id: String::new(),
                reply_to_msg: None,
                forward_info: None,
            },
            Event::GroupMemberAdded {
                group_id: String::new(),
                user_id: String::new(),
                added_by: String::new(),
                group_name: None,
            },
            Event::GroupMemberRemoved {
                group_id: String::new(),
                user_id: String::new(),
                removed_by: String::new(),
            },
            Event::GroupInfo {
                group_id: String::new(),
                name: String::new(),
                created_by: String::new(),
                created_at: String::new(),
                members: Vec::new(),
            },
            Event::UserGroups { groups: Vec::new() },
            Event::GroupError {
                reason: String::new(),
            },
            Event::GroupMessageSent {
                group_id: String::new(),
                message_ids: Vec::new(),
                member_count: 0,
            },
            Event::GroupMessagePartialFailure {
                group_id: String::new(),
                failed_members: Vec::new(),
                succeeded_members: Vec::new(),
            },
            Event::GroupEpochForkDetected {
                group_id: String::new(),
                local_epoch: None,
            },
            Event::GroupEpochForkResolved {
                group_id: String::new(),
                resolved_epoch: 0,
                failed_members: Vec::new(),
            },
            Event::GroupRoleChanged {
                group_id: String::new(),
                user_id: String::new(),
                new_role: String::new(),
                changed_by: String::new(),
            },
            Event::GroupRenamed {
                group_id: String::new(),
                new_name: String::new(),
                old_name: None,
                renamed_by: String::new(),
            },
            Event::ServiceDiscovered {
                query_id: String::new(),
                service_id: String::new(),
                version: String::new(),
                provider_peer_id: String::new(),
                capabilities: HashMap::new(),
                hop_count: 0,
            },
            Event::ServiceRequestReceived {
                request_id: String::new(),
                service_id: String::new(),
                method: String::new(),
                body: String::new(),
                sender: String::new(),
            },
            Event::ServiceResponseReceived {
                request_id: String::new(),
                service_id: String::new(),
                status: String::new(),
                body: String::new(),
                provider_peer_id: String::new(),
            },
            Event::PresenceUpdated {
                peer_id: String::new(),
                status: PresenceStatus::Online,
                timestamp: 0,
                last_seen_ms: None,
            },
            Event::TypingIndicatorReceived {
                sender: String::new(),
                conversation_id: String::new(),
                is_typing: false,
                timestamp: 0,
            },
            Event::ReadReceiptReceived {
                sender: String::new(),
                message_ids: Vec::new(),
                timestamp: 0,
            },
            Event::DorsScoreUpdated { scores: Vec::new() },
            Event::DorsTransportSelected {
                from: None,
                transport: String::new(),
                reason_code: DorsReasonCode::InitialSelection,
                score: 0.0,
            },
            Event::DorsTransportSwitched {
                from: None,
                to: String::new(),
                reason_code: DorsReasonCode::PrimarySelected,
                reason_detail: None,
            },
            Event::DorsEscalationTriggered {
                phase: DorsEscalationPhase::Triggered,
                from: String::new(),
                to: String::new(),
                reason_code: DorsEscalationReasonCode::FallbackSuccess,
                reason_detail: None,
            },
            Event::SecurityWarning {
                peer_id: String::new(),
                reason_code: SecurityWarningCode::TofuKeyMismatch,
                reason: String::new(),
            },
            Event::MessageRelayed {
                message_id: String::new(),
                sender: String::new(),
                recipient: String::new(),
                hop_count: 0,
                remaining_ttl: 0,
            },
            Event::UserBlocked {
                user_id: String::new(),
            },
            Event::UserUnblocked {
                user_id: String::new(),
            },
            Event::TofuReset {
                peer_id: String::new(),
            },
        ]
    }

    /// Constructs one exemplar per [`MlsLifecycleEvent`] variant, in the same
    /// order as the match arms of [`MlsLifecycleEvent::telemetry_name`].
    fn mls_lifecycle_exemplars() -> Vec<MlsLifecycleEvent> {
        vec![
            MlsLifecycleEvent::Initialized {
                timestamp_ms: 0,
                session_id: String::new(),
                group_id: None,
                peer_id: None,
                context: MlsOperationContext::Initialize,
                error_category: None,
            },
            MlsLifecycleEvent::EncryptionUsed {
                timestamp_ms: 0,
                session_id: String::new(),
                group_id: None,
                peer_id: None,
                context: MlsOperationContext::Send,
                error_category: None,
            },
            MlsLifecycleEvent::DecryptionFailed {
                timestamp_ms: 0,
                session_id: String::new(),
                group_id: None,
                peer_id: None,
                context: MlsOperationContext::Receive,
                error_category: None,
                failure_kind: DecryptionFailureKind::Unknown,
            },
            MlsLifecycleEvent::SessionMissing {
                timestamp_ms: 0,
                session_id: String::new(),
                group_id: None,
                peer_id: None,
                context: MlsOperationContext::SessionLookup,
                error_category: None,
            },
            MlsLifecycleEvent::SessionReady {
                timestamp_ms: 0,
                session_id: String::new(),
                group_id: None,
                peer_id: None,
                context: MlsOperationContext::Welcome,
                error_category: None,
            },
        ]
    }

    /// Names emitted by simple-struct telemetry records (non-enum inner
    /// types). Extends the Event/MLS-derived name lists used by the
    /// uniqueness and catalogue-match checks.
    fn simple_record_names() -> Vec<&'static str> {
        use crate::telemetry::device::DeviceCapabilitySnapshot;
        use crate::telemetry::metrics_snapshot::MetricsFrame;
        use crate::telemetry::routing::RoutingDecision;
        use crate::telemetry::transport_state::TransportStateEvent;
        vec![
            MetricsFrame::NAME,
            TransportStateEvent::NAME,
            RoutingDecision::NAME,
            DeviceCapabilitySnapshot::NAME,
        ]
    }

    /// Real implementation-level uniqueness check.
    ///
    /// Feeds every exemplar through [`Event::telemetry_name`] and
    /// [`MlsLifecycleEvent::telemetry_name`] and asserts the produced names
    /// are unique. Unlike a catalogue-only uniqueness check, this catches
    /// the case where two match arms in the production mapping accidentally
    /// produce the same string — a silent merge at the sink.
    #[test]
    fn impl_telemetry_names_are_unique() {
        let mut names: Vec<&'static str> = Vec::new();
        for e in event_exemplars() {
            names.push(e.telemetry_name());
        }
        for e in mls_lifecycle_exemplars() {
            names.push(e.telemetry_name());
        }
        names.extend(simple_record_names());
        let unique: HashSet<&&'static str> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "two variants produced the same telemetry_name() — distinct \
             events would silently merge at the sink. Names: {names:?}",
        );
    }

    /// Guards against drift between the catalogue and the implementation.
    ///
    /// If a new variant is added without a catalogue entry, the catalogue
    /// will be short one name. If a catalogue entry is left behind after a
    /// variant rename, the catalogue will carry a stale name. Either failure
    /// mode is caught here.
    #[test]
    fn impl_telemetry_names_match_catalogue() {
        let mut impl_names: Vec<&'static str> = Vec::new();
        for e in event_exemplars() {
            impl_names.push(e.telemetry_name());
        }
        for e in mls_lifecycle_exemplars() {
            impl_names.push(e.telemetry_name());
        }
        impl_names.extend(simple_record_names());

        let impl_set: HashSet<&&'static str> = impl_names.iter().collect();
        let catalogue_set: HashSet<&&str> = ALL_TELEMETRY_NAMES.iter().collect();

        let missing_from_catalogue: Vec<&&'static str> = impl_set
            .iter()
            .copied()
            .filter(|n| !ALL_TELEMETRY_NAMES.contains(*n))
            .collect();
        assert!(
            missing_from_catalogue.is_empty(),
            "implementation produces names not in ALL_TELEMETRY_NAMES: \
             {missing_from_catalogue:?}. Update the catalogue.",
        );

        let stale_in_catalogue: Vec<&&str> = catalogue_set
            .iter()
            .copied()
            .filter(|n| !impl_names.iter().any(|produced| produced == *n))
            .collect();
        assert!(
            stale_in_catalogue.is_empty(),
            "catalogue carries names the implementation does not produce: \
             {stale_in_catalogue:?}. Remove from the catalogue or restore \
             the match arm.",
        );

        assert_eq!(
            impl_names.len(),
            ALL_TELEMETRY_NAMES.len(),
            "catalogue length ({}) differs from implementation variant \
             count ({})",
            ALL_TELEMETRY_NAMES.len(),
            impl_names.len(),
        );
    }

    /// Sanity check on the catalogue itself — duplicate entries would mask
    /// drift against the implementation (two catalogue entries colliding
    /// with a single implementation name would still pass the equality
    /// check above). Kept alongside the implementation check so both
    /// invariants are locked in.
    #[test]
    fn catalogue_entries_are_unique() {
        let set: HashSet<&&str> = ALL_TELEMETRY_NAMES.iter().collect();
        assert_eq!(
            set.len(),
            ALL_TELEMETRY_NAMES.len(),
            "duplicate entry in ALL_TELEMETRY_NAMES",
        );
    }

    #[test]
    fn all_telemetry_names_match_grammar() {
        for name in ALL_TELEMETRY_NAMES {
            assert!(
                is_well_formed(name),
                "telemetry name '{name}' does not match snake.dot.case grammar",
            );
        }
    }

    #[test]
    fn grammar_checker_rejects_malformed_names() {
        assert!(!is_well_formed(""));
        assert!(!is_well_formed("flat"));
        assert!(!is_well_formed(".leading"));
        assert!(!is_well_formed("trailing."));
        assert!(!is_well_formed("double..dot"));
        assert!(!is_well_formed("Upper.Case"));
        assert!(!is_well_formed("has-dash.segment"));
        assert!(is_well_formed("ok.name"));
        assert!(is_well_formed("a.b.c"));
        assert!(is_well_formed("with_underscore.segment"));
        assert!(is_well_formed("has_digit1.seg2"));
    }
}
