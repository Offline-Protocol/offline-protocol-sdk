//! Telemetry primitives for the Offline Protocol SDK.
//!
//! This module is the unified observer surface for everything the SDK emits:
//! protocol events, MLS lifecycle events, periodic metrics snapshots,
//! transport-status transitions, structured routing decisions, and
//! device-capability snapshots all flow as [`TelemetryRecord`] values
//! through a single [`TelemetrySink`].

pub(crate) mod aggregator;
pub mod config;
pub(crate) mod context;
pub mod device;
pub(crate) mod dispatch;
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
pub(crate) use dispatch::dispatch_record;
pub use metrics_snapshot::MetricsFrame;
pub use record::TelemetryRecord;
pub use routing::{RoutingDecision, RoutingPhase, RoutingReasonCode};
pub use sampling::CategorySampler;
pub(crate) use scrubber::Scrubber;
pub use sink::{NoopTelemetrySink, TelemetrySink};
pub use transport_state::TransportStateEvent;
