//! Telemetry primitives for the Offline Protocol SDK.
//!
//! This module is the unified observer surface for everything the SDK emits:
//! protocol events, MLS lifecycle events, transport-state transitions,
//! periodic metrics snapshots, routing decisions, and device-capability
//! snapshots all flow as [`TelemetryRecord`] values through a single
//! [`TelemetrySink`].
//!
//! This PR ships the types only — emit-path wiring, the `install_*` entry
//! point on the protocol engine, and FFI bridging land in follow-up work.
//! The `telemetry` cargo feature is present (default-on) but does not yet
//! gate any code paths; it is reserved for future compile-time control of
//! emit hot-paths.

pub mod config;
pub mod record;
pub mod sampling;
pub mod scrubber;
pub mod sink;

pub use config::{MlsVerbosity, TelemetryConfig};
pub use record::{
    DeviceCapabilitySnapshot, MetricsFrame, RoutingDecision, TelemetryRecord, TransportStateEvent,
};
pub use sampling::CategorySampler;
pub use scrubber::Scrubber;
pub use sink::{NoopTelemetrySink, TelemetrySink};
