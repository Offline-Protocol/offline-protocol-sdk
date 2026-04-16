//! Unified telemetry record taxonomy.
//!
//! [`TelemetryRecord`] is the single typed surface that every SDK event,
//! metric, and lifecycle signal flows through. Two variants wrap existing
//! types in place (`Protocol` wraps [`crate::events::Event`], `Mls` wraps
//! [`crate::mls_observability::MlsLifecycleEvent`]); the remaining four are
//! placeholder carriers whose fields are filled in by follow-up work
//! (periodic metrics snapshots, transport state transitions, routing
//! decisions, device-capability snapshots).
//!
//! The enum is `#[non_exhaustive]` so new categories can be added without a
//! major-version bump. Inner placeholder structs are also `#[non_exhaustive]`
//! and expose `pub fn new(...)` constructors — additive field growth in
//! follow-up items is non-breaking for third-party consumers.

use serde::Serialize;

use crate::events::Event;
use crate::mls_observability::MlsLifecycleEvent;

/// A single typed telemetry emission.
///
/// Every record corresponds to exactly one `snake.dot.case` name returned by
/// [`TelemetryRecord::name`]. Category-level names are used for the
/// placeholder variants (`transport.state.changed`, `metrics.snapshot`,
/// `routing.decision`, `device.capability.snapshot`); the `Protocol` and
/// `Mls` variants map each sub-variant to a distinct dotted name.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
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
    /// A transport status transition (connect, disconnect, error, etc.).
    TransportState(TransportStateEvent),
    /// A periodic snapshot of aggregated metrics.
    MetricsSnapshot(MetricsFrame),
    /// A routing decision emitted by DORS / PathSelector / RelayManager.
    Routing(RoutingDecision),
    /// A device-capability change (battery, charging, advertised services).
    Device(DeviceCapabilitySnapshot),
}

impl TelemetryRecord {
    /// Returns the stable `snake.dot.case` name for this record.
    ///
    /// Names are intended to be compatible with OpenTelemetry's naming
    /// conventions and are stable across minor versions of the SDK. For the
    /// `Protocol` and `Mls` variants the name is derived from the nested
    /// sub-variant; for the placeholder variants the category-level name is
    /// returned until follow-up work introduces sub-types.
    pub fn name(&self) -> &'static str {
        match self {
            TelemetryRecord::Protocol(event) => protocol_event_name(event.as_ref()),
            TelemetryRecord::Mls(event) => mls_lifecycle_event_name(event),
            TelemetryRecord::TransportState(_) => "transport.state.changed",
            TelemetryRecord::MetricsSnapshot(_) => "metrics.snapshot",
            TelemetryRecord::Routing(_) => "routing.decision",
            TelemetryRecord::Device(_) => "device.capability.snapshot",
        }
    }
}

fn protocol_event_name(event: &Event) -> &'static str {
    match event {
        Event::MessageSent { .. } => "protocol.message.sent",
        Event::MessageReceived { .. } => "protocol.message.received",
        Event::MessageDelivered { .. } => "protocol.message.delivered",
        Event::MessageFailed { .. } => "protocol.message.failed",
        Event::MessageDecryptionFailed { .. } => "protocol.message.decryption_failed",
        Event::TransportSwitched { .. } => "protocol.transport.switched",
        Event::RelayPromoted { .. } => "protocol.relay.promoted",
        Event::RelayDemoted { .. } => "protocol.relay.demoted",
        Event::NeighborDiscovered { .. } => "protocol.neighbor.discovered",
        Event::NeighborLost { .. } => "protocol.neighbor.lost",
        Event::NetworkMetrics { .. } => "protocol.network.metrics",
        Event::FileProgress { .. } => "protocol.file.progress",
        Event::FileReceived { .. } => "protocol.file.received",
        Event::MediaSent { .. } => "protocol.media.sent",
        Event::MessageDeferred { .. } => "protocol.message.deferred",
        Event::AckEvicted { .. } => "protocol.ack.evicted",
        Event::FragmentAssemblyEvicted { .. } => "protocol.fragment.assembly_evicted",
        Event::RelayDemotedBattery { .. } => "protocol.relay.demoted_battery",
        Event::SecureSessionEstablished { .. } => "protocol.secure_session.established",
        Event::SecureSessionFailed { .. } => "protocol.secure_session.failed",
        Event::WelcomeSendAttempted { .. } => "protocol.welcome.send_attempted",
        Event::WelcomeSendSucceeded { .. } => "protocol.welcome.send_succeeded",
        Event::WelcomeSendFailed { .. } => "protocol.welcome.send_failed",
        Event::WelcomeSendExpired { .. } => "protocol.welcome.send_expired",
        Event::ConnectionRequestReceived { .. } => "protocol.connection.request_received",
        Event::ConnectionAccepted { .. } => "protocol.connection.accepted",
        Event::ConnectionRejected { .. } => "protocol.connection.rejected",
        Event::ConnectionRequestCancelled { .. } => "protocol.connection.request_cancelled",
        Event::GroupCreated { .. } => "protocol.group.created",
        Event::GroupMessageReceived { .. } => "protocol.group.message_received",
        Event::GroupMemberAdded { .. } => "protocol.group.member_added",
        Event::GroupMemberRemoved { .. } => "protocol.group.member_removed",
        Event::GroupInfo { .. } => "protocol.group.info",
        Event::UserGroups { .. } => "protocol.group.user_groups",
        Event::GroupError { .. } => "protocol.group.error",
        Event::GroupMessageSent { .. } => "protocol.group.message_sent",
        Event::GroupMessagePartialFailure { .. } => "protocol.group.message_partial_failure",
        Event::GroupEpochForkDetected { .. } => "protocol.group.epoch_fork_detected",
        Event::GroupEpochForkResolved { .. } => "protocol.group.epoch_fork_resolved",
        Event::GroupRoleChanged { .. } => "protocol.group.role_changed",
        Event::GroupRenamed { .. } => "protocol.group.renamed",
        Event::ServiceDiscovered { .. } => "protocol.service.discovered",
        Event::ServiceRequestReceived { .. } => "protocol.service.request_received",
        Event::ServiceResponseReceived { .. } => "protocol.service.response_received",
        Event::PresenceUpdated { .. } => "protocol.presence.updated",
        Event::TypingIndicatorReceived { .. } => "protocol.typing.received",
        Event::ReadReceiptReceived { .. } => "protocol.read_receipt.received",
        Event::DorsScoreUpdated { .. } => "protocol.dors.score_updated",
        Event::DorsTransportSelected { .. } => "protocol.dors.transport_selected",
        Event::DorsTransportSwitched { .. } => "protocol.dors.transport_switched",
        Event::DorsEscalationTriggered { .. } => "protocol.dors.escalation_triggered",
        Event::SecurityWarning { .. } => "protocol.security.warning",
        Event::MessageRelayed { .. } => "protocol.message.relayed",
        Event::UserBlocked { .. } => "protocol.user.blocked",
        Event::UserUnblocked { .. } => "protocol.user.unblocked",
        Event::TofuReset { .. } => "protocol.tofu.reset",
    }
}

fn mls_lifecycle_event_name(event: &MlsLifecycleEvent) -> &'static str {
    match event {
        MlsLifecycleEvent::Initialized { .. } => "mls.initialized",
        MlsLifecycleEvent::EncryptionUsed { .. } => "mls.encryption_used",
        MlsLifecycleEvent::DecryptionFailed { .. } => "mls.decryption_failed",
        MlsLifecycleEvent::SessionMissing { .. } => "mls.session_missing",
        MlsLifecycleEvent::SessionReady { .. } => "mls.session_ready",
    }
}

/// Placeholder record for a transport state transition.
///
/// Fields beyond `timestamp_ms` are populated by follow-up work that wires
/// DORS / transport crates into the telemetry path.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize)]
pub struct TransportStateEvent {
    /// Wall-clock timestamp in Unix milliseconds when the transition occurred.
    pub timestamp_ms: i64,
}

impl TransportStateEvent {
    /// Creates a new transport-state event with only the timestamp populated.
    pub fn new(timestamp_ms: i64) -> Self {
        Self { timestamp_ms }
    }
}

/// Placeholder record for a periodic metrics snapshot.
///
/// Fields beyond `timestamp_ms` are populated by follow-up work that
/// aggregates transport / dedup / retry-queue / topology metrics on the
/// cadence configured via `TelemetryConfig::metrics_cadence_ms`.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize)]
pub struct MetricsFrame {
    /// Wall-clock timestamp in Unix milliseconds when the snapshot was taken.
    pub timestamp_ms: i64,
}

impl MetricsFrame {
    /// Creates a new metrics snapshot with only the timestamp populated.
    pub fn new(timestamp_ms: i64) -> Self {
        Self { timestamp_ms }
    }
}

/// Placeholder record for a routing decision.
///
/// Fields beyond `timestamp_ms` are populated by follow-up work that emits
/// structured records from DORS / PathSelector / RelayManager as typed
/// supersets of the existing `DorsScoreUpdated` event.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize)]
pub struct RoutingDecision {
    /// Wall-clock timestamp in Unix milliseconds when the decision was made.
    pub timestamp_ms: i64,
}

impl RoutingDecision {
    /// Creates a new routing decision with only the timestamp populated.
    pub fn new(timestamp_ms: i64) -> Self {
        Self { timestamp_ms }
    }
}

/// Placeholder record for a device-capability snapshot.
///
/// Fields beyond `timestamp_ms` are populated by follow-up work that emits on
/// battery / charging / advertised-service changes.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize)]
pub struct DeviceCapabilitySnapshot {
    /// Wall-clock timestamp in Unix milliseconds when the snapshot was taken.
    pub timestamp_ms: i64,
}

impl DeviceCapabilitySnapshot {
    /// Creates a new device-capability snapshot with only the timestamp populated.
    pub fn new(timestamp_ms: i64) -> Self {
        Self { timestamp_ms }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mls_observability::MlsOperationContext;

    #[test]
    fn placeholder_names_are_stable() {
        assert_eq!(
            TelemetryRecord::TransportState(TransportStateEvent::new(0)).name(),
            "transport.state.changed",
        );
        assert_eq!(
            TelemetryRecord::MetricsSnapshot(MetricsFrame::new(0)).name(),
            "metrics.snapshot",
        );
        assert_eq!(
            TelemetryRecord::Routing(RoutingDecision::new(0)).name(),
            "routing.decision",
        );
        assert_eq!(
            TelemetryRecord::Device(DeviceCapabilitySnapshot::new(0)).name(),
            "device.capability.snapshot",
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

    #[test]
    fn record_serializes_to_json() {
        let event = MlsLifecycleEvent::Initialized {
            timestamp_ms: 42,
            session_id: "sid".into(),
            group_id: None,
            peer_id: None,
            context: MlsOperationContext::Initialize,
            error_category: None,
        };
        let record = TelemetryRecord::Mls(event);
        let json = serde_json::to_value(&record).expect("serialize");
        assert!(
            json.get("mls").is_some(),
            "expected externally-tagged variant key, got {json}"
        );
    }
}
