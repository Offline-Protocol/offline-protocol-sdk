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
pub mod protocol_state_storage;
pub mod storage_conformance;
pub mod telemetry;
pub mod transport_manager;
pub mod visualization;

pub use config::{
    DataConfig, EncryptionConfig, GroupConfig, OverflowPolicy, PendingQueueConfig, ProtocolConfig,
    SecurityConfig, DEFAULT_PENDING_TTL_MS,
};
pub use error::{Error, EstablishmentState, Result, SessionStateError};
pub use events::{
    DecryptionFailureCode, DorsEscalationPhase, DorsEscalationReasonCode, DorsReasonCode, Event,
    EventCallback, GroupInfoMember, PresenceSource, PresenceStatus, UserGroupSummary,
};
pub use group_mesh::{GroupRichReadiness, GroupSendOptions, RelaySyncState};
/// Replicated-document types.
///
/// Re-exported so callers never name `offline-protocol-data` directly: the
/// value model and the size constants are part of this crate's surface, and
/// the engine behind them is not part of anyone's.
#[cfg(feature = "data")]
pub use offline_protocol_data::{DataValue, DOC_SIZE_WARN_BYTES, MAX_DOC_BYTES, MAX_VALUE_BYTES};
pub use offline_protocol_services::MeshServices;
pub use protocol::mesh_relay::{MeshRelayConfig, MeshRelayStats};
pub use protocol::{GatewayCarrier, MediaSendOptions, OfflineProtocol, SendMessageOptions};
pub use protocol_state_storage::{
    ProtocolStateError, ProtocolStateResult, ProtocolStateStorage,
    MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES,
};
pub use transport_manager::TransportManager;
pub use visualization::{
    MessageStats, NetworkLink, NetworkNode, NetworkTopology, NetworkVisualizer, NodeRole,
};

#[cfg(test)]
mod dors_integration_tests;
#[cfg(test)]
mod group_mesh_tests;
#[cfg(test)]
mod test_identity;

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
