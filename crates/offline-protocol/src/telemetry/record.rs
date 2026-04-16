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
//! The enum is `#[non_exhaustive]` so new categories can be added without a
//! major-version bump. The record intentionally does not derive `Serialize`:
//! [`TelemetryRecord::name`] is the canonical wire identity, and sinks that
//! need on-the-wire serialization serialize the inner typed payload
//! (`Event`/`MlsLifecycleEvent`) directly — this avoids maintaining two
//! parallel naming schemes.

use crate::events::Event;
use crate::mls_observability::MlsLifecycleEvent;

/// A single typed telemetry emission.
///
/// Every record corresponds to exactly one `snake.dot.case` name returned by
/// [`TelemetryRecord::name`]. The name is delegated to the inner payload's
/// `telemetry_name()` method so there is a single source of truth for variant
/// naming.
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
    /// Returns the stable `snake.dot.case` name for this record.
    ///
    /// Names are compatible with OpenTelemetry naming conventions and are
    /// considered stable across minor versions of the SDK.
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
}
