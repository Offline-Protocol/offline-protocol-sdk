//! UniFFI bindings for the Offline Protocol SDK.
//!
//! This is the complete UniFFI implementation with all features fully integrated
//! with the core protocol.

#![allow(unsafe_code)] // Required for UniFFI generated scaffolding
#![allow(missing_docs)] // Types are documented in offline_protocol.udl

use offline_protocol::{
    DeduplicatorMode as CoreDeduplicatorMode, DeviceCapabilitySnapshot as CoreDeviceSnapshot,
    EstablishmentState as CoreEstablishmentState, Event as CoreEvent,
    MediaSendOptions as CoreMediaSendOptions, MetricsFrame as CoreMetricsFrame,
    MlsVerbosity as CoreMlsVerbosity, NetworkVisualizer,
    NoopTelemetrySink as CoreNoopTelemetrySink, OfflineProtocol as CoreProtocol,
    OverflowPolicy as CoreOverflowPolicy, PendingQueueConfig as CorePendingQueueConfig,
    PresenceStatus as CorePresenceStatus, ProtocolConfig as CoreConfig,
    ProtocolStateError as CoreProtocolStateError, ProtocolStateResult as CoreProtocolStateResult,
    ProtocolStateStorage as CoreProtocolStateStorage, RoutingDecision as CoreRoutingDecision,
    RoutingPhase as CoreRoutingPhase, RoutingReasonCode as CoreRoutingReasonCode,
    SendMessageOptions as CoreSendMessageOptions, TelemetryConfig as CoreTelemetryConfig,
    TelemetryRecord as CoreTelemetryRecord, TelemetrySink as CoreTelemetrySink,
    TransportStateEvent as CoreTransportStateEvent, DEFAULT_PENDING_TTL_MS,
};
use offline_protocol_core::{
    ContentType as CoreContentType, ForwardInfo as CoreForwardInfo,
    MediaMetadata as CoreMediaMetadata, MessageId as CoreMessageId,
    MessagePriority as CorePriority, ReplyContext as CoreReplyContext, Timestamp as CoreTimestamp,
    UserId as CoreUserId,
};
use offline_protocol_mls::{
    EncryptedMessage as CoreEncryptedMessage, GroupId as CoreGroupId, GroupInfo as CoreGroupInfo,
    KeyPackageBundle as CoreKeyPackageBundle, MlsManager as CoreMlsManager,
    MlsStorage as CoreMlsStorage, StorageError as CoreStorageError,
    WelcomeMessage as CoreWelcomeMessage,
};
use offline_protocol_router::RelayRole as CoreRelayRole;
use offline_protocol_router::{
    DorsConfig as CoreDorsConfig, GradientRoutingConfig as CoreGradientRoutingConfig, PathSelector,
};
use offline_protocol_transport::{
    ble::BleTransport, internet::InternetTransport, nostr::NostrTransport,
    reticulum::ReticulumTransport, Transport, TransportMetrics as CoreTransportMetrics,
    TransportStatus as CoreTransportStatus, TransportType as CoreTransportType,
};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

// Include the UniFFI scaffolding
uniffi::include_scaffolding!("offline_protocol");

// ---------------------------------------------------------------------------
// Poison-recovery utilities for non-Result methods.
//
// The UniFFI layer targets mobile platforms where a process crash is worse
// than operating on potentially inconsistent state.  Result-returning methods
// use the `lock_inner()` / `lock_ble()` / … helpers that propagate
// `ProtocolError::LockPoisoned`.  Non-Result methods cannot propagate errors,
// so they recover via `into_inner()` and log a warning for observability.
// ---------------------------------------------------------------------------

fn recover_mutex<'a, T>(lock: &'a Mutex<T>, name: &str) -> std::sync::MutexGuard<'a, T> {
    lock.lock().unwrap_or_else(|e| {
        tracing::warn!(lock = name, "Mutex poisoned — recovering with inner value");
        e.into_inner()
    })
}

fn recover_rwlock_read<'a, T>(
    lock: &'a RwLock<T>,
    name: &str,
) -> std::sync::RwLockReadGuard<'a, T> {
    lock.read().unwrap_or_else(|e| {
        tracing::warn!(lock = name, "RwLock poisoned — recovering with inner value");
        e.into_inner()
    })
}

fn recover_rwlock_write<'a, T>(
    lock: &'a RwLock<T>,
    name: &str,
) -> std::sync::RwLockWriteGuard<'a, T> {
    lock.write().unwrap_or_else(|e| {
        tracing::warn!(lock = name, "RwLock poisoned — recovering with inner value");
        e.into_inner()
    })
}

/// Per-peer establishment state (for SessionNotReady and get_establishment_state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstablishmentState {
    NoKeyPackage,
    HaveKeyPackage,
    SessionPending,
    SessionConfirmed,
}

impl From<CoreEstablishmentState> for EstablishmentState {
    fn from(s: CoreEstablishmentState) -> Self {
        match s {
            CoreEstablishmentState::NoKeyPackage => EstablishmentState::NoKeyPackage,
            CoreEstablishmentState::HaveKeyPackage => EstablishmentState::HaveKeyPackage,
            CoreEstablishmentState::SessionPending => EstablishmentState::SessionPending,
            CoreEstablishmentState::SessionConfirmed => EstablishmentState::SessionConfirmed,
        }
    }
}

/// Error types for protocol operations
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// Protocol not started
    #[error("Protocol not started")]
    NotStarted,

    /// Protocol already started
    #[error("Protocol already started")]
    AlreadyStarted,

    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),

    /// Send operation failed
    #[error("Failed to send message: {0}")]
    SendFailed(String),

    /// No key package available for recipient
    #[error("No key package available for recipient: {0}")]
    NoKeyPackage(String),

    /// Session not ready; establishment in progress (state included for UI/retry).
    #[error("Session not ready: {0:?}")]
    SessionNotReady(EstablishmentState),

    /// Outbound message encryption failed
    #[error("Failed to encrypt message: {0}")]
    EncryptFailed(String),

    /// Invalid state for operation
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// MLS not initialized
    #[error("MLS not initialized")]
    MlsNotInitialized,

    /// MLS operation failed
    #[error("MLS error: {0}")]
    MlsError(String),

    /// Operation rejected because the target user is blocked.
    #[error("User is blocked: {0}")]
    UserBlocked(String),

    /// Too many concurrent media transfers to the same recipient; retry
    /// after an active transfer completes.
    #[error(
        "Too many concurrent media transfers to {0}; retry after an active transfer completes"
    )]
    MediaTransferLimit(String),

    /// Internal lock was poisoned by a panicked thread.
    #[error("Internal lock poisoned: {0}")]
    LockPoisoned(String),

    /// Other error
    #[error("{0}")]
    Other(String),

    // New variants are appended only: the generated Swift/Kotlin/Python
    // bindings decode this enum by positional discriminant, so inserting
    // or removing a variant breaks every committed binding.
    /// Transport-layer failure outside an explicit send operation
    /// (start/stop, inbound data processing). Send-path transport
    /// failures surface as `SendFailed`.
    #[error("Transport error: {0}")]
    TransportError(String),

    /// Payload serialization or deserialization failed.
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Mesh service registry/discovery/request failure.
    #[error("Service error: {0}")]
    ServiceError(String),

    /// Group does not exist locally.
    #[error("Group not found: {0}")]
    GroupNotFound(String),

    /// Operation requires a group role or permission the caller does not hold.
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// A caller-supplied argument failed validation.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
}

/// Error types for MLS storage operations
#[derive(Debug, thiserror::Error)]
pub enum MlsStorageError {
    /// Failed to store data
    #[error("Failed to store data")]
    StoreFailed,

    /// Failed to load data
    #[error("Failed to load data")]
    LoadFailed,

    /// Failed to delete data
    #[error("Failed to delete data")]
    DeleteFailed,

    /// Key not found
    #[error("Key not found")]
    KeyNotFound,

    /// Data is corrupted
    #[error("Corrupted data")]
    CorruptedData,
}

impl From<CoreStorageError> for MlsStorageError {
    fn from(err: CoreStorageError) -> Self {
        match err {
            CoreStorageError::StoreFailed(_) => MlsStorageError::StoreFailed,
            CoreStorageError::LoadFailed(_) => MlsStorageError::LoadFailed,
            CoreStorageError::DeleteFailed(_) => MlsStorageError::DeleteFailed,
            CoreStorageError::KeyNotFound(_) => MlsStorageError::KeyNotFound,
            CoreStorageError::CorruptedData(_) => MlsStorageError::CorruptedData,
            CoreStorageError::Unavailable(_) => MlsStorageError::LoadFailed,
            // Required by `#[non_exhaustive]`, NOT a licence to stop
            // classifying — see the note on `From<offline_protocol::Error>`.
            ref other => {
                tracing::warn!(
                    error = %other,
                    "unmapped StorageError variant reached the FFI boundary; \
                     add an explicit arm in offline-protocol-uniffi"
                );
                MlsStorageError::LoadFailed
            }
        }
    }
}

/// MLS Storage callback interface - apps implement this for platform-native secure storage
pub trait MlsStorageProvider: Send + Sync {
    /// Store data with the given key type and ID
    fn store(&self, key_type: String, key_id: String, data: Vec<u8>)
        -> Result<(), MlsStorageError>;

    /// Load data for the given key type and ID
    fn load(&self, key_type: String, key_id: String) -> Result<Option<Vec<u8>>, MlsStorageError>;

    /// Delete data for the given key type and ID
    fn delete(&self, key_type: String, key_id: String) -> Result<(), MlsStorageError>;

    /// List all key IDs for a given key type
    fn list_keys(&self, key_type: String) -> Result<Vec<String>, MlsStorageError>;
}

/// Install-scoped storage callback for non-cryptographic protocol state.
pub trait ProtocolStateStorageProvider: Send + Sync {
    /// Store data with the given key type and ID.
    fn store(&self, key_type: String, key_id: String, data: Vec<u8>)
        -> Result<(), MlsStorageError>;

    /// Load data for the given key type and ID.
    fn load(&self, key_type: String, key_id: String) -> Result<Option<Vec<u8>>, MlsStorageError>;

    /// Delete data for the given key type and ID.
    fn delete(&self, key_type: String, key_id: String) -> Result<(), MlsStorageError>;

    /// List all key IDs for a given key type.
    fn list_keys(&self, key_type: String) -> Result<Vec<String>, MlsStorageError>;
}

/// Wrapper to adapt UniFFI callback to core MlsStorage trait
struct MlsStorageWrapper {
    provider: Arc<dyn MlsStorageProvider>,
}

/// Wrapper to adapt the install-scoped UniFFI callback to core protocol-state
/// storage while remaining a different concrete type from secure storage.
struct ProtocolStateStorageWrapper {
    provider: Arc<dyn ProtocolStateStorageProvider>,
}

#[derive(Clone, Copy)]
enum StorageOperation {
    Store,
    Load,
    Delete,
}

fn map_storage_provider_error(
    error: MlsStorageError,
    operation: StorageOperation,
    key_id: &str,
) -> CoreStorageError {
    match error {
        MlsStorageError::KeyNotFound => CoreStorageError::KeyNotFound(key_id.to_string()),
        MlsStorageError::CorruptedData => {
            CoreStorageError::CorruptedData("Data corrupted".to_string())
        }
        other => {
            let detail = match other {
                MlsStorageError::StoreFailed => "Store failed",
                MlsStorageError::LoadFailed => "Load failed",
                MlsStorageError::DeleteFailed => "Delete failed",
                MlsStorageError::KeyNotFound | MlsStorageError::CorruptedData => unreachable!(),
            }
            .to_string();
            match operation {
                StorageOperation::Store => CoreStorageError::StoreFailed(detail),
                StorageOperation::Load => CoreStorageError::LoadFailed(detail),
                StorageOperation::Delete => CoreStorageError::DeleteFailed(detail),
            }
        }
    }
}

impl CoreMlsStorage for MlsStorageWrapper {
    fn store(
        &self,
        key_type: &str,
        key_id: &str,
        data: &[u8],
    ) -> offline_protocol_mls::storage::StorageResult<()> {
        self.provider
            .store(key_type.to_string(), key_id.to_string(), data.to_vec())
            .map_err(|error| map_storage_provider_error(error, StorageOperation::Store, key_id))
    }

    fn load(
        &self,
        key_type: &str,
        key_id: &str,
    ) -> offline_protocol_mls::storage::StorageResult<Option<Vec<u8>>> {
        self.provider
            .load(key_type.to_string(), key_id.to_string())
            .map_err(|error| map_storage_provider_error(error, StorageOperation::Load, key_id))
    }

    fn delete(
        &self,
        key_type: &str,
        key_id: &str,
    ) -> offline_protocol_mls::storage::StorageResult<()> {
        self.provider
            .delete(key_type.to_string(), key_id.to_string())
            .map_err(|error| map_storage_provider_error(error, StorageOperation::Delete, key_id))
    }

    fn list_keys(
        &self,
        key_type: &str,
    ) -> offline_protocol_mls::storage::StorageResult<Vec<String>> {
        self.provider
            .list_keys(key_type.to_string())
            .map_err(|error| map_storage_provider_error(error, StorageOperation::Load, ""))
    }
}

/// Maps a provider failure onto the protocol-state contract.
///
/// The UDL keeps declaring `MlsStorageError` on the provider callback — that is
/// the app-facing surface, and churning it would mean regenerating every
/// binding for no behavioral gain — but the *Rust* abstraction no longer speaks
/// the MLS crate's error type, so the two storage domains are decoupled where
/// it matters.
fn map_protocol_state_provider_error(
    error: MlsStorageError,
    operation: StorageOperation,
    key_id: &str,
) -> CoreProtocolStateError {
    match error {
        MlsStorageError::KeyNotFound => CoreProtocolStateError::NotFound(key_id.to_string()),
        MlsStorageError::CorruptedData => {
            CoreProtocolStateError::Corrupted("Data corrupted".to_string())
        }
        other => {
            let detail = match other {
                MlsStorageError::StoreFailed => "Store failed",
                MlsStorageError::LoadFailed => "Load failed",
                MlsStorageError::DeleteFailed => "Delete failed",
                MlsStorageError::KeyNotFound | MlsStorageError::CorruptedData => unreachable!(),
            }
            .to_string();
            match operation {
                StorageOperation::Store => CoreProtocolStateError::StoreFailed(detail),
                StorageOperation::Load => CoreProtocolStateError::LoadFailed(detail),
                StorageOperation::Delete => CoreProtocolStateError::DeleteFailed(detail),
            }
        }
    }
}

impl CoreProtocolStateStorage for ProtocolStateStorageWrapper {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> CoreProtocolStateResult<()> {
        self.provider
            .store(key_type.to_string(), key_id.to_string(), data.to_vec())
            .map_err(|error| {
                map_protocol_state_provider_error(error, StorageOperation::Store, key_id)
            })
    }

    fn load(&self, key_type: &str, key_id: &str) -> CoreProtocolStateResult<Option<Vec<u8>>> {
        self.provider
            .load(key_type.to_string(), key_id.to_string())
            .map_err(|error| {
                map_protocol_state_provider_error(error, StorageOperation::Load, key_id)
            })
    }

    fn delete(&self, key_type: &str, key_id: &str) -> CoreProtocolStateResult<()> {
        self.provider
            .delete(key_type.to_string(), key_id.to_string())
            .map_err(|error| {
                map_protocol_state_provider_error(error, StorageOperation::Delete, key_id)
            })
    }

    fn list_keys(&self, key_type: &str) -> CoreProtocolStateResult<Vec<String>> {
        self.provider
            .list_keys(key_type.to_string())
            .map_err(|error| map_protocol_state_provider_error(error, StorageOperation::Load, ""))
    }
}

impl From<offline_protocol::Error> for ProtocolError {
    // Every variant that exists is still classified explicitly here: a new
    // engine error must get its own arm rather than silently degrading to
    // `Other`, which foreign callers cannot act on. (The original taxonomy rot
    // this guards against came from exactly such a catch-all plus blanket
    // `Other(e.to_string())` wraps accreting per-PR — see PR #144.)
    //
    // The trailing `_` arms below are forced by `#[non_exhaustive]` on the
    // engine error enums, which the library crates carry so that adding a
    // variant is not a semver break for published downstream consumers. That
    // trade costs the compile error that used to enforce the rule above, so it
    // is replaced with a runtime `warn!`: an unmapped variant now shows up in
    // logs and telemetry instead of vanishing into `Other`. When you add an
    // engine error variant, add its arm here — the wildcard is a tripwire, not
    // a destination.
    fn from(err: offline_protocol::Error) -> Self {
        use offline_protocol::Error as E;
        use offline_protocol_core::Error as CoreErr;
        match err {
            E::NotStarted => ProtocolError::NotStarted,
            E::AlreadyStarted => ProtocolError::AlreadyStarted,
            E::InvalidConfiguration(msg) => ProtocolError::InvalidConfiguration(msg),
            E::NoKeyPackage(peer_id) => ProtocolError::NoKeyPackage(peer_id),
            E::SessionNotReady(state) => ProtocolError::SessionNotReady(state.into()),
            E::EncryptFailed(message) => ProtocolError::EncryptFailed(message),
            E::MlsNotInitialized => ProtocolError::MlsNotInitialized,
            // MLS-layer "group missing" joins the mesh-layer variant so
            // callers handle one code for the condition.
            E::Mls(offline_protocol_mls::MlsError::GroupNotFound(group_id)) => {
                ProtocolError::GroupNotFound(group_id)
            }
            E::Mls(err) => ProtocolError::MlsError(err.to_string()),
            E::UserBlocked(user_id) => ProtocolError::UserBlocked(user_id),
            E::MediaTransferLimit(user_id) => ProtocolError::MediaTransferLimit(user_id),
            E::Transport(err) => ProtocolError::TransportError(err.to_string()),
            E::Serialization(msg) => ProtocolError::SerializationError(msg),
            E::Service(err) => ProtocolError::ServiceError(err.to_string()),
            E::GroupNotFound(group_id) => ProtocolError::GroupNotFound(group_id),
            E::PermissionDenied(msg) => ProtocolError::PermissionDenied(msg),
            E::InvalidState(msg) => ProtocolError::InvalidState(msg),
            E::InvalidArgument(msg) => ProtocolError::InvalidArgument(msg),
            E::Core(err) => match err {
                CoreErr::SerializationError(msg) | CoreErr::DeserializationError(msg) => {
                    ProtocolError::SerializationError(msg)
                }
                CoreErr::Other(msg) => ProtocolError::Other(msg),
                err @ (CoreErr::InvalidMessage(_)
                | CoreErr::InvalidUserId(_)
                | CoreErr::InvalidAppId(_)
                | CoreErr::InvalidTTL(_)
                | CoreErr::InvalidHopCount(_)
                | CoreErr::InvalidServiceId(_)) => ProtocolError::InvalidArgument(err.to_string()),
                ref other => {
                    tracing::warn!(
                        error = %other,
                        "unmapped core Error variant reached the FFI boundary; \
                         add an explicit arm in offline-protocol-uniffi"
                    );
                    ProtocolError::Other(other.to_string())
                }
            },
            // Router/reliability failures are delivery-infrastructure
            // internals with no FFI-visible producer today; the first one
            // that appears forces an explicit decision here.
            err @ (E::Router(_) | E::Reliability(_)) => ProtocolError::Other(err.to_string()),
            E::Other(msg) => ProtocolError::Other(msg),
            ref other => {
                tracing::warn!(
                    error = %other,
                    "unmapped engine Error variant reached the FFI boundary; \
                     add an explicit arm in offline-protocol-uniffi"
                );
                ProtocolError::Other(other.to_string())
            }
        }
    }
}

/// Error mapping for send-shaped operations (message, group, media,
/// presence, connection-request sends, and the group-leave notification
/// fan-out): a transport-layer failure during an explicit send surfaces as
/// `SendFailed`; every other error keeps its class from the `From` impl.
/// Non-send paths (start/stop, inbound data processing) map transport
/// failures to `TransportError` instead.
///
/// A transport-layer `SendFailed` is unwrapped to its detail message —
/// `ProtocolError::SendFailed`'s own display prefix already says the send
/// failed, and stacking the transport prefix on top would read
/// "Failed to send message: Send failed: ...".
/// Serializes a received message for `receive_message`, in the core serde
/// `Message` shape so the JSON round-trips straight back into
/// `forward_message` — a media message missing `media_metadata` there would
/// forward with its download URL and content key silently dropped. Every
/// field core `Message` requires must be present (`app_id`, `ttl`, ...);
/// `priority` uses the historical Debug casing (`"Medium"`), which core
/// deserialization accepts via serde aliases. Locked down by the
/// `receive_message_json_round_trips_into_core_message` test.
fn received_message_to_json(msg: &offline_protocol_core::Message) -> Option<String> {
    let msg_id = msg.id.as_str();
    let mut json_value = serde_json::json!({
        "id": msg.id.as_str(),
        "sender": msg.sender.as_str(),
        "recipient": msg.recipient.as_str(),
        "app_id": msg.app_id.as_str(),
        "content": msg.content,
        "timestamp": msg.timestamp.as_millis(),
        "lamport_clock": msg.lamport_clock.value(),
        "ttl": msg.ttl.value(),
        "hop_count": msg.hop_count.value(),
        "priority": format!("{:?}", msg.priority),
    });
    if let Some(ref fwd) = msg.forwarded_from {
        json_value["forwarded_from"] = serde_json::json!({
            "original_sender": fwd.original_sender.as_str(),
            "original_message_id": fwd.original_message_id.as_str(),
            "original_timestamp": fwd.original_timestamp.as_millis(),
            "forward_count": fwd.forward_count,
        });
    }
    json_value["content_type"] = serde_json::json!(msg.content_type);
    if let Some(ref media) = msg.media_metadata {
        match serde_json::to_value(media) {
            Ok(value) => json_value["media_metadata"] = value,
            Err(e) => tracing::warn!(
                message_id = %msg_id,
                error = %e,
                "Failed to serialize media metadata, omitting from message JSON"
            ),
        }
    }
    match serde_json::to_string(&json_value) {
        Ok(json) => Some(json),
        Err(e) => {
            tracing::error!(
                message_id = %msg_id,
                error = %e,
                "Failed to serialize received message — message lost"
            );
            None
        }
    }
}

fn map_send_error(err: offline_protocol::Error) -> ProtocolError {
    match err {
        offline_protocol::Error::Transport(offline_protocol_transport::Error::SendFailed(msg)) => {
            ProtocolError::SendFailed(msg)
        }
        offline_protocol::Error::Transport(e) => ProtocolError::SendFailed(e.to_string()),
        other => other.into(),
    }
}

/// Classifies a receive-side chunk rejection: resource-exhaustion and
/// failed-transfer tombstones are receiver state (`InvalidState` — a retry
/// with a fresh transfer is meaningful), while malformed or mismatched
/// chunks are caller-input defects (`InvalidArgument`).
fn map_chunk_rejection(
    rejection: offline_protocol::file_transfer::ChunkRejection,
) -> ProtocolError {
    use offline_protocol::file_transfer::ChunkRejection;
    let msg = format!("File chunk rejected: {}", rejection.as_str());
    if rejection.is_resource_exhaustion() || rejection == ChunkRejection::PreviouslyFailed {
        ProtocolError::InvalidState(msg)
    } else {
        ProtocolError::InvalidArgument(msg)
    }
}

/// Message priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePriority {
    Low,
    Medium,
    High,
    Critical,
}

impl From<MessagePriority> for CorePriority {
    fn from(priority: MessagePriority) -> Self {
        match priority {
            MessagePriority::Low => CorePriority::Low,
            MessagePriority::Medium => CorePriority::Medium,
            MessagePriority::High => CorePriority::High,
            MessagePriority::Critical => CorePriority::Critical,
        }
    }
}

/// Presence status for a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceStatus {
    Online,
    Away,
    Offline,
}

impl From<PresenceStatus> for CorePresenceStatus {
    fn from(status: PresenceStatus) -> Self {
        match status {
            PresenceStatus::Online => CorePresenceStatus::Online,
            PresenceStatus::Away => CorePresenceStatus::Away,
            PresenceStatus::Offline => CorePresenceStatus::Offline,
        }
    }
}

/// Transport types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransportType {
    Internet,
    Ble,
    #[serde(rename = "wifiDirect")]
    WiFiDirect,
    Reticulum,
    Nostr,
}

/// Protocol state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolState {
    Stopped,
    Running,
    Paused,
}

/// Relay priority
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayPriority {
    Low,
    Medium,
    High,
}

/// BLE peer device information
#[derive(Debug, Clone)]
pub struct PeerDevice {
    pub peer_id: String,
    pub rssi: i16,
    pub last_seen_ms: u64,
}

/// Transport metrics — legacy 6-field counters plus the ~12 optional fields
/// from the richer Rust `TransportMetrics`. Same dict flows through the pull
/// path (`get_transport_metrics`) and the push path (`MetricsFrame.transports`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportMetrics {
    pub packets_sent: u32,
    pub packets_received: u32,
    pub bytes_sent: u32,
    pub bytes_received: u32,
    pub error_rate: f32,
    pub avg_latency_ms: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rssi: Option<i16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub congestion: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_level: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_charging: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_connection_count: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_active_relay: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_ratio: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_rate: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_hop_count: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energy_cost: Option<f32>,
}

// ============================================================================
// TELEMETRY — FFI mirror types for `offline_protocol::telemetry::*`
// ============================================================================

/// MLS lifecycle verbosity tier, mirror of `offline_protocol::MlsVerbosity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlsVerbosity {
    Off,
    Lifecycle,
    Diagnostic,
}

/// Underlying connection status of a single transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransportStatus {
    Available,
    Unavailable,
    Connecting,
    Disconnected,
    Error,
}

/// Local relay role reported by `DeviceCapabilitySnapshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RelayRole {
    Regular,
    Relay,
}

/// Which kind of routing decision a `RoutingDecision` record describes.
///
/// `Unknown` is emitted when the core crate reports a variant this FFI build
/// does not recognise (new-core / old-FFI skew). Consumers should surface it
/// as "unrecognised decision" rather than folding it into an existing phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutingPhase {
    ScoreUpdated,
    Selected,
    Switched,
    Escalated,
    Unknown,
}

/// Flat reason space for routing decisions (unifies the legacy
/// `DorsReasonCode` / `DorsEscalationReasonCode` enums).
///
/// `Unknown` is emitted when the core crate reports a variant this FFI build
/// does not recognise — see [`RoutingPhase::Unknown`] for rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutingReasonCode {
    InitialSelection,
    PrimarySelected,
    PrimarySuccess,
    FallbackSuccess,
    EscalationApplied,
    CurrentUnavailable,
    RetryThreshold,
    PoorSignal,
    Congestion,
    LowTtl,
    LowSuccessRate,
    Unknown,
}

/// Per-transport entry inside a `MetricsFrame`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportMetricsEntry {
    pub transport: TransportType,
    pub metrics: TransportMetrics,
}

/// Retry-queue statistics, mirror of `offline_protocol_reliability::RetryQueueStats`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryQueueStats {
    pub total_count: u64,
    pub ready_count: u64,
    pub critical_priority_count: u64,
    pub high_priority_count: u64,
    pub medium_priority_count: u64,
    pub low_priority_count: u64,
}

/// Deduplicator statistics, mirror of `offline_protocol_reliability::DeduplicatorStats`.
///
/// UDL-level name is `DeduplicatorStatsFrame` to avoid colliding with the
/// `DedupStats` dict already exposed by the legacy `get_dedup_stats` pull
/// API (which has a different field shape).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeduplicatorStatsFrame {
    pub total_tracked: u64,
    pub recent_tracked: u64,
    pub capacity_used_percent: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub false_positive_rate: Option<f64>,
    pub mode: String,
}

/// Periodic snapshot of protocol-wide counters and per-transport metrics.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsFrame {
    pub timestamp_ms: i64,
    pub transports: Vec<TransportMetricsEntry>,
    pub retry_queue: RetryQueueStats,
    pub dedup: DeduplicatorStatsFrame,
    pub ack_pending: u64,
    pub neighbor_count: u64,
    pub is_local_relay: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_transport: Option<TransportType>,
}

/// A single `TransportStatus` transition observed by the protocol engine.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportStateEvent {
    pub timestamp_ms: i64,
    pub transport: TransportType,
    pub previous: TransportStatus,
    pub current: TransportStatus,
}

/// Per-transport score breakdown carried by `RoutingDecision` at the
/// diagnostic verbosity tier.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingScoreEntry {
    pub transport: TransportType,
    pub signal: f32,
    pub proximity: f32,
    pub bandwidth: f32,
    pub congestion: f32,
    pub energy: f32,
    pub reliability: f32,
    pub load: f32,
    pub total: f32,
}

/// A structured routing decision (superset of legacy `Event::Dors*` events).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingDecision {
    pub timestamp_ms: i64,
    pub phase: RoutingPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<TransportType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<TransportType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winning_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<RoutingReasonCode>,
    pub scores: Vec<RoutingScoreEntry>,
}

/// Snapshot of local device capability at the moment of emission.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCapabilitySnapshot {
    pub timestamp_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_level: Option<u8>,
    pub is_charging: bool,
    pub relay_role: RelayRole,
    pub changed_fields: u8,
}

/// Runtime configuration for the telemetry subsystem. All fields are
/// optional on the foreign side; the Rust adapter fills in the
/// privacy-preserving defaults from `TelemetryConfig::default()` when a
/// field is `None`.
///
/// `enable_poll_queue` is FFI-local (not forwarded to `CoreTelemetryConfig`)
/// and controls whether the adapter builds the pull-channel JSON envelope on
/// every emit. Push-only consumers should pass `Some(false)` to skip the
/// per-emit `serde_json` serialization; default (`None` → `true`) preserves
/// the original behaviour so `poll_telemetry_frame` keeps working without
/// the caller opting in.
#[derive(Debug, Clone, Default)]
pub struct TelemetryConfig {
    pub scrub_ids: Option<bool>,
    pub mls_verbosity: Option<MlsVerbosity>,
    pub metrics_cadence_ms: Option<u64>,
    pub routing_diagnostic: Option<bool>,
    pub enable_poll_queue: Option<bool>,
    pub mls_sampling_bypass: Option<bool>,
}

// ---- Conversions: core telemetry types → FFI dicts ----

impl From<MlsVerbosity> for CoreMlsVerbosity {
    fn from(v: MlsVerbosity) -> Self {
        match v {
            MlsVerbosity::Off => CoreMlsVerbosity::Off,
            MlsVerbosity::Lifecycle => CoreMlsVerbosity::Lifecycle,
            MlsVerbosity::Diagnostic => CoreMlsVerbosity::Diagnostic,
        }
    }
}

impl From<CoreTransportType> for TransportType {
    fn from(t: CoreTransportType) -> Self {
        match t {
            CoreTransportType::Internet => TransportType::Internet,
            CoreTransportType::BLE => TransportType::Ble,
            CoreTransportType::WiFiDirect => TransportType::WiFiDirect,
            CoreTransportType::Reticulum => TransportType::Reticulum,
            CoreTransportType::Nostr => TransportType::Nostr,
        }
    }
}

impl From<TransportType> for CoreTransportType {
    fn from(t: TransportType) -> Self {
        match t {
            TransportType::Internet => CoreTransportType::Internet,
            TransportType::Ble => CoreTransportType::BLE,
            TransportType::WiFiDirect => CoreTransportType::WiFiDirect,
            TransportType::Reticulum => CoreTransportType::Reticulum,
            TransportType::Nostr => CoreTransportType::Nostr,
        }
    }
}

impl From<CoreTransportStatus> for TransportStatus {
    fn from(s: CoreTransportStatus) -> Self {
        match s {
            CoreTransportStatus::Available => TransportStatus::Available,
            CoreTransportStatus::Unavailable => TransportStatus::Unavailable,
            CoreTransportStatus::Connecting => TransportStatus::Connecting,
            CoreTransportStatus::Disconnected => TransportStatus::Disconnected,
            CoreTransportStatus::Error => TransportStatus::Error,
        }
    }
}

impl From<CoreRelayRole> for RelayRole {
    fn from(r: CoreRelayRole) -> Self {
        match r {
            CoreRelayRole::Regular => RelayRole::Regular,
            CoreRelayRole::Relay => RelayRole::Relay,
        }
    }
}

impl From<CoreRoutingPhase> for RoutingPhase {
    fn from(p: CoreRoutingPhase) -> Self {
        match p {
            CoreRoutingPhase::ScoreUpdated => RoutingPhase::ScoreUpdated,
            CoreRoutingPhase::Selected => RoutingPhase::Selected,
            CoreRoutingPhase::Switched => RoutingPhase::Switched,
            CoreRoutingPhase::Escalated => RoutingPhase::Escalated,
            // `RoutingPhase` is `#[non_exhaustive]` on the core side. Map
            // unrecognised variants to a dedicated `Unknown` so consumers
            // see the drift instead of a plausible-looking existing phase.
            // Warn once per process so operators notice a new-core /
            // old-FFI skew without spamming logs if the unknown variant
            // sits on a hot path (routing decisions fire on every send).
            other => {
                static WARN_ONCE: std::sync::Once = std::sync::Once::new();
                WARN_ONCE.call_once(|| {
                    tracing::warn!(
                        variant = ?other,
                        "telemetry: unknown CoreRoutingPhase variant; mapping to Unknown — FFI crate likely out of date (further occurrences suppressed)",
                    );
                });
                RoutingPhase::Unknown
            }
        }
    }
}

impl From<CoreRoutingReasonCode> for RoutingReasonCode {
    fn from(r: CoreRoutingReasonCode) -> Self {
        match r {
            CoreRoutingReasonCode::InitialSelection => RoutingReasonCode::InitialSelection,
            CoreRoutingReasonCode::PrimarySelected => RoutingReasonCode::PrimarySelected,
            CoreRoutingReasonCode::PrimarySuccess => RoutingReasonCode::PrimarySuccess,
            CoreRoutingReasonCode::FallbackSuccess => RoutingReasonCode::FallbackSuccess,
            CoreRoutingReasonCode::EscalationApplied => RoutingReasonCode::EscalationApplied,
            CoreRoutingReasonCode::CurrentUnavailable => RoutingReasonCode::CurrentUnavailable,
            CoreRoutingReasonCode::RetryThreshold => RoutingReasonCode::RetryThreshold,
            CoreRoutingReasonCode::PoorSignal => RoutingReasonCode::PoorSignal,
            CoreRoutingReasonCode::Congestion => RoutingReasonCode::Congestion,
            CoreRoutingReasonCode::LowTtl => RoutingReasonCode::LowTtl,
            CoreRoutingReasonCode::LowSuccessRate => RoutingReasonCode::LowSuccessRate,
            // Warn once per process — same rationale as `RoutingPhase`
            // above: reason codes ride along with every routing decision,
            // so a hot-path variant added to the core would otherwise
            // drown the tracing layer.
            other => {
                static WARN_ONCE: std::sync::Once = std::sync::Once::new();
                WARN_ONCE.call_once(|| {
                    tracing::warn!(
                        variant = ?other,
                        "telemetry: unknown CoreRoutingReasonCode variant; mapping to Unknown — FFI crate likely out of date (further occurrences suppressed)",
                    );
                });
                RoutingReasonCode::Unknown
            }
        }
    }
}

impl From<&CoreTransportMetrics> for TransportMetrics {
    fn from(m: &CoreTransportMetrics) -> Self {
        // Legacy six-field shape. The Rust `TransportMetrics` core does not
        // track packet or byte counters, so the four `packets_*` / `bytes_*`
        // fields are zero-filled. A previous pass set `packets_sent =
        // success_count + failure_count`, but that conflates per-message
        // send *attempts* with packet-level I/O and misleads dashboards;
        // the richer `success_count` / `failure_count` surface reaches
        // consumers through `delivery_ratio` / `drop_rate` / `error_rate`
        // below. `error_rate` and `avg_latency_ms` are derived from the
        // real Rust fields.
        let avg_latency_ms = m.latency_ms.unwrap_or(0);
        let error_rate = m.effective_drop_ratio().unwrap_or(0.0);
        TransportMetrics {
            packets_sent: 0,
            packets_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            error_rate,
            avg_latency_ms,
            rssi: m.rssi,
            bandwidth_bps: m.bandwidth_bps,
            congestion: Some(m.congestion),
            queue_depth: Some(m.queue_depth as u32),
            battery_level: m.battery_level,
            is_charging: Some(m.is_charging),
            relay_connection_count: Some(m.relay_connection_count),
            is_active_relay: Some(m.is_active_relay),
            delivery_ratio: m.delivery_ratio,
            drop_rate: m.drop_rate,
            average_hop_count: m.average_hop_count,
            energy_cost: m.energy_cost,
        }
    }
}

impl From<&CoreMetricsFrame> for MetricsFrame {
    fn from(f: &CoreMetricsFrame) -> Self {
        let transports = f
            .transports
            .iter()
            .map(|(t, m)| TransportMetricsEntry {
                transport: (*t).into(),
                metrics: TransportMetrics::from(m),
            })
            .collect();
        MetricsFrame {
            timestamp_ms: f.timestamp_ms,
            transports,
            retry_queue: RetryQueueStats {
                total_count: f.retry_queue.total_count as u64,
                ready_count: f.retry_queue.ready_count as u64,
                critical_priority_count: f.retry_queue.critical_priority_count as u64,
                high_priority_count: f.retry_queue.high_priority_count as u64,
                medium_priority_count: f.retry_queue.medium_priority_count as u64,
                low_priority_count: f.retry_queue.low_priority_count as u64,
            },
            dedup: DeduplicatorStatsFrame {
                total_tracked: f.dedup.total_tracked as u64,
                recent_tracked: f.dedup.recent_tracked as u64,
                capacity_used_percent: f.dedup.capacity_used_percent,
                false_positive_rate: f.dedup.false_positive_rate,
                // Explicit stable-string mapping. Do NOT switch to
                // `format!("{:?}", ...)` — that would tie the public FFI
                // wire format to the Rust `Debug` impl of
                // `DeduplicatorMode`, which is not a stability guarantee.
                mode: match f.dedup.mode {
                    CoreDeduplicatorMode::HashMap => "hashMap".to_string(),
                    CoreDeduplicatorMode::BloomFilter => "bloomFilter".to_string(),
                },
            },
            ack_pending: f.ack_pending as u64,
            neighbor_count: f.neighbor_count as u64,
            is_local_relay: f.is_local_relay,
            current_transport: f.current_transport.map(Into::into),
        }
    }
}

impl From<&CoreTransportStateEvent> for TransportStateEvent {
    fn from(e: &CoreTransportStateEvent) -> Self {
        TransportStateEvent {
            timestamp_ms: e.timestamp_ms,
            transport: e.transport.into(),
            previous: e.previous.into(),
            current: e.current.into(),
        }
    }
}

impl From<&CoreRoutingDecision> for RoutingDecision {
    fn from(d: &CoreRoutingDecision) -> Self {
        let scores = d
            .scores
            .iter()
            .map(|(t, s)| RoutingScoreEntry {
                transport: (*t).into(),
                signal: s.signal,
                proximity: s.proximity,
                bandwidth: s.bandwidth,
                congestion: s.congestion,
                energy: s.energy,
                reliability: s.reliability,
                load: s.load,
                total: s.total,
            })
            .collect();
        RoutingDecision {
            timestamp_ms: d.timestamp_ms,
            phase: d.phase.into(),
            from: d.from.map(Into::into),
            to: d.to.map(Into::into),
            winning_score: d.winning_score,
            reason_code: d.reason_code.map(Into::into),
            scores,
        }
    }
}

impl From<&CoreDeviceSnapshot> for DeviceCapabilitySnapshot {
    fn from(s: &CoreDeviceSnapshot) -> Self {
        DeviceCapabilitySnapshot {
            timestamp_ms: s.timestamp_ms,
            battery_level: s.battery_level,
            is_charging: s.is_charging,
            relay_role: s.relay_role.into(),
            changed_fields: s.changed_fields,
        }
    }
}

fn telemetry_config_into_core(cfg: TelemetryConfig) -> CoreTelemetryConfig {
    let mut core = CoreTelemetryConfig::default();
    if let Some(v) = cfg.scrub_ids {
        core = core.with_scrub_ids(v);
    }
    if let Some(v) = cfg.mls_verbosity {
        core = core.with_mls_verbosity(v.into());
    }
    // The foreign side passes `metrics_cadence_ms = None` to mean "accept
    // the default". Disabling periodic emission is a distinct wire value we
    // cannot currently express through the single `Option<u64>` field — if
    // that becomes needed, extend the UDL dict with a dedicated flag. For
    // now, any explicit value overrides the default; absence leaves it.
    if let Some(ms) = cfg.metrics_cadence_ms {
        core = core.with_metrics_cadence(Some(Duration::from_millis(ms)));
    }
    if let Some(v) = cfg.routing_diagnostic {
        core = core.with_routing_diagnostic(v);
    }
    if let Some(v) = cfg.mls_sampling_bypass {
        core = core.with_mls_sampling_bypass(v);
    }
    core
}

/// Bounded capacity for the telemetry poll queue. Sized for bursty emission
/// (per-send routing decisions + 5 s metrics snapshots + event stream);
/// overflow drops oldest.
const TELEMETRY_POLL_QUEUE_CAP: usize = 1024;

/// Adapts the foreign `TelemetrySink` trait to the core `TelemetrySink`
/// trait, and incidentally populates a bounded poll queue so apps that
/// prefer polling can pull records too.
///
/// Pull envelopes are the `TelemetryRecord` discriminated-union shape the
/// TypeScript layer expects: exactly one typed payload field next to
/// `category`. Each `push_*` helper builds the matching envelope, enqueues
/// it, and fires the typed foreign callback in one place so the two
/// channels cannot drift.
///
/// `poll_queue_enabled` is set from `TelemetryConfig::enable_poll_queue` at
/// install time. When `false`, the adapter short-circuits every pull-queue
/// code path *before* the `serde_json` serialization so push-only consumers
/// pay only the typed-callback cost.
struct TelemetrySinkAdapter {
    callback: Arc<dyn TelemetrySink>,
    queue: Arc<Mutex<VecDeque<String>>>,
    poll_queue_enabled: bool,
}

impl TelemetrySinkAdapter {
    fn enqueue(&self, envelope: String) {
        if !self.poll_queue_enabled {
            return;
        }
        let mut q = recover_mutex(&self.queue, "telemetry_queue");
        if q.len() >= TELEMETRY_POLL_QUEUE_CAP {
            q.pop_front();
        }
        q.push_back(envelope);
    }

    /// Serializes an envelope built from `(category, payload_key, payload)`
    /// pairs. Uses a `BTreeMap<&'static str, serde_json::Value>` so the
    /// caller picks the payload field name (`frame`, `event`, `decision`,
    /// etc.) that matches the TS `TelemetryRecord` union.
    fn envelope(category: &str, fields: &[(&str, serde_json::Value)]) -> String {
        let mut obj = serde_json::Map::with_capacity(fields.len() + 1);
        obj.insert(
            "category".to_string(),
            serde_json::Value::String(category.to_string()),
        );
        for (k, v) in fields {
            obj.insert((*k).to_string(), v.clone());
        }
        // `serde_json::to_string` on a `Value::Object` cannot fail for owned,
        // finite values — an `.expect` here would only fire on OOM, which we
        // cannot meaningfully recover from inside the telemetry path.
        serde_json::to_string(&serde_json::Value::Object(obj))
            .unwrap_or_else(|_| format!(r#"{{"category":"{}"}}"#, category))
    }

    /// Builds the extension-error envelope + the `payloadJson` string that
    /// the two failure helpers below share. Kept separate so the pull-queue
    /// envelope and the push-callback payload stay bit-identical.
    fn serialization_failure_parts(name: &str, err: &dyn std::fmt::Display) -> (String, String) {
        let payload_json = serde_json::json!({
            "telemetry_error": "serialization_failed",
            "record": name,
            "message": format!("{}", err),
        })
        .to_string();
        let envelope = Self::envelope(
            "extension",
            &[
                (
                    "name",
                    serde_json::Value::String(format!("telemetry.error.{name}")),
                ),
                (
                    "payloadJson",
                    serde_json::Value::String(payload_json.clone()),
                ),
            ],
        );
        (envelope, payload_json)
    }

    /// Pull-channel-only failure path. Used for the four typed-DTO
    /// variants (metrics frame, transport state, routing decision, device
    /// capability) where the push callback still fires with the owned
    /// DTO — we must not re-dispatch through `on_extension` or the
    /// consumer sees two records for one emit.
    fn enqueue_serialization_failure(&self, name: &str, err: &dyn std::fmt::Display) {
        tracing::warn!(
            record = name,
            error = %err,
            "telemetry: pull-queue serialization failed; enqueueing extension-error envelope (typed push already fired)",
        );
        // Callers already bail out of this helper when the pull queue is
        // disabled, so no flag check here. Keep that invariant — a future
        // caller that drops the guard would silently allocate.
        let (envelope, _payload_json) = Self::serialization_failure_parts(name, err);
        self.enqueue(envelope);
    }

    /// Last-resort path for the two JSON-string variants (`Protocol`,
    /// `Mls`) whose typed callback takes a pre-serialized string — if
    /// serialization fails there is no DTO to hand the typed callback.
    /// Enqueue the extension-error envelope AND fire `on_extension` so
    /// downstream observers still see exactly one record. When the pull
    /// queue is disabled, only the `on_extension` callback fires.
    fn emit_serialization_failure(&self, name: &str, err: &dyn std::fmt::Display) {
        tracing::warn!(
            record = name,
            error = %err,
            "telemetry: record serialization failed; dispatching via on_extension",
        );
        let payload_json = Self::serialization_failure_payload_json(name, err);
        if self.poll_queue_enabled {
            let envelope = Self::envelope(
                "extension",
                &[
                    (
                        "name",
                        serde_json::Value::String(format!("telemetry.error.{name}")),
                    ),
                    (
                        "payloadJson",
                        serde_json::Value::String(payload_json.clone()),
                    ),
                ],
            );
            self.enqueue(envelope);
        }
        self.callback
            .on_extension(format!("telemetry.error.{name}"), payload_json);
    }

    /// Just the `payloadJson` string for a serialization failure, split out
    /// from `serialization_failure_parts` so the push-only path can avoid
    /// building the full envelope when the pull queue is off.
    fn serialization_failure_payload_json(name: &str, err: &dyn std::fmt::Display) -> String {
        serde_json::json!({
            "telemetry_error": "serialization_failed",
            "record": name,
            "message": format!("{}", err),
        })
        .to_string()
    }

    /// Build-and-enqueue a rich-DTO envelope. No-op when the pull queue is
    /// disabled — avoids the `serde_json::to_value` + envelope allocation
    /// for push-only consumers. On serialization failure falls through to
    /// `enqueue_serialization_failure`, which is the pull-channel-only
    /// failure path (the typed push callback still fires in the caller).
    fn maybe_enqueue_rich<T: Serialize>(&self, category: &'static str, key: &'static str, dto: &T) {
        if !self.poll_queue_enabled {
            return;
        }
        match serde_json::to_value(dto) {
            Ok(value) => self.enqueue(Self::envelope(category, &[(key, value)])),
            Err(err) => self.enqueue_serialization_failure(category, &err),
        }
    }

    /// Build-and-enqueue a string-payload envelope (Protocol / Mls variants).
    /// No-op when the pull queue is disabled.
    fn maybe_enqueue_string_payload(&self, category: &'static str, key: &'static str, value: &str) {
        if !self.poll_queue_enabled {
            return;
        }
        self.enqueue(Self::envelope(
            category,
            &[(key, serde_json::Value::String(value.to_string()))],
        ));
    }

    /// Build-and-enqueue the forward-compat `extension` envelope.
    /// No-op when the pull queue is disabled.
    fn maybe_enqueue_extension(&self, name: &str, payload_json: &str) {
        if !self.poll_queue_enabled {
            return;
        }
        self.enqueue(Self::envelope(
            "extension",
            &[
                ("name", serde_json::Value::String(name.to_string())),
                (
                    "payloadJson",
                    serde_json::Value::String(payload_json.to_string()),
                ),
            ],
        ));
    }
}

impl CoreTelemetrySink for TelemetrySinkAdapter {
    fn emit(&self, record: &CoreTelemetryRecord) {
        match record {
            CoreTelemetryRecord::Protocol(ev) => match ev.to_json() {
                Ok(json) => {
                    self.maybe_enqueue_string_payload("protocol", "eventJson", &json);
                    self.callback.on_protocol_event(json);
                }
                Err(err) => self.emit_serialization_failure("protocol", &err),
            },
            CoreTelemetryRecord::Mls(ev) => match serde_json::to_string(ev) {
                Ok(json) => {
                    self.maybe_enqueue_string_payload("mls", "eventJson", &json);
                    self.callback.on_mls_event(json);
                }
                Err(err) => self.emit_serialization_failure("mls", &err),
            },
            // Rich-variant arms: the typed push callback takes an owned
            // Rust DTO that cannot fail to cross the FFI boundary, so it
            // fires unconditionally. Pull-queue JSON serialization is the
            // only thing that can fail (e.g. NaN floats); on failure the
            // queue gets an extension-error envelope under the variant's
            // `telemetry.error.<name>` key and the push callback is NOT
            // re-fired through `on_extension`. When the pull queue is
            // disabled (opt-in via `enable_poll_queue=false`), the
            // `maybe_enqueue_rich` helper short-circuits *before* the
            // `serde_json::to_value` call, so push-only consumers pay no
            // serialization cost.
            CoreTelemetryRecord::MetricsSnapshot(frame) => {
                let dto = MetricsFrame::from(frame.as_ref());
                self.maybe_enqueue_rich("metricsFrame", "frame", &dto);
                self.callback.on_metrics_frame(dto);
            }
            CoreTelemetryRecord::TransportState(event) => {
                let dto = TransportStateEvent::from(event);
                self.maybe_enqueue_rich("transportState", "event", &dto);
                self.callback.on_transport_state(dto);
            }
            CoreTelemetryRecord::Routing(decision) => {
                let dto = RoutingDecision::from(decision.as_ref());
                self.maybe_enqueue_rich("routingDecision", "decision", &dto);
                self.callback.on_routing_decision(dto);
            }
            CoreTelemetryRecord::Device(snapshot) => {
                let dto = DeviceCapabilitySnapshot::from(snapshot);
                self.maybe_enqueue_rich("deviceCapability", "snapshot", &dto);
                self.callback.on_device_capability(dto);
            }
            // Forward-compat: any variant added to CoreTelemetryRecord after
            // this code was written lands here with its stable name. The
            // inner payload is not `Serialize` (`TelemetryRecord` docstring),
            // so today we emit `{}`. New variants should be given their own
            // arm in a follow-up before they ship to avoid losing data here;
            // the `forward_compat_extension_coverage` test pins the set of
            // known variants so drift fails fast.
            other => {
                let name = other.name().to_string();
                let payload_json = "{}".to_string();
                self.maybe_enqueue_extension(&name, &payload_json);
                self.callback.on_extension(name, payload_json);
            }
        }
    }
}

/// Content type for messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Text,
    Image,
    Video,
    Audio,
    VoiceNote,
    VideoNote,
    File,
    FileChunk,
    Poll,
}

impl From<ContentType> for CoreContentType {
    fn from(ct: ContentType) -> Self {
        match ct {
            ContentType::Text => CoreContentType::Text,
            ContentType::Image => CoreContentType::Image,
            ContentType::Video => CoreContentType::Video,
            ContentType::Audio => CoreContentType::Audio,
            ContentType::VoiceNote => CoreContentType::VoiceNote,
            ContentType::VideoNote => CoreContentType::VideoNote,
            ContentType::File => CoreContentType::File,
            ContentType::FileChunk => CoreContentType::FileChunk,
            ContentType::Poll => CoreContentType::Poll,
        }
    }
}

/// Media metadata for attachments.
#[derive(Debug, Clone)]
pub struct MediaMetadata {
    pub mime_type: String,
    pub file_name: String,
    pub file_size: u64,
    pub duration_ms: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub thumbnail_base64: Option<String>,
    pub media_id: Option<String>,
    pub download_url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub encryption_key: Option<String>,
    pub iv: Option<String>,
    pub ciphertext_hash: Option<String>,
    pub sticker_provider: Option<String>,
    pub sticker_remote_id: Option<String>,
    pub sticker_kind: Option<String>,
}

impl From<MediaMetadata> for CoreMediaMetadata {
    fn from(m: MediaMetadata) -> Self {
        CoreMediaMetadata {
            mime_type: m.mime_type,
            file_name: m.file_name,
            file_size: m.file_size,
            duration_ms: m.duration_ms,
            width: m.width,
            height: m.height,
            thumbnail_base64: m.thumbnail_base64,
            media_id: m.media_id,
            download_url: m.download_url,
            thumbnail_url: m.thumbnail_url,
            encryption_key: m.encryption_key,
            iv: m.iv,
            ciphertext_hash: m.ciphertext_hash,
            sticker_provider: m.sticker_provider,
            sticker_remote_id: m.sticker_remote_id,
            sticker_kind: m.sticker_kind,
        }
    }
}

/// Quoted-reply context: an unverified display-level hint mirroring the core
/// `ReplyContext`. Converted to the validated core type by
/// `send_message_rich` (an invalid `sender` fails the whole call).
#[derive(Debug, Clone)]
pub struct ReplyContext {
    pub sender: String,
    pub text: String,
    pub timestamp: Option<i64>,
    pub reply_media_label: Option<String>,
    pub reply_content_type: Option<String>,
}

impl TryFrom<ReplyContext> for CoreReplyContext {
    type Error = ProtocolError;

    fn try_from(rc: ReplyContext) -> Result<Self, Self::Error> {
        Ok(CoreReplyContext {
            sender: CoreUserId::new(&rc.sender).map_err(|e| {
                ProtocolError::InvalidArgument(format!("Invalid reply_context.sender: {}", e))
            })?,
            text: rc.text,
            timestamp: rc.timestamp.map(CoreTimestamp::from_millis),
            reply_media_label: rc.reply_media_label,
            reply_content_type: rc.reply_content_type,
        })
    }
}

/// Forwarding attribution: an unverified display-level hint mirroring the
/// core `ForwardInfo`.
#[derive(Debug, Clone)]
pub struct ForwardInfo {
    pub original_sender: String,
    pub original_message_id: String,
    pub original_timestamp: i64,
    pub forward_count: u32,
}

impl TryFrom<ForwardInfo> for CoreForwardInfo {
    type Error = ProtocolError;

    fn try_from(fi: ForwardInfo) -> Result<Self, Self::Error> {
        Ok(CoreForwardInfo {
            original_sender: CoreUserId::new(&fi.original_sender).map_err(|e| {
                ProtocolError::InvalidArgument(format!(
                    "Invalid forward_info.original_sender: {}",
                    e
                ))
            })?,
            original_message_id: CoreMessageId::from_str(&fi.original_message_id).map_err(|e| {
                ProtocolError::InvalidArgument(format!(
                    "Invalid forward_info.original_message_id: {}",
                    e
                ))
            })?,
            original_timestamp: CoreTimestamp::from_millis(fi.original_timestamp),
            forward_count: fi.forward_count,
        })
    }
}

/// Options for `send_message_rich`, mirroring the core `SendMessageOptions`
/// (minus `via_transport`, which the wrapper derives from the session's
/// forced-transport setting).
#[derive(Debug, Clone)]
pub struct SendMessageOptions {
    pub priority: Option<MessagePriority>,
    pub reply_to_msg: Option<String>,
    pub content_type: Option<ContentType>,
    pub reply_context: Option<ReplyContext>,
    pub media_metadata: Option<MediaMetadata>,
    pub forward_info: Option<ForwardInfo>,
}

/// Options for `send_media_rich`, mirroring the core `MediaSendOptions`.
#[derive(Debug, Clone)]
pub struct MediaSendOptions {
    pub media_metadata: Option<MediaMetadata>,
    pub caption: Option<String>,
    pub reply_to_msg: Option<String>,
    pub reply_context: Option<ReplyContext>,
    pub forward_info: Option<ForwardInfo>,
    pub file_id: Option<String>,
}

/// File transfer progress
#[derive(Debug, Clone)]
pub struct FileProgress {
    pub file_id: String,
    pub chunks_sent: u32,
    pub total_chunks: u32,
    pub percentage: u8,
}

/// Message delivery statistics
#[derive(Debug, Clone)]
pub struct MessageStats {
    pub message_id: String,
    pub sent_at_ms: u64,
    pub delivered_at_ms: Option<u64>,
    pub hop_count: u8,
    pub status: String,
}

/// Network topology node
#[derive(Debug, Clone)]
pub struct NetworkNode {
    pub node_id: String,
    pub role: String,
    pub rssi: Option<i16>,
    pub last_seen_ms: u64,
}

/// Network topology link
#[derive(Debug, Clone)]
pub struct NetworkLink {
    pub source_id: String,
    pub target_id: String,
    pub transport: String,
    pub quality: f32,
}

/// Network topology
#[derive(Debug, Clone)]
pub struct NetworkTopology {
    pub nodes: Vec<NetworkNode>,
    pub links: Vec<NetworkLink>,
    pub message_stats: Vec<MessageStats>,
}

// ========================================================================
// MLS TYPES
// ========================================================================

/// Key package bundle for distribution
#[derive(Debug, Clone)]
pub struct MlsKeyPackageBundle {
    pub package_id: String,
    pub user_id: String,
    pub key_package_data: Vec<u8>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub synced: bool,
}

impl From<CoreKeyPackageBundle> for MlsKeyPackageBundle {
    fn from(bundle: CoreKeyPackageBundle) -> Self {
        Self {
            package_id: bundle.package_id,
            user_id: bundle.user_id,
            key_package_data: bundle.key_package_data,
            created_at_ms: bundle.created_at_ms,
            expires_at_ms: bundle.expires_at_ms,
            synced: bundle.synced,
        }
    }
}

/// Welcome message for inviting users to a group
#[derive(Debug, Clone)]
pub struct MlsWelcomeMessage {
    pub group_id: String,
    pub welcome_data: Vec<u8>,
    pub inviter_id: String,
    pub group_name: Option<String>,
    pub timestamp_ms: u64,
}

impl From<CoreWelcomeMessage> for MlsWelcomeMessage {
    fn from(msg: CoreWelcomeMessage) -> Self {
        Self {
            group_id: msg.group_id.as_str().to_string(),
            welcome_data: msg.welcome_data,
            inviter_id: msg.inviter_id,
            group_name: msg.group_name,
            timestamp_ms: msg.timestamp_ms,
        }
    }
}

impl TryFrom<MlsWelcomeMessage> for CoreWelcomeMessage {
    type Error = offline_protocol_mls::MlsError;

    fn try_from(msg: MlsWelcomeMessage) -> Result<Self, Self::Error> {
        Ok(Self {
            group_id: CoreGroupId::new(msg.group_id)?,
            welcome_data: msg.welcome_data,
            inviter_id: msg.inviter_id,
            group_name: msg.group_name,
            timestamp_ms: msg.timestamp_ms,
        })
    }
}

/// Encrypted message for transport
#[derive(Debug, Clone)]
pub struct MlsEncryptedMessage {
    pub group_id: String,
    pub message_type: String,
    pub epoch: u64,
    pub ciphertext: Vec<u8>,
    pub sender_id: String,
    pub timestamp_ms: u64,
}

impl From<CoreEncryptedMessage> for MlsEncryptedMessage {
    fn from(msg: CoreEncryptedMessage) -> Self {
        Self {
            group_id: msg.group_id.as_str().to_string(),
            message_type: msg.message_type.as_str().to_string(),
            epoch: msg.epoch,
            ciphertext: msg.ciphertext,
            sender_id: msg.sender_id,
            timestamp_ms: msg.timestamp_ms,
        }
    }
}

impl TryFrom<MlsEncryptedMessage> for CoreEncryptedMessage {
    type Error = offline_protocol_mls::MlsError;

    fn try_from(msg: MlsEncryptedMessage) -> Result<Self, Self::Error> {
        use offline_protocol_mls::MlsMessageType;
        let message_type =
            MlsMessageType::from_str_opt(&msg.message_type).unwrap_or(MlsMessageType::Application);
        Ok(Self {
            group_id: CoreGroupId::new(msg.group_id)?,
            message_type,
            epoch: msg.epoch,
            ciphertext: msg.ciphertext,
            sender_id: msg.sender_id,
            timestamp_ms: msg.timestamp_ms,
        })
    }
}

/// Result of adding a member to an MLS group.
///
/// Contains both the Welcome message (to be sent to the invitee) and the
/// Commit message (to be distributed to all existing group members so they
/// can advance their MLS epoch).
#[derive(Debug, Clone)]
pub struct MlsAddMemberResult {
    pub welcome: MlsWelcomeMessage,
    pub commit: MlsEncryptedMessage,
}

/// Group information
#[derive(Debug, Clone)]
pub struct MlsGroupInfo {
    pub group_id: String,
    pub name: Option<String>,
    pub members: Vec<String>,
    pub epoch: u64,
    pub is_session: bool,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
}

impl From<CoreGroupInfo> for MlsGroupInfo {
    fn from(info: CoreGroupInfo) -> Self {
        Self {
            group_id: info.group_id.as_str().to_string(),
            name: info.name,
            members: info.members,
            epoch: info.epoch,
            is_session: info.is_session,
            created_at_ms: info.created_at_ms,
            last_activity_ms: info.last_activity_ms,
        }
    }
}

/// Whether a rich group send right now would seal its extras, and which
/// members are in the way. Point-in-time and advisory: the send path
/// re-evaluates the gate itself.
#[derive(Debug, Clone)]
pub struct GroupRichReadiness {
    pub ready: bool,
    pub unknown_members: Vec<String>,
}

impl From<offline_protocol::GroupRichReadiness> for GroupRichReadiness {
    fn from(r: offline_protocol::GroupRichReadiness) -> Self {
        Self {
            ready: r.ready,
            unknown_members: r.unknown_members,
        }
    }
}

/// Relay-side registration state of a group (point-in-time; transitions
/// surface as the `group_relay_sync_changed` event).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaySyncState {
    Synced,
    Pending,
    Unsynced,
}

impl From<offline_protocol::RelaySyncState> for RelaySyncState {
    fn from(s: offline_protocol::RelaySyncState) -> Self {
        match s {
            offline_protocol::RelaySyncState::Synced => Self::Synced,
            offline_protocol::RelaySyncState::Pending => Self::Pending,
            offline_protocol::RelaySyncState::Unsynced => Self::Unsynced,
        }
    }
}

/// DORS configuration
#[derive(Debug, Clone)]
pub struct DorsConfig {
    pub prefer_online: bool,
    pub switch_hysteresis: f32,
    pub switch_cooldown_secs: u64,
    pub ble_to_wifi_retry_threshold: u32,
    pub min_success_rate_before_escalation: f32,
    pub min_ble_samples_before_success_rate_escalation: u64,
    pub rssi_switch_threshold: i16,
    pub congestion_queue_threshold: u64,
    pub stability_window_secs: u64,
    pub poor_signal_duration_secs: u64,
    pub ttl_escalation_threshold: u8,
    pub congestion_duration_secs: u64,
    pub ttl_escalation_hold_secs: u64,
    pub history_window_size: u64,
    pub queue_recovery_ratio: f32,
}

/// ACK configuration
#[derive(Debug, Clone)]
pub struct AckConfig {
    pub default_timeout_ms: u64,
    pub max_pending_acks: u64,
}

/// Retry configuration
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f32,
    pub outbox_max_lifetime_ms: u64,
    pub pending_message_max_lifetime_ms: u64,
}

/// Deduplication configuration
#[derive(Debug, Clone)]
pub struct DedupConfig {
    pub max_tracked_messages: u64,
    pub retention_time_secs: u64,
}

/// Deduplicator statistics for monitoring
#[derive(Debug, Clone)]
pub struct DedupStats {
    pub total_tracked: u64,
    pub recent_tracked: u64,
    pub capacity_used_percent: u8,
    pub mode: String,
}

/// Reliability configuration
#[derive(Debug, Clone)]
pub struct ReliabilityConfig {
    pub ack: AckConfig,
    pub retry: RetryConfig,
    pub dedup: DedupConfig,
}

/// Path selection configuration
#[derive(Debug, Clone)]
pub struct PathConfig {
    pub forward_to_top_k: u32,
    pub max_congestion_level: u32,
}

/// Gradient routing table entry - represents a learned route to a destination
#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub next_hop: String,
    pub hop_count: u8,
    pub quality: f32,
    pub last_seen_ms: u64,
}

/// Gradient routing configuration
#[derive(Debug, Clone)]
pub struct GradientRoutingConfig {
    pub max_routes_per_destination: u32,
    pub route_ttl_secs: u64,
    pub max_routing_table_size: u32,
}

/// Routing table statistics for monitoring
#[derive(Debug, Clone)]
pub struct RoutingStats {
    pub destination_count: u32,
    pub route_count: u32,
}

/// Relay configuration
#[derive(Debug, Clone)]
pub struct RelayConfig {
    pub relay_threshold: u64,
    pub min_battery_for_relay: u8,
    pub allow_relay: bool,
    pub relay_priority: RelayPriority,
}

/// Transport configuration
#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub ble_enabled: bool,
    pub wifi_direct_enabled: bool,
    pub internet_enabled: bool,
    pub reticulum_enabled: bool,
    pub nostr_enabled: bool,
    /// See [`ProtocolConfig::nostr_sealing_enabled`].
    pub nostr_sealing_enabled: bool,
}

/// Encryption configuration for automatic MLS handling
#[derive(Debug, Clone)]
pub struct EncryptionConfig {
    /// Whether automatic encryption is enabled (default: true)
    pub enabled: bool,
    /// Auto-exchange key packages on peer discovery (default: true)
    pub auto_key_exchange: bool,
    /// Store pending messages when no session exists (default: true)
    pub store_pending: bool,
    /// Require encryption for outbound sends (default: true — sends fail
    /// closed instead of silently degrading to plaintext; set to false to
    /// explicitly opt in to plaintext operation)
    pub require_encryption: bool,
    /// Pending queue configuration for encrypted pre-session messages.
    pub pending_queue: PendingQueueConfig,
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_key_exchange: true,
            store_pending: true,
            require_encryption: true,
            pending_queue: PendingQueueConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub enum OverflowPolicy {
    #[default]
    DropOldest,
    DropNewest,
}

#[derive(Debug, Clone)]
pub struct PendingQueueConfig {
    pub max_pending_per_peer: u64,
    pub max_pending_global: u64,
    pub pending_ttl_ms: u64,
    pub overflow_policy: OverflowPolicy,
}

impl Default for PendingQueueConfig {
    fn default() -> Self {
        Self {
            max_pending_per_peer: 64,
            max_pending_global: 4096,
            // Mirrors the core default (30 min); see DEFAULT_PENDING_TTL_MS in
            // offline-protocol/src/config.rs for the deferred-ACK rationale.
            pending_ttl_ms: DEFAULT_PENDING_TTL_MS,
            overflow_policy: OverflowPolicy::DropOldest,
        }
    }
}

/// Protocol configuration (simplified)
#[derive(Debug, Clone)]
pub struct ProtocolConfig {
    pub app_id: String,
    pub user_id: String,
    pub ble_enabled: bool,
    pub wifi_direct_enabled: bool,
    pub internet_enabled: bool,
    pub reticulum_enabled: bool,
    pub nostr_enabled: bool,
    pub prefer_online: bool,
    pub initial_ttl: u8,
    pub encryption_enabled: bool,
    pub auto_key_exchange: bool,
    pub store_pending: bool,
    pub require_encryption: bool,
    pub max_pending_per_peer: u64,
    pub max_pending_global: u64,
    pub pending_ttl_ms: u64,
    pub overflow_policy: OverflowPolicy,
    pub max_group_members: u32,
    pub group_relay_enabled: bool,
    /// Whether a relay-synced group may send one O(1) relay broadcast instead
    /// of per-member fan-out (default on, additionally gated on the relay's
    /// `group_delivery_v2` capability). See the UDL dictionary and
    /// `GroupConfig::relay_broadcast_enabled` for why the default is safe.
    pub group_relay_broadcast_enabled: bool,
    /// Whether an unauthorized MLS membership commit is refused rather than
    /// applied-and-reported (default off). See the UDL dictionary and
    /// `GroupConfig::enforce_admin_commits` — enabling this risks permanently
    /// forking members whose admin overlay disagrees.
    pub group_enforce_admin_commits: bool,
    pub require_transport_identity: bool,
    /// Kill switch for the compact binary wire codec (default on). See the UDL
    /// dictionary and `TransportConfig::binary_wire_enabled` for semantics.
    pub binary_wire_enabled: bool,
    /// Kill switch for sealing outgoing Nostr frames into NIP-59 gift wraps
    /// (default on). See the UDL dictionary and
    /// `TransportConfig::nostr_sealing_enabled` — unlike the negotiated
    /// switches, this one is safe to flip on a single device.
    pub nostr_sealing_enabled: bool,
    /// Kill switch for the compact MLS envelope (default on). See the UDL
    /// dictionary and `EncryptionConfig::compact_envelope_enabled` for
    /// semantics.
    pub compact_envelope_enabled: bool,
    /// Kill switch for the sealed rich payload (default on). See the UDL
    /// dictionary and `EncryptionConfig::rich_payload_enabled` for
    /// semantics.
    pub rich_payload_enabled: bool,
    /// Kill switch for 1:1 MLS crypto-failure recovery (default on). See the UDL
    /// dictionary and `EncryptionConfig::crypto_recovery_enabled` for semantics.
    pub crypto_recovery_enabled: bool,
}

/// Extended protocol configuration with all options
#[derive(Debug, Clone)]
pub struct ProtocolConfigExtended {
    pub app_id: String,
    pub user_id: String,
    pub transport: TransportConfig,
    pub dors: DorsConfig,
    pub relay: RelayConfig,
    pub path: PathConfig,
    pub reliability: ReliabilityConfig,
    pub initial_ttl: u8,
}

impl From<ProtocolConfig> for CoreConfig {
    fn from(config: ProtocolConfig) -> Self {
        let mut core_config = CoreConfig::new(config.app_id, config.user_id);
        core_config.transport.ble_enabled = config.ble_enabled;
        core_config.transport.wifi_direct_enabled = config.wifi_direct_enabled;
        core_config.transport.internet_enabled = config.internet_enabled;
        core_config.transport.reticulum_enabled = config.reticulum_enabled;
        core_config.transport.nostr_enabled = config.nostr_enabled;
        core_config.transport.nostr_sealing_enabled = config.nostr_sealing_enabled;
        core_config.transport.binary_wire_enabled = config.binary_wire_enabled;
        core_config.encryption.compact_envelope_enabled = config.compact_envelope_enabled;
        core_config.encryption.rich_payload_enabled = config.rich_payload_enabled;
        core_config.encryption.crypto_recovery_enabled = config.crypto_recovery_enabled;
        core_config.dors.prefer_online = config.prefer_online;
        core_config.initial_ttl = config.initial_ttl;
        core_config.encryption.enabled = config.encryption_enabled;
        core_config.encryption.auto_key_exchange = config.auto_key_exchange;
        core_config.encryption.store_pending = config.store_pending;
        core_config.encryption.require_encryption = config.require_encryption;
        core_config.encryption.pending_queue = CorePendingQueueConfig {
            max_pending_per_peer: config.max_pending_per_peer as usize,
            max_pending_global: config.max_pending_global as usize,
            pending_ttl_ms: config.pending_ttl_ms,
            overflow_policy: match config.overflow_policy {
                OverflowPolicy::DropOldest => CoreOverflowPolicy::DropOldest,
                OverflowPolicy::DropNewest => CoreOverflowPolicy::DropNewest,
            },
            // Byte budgets are not FFI-exposed; core defaults apply.
            ..CorePendingQueueConfig::default()
        };
        core_config.group.max_group_members = config.max_group_members as usize;
        core_config.group.relay_enabled = config.group_relay_enabled;
        core_config.group.relay_broadcast_enabled = config.group_relay_broadcast_enabled;
        core_config.group.enforce_admin_commits = config.group_enforce_admin_commits;
        core_config.security.require_transport_identity = config.require_transport_identity;
        core_config
    }
}

/// Event callback trait
pub trait EventCallback: Send + Sync {
    fn on_event(&self, event_json: String);
}

/// Unified telemetry sink — foreign counterpart of
/// `offline_protocol::TelemetrySink`. Dispatches are synchronous; see the
/// UDL-level comment on `callback interface TelemetrySink` for the thread
/// and locking contract.
///
/// # Failure semantics
///
/// The four typed-DTO variants (`on_metrics_frame`, `on_transport_state`,
/// `on_routing_decision`, `on_device_capability`) always fire — the DTOs
/// are plain Rust structs crossing the FFI boundary and nothing between
/// `emit` and the foreign call site can fail. If the pull-queue JSON
/// envelope fails to serialize (e.g. a NaN float slipped through), the
/// queue receives an `extension` envelope named
/// `telemetry.error.<variant>`; the typed push callback is NOT re-fired
/// through `on_extension`.
///
/// The two JSON-string variants (`on_protocol_event`, `on_mls_event`) can
/// only fire when the inner event serializes. On serialization failure
/// the typed callback is skipped *and* `on_extension` fires with
/// `telemetry.error.<variant>` so consumers see exactly one record per
/// emit regardless of outcome.
pub trait TelemetrySink: Send + Sync {
    fn on_protocol_event(&self, event_json: String);
    fn on_mls_event(&self, event_json: String);
    fn on_metrics_frame(&self, frame: MetricsFrame);
    fn on_transport_state(&self, event: TransportStateEvent);
    fn on_routing_decision(&self, decision: RoutingDecision);
    fn on_device_capability(&self, snapshot: DeviceCapabilitySnapshot);
    fn on_extension(&self, name: String, payload_json: String);
}

/// BLE transport callback trait — notifies platform when outgoing fragments are available.
/// Replaces timer-based polling with event-driven sending.
pub trait BleTransportCallback: Send + Sync {
    fn on_fragments_available(&self);
}

/// WiFi Direct transport callback trait — notifies platform when outgoing messages are available.
pub trait WifiDirectTransportCallback: Send + Sync {
    fn on_messages_available(&self);
}

/// Reticulum transport callback trait — notifies platform when outgoing messages are available.
pub trait ReticulumTransportCallback: Send + Sync {
    fn on_messages_available(&self);
}

/// Nostr transport callback trait — notifies platform when outgoing messages are available.
pub trait NostrTransportCallback: Send + Sync {
    fn on_messages_available(&self);
}

/// BLE fragment for outgoing data
#[derive(Debug, Clone)]
pub struct BleFragment {
    pub recipient_id: String,
    pub data: Vec<u8>,
}

/// Internet message for outgoing data
#[derive(Debug, Clone)]
pub struct InternetMessage {
    /// Unique message identifier. Use this with `internet_confirm_sent()` or
    /// `internet_send_failed()`/`internet_send_failed_with_reason()` to report
    /// the send outcome.
    pub message_id: String,
    pub recipient_id: String,
    pub data: Vec<u8>,
    pub reply_to_msg: Option<String>,
    /// Server-plane control op the bridge must translate to a relay-native
    /// message (see `OfflineProtocol::internet_control_op`); None = normal
    /// traffic, send verbatim as `SendMessage`.
    pub control_op: Option<String>,
    /// JSON payload after the internal prefix (empty for payload-less ops).
    pub control_payload: Option<String>,
}

/// WiFi Direct message for outgoing data
#[derive(Debug, Clone)]
pub struct WifiDirectMessage {
    pub recipient_id: String,
    pub data: Vec<u8>,
}

/// Reticulum message for outgoing data
#[derive(Debug, Clone)]
pub struct ReticulumMessage {
    /// Unique message identifier. Use this with `reticulum_confirm_sent()` or
    /// `reticulum_send_failed()`/`reticulum_send_failed_with_reason()` to report
    /// the send outcome.
    pub message_id: String,
    pub recipient_id: String,
    pub data: Vec<u8>,
    pub reply_to_msg: Option<String>,
}

/// Nostr message for outgoing data.
///
/// The `event_json` field contains a complete, pre-signed `["EVENT", {...}]`
/// string. The platform should send it directly over the relay WebSocket.
///
/// Use `event_id` to correlate relay `["OK", event_id, accepted, reason]`
/// responses: on acceptance call `nostr_confirm_sent(message_id)`, on
/// rejection call `nostr_send_failed_with_reason(message_id, reason)`.
#[derive(Debug, Clone)]
pub struct NostrMessage {
    /// Unique message identifier. Use this with `nostr_confirm_sent()` or
    /// `nostr_send_failed()`/`nostr_send_failed_with_reason()` to report
    /// the send outcome.
    pub message_id: String,
    /// Nostr event ID (64-char hex SHA-256). Use this to match relay
    /// `["OK", event_id, ...]` responses back to this message.
    pub event_id: String,
    /// Complete signed Nostr event JSON: `["EVENT", {...}]`.
    /// The platform should send this string directly over the WebSocket.
    pub event_json: String,
}

/// Internal state for BLE operations
struct BleState {
    fragments: VecDeque<(String, Vec<u8>)>,
    peer_count: u32,
    peers: HashMap<String, PeerDevice>,
}

/// Internal state for Internet transport operations
struct InternetState {
    /// Whether internet transport is connected
    is_connected: bool,
}

/// Internal state for WiFi Direct transport operations
struct WifiDirectState {
    /// Whether WiFi Direct is connected to a peer group
    is_connected: bool,
    /// Peer device address (if connected)
    connected_peer: Option<String>,
}

/// Internal state for Reticulum transport operations
struct ReticulumState {
    /// Whether Reticulum transport is connected
    is_connected: bool,
}

/// Internal state for Nostr transport operations
struct NostrState {
    /// Whether Nostr transport is connected to relays
    is_connected: bool,
}

/// Main protocol wrapper for UniFFI - COMPLETE IMPLEMENTATION
pub struct OfflineProtocol {
    inner: Mutex<CoreProtocol>,
    state: RwLock<ProtocolState>,
    event_callback: Arc<RwLock<Option<Arc<dyn EventCallback>>>>,
    event_queue: Arc<Mutex<VecDeque<String>>>,
    telemetry_queue: Arc<Mutex<VecDeque<String>>>,
    ble_state: Mutex<BleState>,
    internet_state: Mutex<InternetState>,
    wifi_direct_state: Mutex<WifiDirectState>,
    reticulum_state: Mutex<ReticulumState>,
    nostr_state: Mutex<NostrState>,
    visualizer: Mutex<NetworkVisualizer>,
    path_selector: Mutex<PathSelector>,
    battery_level: RwLock<Option<u8>>,
    relay_priority: RwLock<RelayPriority>,
    forced_transport: RwLock<Option<TransportType>>,
    dors_config: RwLock<Option<DorsConfig>>,
}

impl OfflineProtocol {
    /// Creates a new protocol instance
    pub fn new(config: ProtocolConfig) -> Result<Self, ProtocolError> {
        let user_id = config.user_id.clone();
        let ble_enabled = config.ble_enabled;
        let internet_enabled = config.internet_enabled;
        let reticulum_enabled = config.reticulum_enabled;
        let nostr_enabled = config.nostr_enabled;
        let core_config: CoreConfig = config.into();
        core_config.validate().map_err(ProtocolError::from)?;

        let mut protocol = CoreProtocol::new(core_config).map_err(ProtocolError::from)?;

        // Add BLE transport if enabled
        // The transport manager owns the transport, and we'll access it through there
        if ble_enabled {
            let ble_transport = BleTransport::new(user_id.clone());
            protocol
                .transport_manager_mut()
                .add_transport(CoreTransportType::BLE, Box::new(ble_transport));
        }

        // Add Internet transport if enabled
        // The platform code (iOS/Android) will manage the actual WebSocket connection
        // and call internetStatusChanged when connected/disconnected
        if internet_enabled {
            let internet_transport = InternetTransport::new(user_id.clone());
            protocol
                .transport_manager_mut()
                .add_transport(CoreTransportType::Internet, Box::new(internet_transport));
        }

        // Add Reticulum transport if enabled
        // The platform bridges to a running Reticulum daemon or RNode hardware
        if reticulum_enabled {
            let reticulum_transport = ReticulumTransport::new(user_id.clone());
            protocol
                .transport_manager_mut()
                .add_transport(CoreTransportType::Reticulum, Box::new(reticulum_transport));
        }

        // Add Nostr transport if enabled
        // The platform bridges to Nostr relay WebSocket connections
        if nostr_enabled {
            let nostr_transport = NostrTransport::new(user_id.clone()).map_err(|e| {
                ProtocolError::InvalidConfiguration(format!(
                    "Failed to create Nostr keypair: {}",
                    e
                ))
            })?;
            protocol
                .transport_manager_mut()
                .add_transport(CoreTransportType::Nostr, Box::new(nostr_transport));
        }

        // Create the event queue and callback that will be shared with the event handler
        let event_queue = Arc::new(Mutex::new(VecDeque::new()));
        let event_queue_clone = event_queue.clone();
        let event_callback = Arc::new(RwLock::new(None::<Arc<dyn EventCallback>>));
        let event_callback_clone = event_callback.clone();

        // Register event handler with core protocol to forward all events
        // This bridges events from the core protocol to JavaScript
        protocol.on_event(move |event| {
            // Convert event to JSON
            if let Ok(event_json) = event.to_json() {
                // Clone callback Arc outside the lock to avoid holding the
                // RwLock during callback invocation (prevents deadlock if the
                // callback re-enters the protocol).
                let callback_arc = recover_rwlock_read(&event_callback_clone, "event_callback")
                    .as_ref()
                    .cloned();
                if let Some(callback) = callback_arc {
                    callback.on_event(event_json.clone());
                }

                // Add to event queue for polling
                let mut queue = recover_mutex(&event_queue_clone, "event_queue");
                queue.push_back(event_json);

                // Limit queue size to prevent memory issues
                if queue.len() > 1000 {
                    queue.pop_front();
                }
            }
        });

        Ok(Self {
            inner: Mutex::new(protocol),
            state: RwLock::new(ProtocolState::Stopped),
            event_callback,
            event_queue,
            telemetry_queue: Arc::new(Mutex::new(VecDeque::new())),
            ble_state: Mutex::new(BleState {
                fragments: VecDeque::new(),
                peer_count: 0,
                peers: HashMap::new(),
            }),
            internet_state: Mutex::new(InternetState {
                is_connected: false,
            }),
            wifi_direct_state: Mutex::new(WifiDirectState {
                is_connected: false,
                connected_peer: None,
            }),
            reticulum_state: Mutex::new(ReticulumState {
                is_connected: false,
            }),
            nostr_state: Mutex::new(NostrState {
                is_connected: false,
            }),
            visualizer: Mutex::new(NetworkVisualizer::new(user_id.clone())),
            path_selector: Mutex::new(PathSelector::new()),
            battery_level: RwLock::new(None),
            relay_priority: RwLock::new(RelayPriority::Medium),
            forced_transport: RwLock::new(None),
            dors_config: RwLock::new(None),
        })
    }

    // ========================================================================
    // LOCK HELPERS (poison-safe wrappers for Result-returning methods)
    //
    // Result-returning methods use these helpers to propagate
    // `ProtocolError::LockPoisoned`.  Non-Result methods use the
    // module-level `recover_mutex` / `recover_rwlock_read` /
    // `recover_rwlock_write` utilities instead.
    // ========================================================================

    /// Lock the core protocol mutex, converting poison errors.
    fn lock_inner(&self) -> Result<std::sync::MutexGuard<'_, CoreProtocol>, ProtocolError> {
        self.inner
            .lock()
            .map_err(|e| ProtocolError::LockPoisoned(format!("inner: {}", e)))
    }

    /// Lock the BLE state mutex, converting poison errors.
    fn lock_ble(&self) -> Result<std::sync::MutexGuard<'_, BleState>, ProtocolError> {
        self.ble_state
            .lock()
            .map_err(|e| ProtocolError::LockPoisoned(format!("ble_state: {}", e)))
    }

    /// Lock the Internet state mutex, converting poison errors.
    fn lock_internet(&self) -> Result<std::sync::MutexGuard<'_, InternetState>, ProtocolError> {
        self.internet_state
            .lock()
            .map_err(|e| ProtocolError::LockPoisoned(format!("internet_state: {}", e)))
    }

    /// Lock the WiFi Direct state mutex, converting poison errors.
    fn lock_wifi_direct(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, WifiDirectState>, ProtocolError> {
        self.wifi_direct_state
            .lock()
            .map_err(|e| ProtocolError::LockPoisoned(format!("wifi_direct_state: {}", e)))
    }

    /// Lock the Reticulum state mutex, converting poison errors.
    fn lock_reticulum(&self) -> Result<std::sync::MutexGuard<'_, ReticulumState>, ProtocolError> {
        self.reticulum_state
            .lock()
            .map_err(|e| ProtocolError::LockPoisoned(format!("reticulum_state: {}", e)))
    }

    /// Lock the Nostr state mutex, converting poison errors.
    fn lock_nostr(&self) -> Result<std::sync::MutexGuard<'_, NostrState>, ProtocolError> {
        self.nostr_state
            .lock()
            .map_err(|e| ProtocolError::LockPoisoned(format!("nostr_state: {}", e)))
    }

    /// Acquire the inner + transport locks and call `f` with the transport
    /// registered under `transport_type`.  Returns `None` when the transport
    /// is absent.  Dispatch is through the `Transport` trait — no downcast,
    /// so it cannot silently miss a registered transport.
    fn with_transport<F, R>(&self, transport_type: CoreTransportType, f: F) -> Option<R>
    where
        F: FnOnce(&dyn Transport) -> R,
    {
        let protocol = recover_mutex(&self.inner, "inner");
        let transport_arc = protocol.transport_manager().get_transport(transport_type)?;
        Some(f(&*transport_arc))
    }

    /// Like `with_transport` but uses fallible lock acquisition
    /// (`lock_inner`) so it can propagate lock-poison errors.
    fn with_transport_fallible<F, R>(
        &self,
        transport_type: CoreTransportType,
        f: F,
    ) -> Result<Option<R>, ProtocolError>
    where
        F: FnOnce(&dyn Transport) -> R,
    {
        let protocol = self.lock_inner()?;
        let transport_arc = match protocol.transport_manager().get_transport(transport_type) {
            Some(arc) => arc,
            None => return Ok(None),
        };
        Ok(Some(f(&*transport_arc)))
    }

    /// Acquire the inner + transport locks, downcast to `BleTransport`, and
    /// call `f` with it.  Returns `None` when the transport is absent or not
    /// a `BleTransport`.  Only for BLE-specific inherent methods (peer
    /// registry, MTU telemetry counters) — everything on the `Transport`
    /// trait goes through [`Self::with_transport`] instead.
    fn with_ble_transport<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&BleTransport) -> R,
    {
        let protocol = recover_mutex(&self.inner, "inner");
        let transport_arc = protocol
            .transport_manager()
            .get_transport(CoreTransportType::BLE)?;
        let ble_transport = transport_arc.as_any().downcast_ref::<BleTransport>()?;
        Some(f(ble_transport))
    }

    /// Like `with_ble_transport` but uses fallible lock acquisition
    /// (`lock_inner`) so it can propagate lock-poison errors.
    fn with_ble_transport_fallible<F, R>(&self, f: F) -> Result<Option<R>, ProtocolError>
    where
        F: FnOnce(&BleTransport) -> R,
    {
        let protocol = self.lock_inner()?;
        let transport_arc = match protocol
            .transport_manager()
            .get_transport(CoreTransportType::BLE)
        {
            Some(arc) => arc,
            None => return Ok(None),
        };
        let ble_transport = match transport_arc.as_any().downcast_ref::<BleTransport>() {
            Some(bt) => bt,
            None => return Ok(None),
        };
        Ok(Some(f(ble_transport)))
    }

    /// Acquire the inner + transport locks, downcast to `NostrTransport`,
    /// and call `f` with it.  Returns `None` when the transport is absent or
    /// not a `NostrTransport`.  Only for Nostr-specific inherent methods
    /// (signed events, pubkey, subscriptions) — everything on the
    /// `Transport` trait goes through [`Self::with_transport`] instead.
    fn with_nostr_transport<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&offline_protocol_transport::nostr::NostrTransport) -> R,
    {
        let protocol = recover_mutex(&self.inner, "inner");
        let transport_arc = protocol
            .transport_manager()
            .get_transport(CoreTransportType::Nostr)?;
        let nostr_transport = transport_arc
            .as_any()
            .downcast_ref::<offline_protocol_transport::nostr::NostrTransport>(
        )?;
        Some(f(nostr_transport))
    }

    /// Lock the visualizer mutex, converting poison errors.
    fn lock_visualizer(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, NetworkVisualizer>, ProtocolError> {
        self.visualizer
            .lock()
            .map_err(|e| ProtocolError::LockPoisoned(format!("visualizer: {}", e)))
    }

    /// Write-lock the protocol state, converting poison errors.
    fn write_state(&self) -> Result<std::sync::RwLockWriteGuard<'_, ProtocolState>, ProtocolError> {
        self.state
            .write()
            .map_err(|e| ProtocolError::LockPoisoned(format!("state: {}", e)))
    }

    /// Write-lock the relay priority, converting poison errors.
    fn write_relay_priority(
        &self,
    ) -> Result<std::sync::RwLockWriteGuard<'_, RelayPriority>, ProtocolError> {
        self.relay_priority
            .write()
            .map_err(|e| ProtocolError::LockPoisoned(format!("relay_priority: {}", e)))
    }

    /// Read-lock the forced transport, converting poison errors.
    fn read_forced_transport(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, Option<TransportType>>, ProtocolError> {
        self.forced_transport
            .read()
            .map_err(|e| ProtocolError::LockPoisoned(format!("forced_transport: {}", e)))
    }

    /// Write-lock the forced transport, converting poison errors.
    fn write_forced_transport(
        &self,
    ) -> Result<std::sync::RwLockWriteGuard<'_, Option<TransportType>>, ProtocolError> {
        self.forced_transport
            .write()
            .map_err(|e| ProtocolError::LockPoisoned(format!("forced_transport: {}", e)))
    }

    /// Write-lock the DORS config, converting poison errors.
    fn write_dors_config(
        &self,
    ) -> Result<std::sync::RwLockWriteGuard<'_, Option<DorsConfig>>, ProtocolError> {
        self.dors_config
            .write()
            .map_err(|e| ProtocolError::LockPoisoned(format!("dors_config: {}", e)))
    }

    // ========================================================================
    // LIFECYCLE MANAGEMENT
    // ========================================================================

    /// Starts the protocol
    pub fn start(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol.start().map_err(ProtocolError::from)?;
        *self.write_state()? = ProtocolState::Running;

        drop(protocol);

        // Emit a network metrics event when started to verify event system is working
        let event = CoreEvent::NetworkMetrics {
            neighbor_count: 0,
            relay_count: 0,
            delivery_ratio: 0.0,
            avg_latency_ms: 0,
        };
        self.emit_event(event);

        Ok(())
    }

    /// Stops the protocol
    pub fn stop(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol.stop().map_err(ProtocolError::from)?;
        *self.write_state()? = ProtocolState::Stopped;
        Ok(())
    }

    /// Pauses the protocol
    pub fn pause(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol.pause().map_err(ProtocolError::from)?;
        *self.write_state()? = ProtocolState::Paused;
        Ok(())
    }

    /// Resumes the protocol
    pub fn resume(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol.resume().map_err(ProtocolError::from)?;
        *self.write_state()? = ProtocolState::Running;
        Ok(())
    }

    /// Gets the current protocol state
    pub fn get_state(&self) -> ProtocolState {
        *recover_rwlock_read(&self.state, "state")
    }

    /// Process internal protocol operations
    pub fn process(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol.process().map_err(ProtocolError::from)?;

        // Events are handled through the event callback system registered via on_event
        // The platform code polls for events using poll_event()

        Ok(())
    }

    // ========================================================================
    // EVENT HANDLING
    // ========================================================================

    /// Sets the event callback
    pub fn set_event_callback(&self, callback: Box<dyn EventCallback>) {
        *recover_rwlock_write(&self.event_callback, "event_callback") = Some(Arc::from(callback));
    }

    /// Internal: Emit an event through the callback
    fn emit_event(&self, event: crate::CoreEvent) {
        // Convert event to JSON
        if let Ok(event_json) = event.to_json() {
            // Call the callback if set
            if let Some(callback) =
                recover_rwlock_read(&self.event_callback, "event_callback").as_ref()
            {
                callback.on_event(event_json.clone());
            }

            // Also queue it for polling
            let mut queue = recover_mutex(&self.event_queue, "event_queue");
            queue.push_back(event_json);

            // Limit queue size to prevent memory issues
            if queue.len() > 1000 {
                queue.pop_front();
            }
        }
    }

    /// Internal: single seam for all carrier-level reachability signals
    /// (BLE discovery, WiFi Direct connections, and inbound messages on
    /// Internet/WiFi Direct/Reticulum/Nostr).
    ///
    /// Notifies the core protocol via `on_neighbor_discovered()` — which
    /// drives auto key exchange, outbox flush, and MLS Welcome re-arm and
    /// has its own is_user_blocked()/self checks — then emits the
    /// `NeighborDiscovered` platform event. The blocked read here is only
    /// needed to suppress the platform event.
    ///
    /// Message-path callers must drain `receive_message()` first so a
    /// handshake confirmation in the same inbound batch lands in
    /// `confirmed_sessions` before the Welcome re-arm decision. This
    /// ordering is regression-tested by
    /// `test_internet_confirm_in_same_batch_suppresses_redundant_welcome`.
    fn notify_neighbor_reachable(
        &self,
        peer_id: &str,
        transport: &str,
        rssi: Option<i16>,
    ) -> Result<(), ProtocolError> {
        if peer_id.is_empty() {
            return Ok(());
        }

        let is_blocked = {
            let mut protocol = self.lock_inner()?;
            let blocked = protocol.is_user_blocked(peer_id);
            protocol.on_neighbor_discovered(peer_id);
            blocked
        };

        if !is_blocked {
            self.emit_event(CoreEvent::NeighborDiscovered {
                peer_id: peer_id.to_string(),
                transport: transport.to_string(),
                rssi,
            });
        }

        Ok(())
    }

    /// Polls for the next event (returns JSON string or None)
    pub fn poll_event(&self) -> Option<String> {
        // Get from queue
        let mut queue = recover_mutex(&self.event_queue, "event_queue");
        queue.pop_front()
    }

    /// Emits a test event to verify the event system is working
    pub fn emit_test_event(&self) {
        let event = CoreEvent::NetworkMetrics {
            neighbor_count: 0,
            relay_count: 0,
            delivery_ratio: 0.0,
            avg_latency_ms: 0,
        };
        self.emit_event(event);
    }

    /// Installs a unified telemetry sink. Replaces any previously installed
    /// sink; a single adapter fans every `TelemetryRecord` out to the
    /// foreign callback and, concurrently, into the bounded poll queue
    /// backing `poll_telemetry_frame` (unless the caller disables it via
    /// `TelemetryConfig::enable_poll_queue = false`).
    pub fn install_telemetry_sink(
        &self,
        sink: Box<dyn TelemetrySink>,
        config: TelemetryConfig,
    ) -> Result<(), ProtocolError> {
        // `None` defaults to enabled to preserve the shipped behaviour of
        // `poll_telemetry_frame` returning queued records when the caller
        // does not opt out. Push-only consumers pass `Some(false)` to skip
        // the per-emit JSON envelope construction on the routing hot path.
        let poll_queue_enabled = config.enable_poll_queue.unwrap_or(true);
        let adapter = Arc::new(TelemetrySinkAdapter {
            callback: Arc::from(sink),
            queue: self.telemetry_queue.clone(),
            poll_queue_enabled,
        });
        let core_cfg = telemetry_config_into_core(config);
        let mut protocol = self.lock_inner()?;
        protocol
            .install_telemetry_sink(adapter as Arc<dyn CoreTelemetrySink>, core_cfg)
            .map_err(ProtocolError::from)
    }

    /// Polls the next buffered telemetry record as a JSON envelope. Every
    /// envelope carries a `category` discriminator plus variant-specific
    /// fields:
    ///
    /// | category           | extra fields                     |
    /// |--------------------|----------------------------------|
    /// | `protocol`         | `eventJson: string`              |
    /// | `mls`              | `eventJson: string`              |
    /// | `metricsFrame`     | `frame: MetricsFrame`            |
    /// | `transportState`   | `event: TransportStateEvent`     |
    /// | `routingDecision`  | `decision: RoutingDecision`      |
    /// | `deviceCapability` | `snapshot: DeviceCapabilitySnapshot` |
    /// | `extension`        | `name: string, payloadJson: string` |
    ///
    /// Returns `None` when empty. The queue is bounded
    /// (`TELEMETRY_POLL_QUEUE_CAP`, drop-oldest on overflow) and is only
    /// populated while a sink is installed.
    pub fn poll_telemetry_frame(&self) -> Option<String> {
        recover_mutex(&self.telemetry_queue, "telemetry_queue").pop_front()
    }

    /// Detaches the installed telemetry sink. Subsequent emissions drop on
    /// the core side (a `NoopTelemetrySink` replaces the current sink), and
    /// any records still sitting in the pull queue are drained so a
    /// subsequent `install_telemetry_sink` starts with a clean slate.
    ///
    /// Idempotent — calling this without a prior install still produces a
    /// noop-sink install and drains the (empty) queue.
    pub fn uninstall_telemetry_sink(&self) -> Result<(), ProtocolError> {
        let noop: Arc<dyn CoreTelemetrySink> = Arc::new(CoreNoopTelemetrySink);
        let mut protocol = self.lock_inner()?;
        protocol
            .install_telemetry_sink(noop, CoreTelemetryConfig::default())
            .map_err(ProtocolError::from)?;
        drop(protocol);
        recover_mutex(&self.telemetry_queue, "telemetry_queue").clear();
        Ok(())
    }

    /// Stable, opaque per-install telemetry identifier (32 hex chars),
    /// derived from the SDK-managed persistent scrub secret. The secret
    /// itself never crosses the FFI and cannot be recovered from the id.
    ///
    /// Returns `None` until the persistent secret is available — i.e. before
    /// secure storage is wired via `initialize_mls`, or when persisting the
    /// secret failed this session. Unaffected by installing a telemetry sink
    /// or by an app-supplied scrub secret.
    pub fn telemetry_install_id(&self) -> Option<String> {
        recover_mutex(&self.inner, "inner").telemetry_install_id()
    }

    // ========================================================================
    // TRANSPORT CALLBACKS (EVENT-DRIVEN SENDING)
    // ========================================================================

    /// Registers a BLE transport callback that fires when outgoing fragments
    /// become available. This replaces timer-based polling — the platform
    /// should call `ble_get_next_fragment()` inside the callback.
    pub fn set_ble_transport_callback(&self, callback: Box<dyn BleTransportCallback>) {
        let callback: Arc<dyn BleTransportCallback> = Arc::from(callback);
        self.with_transport(CoreTransportType::BLE, |transport| {
            transport.set_on_messages_available(Arc::new(move || {
                callback.on_fragments_available();
            }));
        });
    }

    /// Registers a WiFi Direct transport callback that fires when outgoing
    /// messages become available. This replaces timer-based polling.
    pub fn set_wifi_direct_transport_callback(
        &self,
        callback: Box<dyn WifiDirectTransportCallback>,
    ) {
        let callback: Arc<dyn WifiDirectTransportCallback> = Arc::from(callback);
        self.with_transport(CoreTransportType::WiFiDirect, |transport| {
            transport.set_on_messages_available(Arc::new(move || {
                callback.on_messages_available();
            }));
        });
    }

    /// Registers a Reticulum transport callback that fires when outgoing
    /// messages become available. This replaces timer-based polling.
    pub fn set_reticulum_transport_callback(&self, callback: Box<dyn ReticulumTransportCallback>) {
        let callback: Arc<dyn ReticulumTransportCallback> = Arc::from(callback);
        self.with_transport(CoreTransportType::Reticulum, |transport| {
            transport.set_on_messages_available(Arc::new(move || {
                callback.on_messages_available();
            }));
        });
    }

    /// Registers a Nostr transport callback that fires when outgoing
    /// messages become available. This replaces timer-based polling.
    pub fn set_nostr_transport_callback(&self, callback: Box<dyn NostrTransportCallback>) {
        let callback: Arc<dyn NostrTransportCallback> = Arc::from(callback);
        self.with_transport(CoreTransportType::Nostr, |transport| {
            transport.set_on_messages_available(Arc::new(move || {
                callback.on_messages_available();
            }));
        });
    }

    // ========================================================================
    // MESSAGING
    // ========================================================================

    /// Sends a message
    pub fn send_message(
        &self,
        recipient: String,
        content: String,
        priority: MessagePriority,
        reply_to_msg: Option<String>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;

        // Check if a transport is forced (bypasses DORS)
        let forced = *self.read_forced_transport()?;

        // If a transport is forced, use it directly; otherwise use DORS selection
        let message_id = if let Some(forced_type) = forced {
            let core_transport = match forced_type {
                TransportType::Internet => CoreTransportType::Internet,
                TransportType::Ble => CoreTransportType::BLE,
                TransportType::WiFiDirect => CoreTransportType::WiFiDirect,
                TransportType::Reticulum => CoreTransportType::Reticulum,
                TransportType::Nostr => CoreTransportType::Nostr,
            };
            protocol
                .send_message_via_transport(
                    &recipient,
                    &content,
                    Some(priority.into()),
                    core_transport,
                    reply_to_msg,
                )
                .map_err(map_send_error)?
        } else {
            protocol
                .send_message(&recipient, &content, Some(priority.into()), reply_to_msg)
                .map_err(map_send_error)?
        };

        Ok(message_id.as_str())
    }

    /// Rich send surface: like `send_message`, but accepting
    /// `SendMessageOptions`. Rich fields are sealed inside the MLS
    /// ciphertext for capable recipients and silently dropped otherwise —
    /// never sent cleartext. Honors a forced transport like `send_message`.
    pub fn send_message_rich(
        &self,
        recipient: String,
        content: String,
        options: SendMessageOptions,
    ) -> Result<String, ProtocolError> {
        // Validate the display-hint dictionaries into core types before
        // taking the protocol lock; a hostile identifier fails the call.
        let reply_context = options
            .reply_context
            .map(CoreReplyContext::try_from)
            .transpose()?;
        let forward_info = options
            .forward_info
            .map(CoreForwardInfo::try_from)
            .transpose()?;

        let mut protocol = self.lock_inner()?;

        // Check if a transport is forced (bypasses DORS), same as send_message.
        let forced = *self.read_forced_transport()?;
        let via_transport = forced.map(|forced_type| match forced_type {
            TransportType::Internet => CoreTransportType::Internet,
            TransportType::Ble => CoreTransportType::BLE,
            TransportType::WiFiDirect => CoreTransportType::WiFiDirect,
            TransportType::Reticulum => CoreTransportType::Reticulum,
            TransportType::Nostr => CoreTransportType::Nostr,
        });

        let core_options = CoreSendMessageOptions {
            priority: options.priority.map(|p| p.into()),
            reply_to_msg: options.reply_to_msg,
            content_type: options.content_type.map(|ct| ct.into()),
            reply_context,
            media_metadata: options.media_metadata.map(CoreMediaMetadata::from),
            forward_info,
            via_transport,
        };

        let message_id = protocol
            .send_message_with(&recipient, &content, core_options)
            .map_err(map_send_error)?;
        Ok(message_id.as_str())
    }

    /// Forwards a message to a new recipient with original sender attribution.
    ///
    /// `original_message_json` must be the core-serde message shape. The
    /// JSON returned by `receive_message` round-trips here directly. The
    /// `message_received` event does NOT (different field names, no
    /// `app_id`/`ttl`) — apps forwarding from event data must build the
    /// core shape themselves: `id`, `sender`, `recipient`, `app_id`,
    /// `priority` (lowercase or capitalized), `ttl`, `hop_count`,
    /// `timestamp` (epoch millis), `content`, plus the optional rich
    /// fields. Include `media_metadata` — with its `encryption_key`/`iv` —
    /// when forwarding cloud media: toward a rich-capable recipient those
    /// travel inside the MLS-sealed body (the only carrier that can
    /// deliver media secrets); toward legacy recipients the secrets are
    /// stripped at the wire and only URL/attribution survive.
    pub fn forward_message(
        &self,
        original_message_json: String,
        new_recipient: String,
        priority: Option<MessagePriority>,
    ) -> Result<String, ProtocolError> {
        let original: offline_protocol_core::Message = serde_json::from_str(&original_message_json)
            .map_err(|e| {
                ProtocolError::InvalidConfiguration(format!(
                    "Failed to parse original message JSON: {}",
                    e
                ))
            })?;
        let core_priority = priority.map(|p| p.into());
        let mut protocol = self.lock_inner()?;
        let message_id = protocol
            .forward_message(&original, &new_recipient, core_priority)
            .map_err(map_send_error)?;
        Ok(message_id.as_str())
    }

    /// Receives the next message (returns JSON string or None)
    pub fn receive_message(&self) -> Option<String> {
        let mut protocol = recover_mutex(&self.inner, "inner");
        protocol
            .receive_message()
            .and_then(|msg| received_message_to_json(&msg))
    }

    // ========================================================================
    // CONNECTION REQUESTS (TRANSPORT-AGNOSTIC)
    // ========================================================================

    /// Sends a connection request to another user via any available transport (DORS-routed).
    pub fn send_connection_request(
        &self,
        recipient: String,
        sender_name: String,
        key_package: Option<Vec<u8>>,
        initial_message: Option<String>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        let message_id = protocol
            .send_connection_request(&recipient, &sender_name, key_package, initial_message)
            .map_err(map_send_error)?;
        Ok(message_id.as_str())
    }

    /// Accepts a connection request from another user via any available transport (DORS-routed).
    pub fn accept_connection_request(
        &self,
        recipient: String,
        accepter_name: String,
        key_package: Option<Vec<u8>>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        let message_id = protocol
            .accept_connection_request(&recipient, &accepter_name, key_package)
            .map_err(map_send_error)?;
        Ok(message_id.as_str())
    }

    /// Rejects a connection request from another user via any available transport (DORS-routed).
    pub fn reject_connection_request(&self, recipient: String) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        let message_id = protocol
            .reject_connection_request(&recipient)
            .map_err(map_send_error)?;
        Ok(message_id.as_str())
    }

    /// Cancels a previously sent connection request via any available transport (DORS-routed).
    pub fn cancel_connection_request(&self, recipient: String) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        let message_id = protocol
            .cancel_connection_request(&recipient)
            .map_err(map_send_error)?;
        Ok(message_id.as_str())
    }

    // ========================================================================
    // SERVICE DISCOVERY (delegated via MeshServices wrapper)
    // ========================================================================

    /// Registers a local service for discovery.
    pub(crate) fn svc_register_service(
        &self,
        service_id: String,
        version: String,
        capabilities: HashMap<String, String>,
    ) -> Result<(), ProtocolError> {
        use offline_protocol_core::{ServiceDescriptor, ServiceId};
        let sid = ServiceId::new(&service_id)
            .map_err(|e| ProtocolError::InvalidConfiguration(e.to_string()))?;
        let descriptor = ServiceDescriptor {
            service_id: sid,
            version,
            capabilities,
        };
        let mut protocol = self.lock_inner()?;
        protocol
            .register_service(descriptor)
            .map_err(ProtocolError::from)
    }

    /// Unregisters a local service. Returns true if found and removed.
    pub(crate) fn svc_unregister_service(&self, service_id: String) -> Result<bool, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol
            .unregister_service(&service_id)
            .map_err(ProtocolError::from)
    }

    /// Broadcasts a service discovery query. Returns a query_id.
    pub(crate) fn svc_discover_services(
        &self,
        service_id: Option<String>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol
            .discover_services(service_id.as_deref())
            .map_err(ProtocolError::from)
    }

    /// Sends a service request to a specific provider peer. Returns a request_id.
    pub(crate) fn svc_send_service_request(
        &self,
        provider: String,
        service_id: String,
        method: String,
        body: String,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol
            .send_service_request(&provider, &service_id, &method, &body)
            .map_err(map_send_error)
    }

    /// Responds to a service request from another peer.
    pub(crate) fn svc_respond_to_service_request(
        &self,
        request_id: String,
        requester: String,
        service_id: String,
        status: String,
        body: String,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        let message_id = protocol
            .respond_to_service_request(&request_id, &requester, &service_id, &status, &body)
            .map_err(map_send_error)?;
        Ok(message_id.as_str())
    }

    // ========================================================================
    // BLE TRANSPORT OPERATIONS
    // ========================================================================

    /// BLE: Peer discovered
    pub fn ble_peer_discovered(&self, peer_id: String, rssi: i16) -> Result<(), ProtocolError> {
        // Update local state for tracking
        let mut ble_state = self.lock_ble()?;
        let peer = PeerDevice {
            peer_id: peer_id.clone(),
            rssi,
            last_seen_ms: SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        };
        ble_state.peers.insert(peer_id.clone(), peer);
        ble_state.peer_count = ble_state.peers.len() as u32;
        drop(ble_state);

        // Register peer with the BLE transport so send() can route to them
        self.with_ble_transport_fallible(|ble_transport| {
            ble_transport.on_peer_discovered(offline_protocol_transport::ble::PeerDevice {
                device_id: peer_id.clone(),
                address: String::new(),
                rssi,
                last_seen: SystemTime::now(),
                connected: true,
            });
        })?;

        self.notify_neighbor_reachable(&peer_id, "BLE", Some(rssi))
    }

    /// BLE: Peer lost
    pub fn ble_peer_lost(&self, peer_id: String) -> Result<(), ProtocolError> {
        let mut ble_state = self.lock_ble()?;
        ble_state.peers.remove(&peer_id);
        ble_state.peer_count = ble_state.peers.len() as u32;
        drop(ble_state);

        // Unregister peer from the BLE transport
        self.with_ble_transport_fallible(|ble_transport| {
            ble_transport.on_peer_lost(&peer_id);
        })?;

        // Notify the core protocol of neighbor loss
        {
            let mut protocol = self.lock_inner()?;
            protocol.on_neighbor_lost(&peer_id);
        }

        // Emit NeighborLost event
        let event = CoreEvent::NeighborLost {
            peer_id: peer_id.clone(),
        };
        self.emit_event(event);

        Ok(())
    }

    /// BLE: Status changed
    pub fn ble_status_changed(&self, is_available: bool) -> Result<(), ProtocolError> {
        // Update the BLE transport status based on platform availability
        let new_status = if is_available {
            offline_protocol_transport::TransportStatus::Available
        } else {
            offline_protocol_transport::TransportStatus::Unavailable
        };
        self.with_transport_fallible(CoreTransportType::BLE, |transport| {
            transport.on_status_changed(new_status);
        })?;

        Ok(())
    }

    /// BLE: Record the negotiated MTU (max usable fragment payload) for a peer.
    ///
    /// Callers pass the already header-adjusted value: iOS reads
    /// `CBPeripheral.maximumWriteValueLength(for: .withoutResponse)`, and
    /// Android subtracts the 3-byte ATT overhead from `onMtuChanged`'s value.
    /// The Rust transport clamps to [BLE_MAX_FRAGMENT_SIZE, MAX_REASONABLE_BLE_PAYLOAD].
    ///
    /// If the BLE transport is not registered (meaning "BLE not configured
    /// on this instance"), the call is a warn-and-drop no-op rather than an
    /// error — the platform layer should be free to report MTUs
    /// unconditionally without having to branch on transport configuration.
    /// A non-MTU-aware transport registered under BLE warns via the
    /// `Transport::set_peer_mtu` default instead.
    pub fn ble_set_peer_mtu(&self, peer_id: String, max_payload: u32) -> Result<(), ProtocolError> {
        let dispatched = self.with_transport_fallible(CoreTransportType::BLE, |transport| {
            transport.set_peer_mtu(&peer_id, max_payload as usize);
        })?;
        if dispatched.is_none() {
            tracing::warn!(
                %peer_id,
                max_payload,
                "ble_set_peer_mtu: BLE transport not registered; ignoring"
            );
        }
        Ok(())
    }

    /// BLE: Forget the MTU for a peer (called on disconnect).
    ///
    /// Mirrors [`Self::ble_set_peer_mtu`] — both "BLE not configured"
    /// shapes warn-and-return-Ok so platform teardown paths can call
    /// unconditionally.
    pub fn ble_clear_peer_mtu(&self, peer_id: String) -> Result<(), ProtocolError> {
        let dispatched = self.with_transport_fallible(CoreTransportType::BLE, |transport| {
            transport.clear_peer_mtu(&peer_id);
        })?;
        if dispatched.is_none() {
            tracing::warn!(
                %peer_id,
                "ble_clear_peer_mtu: BLE transport not registered; ignoring"
            );
        }
        Ok(())
    }

    /// BLE: Monotonic count of undersized MTU reports since transport
    /// creation.
    ///
    /// A non-zero value indicates that at least one peer reported a max
    /// usable fragment payload below the Rust transport's fallback floor
    /// and is now being served the floor — which is *higher* than the
    /// real link capacity, so outbound writes to that peer are dropped
    /// by the controller. Surface in dashboards to detect controllers
    /// that violate the target-platform assumption. Returns 0 when the
    /// BLE transport is not registered.
    ///
    /// **Lock handling:** this method intentionally uses
    /// `recover_mutex` rather than `Self::lock_inner` because it is
    /// a pure, read-only telemetry getter with an infallible return
    /// type. Propagating `LockPoisoned` as a `Result<u64, ProtocolError>`
    /// would force every dashboard call site to unwrap, and telemetry
    /// fetches are the least useful place to hide panic-producing
    /// `unwrap()`s. Recovering the poisoned guard just to read an
    /// atomic is safe: the atomic counter is the only state read here,
    /// and poisoning cannot corrupt it. The setter siblings
    /// (`ble_set_peer_mtu`, `ble_clear_peer_mtu`) still propagate
    /// poisoning because they mutate state and the caller has a
    /// natural error channel.
    pub fn ble_undersized_mtu_reports(&self) -> u64 {
        self.with_ble_transport(|ble_transport| ble_transport.undersized_mtu_reports())
            .unwrap_or(0)
    }

    /// BLE: Monotonic count of `fragment_message` calls that fell back
    /// to the fragment-size floor because no per-peer MTU was on file
    /// for the recipient **and the recipient is still a registered
    /// direct BLE peer** at the moment of fallback.
    ///
    /// In healthy operation this should remain zero. Both platforms
    /// push the MTU BEFORE announcing the peer (iOS: `bleSetPeerMtu`
    /// precedes `blePeerDiscovered`; Android: the facade flushes the
    /// staged MTU via `onDeviceIdResolved` before `blePeerDiscovered`
    /// fires), so by the time any fragmenting send can reach a live
    /// peer the MTU entry is already on file. The counter
    /// deliberately excludes the benign send / on_peer_lost race
    /// (message enqueued while peer was live, `on_peer_lost` dropped
    /// both maps before `get_next_fragment` popped it) because
    /// counting that case would mask the real signal on every
    /// disconnect with in-flight sends.
    ///
    /// A non-zero value therefore means either (a) the MTU-before-
    /// discover ordering invariant regressed on one of the platforms,
    /// or (b) the `recipient -> device_id` keying contract broke —
    /// at which point every fragmenting send to live peers is
    /// silently regressing to the 185-byte floor. Surface in
    /// dashboards so production alerts fire the first time the
    /// invariant breaks. Returns 0 when the BLE transport is not
    /// registered. Uses `recover_mutex` for the same reason as
    /// [`Self::ble_undersized_mtu_reports`].
    pub fn ble_fragment_fallback_count(&self) -> u64 {
        self.with_ble_transport(|ble_transport| ble_transport.fragment_fallback_count())
            .unwrap_or(0)
    }

    /// BLE: Fragment received
    pub fn ble_fragment_received(
        &self,
        _sender_id: String,
        fragment: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::BLE)
        {
            transport_arc.on_fragment_received(fragment).map_err(|e| {
                ProtocolError::TransportError(format!("Fragment processing failed: {}", e))
            })?;
        }

        while protocol.receive_message().is_some() {}

        Ok(())
    }

    /// BLE: Get next fragment to send
    pub fn ble_get_next_fragment(&self) -> Option<BleFragment> {
        let fragment = self
            .with_transport(CoreTransportType::BLE, |transport| {
                // Ensure BLE is available for fragment polling
                if transport.status() != offline_protocol_transport::TransportStatus::Available {
                    transport
                        .on_status_changed(offline_protocol_transport::TransportStatus::Available);
                }

                // Get next fragment
                if let Ok(Some((recipient, data))) = transport.get_next_fragment() {
                    return Some(BleFragment {
                        recipient_id: recipient,
                        data,
                    });
                }
                None
            })
            .flatten();
        if fragment.is_some() {
            return fragment;
        }

        // Fallback to local queue for backwards compatibility
        let mut ble_state = recover_mutex(&self.ble_state, "ble_state");
        if let Some((recipient, data)) = ble_state.fragments.pop_front() {
            return Some(BleFragment {
                recipient_id: recipient,
                data,
            });
        }

        None
    }

    /// BLE: Return fragment (marks last fragment as sent)
    pub fn ble_return_fragment(&self) {
        // This is a no-op for backwards compatibility
        // Fragment sending confirmation is handled by the transport layer
    }

    /// BLE: Get peer count
    pub fn ble_get_peer_count(&self) -> u32 {
        let ble_state = recover_mutex(&self.ble_state, "ble_state");
        ble_state.peer_count
    }

    // ========================================================================
    // INTERNET TRANSPORT OPERATIONS
    // ========================================================================

    /// Internet: Status changed (connected/disconnected to relay server)
    ///
    /// EDGE CASE HANDLING:
    /// - When internet reconnects, triggers immediate flush of pending outbox messages
    /// - Handles race conditions between transport switching and message sending
    /// - Ensures messages queued during disconnection are sent when transport is available
    pub fn internet_status_changed(&self, is_connected: bool) -> Result<(), ProtocolError> {
        // Atomically read previous state and update in a single lock scope
        // (two scopes would let concurrent calls interleave between the read
        // and the write, double-flushing or missing the flush below).
        let was_connected = {
            let mut internet_state = self.lock_internet()?;
            let prev = internet_state.is_connected;
            internet_state.is_connected = is_connected;
            prev
        };

        // Update the Internet transport status in the transport manager.
        // Unconditional even on a redundant report: the transport's own
        // on_status_changed is idempotent.
        {
            let new_status = if is_connected {
                offline_protocol_transport::TransportStatus::Available
            } else {
                offline_protocol_transport::TransportStatus::Disconnected
            };
            self.with_transport_fallible(CoreTransportType::Internet, |transport| {
                transport.on_status_changed(new_status);
            })?;
        }

        // When reconnecting after disconnection, immediately flush all pending
        // outbox messages (bypasses backoff timers)
        if is_connected && !was_connected {
            let mut protocol = self.lock_inner()?;
            protocol.flush_outbox_all();
        }

        // Emit the connection event only on a real transition — bridges may
        // re-report the current state (e.g. on auth refresh), and duplicate
        // TransportSwitched events would show phantom switches downstream.
        if was_connected != is_connected {
            let event = if is_connected {
                CoreEvent::TransportSwitched {
                    from: None,
                    to: "Internet".to_string(),
                    reason: "Connected to relay server".to_string(),
                }
            } else {
                CoreEvent::TransportSwitched {
                    from: Some("Internet".to_string()),
                    to: "None".to_string(),
                    reason: "Disconnected from relay server".to_string(),
                }
            };
            self.emit_event(event);
        }

        Ok(())
    }

    /// Internet: Message received from relay server
    ///
    /// `sender_id` is the relay envelope's `sender` field, which the relay
    /// stamps server-side from the *authenticated* uploading connection —
    /// it is never client-supplied. When non-empty it is attached to the
    /// deserialized message as its transport-verified peer identity, which
    /// the control-message security gate strict-matches against
    /// `message.sender` for frames claiming direct origin (`hop_count == 0`).
    /// Mesh-relayed frames (`hop_count > 0`) are exempt: there the uploader
    /// is the carrier, not the origin. An empty `sender_id` falls back to
    /// unattributed ingest so the message is still processed best-effort.
    ///
    /// **`sender_id` is a reachability assertion, not just an attribution.**
    /// A non-empty value routes through `notify_neighbor_reachable` (private),
    /// so the core starts tracking it as a live neighbor: outbox flush,
    /// Welcome re-arm, auto key exchange (an outbound key-package DM),
    /// `NeighborDiscovered`, and service-discovery fan-out. Bridges that
    /// inject *locally synthesized* frames — relay server answers such as
    /// `__GROUP_CREATED__`/`__GROUP_ERROR__`, which no peer transmitted —
    /// must pass an empty `sender_id`, and must build those frames with
    /// `requires_ack: false` so the receive path does not address a delivery
    /// ACK to their placeholder `sender` either. Passing a placeholder
    /// identity here manufactures a phantom peer whose undeliverable DMs the
    /// relay answers with `DeliveryError` forever (see
    /// `test_synthesized_relay_frame_creates_no_phantom_peer`).
    pub fn internet_message_received(
        &self,
        sender_id: String,
        data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::Internet)
        {
            let ingest_result = if sender_id.is_empty() {
                transport_arc.on_data_received(data)
            } else {
                transport_arc.on_data_received_from(data, sender_id.clone())
            };
            if let Err(e) = ingest_result {
                return Err(ProtocolError::TransportError(format!(
                    "Failed to process internet message: {}",
                    e
                )));
            }
        }

        while protocol.receive_message().is_some() {}
        drop(protocol);

        self.notify_neighbor_reachable(&sender_id, "Internet", None)
    }

    /// Internet: Get next message to send via WebSocket.
    ///
    /// Returns the next queued message with its `message_id`. After sending
    /// over the wire, the platform **must** call either `internet_confirm_sent(message_id)`
    /// or `internet_send_failed(message_id)`/`internet_send_failed_with_reason(message_id, reason)`
    /// to close the feedback loop.
    pub fn internet_get_next_message(&self) -> Option<InternetMessage> {
        {
            let internet_state = recover_mutex(&self.internet_state, "internet_state");
            if !internet_state.is_connected {
                return None;
            }
        }

        let mut protocol = recover_mutex(&self.inner, "inner");
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::Internet)
        {
            if let Ok(Some((message_id, data))) = transport_arc.get_next_message() {
                match transport_arc.deserialize_message(&data) {
                    Ok(message) => {
                        let control = protocol.internet_control_op(&message);
                        return Some(InternetMessage {
                            message_id,
                            recipient_id: message.recipient.as_str().to_string(),
                            data,
                            reply_to_msg: message
                                .reply_to_msg
                                .as_ref()
                                .map(|id| id.as_str().to_string()),
                            control_op: control.as_ref().map(|(op, _)| op.to_string()),
                            control_payload: control.map(|(_, payload)| payload),
                        });
                    }
                    Err(e) => {
                        // The transport serialized this exact frame when it
                        // was queued, so a round-trip failure here is a serde
                        // asymmetry bug that must be investigated — never a
                        // silent drop. The frame is already popped and gone;
                        // close the confirm/fail feedback loop now instead of
                        // letting the pending entry age out as an anonymous
                        // timeout failure.
                        tracing::warn!(
                            message_id = %message_id,
                            data_len = data.len(),
                            error = %e,
                            "Dropping internet outbox frame: failed to deserialize a frame the transport itself serialized — message is permanently lost"
                        );
                        if let Err(err) = protocol.on_transport_send_failed(
                            &message_id,
                            Some("Internet outbox frame failed deserialization".to_string()),
                        ) {
                            tracing::warn!(
                                message_id = %message_id,
                                error = %err,
                                "Failed to apply transport-failure to pending welcome lifecycle (frame may not be a welcome)"
                            );
                        }
                        transport_arc.report_send_failure(&message_id);
                    }
                }
            }
        }

        None
    }

    /// Internet: Confirm that a message was successfully sent over the wire.
    ///
    /// The platform must call this after the WebSocket send completes successfully.
    /// This feeds real delivery data into transport metrics so DORS can make
    /// accurate routing decisions.
    pub fn internet_confirm_sent(&self, message_id: String) {
        let mut protocol = recover_mutex(&self.inner, "inner");
        if let Err(err) = protocol.on_transport_send_confirmed(&message_id) {
            tracing::warn!(
                message_id = %message_id,
                error = %err,
                "Failed to apply welcome lifecycle transport confirmation"
            );
        }
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::Internet)
        {
            transport_arc.confirm_sent(&message_id);
        }
    }

    /// Internet: Report that a message failed to send over the wire.
    ///
    /// The platform must call this when the WebSocket send fails.
    pub fn internet_send_failed(&self, message_id: String) {
        self.internet_send_failed_with_reason(
            message_id,
            Some("Internet transport send failed".to_string()),
        );
    }

    /// Internet: Report that a message failed to send over the wire.
    ///
    /// `reason` should carry platform-specific error context so reliability
    /// telemetry can classify root causes more accurately.
    pub fn internet_send_failed_with_reason(&self, message_id: String, reason: Option<String>) {
        let mut protocol = recover_mutex(&self.inner, "inner");
        if let Err(err) = protocol.on_transport_send_failed(&message_id, reason) {
            tracing::warn!(
                message_id = %message_id,
                error = %err,
                "Failed to apply welcome lifecycle transport failure"
            );
        }
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::Internet)
        {
            transport_arc.report_send_failure(&message_id);
        }
    }

    /// Internet: Relay-sourced presence for a peer (the bridge's
    /// `PresenceStatus` / `PresenceStatusWithLastSeen` handler).
    ///
    /// `online=true` drives the reachability machinery (welcome re-arm,
    /// outbox flush, auto key exchange); `online=false` parks pending
    /// welcomes without burning retry budget. Emits `presence_updated` with
    /// the optional `last_seen_ms` — except for self, blocked, or empty peer
    /// ids, which are dropped without an event.
    pub fn internet_peer_presence(&self, peer_id: String, online: bool, last_seen_ms: Option<i64>) {
        if peer_id.is_empty() {
            return;
        }
        let mut protocol = recover_mutex(&self.inner, "inner");
        protocol.on_peer_presence(&peer_id, online, last_seen_ms);
    }

    /// Internet: Peers the platform layer should watch via relay
    /// `CheckPresence` queries — every peer with an undelivered or
    /// session-unproven MLS welcome, plus every recipient of a pending or
    /// unreachable-parked outbox message. The platform polls this on its
    /// presence-watch tick; the SDK now owns the "watch my DeliveryError
    /// recipients" duty, so the platform no longer needs to union in its
    /// own list (the presence-online answer is what re-drives parked DMs).
    pub fn internet_presence_watchlist(&self) -> Vec<String> {
        let protocol = recover_mutex(&self.inner, "inner");
        protocol.presence_watch_peers()
    }

    /// Internet: capability tokens from the relay's `Authenticated` answer
    /// (e.g. `group_delivery_v2`).
    ///
    /// The bridge MUST call this before `internet_status_changed(true)` on
    /// each (re)connect — the false→true flush has to see the capabilities,
    /// or the first sends after a reconnect take the capability-less path.
    /// Wholesale replace; the SDK clears the set itself when the Internet
    /// transport drops, so a relay that predates the advertisement simply
    /// leaves it empty.
    pub fn internet_relay_capabilities(
        &self,
        capabilities: Vec<String>,
    ) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol.set_relay_capabilities(capabilities);
        Ok(())
    }

    /// Internet: a relay `GroupMessageSent` settled delivery report,
    /// forwarded verbatim as the raw server frame JSON.
    ///
    /// Correlated by its `message_id` to an in-flight group broadcast: the
    /// tracker settles, members the relay did not reach get a per-member
    /// re-send through the ordinary delivery ladder, and the app sees
    /// `group_message_delivery_report`. A dedicated entry point (not
    /// message-plane injection) so it cannot be forged through the
    /// notification ciphertext injector. Reports that correlate with
    /// nothing are ignored; malformed JSON is an error.
    pub fn internet_group_report_received(&self, report_json: String) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol
            .handle_relay_group_delivery_report(&report_json)
            .map_err(ProtocolError::from)
    }

    // ========================================================================
    // WIFI DIRECT TRANSPORT OPERATIONS
    // ========================================================================

    /// WiFi Direct: Status changed (connected/disconnected to peer group)
    pub fn wifi_direct_status_changed(&self, is_connected: bool) -> Result<(), ProtocolError> {
        // Update internal state
        {
            let mut wifi_direct_state = self.lock_wifi_direct()?;
            wifi_direct_state.is_connected = is_connected;
            if !is_connected {
                wifi_direct_state.connected_peer = None;
            }
        }

        // Update the WiFi Direct transport status in the transport manager
        let new_status = if is_connected {
            offline_protocol_transport::TransportStatus::Available
        } else {
            offline_protocol_transport::TransportStatus::Disconnected
        };
        self.with_transport_fallible(CoreTransportType::WiFiDirect, |transport| {
            transport.on_status_changed(new_status);
        })?;

        // Emit connection event
        let event = if is_connected {
            CoreEvent::TransportSwitched {
                from: None,
                to: "WiFiDirect".to_string(),
                reason: "Connected to WiFi Direct peer group".to_string(),
            }
        } else {
            CoreEvent::TransportSwitched {
                from: Some("WiFiDirect".to_string()),
                to: "None".to_string(),
                reason: "Disconnected from WiFi Direct peer group".to_string(),
            }
        };
        self.emit_event(event);

        Ok(())
    }

    /// WiFi Direct: Message received from peer
    pub fn wifi_direct_message_received(
        &self,
        sender_id: String,
        data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        if let Some(transport_arc) = protocol
            .transport_manager()
            .get_transport(CoreTransportType::WiFiDirect)
        {
            if let Err(e) = transport_arc.on_data_received(data) {
                return Err(ProtocolError::TransportError(format!(
                    "Failed to process WiFi Direct message: {}",
                    e
                )));
            }
        }

        while protocol.receive_message().is_some() {}
        drop(protocol);

        self.notify_neighbor_reachable(&sender_id, "WiFiDirect", None)
    }

    /// WiFi Direct: Get next message to send
    pub fn wifi_direct_get_next_message(&self) -> Option<WifiDirectMessage> {
        // Check if connected
        {
            let wifi_direct_state = recover_mutex(&self.wifi_direct_state, "wifi_direct_state");
            if !wifi_direct_state.is_connected {
                return None;
            }
        }

        // Try to get message from the WiFi Direct transport
        self.with_transport(CoreTransportType::WiFiDirect, |transport| {
            if let Ok(Some((recipient, data))) = transport.get_next_message() {
                return Some(WifiDirectMessage {
                    recipient_id: recipient,
                    data,
                });
            }
            None
        })
        .flatten()
    }

    /// WiFi Direct: Peer connected
    pub fn wifi_direct_peer_connected(&self, peer_id: String) -> Result<(), ProtocolError> {
        // Update internal state
        {
            let mut wifi_direct_state = self.lock_wifi_direct()?;
            wifi_direct_state.connected_peer = Some(peer_id.clone());
        }

        self.notify_neighbor_reachable(&peer_id, "WiFiDirect", None)
    }

    /// WiFi Direct: Peer disconnected
    pub fn wifi_direct_peer_disconnected(&self, peer_id: String) -> Result<(), ProtocolError> {
        // Update internal state
        {
            let mut wifi_direct_state = self.lock_wifi_direct()?;
            if wifi_direct_state.connected_peer.as_ref() == Some(&peer_id) {
                wifi_direct_state.connected_peer = None;
            }
        }

        // Notify the core protocol of neighbor loss, matching ble_peer_lost —
        // WiFi Direct is the only other carrier with an explicit disconnect
        // signal, so core discovery tracking must not outlive the connection.
        {
            let mut protocol = self.lock_inner()?;
            protocol.on_neighbor_lost(&peer_id);
        }

        // Emit NeighborLost event
        let event = CoreEvent::NeighborLost { peer_id };
        self.emit_event(event);

        Ok(())
    }

    // ========================================================================
    // RETICULUM TRANSPORT
    // ========================================================================

    /// Called by the platform when the Reticulum daemon connection status changes.
    pub fn reticulum_status_changed(&self, is_connected: bool) -> Result<(), ProtocolError> {
        // Atomically read previous state and update in a single lock scope
        let was_connected = {
            let mut reticulum_state = self.lock_reticulum()?;
            let prev = reticulum_state.is_connected;
            reticulum_state.is_connected = is_connected;
            prev
        };

        // Update the Reticulum transport status in the transport manager
        self.with_transport_fallible(CoreTransportType::Reticulum, |transport| {
            let new_status = if is_connected {
                offline_protocol_transport::TransportStatus::Available
            } else {
                offline_protocol_transport::TransportStatus::Disconnected
            };
            transport.on_status_changed(new_status);
        })?;

        // When reconnecting after disconnection, immediately flush all pending
        // outbox messages (bypasses backoff timers)
        if is_connected && !was_connected {
            let mut protocol = self.lock_inner()?;
            protocol.flush_outbox_all();
        }

        // Emit connection event
        let event = if is_connected {
            CoreEvent::TransportSwitched {
                from: None,
                to: "Reticulum".to_string(),
                reason: "Connected to Reticulum daemon".to_string(),
            }
        } else {
            CoreEvent::TransportSwitched {
                from: Some("Reticulum".to_string()),
                to: "None".to_string(),
                reason: "Disconnected from Reticulum daemon".to_string(),
            }
        };
        self.emit_event(event);

        Ok(())
    }

    /// Called by the platform when data is received from the Reticulum daemon.
    pub fn reticulum_message_received(
        &self,
        sender_id: String,
        data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        // Deliver data to the transport in an isolated lock scope so the
        // inner + transport locks are released before we re-acquire inner
        // for receive_message().  This reduces contention.
        if let Some(Err(e)) = self.with_transport_fallible(CoreTransportType::Reticulum, |rt| {
            rt.on_data_received_from(data, sender_id.clone())
        })? {
            return Err(ProtocolError::TransportError(format!(
                "Failed to process reticulum message: {}",
                e
            )));
        }

        // Separate lock scope: drain received messages before the
        // reachability notification re-acquires the inner lock.
        {
            let mut protocol = self.lock_inner()?;
            while protocol.receive_message().is_some() {}
        }

        self.notify_neighbor_reachable(&sender_id, "Reticulum", None)
    }

    /// Returns the next outgoing Reticulum message, if any.
    /// The platform should send this via the Reticulum daemon, then call
    /// `reticulum_confirm_sent()` or `reticulum_send_failed()`.
    pub fn reticulum_get_next_message(&self) -> Option<ReticulumMessage> {
        // Note: there is a benign TOCTOU between the `is_connected` check and
        // the subsequent `with_transport` call — the connection state
        // could change between the two lock acquisitions.  This is acceptable:
        //   - Race to disconnected: `get_next_message()` returns None or the
        //     transport handles it gracefully; the message stays in the queue.
        //   - Race to connected: we return None this call; the next poll or
        //     callback-driven invocation picks it up.
        // This mirrors `wifi_direct_get_next_message` and avoids holding two
        // mutexes (`reticulum_state` + `inner`/transport) simultaneously.
        {
            let reticulum_state = recover_mutex(&self.reticulum_state, "reticulum_state");
            if !reticulum_state.is_connected {
                return None;
            }
        }

        self.with_transport(CoreTransportType::Reticulum, |rt| {
            if let Ok(Some((message_id, data))) = rt.get_next_message() {
                match rt.deserialize_message(&data) {
                    Ok(message) => {
                        return Some(ReticulumMessage {
                            message_id,
                            recipient_id: message.recipient.as_str().to_string(),
                            data,
                            reply_to_msg: message
                                .reply_to_msg
                                .as_ref()
                                .map(|id| id.as_str().to_string()),
                        });
                    }
                    Err(e) => {
                        // IMPORTANT: permanent message loss — the message has
                        // been dequeued from the transport but cannot be
                        // deserialized, so it is reported as a send failure
                        // and discarded.  The API returns Option (not Result),
                        // so we cannot propagate the error to the caller.
                        // Consistent with Internet transport's handling.
                        tracing::error!(
                            message_id = %message_id,
                            error = %e,
                            "Failed to deserialize reticulum message — message permanently lost, reporting failure"
                        );
                        rt.report_send_failure(&message_id);
                    }
                }
            }
            None
        })
        .flatten()
    }

    /// Called by the platform after successfully sending a Reticulum message.
    pub fn reticulum_confirm_sent(&self, message_id: String) {
        {
            let mut protocol = recover_mutex(&self.inner, "inner");
            if let Err(err) = protocol.on_transport_send_confirmed(&message_id) {
                tracing::warn!(
                    message_id = %message_id,
                    error = %err,
                    "Failed to apply welcome lifecycle transport confirmation (reticulum)"
                );
            }
        }
        self.with_transport(CoreTransportType::Reticulum, |rt| {
            rt.confirm_sent(&message_id);
        });
    }

    /// Called by the platform when sending a Reticulum message fails.
    pub fn reticulum_send_failed(&self, message_id: String) {
        self.reticulum_send_failed_with_reason(
            message_id,
            Some("Reticulum transport send failed".to_string()),
        );
    }

    /// Called by the platform when sending a Reticulum message fails, with an
    /// optional reason for diagnostics.
    pub fn reticulum_send_failed_with_reason(&self, message_id: String, reason: Option<String>) {
        {
            let mut protocol = recover_mutex(&self.inner, "inner");
            if let Err(err) = protocol.on_transport_send_failed(&message_id, reason) {
                tracing::warn!(
                    message_id = %message_id,
                    error = %err,
                    "Failed to apply welcome lifecycle transport failure (reticulum)"
                );
            }
        }
        self.with_transport(CoreTransportType::Reticulum, |rt| {
            rt.report_send_failure(&message_id);
        });
    }

    // ========================================================================
    // NOSTR TRANSPORT
    // ========================================================================

    /// Called by the platform when the Nostr relay connection status changes.
    pub fn nostr_status_changed(&self, is_connected: bool) -> Result<(), ProtocolError> {
        let was_connected = {
            let mut nostr_state = self.lock_nostr()?;
            let prev = nostr_state.is_connected;
            nostr_state.is_connected = is_connected;
            prev
        };

        self.with_transport_fallible(CoreTransportType::Nostr, |transport| {
            let new_status = if is_connected {
                offline_protocol_transport::TransportStatus::Available
            } else {
                offline_protocol_transport::TransportStatus::Disconnected
            };
            transport.on_status_changed(new_status);
        })?;

        if is_connected && !was_connected {
            let mut protocol = self.lock_inner()?;
            protocol.flush_outbox_all();
        }

        let event = if is_connected {
            CoreEvent::TransportSwitched {
                from: None,
                to: "Nostr".to_string(),
                reason: "Connected to Nostr relays".to_string(),
            }
        } else {
            CoreEvent::TransportSwitched {
                from: Some("Nostr".to_string()),
                to: "None".to_string(),
                reason: "Disconnected from Nostr relays".to_string(),
            }
        };
        self.emit_event(event);

        Ok(())
    }

    /// Called by the platform when data is received from a Nostr relay.
    ///
    /// Prefer [`Self::nostr_message_received_at`], which additionally carries
    /// the event's `created_at`. This entry leaves the receive watermark
    /// untouched, so a bridge that only calls it re-fetches the same slice of
    /// relay history on every reconnect.
    pub fn nostr_message_received(
        &self,
        sender_id: String,
        data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        self.nostr_message_received_inner(sender_id, data, None)
    }

    /// Called by the platform when data is received from a Nostr relay,
    /// carrying the relay event's `created_at` (unix **seconds**, straight from
    /// the event object).
    ///
    /// The timestamp advances this install's receive watermark, which is
    /// persisted and becomes the `since` of every subsequent subscription
    /// filter — that is what stops a relay from replaying its whole retention
    /// window on each reconnect. Pass the value verbatim; the core clamps it
    /// (non-positive and far-future values are ignored) and only ever moves the
    /// mark forward.
    ///
    /// The mark advances only for frames that decode into a protocol message,
    /// so junk addressed to this device's routing tag cannot push the window
    /// past history still owed to us.
    pub fn nostr_message_received_at(
        &self,
        sender_id: String,
        data: Vec<u8>,
        created_at: i64,
    ) -> Result<(), ProtocolError> {
        self.nostr_message_received_inner(sender_id, data, Some(created_at))
    }

    /// Shared body of the two Nostr receive entries.
    ///
    /// `sender_id` is the sender's Nostr pubkey hex, straight off the relay
    /// event. It has two jobs and neither is identification:
    ///
    /// 1. it is the ECDH input that unseals a gift wrap (for a sealed frame it
    ///    is a **single-use** key that names nobody at all); and
    /// 2. it is a diagnostic label in the logs.
    ///
    /// The identity surfaced via `NeighborDiscovered` is always the
    /// deserialized `Message.sender`; a frame that fails to deserialize
    /// surfaces no discovery at all.
    ///
    /// Unlike BLE/Reticulum, the Nostr pubkey is never the protocol user ID.
    /// Setting it as `transport_peer_id` would cause the security gate to
    /// reject every control message (identity mismatch). We therefore enqueue
    /// with `on_data_received` (no transport peer ID) and rely on the protocol-
    /// level signature check instead.
    fn nostr_message_received_inner(
        &self,
        sender_id: String,
        data: Vec<u8>,
        created_at: Option<i64>,
    ) -> Result<(), ProtocolError> {
        // Unseal before anything else looks at the bytes. This is the single
        // point where a gift wrap becomes a protocol frame: it is done once and
        // the plaintext reused, so the deserialize peek below and the enqueue
        // that follows can never disagree about what arrived. A frame that is
        // not sealed, or that is sealed to someone else, comes back unchanged
        // and takes the ordinary decode path.
        let data = self
            .with_transport(CoreTransportType::Nostr, |nt| {
                nt.as_any()
                    .downcast_ref::<offline_protocol_transport::nostr::NostrTransport>()
                    .map(|nostr| nostr.unseal_event_payload(&sender_id, &data).into_owned())
            })
            .flatten()
            .unwrap_or(data);

        // Extract the real sender from the protocol message before consuming `data`.
        // The Message.sender field is the authoritative user identity; the Nostr
        // pubkey (`sender_id`) is only a transport-level routing key.
        let real_sender: Option<String> = self
            .with_transport(CoreTransportType::Nostr, |nt| {
                nt.deserialize_message(&data)
                    .ok()
                    .map(|msg| msg.sender.as_str().to_string())
            })
            .flatten();

        // Use on_data_received (no transport_peer_id) because the Nostr pubkey
        // is the sender's install signing key, not the protocol user_id. Passing
        // the pubkey as transport_peer_id would cause validate_transport_sender
        // to reject the message due to the identity mismatch.
        if let Some(Err(e)) =
            self.with_transport_fallible(CoreTransportType::Nostr, |nt| nt.on_data_received(data))?
        {
            return Err(ProtocolError::TransportError(format!(
                "Failed to process nostr message: {}",
                e
            )));
        }

        {
            let mut protocol = self.lock_inner()?;
            while protocol.receive_message().is_some() {}

            // Gated on `real_sender`: the watermark means "receive progress has
            // reached here", and a frame that did not deserialize was never
            // processed. Counting it would let undecodable traffic addressed to
            // our (publicly derivable) routing tag drag the window past real
            // messages the relay still owes us.
            if let (Some(created_at), Some(_)) = (created_at, real_sender.as_ref()) {
                protocol.note_nostr_event_received(created_at);
            }
        }

        // Discovery must only ever surface the canonical protocol user id
        // (`Message.sender`): `NeighborDiscovered.peer_id` feeds
        // self-suppression, user blocking, and outbox flush, which all key
        // on that namespace. A frame that fails to deserialize has no such
        // id — and the Nostr pubkey is a per-install signing key, not an
        // identity — so it surfaces no discovery at all.
        let Some(peer_id) = real_sender else {
            tracing::warn!(
                nostr_pubkey = %sender_id,
                "Undecodable Nostr frame; skipping neighbor discovery"
            );
            return Ok(());
        };

        self.notify_neighbor_reachable(&peer_id, "Nostr", None)
    }

    /// Returns the next outgoing Nostr message, if any.
    ///
    /// The `event_json` field contains a complete, signed `["EVENT", {...}]`
    /// string. The platform should send it directly over the relay WebSocket,
    /// then call `nostr_confirm_sent()` or `nostr_send_failed()`.
    pub fn nostr_get_next_message(&self) -> Option<NostrMessage> {
        {
            let nostr_state = recover_mutex(&self.nostr_state, "nostr_state");
            if !nostr_state.is_connected {
                return None;
            }
        }

        self.with_nostr_transport(|nt| match nt.get_next_signed_event() {
            Ok(Some(signed)) => Some(NostrMessage {
                message_id: signed.message_id,
                event_id: signed.event_id,
                event_json: signed.event_json,
            }),
            Ok(None) => None,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "Failed to create signed Nostr event"
                );
                None
            }
        })
        .flatten()
    }

    /// Called by the platform after successfully publishing a Nostr event.
    pub fn nostr_confirm_sent(&self, message_id: String) {
        {
            let mut protocol = recover_mutex(&self.inner, "inner");
            if let Err(err) = protocol.on_transport_send_confirmed(&message_id) {
                tracing::warn!(
                    message_id = %message_id,
                    error = %err,
                    "Failed to apply welcome lifecycle transport confirmation (nostr)"
                );
            }
        }
        self.with_transport(CoreTransportType::Nostr, |nt| {
            nt.confirm_sent(&message_id);
        });
    }

    /// Called by the platform when publishing a Nostr event fails.
    pub fn nostr_send_failed(&self, message_id: String) {
        self.nostr_send_failed_with_reason(
            message_id,
            Some("Nostr transport send failed".to_string()),
        );
    }

    /// Called by the platform when publishing a Nostr event fails, with an
    /// optional reason for diagnostics.
    pub fn nostr_send_failed_with_reason(&self, message_id: String, reason: Option<String>) {
        {
            let mut protocol = recover_mutex(&self.inner, "inner");
            if let Err(err) = protocol.on_transport_send_failed(&message_id, reason) {
                tracing::warn!(
                    message_id = %message_id,
                    error = %err,
                    "Failed to apply welcome lifecycle transport failure (nostr)"
                );
            }
        }
        self.with_transport(CoreTransportType::Nostr, |nt| {
            nt.report_send_failure(&message_id);
        });
    }

    /// Returns this install's Nostr signing public key as a 64-char hex string.
    ///
    /// Used for display, and as the identity a peer seals gift wraps to once
    /// they have received our key package. Returns `None` if the Nostr
    /// transport is not configured.
    ///
    /// **No longer usable as a self-event filter.** With sealing on (the
    /// default) every event we publish is signed by a fresh single-use key, so
    /// comparing an inbound event's `pubkey` against this value never matches
    /// our own sealed traffic — and nothing else on a gift wrap identifies its
    /// author, by design. The bridges keep the comparison only for the legacy
    /// unsealed form; self-delivery is otherwise prevented by the `#p`
    /// subscription filter and, for self-addressed messages, by the engine's
    /// deduplication.
    ///
    /// The key is ephemeral until `initialize_mls` installs the persisted
    /// per-install signing secret, so read it after MLS initialization rather
    /// than caching it across that boundary.
    pub fn nostr_get_public_key(&self) -> Option<String> {
        self.with_nostr_transport(|nt| nt.public_key_hex())
    }

    /// Returns a NIP-01 subscription filter JSON for this device's routing tag
    /// (the public addressing label peers derive from our user ID).
    ///
    /// Send this to each relay after connecting:
    /// `["REQ", "<sub_id>", {"#p": ["<routing_tag>"], "kinds": [4]}]`
    pub fn nostr_get_subscription_filter(&self, subscription_id: String) -> Option<String> {
        self.with_nostr_transport(|nt| nt.create_subscription(&subscription_id).ok())
            .flatten()
    }

    // ========================================================================
    // TRANSPORT MANAGEMENT
    // ========================================================================

    /// Removes a transport
    pub fn remove_transport(&self, transport_type: TransportType) -> Result<(), ProtocolError> {
        let core_transport_type = match transport_type {
            TransportType::Internet => CoreTransportType::Internet,
            TransportType::Ble => CoreTransportType::BLE,
            TransportType::WiFiDirect => CoreTransportType::WiFiDirect,
            TransportType::Reticulum => CoreTransportType::Reticulum,
            TransportType::Nostr => CoreTransportType::Nostr,
        };

        let mut protocol = self.lock_inner()?;
        protocol
            .transport_manager_mut()
            .remove_transport(core_transport_type);
        Ok(())
    }

    /// Gets list of active transports
    pub fn get_active_transports(&self) -> Vec<String> {
        let protocol = recover_mutex(&self.inner, "inner");
        let transports = protocol.transport_manager().get_active_transports();
        transports.iter().map(|t| format!("{:?}", t)).collect()
    }

    /// **Legacy no-op — retained only for source/ABI compatibility.**
    ///
    /// This method predates the per-transport metrics tracking the Rust
    /// core now performs internally via `Transport::metrics()`. It has
    /// never written anywhere the SDK reads from, and the 12 extended
    /// optional fields introduced in the telemetry workstream are
    /// **also discarded**. Use `get_transport_metrics(transport_type)` to
    /// read live metrics, or install a `TelemetrySink` to observe the
    /// push stream (`MetricsFrame`). Do not build new integrations around
    /// this method; **it is scheduled for removal in the v1.0 release.**
    pub fn update_transport_metrics(
        &self,
        _transport_type: TransportType,
        _metrics: TransportMetrics,
    ) -> Result<(), ProtocolError> {
        // First-call warning so integrators wiring this method up don't
        // debug an invisible no-op. Uses `std::sync::Once` (via the static
        // below) so long-running relays don't spam the tracing layer — one
        // line per process lifetime is enough to surface the mistake.
        static WARN_ONCE: std::sync::Once = std::sync::Once::new();
        WARN_ONCE.call_once(|| {
            tracing::warn!(
                "update_transport_metrics is a no-op retained for source/ABI compat \
                 and discards every field (including the 12 extended optional ones). \
                 Read live metrics via `get_transport_metrics(...)` or install a \
                 TelemetrySink to observe `MetricsFrame` push updates. This method \
                 is scheduled for removal in the v1.0 release.",
            );
        });
        Ok(())
    }

    // ========================================================================
    // DORS DECISION SUPPORT
    // ========================================================================

    /// Checks if should escalate to WiFi
    pub fn should_escalate_to_wifi(&self) -> bool {
        let protocol = recover_mutex(&self.inner, "inner");
        protocol.transport_manager().should_escalate_to_wifi()
    }

    // ========================================================================
    // MEDIA AND FILE TRANSFER
    // ========================================================================

    /// Sends a media attachment through the protocol.
    ///
    /// The platform reads the file and passes the raw bytes. The SDK chunks
    /// the data, sends each chunk as a message (internet-preferred), and
    /// emits progress events.
    pub fn send_media(
        &self,
        recipient: String,
        file_data: Vec<u8>,
        file_name: String,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        let core_meta = media_metadata.map(CoreMediaMetadata::from);
        protocol
            .send_media(
                recipient,
                file_data,
                file_name,
                content_type.into(),
                core_meta,
            )
            .map_err(map_send_error)
    }

    /// Sends a media attachment with rich options: like `send_media`, plus
    /// caption, reply threading, quoted-reply context, forward attribution,
    /// and a caller-supplied `file_id`. Rich fields are sealed inside the
    /// chunk-0 MLS ciphertext for capable recipients and silently dropped
    /// otherwise — never sent cleartext.
    pub fn send_media_rich(
        &self,
        recipient: String,
        file_data: Vec<u8>,
        file_name: String,
        content_type: ContentType,
        options: MediaSendOptions,
    ) -> Result<String, ProtocolError> {
        // Validate the display-hint dictionaries into core types before
        // taking the protocol lock; a hostile identifier fails the call.
        let reply_context = options
            .reply_context
            .map(CoreReplyContext::try_from)
            .transpose()?;
        let forward_info = options
            .forward_info
            .map(CoreForwardInfo::try_from)
            .transpose()?;

        let core_options = CoreMediaSendOptions {
            media_metadata: options.media_metadata.map(CoreMediaMetadata::from),
            caption: options.caption,
            reply_to_msg: options.reply_to_msg,
            reply_context,
            forward_info,
            file_id: options.file_id,
        };

        let mut protocol = self.lock_inner()?;
        protocol
            .send_media_with(
                recipient,
                file_data,
                file_name,
                content_type.into(),
                core_options,
            )
            .map_err(map_send_error)
    }

    /// Convenience: sends a generic file (delegates to send_media with ContentType::File).
    pub fn send_file(
        &self,
        recipient: String,
        file_data: Vec<u8>,
        file_name: String,
    ) -> Result<String, ProtocolError> {
        self.send_media(recipient, file_data, file_name, ContentType::File, None)
    }

    /// Processes a received file chunk (manual path, for platforms handling
    /// their own chunk routing outside the protocol receive loop).
    ///
    /// Fails if the chunk is rejected: malformed or out-of-bounds fields,
    /// metadata that does not match the existing assembly, or a receive-path
    /// resource limit (all manual chunks share one pseudo-sender, so they are
    /// collectively subject to the per-sender concurrent-transfer quota and
    /// the global buffer budget). The error message carries the stable
    /// rejection reason. Once a transfer fails, its remaining chunks are
    /// rejected with reason `previously_failed`; retry the transfer under a
    /// fresh `file_id` (the failed id is re-admitted only after no chunk for
    /// it has been seen for the 300 s stale timeout).
    #[allow(clippy::too_many_arguments)]
    pub fn process_file_chunk(
        &self,
        file_id: String,
        chunk_index: u32,
        total_chunks: u32,
        file_size: u64,
        file_name: String,
        file_checksum: String,
        data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;

        use offline_protocol::file_transfer::{FileChunk, FileTransferManager};
        let chunk = FileChunk {
            file_id,
            file_name,
            file_size,
            total_chunks,
            chunk_index,
            chunk_data: data,
            file_checksum,
        };

        // The manual path carries no wire sender; all manual chunks share
        // the MANUAL_SENDER pseudo-identity for sender binding and quota.
        protocol
            .file_transfer_manager_mut()
            .process_chunk(FileTransferManager::MANUAL_SENDER, chunk)
            .map_err(map_chunk_rejection)?;
        Ok(())
    }

    /// Gets file transfer progress.
    pub fn get_file_progress(&self, file_id: String) -> Option<FileProgress> {
        let protocol = recover_mutex(&self.inner, "inner");
        let core_progress = protocol.file_transfer_manager().get_progress(&file_id)?;

        Some(FileProgress {
            file_id: core_progress.file_id,
            chunks_sent: core_progress.chunks_completed,
            total_chunks: core_progress.total_chunks,
            percentage: core_progress.percentage,
        })
    }

    /// Finalizes a file transfer, returning the reassembled bytes.
    pub fn finalize_file(&self, file_id: String) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        protocol
            .file_transfer_manager_mut()
            .finalize_file(&file_id)
            .ok_or_else(|| {
                ProtocolError::InvalidState("File not found or incomplete".to_string())
            })?;
        Ok(())
    }

    /// Cancels an active file transfer.
    pub fn cancel_file_transfer(&self, file_id: String) -> Result<(), ProtocolError> {
        let mut protocol = self.lock_inner()?;
        if protocol
            .file_transfer_manager_mut()
            .cancel_transfer(&file_id)
        {
            Ok(())
        } else {
            Err(ProtocolError::InvalidState(
                "File transfer not found".to_string(),
            ))
        }
    }

    // ========================================================================
    // NETWORK VISUALIZATION AND METRICS
    // ========================================================================

    /// Gets network topology
    pub fn get_topology(&self) -> Result<NetworkTopology, ProtocolError> {
        let visualizer = self.lock_visualizer()?;
        let core_topology = visualizer.get_topology();

        // Convert to uniffi types
        let nodes = core_topology
            .nodes
            .iter()
            .map(|n| NetworkNode {
                node_id: n.user_id.clone(),
                role: format!("{:?}", n.role),
                rssi: n.battery_level.map(|b| b as i16),
                last_seen_ms: n.last_seen as u64,
            })
            .collect();

        let links = core_topology
            .links
            .iter()
            .map(|l| NetworkLink {
                source_id: l.from.clone(),
                target_id: l.to.clone(),
                transport: format!("{:?}", l.transport),
                quality: l.quality,
            })
            .collect();

        let message_stats = vec![]; // Would need to be tracked separately

        Ok(NetworkTopology {
            nodes,
            links,
            message_stats,
        })
    }

    /// Gets message statistics
    pub fn get_message_stats(&self) -> Vec<MessageStats> {
        let visualizer = recover_mutex(&self.visualizer, "visualizer");
        let core_stats = visualizer.get_message_stats();

        core_stats
            .iter()
            .map(|s| MessageStats {
                message_id: s.message_id.clone(),
                sent_at_ms: s.sent_at as u64,
                delivered_at_ms: s.delivered_at.map(|t| t as u64),
                hop_count: s.hop_count,
                status: (if s.delivered_at.is_some() {
                    "delivered"
                } else {
                    "pending"
                })
                .to_string(),
            })
            .collect()
    }

    /// Gets delivery success rate
    pub fn get_delivery_success_rate(&self) -> f32 {
        let visualizer = recover_mutex(&self.visualizer, "visualizer");
        visualizer.delivery_success_rate()
    }

    /// Gets median latency
    pub fn get_median_latency(&self) -> u64 {
        let visualizer = recover_mutex(&self.visualizer, "visualizer");
        visualizer.median_latency().unwrap_or(0)
    }

    /// Gets median hop count
    pub fn get_median_hops(&self) -> u8 {
        let visualizer = recover_mutex(&self.visualizer, "visualizer");
        visualizer.median_hops().unwrap_or(0)
    }

    // ========================================================================
    // BATTERY AND DEVICE MANAGEMENT
    // ========================================================================

    /// Sets the battery level for relay decisions
    pub fn set_battery_level(&self, level: u8) {
        *recover_rwlock_write(&self.battery_level, "battery_level") = Some(level.min(100));
    }

    /// Gets the current battery level
    pub fn get_battery_level(&self) -> Option<u8> {
        *recover_rwlock_read(&self.battery_level, "battery_level")
    }

    // ========================================================================
    // RELAY MANAGEMENT
    // ========================================================================

    /// Sets the relay priority
    pub fn set_relay_priority(&self, priority: RelayPriority) -> Result<(), ProtocolError> {
        *self.write_relay_priority()? = priority;
        Ok(())
    }

    /// Gets the current relay priority
    pub fn get_relay_priority(&self) -> RelayPriority {
        *recover_rwlock_read(&self.relay_priority, "relay_priority")
    }

    /// Checks if this device is currently acting as a relay
    pub fn is_relay(&self) -> bool {
        // Check if we have enough connections and battery to be a relay
        let battery = self.get_battery_level();
        let ble_state = recover_mutex(&self.ble_state, "ble_state");
        let peer_count = ble_state.peer_count;
        drop(ble_state);

        match self.get_relay_priority() {
            RelayPriority::Low => false,
            RelayPriority::High => {
                // High priority: be a relay if we have at least one connection
                peer_count > 0 && battery.unwrap_or(100) > 20
            }
            RelayPriority::Medium => {
                // Medium priority: default threshold
                peer_count >= 3 && battery.unwrap_or(100) > 30
            }
        }
    }

    // ========================================================================
    // TRANSPORT METRICS
    // ========================================================================

    /// Gets detailed metrics for a specific transport. Pulls directly from
    /// the underlying `Transport::metrics()`; the same `TransportMetrics`
    /// shape also flows through the push path inside `MetricsFrame`.
    pub fn get_transport_metrics(&self, transport_type: TransportType) -> Option<TransportMetrics> {
        let protocol = self.lock_inner().ok()?;
        let core_type: CoreTransportType = transport_type.into();
        let transport_arc = protocol.transport_manager().get_transport(core_type)?;
        let metrics = transport_arc.metrics();
        Some(TransportMetrics::from(&metrics))
    }

    // ========================================================================
    // MANUAL TRANSPORT CONTROL
    // ========================================================================

    /// Forces the protocol to use a specific transport (overrides DORS)
    pub fn force_transport(&self, transport_type: TransportType) -> Result<(), ProtocolError> {
        *self.write_forced_transport()? = Some(transport_type);
        Ok(())
    }

    /// Releases the transport lock and lets DORS make decisions again
    pub fn release_transport_lock(&self) {
        *recover_rwlock_write(&self.forced_transport, "forced_transport") = None;
    }

    // ========================================================================
    // CONFIGURATION UPDATES
    // ========================================================================

    /// Updates DORS configuration at runtime
    pub fn update_dors_config(&self, config: DorsConfig) -> Result<(), ProtocolError> {
        // Store locally for retrieval
        *self.write_dors_config()? = Some(config.clone());

        // Convert to core DorsConfig and update the protocol
        let core_config = CoreDorsConfig {
            switch_hysteresis: config.switch_hysteresis,
            switch_cooldown_secs: config.switch_cooldown_secs,
            ble_to_wifi_retry_threshold: config.ble_to_wifi_retry_threshold,
            min_success_rate_before_escalation: config.min_success_rate_before_escalation,
            min_ble_samples_before_success_rate_escalation: config
                .min_ble_samples_before_success_rate_escalation
                as usize,
            rssi_switch_threshold: config.rssi_switch_threshold,
            congestion_queue_threshold: config.congestion_queue_threshold as usize,
            stability_window_secs: config.stability_window_secs,
            poor_signal_duration_secs: config.poor_signal_duration_secs,
            ttl_escalation_threshold: config.ttl_escalation_threshold,
            prefer_online: config.prefer_online,
            congestion_duration_secs: config.congestion_duration_secs,
            ttl_escalation_hold_secs: config.ttl_escalation_hold_secs,
            history_window_size: config.history_window_size as usize,
            queue_recovery_ratio: config.queue_recovery_ratio,
            // Use defaults for fields not exposed via uniffi
            low_battery_threshold: 20,
            relay_min_battery_level: 30,
            relay_optimal_connection_count: 4,
        };

        let mut protocol = self.lock_inner()?;
        protocol.update_dors_config(core_config);

        Ok(())
    }

    /// Gets the current DORS configuration
    pub fn get_dors_config(&self) -> DorsConfig {
        if let Some(config) = recover_rwlock_read(&self.dors_config, "dors_config").clone() {
            return config;
        }

        let core = CoreDorsConfig::default();
        DorsConfig {
            prefer_online: core.prefer_online,
            switch_hysteresis: core.switch_hysteresis,
            switch_cooldown_secs: core.switch_cooldown_secs,
            ble_to_wifi_retry_threshold: core.ble_to_wifi_retry_threshold,
            min_success_rate_before_escalation: core.min_success_rate_before_escalation,
            min_ble_samples_before_success_rate_escalation: core
                .min_ble_samples_before_success_rate_escalation
                as u64,
            rssi_switch_threshold: core.rssi_switch_threshold,
            congestion_queue_threshold: core.congestion_queue_threshold as u64,
            stability_window_secs: core.stability_window_secs,
            poor_signal_duration_secs: core.poor_signal_duration_secs,
            ttl_escalation_threshold: core.ttl_escalation_threshold,
            congestion_duration_secs: core.congestion_duration_secs,
            ttl_escalation_hold_secs: core.ttl_escalation_hold_secs,
            history_window_size: core.history_window_size as u64,
            queue_recovery_ratio: core.queue_recovery_ratio,
        }
    }

    // ========================================================================
    // GRADIENT ROUTING TABLE OPERATIONS
    // ========================================================================

    /// Learns a route from an incoming message.
    /// Call this when receiving a message from a neighbor to record that
    /// the neighbor can reach the message's original sender.
    pub fn learn_route(
        &self,
        destination: String,
        next_hop: String,
        hop_count: u8,
        quality: f32,
        sequence_number: u32,
    ) {
        let mut path_selector = recover_mutex(&self.path_selector, "path_selector");
        path_selector.routing_table_mut().learn_route(
            &destination,
            &next_hop,
            hop_count,
            quality,
            sequence_number,
        );
    }

    /// Gets the best (highest quality) route to a destination.
    /// Returns None if no route is known or all routes have expired.
    pub fn get_best_route(&self, destination: String) -> Option<RouteEntry> {
        let path_selector = recover_mutex(&self.path_selector, "path_selector");
        path_selector.get_route_to(&destination).map(|entry| {
            let elapsed = entry.last_seen.elapsed();
            let last_seen_ms = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
                .saturating_sub(elapsed.as_millis() as u64);

            RouteEntry {
                next_hop: entry.next_hop.clone(),
                hop_count: entry.hop_count,
                quality: entry.quality,
                last_seen_ms,
            }
        })
    }

    /// Gets all valid (non-expired) routes to a destination.
    /// Routes are returned in no particular order.
    pub fn get_all_routes(&self, destination: String) -> Vec<RouteEntry> {
        let mut path_selector = recover_mutex(&self.path_selector, "path_selector");
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        path_selector
            .routing_table_mut()
            .get_routes(&destination)
            .into_iter()
            .map(|entry| {
                let elapsed = entry.last_seen.elapsed();
                let last_seen_ms = now - (elapsed.as_millis() as u64);

                RouteEntry {
                    next_hop: entry.next_hop.clone(),
                    hop_count: entry.hop_count,
                    quality: entry.quality,
                    last_seen_ms,
                }
            })
            .collect()
    }

    /// Checks if a route exists to the destination.
    pub fn has_route(&self, destination: String) -> bool {
        let path_selector = recover_mutex(&self.path_selector, "path_selector");
        path_selector.has_route_to(&destination)
    }

    /// Removes all routes through a neighbor.
    /// Call this when a neighbor disconnects to clean up stale routes.
    pub fn remove_neighbor_routes(&self, neighbor_id: String) {
        let mut path_selector = recover_mutex(&self.path_selector, "path_selector");
        path_selector.remove_neighbor_routes(&neighbor_id);
    }

    /// Cleans up expired routes.
    /// Call this periodically (e.g., every 30 seconds) for maintenance.
    pub fn cleanup_expired_routes(&self) {
        let mut path_selector = recover_mutex(&self.path_selector, "path_selector");
        path_selector.cleanup_routes();
    }

    /// Gets routing table statistics for monitoring.
    pub fn get_routing_stats(&self) -> RoutingStats {
        let path_selector = recover_mutex(&self.path_selector, "path_selector");
        let (destination_count, route_count) = path_selector.routing_stats();

        RoutingStats {
            destination_count: destination_count as u32,
            route_count: route_count as u32,
        }
    }

    /// Updates the gradient routing configuration.
    pub fn update_routing_config(&self, config: GradientRoutingConfig) {
        let core_config = CoreGradientRoutingConfig {
            enabled: true,
            max_routes_per_destination: config.max_routes_per_destination as usize,
            route_ttl_secs: config.route_ttl_secs,
            max_routing_table_size: config.max_routing_table_size as usize,
        };

        // Create a new PathSelector with the updated routing config
        let mut path_selector = recover_mutex(&self.path_selector, "path_selector");
        let mut path_config = path_selector.config().clone();
        path_config.gradient_routing = core_config;
        *path_selector =
            PathSelector::with_config(path_config, offline_protocol_router::RelayManager::new());
    }

    /// Updates the ACK configuration at runtime.
    pub fn update_ack_config(&self, config: AckConfig) -> Result<(), ProtocolError> {
        let core_config = offline_protocol::AckConfig {
            default_timeout_ms: config.default_timeout_ms,
            max_pending_acks: config.max_pending_acks as usize,
        };
        let mut protocol = recover_mutex(&self.inner, "inner");
        protocol
            .update_ack_config(core_config)
            .map_err(ProtocolError::from)
    }

    /// Updates the retry configuration at runtime.
    pub fn update_retry_config(&self, config: RetryConfig) -> Result<(), ProtocolError> {
        let core_config = offline_protocol::RetryConfig {
            max_retries: config.max_retries,
            initial_delay_ms: config.initial_delay_ms,
            max_delay_ms: config.max_delay_ms,
            backoff_multiplier: config.backoff_multiplier,
            outbox_max_lifetime_ms: config.outbox_max_lifetime_ms,
            pending_message_max_lifetime_ms: config.pending_message_max_lifetime_ms,
        };
        let mut protocol = recover_mutex(&self.inner, "inner");
        protocol
            .update_retry_config(core_config)
            .map_err(ProtocolError::from)
    }

    /// Updates the deduplication configuration at runtime.
    pub fn update_dedup_config(&self, config: DedupConfig) -> Result<(), ProtocolError> {
        let core_config = offline_protocol::DeduplicatorConfig {
            max_tracked_messages: config.max_tracked_messages as usize,
            retention_time_secs: config.retention_time_secs,
            ..Default::default()
        };
        let mut protocol = recover_mutex(&self.inner, "inner");
        protocol
            .update_dedup_config(core_config)
            .map_err(ProtocolError::from)
    }

    /// Gets deduplicator statistics for monitoring.
    pub fn get_dedup_stats(&self) -> DedupStats {
        let protocol = recover_mutex(&self.inner, "inner");
        let stats = protocol.deduplicator_stats();
        DedupStats {
            total_tracked: stats.total_tracked as u64,
            recent_tracked: stats.recent_tracked as u64,
            capacity_used_percent: stats.capacity_used_percent,
            mode: format!("{:?}", stats.mode),
        }
    }

    /// Gets the number of pending ACKs.
    pub fn get_pending_ack_count(&self) -> u64 {
        let protocol = recover_mutex(&self.inner, "inner");
        protocol.pending_ack_count() as u64
    }

    /// Gets the retry queue size.
    pub fn get_retry_queue_size(&self) -> u64 {
        let protocol = recover_mutex(&self.inner, "inner");
        protocol.retry_queue_size() as u64
    }

    // ========================================================================
    // MLS (END-TO-END ENCRYPTION) OPERATIONS
    // ========================================================================

    /// Initialize MLS with separate secure and install-scoped state providers.
    pub fn initialize_mls(
        &self,
        secure_storage: Box<dyn MlsStorageProvider>,
        protocol_state_storage: Box<dyn ProtocolStateStorageProvider>,
    ) -> Result<(), ProtocolError> {
        let secure_wrapper = Arc::new(MlsStorageWrapper {
            provider: Arc::from(secure_storage),
        });
        let state_wrapper = Arc::new(ProtocolStateStorageWrapper {
            provider: Arc::from(protocol_state_storage),
        });

        // Single-authority lifecycle:
        // - CoreProtocol owns the only MlsManager instance for this runtime.
        // - UniFFI manual MLS APIs must route through that owner.
        // - Repeated calls are idempotent and never replace the existing manager.
        let mut protocol = self.lock_inner()?;
        if protocol.is_mls_initialized() {
            return Ok(());
        }
        protocol
            .initialize_mls(secure_wrapper, state_wrapper)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))?;
        Ok(())
    }

    /// Check if MLS is initialized
    pub fn is_mls_initialized(&self) -> bool {
        let protocol = recover_mutex(&self.inner, "inner");
        protocol.is_mls_initialized()
    }

    /// Returns the core-owned MLS manager handle.
    ///
    /// This is the only MLS state owner for the runtime. UniFFI must never
    /// create or cache an independent manager because that would diverge
    /// key-package/session/group state from auto-encryption flows.
    fn get_mls_manager(&self) -> Result<Arc<RwLock<CoreMlsManager>>, ProtocolError> {
        let protocol = self.lock_inner()?;
        protocol
            .mls_manager()
            .cloned()
            .ok_or(ProtocolError::MlsNotInitialized)
    }

    /// Generate a key package for distribution
    pub fn mls_generate_key_package(&self) -> Result<MlsKeyPackageBundle, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|e| ProtocolError::LockPoisoned(format!("mls_manager: {}", e)))?;
        guard
            .generate_key_package()
            .map(MlsKeyPackageBundle::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Get an existing key package or generate a new one
    pub fn mls_get_or_create_key_package(&self) -> Result<MlsKeyPackageBundle, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|e| ProtocolError::LockPoisoned(format!("mls_manager: {}", e)))?;
        guard
            .get_or_create_key_package()
            .map(MlsKeyPackageBundle::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Import a contact's key package
    pub fn mls_import_key_package(
        &self,
        user_id: String,
        key_package_data: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        // Routed through the protocol object rather than straight to the
        // MlsManager: the TOFU store lives on `OfflineProtocol`, and without it
        // this entry point imported a key package under any peer id with no
        // check against that peer's pinned signature key.
        let mut guard = self.lock_inner()?;
        guard
            .manual_mls_import_key_package(&user_id, &key_package_data)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Get pending key packages
    pub fn mls_get_pending_key_packages(&self) -> Vec<MlsKeyPackageBundle> {
        let manager = match self.get_mls_manager() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        let guard = match manager.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        guard
            .get_pending_key_packages()
            .unwrap_or_default()
            .into_iter()
            .map(MlsKeyPackageBundle::from)
            .collect()
    }

    /// Mark a key package as synced
    pub fn mls_mark_key_package_synced(&self, package_id: String) -> Result<(), ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|e| ProtocolError::LockPoisoned(format!("mls_manager: {}", e)))?;
        guard
            .mark_key_package_synced(&package_id)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Check if a 1:1 session exists
    pub fn mls_has_session(&self, other_user_id: String) -> bool {
        let manager = match self.get_mls_manager() {
            Ok(m) => m,
            Err(_) => return false,
        };
        let guard = match manager.read() {
            Ok(g) => g,
            Err(_) => return false,
        };
        guard.has_session(&other_user_id).unwrap_or(false)
    }

    /// Check if a pending key package is available for a peer
    pub fn has_pending_key_package(&self, peer_id: String) -> bool {
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => {
                return false;
            }
        };
        guard.has_pending_key_package(&peer_id)
    }

    /// Returns the current establishment state for a peer.
    pub fn get_establishment_state(
        &self,
        peer_id: String,
    ) -> Result<EstablishmentState, ProtocolError> {
        let guard = self.lock_inner()?;
        guard
            .get_establishment_state(&peer_id)
            .map(Into::into)
            .map_err(ProtocolError::from)
    }

    /// Establish a secure session with a peer (high-level API)
    ///
    /// This method handles the complete session establishment flow:
    /// - If session already exists, returns None
    /// - If a pending key package is available, imports it, creates session, sends Welcome
    /// - If no key package is available, returns SessionNotReady(state) so caller can retry
    pub fn establish_secure_session(
        &self,
        peer_id: String,
    ) -> Result<Option<MlsWelcomeMessage>, ProtocolError> {
        let mut guard = self.lock_inner()?;

        guard
            .establish_secure_session(&peer_id)
            .map(|opt| opt.map(MlsWelcomeMessage::from))
            .map_err(ProtocolError::from)
    }

    /// Create a 1:1 session
    pub fn mls_create_session(
        &self,
        other_user_id: String,
    ) -> Result<MlsWelcomeMessage, ProtocolError> {
        let mut guard = self.lock_inner()?;
        guard
            .manual_mls_create_session(&other_user_id)
            .map(MlsWelcomeMessage::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Join a session using a Welcome message
    pub fn mls_join_session(
        &self,
        welcome: MlsWelcomeMessage,
    ) -> Result<MlsGroupInfo, ProtocolError> {
        let core_welcome: CoreWelcomeMessage = welcome
            .try_into()
            .map_err(|e: offline_protocol_mls::MlsError| ProtocolError::MlsError(e.to_string()))?;
        let mut guard = self.lock_inner()?;
        guard
            .manual_mls_join_session(&core_welcome)
            .map(MlsGroupInfo::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Encrypt a message for a 1:1 session
    pub fn mls_encrypt_for_user(
        &self,
        other_user_id: String,
        plaintext: Vec<u8>,
    ) -> Result<MlsEncryptedMessage, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|e| ProtocolError::LockPoisoned(format!("mls_manager: {}", e)))?;
        guard
            .encrypt_for_user(&other_user_id, &plaintext)
            .map(MlsEncryptedMessage::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Decrypt a message from a 1:1 session
    pub fn mls_decrypt_from_user(
        &self,
        encrypted: MlsEncryptedMessage,
    ) -> Result<Option<Vec<u8>>, ProtocolError> {
        let core_encrypted: CoreEncryptedMessage = encrypted
            .try_into()
            .map_err(|e: offline_protocol_mls::MlsError| ProtocolError::MlsError(e.to_string()))?;
        let mut guard = self.lock_inner()?;
        guard
            .manual_mls_decrypt_from_user(&core_encrypted)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// List all active 1:1 sessions
    pub fn mls_list_sessions(&self) -> Vec<String> {
        let manager = match self.get_mls_manager() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        let guard = match manager.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        guard.list_sessions().unwrap_or_default()
    }

    /// Delete a 1:1 session
    pub fn mls_delete_session(&self, other_user_id: String) -> Result<(), ProtocolError> {
        let mut guard = self.lock_inner()?;
        guard
            .manual_mls_delete_session(&other_user_id)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Get a pending Welcome message
    pub fn mls_get_pending_welcome(&self, other_user_id: String) -> Option<MlsWelcomeMessage> {
        let manager = self.get_mls_manager().ok()?;
        let guard = manager.read().ok()?;
        guard
            .get_pending_welcome(&other_user_id)
            .ok()
            .flatten()
            .map(MlsWelcomeMessage::from)
    }

    /// Clear a pending Welcome message
    pub fn mls_clear_pending_welcome(&self, other_user_id: String) -> Result<(), ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|e| ProtocolError::LockPoisoned(format!("mls_manager: {}", e)))?;
        guard
            .clear_pending_welcome(&other_user_id)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Decrypt any encrypted message
    pub fn mls_decrypt(
        &self,
        encrypted: MlsEncryptedMessage,
    ) -> Result<Option<Vec<u8>>, ProtocolError> {
        let core_encrypted: CoreEncryptedMessage = encrypted
            .try_into()
            .map_err(|e: offline_protocol_mls::MlsError| ProtocolError::MlsError(e.to_string()))?;
        let mut guard = self.lock_inner()?;
        guard
            .manual_mls_decrypt(&core_encrypted)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Process a Welcome message
    pub fn mls_process_welcome(
        &self,
        welcome: MlsWelcomeMessage,
    ) -> Result<MlsGroupInfo, ProtocolError> {
        let core_welcome: CoreWelcomeMessage = welcome
            .try_into()
            .map_err(|e: offline_protocol_mls::MlsError| ProtocolError::MlsError(e.to_string()))?;
        let mut guard = self.lock_inner()?;
        guard
            .manual_mls_process_welcome(&core_welcome)
            .map(MlsGroupInfo::from)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    // ========================================================================
    // IDENTITY AND SIGNING OPERATIONS
    // ========================================================================

    /// Get the identity public key (Ed25519, 32 bytes).
    ///
    /// This is the public key used for MLS operations and can be shared with others
    /// to establish identity and verify signatures.
    pub fn get_identity_public_key(&self) -> Result<Vec<u8>, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|e| ProtocolError::LockPoisoned(format!("mls_manager: {}", e)))?;
        guard
            .get_identity_public_key()
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Derive a deterministic user ID from a public key.
    ///
    /// Returns a base58-encoded string derived from `SHA-256(publicKey)[0:20]`.
    /// The same public key always produces the same user ID.
    pub fn derive_user_id_from_public_key(&self, public_key: Vec<u8>) -> String {
        CoreMlsManager::derive_user_id_from_public_key(&public_key)
    }

    /// Sign arbitrary data with the identity private key (Ed25519).
    ///
    /// Returns the signature as raw bytes (64 bytes).
    pub fn sign_data(&self, data: Vec<u8>) -> Result<Vec<u8>, ProtocolError> {
        let manager = self.get_mls_manager()?;
        let guard = manager
            .read()
            .map_err(|e| ProtocolError::LockPoisoned(format!("mls_manager: {}", e)))?;
        guard
            .sign_data(&data)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    /// Verify a signature against a public key.
    ///
    /// Returns true if the signature is valid, false otherwise.
    pub fn verify_signature(
        &self,
        public_key: Vec<u8>,
        data: Vec<u8>,
        signature: Vec<u8>,
    ) -> Result<bool, ProtocolError> {
        CoreMlsManager::verify_signature(&public_key, &data, &signature)
            .map_err(|e| ProtocolError::MlsError(e.to_string()))
    }

    // ========================================================================
    // PRESENCE, TYPING INDICATORS, AND READ RECEIPTS
    // ========================================================================

    /// Send a presence update to a peer via the protocol (routed through DORS).
    pub fn send_presence_update(
        &self,
        recipient: String,
        status: PresenceStatus,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        let message_id = protocol
            .send_presence_update(&recipient, status.into())
            .map_err(map_send_error)?;
        Ok(message_id.as_str())
    }

    /// Send a typing indicator to a peer via the protocol (routed through DORS).
    /// For direct messages, conversation_id should be the recipient's username.
    /// For group chats, conversation_id should be the group_id.
    pub fn send_typing_indicator(
        &self,
        recipient: String,
        conversation_id: String,
        is_typing: bool,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        let message_id = protocol
            .send_typing_indicator(&recipient, &conversation_id, is_typing)
            .map_err(map_send_error)?;
        Ok(message_id.as_str())
    }

    /// Send a read receipt to a peer via the protocol (routed through DORS).
    /// Indicates that the given messages have been read.
    pub fn send_read_receipt(
        &self,
        recipient: String,
        message_ids: Vec<String>,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.lock_inner()?;
        let message_id = protocol
            .send_read_receipt(&recipient, message_ids)
            .map_err(map_send_error)?;
        Ok(message_id.as_str())
    }

    // ========================================================================
    // RELAY SERVER API (JSON payload formatters for WebSocket relay)
    // ========================================================================

    /// Check if a user is online via relay server.
    /// Returns JSON string to send via WebSocket relay.
    pub fn check_presence(&self, username: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "CheckPresence",
            "username": username
        });
        serde_json::to_string(&payload).map_err(|e| {
            ProtocolError::SerializationError(format!("Failed to serialize CheckPresence: {}", e))
        })
    }

    /// Request prekey bundle for a user to establish encrypted communication.
    /// Returns JSON string to send via WebSocket relay.
    pub fn request_prekey_bundle(&self, username: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "RequestPreKeyBundle",
            "username": username
        });
        serde_json::to_string(&payload).map_err(|e| {
            ProtocolError::SerializationError(format!(
                "Failed to serialize RequestPreKeyBundle: {}",
                e
            ))
        })
    }

    /// Upload identity key and prekeys for Signal Protocol.
    /// Returns JSON string to send via WebSocket relay.
    pub fn upload_keys(
        &self,
        identity_key: String,
        signed_prekey_json: String,
        one_time_prekeys_json: String,
    ) -> Result<String, ProtocolError> {
        let signed_prekey: serde_json::Value =
            serde_json::from_str(&signed_prekey_json).map_err(|e| {
                ProtocolError::InvalidArgument(format!("Failed to parse signed_prekey JSON: {}", e))
            })?;

        let one_time_prekeys: Vec<serde_json::Value> = serde_json::from_str(&one_time_prekeys_json)
            .map_err(|e| {
                ProtocolError::InvalidArgument(format!(
                    "Failed to parse one_time_prekeys JSON: {}",
                    e
                ))
            })?;

        let payload = serde_json::json!({
            "type": "UploadKeys",
            "identity_key": identity_key,
            "signed_prekey": signed_prekey,
            "one_time_prekeys": one_time_prekeys
        });
        serde_json::to_string(&payload).map_err(|e| {
            ProtocolError::SerializationError(format!("Failed to serialize UploadKeys: {}", e))
        })
    }

    /// Set typing indicator via relay server (JSON payload formatter).
    /// Returns JSON string to send via WebSocket relay.
    pub fn set_typing(&self, conversation_id: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "SetTyping",
            "conversation_id": conversation_id
        });
        serde_json::to_string(&payload).map_err(|e| {
            ProtocolError::SerializationError(format!("Failed to serialize SetTyping: {}", e))
        })
    }

    /// Clear typing indicator via relay server (JSON payload formatter).
    /// Returns JSON string to send via WebSocket relay.
    pub fn clear_typing(&self, conversation_id: String) -> Result<String, ProtocolError> {
        let payload = serde_json::json!({
            "type": "ClearTyping",
            "conversation_id": conversation_id
        });
        serde_json::to_string(&payload).map_err(|e| {
            ProtocolError::SerializationError(format!("Failed to serialize ClearTyping: {}", e))
        })
    }

    // ========================================================================
    // GROUP MESSAGING (MLS-encrypted, transport-agnostic)
    // ========================================================================

    /// Create a new MLS group.
    pub fn create_group(&self, group_name: String) -> Result<MlsGroupInfo, ProtocolError> {
        let mut guard = self.lock_inner()?;
        guard
            .create_group(&group_name)
            .map(MlsGroupInfo::from)
            .map_err(ProtocolError::from)
    }

    /// Send an MLS-encrypted message to all group members.
    pub fn send_group_message(
        &self,
        group_id: String,
        content: String,
        priority: Option<MessagePriority>,
        reply_to_msg: Option<String>,
    ) -> Result<Vec<String>, ProtocolError> {
        let core_priority = priority.map(|p| p.into());
        let mut guard = self.lock_inner()?;
        guard
            .send_group_message(&group_id, &content, core_priority, reply_to_msg.as_deref())
            .map(|ids| ids.into_iter().map(|id| id.as_str().to_string()).collect())
            .map_err(map_send_error)
    }

    /// Forward a message to all members of a group with forwarding attribution.
    pub fn forward_message_to_group(
        &self,
        original_message_json: String,
        group_id: String,
        priority: Option<MessagePriority>,
    ) -> Result<Vec<String>, ProtocolError> {
        let original: offline_protocol_core::Message = serde_json::from_str(&original_message_json)
            .map_err(|e| {
                ProtocolError::InvalidConfiguration(format!(
                    "Failed to parse original message JSON: {}",
                    e
                ))
            })?;
        let core_priority = priority.map(|p| p.into());
        let mut guard = self.lock_inner()?;
        guard
            .forward_message_to_group(&original, &group_id, core_priority)
            .map(|ids| ids.into_iter().map(|id| id.as_str().to_string()).collect())
            .map_err(map_send_error)
    }

    /// Invite a user to an MLS group.
    pub fn invite_to_group(
        &self,
        group_id: String,
        invitee_user_id: String,
    ) -> Result<(), ProtocolError> {
        let mut guard = self.lock_inner()?;
        guard
            .invite_to_group(&group_id, &invitee_user_id)
            .map_err(ProtocolError::from)
    }

    /// Remove a member from an MLS group.
    pub fn remove_from_group(
        &self,
        group_id: String,
        member_id: String,
    ) -> Result<(), ProtocolError> {
        let mut guard = self.lock_inner()?;
        guard
            .remove_from_group(&group_id, &member_id)
            .map_err(ProtocolError::from)
    }

    /// Leave an MLS group.
    pub fn leave_group(&self, group_id: String) -> Result<(), ProtocolError> {
        let mut guard = self.lock_inner()?;
        // map_send_error: total failure of the leave-notification fan-out is
        // a send failure (retryable), same as send_group_message.
        guard.leave_group(&group_id).map_err(map_send_error)
    }

    /// List all MLS groups (excluding 1:1 sessions).
    pub fn list_groups(&self) -> Result<Vec<String>, ProtocolError> {
        let guard = self.lock_inner()?;
        guard.list_groups().map_err(ProtocolError::from)
    }

    /// Get information about an MLS group.
    pub fn get_group_info(&self, group_id: String) -> Result<Option<MlsGroupInfo>, ProtocolError> {
        let guard = self.lock_inner()?;
        Ok(guard
            .get_group_info(&group_id)
            .map_err(ProtocolError::from)?
            .map(MlsGroupInfo::from))
    }

    /// Whether a rich group send right now would seal its extras, and which
    /// members hold the gate closed (point-in-time, advisory).
    pub fn group_rich_readiness(
        &self,
        group_id: String,
    ) -> Result<GroupRichReadiness, ProtocolError> {
        let guard = self.lock_inner()?;
        guard
            .group_rich_readiness(&group_id)
            .map(GroupRichReadiness::from)
            .map_err(ProtocolError::from)
    }

    /// Relay-side registration state of a group (point-in-time; transitions
    /// surface as the `group_relay_sync_changed` event).
    pub fn group_relay_sync_state(
        &self,
        group_id: String,
    ) -> Result<RelaySyncState, ProtocolError> {
        let guard = self.lock_inner()?;
        Ok(guard.group_relay_sync_state(&group_id).into())
    }

    /// Register (or re-register) a group with the relay server on demand.
    /// Outcome arrives as `group_relay_sync_changed`; returns true when the
    /// frame was queued or the group is already synced.
    pub fn request_group_relay_registration(
        &self,
        group_id: String,
    ) -> Result<bool, ProtocolError> {
        let mut guard = self.lock_inner()?;
        guard
            .request_group_relay_registration(&group_id)
            .map_err(ProtocolError::from)
    }

    /// Set a member's role in a group (admin only).
    /// `role` must be `"admin"` or `"member"`.
    pub fn set_member_role(
        &self,
        group_id: String,
        user_id: String,
        role: String,
    ) -> Result<(), ProtocolError> {
        let parsed_role: offline_protocol_mls::GroupRole =
            role.parse()
                .map_err(|e: offline_protocol_mls::ParseGroupRoleError| {
                    ProtocolError::InvalidArgument(e.to_string())
                })?;
        let mut guard = self.lock_inner()?;
        guard
            .set_member_role(&group_id, &user_id, parsed_role)
            .map_err(ProtocolError::from)
    }

    /// Get a member's role in a group.
    /// Returns `"admin"` or `"member"`.
    pub fn get_member_role(
        &self,
        group_id: String,
        user_id: String,
    ) -> Result<String, ProtocolError> {
        let guard = self.lock_inner()?;
        guard
            .get_member_role(&group_id, &user_id)
            .map(|r| r.to_string())
            .map_err(ProtocolError::from)
    }

    /// Get all member roles in a group.
    /// Returns a map of user_id -> role string (`"admin"` or `"member"`).
    pub fn get_group_roles(
        &self,
        group_id: String,
    ) -> Result<std::collections::HashMap<String, String>, ProtocolError> {
        let guard = self.lock_inner()?;
        guard
            .get_group_roles(&group_id)
            .map(|roles| roles.into_iter().map(|(k, v)| (k, v.to_string())).collect())
            .map_err(ProtocolError::from)
    }

    /// Rename a group (admin only, broadcasts to all members).
    pub fn rename_group(&self, group_id: String, new_name: String) -> Result<(), ProtocolError> {
        let mut guard = self.lock_inner()?;
        guard
            .rename_group(&group_id, &new_name)
            .map_err(ProtocolError::from)
    }

    // ========================================================================
    // TOFU MANAGEMENT
    // ========================================================================

    /// Reset the TOFU-pinned public key for a peer, allowing re-pinning on next contact.
    /// Returns `true` if an entry was removed, `false` if no entry existed (idempotent).
    pub fn reset_tofu_for_peer(&self, peer_id: String) -> Result<bool, ProtocolError> {
        let mut guard = self.lock_inner()?;
        Ok(guard.reset_tofu_for_peer(&peer_id))
    }

    // ========================================================================
    // USER BLOCKING
    // ========================================================================

    /// Block a user (silently drops all their messages, no notification sent).
    pub fn block_user(&self, user_id: String) -> Result<(), ProtocolError> {
        let mut guard = self.lock_inner()?;
        guard.block_user(&user_id).map_err(ProtocolError::from)
    }

    /// Unblock a previously blocked user.
    pub fn unblock_user(&self, user_id: String) -> Result<(), ProtocolError> {
        let mut guard = self.lock_inner()?;
        guard.unblock_user(&user_id).map_err(ProtocolError::from)
    }

    /// Get list of currently blocked user IDs.
    pub fn get_blocked_users(&self) -> Result<Vec<String>, ProtocolError> {
        let guard = self.lock_inner()?;
        Ok(guard.get_blocked_users())
    }

    /// Check if a user is currently blocked.
    pub fn is_user_blocked(&self, user_id: String) -> Result<bool, ProtocolError> {
        let guard = self.lock_inner()?;
        Ok(guard.is_user_blocked(&user_id))
    }
}

/// Standalone mesh services interface for UniFFI.
///
/// Holds an `Arc<OfflineProtocol>` and delegates through public wrapper methods,
/// avoiding direct exposure of internal synchronization primitives.
pub struct MeshServices {
    protocol: Arc<OfflineProtocol>,
}

impl MeshServices {
    /// Creates a MeshServices instance sharing the given protocol's state.
    pub fn new(protocol: Arc<OfflineProtocol>) -> Result<Self, ProtocolError> {
        Ok(Self { protocol })
    }

    /// Registers a local service that this node offers for discovery.
    pub fn register_service(
        &self,
        service_id: String,
        version: String,
        capabilities: HashMap<String, String>,
    ) -> Result<(), ProtocolError> {
        self.protocol
            .svc_register_service(service_id, version, capabilities)
    }

    /// Unregisters a local service. Returns true if found and removed.
    pub fn unregister_service(&self, service_id: String) -> Result<bool, ProtocolError> {
        self.protocol.svc_unregister_service(service_id)
    }

    /// Broadcasts a service discovery query. Returns a query_id.
    pub fn discover_services(&self, service_id: Option<String>) -> Result<String, ProtocolError> {
        self.protocol.svc_discover_services(service_id)
    }

    /// Sends a service request to a specific provider peer. Returns a request_id.
    pub fn send_service_request(
        &self,
        provider: String,
        service_id: String,
        method: String,
        body: String,
    ) -> Result<String, ProtocolError> {
        self.protocol
            .svc_send_service_request(provider, service_id, method, body)
    }

    /// Responds to a service request from another peer.
    pub fn respond_to_service_request(
        &self,
        request_id: String,
        requester: String,
        service_id: String,
        status: String,
        body: String,
    ) -> Result<String, ProtocolError> {
        self.protocol
            .svc_respond_to_service_request(request_id, requester, service_id, status, body)
    }
}

#[cfg(test)]
mod error_mapping_tests {
    use super::{map_chunk_rejection, map_send_error, ProtocolError};
    use offline_protocol::Error as E;
    use offline_protocol_core::Error as CoreErr;
    use offline_protocol_services::ServiceError;
    use offline_protocol_transport::Error as TransportErr;

    #[test]
    fn from_maps_engine_error_classes() {
        let cases: Vec<(E, fn(&ProtocolError) -> bool)> = vec![
            (E::Transport(TransportErr::SendFailed("boom".into())), |e| {
                matches!(e, ProtocolError::TransportError(_))
            }),
            (E::Serialization("bad json".into()), |e| {
                matches!(e, ProtocolError::SerializationError(_))
            }),
            (
                E::Service(ServiceError::PayloadTooLarge("query".into())),
                |e| matches!(e, ProtocolError::ServiceError(_)),
            ),
            (E::GroupNotFound("g1".into()), |e| {
                matches!(e, ProtocolError::GroupNotFound(_))
            }),
            // MLS-layer group-missing folds into GroupNotFound; every other
            // MLS error keeps the MlsError class.
            (
                E::Mls(offline_protocol_mls::MlsError::GroupNotFound("g1".into())),
                |e| matches!(e, ProtocolError::GroupNotFound(_)),
            ),
            (
                E::Mls(offline_protocol_mls::MlsError::Encryption("boom".into())),
                |e| matches!(e, ProtocolError::MlsError(_)),
            ),
            (
                E::PermissionDenied("Only admins can invite members".into()),
                |e| matches!(e, ProtocolError::PermissionDenied(_)),
            ),
            (
                E::InvalidState("Cannot demote the last admin".into()),
                |e| matches!(e, ProtocolError::InvalidState(_)),
            ),
            (
                E::InvalidArgument("Group name cannot be empty".into()),
                |e| matches!(e, ProtocolError::InvalidArgument(_)),
            ),
            (E::Other("misc".into()), |e| {
                matches!(e, ProtocolError::Other(_))
            }),
        ];
        for (input, check) in cases {
            let debug = format!("{:?}", input);
            let mapped = ProtocolError::from(input);
            assert!(check(&mapped), "wrong mapping for {debug}: got {mapped:?}");
        }
    }

    #[test]
    fn from_splits_core_errors_by_class() {
        let ser = ProtocolError::from(E::Core(CoreErr::SerializationError("x".into())));
        assert!(matches!(ser, ProtocolError::SerializationError(_)));
        let de = ProtocolError::from(E::Core(CoreErr::DeserializationError("x".into())));
        assert!(matches!(de, ProtocolError::SerializationError(_)));
        let arg = ProtocolError::from(E::Core(CoreErr::InvalidUserId("bad id".into())));
        assert!(matches!(arg, ProtocolError::InvalidArgument(_)));
        let ttl = ProtocolError::from(E::Core(CoreErr::InvalidTTL(0)));
        assert!(matches!(ttl, ProtocolError::InvalidArgument(_)));
        let other = ProtocolError::from(E::Core(CoreErr::Other("misc".into())));
        assert!(matches!(other, ProtocolError::Other(_)));
    }

    #[test]
    fn send_paths_map_transport_failures_to_send_failed() {
        let sent = map_send_error(E::Transport(TransportErr::SendFailed(
            "All transports failed".into(),
        )));
        assert!(matches!(sent, ProtocolError::SendFailed(_)));
        // The transport-layer "Send failed: " prefix is unwrapped — the
        // FFI variant's own display already announces the send failure.
        assert_eq!(
            sent.to_string(),
            "Failed to send message: All transports failed"
        );

        // Any transport-layer failure on a send path becomes SendFailed,
        // not just TransportErr::SendFailed.
        let unreachable =
            map_send_error(E::Transport(TransportErr::PeerNotReachable("bob".into())));
        assert!(matches!(unreachable, ProtocolError::SendFailed(_)));

        // Non-transport errors keep their class on send paths.
        let group = map_send_error(E::GroupNotFound("g1".into()));
        assert!(matches!(group, ProtocolError::GroupNotFound(_)));
        let mls = map_send_error(E::MlsNotInitialized);
        assert!(matches!(mls, ProtocolError::MlsNotInitialized));
    }

    #[test]
    fn chunk_rejections_split_receiver_state_from_bad_input() {
        use offline_protocol::file_transfer::ChunkRejection as R;
        // Receiver-side resource conditions and failed-transfer tombstones
        // are retryable with a fresh transfer — they must not surface as
        // permanent input errors.
        for rejection in [
            R::TooManyTransfers,
            R::SenderQuotaExceeded,
            R::BufferBudgetExhausted,
            R::PreviouslyFailed,
        ] {
            assert!(
                matches!(
                    map_chunk_rejection(rejection),
                    ProtocolError::InvalidState(_)
                ),
                "expected InvalidState for {rejection:?}"
            );
        }
        // Malformed or mismatched chunks are caller-input defects.
        for rejection in [R::Invalid, R::MetadataMismatch, R::SenderMismatch] {
            assert!(
                matches!(
                    map_chunk_rejection(rejection),
                    ProtocolError::InvalidArgument(_)
                ),
                "expected InvalidArgument for {rejection:?}"
            );
        }
    }

    #[test]
    fn udl_protocol_error_variant_order_is_pinned() {
        // Generated Swift/Kotlin/Python bindings decode ProtocolError by
        // positional discriminant, so the UDL enum is append-only. If this
        // fails you either reordered/removed a variant (never do that) or
        // appended one — extend this list and regenerate every committed
        // binding.
        const EXPECTED: [&str; 20] = [
            "NotStarted",
            "AlreadyStarted",
            "InvalidConfiguration",
            "SendFailed",
            "NoKeyPackage",
            "SessionNotReady",
            "EncryptFailed",
            "InvalidState",
            "MlsNotInitialized",
            "MlsError",
            "UserBlocked",
            "MediaTransferLimit",
            "LockPoisoned",
            "Other",
            "TransportError",
            "SerializationError",
            "ServiceError",
            "GroupNotFound",
            "PermissionDenied",
            "InvalidArgument",
        ];
        let udl = include_str!("offline_protocol.udl");
        let enum_body = udl
            .split_once("enum ProtocolError {")
            .expect("ProtocolError enum missing from UDL")
            .1
            .split_once('}')
            .expect("unterminated ProtocolError enum in UDL")
            .0;
        let variants: Vec<&str> = enum_body.split('"').skip(1).step_by(2).collect();
        assert_eq!(variants, EXPECTED, "ProtocolError UDL variants drifted");
    }

    #[test]
    fn transport_error_display_keeps_engine_prefix() {
        // Callers that only look at the message must see the same
        // "Transport error: ..." text the old catch-all produced.
        let mapped = ProtocolError::from(E::Transport(TransportErr::SendFailed("boom".into())));
        assert_eq!(mapped.to_string(), "Transport error: Send failed: boom");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::thread;

    #[derive(Default)]
    struct TestMlsStorageProvider {
        data: Mutex<HashMap<(String, String), Vec<u8>>>,
    }

    impl MlsStorageProvider for TestMlsStorageProvider {
        fn store(
            &self,
            key_type: String,
            key_id: String,
            data: Vec<u8>,
        ) -> Result<(), MlsStorageError> {
            let mut guard = self.data.lock().map_err(|_| MlsStorageError::StoreFailed)?;
            guard.insert((key_type, key_id), data);
            Ok(())
        }

        fn load(
            &self,
            key_type: String,
            key_id: String,
        ) -> Result<Option<Vec<u8>>, MlsStorageError> {
            let guard = self.data.lock().map_err(|_| MlsStorageError::LoadFailed)?;
            Ok(guard.get(&(key_type, key_id)).cloned())
        }

        fn delete(&self, key_type: String, key_id: String) -> Result<(), MlsStorageError> {
            let mut guard = self
                .data
                .lock()
                .map_err(|_| MlsStorageError::DeleteFailed)?;
            guard.remove(&(key_type, key_id));
            Ok(())
        }

        fn list_keys(&self, key_type: String) -> Result<Vec<String>, MlsStorageError> {
            let guard = self.data.lock().map_err(|_| MlsStorageError::LoadFailed)?;
            Ok(guard
                .keys()
                .filter_map(|(stored_type, key_id)| {
                    if stored_type == &key_type {
                        Some(key_id.clone())
                    } else {
                        None
                    }
                })
                .collect())
        }
    }

    impl ProtocolStateStorageProvider for TestMlsStorageProvider {
        fn store(
            &self,
            key_type: String,
            key_id: String,
            data: Vec<u8>,
        ) -> Result<(), MlsStorageError> {
            MlsStorageProvider::store(self, key_type, key_id, data)
        }

        fn load(
            &self,
            key_type: String,
            key_id: String,
        ) -> Result<Option<Vec<u8>>, MlsStorageError> {
            MlsStorageProvider::load(self, key_type, key_id)
        }

        fn delete(&self, key_type: String, key_id: String) -> Result<(), MlsStorageError> {
            MlsStorageProvider::delete(self, key_type, key_id)
        }

        fn list_keys(&self, key_type: String) -> Result<Vec<String>, MlsStorageError> {
            MlsStorageProvider::list_keys(self, key_type)
        }
    }

    fn create_test_config() -> ProtocolConfig {
        ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: true,
            internet_enabled: true,
            reticulum_enabled: false,
            nostr_enabled: false,
            prefer_online: false,
            initial_ttl: 8,
            encryption_enabled: true,
            auto_key_exchange: true,
            store_pending: true,
            require_encryption: false,
            max_pending_per_peer: 64,
            max_pending_global: 4096,
            pending_ttl_ms: DEFAULT_PENDING_TTL_MS,
            overflow_policy: OverflowPolicy::DropOldest,
            max_group_members: 256,
            group_relay_enabled: true,
            group_relay_broadcast_enabled: true,
            group_enforce_admin_commits: false,
            require_transport_identity: false,
        }
    }

    fn create_ble_only_config() -> ProtocolConfig {
        ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: true,
            wifi_direct_enabled: false,
            internet_enabled: false,
            reticulum_enabled: false,
            nostr_enabled: false,
            prefer_online: false,
            initial_ttl: 8,
            encryption_enabled: true,
            auto_key_exchange: true,
            store_pending: true,
            require_encryption: false,
            max_pending_per_peer: 64,
            max_pending_global: 4096,
            pending_ttl_ms: DEFAULT_PENDING_TTL_MS,
            overflow_policy: OverflowPolicy::DropOldest,
            max_group_members: 256,
            group_relay_enabled: true,
            group_relay_broadcast_enabled: true,
            group_enforce_admin_commits: false,
            require_transport_identity: false,
        }
    }

    #[test]
    fn test_mls_initialize_is_idempotent() {
        let config = create_test_config();
        let protocol = OfflineProtocol::new(config).unwrap();

        protocol
            .initialize_mls(
                Box::new(TestMlsStorageProvider::default()),
                Box::new(TestMlsStorageProvider::default()),
            )
            .unwrap();
        let first_handle = {
            let guard = protocol.inner.lock().unwrap();
            guard.mls_manager().cloned().unwrap()
        };

        protocol
            .initialize_mls(
                Box::new(TestMlsStorageProvider::default()),
                Box::new(TestMlsStorageProvider::default()),
            )
            .unwrap();
        let second_handle = {
            let guard = protocol.inner.lock().unwrap();
            guard.mls_manager().cloned().unwrap()
        };

        assert!(protocol.is_mls_initialized());
        assert!(Arc::ptr_eq(&first_handle, &second_handle));
        assert!(protocol.mls_generate_key_package().is_ok());
    }

    #[test]
    fn test_mls_initialize_is_race_safe_single_instance() {
        let protocol = Arc::new(OfflineProtocol::new(create_test_config()).unwrap());
        let mut join_handles = Vec::new();

        for _ in 0..8 {
            let protocol_clone = Arc::clone(&protocol);
            join_handles.push(thread::spawn(move || {
                protocol_clone
                    .initialize_mls(
                        Box::new(TestMlsStorageProvider::default()),
                        Box::new(TestMlsStorageProvider::default()),
                    )
                    .unwrap();
                let core_guard = protocol_clone.inner.lock().unwrap();
                let mls_handle = core_guard.mls_manager().cloned().unwrap();
                Arc::as_ptr(&mls_handle) as usize
            }));
        }

        let first_ptr = join_handles.remove(0).join().unwrap();
        for handle in join_handles {
            let ptr = handle.join().unwrap();
            assert_eq!(ptr, first_ptr);
        }
    }

    #[test]
    fn test_high_level_api_sees_groups_created_via_core() {
        let protocol = Arc::new(OfflineProtocol::new(create_test_config()).unwrap());
        protocol
            .initialize_mls(
                Box::new(TestMlsStorageProvider::default()),
                Box::new(TestMlsStorageProvider::default()),
            )
            .unwrap();

        // Create groups through the core MlsManager directly.
        {
            let core_guard = protocol.inner.lock().unwrap();
            let manager = core_guard.mls_manager().cloned().unwrap();
            let guard = manager.read().unwrap();
            for i in 0..20 {
                guard.create_group(&format!("core-group-{}", i)).unwrap();
            }
        }

        // The high-level API should see the same groups.
        let from_high_level = protocol.list_groups().unwrap();
        let from_core_api = {
            let core_guard = protocol.inner.lock().unwrap();
            let manager = core_guard.mls_manager().cloned().unwrap();
            let groups = manager
                .read()
                .unwrap()
                .list_groups()
                .unwrap()
                .into_iter()
                .map(|group_id| group_id.as_str().to_string())
                .collect::<Vec<_>>();
            groups
        };

        assert_eq!(from_high_level.len(), from_core_api.len());
        assert_eq!(from_high_level.len(), 20);
        for group_id in from_core_api {
            assert!(from_high_level.contains(&group_id));
        }
    }

    #[test]
    fn test_protocol_creation() {
        let config = create_test_config();

        let protocol = OfflineProtocol::new(config);
        assert!(protocol.is_ok());
    }

    #[test]
    fn test_protocol_config_maps_pending_queue_settings_to_core() {
        let mut config = create_test_config();
        config.max_pending_per_peer = 11;
        config.max_pending_global = 99;
        config.pending_ttl_ms = 55_000;
        config.overflow_policy = OverflowPolicy::DropNewest;

        let core: CoreConfig = config.into();
        assert_eq!(core.encryption.pending_queue.max_pending_per_peer, 11);
        assert_eq!(core.encryption.pending_queue.max_pending_global, 99);
        assert_eq!(core.encryption.pending_queue.pending_ttl_ms, 55_000);
        assert_eq!(
            core.encryption.pending_queue.overflow_policy,
            CoreOverflowPolicy::DropNewest
        );
    }

    #[test]
    fn test_protocol_lifecycle() {
        let config = create_test_config();

        let protocol = OfflineProtocol::new(config).unwrap();
        assert_eq!(protocol.get_state(), ProtocolState::Stopped);

        assert!(protocol.start().is_ok());
        assert_eq!(protocol.get_state(), ProtocolState::Running);

        assert!(protocol.pause().is_ok());
        assert_eq!(protocol.get_state(), ProtocolState::Paused);

        assert!(protocol.resume().is_ok());
        assert_eq!(protocol.get_state(), ProtocolState::Running);

        assert!(protocol.stop().is_ok());
        assert_eq!(protocol.get_state(), ProtocolState::Stopped);
    }

    #[test]
    fn test_ble_peer_management() {
        let config = create_test_config();

        let protocol = OfflineProtocol::new(config).unwrap();

        assert_eq!(protocol.ble_get_peer_count(), 0);

        protocol
            .ble_peer_discovered("peer1".to_string(), -50)
            .unwrap();
        assert_eq!(protocol.ble_get_peer_count(), 1);

        protocol
            .ble_peer_discovered("peer2".to_string(), -60)
            .unwrap();
        assert_eq!(protocol.ble_get_peer_count(), 2);

        protocol.ble_peer_lost("peer1".to_string()).unwrap();
        assert_eq!(protocol.ble_get_peer_count(), 1);
    }

    #[test]
    fn test_file_transfer_tracking() {
        let config = create_test_config();
        let protocol = OfflineProtocol::new(config).unwrap();

        let file_id = "file_test_001".to_string();

        assert!(protocol.get_file_progress(file_id.clone()).is_none());

        protocol
            .process_file_chunk(
                file_id.clone(),
                0,
                2,
                100,
                "test.txt".to_string(),
                "abc123".to_string(),
                vec![0u8; 50],
            )
            .unwrap();

        let progress = protocol.get_file_progress(file_id.clone());
        assert!(progress.is_some());
        let progress = progress.unwrap();
        assert_eq!(progress.chunks_sent, 1);
        assert_eq!(progress.total_chunks, 2);
        assert!(progress.percentage < 100);
    }

    #[test]
    fn test_gradient_routing_learn_and_query() {
        let config = create_ble_only_config();

        let protocol = OfflineProtocol::new(config).unwrap();

        // Initially no routes
        assert!(!protocol.has_route("alice".to_string()));
        assert!(protocol.get_best_route("alice".to_string()).is_none());

        // Learn a route to alice through peer1
        protocol.learn_route(
            "alice".to_string(),
            "peer1".to_string(),
            2,   // hop count
            0.8, // quality
            0,   // sequence_number (none from message)
        );

        // Should now have a route
        assert!(protocol.has_route("alice".to_string()));

        let route = protocol.get_best_route("alice".to_string());
        assert!(route.is_some());
        let route = route.unwrap();
        assert_eq!(route.next_hop, "peer1");
        assert_eq!(route.hop_count, 2);
        assert!((route.quality - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_gradient_routing_multiple_routes() {
        let config = create_ble_only_config();

        let protocol = OfflineProtocol::new(config).unwrap();

        // Learn multiple routes to the same destination
        protocol.learn_route("bob".to_string(), "peer1".to_string(), 3, 0.7, 0);
        protocol.learn_route("bob".to_string(), "peer2".to_string(), 2, 0.9, 0);
        protocol.learn_route("bob".to_string(), "peer3".to_string(), 1, 0.6, 0);

        // Best route should be through peer2 (highest quality)
        let best = protocol.get_best_route("bob".to_string());
        assert!(best.is_some());
        assert_eq!(best.unwrap().next_hop, "peer2");

        // All routes should be returned
        let all_routes = protocol.get_all_routes("bob".to_string());
        assert_eq!(all_routes.len(), 3);
    }

    #[test]
    fn test_gradient_routing_remove_neighbor() {
        let config = create_ble_only_config();

        let protocol = OfflineProtocol::new(config).unwrap();

        // Learn routes through peer1
        protocol.learn_route("alice".to_string(), "peer1".to_string(), 2, 0.8, 0);
        protocol.learn_route("bob".to_string(), "peer1".to_string(), 3, 0.7, 0);

        // Learn route through peer2
        protocol.learn_route("charlie".to_string(), "peer2".to_string(), 1, 0.9, 0);

        // All destinations should be reachable
        assert!(protocol.has_route("alice".to_string()));
        assert!(protocol.has_route("bob".to_string()));
        assert!(protocol.has_route("charlie".to_string()));

        // Remove peer1 (simulating disconnect)
        protocol.remove_neighbor_routes("peer1".to_string());

        // Routes through peer1 should be gone
        assert!(!protocol.has_route("alice".to_string()));
        assert!(!protocol.has_route("bob".to_string()));

        // Route through peer2 should remain
        assert!(protocol.has_route("charlie".to_string()));
    }

    #[test]
    fn test_gradient_routing_stats() {
        let config = create_ble_only_config();

        let protocol = OfflineProtocol::new(config).unwrap();

        // Initially empty
        let stats = protocol.get_routing_stats();
        assert_eq!(stats.destination_count, 0);
        assert_eq!(stats.route_count, 0);

        // Add some routes
        protocol.learn_route("alice".to_string(), "peer1".to_string(), 2, 0.8, 0);
        protocol.learn_route("alice".to_string(), "peer2".to_string(), 3, 0.6, 0);
        protocol.learn_route("bob".to_string(), "peer1".to_string(), 1, 0.9, 0);

        let stats = protocol.get_routing_stats();
        assert_eq!(stats.destination_count, 2); // alice and bob
        assert_eq!(stats.route_count, 3); // 2 routes to alice, 1 to bob
    }

    #[test]
    fn test_gradient_routing_config_update() {
        let config = create_ble_only_config();

        let protocol = OfflineProtocol::new(config).unwrap();

        // Update routing config
        let routing_config = GradientRoutingConfig {
            max_routes_per_destination: 5,
            route_ttl_secs: 600,
            max_routing_table_size: 500,
        };
        protocol.update_routing_config(routing_config);

        // Config should be applied (routing table is reset with new config)
        let stats = protocol.get_routing_stats();
        assert_eq!(stats.destination_count, 0);
        assert_eq!(stats.route_count, 0);
    }

    fn create_reticulum_config() -> ProtocolConfig {
        ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            app_id: "test-app".to_string(),
            user_id: "user123".to_string(),
            ble_enabled: false,
            wifi_direct_enabled: false,
            internet_enabled: false,
            reticulum_enabled: true,
            nostr_enabled: false,
            prefer_online: false,
            initial_ttl: 8,
            encryption_enabled: true,
            auto_key_exchange: true,
            store_pending: true,
            require_encryption: false,
            max_pending_per_peer: 64,
            max_pending_global: 4096,
            pending_ttl_ms: DEFAULT_PENDING_TTL_MS,
            overflow_policy: OverflowPolicy::DropOldest,
            max_group_members: 256,
            group_relay_enabled: true,
            group_relay_broadcast_enabled: true,
            group_enforce_admin_commits: false,
            require_transport_identity: false,
        }
    }

    #[test]
    fn test_reticulum_status_changed() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        // Connect
        assert!(protocol.reticulum_status_changed(true).is_ok());

        // Disconnect
        assert!(protocol.reticulum_status_changed(false).is_ok());
    }

    #[test]
    fn test_reticulum_get_next_message_when_disconnected() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        // Not connected — should return None
        assert!(protocol.reticulum_get_next_message().is_none());
    }

    #[test]
    fn test_reticulum_get_next_message_when_connected() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        protocol.reticulum_status_changed(true).unwrap();

        // No messages queued — should return None without error
        assert!(protocol.reticulum_get_next_message().is_none());
    }

    #[test]
    fn test_reticulum_confirm_sent() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        protocol.reticulum_status_changed(true).unwrap();

        // Confirming a non-existent message_id should not panic
        protocol.reticulum_confirm_sent("nonexistent-id".to_string());
    }

    #[test]
    fn test_reticulum_send_failed() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        protocol.reticulum_status_changed(true).unwrap();

        // Reporting failure for a non-existent message_id should not panic
        protocol.reticulum_send_failed("nonexistent-id".to_string());
    }

    #[test]
    fn test_reticulum_send_failed_with_reason() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        protocol.reticulum_status_changed(true).unwrap();

        protocol.reticulum_send_failed_with_reason(
            "nonexistent-id".to_string(),
            Some("timeout".to_string()),
        );
    }

    #[test]
    fn test_reticulum_status_rapid_toggle() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        // Rapid connect/disconnect cycling should not panic or corrupt state
        for _ in 0..10 {
            protocol.reticulum_status_changed(true).unwrap();
            protocol.reticulum_status_changed(false).unwrap();
        }

        // Final connect to verify state is healthy
        protocol.reticulum_status_changed(true).unwrap();
        assert!(protocol.reticulum_get_next_message().is_none());
    }

    #[test]
    fn test_reticulum_message_received_empty_sender() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        protocol.reticulum_status_changed(true).unwrap();

        // Empty sender_id — should not panic; NeighborDiscovered should be suppressed
        let result = protocol.reticulum_message_received("".to_string(), vec![0u8; 10]);
        // The data isn't valid JSON/message format so the transport may reject it,
        // but it should not panic regardless.
        let _ = result;
    }

    #[test]
    fn test_reticulum_send_receive_round_trip() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        // Connect and force Reticulum transport so DORS doesn't pick another
        protocol.reticulum_status_changed(true).unwrap();
        protocol.force_transport(TransportType::Reticulum).unwrap();

        // Send a message — this enqueues it in the Reticulum transport's send_queue
        let msg_id = protocol
            .send_message(
                "recipient-peer".to_string(),
                "hello via reticulum".to_string(),
                MessagePriority::Medium,
                None,
            )
            .unwrap();
        assert!(!msg_id.is_empty());

        // Retrieve the outgoing message via the platform bridge method
        let outgoing = protocol.reticulum_get_next_message();
        assert!(outgoing.is_some(), "Expected an outgoing Reticulum message");
        let outgoing = outgoing.unwrap();
        assert_eq!(outgoing.recipient_id, "recipient-peer");
        assert!(!outgoing.data.is_empty());

        // Confirm the message was sent — should not panic
        protocol.reticulum_confirm_sent(outgoing.message_id.clone());

        // Queue should now be empty
        assert!(protocol.reticulum_get_next_message().is_none());
    }

    #[test]
    fn test_reticulum_send_round_trip_with_failure() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        protocol.reticulum_status_changed(true).unwrap();
        protocol.force_transport(TransportType::Reticulum).unwrap();

        let _msg_id = protocol
            .send_message(
                "recipient-peer".to_string(),
                "will fail".to_string(),
                MessagePriority::Medium,
                None,
            )
            .unwrap();

        let outgoing = protocol.reticulum_get_next_message().unwrap();

        // Report failure — should not panic and should update metrics
        protocol.reticulum_send_failed_with_reason(
            outgoing.message_id,
            Some("daemon unreachable".to_string()),
        );

        // Queue should be empty after the failure report
        assert!(protocol.reticulum_get_next_message().is_none());
    }

    #[test]
    fn test_reticulum_reconnect_flushes_outbox() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        protocol.reticulum_status_changed(true).unwrap();
        protocol.force_transport(TransportType::Reticulum).unwrap();

        // Send a message while connected
        protocol
            .send_message(
                "peer-a".to_string(),
                "buffered msg".to_string(),
                MessagePriority::Medium,
                None,
            )
            .unwrap();

        // Disconnect — the message stays in the outbox
        protocol.reticulum_status_changed(false).unwrap();
        assert!(
            protocol.reticulum_get_next_message().is_none(),
            "Should return None when disconnected"
        );

        // Reconnect — this triggers flush_outbox_all
        protocol.reticulum_status_changed(true).unwrap();

        // The flushed message should now be retrievable
        let outgoing = protocol.reticulum_get_next_message();
        assert!(
            outgoing.is_some(),
            "Expected outbox to be flushed on reconnect"
        );
    }

    #[test]
    fn test_reticulum_set_transport_callback() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        let call_count = Arc::new(AtomicUsize::new(0));

        struct TestCallback {
            count: Arc<AtomicUsize>,
        }

        impl ReticulumTransportCallback for TestCallback {
            fn on_messages_available(&self) {
                self.count.fetch_add(1, Ordering::SeqCst);
            }
        }

        protocol.set_reticulum_transport_callback(Box::new(TestCallback {
            count: call_count.clone(),
        }));

        // Connect and force Reticulum
        protocol.reticulum_status_changed(true).unwrap();
        protocol.force_transport(TransportType::Reticulum).unwrap();

        // Send a message — should trigger the callback
        protocol
            .send_message(
                "peer-b".to_string(),
                "trigger callback".to_string(),
                MessagePriority::Medium,
                None,
            )
            .unwrap();

        assert!(
            call_count.load(Ordering::SeqCst) > 0,
            "Expected transport callback to have been invoked"
        );
    }

    #[test]
    fn test_reticulum_status_changed_idempotent_disconnect() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        // Disconnecting when already disconnected should be idempotent
        assert!(protocol.reticulum_status_changed(false).is_ok());
        assert!(protocol.reticulum_status_changed(false).is_ok());

        // Connect then double-disconnect
        protocol.reticulum_status_changed(true).unwrap();
        assert!(protocol.reticulum_status_changed(false).is_ok());
        assert!(protocol.reticulum_status_changed(false).is_ok());

        // State should remain healthy
        assert!(protocol.reticulum_get_next_message().is_none());
        assert!(protocol.reticulum_status_changed(true).is_ok());
    }

    #[test]
    fn test_reticulum_status_changed_idempotent_connect() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        // Connecting when already connected should be idempotent
        assert!(protocol.reticulum_status_changed(true).is_ok());
        assert!(protocol.reticulum_status_changed(true).is_ok());

        // Disconnect then double-connect
        protocol.reticulum_status_changed(false).unwrap();
        assert!(protocol.reticulum_status_changed(true).is_ok());
        assert!(protocol.reticulum_status_changed(true).is_ok());

        // State should remain healthy
        assert!(protocol.reticulum_get_next_message().is_none());
        assert!(protocol.reticulum_status_changed(false).is_ok());
    }

    #[test]
    fn test_reticulum_message_received_blocked_sender() {
        // Sender protocol sends a message; we capture serialized bytes
        let sender_config = ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "sender-user".to_string(),
            ..create_reticulum_config()
        };
        let sender = OfflineProtocol::new(sender_config).unwrap();
        sender.start().unwrap();
        sender.reticulum_status_changed(true).unwrap();
        sender.force_transport(TransportType::Reticulum).unwrap();

        sender
            .send_message(
                "receiver-user".to_string(),
                "hello from blocked sender".to_string(),
                MessagePriority::Medium,
                None,
            )
            .unwrap();

        let outgoing = sender
            .reticulum_get_next_message()
            .expect("Expected outgoing message from sender");
        let serialized_data = outgoing.data;

        // Receiver protocol blocks the sender before ingesting data
        let receiver_config = ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_reticulum_config()
        };
        let receiver = OfflineProtocol::new(receiver_config).unwrap();
        receiver.start().unwrap();
        receiver.reticulum_status_changed(true).unwrap();

        // Block the sender
        receiver.block_user("sender-user".to_string()).unwrap();

        // Feed valid serialized message data from blocked sender — should not panic
        let result =
            receiver.reticulum_message_received("sender-user".to_string(), serialized_data);
        assert!(
            result.is_ok(),
            "Processing message from blocked sender should not error"
        );
    }

    #[test]
    fn test_reticulum_message_received_valid_data() {
        // Sender protocol sends a message; we capture the serialized bytes
        // from its transport queue and feed them into a receiver protocol
        // via reticulum_message_received.
        let sender_config = ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "sender-user".to_string(),
            ..create_reticulum_config()
        };
        let sender = OfflineProtocol::new(sender_config).unwrap();
        sender.start().unwrap();
        sender.reticulum_status_changed(true).unwrap();
        sender.force_transport(TransportType::Reticulum).unwrap();

        sender
            .send_message(
                "receiver-user".to_string(),
                "hello from sender".to_string(),
                MessagePriority::Medium,
                None,
            )
            .unwrap();

        let outgoing = sender
            .reticulum_get_next_message()
            .expect("Expected outgoing message from sender");
        let serialized_data = outgoing.data;

        // Receiver protocol ingests the serialized bytes
        let receiver_config = ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_reticulum_config()
        };
        let receiver = OfflineProtocol::new(receiver_config).unwrap();
        receiver.start().unwrap();
        receiver.reticulum_status_changed(true).unwrap();

        // Feed valid serialized message data — should succeed
        let result =
            receiver.reticulum_message_received("sender-user".to_string(), serialized_data);
        assert!(
            result.is_ok(),
            "Expected valid message to be processed successfully"
        );

        // The message should have been received and drained by receive_message()
        // inside reticulum_message_received, so the high-level receive_message
        // should return None (already consumed).
        assert!(
            receiver.receive_message().is_none(),
            "Message should have been consumed internally"
        );
    }

    #[test]
    fn test_reticulum_set_transport_callback_without_transport() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Config with reticulum DISABLED (BLE enabled to satisfy the
        // "at least one transport" requirement) — no ReticulumTransport
        // will be created.
        let config = ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            reticulum_enabled: false,
            ble_enabled: true,
            ..create_reticulum_config()
        };
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        struct NoopCallback {
            count: Arc<AtomicUsize>,
        }

        impl ReticulumTransportCallback for NoopCallback {
            fn on_messages_available(&self) {
                self.count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let call_count = Arc::new(AtomicUsize::new(0));

        // Setting callback without Reticulum transport should be a no-op
        protocol.set_reticulum_transport_callback(Box::new(NoopCallback {
            count: call_count.clone(),
        }));

        // Callback should never have been invoked
        assert_eq!(call_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_reticulum_message_received_while_disconnected() {
        let config = create_reticulum_config();
        let protocol = OfflineProtocol::new(config).unwrap();
        protocol.start().unwrap();

        // Do NOT call reticulum_status_changed(true) — transport is disconnected.
        // Calling reticulum_message_received should not panic.
        let result = protocol.reticulum_message_received("some-sender".to_string(), vec![0u8; 32]);
        // The data is not valid message format, but it must not panic.
        let _ = result;
    }

    // ---- Reachability seam: notify_neighbor_reachable (issue #136) ----

    /// Produces valid serialized protocol-message bytes from `sender_id`
    /// addressed to `recipient`. Every transport uses the shared JSON wire
    /// format, so Internet-produced bytes are valid inbound data on any path.
    fn serialized_message_from(sender_id: &str, recipient: &str) -> Vec<u8> {
        let sender = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: sender_id.to_string(),
            ..create_test_config()
        })
        .unwrap();
        sender.start().unwrap();
        sender.internet_status_changed(true).unwrap();
        sender.force_transport(TransportType::Internet).unwrap();
        sender
            .send_message(
                recipient.to_string(),
                "reachability probe".to_string(),
                MessagePriority::Medium,
                None,
            )
            .unwrap();
        sender
            .internet_get_next_message()
            .expect("Expected outgoing message from sender")
            .data
    }

    fn drained_events(protocol: &OfflineProtocol) -> Vec<String> {
        let mut events = Vec::new();
        while let Some(event) = protocol.poll_event() {
            events.push(event);
        }
        events
    }

    /// Builds serialized bytes for a security-gated control frame, the same
    /// shape the platform bridges construct (`buildInternalMessageBytes`).
    /// Fixed id is fine: each caller uses a fresh protocol instance.
    fn serialized_control_frame(sender: &str, recipient: &str, hop_count: u8) -> Vec<u8> {
        serde_json::json!({
            "id": "6dd7f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f",
            "sender": sender,
            "recipient": recipient,
            "content": "__CONN_REQ__{\"data\":\"test\"}",
            "app_id": "test-app",
            "priority": "medium",
            "ttl": 8,
            "hop_count": hop_count,
            "requires_ack": true,
            "timestamp": 1_700_000_000_000_i64,
        })
        .to_string()
        .into_bytes()
    }

    /// Redundant same-state reports (e.g. bridge auth refresh) must not emit
    /// phantom TransportSwitched events; only real transitions do.
    #[test]
    fn test_internet_status_changed_redundant_report_emits_no_event() {
        let protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.start().unwrap();

        protocol.internet_status_changed(true).unwrap();
        drained_events(&protocol);

        protocol.internet_status_changed(true).unwrap();
        assert!(
            !drained_events(&protocol)
                .iter()
                .any(|e| e.contains("transport_switched")),
            "redundant connected report must not emit transport_switched"
        );

        protocol.internet_status_changed(false).unwrap();
        assert!(
            drained_events(&protocol)
                .iter()
                .any(|e| e.contains("transport_switched")),
            "real transition must still emit transport_switched"
        );
    }

    /// End-to-end pin for the internet sender-attribution wiring: a gated
    /// control frame claiming direct origin (`hop_count == 0`) whose relay-
    /// authenticated uploader does not match `message.sender` must be dropped
    /// with a `security_warning`, while matched and mesh-relayed frames pass.
    #[test]
    fn test_internet_control_frame_spoofed_sender_rejected() {
        let receiver = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();
        receiver.internet_status_changed(true).unwrap();

        // Uploader "eve" (relay-authenticated) delivers a hop-0 frame claiming
        // to originate from "alice" — spoofing, must be security-rejected.
        let frame = serialized_control_frame("alice", "receiver-user", 0);
        receiver
            .internet_message_received("eve".to_string(), frame)
            .unwrap();

        assert!(
            drained_events(&receiver)
                .iter()
                .any(|e| e.contains("security_warning") && e.contains("alice")),
            "spoofed hop-0 control frame over internet must emit a security warning"
        );
    }

    #[test]
    fn test_internet_control_frame_matching_sender_passes_gate() {
        let receiver = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();
        receiver.internet_status_changed(true).unwrap();

        // Uploader "alice" delivers her own hop-0 frame — identities match.
        let frame = serialized_control_frame("alice", "receiver-user", 0);
        receiver
            .internet_message_received("alice".to_string(), frame)
            .unwrap();

        assert!(
            !drained_events(&receiver)
                .iter()
                .any(|e| e.contains("security_warning")),
            "matched sender must pass the transport-identity gate"
        );
    }

    #[test]
    fn test_internet_control_frame_relayed_carrier_mismatch_passes_gate() {
        let receiver = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();
        receiver.internet_status_changed(true).unwrap();

        // Uploader "carol" relays alice's frame (hop 1): carrier/origin
        // mismatch is expected and must not trip the gate.
        let frame = serialized_control_frame("alice", "receiver-user", 1);
        receiver
            .internet_message_received("carol".to_string(), frame)
            .unwrap();

        assert!(
            !drained_events(&receiver)
                .iter()
                .any(|e| e.contains("security_warning")),
            "mesh-relayed control frame must not be rejected for carrier/origin mismatch"
        );
    }

    /// An empty authenticated sender (relay frame without attribution) must
    /// fall back to unattributed ingest: no strict match against an empty
    /// identity, and the frame is still processed best-effort.
    #[test]
    fn test_internet_message_received_empty_sender_falls_back_to_unattributed() {
        let receiver = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();
        receiver.internet_status_changed(true).unwrap();

        // A parseable ConnectionRequestPayload, so full processing is
        // observable as a connection_request_received event.
        let frame = serde_json::json!({
            "id": "6dd7f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f",
            "sender": "alice",
            "recipient": "receiver-user",
            "content": "__CONN_REQ__{\"sender_name\":\"Alice\",\"timestamp_ms\":1700000000000}",
            "app_id": "test-app",
            "priority": "medium",
            "ttl": 8,
            "hop_count": 0,
            "requires_ack": true,
            "timestamp": 1_700_000_000_000_i64,
        })
        .to_string()
        .into_bytes();
        receiver
            .internet_message_received(String::new(), frame)
            .unwrap();

        let events = drained_events(&receiver);
        assert!(
            !events.iter().any(|e| e.contains("security_warning")),
            "unattributed ingest must not strict-match against an empty identity"
        );
        assert!(
            events.iter().any(|e| e.contains("connection_request")),
            "unattributed control frame must still be processed best-effort"
        );
    }

    #[test]
    fn test_internet_message_received_marks_neighbor_reachable() {
        let data = serialized_message_from("sender-user", "receiver-user");

        let receiver = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();
        receiver.internet_status_changed(true).unwrap();

        receiver
            .internet_message_received("sender-user".to_string(), data)
            .unwrap();

        assert!(
            receiver.lock_inner().unwrap().is_known_peer("sender-user"),
            "inbound internet message must notify the core protocol of the reachable sender"
        );
        assert!(
            drained_events(&receiver)
                .iter()
                .any(|e| e.contains("neighbor_discovered") && e.contains("sender-user")),
            "inbound internet message must still emit NeighborDiscovered"
        );
    }

    /// Bridge-synthesized relay answers (GroupCreated/GroupError/group
    /// snapshots) name no real peer: nothing transmitted them, so the
    /// placeholder in their body must stay inert.
    ///
    /// Regression (phantom peer): the platform bridges used to pass a literal
    /// `"relay"` as the FFI `sender_id` and stamp `requires_ack: true`. That
    /// fed `notify_neighbor_reachable`, so the core tracked `relay` as a live
    /// neighbor (auto key-package DM, `NeighborDiscovered`, service-discovery
    /// fan-out) *and* returned a delivery ACK addressed to it for every single
    /// injected frame. Both produced undeliverable outbound DMs, whose relay
    /// `DeliveryError`s pinned `relay` in the bridge's presence-watch set
    /// indefinitely.
    ///
    /// The last assertion is the load-bearing one: it catches both vectors at
    /// once, and stays honest regardless of how the fix is implemented.
    #[test]
    fn test_synthesized_relay_frame_creates_no_phantom_peer() {
        let receiver = OfflineProtocol::new(ProtocolConfig {
            user_id: "receiver-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();
        receiver.internet_status_changed(true).unwrap();
        // Ignore whatever startup produced; only post-injection effects matter.
        drained_events(&receiver);
        while receiver.internet_get_next_message().is_some() {}

        // Exactly what `injectGroupInternalMessage(actorId: nil, …)` builds:
        // placeholder `sender` in the body (Rust `UserId` rejects empty), no
        // ACK requested, and ingested unattributed via an empty `sender_id`.
        let frame = serde_json::json!({
            "id": "6dd7f6f0-9d2c-4b6a-8f3e-2a1b0c9d8e7f",
            "sender": "relay",
            "recipient": "receiver-user",
            "content": "__GROUP_CREATED__{\"group_id\":\"group-1\",\"name\":\"Test Group\"}",
            "app_id": "test-app",
            "priority": "medium",
            "ttl": 8,
            "hop_count": 0,
            "requires_ack": false,
            "timestamp": 1_700_000_000_000_i64,
        })
        .to_string()
        .into_bytes();

        receiver
            .internet_message_received(String::new(), frame)
            .unwrap();

        let events = drained_events(&receiver);
        assert!(
            events.iter().any(|e| e.contains("group_created")),
            "the synthesized relay answer must still be processed and surfaced"
        );
        assert!(
            !receiver.lock_inner().unwrap().is_known_peer("relay"),
            "a synthesized relay answer must not make its placeholder a tracked neighbor"
        );
        assert!(
            !events.iter().any(|e| e.contains("neighbor_discovered")),
            "a synthesized relay answer must not emit NeighborDiscovered"
        );

        let mut outbound_recipients = Vec::new();
        while let Some(msg) = receiver.internet_get_next_message() {
            outbound_recipients.push(msg.recipient_id);
        }
        assert!(
            !outbound_recipients.iter().any(|r| r == "relay"),
            "nothing may be addressed to the placeholder sender (no key package, \
             no delivery ACK); got outbound recipients: {:?}",
            outbound_recipients
        );
    }

    #[test]
    fn test_wifi_direct_message_received_marks_neighbor_reachable() {
        let data = serialized_message_from("sender-user", "receiver-user");

        let receiver = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();
        receiver.wifi_direct_status_changed(true).unwrap();

        receiver
            .wifi_direct_message_received("sender-user".to_string(), data)
            .unwrap();

        assert!(
            receiver.lock_inner().unwrap().is_known_peer("sender-user"),
            "inbound WiFi Direct message must notify the core protocol of the reachable sender"
        );
    }

    #[test]
    fn test_wifi_direct_peer_connected_marks_neighbor_reachable() {
        let protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.start().unwrap();

        protocol
            .wifi_direct_peer_connected("wifi-peer".to_string())
            .unwrap();

        assert!(
            protocol.lock_inner().unwrap().is_known_peer("wifi-peer"),
            "WiFi Direct connection must notify the core protocol of the reachable peer"
        );
    }

    #[test]
    fn test_wifi_direct_peer_disconnected_marks_neighbor_lost() {
        let protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.start().unwrap();

        protocol
            .wifi_direct_peer_connected("wifi-peer".to_string())
            .unwrap();
        protocol
            .wifi_direct_peer_disconnected("wifi-peer".to_string())
            .unwrap();

        assert!(
            !protocol.lock_inner().unwrap().is_known_peer("wifi-peer"),
            "WiFi Direct disconnect must clear core discovery tracking, matching ble_peer_lost"
        );
        assert!(
            drained_events(&protocol)
                .iter()
                .any(|e| e.contains("neighbor_lost") && e.contains("wifi-peer")),
            "WiFi Direct disconnect must still emit NeighborLost"
        );
    }

    #[test]
    fn test_ble_peer_discovered_marks_neighbor_reachable() {
        let protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.start().unwrap();

        protocol
            .ble_peer_discovered("ble-peer".to_string(), -40)
            .unwrap();

        assert!(
            protocol.lock_inner().unwrap().is_known_peer("ble-peer"),
            "BLE discovery must notify the core protocol of the reachable peer"
        );
    }

    #[test]
    fn test_reticulum_message_received_marks_neighbor_reachable() {
        let data = serialized_message_from("sender-user", "receiver-user");

        let receiver = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_reticulum_config()
        })
        .unwrap();
        receiver.start().unwrap();
        receiver.reticulum_status_changed(true).unwrap();

        receiver
            .reticulum_message_received("sender-user".to_string(), data)
            .unwrap();

        assert!(
            receiver.lock_inner().unwrap().is_known_peer("sender-user"),
            "inbound Reticulum message must notify the core protocol of the reachable sender"
        );
    }

    #[test]
    fn test_nostr_message_received_marks_neighbor_reachable() {
        let data = serialized_message_from("sender-user", "receiver-user");

        let receiver = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            nostr_enabled: true,
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();

        receiver
            .nostr_message_received("nostr-routing-pubkey".to_string(), data)
            .unwrap();

        let core = receiver.lock_inner().unwrap();
        assert!(
            core.is_known_peer("sender-user"),
            "inbound Nostr message must track the real sender from the message body"
        );
        assert!(
            !core.is_known_peer("nostr-routing-pubkey"),
            "the Nostr routing pubkey is not a protocol identity and must not be tracked"
        );
    }

    #[test]
    fn test_nostr_undecodable_frame_surfaces_no_discovery() {
        let receiver = OfflineProtocol::new(ProtocolConfig {
            user_id: "receiver-user".to_string(),
            nostr_enabled: true,
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();

        receiver
            .nostr_message_received(
                "nostr-routing-pubkey".to_string(),
                b"not a protocol message".to_vec(),
            )
            .unwrap();

        assert!(
            !receiver
                .lock_inner()
                .unwrap()
                .is_known_peer("nostr-routing-pubkey"),
            "an undecodable frame must not surface the Nostr pubkey as a discovered peer"
        );
    }

    /// Reads the `since` the transport would put on its next REQ filter.
    fn nostr_subscription_since(protocol: &OfflineProtocol) -> i64 {
        let filter = protocol
            .nostr_get_subscription_filter("sub".to_string())
            .expect("nostr transport registered");
        let parsed: serde_json::Value = serde_json::from_str(&filter).unwrap();
        parsed[2]["since"].as_i64().unwrap()
    }

    fn now_unix_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn test_nostr_message_received_at_advances_the_subscription_window() {
        let data = serialized_message_from("sender-user", "receiver-user");

        let receiver = OfflineProtocol::new(ProtocolConfig {
            user_id: "receiver-user".to_string(),
            nostr_enabled: true,
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();

        let first_run_since = nostr_subscription_since(&receiver);
        let created_at = now_unix_secs() - 30;

        receiver
            .nostr_message_received_at("nostr-routing-pubkey".to_string(), data, created_at)
            .unwrap();

        let advanced_since = nostr_subscription_since(&receiver);
        assert!(
            advanced_since > first_run_since,
            "an event's created_at must move the subscription window forward \
             ({first_run_since} -> {advanced_since})"
        );
        assert!(
            advanced_since < created_at,
            "since must still sit below the mark by the jitter + skew overlap"
        );
    }

    #[test]
    fn test_nostr_undecodable_frame_does_not_advance_the_watermark() {
        // The routing tag is derived from a public user id, so anyone can
        // publish to it. Junk must not drag the receive window past history
        // the relay still owes us.
        let receiver = OfflineProtocol::new(ProtocolConfig {
            user_id: "receiver-user".to_string(),
            nostr_enabled: true,
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();

        let before = nostr_subscription_since(&receiver);
        receiver
            .nostr_message_received_at(
                "nostr-routing-pubkey".to_string(),
                b"not a protocol message".to_vec(),
                now_unix_secs(),
            )
            .unwrap();

        assert_eq!(
            nostr_subscription_since(&receiver),
            before,
            "an undecodable frame must not advance the receive watermark"
        );
    }

    #[test]
    fn test_legacy_nostr_message_received_leaves_the_watermark_alone() {
        // The timestamp-less entry stays for bridges that have not been
        // updated; it must keep working, just without narrowing the window.
        let data = serialized_message_from("sender-user", "receiver-user");

        let receiver = OfflineProtocol::new(ProtocolConfig {
            user_id: "receiver-user".to_string(),
            nostr_enabled: true,
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();

        let before = nostr_subscription_since(&receiver);
        receiver
            .nostr_message_received("nostr-routing-pubkey".to_string(), data)
            .unwrap();

        assert_eq!(nostr_subscription_since(&receiver), before);
        assert!(
            receiver.lock_inner().unwrap().is_known_peer("sender-user"),
            "the legacy entry must still deliver and surface discovery"
        );
    }

    #[test]
    fn test_internet_message_received_blocked_sender_not_tracked() {
        let data = serialized_message_from("sender-user", "receiver-user");

        let receiver = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();
        receiver.internet_status_changed(true).unwrap();
        receiver.block_user("sender-user".to_string()).unwrap();

        receiver
            .internet_message_received("sender-user".to_string(), data)
            .unwrap();

        assert!(
            !receiver.lock_inner().unwrap().is_known_peer("sender-user"),
            "blocked senders must not be tracked as neighbors"
        );
        assert!(
            !drained_events(&receiver)
                .iter()
                .any(|e| e.contains("neighbor_discovered")),
            "blocked senders must not produce a NeighborDiscovered event"
        );
    }

    #[test]
    fn test_internet_message_received_empty_sender_no_event() {
        let data = serialized_message_from("sender-user", "receiver-user");

        let receiver = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "receiver-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        receiver.start().unwrap();
        receiver.internet_status_changed(true).unwrap();

        receiver
            .internet_message_received(String::new(), data)
            .unwrap();

        assert!(
            !receiver.lock_inner().unwrap().is_known_peer(""),
            "an empty sender id must not be tracked as a neighbor"
        );
        assert!(
            !drained_events(&receiver)
                .iter()
                .any(|e| e.contains("neighbor_discovered")),
            "an empty sender id must not produce a NeighborDiscovered event"
        );
    }

    /// Drains and returns everything queued on `protocol`'s internet outbound path.
    fn drain_internet_outbox(protocol: &OfflineProtocol) -> Vec<InternetMessage> {
        let mut items = Vec::new();
        while let Some(msg) = protocol.internet_get_next_message() {
            items.push(msg);
        }
        items
    }

    fn is_welcome_wire_message(data: &[u8]) -> bool {
        String::from_utf8_lossy(data).contains("__MLS_WELCOME__")
    }

    /// Regression test for the drain-before-notify ordering documented on
    /// `notify_neighbor_reachable`: message-path callers must drain
    /// `receive_message()` before the reachability notification so a handshake
    /// confirmation in the same inbound batch lands in `confirmed_sessions`
    /// before the Welcome re-arm decision. If the order is ever flipped,
    /// `rearm_welcome_for_peer` sees a re-armable (Failed/Expired) lifecycle
    /// for a peer whose confirmation is still sitting undrained in the batch,
    /// and re-sends a redundant Welcome to an already-confirmed peer.
    #[test]
    fn test_internet_confirm_in_same_batch_suppresses_redundant_welcome() {
        let owner = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "owner-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        owner
            .initialize_mls(
                Box::new(TestMlsStorageProvider::default()),
                Box::new(TestMlsStorageProvider::default()),
            )
            .unwrap();
        owner.start().unwrap();
        owner.internet_status_changed(true).unwrap();
        owner.force_transport(TransportType::Internet).unwrap();

        let peer = OfflineProtocol::new(ProtocolConfig {
            binary_wire_enabled: true,
            nostr_sealing_enabled: true,
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
            user_id: "peer-user".to_string(),
            ..create_test_config()
        })
        .unwrap();
        peer.initialize_mls(
            Box::new(TestMlsStorageProvider::default()),
            Box::new(TestMlsStorageProvider::default()),
        )
        .unwrap();
        peer.start().unwrap();
        peer.internet_status_changed(true).unwrap();
        peer.force_transport(TransportType::Internet).unwrap();

        // Discovery seed: an inbound message from the owner makes the peer
        // auto-send its key package.
        peer.internet_message_received(
            "owner-user".to_string(),
            serialized_message_from("owner-user", "peer-user"),
        )
        .unwrap();

        // Pump peer -> owner so the owner holds the peer's key package.
        // (Auto-establish may create the session and queue the Welcome
        // during this drain already.)
        for msg in drain_internet_outbox(&peer) {
            owner
                .internet_message_received("peer-user".to_string(), msg.data)
                .unwrap();
        }

        // Ensure the session exists; `Ok(None)` if auto-establish beat us.
        owner
            .establish_secure_session("peer-user".to_string())
            .unwrap();

        // Exactly one Welcome must be queued. The owner's other outbound
        // items (its own key package) are deliberately NOT forwarded to the
        // peer — that would start the both-create flow and change the
        // scenario.
        let outbox = drain_internet_outbox(&owner);
        let welcomes: Vec<&InternetMessage> = outbox
            .iter()
            .filter(|m| is_welcome_wire_message(&m.data))
            .collect();
        assert_eq!(welcomes.len(), 1, "expected exactly one queued Welcome");
        let welcome = welcomes[0];

        // The wire send fails: the Welcome lifecycle leaves SendAttempted for
        // a re-armable state (Failed with a backoff retry pending, or Expired
        // once the budget is exhausted).
        owner.internet_send_failed(welcome.message_id.clone());
        assert!(
            drained_events(&owner)
                .iter()
                .any(|e| e.contains("welcome_send_failed")),
            "the reported send failure must reach the welcome lifecycle"
        );

        // The peer got the Welcome anyway (an earlier attempt landed, or it
        // arrived over another carrier): it adopts the session and queues its
        // proactive encrypted session-confirm.
        peer.internet_message_received("owner-user".to_string(), welcome.data.clone())
            .unwrap();

        // After adopting, the peer queues its proactive encrypted
        // session-confirm alongside other control traffic (e.g. reliability
        // ACKs). Each `internet_message_received` call is its own inbound
        // batch, and a non-confirm batch may legitimately re-arm — the
        // invariant under test is only that the batch CARRYING the
        // confirmation must not. So deliver exactly the encrypted confirm.
        let confirm = drain_internet_outbox(&peer)
            .into_iter()
            .find(|m| String::from_utf8_lossy(&m.data).contains("__MLS_ENC__"))
            .expect("peer must queue its proactive encrypted session-confirm");

        // The moment under test: the encrypted confirm arrives in the same
        // inbound batch that marks the peer reachable again.
        owner
            .internet_message_received("peer-user".to_string(), confirm.data)
            .unwrap();

        assert!(
            drained_events(&owner)
                .iter()
                .any(|e| e.contains("secure_session_established")),
            "the encrypted confirm must confirm the session during the drain"
        );
        assert!(
            !drain_internet_outbox(&owner)
                .iter()
                .any(|m| is_welcome_wire_message(&m.data)),
            "no Welcome may be re-sent to a peer whose confirmation arrived in the same batch"
        );
        assert!(
            owner.lock_inner().unwrap().is_known_peer("peer-user"),
            "the reachability notification must still run after the drain"
        );
    }

    // ---- Telemetry sink adapter end-to-end wiring (step 7) ----

    #[derive(Default)]
    struct TestTelemetrySink {
        protocol_events: Mutex<Vec<String>>,
        metrics_frames: Mutex<Vec<MetricsFrame>>,
        transport_states: Mutex<Vec<TransportStateEvent>>,
        routing_decisions: Mutex<Vec<RoutingDecision>>,
        device_snapshots: Mutex<Vec<DeviceCapabilitySnapshot>>,
        extensions: Mutex<Vec<(String, String)>>,
    }

    impl TelemetrySink for TestTelemetrySink {
        fn on_protocol_event(&self, event_json: String) {
            self.protocol_events.lock().unwrap().push(event_json);
        }
        fn on_mls_event(&self, _event_json: String) {}
        fn on_metrics_frame(&self, frame: MetricsFrame) {
            self.metrics_frames.lock().unwrap().push(frame);
        }
        fn on_transport_state(&self, event: TransportStateEvent) {
            self.transport_states.lock().unwrap().push(event);
        }
        fn on_routing_decision(&self, decision: RoutingDecision) {
            self.routing_decisions.lock().unwrap().push(decision);
        }
        fn on_device_capability(&self, snapshot: DeviceCapabilitySnapshot) {
            self.device_snapshots.lock().unwrap().push(snapshot);
        }
        fn on_extension(&self, name: String, payload_json: String) {
            self.extensions.lock().unwrap().push((name, payload_json));
        }
    }

    /// Exercise the adapter directly: build a `CoreTelemetryRecord`, feed it
    /// through the adapter's `emit`, and assert both the typed foreign
    /// callback fires *and* the poll queue captures a matching envelope.
    /// This is a unit test of the adapter; end-to-end coverage (that the
    /// core protocol's event emit path reaches the adapter) lives in the
    /// `offline-protocol` crate's sink tests.
    fn install_sink_via_adapter(
        protocol: &OfflineProtocol,
        sink: Arc<TestTelemetrySink>,
    ) -> Arc<TelemetrySinkAdapter> {
        struct Forward(Arc<TestTelemetrySink>);
        impl TelemetrySink for Forward {
            fn on_protocol_event(&self, j: String) {
                self.0.on_protocol_event(j)
            }
            fn on_mls_event(&self, j: String) {
                self.0.on_mls_event(j)
            }
            fn on_metrics_frame(&self, f: MetricsFrame) {
                self.0.on_metrics_frame(f)
            }
            fn on_transport_state(&self, e: TransportStateEvent) {
                self.0.on_transport_state(e)
            }
            fn on_routing_decision(&self, d: RoutingDecision) {
                self.0.on_routing_decision(d)
            }
            fn on_device_capability(&self, s: DeviceCapabilitySnapshot) {
                self.0.on_device_capability(s)
            }
            fn on_extension(&self, n: String, p: String) {
                self.0.on_extension(n, p)
            }
        }
        Arc::new(TelemetrySinkAdapter {
            callback: Arc::new(Forward(sink)),
            queue: protocol.telemetry_queue.clone(),
            poll_queue_enabled: true,
        })
    }

    /// Variant of `install_sink_via_adapter` with `poll_queue_enabled=false`
    /// so push-only tests can assert the pull queue is skipped.
    fn install_sink_via_adapter_push_only(
        protocol: &OfflineProtocol,
        sink: Arc<TestTelemetrySink>,
    ) -> Arc<TelemetrySinkAdapter> {
        struct Forward(Arc<TestTelemetrySink>);
        impl TelemetrySink for Forward {
            fn on_protocol_event(&self, j: String) {
                self.0.on_protocol_event(j)
            }
            fn on_mls_event(&self, j: String) {
                self.0.on_mls_event(j)
            }
            fn on_metrics_frame(&self, f: MetricsFrame) {
                self.0.on_metrics_frame(f)
            }
            fn on_transport_state(&self, e: TransportStateEvent) {
                self.0.on_transport_state(e)
            }
            fn on_routing_decision(&self, d: RoutingDecision) {
                self.0.on_routing_decision(d)
            }
            fn on_device_capability(&self, s: DeviceCapabilitySnapshot) {
                self.0.on_device_capability(s)
            }
            fn on_extension(&self, n: String, p: String) {
                self.0.on_extension(n, p)
            }
        }
        Arc::new(TelemetrySinkAdapter {
            callback: Arc::new(Forward(sink)),
            queue: protocol.telemetry_queue.clone(),
            poll_queue_enabled: false,
        })
    }

    #[test]
    fn adapter_forwards_protocol_event_to_typed_method_and_queue() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        let record = CoreTelemetryRecord::Protocol(Box::new(CoreEvent::NetworkMetrics {
            neighbor_count: 1,
            relay_count: 0,
            delivery_ratio: 0.5,
            avg_latency_ms: 42,
        }));
        adapter.emit(&record);

        let events = sink.protocol_events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            events[0].contains("\"type\":\"network_metrics\""),
            "got {:?}",
            events[0]
        );

        let envelope = protocol
            .poll_telemetry_frame()
            .expect("poll should return the queued envelope");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(parsed["category"], "protocol");
        // Envelope shape matches the TS `TelemetryRecord` union: protocol
        // variants carry an `eventJson` string field, not a nested payload.
        let event_json = parsed["eventJson"]
            .as_str()
            .expect("eventJson must be a string");
        let event_parsed: serde_json::Value = serde_json::from_str(event_json).unwrap();
        assert_eq!(event_parsed["type"], "network_metrics");
    }

    #[test]
    fn install_telemetry_sink_with_default_config_succeeds() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        #[allow(dead_code)]
        struct Forward(Arc<TestTelemetrySink>);
        impl TelemetrySink for Forward {
            fn on_protocol_event(&self, _: String) {}
            fn on_mls_event(&self, _: String) {}
            fn on_metrics_frame(&self, _: MetricsFrame) {}
            fn on_transport_state(&self, _: TransportStateEvent) {}
            fn on_routing_decision(&self, _: RoutingDecision) {}
            fn on_device_capability(&self, _: DeviceCapabilitySnapshot) {}
            fn on_extension(&self, _: String, _: String) {}
        }
        protocol
            .install_telemetry_sink(Box::new(Forward(sink)), TelemetryConfig::default())
            .expect("install must succeed with defaults");
        // A fresh install enqueues a bootstrap metrics snapshot on the
        // next tick — we do not call process() here, so the queue is empty.
        assert!(protocol.poll_telemetry_frame().is_none());
    }

    #[test]
    fn get_transport_metrics_returns_real_data_not_mock() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        protocol.start().unwrap();
        // BLE is enabled — pull metrics must succeed and return a
        // real-data shape (is_charging is non-optional on the Rust side and
        // the BLE transport always reports a concrete boolean).
        let metrics = protocol
            .get_transport_metrics(TransportType::Ble)
            .expect("BLE transport is enabled");
        assert!(
            metrics.is_charging.is_some(),
            "is_charging should be populated from Rust TransportMetrics, got {:?}",
            metrics
        );
    }

    // ---- Poll-envelope payload-shape coverage ----
    //
    // These tests pin the wire contract between the Rust adapter and the
    // TypeScript `TelemetryRecord` discriminated union (see
    // `bindings/react-native/src/types.ts`). Each typed variant must enqueue
    // the full payload under its variant-specific key (`frame`, `event`,
    // `decision`, `snapshot`, `eventJson`) — not a truncated timestamp.

    use offline_protocol::{DeduplicatorMode, DeduplicatorStats, RetryQueueStats};
    use offline_protocol_router::{TransportScore, TransportScoreFactors};

    fn sample_core_metrics_frame() -> CoreMetricsFrame {
        CoreMetricsFrame {
            timestamp_ms: 1_700_000_000_000,
            transports: vec![(
                CoreTransportType::BLE,
                CoreTransportMetrics {
                    rssi: Some(-60),
                    latency_ms: Some(50),
                    success_count: 10,
                    failure_count: 1,
                    battery_level: Some(80),
                    is_charging: true,
                    is_active_relay: false,
                    ..Default::default()
                },
            )],
            retry_queue: RetryQueueStats {
                total_count: 3,
                ready_count: 1,
                critical_priority_count: 0,
                high_priority_count: 1,
                medium_priority_count: 1,
                low_priority_count: 1,
            },
            dedup: DeduplicatorStats {
                total_tracked: 100,
                recent_tracked: 25,
                capacity_used_percent: 12,
                false_positive_rate: None,
                mode: DeduplicatorMode::HashMap,
            },
            ack_pending: 2,
            neighbor_count: 5,
            is_local_relay: false,
            current_transport: Some(CoreTransportType::BLE),
        }
    }

    #[test]
    fn adapter_metrics_frame_envelope_carries_full_frame() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        let record = CoreTelemetryRecord::MetricsSnapshot(Box::new(sample_core_metrics_frame()));
        adapter.emit(&record);

        // Push channel got a typed frame.
        let frames = sink.metrics_frames.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].timestamp_ms, 1_700_000_000_000);
        assert_eq!(frames[0].transports.len(), 1);

        // Pull channel got the same frame, under the `frame` key, with full
        // nested content — not just a timestamp.
        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(parsed["category"], "metricsFrame");
        let frame = &parsed["frame"];
        assert!(
            frame.is_object(),
            "frame must be an object, got {:?}",
            frame
        );
        assert_eq!(frame["timestampMs"], 1_700_000_000_000_i64);
        assert_eq!(frame["ackPending"], 2);
        assert_eq!(frame["neighborCount"], 5);
        assert_eq!(frame["isLocalRelay"], false);
        assert_eq!(frame["currentTransport"], "ble");
        let transports = frame["transports"].as_array().expect("transports array");
        assert_eq!(transports.len(), 1);
        assert_eq!(transports[0]["transport"], "ble");
        assert_eq!(transports[0]["metrics"]["rssi"], -60);
        assert_eq!(transports[0]["metrics"]["batteryLevel"], 80);
    }

    #[test]
    fn adapter_transport_state_envelope_carries_full_event() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        let record = CoreTelemetryRecord::TransportState(CoreTransportStateEvent {
            timestamp_ms: 1_700_000_001_000,
            transport: CoreTransportType::WiFiDirect,
            previous: CoreTransportStatus::Available,
            current: CoreTransportStatus::Connecting,
        });
        adapter.emit(&record);

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(parsed["category"], "transportState");
        let event = &parsed["event"];
        assert_eq!(event["timestampMs"], 1_700_000_001_000_i64);
        assert_eq!(event["transport"], "wifiDirect");
        assert_eq!(event["previous"], "available");
        assert_eq!(event["current"], "connecting");
    }

    #[test]
    fn adapter_routing_decision_envelope_carries_full_decision() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        let score = TransportScore::from_factors(TransportScoreFactors {
            signal: 0.9,
            proximity: 0.8,
            bandwidth: 0.7,
            congestion: 0.6,
            energy: 0.5,
            reliability: 0.95,
            load: 0.4,
            total: 0.82,
        });
        let decision = CoreRoutingDecision {
            timestamp_ms: 1_700_000_002_000,
            phase: CoreRoutingPhase::Switched,
            from: Some(CoreTransportType::BLE),
            to: Some(CoreTransportType::WiFiDirect),
            winning_score: Some(0.82),
            reason_code: Some(CoreRoutingReasonCode::PoorSignal),
            scores: vec![(CoreTransportType::WiFiDirect, score)],
        };
        adapter.emit(&CoreTelemetryRecord::Routing(Box::new(decision)));

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(parsed["category"], "routingDecision");
        let d = &parsed["decision"];
        assert_eq!(d["timestampMs"], 1_700_000_002_000_i64);
        assert_eq!(d["phase"], "switched");
        assert_eq!(d["from"], "ble");
        assert_eq!(d["to"], "wifiDirect");
        assert_eq!(d["reasonCode"], "poorSignal");
        let scores = d["scores"].as_array().expect("scores array");
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0]["transport"], "wifiDirect");
        // Scores are f32 on the wire; compare with tolerance to avoid
        // spurious failures from the f32→f64 JSON widen.
        let signal = scores[0]["signal"].as_f64().unwrap();
        let total = scores[0]["total"].as_f64().unwrap();
        assert!((signal - 0.9).abs() < 1e-4, "signal={signal}");
        assert!((total - 0.82).abs() < 1e-4, "total={total}");
    }

    #[test]
    fn adapter_device_capability_envelope_carries_full_snapshot() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        let snapshot = CoreDeviceSnapshot {
            timestamp_ms: 1_700_000_003_000,
            battery_level: Some(42),
            is_charging: true,
            relay_role: CoreRelayRole::Relay,
            changed_fields: 0b111,
        };
        adapter.emit(&CoreTelemetryRecord::Device(snapshot));

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(parsed["category"], "deviceCapability");
        let s = &parsed["snapshot"];
        assert_eq!(s["timestampMs"], 1_700_000_003_000_i64);
        assert_eq!(s["batteryLevel"], 42);
        assert_eq!(s["isCharging"], true);
        assert_eq!(s["relayRole"], "relay");
        assert_eq!(s["changedFields"], 0b111);
    }

    #[test]
    fn adapter_protocol_event_envelope_has_event_json_string_field() {
        // Complements the existing `adapter_forwards_protocol_event_...` test
        // by pinning the field name the TS `TelemetryRecord` union expects
        // (`eventJson`), so a future rename on either side fails loudly here.
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        let record = CoreTelemetryRecord::Protocol(Box::new(CoreEvent::NetworkMetrics {
            neighbor_count: 3,
            relay_count: 1,
            delivery_ratio: 0.75,
            avg_latency_ms: 10,
        }));
        adapter.emit(&record);

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(parsed["category"], "protocol");
        assert!(
            parsed.get("eventJson").and_then(|v| v.as_str()).is_some(),
            "envelope must carry eventJson string (matches TS TelemetryRecord)"
        );
        // Must NOT carry the pre-fix `payload` key.
        assert!(
            parsed.get("payload").is_none(),
            "legacy `payload` key must not appear: {envelope}"
        );
    }

    #[test]
    fn adapter_poll_queue_drops_oldest_at_capacity() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        // Push TELEMETRY_POLL_QUEUE_CAP + 5 records of two distinct shapes
        // (device + transport-state) so we can identify which end was dropped.
        for _ in 0..TELEMETRY_POLL_QUEUE_CAP {
            adapter.emit(&CoreTelemetryRecord::Device(CoreDeviceSnapshot {
                timestamp_ms: 0,
                battery_level: Some(1),
                is_charging: false,
                relay_role: CoreRelayRole::Regular,
                changed_fields: 0,
            }));
        }
        for _ in 0..5 {
            adapter.emit(&CoreTelemetryRecord::TransportState(
                CoreTransportStateEvent {
                    timestamp_ms: 0,
                    transport: CoreTransportType::BLE,
                    previous: CoreTransportStatus::Available,
                    current: CoreTransportStatus::Disconnected,
                },
            ));
        }

        // Queue is capped, so the 5 newest (transportState) must be present
        // and 5 oldest (device) must have been dropped.
        let mut first_five_categories = Vec::with_capacity(5);
        for _ in 0..5 {
            let env = protocol.poll_telemetry_frame().expect("non-empty");
            let parsed: serde_json::Value = serde_json::from_str(&env).unwrap();
            first_five_categories.push(parsed["category"].as_str().unwrap().to_string());
        }
        // After popping 5, the last 5 remaining should be transportState.
        for _ in 0..(TELEMETRY_POLL_QUEUE_CAP - 5 - 5) {
            protocol.poll_telemetry_frame().expect("non-empty mid");
        }
        for _ in 0..5 {
            let env = protocol.poll_telemetry_frame().expect("tail");
            let parsed: serde_json::Value = serde_json::from_str(&env).unwrap();
            assert_eq!(
                parsed["category"], "transportState",
                "tail must be the newly-pushed transportState records (drop-oldest semantics)"
            );
        }
        assert!(protocol.poll_telemetry_frame().is_none(), "queue drained");
    }

    #[test]
    fn adapter_protocol_serialization_failure_emits_extension_fallback() {
        // Exercise `emit_serialization_failure` directly. The real Protocol
        // and Mls arms only fall through when `to_json` / `to_string` fails,
        // which is effectively unreachable for the stable `Event` shape; the
        // fallback itself is still worth pinning to prove we no longer drop
        // silently.
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        struct FakeErr;
        impl std::fmt::Display for FakeErr {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "synthetic")
            }
        }
        adapter.emit_serialization_failure("protocol", &FakeErr);

        let extensions = sink.extensions.lock().unwrap();
        assert_eq!(extensions.len(), 1);
        let (name, payload_json) = &extensions[0];
        assert_eq!(name, "telemetry.error.protocol");
        let parsed_payload: serde_json::Value =
            serde_json::from_str(payload_json).expect("payload_json must parse");
        assert_eq!(parsed_payload["telemetry_error"], "serialization_failed");
        assert_eq!(parsed_payload["record"], "protocol");

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(parsed["category"], "extension");
        assert_eq!(parsed["name"], "telemetry.error.protocol");
        assert!(parsed["payloadJson"].is_string());
    }

    #[test]
    fn get_transport_metrics_returns_none_for_disabled_transport() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        protocol.start().unwrap();
        // `create_ble_only_config` only registers BLE with the transport
        // manager. The new implementation of `get_transport_metrics` pulls
        // from `TransportManager::get_transport(...)` — disabled transports
        // return `None`. This is the breaking contract change called out
        // in CHANGELOG (prior behaviour: always Some(zeroed_struct)).
        assert!(
            protocol
                .get_transport_metrics(TransportType::WiFiDirect)
                .is_none(),
            "disabled transport must return None, not a zeroed stub"
        );
        assert!(
            protocol
                .get_transport_metrics(TransportType::Nostr)
                .is_none(),
            "disabled transport must return None, not a zeroed stub"
        );
    }

    #[test]
    fn adapter_sink_replacement_shares_poll_queue_but_isolates_push() {
        // Installing a second sink replaces the first on the core side, but
        // both adapters share the same `protocol.telemetry_queue`. This
        // test pins both halves of that contract: (a) sink A only sees the
        // records emitted while it was installed, sink B only sees its
        // own, and (b) the pull queue accumulates both in order regardless
        // of which sink is currently live.
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();

        let sink_a = Arc::new(TestTelemetrySink::default());
        let adapter_a = install_sink_via_adapter(&protocol, sink_a.clone());
        adapter_a.emit(&CoreTelemetryRecord::Device(CoreDeviceSnapshot {
            timestamp_ms: 1,
            battery_level: None,
            is_charging: false,
            relay_role: CoreRelayRole::Regular,
            changed_fields: 0,
        }));

        let sink_b = Arc::new(TestTelemetrySink::default());
        let adapter_b = install_sink_via_adapter(&protocol, sink_b.clone());
        adapter_b.emit(&CoreTelemetryRecord::Device(CoreDeviceSnapshot {
            timestamp_ms: 2,
            battery_level: None,
            is_charging: false,
            relay_role: CoreRelayRole::Regular,
            changed_fields: 0,
        }));

        assert_eq!(sink_a.device_snapshots.lock().unwrap().len(), 1);
        assert_eq!(sink_b.device_snapshots.lock().unwrap().len(), 1);

        let e1 = protocol.poll_telemetry_frame().expect("first envelope");
        let e2 = protocol.poll_telemetry_frame().expect("second envelope");
        let p1: serde_json::Value = serde_json::from_str(&e1).unwrap();
        let p2: serde_json::Value = serde_json::from_str(&e2).unwrap();
        assert_eq!(p1["snapshot"]["timestampMs"], 1);
        assert_eq!(p2["snapshot"]["timestampMs"], 2);
        assert!(
            protocol.poll_telemetry_frame().is_none(),
            "queue drained cleanly"
        );
    }

    #[test]
    fn enqueue_serialization_failure_rich_variant_does_not_fire_extension_callback() {
        // Rich-variant failure contract: the pull queue gets an
        // extension-error envelope under `telemetry.error.<variant>`, and
        // `on_extension` does NOT fire (the typed push already carried
        // the DTO — double-firing would deliver the same emit twice).
        //
        // Serde-level failure is effectively unreachable in practice for
        // rich variants (`serde_json::to_value` maps NaN/Inf to `null`
        // rather than erroring), so we exercise the helper directly. This
        // test pins the contract of `enqueue_serialization_failure` so a
        // future refactor that accidentally re-routes rich-variant
        // failures through `emit_serialization_failure` (which also fires
        // the callback) trips the assertion below.
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        struct FakeErr;
        impl std::fmt::Display for FakeErr {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "synthetic")
            }
        }
        adapter.enqueue_serialization_failure("metricsFrame", &FakeErr);

        assert!(
            sink.extensions.lock().unwrap().is_empty(),
            "on_extension must not fire — rich-variant typed push already delivered",
        );

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(parsed["category"], "extension");
        assert_eq!(parsed["name"], "telemetry.error.metricsFrame");
        assert!(
            parsed.get("frame").is_none(),
            "pull channel must not leak a null-valued frame on serialization failure"
        );
    }

    // ---- Shape-parity snapshot tests ----
    //
    // These tests pin the EXACT JSON envelope shape for every `category` the
    // adapter produces. They are the canonical contract that the
    // handwritten iOS (`bindings/react-native/ios/OfflineProtocolModule.swift`
    // `TelemetrySinkImpl.encode(...)`) and Android
    // (`bindings/react-native/android/.../OfflineProtocolModule.kt`
    // `encodeFrame` / `encodeRouting` / ...) encoders must reproduce bit-for-
    // bit when dispatching push events to React Native. If these tests change
    // shape (new field, renamed key, different casing), the iOS and Android
    // encoders MUST be updated in the same PR — the TS `TelemetryRecord`
    // discriminated union (`bindings/react-native/src/types.ts`) flows from
    // this shape.
    //
    // We assert `parsed == json!({...})` rather than checking individual
    // fields so the tests fail on unexpected additions too.

    /// Fixture with clean float values so the snapshot comparison is not
    /// sensitive to f32 precision drift. `success_count=0, failure_count=0`
    /// makes `effective_drop_ratio()` return None → error_rate = 0.0.
    fn shape_parity_metrics_fixture() -> CoreMetricsFrame {
        CoreMetricsFrame {
            timestamp_ms: 1_700_000_000_000,
            transports: vec![(
                CoreTransportType::BLE,
                CoreTransportMetrics {
                    rssi: Some(-60),
                    latency_ms: Some(50),
                    success_count: 0,
                    failure_count: 0,
                    battery_level: Some(80),
                    is_charging: true,
                    is_active_relay: false,
                    ..Default::default()
                },
            )],
            retry_queue: RetryQueueStats {
                total_count: 3,
                ready_count: 1,
                critical_priority_count: 0,
                high_priority_count: 1,
                medium_priority_count: 1,
                low_priority_count: 1,
            },
            dedup: DeduplicatorStats {
                total_tracked: 100,
                recent_tracked: 25,
                capacity_used_percent: 12,
                false_positive_rate: None,
                mode: DeduplicatorMode::HashMap,
            },
            ack_pending: 2,
            neighbor_count: 5,
            is_local_relay: false,
            current_transport: Some(CoreTransportType::BLE),
        }
    }

    #[test]
    fn shape_parity_metrics_frame_envelope() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        let record = CoreTelemetryRecord::MetricsSnapshot(Box::new(shape_parity_metrics_fixture()));
        adapter.emit(&record);

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({
                "category": "metricsFrame",
                "frame": {
                    "timestampMs": 1_700_000_000_000_i64,
                    "transports": [{
                        "transport": "ble",
                        "metrics": {
                            "errorRate": 0.0,
                            "avgLatencyMs": 50,
                            "packetsSent": 0,
                            "packetsReceived": 0,
                            "bytesSent": 0,
                            "bytesReceived": 0,
                            "rssi": -60,
                            "batteryLevel": 80,
                            "isCharging": true,
                            "congestion": 0.0,
                            "queueDepth": 0,
                            "relayConnectionCount": 0,
                            "isActiveRelay": false,
                        }
                    }],
                    "retryQueue": {
                        "totalCount": 3,
                        "readyCount": 1,
                        "criticalPriorityCount": 0,
                        "highPriorityCount": 1,
                        "mediumPriorityCount": 1,
                        "lowPriorityCount": 1,
                    },
                    "dedup": {
                        "totalTracked": 100,
                        "recentTracked": 25,
                        "capacityUsedPercent": 12,
                        "mode": "hashMap",
                    },
                    "ackPending": 2,
                    "neighborCount": 5,
                    "isLocalRelay": false,
                    "currentTransport": "ble",
                }
            }),
            "metricsFrame envelope shape changed — update iOS encode(frame:) and Android encodeFrame() in lockstep, then update this snapshot",
        );
    }

    #[test]
    fn shape_parity_transport_state_envelope() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        adapter.emit(&CoreTelemetryRecord::TransportState(
            CoreTransportStateEvent {
                timestamp_ms: 1_700_000_001_000,
                transport: CoreTransportType::WiFiDirect,
                previous: CoreTransportStatus::Available,
                current: CoreTransportStatus::Connecting,
            },
        ));

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({
                "category": "transportState",
                "event": {
                    "timestampMs": 1_700_000_001_000_i64,
                    "transport": "wifiDirect",
                    "previous": "available",
                    "current": "connecting",
                }
            }),
            "transportState envelope shape changed — update iOS encode(event:) and Android encodeTransportState() in lockstep",
        );
    }

    #[test]
    fn shape_parity_routing_decision_envelope() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        // Hand-built score with tidy rational values — f32 serde yields
        // exact round-trip for these, so the snapshot stays stable across
        // platforms.
        let score = TransportScore::from_factors(TransportScoreFactors {
            signal: 0.5,
            proximity: 0.25,
            bandwidth: 0.125,
            congestion: 0.0625,
            energy: 0.125,
            reliability: 0.5,
            load: 0.25,
            total: 0.5,
        });
        let decision = CoreRoutingDecision {
            timestamp_ms: 1_700_000_002_000,
            phase: CoreRoutingPhase::Switched,
            from: Some(CoreTransportType::BLE),
            to: Some(CoreTransportType::WiFiDirect),
            winning_score: Some(0.5),
            reason_code: Some(CoreRoutingReasonCode::PoorSignal),
            scores: vec![(CoreTransportType::WiFiDirect, score)],
        };
        adapter.emit(&CoreTelemetryRecord::Routing(Box::new(decision)));

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({
                "category": "routingDecision",
                "decision": {
                    "timestampMs": 1_700_000_002_000_i64,
                    "phase": "switched",
                    "from": "ble",
                    "to": "wifiDirect",
                    "winningScore": 0.5,
                    "reasonCode": "poorSignal",
                    "scores": [{
                        "transport": "wifiDirect",
                        "signal": 0.5,
                        "proximity": 0.25,
                        "bandwidth": 0.125,
                        "congestion": 0.0625,
                        "energy": 0.125,
                        "reliability": 0.5,
                        "load": 0.25,
                        "total": 0.5,
                    }]
                }
            }),
            "routingDecision envelope shape changed — update iOS encode(decision:) and Android encodeRouting() in lockstep",
        );
    }

    #[test]
    fn shape_parity_device_capability_envelope() {
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        adapter.emit(&CoreTelemetryRecord::Device(CoreDeviceSnapshot {
            timestamp_ms: 1_700_000_003_000,
            battery_level: Some(42),
            is_charging: true,
            relay_role: CoreRelayRole::Relay,
            changed_fields: 0b111,
        }));

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({
                "category": "deviceCapability",
                "snapshot": {
                    "timestampMs": 1_700_000_003_000_i64,
                    "batteryLevel": 42,
                    "isCharging": true,
                    "relayRole": "relay",
                    "changedFields": 7,
                }
            }),
            "deviceCapability envelope shape changed — update iOS encode(snapshot:) and Android encodeDevice() in lockstep",
        );
    }

    #[test]
    fn shape_parity_protocol_envelope_is_eventjson_string() {
        // Protocol and Mls variants carry a pre-serialized event JSON
        // string. The envelope wraps it as `eventJson: string`; the inner
        // event shape is owned by the core `Event` enum, not this FFI crate.
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        adapter.emit(&CoreTelemetryRecord::Protocol(Box::new(
            CoreEvent::NetworkMetrics {
                neighbor_count: 3,
                relay_count: 1,
                delivery_ratio: 0.75,
                avg_latency_ms: 10,
            },
        )));

        let envelope = protocol.poll_telemetry_frame().expect("envelope queued");
        let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(parsed["category"], "protocol");
        let keys: Vec<&str> = parsed
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            keys,
            vec!["category", "eventJson"],
            "protocol envelope MUST have exactly two keys — update iOS/Android onProtocolEvent dispatch if this changes",
        );
        assert!(
            parsed["eventJson"].is_string(),
            "eventJson must be a string"
        );
    }

    // ---- Forward-compat coverage: pin the known variant set ----

    #[test]
    fn forward_compat_extension_coverage() {
        // Every TelemetryRecord variant this FFI build knows how to type
        // MUST route to its dedicated `on_*` callback. If a new variant is
        // added to `CoreTelemetryRecord` without a matching arm in
        // `TelemetrySinkAdapter::emit`, it falls through to the `other =>`
        // extension arm. This test pins the known set so the drift is
        // noticed the moment the new variant ships.
        //
        // We cannot rely on Rust exhaustiveness here — `CoreTelemetryRecord`
        // is `#[non_exhaustive]` in the core crate, so the match must keep
        // a catchall arm. This test is the compile-time-adjacent safety net.
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink.clone());

        let records: Vec<CoreTelemetryRecord> = vec![
            CoreTelemetryRecord::Protocol(Box::new(CoreEvent::NetworkMetrics {
                neighbor_count: 0,
                relay_count: 0,
                delivery_ratio: 0.0,
                avg_latency_ms: 0,
            })),
            CoreTelemetryRecord::MetricsSnapshot(Box::new(sample_core_metrics_frame())),
            CoreTelemetryRecord::TransportState(CoreTransportStateEvent {
                timestamp_ms: 0,
                transport: CoreTransportType::BLE,
                previous: CoreTransportStatus::Available,
                current: CoreTransportStatus::Available,
            }),
            CoreTelemetryRecord::Routing(Box::new(CoreRoutingDecision {
                timestamp_ms: 0,
                phase: CoreRoutingPhase::Selected,
                from: None,
                to: None,
                winning_score: None,
                reason_code: None,
                scores: vec![],
            })),
            CoreTelemetryRecord::Device(CoreDeviceSnapshot {
                timestamp_ms: 0,
                battery_level: None,
                is_charging: false,
                relay_role: CoreRelayRole::Regular,
                changed_fields: 0,
            }),
            CoreTelemetryRecord::Mls(offline_protocol::MlsLifecycleEvent::Initialized {
                timestamp_ms: 0,
                session_id: "s".into(),
                group_id: None,
                peer_id: None,
                context: offline_protocol::MlsOperationContext::Initialize,
                error_category: None,
            }),
        ];

        for rec in &records {
            adapter.emit(rec);
        }

        while let Some(envelope) = protocol.poll_telemetry_frame() {
            let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
            let category = parsed["category"].as_str().unwrap();
            assert_ne!(
                category,
                "extension",
                "record `{}` fell through to the forward-compat extension arm — \
                 add a typed match arm in `TelemetrySinkAdapter::emit` and a \
                 matching foreign-side dispatch before shipping the new variant",
                parsed
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<unknown>"),
            );
        }
    }

    // ---- enable_poll_queue opt-in ----

    #[test]
    fn push_only_sink_skips_pull_queue() {
        // With `poll_queue_enabled = false`, the adapter fires typed
        // callbacks but does NOT enqueue envelopes. Protects the routing
        // hot path from per-send JSON serialization for push-only apps.
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter_push_only(&protocol, sink.clone());

        adapter.emit(&CoreTelemetryRecord::MetricsSnapshot(Box::new(
            sample_core_metrics_frame(),
        )));
        adapter.emit(&CoreTelemetryRecord::Device(CoreDeviceSnapshot {
            timestamp_ms: 42,
            battery_level: Some(10),
            is_charging: false,
            relay_role: CoreRelayRole::Regular,
            changed_fields: 1,
        }));
        adapter.emit(&CoreTelemetryRecord::TransportState(
            CoreTransportStateEvent {
                timestamp_ms: 43,
                transport: CoreTransportType::BLE,
                previous: CoreTransportStatus::Available,
                current: CoreTransportStatus::Disconnected,
            },
        ));

        // Typed push callbacks still fire.
        assert_eq!(sink.metrics_frames.lock().unwrap().len(), 1);
        assert_eq!(sink.device_snapshots.lock().unwrap().len(), 1);
        assert_eq!(sink.transport_states.lock().unwrap().len(), 1);

        // Pull queue stays empty — no envelope was built or enqueued.
        assert!(
            protocol.poll_telemetry_frame().is_none(),
            "pull queue must be empty when enable_poll_queue=false",
        );
    }

    #[test]
    fn push_only_sink_protocol_event_string_variant_skips_pull_queue() {
        // Protocol / Mls variants take a different path (pre-serialized
        // JSON string). Make sure that path also respects the flag.
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter_push_only(&protocol, sink.clone());

        adapter.emit(&CoreTelemetryRecord::Protocol(Box::new(
            CoreEvent::NetworkMetrics {
                neighbor_count: 1,
                relay_count: 0,
                delivery_ratio: 0.5,
                avg_latency_ms: 42,
            },
        )));

        assert_eq!(sink.protocol_events.lock().unwrap().len(), 1);
        assert!(
            protocol.poll_telemetry_frame().is_none(),
            "pull queue must be empty when enable_poll_queue=false",
        );
    }

    #[test]
    fn install_telemetry_sink_enable_poll_queue_false_routes_through() {
        // End-to-end: install_telemetry_sink reads config.enable_poll_queue
        // and forwards it to the adapter. We do not call process(), so the
        // bootstrap metrics snapshot never fires; after install the queue
        // must remain empty whether or not the flag was set, but this test
        // pins that the public install path honours the opt-out.
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();

        #[allow(dead_code)]
        struct Forward;
        impl TelemetrySink for Forward {
            fn on_protocol_event(&self, _: String) {}
            fn on_mls_event(&self, _: String) {}
            fn on_metrics_frame(&self, _: MetricsFrame) {}
            fn on_transport_state(&self, _: TransportStateEvent) {}
            fn on_routing_decision(&self, _: RoutingDecision) {}
            fn on_device_capability(&self, _: DeviceCapabilitySnapshot) {}
            fn on_extension(&self, _: String, _: String) {}
        }

        let cfg = TelemetryConfig {
            enable_poll_queue: Some(false),
            ..Default::default()
        };
        protocol
            .install_telemetry_sink(Box::new(Forward), cfg)
            .expect("install must succeed");
        assert!(protocol.poll_telemetry_frame().is_none());
    }

    #[test]
    fn uninstall_telemetry_sink_drains_queue_and_detaches() {
        // Pin the uninstall contract: after calling it (a) the pull queue
        // is drained, so subsequent `poll_telemetry_frame` returns None,
        // and (b) future emissions through a freshly-built adapter do NOT
        // land in the queue because the core-side sink is now a NoopTS.
        let protocol = OfflineProtocol::new(create_ble_only_config()).unwrap();
        let sink = Arc::new(TestTelemetrySink::default());
        let adapter = install_sink_via_adapter(&protocol, sink);

        // Prime the queue with a couple of records via the adapter.
        let record = CoreTelemetryRecord::Protocol(Box::new(CoreEvent::NetworkMetrics {
            neighbor_count: 1,
            relay_count: 0,
            delivery_ratio: 0.5,
            avg_latency_ms: 42,
        }));
        adapter.emit(&record);
        adapter.emit(&record);
        assert!(protocol.poll_telemetry_frame().is_some());

        // Uninstall drains the queue AND replaces the core sink.
        protocol
            .uninstall_telemetry_sink()
            .expect("uninstall must succeed");
        assert!(protocol.poll_telemetry_frame().is_none());

        // Idempotent: second call is a no-op and still succeeds.
        protocol
            .uninstall_telemetry_sink()
            .expect("uninstall must be idempotent");
        assert!(protocol.poll_telemetry_frame().is_none());
    }

    #[test]
    fn receive_message_json_round_trips_into_core_message() {
        // The documented `forward_message` contract: the JSON emitted by
        // `receive_message` must parse back into a core `Message` — with
        // the forwarding attribution and the cloud-media metadata
        // (download URL + content key) intact. This is exactly what an app
        // does when it forwards a received media message.
        let mut msg = offline_protocol_core::Message::new(
            offline_protocol_core::UserId::new("alice").unwrap(),
            offline_protocol_core::UserId::new("bob").unwrap(),
            offline_protocol_core::AppId::new("test-app").unwrap(),
            "check out this photo",
        );
        msg.content_type = offline_protocol_core::ContentType::Image;
        msg.media_metadata = Some(offline_protocol_core::MediaMetadata {
            mime_type: "image/jpeg".to_string(),
            file_name: "photo.jpg".to_string(),
            file_size: 42,
            duration_ms: None,
            width: Some(10),
            height: Some(10),
            thumbnail_base64: None,
            media_id: None,
            download_url: Some("https://cdn.example/blob/1".to_string()),
            thumbnail_url: None,
            encryption_key: Some("a2V5LWJ5dGVz".to_string()),
            iv: Some("aXYtYnl0ZXM=".to_string()),
            ciphertext_hash: None,
            sticker_provider: None,
            sticker_remote_id: None,
            sticker_kind: None,
        });
        msg.forwarded_from = Some(offline_protocol_core::ForwardInfo {
            original_sender: offline_protocol_core::UserId::new("dave").unwrap(),
            original_message_id: offline_protocol_core::MessageId::new(),
            original_timestamp: offline_protocol_core::Timestamp::now(),
            forward_count: 1,
        });

        let json = received_message_to_json(&msg).expect("serialization must succeed");
        let parsed: offline_protocol_core::Message =
            serde_json::from_str(&json).expect("receive_message JSON must parse as core Message");

        assert_eq!(parsed.id, msg.id);
        assert_eq!(parsed.sender, msg.sender);
        assert_eq!(parsed.recipient, msg.recipient);
        assert_eq!(parsed.app_id, msg.app_id);
        assert_eq!(parsed.ttl, msg.ttl);
        assert_eq!(parsed.priority, msg.priority);
        assert_eq!(parsed.content, msg.content);
        assert_eq!(
            parsed.content_type,
            offline_protocol_core::ContentType::Image
        );
        let media = parsed.media_metadata.expect("media metadata must survive");
        assert_eq!(media.encryption_key.as_deref(), Some("a2V5LWJ5dGVz"));
        assert_eq!(media.iv.as_deref(), Some("aXYtYnl0ZXM="));
        assert_eq!(
            media.download_url.as_deref(),
            Some("https://cdn.example/blob/1")
        );
        let fwd = parsed.forwarded_from.expect("attribution must survive");
        assert_eq!(fwd.original_sender.as_str(), "dave");
        assert_eq!(fwd.forward_count, 1);
    }
}
