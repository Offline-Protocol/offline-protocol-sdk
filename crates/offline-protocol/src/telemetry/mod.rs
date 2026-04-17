//! Telemetry primitives for the Offline Protocol SDK.
//!
//! This module is the unified observer surface for everything the SDK emits:
//! protocol events and MLS lifecycle events flow as [`TelemetryRecord`]
//! values through a single [`TelemetrySink`]. Additional categories
//! (transport state, metrics snapshots, routing decisions, device-capability
//! snapshots) are introduced alongside the emit sites that populate them.
//!
//! This PR ships the types only — emit-path wiring and the `install_*` entry
//! point on the protocol engine land in follow-up work.

pub(crate) mod aggregator;
pub mod config;
pub(crate) mod context;
pub mod device;
pub mod metrics_snapshot;
pub mod record;
pub mod routing;
pub mod sampling;
pub(crate) mod scrub_event;
pub(crate) mod scrubber;
pub mod sink;
pub mod transport_state;

pub use config::{MlsVerbosity, TelemetryConfig};
pub(crate) use context::TelemetryContext;
pub use device::{DeviceCapabilitySnapshot, CHANGED_BATTERY, CHANGED_CHARGING, CHANGED_RELAY_ROLE};
pub use metrics_snapshot::MetricsFrame;
pub use record::TelemetryRecord;
pub use routing::{RoutingDecision, RoutingPhase, RoutingReasonCode, SuppressionReason};
pub use sampling::CategorySampler;
pub(crate) use scrubber::Scrubber;
pub use sink::{NoopTelemetrySink, TelemetrySink};
pub use transport_state::TransportStateEvent;
