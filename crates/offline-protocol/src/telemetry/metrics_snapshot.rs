//! Periodic snapshot of protocol-wide counters and per-transport metrics.
//!
//! Emitted on the cadence configured by
//! [`crate::telemetry::TelemetryConfig::metrics_cadence`] from
//! `OfflineProtocol::process()`. When cadence is `None`, no periodic emission
//! occurs — apps fall back to pull-based access via `TransportManager::metrics()`.
//!
//! The frame reuses [`TransportMetrics`] verbatim; there is no parallel
//! "rich" struct. Extending that struct (when UniFFI bindings catch up)
//! automatically widens the frame.

use offline_protocol_reliability::{DeduplicatorStats, RetryQueueStats};
use offline_protocol_transport::{TransportMetrics, TransportType};

/// Snapshot of every push-able counter the SDK exposes.
///
/// The inner collections are pre-materialized at aggregation time so the sink
/// receives owned-but-cheap data — no iteration, no locking on the sink side.
#[derive(Debug, Clone)]
pub struct MetricsFrame {
    /// Emission timestamp (milliseconds since Unix epoch).
    pub timestamp_ms: i64,
    /// Per-transport metrics for every transport currently reporting
    /// `TransportStatus::Available`.
    pub transports: Vec<(TransportType, TransportMetrics)>,
    /// Retry-queue statistics (pending + ready + per-priority counts).
    pub retry_queue: RetryQueueStats,
    /// Deduplicator statistics (tracked ids, capacity, mode).
    pub dedup: DeduplicatorStats,
    /// Number of messages awaiting acknowledgement.
    pub ack_pending: usize,
    /// Number of currently known neighbors.
    pub neighbor_count: usize,
    /// Whether this node is currently acting as a relay on the mesh.
    ///
    /// Reflects `RelayManager::current_role()` at the moment of emission.
    /// Named deliberately — this is a local-role flag, not a peer count.
    /// A future record with an explicit per-peer-relay registry may add a
    /// `relay_peer_count` field alongside; those semantics are orthogonal.
    pub is_local_relay: bool,
    /// Currently selected transport, if any.
    pub current_transport: Option<TransportType>,
}

impl MetricsFrame {
    /// Stable OTel-friendly name for this record.
    pub const NAME: &'static str = "protocol.metrics.snapshot";

    /// Returns the stable telemetry name.
    pub fn telemetry_name(&self) -> &'static str {
        Self::NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_reliability::DeduplicatorMode;

    fn sample_frame() -> MetricsFrame {
        MetricsFrame {
            timestamp_ms: 0,
            transports: Vec::new(),
            retry_queue: RetryQueueStats {
                total_count: 0,
                ready_count: 0,
                critical_priority_count: 0,
                high_priority_count: 0,
                medium_priority_count: 0,
                low_priority_count: 0,
            },
            dedup: DeduplicatorStats {
                total_tracked: 0,
                recent_tracked: 0,
                capacity_used_percent: 0,
                false_positive_rate: None,
                mode: DeduplicatorMode::HashMap,
            },
            ack_pending: 0,
            neighbor_count: 0,
            is_local_relay: false,
            current_transport: None,
        }
    }

    #[test]
    fn name_is_stable() {
        assert_eq!(MetricsFrame::NAME, "protocol.metrics.snapshot");
        assert_eq!(sample_frame().telemetry_name(), "protocol.metrics.snapshot");
    }
}
