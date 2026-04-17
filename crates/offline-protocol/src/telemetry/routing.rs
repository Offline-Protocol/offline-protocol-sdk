//! Structured superset of the legacy `Event::Dors*` events.
//!
//! Where the `Event::DorsScoreUpdated` / `Event::DorsTransportSelected` /
//! `Event::DorsTransportSwitched` / `Event::DorsEscalationTriggered` variants
//! report a flattened subset (transport name + score + reason code), this
//! record carries the full per-factor scoring breakdown that DORS evaluates
//! before it decides which transport to use.
//!
//! The legacy `Event::Dors*` path continues to fire for back-compat — apps
//! that want the richer shape consume [`crate::telemetry::TelemetrySink`] and
//! opt into score populating via
//! [`crate::telemetry::TelemetryConfig::with_routing_diagnostic`].

use offline_protocol_router::TransportScore;
use offline_protocol_transport::TransportType;

/// Which kind of routing decision this record describes.
///
/// `#[non_exhaustive]` so new phases can be added without a breaking-change bump.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingPhase {
    /// DORS recomputed per-transport scores without switching.
    ScoreUpdated,
    /// DORS picked a transport (initial selection or post-switch).
    Selected,
    /// DORS changed the active transport.
    Switched,
    /// BLE→WiFi escalation triggered or applied.
    Escalated,
}

/// Top-level reason space for routing decisions.
///
/// Unifies the legacy `DorsReasonCode` (selection/switch reasons) and
/// `DorsEscalationReasonCode` (escalation reasons) into a single flat enum
/// that a sink can match on without juggling multiple legacy code spaces.
/// `#[non_exhaustive]` so new reasons can be added.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingReasonCode {
    /// Initial selection at protocol start.
    InitialSelection,
    /// Primary transport selected on merit.
    PrimarySelected,
    /// Primary transport's last send succeeded.
    PrimarySuccess,
    /// Fallback transport's last send succeeded.
    FallbackSuccess,
    /// Escalation fallback was applied end-to-end.
    EscalationApplied,
    /// Current transport became unavailable.
    CurrentUnavailable,
    /// Retry-failure threshold tripped escalation.
    RetryThreshold,
    /// Sustained poor signal tripped escalation.
    PoorSignal,
    /// Sustained congestion tripped escalation.
    Congestion,
    /// Low TTL headroom tripped escalation.
    LowTtl,
    /// Sustained low success rate tripped escalation.
    LowSuccessRate,
}

/// A single structured routing decision.
///
/// Populated by the DORS callback installed by
/// `OfflineProtocol::install_telemetry_sink`. The `scores` field is only
/// populated when `TelemetryConfig::routing_diagnostic` is `true`; at the
/// default verbosity the vector is empty, keeping the hot-path allocation
/// free.
///
/// A future PR will introduce a `Suppressed` phase carrying hysteresis /
/// cooldown / stability detail; that variant and its payload were excluded
/// from this record on purpose so the shape can be designed alongside the
/// emit sites that produce it.
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    /// Emission timestamp (milliseconds since Unix epoch).
    pub timestamp_ms: i64,
    /// Which kind of decision this is.
    pub phase: RoutingPhase,
    /// Previous transport, when applicable.
    pub from: Option<TransportType>,
    /// Chosen / target transport, when applicable.
    pub to: Option<TransportType>,
    /// Total score of the winning transport, when applicable.
    pub winning_score: Option<f32>,
    /// Reason for the decision, when one applies.
    pub reason_code: Option<RoutingReasonCode>,
    /// Full per-transport score breakdown. Empty unless
    /// `TelemetryConfig::routing_diagnostic` is enabled.
    pub scores: Vec<(TransportType, TransportScore)>,
}

impl RoutingDecision {
    /// Stable OTel-friendly name for this record.
    pub const NAME: &'static str = "protocol.routing.decision";

    /// Returns the stable telemetry name.
    pub fn telemetry_name(&self) -> &'static str {
        Self::NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_stable() {
        let decision = RoutingDecision {
            timestamp_ms: 0,
            phase: RoutingPhase::Selected,
            from: None,
            to: Some(TransportType::BLE),
            winning_score: Some(84.0),
            reason_code: Some(RoutingReasonCode::InitialSelection),
            scores: Vec::new(),
        };
        assert_eq!(RoutingDecision::NAME, "protocol.routing.decision");
        assert_eq!(decision.telemetry_name(), "protocol.routing.decision");
    }
}
