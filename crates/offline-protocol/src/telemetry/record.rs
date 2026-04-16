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
}

impl TelemetryRecord {
    /// Returns the stable name for this record. See the module-level docs
    /// for the exact grammar and stability guarantees.
    pub fn name(&self) -> &'static str {
        match self {
            TelemetryRecord::Protocol(event) => event.telemetry_name(),
            TelemetryRecord::Mls(event) => event.telemetry_name(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mls_observability::MlsOperationContext;
    use std::collections::HashSet;

    /// Hand-maintained catalogue of every `telemetry_name()` the SDK emits.
    ///
    /// This list must stay in lock-step with the match arms in
    /// `Event::telemetry_name` and `MlsLifecycleEvent::telemetry_name`.
    /// Compiler exhaustiveness on those matches guarantees every variant
    /// produces *some* name; the tests below then enforce two additional
    /// invariants that the compiler cannot: (1) no two variants collide on
    /// the same name (which would silently merge distinct events at the
    /// sink), and (2) every name conforms to the documented `snake.dot.case`
    /// grammar.
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
        "protocol.media.sent",
        "protocol.message.deferred",
        "protocol.ack.evicted",
        "protocol.fragment.assembly_evicted",
        "protocol.relay.demoted_battery",
        "protocol.secure_session.established",
        "protocol.secure_session.failed",
        "protocol.welcome.send_attempted",
        "protocol.welcome.send_succeeded",
        "protocol.welcome.send_failed",
        "protocol.welcome.send_expired",
        "protocol.connection.request_received",
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
    fn all_telemetry_names_are_unique() {
        let set: HashSet<&&str> = ALL_TELEMETRY_NAMES.iter().collect();
        assert_eq!(
            set.len(),
            ALL_TELEMETRY_NAMES.len(),
            "duplicate telemetry names — two variants collide on the same \
             wire identity. Distinct events would silently merge at the sink.",
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
