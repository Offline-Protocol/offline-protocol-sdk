//! Main protocol engine for the Offline Protocol SDK.
//!
//! This crate ties together all the components (core, transport, router, reliability)
//! into a single easy-to-use API.
//!
//! All code in this crate is 100% safe Rust with no unsafe blocks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod config;
pub mod constants;
pub mod error;
pub mod events;
pub mod file_transfer;
mod group_mesh;
mod media_envelope;
pub mod mls;
pub mod mls_observability;
pub mod protocol;
pub mod telemetry;
pub mod transport_manager;
pub mod visualization;

pub use config::{
    EncryptionConfig, GroupConfig, OverflowPolicy, PendingQueueConfig, ProtocolConfig,
    SecurityConfig,
};
pub use error::{Error, EstablishmentState, Result, SessionStateError};
pub use events::{
    DecryptionFailureCode, DorsEscalationPhase, DorsEscalationReasonCode, DorsReasonCode, Event,
    EventCallback, GroupInfoMember, PresenceSource, PresenceStatus, UserGroupSummary,
};
pub use group_mesh::GroupSendOptions;
pub use offline_protocol_services::MeshServices;
pub use protocol::{MediaSendOptions, OfflineProtocol, SendMessageOptions};
pub use transport_manager::TransportManager;
pub use visualization::{
    MessageStats, NetworkLink, NetworkNode, NetworkTopology, NetworkVisualizer, NodeRole,
};

#[cfg(test)]
mod dors_integration_tests;
#[cfg(test)]
mod group_mesh_tests;

// Re-export reliability types for configuration and telemetry frames
pub use offline_protocol_reliability::{
    AckConfig, DeduplicatorConfig, DeduplicatorMode, DeduplicatorStats, RetryConfig,
    RetryQueueStats,
};

// Re-export MLS types for end-to-end encryption
pub use mls::{
    EncryptedMessage, GroupId, GroupInfo, KeyPackageBundle, MlsManager, MlsStorage, WelcomeMessage,
};
pub use mls_observability::{
    DecryptionFailureKind, MlsErrorCategory, MlsEventEmitter, MlsLifecycleEvent,
    MlsOperationContext, NoopMlsEventEmitter,
};
pub use telemetry::{
    DeviceCapabilitySnapshot, MetricsFrame, MlsVerbosity, NoopTelemetrySink, RoutingDecision,
    RoutingPhase, RoutingReasonCode, TelemetryConfig, TelemetryRecord, TelemetrySink,
    TransportStateEvent,
};
