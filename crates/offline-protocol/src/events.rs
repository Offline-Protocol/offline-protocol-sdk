//! Event types and callbacks.

use offline_protocol_core::{ContentType, MediaMetadata, Message, MessageId};
use offline_protocol_services::ServiceEvent;
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Presence status for a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    /// Peer is online and available.
    Online,
    /// Peer is away / idle.
    Away,
    /// Peer is explicitly offline.
    Offline,
}

/// Event callback type for handling protocol events.
pub type EventCallback = Arc<dyn Fn(Event) + Send + Sync>;

/// Machine-readable reason taxonomy for welcome delivery failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WelcomeReasonCode {
    /// No transport was available or all transport sends failed.
    TransportUnavailable,
    /// Peer was disconnected or became unavailable during send.
    PeerDisconnected,
    /// Send operation timed out.
    Timeout,
    /// Local/internal error prevented send.
    InternalError,
    /// Retry budget or lifecycle TTL was exhausted.
    RetryExhausted,
}

impl WelcomeReasonCode {
    /// Returns the stable machine-readable reason code.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TransportUnavailable => "TRANSPORT_UNAVAILABLE",
            Self::PeerDisconnected => "PEER_DISCONNECTED",
            Self::Timeout => "TIMEOUT",
            Self::InternalError => "INTERNAL_ERROR",
            Self::RetryExhausted => "RETRY_EXHAUSTED",
        }
    }
}

/// Machine-readable reason for DORS selection or switch (observability).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DorsReasonCode {
    /// First selection; no previous transport.
    InitialSelection,
    /// DORS selected primary (before send attempt).
    PrimarySelected,
    /// Primary send succeeded; active transport is now primary.
    PrimarySuccess,
    /// Primary failed; fallback send succeeded.
    FallbackSuccess,
    /// BLE → WiFi fallback succeeded (escalation applied).
    EscalationApplied,
    /// Previous transport unavailable; switched to best available.
    CurrentUnavailable,
}

impl DorsReasonCode {
    /// Returns the stable machine-readable reason code.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InitialSelection => "INITIAL_SELECTION",
            Self::PrimarySelected => "PRIMARY_SELECTED",
            Self::PrimarySuccess => "PRIMARY_SUCCESS",
            Self::FallbackSuccess => "FALLBACK_SUCCESS",
            Self::EscalationApplied => "ESCALATION_APPLIED",
            Self::CurrentUnavailable => "CURRENT_UNAVAILABLE",
        }
    }
}

/// Phase of DORS escalation: recommendation (trigger boundary) vs actual transition (applied).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DorsEscalationPhase {
    /// DORS decided to escalate (typed trigger reason); fallback may not succeed.
    Triggered,
    /// BLE→WiFi fallback send succeeded; escalation was applied.
    Applied,
}

impl DorsEscalationPhase {
    /// Returns the stable string representation (TRIGGERED / APPLIED).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Triggered => "TRIGGERED",
            Self::Applied => "APPLIED",
        }
    }
}

/// Machine-readable reason for DORS escalation (BLE→Wi‑Fi) observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DorsEscalationReasonCode {
    /// Fallback to WiFi succeeded after primary (BLE) send failed.
    FallbackSuccess,
    /// Escalation suggested due to retry threshold (e.g. ble_to_wifi_retry_threshold).
    RetryThreshold,
    /// Escalation suggested due to sustained poor BLE signal.
    PoorSignal,
    /// Escalation suggested due to congestion.
    Congestion,
    /// Escalation suggested due to low TTL on messages.
    LowTtl,
    /// Escalation suggested due to BLE success rate below configured minimum (quality degradation).
    LowSuccessRate,
}

impl DorsEscalationReasonCode {
    /// Returns the stable machine-readable reason code.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FallbackSuccess => "FALLBACK_SUCCESS",
            Self::RetryThreshold => "RETRY_THRESHOLD",
            Self::PoorSignal => "POOR_SIGNAL",
            Self::Congestion => "CONGESTION",
            Self::LowTtl => "LOW_TTL",
            Self::LowSuccessRate => "LOW_SUCCESS_RATE",
        }
    }
}

/// Machine-readable reason taxonomy for inbound decryption failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DecryptionFailureCode {
    /// Incoming encrypted payload is malformed and cannot be parsed.
    InvalidPayload,
    /// MLS subsystem is not initialized locally.
    NotInitialized,
    /// Ciphertext is invalid or cannot be decrypted.
    InvalidCiphertext,
    /// Sender identity/signature verification failed.
    IdentityMismatch,
    /// Cryptographic operation failed.
    CryptoFailure,
    /// Failure class is unknown.
    Unknown,
}

/// Events that can occur in the protocol.
///
/// Note: This type implements a custom Debug that redacts sensitive fields
/// (message content) to prevent accidental logging of sensitive data.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A message was sent (queued for delivery).
    MessageSent {
        /// ID of the sent message.
        message_id: String,
        /// Sender's user ID.
        sender: String,
        /// Recipient's user ID.
        recipient: String,
        /// Content of the sent message.
        content: String,
        /// Priority of the message when sent.
        priority: String,
        /// Whether the message requires an acknowledgement.
        requires_ack: bool,
        /// When the message was queued for delivery (wall-clock, for display).
        timestamp: i64,
        /// Lamport logical clock value for causal ordering.
        #[serde(default)]
        lamport_clock: u64,
    },

    /// A message was received.
    MessageReceived {
        /// ID of the received message.
        message_id: String,
        /// Sender's user ID.
        sender: String,
        /// Recipient's user ID.
        recipient: String,
        /// Message content.
        content: String,
        /// Number of hops the message traversed.
        hop_count: u8,
        /// Transport used for final delivery.
        transport: String,
        /// When the message was received (wall-clock, for display).
        timestamp: i64,
        /// Lamport logical clock value for causal ordering.
        #[serde(default)]
        lamport_clock: u64,
        /// ID of the message this is replying to (optional).
        #[serde(skip_serializing_if = "Option::is_none")]
        reply_to_msg: Option<String>,
        /// The type of content (text, image, video, etc.).
        #[serde(default)]
        content_type: String,
        /// Media metadata (present for non-text content).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_metadata: Option<MediaMetadata>,
    },

    /// A message was successfully delivered (ACK received).
    MessageDelivered {
        /// ID of the delivered message.
        message_id: String,
        /// Latency in milliseconds.
        latency_ms: u64,
        /// Number of hops traversed.
        hop_count: u8,
        /// Transport used.
        transport: String,
    },

    /// A message failed to deliver.
    MessageFailed {
        /// ID of the failed message.
        message_id: String,
        /// Reason for failure.
        reason: String,
        /// Number of retries attempted.
        retry_count: u32,
    },

    /// Failed to decrypt an inbound encrypted message.
    MessageDecryptionFailed {
        /// ID of the message that could not be decrypted.
        message_id: String,
        /// Sender's user ID.
        sender: String,
        /// Machine-readable decryption failure code.
        code: DecryptionFailureCode,
        /// Clear failure reason for application handling/logging.
        reason: String,
    },

    /// Transport was switched by DORS.
    TransportSwitched {
        /// Previous transport (if any).
        from: Option<String>,
        /// New transport.
        to: String,
        /// Reason for switch.
        reason: String,
    },

    /// This device was promoted to relay role.
    RelayPromoted {
        /// Number of connections when promoted.
        connection_count: usize,
        /// Battery level when promoted.
        battery_level: u8,
    },

    /// This device was demoted from relay role.
    RelayDemoted {
        /// Reason for demotion.
        reason: String,
    },

    /// A new neighbor was discovered.
    NeighborDiscovered {
        /// Peer ID of the neighbor.
        peer_id: String,
        /// Transport used to discover.
        transport: String,
        /// RSSI signal strength (if available).
        rssi: Option<i16>,
    },

    /// A neighbor was lost (disconnected).
    NeighborLost {
        /// Peer ID of the lost neighbor.
        peer_id: String,
    },

    /// Network metrics update.
    NetworkMetrics {
        /// Number of active neighbors.
        neighbor_count: usize,
        /// Number of active relays.
        relay_count: usize,
        /// Message delivery ratio (0.0-1.0).
        delivery_ratio: f32,
        /// Average message latency in milliseconds.
        avg_latency_ms: u64,
    },

    /// File transfer progress update.
    FileProgress {
        /// File identifier.
        file_id: String,
        /// Number of chunks sent so far.
        chunks_sent: u32,
        /// Total number of chunks.
        total_chunks: u32,
        /// Progress percentage (0-100).
        percentage: u8,
    },

    /// File was completely received.
    FileReceived {
        /// File identifier.
        file_id: String,
        /// File name.
        file_name: String,
        /// File size in bytes.
        file_size: u64,
        /// Sender's user ID.
        sender: String,
        /// The content type of the media (image, video, file, etc.).
        content_type: String,
        /// Media metadata from the first chunk (present for typed media).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_metadata: Option<MediaMetadata>,
        /// Base64-encoded reassembled file data.
        file_data: String,
    },

    /// All chunks of a media attachment were ACK-delivered.
    MediaSent {
        /// File identifier for tracking.
        file_id: String,
        /// The content type of the media.
        content_type: String,
        /// Recipient's user ID.
        recipient: String,
    },

    /// A message was deferred due to network conditions.
    /// The message will be retried automatically.
    MessageDeferred {
        /// ID of the deferred message.
        message_id: String,
        /// Reason for deferral.
        reason: String,
        /// Current retry count.
        retry_count: u32,
        /// Next retry scheduled time (Unix timestamp ms).
        next_retry_at: Option<i64>,
    },

    /// A pending ACK was evicted due to capacity constraints.
    AckEvicted {
        /// ID of the message whose ACK was evicted.
        message_id: String,
        /// Priority of the evicted message.
        priority: String,
        /// Reason for eviction.
        reason: String,
    },

    /// A fragment assembly was evicted to make room for new fragments.
    FragmentAssemblyEvicted {
        /// ID of the message whose fragments were evicted.
        message_id: String,
        /// Completion percentage when evicted.
        completion_percent: u8,
        /// Reason for eviction.
        reason: String,
    },

    /// Relay role was demoted due to battery constraints.
    RelayDemotedBattery {
        /// Battery level at time of demotion.
        battery_level: u8,
        /// Minimum required battery level.
        min_required: u8,
    },

    /// A secure MLS session was successfully established.
    SecureSessionEstablished {
        /// Peer ID of the other party.
        peer_id: String,
        /// MLS group ID for the session.
        group_id: String,
        /// Whether this is a 1:1 session (true) or a multi-party group (false).
        is_session: bool,
        /// Whether the local device initiated the session (sent the Welcome).
        initiated_by_local: bool,
    },

    /// Failed to establish a secure MLS session.
    SecureSessionFailed {
        /// Peer ID of the other party.
        peer_id: String,
        /// Reason for the failure.
        reason: String,
    },

    /// Welcome send lifecycle entered attempted state.
    WelcomeSendAttempted {
        /// Peer identifier for this welcome lifecycle.
        peer_id: String,
        /// Welcome message identifier for lifecycle correlation.
        message_id: String,
        /// MLS group identifier associated with the welcome.
        group_id: String,
        /// 1-based send attempt number.
        attempt: u32,
    },

    /// Welcome send lifecycle reached successful sent state.
    WelcomeSendSucceeded {
        /// Peer identifier for this welcome lifecycle.
        peer_id: String,
        /// Welcome message identifier for lifecycle correlation.
        message_id: String,
        /// MLS group identifier associated with the welcome.
        group_id: String,
        /// 1-based send attempt number.
        attempt: u32,
    },

    /// Welcome send attempt failed and may be retried.
    WelcomeSendFailed {
        /// Peer identifier for this welcome lifecycle.
        peer_id: String,
        /// Welcome message identifier for lifecycle correlation.
        message_id: String,
        /// MLS group identifier associated with the welcome.
        group_id: String,
        /// 1-based send attempt number.
        attempt: u32,
        /// Machine-readable reason code.
        reason_code: WelcomeReasonCode,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Optional transport/native error detail.
        transport_error: Option<String>,
        /// Whether this failure will be retried.
        retryable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Next scheduled retry timestamp in Unix ms.
        next_retry_at: Option<i64>,
    },

    /// Welcome send lifecycle reached terminal expiry.
    WelcomeSendExpired {
        /// Peer identifier for this welcome lifecycle.
        peer_id: String,
        /// Welcome message identifier for lifecycle correlation.
        message_id: String,
        /// Final attempt number when lifecycle expired.
        attempt: u32,
        /// Terminal machine-readable reason code.
        reason_code: WelcomeReasonCode,
    },

    /// A connection request was received from another user.
    ConnectionRequestReceived {
        /// User ID of the sender.
        sender: String,
        /// Display name of the sender.
        sender_name: String,
        /// Timestamp of the request (Unix ms).
        timestamp: i64,
        /// MLS key package data (if provided for encrypted session setup).
        #[serde(skip_serializing_if = "Option::is_none")]
        key_package: Option<Vec<u8>>,
    },

    /// A previously sent connection request was accepted.
    ConnectionAccepted {
        /// User ID of the accepting party.
        accepted_by: String,
        /// Display name of the accepting party.
        accepted_by_name: String,
        /// Timestamp of the acceptance (Unix ms).
        timestamp: i64,
        /// MLS key package data (if provided for encrypted session setup).
        #[serde(skip_serializing_if = "Option::is_none")]
        key_package: Option<Vec<u8>>,
    },

    /// A previously sent connection request was rejected.
    ConnectionRejected {
        /// User ID of the rejecting party.
        rejected_by: String,
    },

    // --- Group (relay) events ---
    /// A group was created (from relay).
    GroupCreated {
        /// Group identifier.
        group_id: String,
        /// Human-readable group name.
        name: String,
    },

    /// A message was received in a group (from relay).
    GroupMessageReceived {
        /// Group identifier.
        group_id: String,
        /// User ID of the sender.
        sender: String,
        /// Message content.
        content: String,
        /// ISO-8601 timestamp.
        timestamp: String,
        /// Unique message identifier.
        message_id: String,
        /// Optional reply-to message ID.
        #[serde(skip_serializing_if = "Option::is_none")]
        reply_to_msg: Option<String>,
    },

    /// A member was added to a group (from relay).
    GroupMemberAdded {
        /// Group identifier.
        group_id: String,
        /// User ID of the added member.
        user_id: String,
        /// User ID of who performed the add.
        added_by: String,
        /// Human-readable group name (present when the invitee first joins).
        #[serde(skip_serializing_if = "Option::is_none")]
        group_name: Option<String>,
    },

    /// A member was removed from a group (from relay).
    GroupMemberRemoved {
        /// Group identifier.
        group_id: String,
        /// User ID of the removed member.
        user_id: String,
        /// User ID of who performed the removal.
        removed_by: String,
    },

    /// Group info was received (from relay).
    GroupInfo {
        /// Group identifier.
        group_id: String,
        /// Human-readable group name.
        name: String,
        /// User ID of the group creator.
        created_by: String,
        /// ISO-8601 creation timestamp.
        created_at: String,
        /// Group member list.
        members: Vec<GroupInfoMember>,
    },

    /// User's groups list was received (from relay).
    UserGroups {
        /// List of group summaries.
        groups: Vec<UserGroupSummary>,
    },

    /// A group operation failed (from relay).
    GroupError {
        /// Human-readable error reason.
        reason: String,
    },

    /// A group message was sent to all members via mesh (MLS-encrypted fan-out).
    GroupMessageSent {
        /// MLS group identifier.
        group_id: String,
        /// Per-member message IDs from fan-out.
        message_ids: Vec<String>,
        /// Number of members the message was sent to.
        member_count: u32,
    },

    /// A group message was only partially delivered (some members failed).
    GroupMessagePartialFailure {
        /// MLS group identifier.
        group_id: String,
        /// Members for whom send failed.
        failed_members: Vec<String>,
        /// Members for whom send succeeded.
        succeeded_members: Vec<String>,
    },

    /// An epoch fork was detected in a group — concurrent commits caused
    /// members to diverge onto different MLS branches. The deterministic
    /// leader will attempt automatic resolution via a key-update commit.
    GroupEpochForkDetected {
        /// MLS group identifier.
        group_id: String,
        /// Our local epoch when the fork was detected, or `None` if MLS
        /// state was unavailable at detection time.
        local_epoch: Option<u64>,
    },

    /// An epoch fork was successfully resolved by the leader issuing a
    /// resync commit that re-established a canonical epoch.
    ///
    /// Members in `failed_members` could not be reached with the resolution
    /// commit and may still be on a forked branch — the application layer
    /// should consider re-inviting them.
    GroupEpochForkResolved {
        /// MLS group identifier.
        group_id: String,
        /// The new canonical epoch after resolution.
        resolved_epoch: u64,
        /// Members to whom the resolution commit could not be sent.
        /// These members may still be on a forked epoch branch and
        /// may need a re-invite to rejoin the canonical group state.
        failed_members: Vec<String>,
    },

    // --- Service discovery ---
    /// A service was discovered on the mesh in response to a discovery query.
    ServiceDiscovered {
        /// Query ID that triggered this discovery.
        query_id: String,
        /// Service identifier.
        service_id: String,
        /// Service version.
        version: String,
        /// Peer user ID of the provider.
        provider_peer_id: String,
        /// Service capabilities.
        capabilities: HashMap<String, String>,
        /// Number of hops from the provider.
        hop_count: u8,
    },

    /// A service request was received from another peer.
    ServiceRequestReceived {
        /// Unique request identifier.
        request_id: String,
        /// Service identifier being invoked.
        service_id: String,
        /// Method name or action.
        method: String,
        /// Request body (JSON or arbitrary string).
        body: String,
        /// Peer user ID of the requester.
        sender: String,
    },

    /// A response to a service request was received.
    ServiceResponseReceived {
        /// Request identifier this response corresponds to.
        request_id: String,
        /// Service identifier.
        service_id: String,
        /// Status: "ok", "not_found", or "error".
        status: String,
        /// Response body.
        body: String,
        /// Peer user ID of the provider.
        provider_peer_id: String,
    },

    // --- Presence, typing, and read receipts ---
    /// A peer's presence status was updated.
    PresenceUpdated {
        /// Peer whose presence changed.
        peer_id: String,
        /// Presence status.
        status: PresenceStatus,
        /// Timestamp of the update (Unix ms).
        timestamp: i64,
    },

    /// A typing indicator was received from a peer.
    TypingIndicatorReceived {
        /// User who is typing (or stopped typing).
        sender: String,
        /// Conversation identifier (recipient username for DMs, group_id for groups).
        conversation_id: String,
        /// Whether the user is currently typing.
        is_typing: bool,
        /// Timestamp of the indicator (Unix ms).
        timestamp: i64,
    },

    /// A read receipt was received from a peer.
    ReadReceiptReceived {
        /// User who read the messages.
        sender: String,
        /// IDs of the messages that were read.
        message_ids: Vec<String>,
        /// Timestamp when the messages were read (Unix ms).
        timestamp: i64,
    },

    // --- DORS observability (OFF-258) ---
    /// DORS scored all available transports for this decision cycle.
    DorsScoreUpdated {
        /// Transport type and score pairs (descending by score).
        scores: Vec<(String, f32)>,
    },
    /// DORS selected a transport for the current message.
    DorsTransportSelected {
        /// Previous transport (if any); provides decision boundary context.
        from: Option<String>,
        /// Selected transport.
        transport: String,
        /// Typed reason for selection (initial vs primary selected).
        reason_code: DorsReasonCode,
        /// Score of the selected transport (supplemental).
        score: f32,
    },
    /// DORS switched from one transport to another. Emitted only when active transport
    /// actually changes after a successful send.
    DorsTransportSwitched {
        /// Previous transport (if any).
        from: Option<String>,
        /// New transport (now active).
        to: String,
        /// Stable enum-backed reason for the transition.
        reason_code: DorsReasonCode,
        /// Optional human-readable context (e.g. "primary failed, fallback succeeded").
        #[serde(skip_serializing_if = "Option::is_none")]
        reason_detail: Option<String>,
    },
    /// DORS escalation from BLE to Wi‑Fi. Use `phase` to distinguish recommendation vs applied.
    DorsEscalationTriggered {
        /// TRIGGERED = DORS decided to escalate (reason = trigger); APPLIED = fallback succeeded.
        phase: DorsEscalationPhase,
        /// Transport we escalated from (e.g. "ble").
        from: String,
        /// Transport we escalated to (e.g. "wifiDirect").
        to: String,
        /// Stable enum-backed reason code.
        reason_code: DorsEscalationReasonCode,
        /// Optional human-readable context.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason_detail: Option<String>,
    },

    /// A security-relevant anomaly was detected (e.g. sender spoofing, TOFU
    /// key mismatch, unsigned control message when signatures are required).
    SecurityWarning {
        /// Peer ID involved in the warning.
        peer_id: String,
        /// Human-readable description of the security issue.
        reason: String,
    },

    /// A message was relayed (forwarded for a third party).
    MessageRelayed {
        /// ID of the relayed message.
        message_id: String,
        /// Original sender.
        sender: String,
        /// Intended recipient.
        recipient: String,
        /// Current hop count after increment.
        hop_count: u8,
        /// Remaining TTL after decrement.
        remaining_ttl: u8,
    },

    /// A user was blocked. Emitted for local UI notification only.
    UserBlocked {
        /// User ID that was blocked.
        user_id: String,
    },

    /// A user was unblocked. Emitted for local UI notification only.
    UserUnblocked {
        /// User ID that was unblocked.
        user_id: String,
    },

    /// A peer's TOFU-pinned public key was manually reset.
    TofuReset {
        /// ID of the peer whose trust was reset.
        peer_id: String,
    },
}

/// Member entry in GroupInfo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfoMember {
    /// User ID of the member.
    pub user_id: String,
    /// Role within the group (e.g. "admin", "member").
    pub role: String,
    /// ISO-8601 timestamp when the member joined.
    pub joined_at: String,
}

/// Group summary in UserGroups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserGroupSummary {
    /// Group identifier.
    pub group_id: String,
    /// Human-readable group name.
    pub name: String,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
}

impl Event {
    /// Creates a MessageSent event.
    pub fn message_sent(message: &Message) -> Self {
        use offline_protocol_core::MessagePriority;

        let priority = match message.priority {
            MessagePriority::Low => "low",
            MessagePriority::Medium => "medium",
            MessagePriority::High => "high",
            MessagePriority::Critical => "critical",
        };

        Self::MessageSent {
            message_id: message.id.as_str(),
            sender: message.sender.as_str().to_string(),
            recipient: message.recipient.as_str().to_string(),
            content: message.content.clone(),
            priority: priority.to_string(),
            requires_ack: message.requires_ack,
            timestamp: message.timestamp.as_millis(),
            lamport_clock: message.lamport_clock.value(),
        }
    }

    /// Creates a MessageDelivered event.
    pub fn message_delivered(
        message_id: MessageId,
        latency_ms: u64,
        hop_count: u8,
        transport: TransportType,
    ) -> Self {
        Self::MessageDelivered {
            message_id: message_id.as_str(),
            latency_ms,
            hop_count,
            transport: transport.to_string(),
        }
    }

    /// Creates a MessageFailed event.
    pub fn message_failed(message_id: MessageId, reason: String, retry_count: u32) -> Self {
        Self::MessageFailed {
            message_id: message_id.as_str(),
            reason,
            retry_count,
        }
    }

    /// Creates a MessageDecryptionFailed event.
    pub fn message_decryption_failed(
        message_id: MessageId,
        sender: String,
        code: DecryptionFailureCode,
        reason: String,
    ) -> Self {
        Self::MessageDecryptionFailed {
            message_id: message_id.as_str(),
            sender,
            code,
            reason,
        }
    }

    /// Creates a TransportSwitched event.
    pub fn transport_switched(
        from: Option<TransportType>,
        to: TransportType,
        reason: String,
    ) -> Self {
        Self::TransportSwitched {
            from: from.map(|t| t.to_string()),
            to: to.to_string(),
            reason,
        }
    }

    /// Creates a RelayPromoted event.
    pub fn relay_promoted(connection_count: usize, battery_level: u8) -> Self {
        Self::RelayPromoted {
            connection_count,
            battery_level,
        }
    }

    /// Creates a RelayDemoted event.
    pub fn relay_demoted(reason: String) -> Self {
        Self::RelayDemoted { reason }
    }

    /// Creates a NeighborDiscovered event.
    pub fn neighbor_discovered(
        peer_id: String,
        transport: TransportType,
        rssi: Option<i16>,
    ) -> Self {
        Self::NeighborDiscovered {
            peer_id,
            transport: format!("{:?}", transport),
            rssi,
        }
    }

    /// Creates a NeighborLost event.
    pub fn neighbor_lost(peer_id: String) -> Self {
        Self::NeighborLost { peer_id }
    }

    /// Creates a NetworkMetrics event.
    pub fn network_metrics(
        neighbor_count: usize,
        relay_count: usize,
        delivery_ratio: f32,
        avg_latency_ms: u64,
    ) -> Self {
        Self::NetworkMetrics {
            neighbor_count,
            relay_count,
            delivery_ratio,
            avg_latency_ms,
        }
    }

    /// Creates a FileProgress event.
    pub fn file_progress(file_id: String, chunks_sent: u32, total_chunks: u32) -> Self {
        let percentage = if total_chunks > 0 {
            ((chunks_sent as f32 / total_chunks as f32) * 100.0) as u8
        } else {
            0
        };

        Self::FileProgress {
            file_id,
            chunks_sent,
            total_chunks,
            percentage,
        }
    }

    /// Creates a FileReceived event.
    pub fn file_received(
        file_id: String,
        file_name: String,
        file_size: u64,
        sender: String,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
        file_data: Vec<u8>,
    ) -> Self {
        use base64::{engine::general_purpose::STANDARD, Engine};
        Self::FileReceived {
            file_id,
            file_name,
            file_size,
            sender,
            content_type: content_type.to_string(),
            media_metadata,
            file_data: STANDARD.encode(&file_data),
        }
    }

    /// Creates a MediaSent event.
    pub fn media_sent(file_id: String, content_type: ContentType, recipient: String) -> Self {
        Self::MediaSent {
            file_id,
            content_type: content_type.to_string(),
            recipient,
        }
    }

    /// Creates a MessageDeferred event.
    pub fn message_deferred(
        message_id: MessageId,
        reason: String,
        retry_count: u32,
        next_retry_at: Option<i64>,
    ) -> Self {
        Self::MessageDeferred {
            message_id: message_id.as_str(),
            reason,
            retry_count,
            next_retry_at,
        }
    }

    /// Creates an AckEvicted event.
    pub fn ack_evicted(message_id: MessageId, priority: &str, reason: String) -> Self {
        Self::AckEvicted {
            message_id: message_id.as_str(),
            priority: priority.to_string(),
            reason,
        }
    }

    /// Creates a FragmentAssemblyEvicted event.
    pub fn fragment_assembly_evicted(
        message_id: String,
        completion_percent: u8,
        reason: String,
    ) -> Self {
        Self::FragmentAssemblyEvicted {
            message_id,
            completion_percent,
            reason,
        }
    }

    /// Creates a RelayDemotedBattery event.
    pub fn relay_demoted_battery(battery_level: u8, min_required: u8) -> Self {
        Self::RelayDemotedBattery {
            battery_level,
            min_required,
        }
    }

    /// Creates a SecureSessionEstablished event.
    pub fn secure_session_established(
        peer_id: String,
        group_id: String,
        is_session: bool,
        initiated_by_local: bool,
    ) -> Self {
        Self::SecureSessionEstablished {
            peer_id,
            group_id,
            is_session,
            initiated_by_local,
        }
    }

    /// Creates a SecureSessionFailed event.
    pub fn secure_session_failed(peer_id: String, reason: String) -> Self {
        Self::SecureSessionFailed { peer_id, reason }
    }

    /// Creates a WelcomeSendAttempted event.
    pub fn welcome_send_attempted(
        peer_id: String,
        message_id: String,
        group_id: String,
        attempt: u32,
    ) -> Self {
        Self::WelcomeSendAttempted {
            peer_id,
            message_id,
            group_id,
            attempt,
        }
    }

    /// Creates a WelcomeSendSucceeded event.
    pub fn welcome_send_succeeded(
        peer_id: String,
        message_id: String,
        group_id: String,
        attempt: u32,
    ) -> Self {
        Self::WelcomeSendSucceeded {
            peer_id,
            message_id,
            group_id,
            attempt,
        }
    }

    /// Creates a WelcomeSendFailed event.
    #[allow(clippy::too_many_arguments)]
    pub fn welcome_send_failed(
        peer_id: String,
        message_id: String,
        group_id: String,
        attempt: u32,
        reason_code: WelcomeReasonCode,
        transport_error: Option<String>,
        retryable: bool,
        next_retry_at: Option<i64>,
    ) -> Self {
        Self::WelcomeSendFailed {
            peer_id,
            message_id,
            group_id,
            attempt,
            reason_code,
            transport_error,
            retryable,
            next_retry_at,
        }
    }

    /// Creates a WelcomeSendExpired event.
    pub fn welcome_send_expired(
        peer_id: String,
        message_id: String,
        attempt: u32,
        reason_code: WelcomeReasonCode,
    ) -> Self {
        Self::WelcomeSendExpired {
            peer_id,
            message_id,
            attempt,
            reason_code,
        }
    }

    /// Creates a ConnectionRequestReceived event.
    pub fn connection_request_received(
        sender: String,
        sender_name: String,
        timestamp: i64,
        key_package: Option<Vec<u8>>,
    ) -> Self {
        Self::ConnectionRequestReceived {
            sender,
            sender_name,
            timestamp,
            key_package,
        }
    }

    /// Creates a ConnectionAccepted event.
    pub fn connection_accepted(
        accepted_by: String,
        accepted_by_name: String,
        timestamp: i64,
        key_package: Option<Vec<u8>>,
    ) -> Self {
        Self::ConnectionAccepted {
            accepted_by,
            accepted_by_name,
            timestamp,
            key_package,
        }
    }

    /// Creates a ConnectionRejected event.
    pub fn connection_rejected(rejected_by: String) -> Self {
        Self::ConnectionRejected { rejected_by }
    }

    /// Creates a PresenceUpdated event.
    pub fn presence_updated(peer_id: String, status: PresenceStatus, timestamp: i64) -> Self {
        Self::PresenceUpdated {
            peer_id,
            status,
            timestamp,
        }
    }

    /// Creates a TypingIndicatorReceived event.
    pub fn typing_indicator_received(
        sender: String,
        conversation_id: String,
        is_typing: bool,
        timestamp: i64,
    ) -> Self {
        Self::TypingIndicatorReceived {
            sender,
            conversation_id,
            is_typing,
            timestamp,
        }
    }

    /// Creates a ReadReceiptReceived event.
    pub fn read_receipt_received(sender: String, message_ids: Vec<String>, timestamp: i64) -> Self {
        Self::ReadReceiptReceived {
            sender,
            message_ids,
            timestamp,
        }
    }

    /// Creates a GroupCreated event.
    pub fn group_created(group_id: String, name: String) -> Self {
        Self::GroupCreated { group_id, name }
    }

    /// Creates a GroupMessageReceived event.
    pub fn group_message_received(
        group_id: String,
        sender: String,
        content: String,
        timestamp: String,
        message_id: String,
        reply_to_msg: Option<String>,
    ) -> Self {
        Self::GroupMessageReceived {
            group_id,
            sender,
            content,
            timestamp,
            message_id,
            reply_to_msg,
        }
    }

    /// Creates a GroupMemberAdded event.
    pub fn group_member_added(
        group_id: String,
        user_id: String,
        added_by: String,
        group_name: Option<String>,
    ) -> Self {
        Self::GroupMemberAdded {
            group_id,
            user_id,
            added_by,
            group_name,
        }
    }

    /// Creates a GroupMemberRemoved event.
    pub fn group_member_removed(group_id: String, user_id: String, removed_by: String) -> Self {
        Self::GroupMemberRemoved {
            group_id,
            user_id,
            removed_by,
        }
    }

    /// Creates a GroupInfo event.
    pub fn group_info(
        group_id: String,
        name: String,
        created_by: String,
        created_at: String,
        members: Vec<GroupInfoMember>,
    ) -> Self {
        Self::GroupInfo {
            group_id,
            name,
            created_by,
            created_at,
            members,
        }
    }

    /// Creates a UserGroups event.
    pub fn user_groups(groups: Vec<UserGroupSummary>) -> Self {
        Self::UserGroups { groups }
    }

    /// Creates a GroupError event.
    pub fn group_error(reason: String) -> Self {
        Self::GroupError { reason }
    }

    /// Creates a GroupMessageSent event.
    pub fn group_message_sent(
        group_id: String,
        message_ids: Vec<String>,
        member_count: u32,
    ) -> Self {
        Self::GroupMessageSent {
            group_id,
            message_ids,
            member_count,
        }
    }

    /// Creates a GroupMessagePartialFailure event.
    pub fn group_message_partial_failure(
        group_id: String,
        failed_members: Vec<String>,
        succeeded_members: Vec<String>,
    ) -> Self {
        Self::GroupMessagePartialFailure {
            group_id,
            failed_members,
            succeeded_members,
        }
    }

    /// Creates a GroupEpochForkDetected event.
    pub fn group_epoch_fork_detected(group_id: String, local_epoch: Option<u64>) -> Self {
        Self::GroupEpochForkDetected {
            group_id,
            local_epoch,
        }
    }

    /// Creates a GroupEpochForkResolved event.
    pub fn group_epoch_fork_resolved(
        group_id: String,
        resolved_epoch: u64,
        failed_members: Vec<String>,
    ) -> Self {
        Self::GroupEpochForkResolved {
            group_id,
            resolved_epoch,
            failed_members,
        }
    }

    /// Creates a ServiceDiscovered event.
    pub fn service_discovered(
        query_id: String,
        service_id: String,
        version: String,
        provider_peer_id: String,
        capabilities: HashMap<String, String>,
        hop_count: u8,
    ) -> Self {
        Self::ServiceDiscovered {
            query_id,
            service_id,
            version,
            provider_peer_id,
            capabilities,
            hop_count,
        }
    }

    /// Creates a ServiceRequestReceived event.
    pub fn service_request_received(
        request_id: String,
        service_id: String,
        method: String,
        body: String,
        sender: String,
    ) -> Self {
        Self::ServiceRequestReceived {
            request_id,
            service_id,
            method,
            body,
            sender,
        }
    }

    /// Creates a ServiceResponseReceived event.
    pub fn service_response_received(
        request_id: String,
        service_id: String,
        status: String,
        body: String,
        provider_peer_id: String,
    ) -> Self {
        Self::ServiceResponseReceived {
            request_id,
            service_id,
            status,
            body,
            provider_peer_id,
        }
    }

    /// Creates a DorsScoreUpdated event (DORS observability).
    pub fn dors_score_updated(scores: Vec<(String, f32)>) -> Self {
        Self::DorsScoreUpdated { scores }
    }

    /// Creates a DorsTransportSelected event (DORS observability).
    pub fn dors_transport_selected(
        from: Option<String>,
        transport: String,
        reason_code: DorsReasonCode,
        score: f32,
    ) -> Self {
        Self::DorsTransportSelected {
            from,
            transport,
            reason_code,
            score,
        }
    }

    /// Creates a DorsTransportSwitched event (DORS observability).
    /// Emitted only when active transport actually changes after a successful send.
    pub fn dors_transport_switched(
        from: Option<String>,
        to: String,
        reason_code: DorsReasonCode,
        reason_detail: Option<String>,
    ) -> Self {
        Self::DorsTransportSwitched {
            from,
            to,
            reason_code,
            reason_detail,
        }
    }

    /// Creates a DorsEscalationTriggered event (DORS observability).
    /// Use phase Triggered at the DORS trigger boundary, Applied when fallback succeeds.
    pub fn dors_escalation_triggered(
        phase: DorsEscalationPhase,
        from: String,
        to: String,
        reason_code: DorsEscalationReasonCode,
        reason_detail: Option<String>,
    ) -> Self {
        Self::DorsEscalationTriggered {
            phase,
            from,
            to,
            reason_code,
            reason_detail,
        }
    }

    /// Creates a SecurityWarning event.
    pub fn security_warning(peer_id: String, reason: String) -> Self {
        Self::SecurityWarning { peer_id, reason }
    }

    /// Creates a MessageRelayed event.
    pub fn message_relayed(
        message_id: String,
        sender: String,
        recipient: String,
        hop_count: u8,
        remaining_ttl: u8,
    ) -> Self {
        Self::MessageRelayed {
            message_id,
            sender,
            recipient,
            hop_count,
            remaining_ttl,
        }
    }

    /// Creates a UserBlocked event.
    pub fn user_blocked(user_id: String) -> Self {
        Self::UserBlocked { user_id }
    }

    /// Creates a UserUnblocked event.
    pub fn user_unblocked(user_id: String) -> Self {
        Self::UserUnblocked { user_id }
    }

    /// Creates a TofuReset event.
    pub fn tofu_reset(peer_id: String) -> Self {
        Self::TofuReset { peer_id }
    }

    /// Converts the event to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Parses an event from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

impl From<ServiceEvent> for Event {
    fn from(event: ServiceEvent) -> Self {
        match event {
            ServiceEvent::ServiceDiscovered {
                query_id,
                service_id,
                version,
                provider_peer_id,
                capabilities,
                hop_count,
            } => Self::ServiceDiscovered {
                query_id,
                service_id,
                version,
                provider_peer_id,
                capabilities,
                hop_count,
            },
            ServiceEvent::ServiceRequestReceived {
                request_id,
                service_id,
                method,
                body,
                sender,
            } => Self::ServiceRequestReceived {
                request_id,
                service_id,
                method,
                body,
                sender,
            },
            ServiceEvent::ServiceResponseReceived {
                request_id,
                service_id,
                status,
                body,
                provider_peer_id,
            } => Self::ServiceResponseReceived {
                request_id,
                service_id,
                status,
                body,
                provider_peer_id,
            },
        }
    }
}

/// Custom Debug implementation that redacts sensitive fields to prevent
/// accidental logging of message content and other potentially sensitive data.
impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MessageSent {
                message_id,
                sender: _,
                recipient: _,
                content,
                priority,
                requires_ack,
                timestamp,
                lamport_clock,
            } => f
                .debug_struct("MessageSent")
                .field("message_id", message_id)
                .field("sender", &"[REDACTED]")
                .field("recipient", &"[REDACTED]")
                .field("content", &format!("[REDACTED {} bytes]", content.len()))
                .field("priority", priority)
                .field("requires_ack", requires_ack)
                .field("timestamp", timestamp)
                .field("lamport_clock", lamport_clock)
                .finish(),
            Self::MessageReceived {
                message_id,
                sender: _,
                recipient: _,
                content,
                hop_count,
                transport,
                timestamp,
                lamport_clock,
                reply_to_msg: _,
                content_type,
                media_metadata: _,
            } => f
                .debug_struct("MessageReceived")
                .field("message_id", message_id)
                .field("sender", &"[REDACTED]")
                .field("recipient", &"[REDACTED]")
                .field("content", &format!("[REDACTED {} bytes]", content.len()))
                .field("hop_count", hop_count)
                .field("transport", transport)
                .field("timestamp", timestamp)
                .field("lamport_clock", lamport_clock)
                .field("reply_to_msg", &"[REDACTED]")
                .field("content_type", content_type)
                .finish(),
            Self::MessageDelivered {
                message_id,
                latency_ms,
                hop_count,
                transport,
            } => f
                .debug_struct("MessageDelivered")
                .field("message_id", message_id)
                .field("latency_ms", latency_ms)
                .field("hop_count", hop_count)
                .field("transport", transport)
                .finish(),
            Self::MessageFailed {
                message_id,
                reason,
                retry_count,
            } => f
                .debug_struct("MessageFailed")
                .field("message_id", message_id)
                .field("reason", reason)
                .field("retry_count", retry_count)
                .finish(),
            Self::MessageDecryptionFailed {
                message_id,
                sender: _,
                code,
                reason,
            } => f
                .debug_struct("MessageDecryptionFailed")
                .field("message_id", message_id)
                .field("sender", &"[REDACTED]")
                .field("code", code)
                .field("reason", reason)
                .finish(),
            Self::TransportSwitched { from, to, reason } => f
                .debug_struct("TransportSwitched")
                .field("from", from)
                .field("to", to)
                .field("reason", reason)
                .finish(),
            Self::RelayPromoted {
                connection_count,
                battery_level,
            } => f
                .debug_struct("RelayPromoted")
                .field("connection_count", connection_count)
                .field("battery_level", battery_level)
                .finish(),
            Self::RelayDemoted { reason } => f
                .debug_struct("RelayDemoted")
                .field("reason", reason)
                .finish(),
            Self::NeighborDiscovered {
                peer_id: _,
                transport,
                rssi,
            } => f
                .debug_struct("NeighborDiscovered")
                .field("peer_id", &"[REDACTED]")
                .field("transport", transport)
                .field("rssi", rssi)
                .finish(),
            Self::NeighborLost { peer_id: _ } => f
                .debug_struct("NeighborLost")
                .field("peer_id", &"[REDACTED]")
                .finish(),
            Self::NetworkMetrics {
                neighbor_count,
                relay_count,
                delivery_ratio,
                avg_latency_ms,
            } => f
                .debug_struct("NetworkMetrics")
                .field("neighbor_count", neighbor_count)
                .field("relay_count", relay_count)
                .field("delivery_ratio", delivery_ratio)
                .field("avg_latency_ms", avg_latency_ms)
                .finish(),
            Self::FileProgress {
                file_id,
                chunks_sent,
                total_chunks,
                percentage,
            } => f
                .debug_struct("FileProgress")
                .field("file_id", file_id)
                .field("chunks_sent", chunks_sent)
                .field("total_chunks", total_chunks)
                .field("percentage", percentage)
                .finish(),
            Self::FileReceived {
                file_id,
                file_name,
                file_size,
                sender: _,
                content_type,
                media_metadata: _,
                file_data,
            } => f
                .debug_struct("FileReceived")
                .field("file_id", file_id)
                .field("file_name", file_name)
                .field("file_size", file_size)
                .field("sender", &"[REDACTED]")
                .field("content_type", content_type)
                .field("file_data", &format!("[{} bytes base64]", file_data.len()))
                .finish(),
            Self::MediaSent {
                file_id,
                content_type,
                recipient: _,
            } => f
                .debug_struct("MediaSent")
                .field("file_id", file_id)
                .field("content_type", content_type)
                .field("recipient", &"[REDACTED]")
                .finish(),
            Self::MessageDeferred {
                message_id,
                reason,
                retry_count,
                next_retry_at,
            } => f
                .debug_struct("MessageDeferred")
                .field("message_id", message_id)
                .field("reason", reason)
                .field("retry_count", retry_count)
                .field("next_retry_at", next_retry_at)
                .finish(),
            Self::AckEvicted {
                message_id,
                priority,
                reason,
            } => f
                .debug_struct("AckEvicted")
                .field("message_id", message_id)
                .field("priority", priority)
                .field("reason", reason)
                .finish(),
            Self::FragmentAssemblyEvicted {
                message_id,
                completion_percent,
                reason,
            } => f
                .debug_struct("FragmentAssemblyEvicted")
                .field("message_id", message_id)
                .field("completion_percent", completion_percent)
                .field("reason", reason)
                .finish(),
            Self::RelayDemotedBattery {
                battery_level,
                min_required,
            } => f
                .debug_struct("RelayDemotedBattery")
                .field("battery_level", battery_level)
                .field("min_required", min_required)
                .finish(),
            Self::SecureSessionEstablished {
                peer_id: _,
                group_id,
                is_session,
                initiated_by_local,
            } => f
                .debug_struct("SecureSessionEstablished")
                .field("peer_id", &"[REDACTED]")
                .field("group_id", group_id)
                .field("is_session", is_session)
                .field("initiated_by_local", initiated_by_local)
                .finish(),
            Self::SecureSessionFailed { peer_id: _, reason } => f
                .debug_struct("SecureSessionFailed")
                .field("peer_id", &"[REDACTED]")
                .field("reason", reason)
                .finish(),
            Self::WelcomeSendAttempted {
                peer_id: _,
                message_id,
                group_id,
                attempt,
            } => f
                .debug_struct("WelcomeSendAttempted")
                .field("peer_id", &"[REDACTED]")
                .field("message_id", message_id)
                .field("group_id", group_id)
                .field("attempt", attempt)
                .finish(),
            Self::WelcomeSendSucceeded {
                peer_id: _,
                message_id,
                group_id,
                attempt,
            } => f
                .debug_struct("WelcomeSendSucceeded")
                .field("peer_id", &"[REDACTED]")
                .field("message_id", message_id)
                .field("group_id", group_id)
                .field("attempt", attempt)
                .finish(),
            Self::WelcomeSendFailed {
                peer_id: _,
                message_id,
                group_id,
                attempt,
                reason_code,
                transport_error,
                retryable,
                next_retry_at,
            } => f
                .debug_struct("WelcomeSendFailed")
                .field("peer_id", &"[REDACTED]")
                .field("message_id", message_id)
                .field("group_id", group_id)
                .field("attempt", attempt)
                .field("reason_code", reason_code)
                .field("transport_error", transport_error)
                .field("retryable", retryable)
                .field("next_retry_at", next_retry_at)
                .finish(),
            Self::WelcomeSendExpired {
                peer_id: _,
                message_id,
                attempt,
                reason_code,
            } => f
                .debug_struct("WelcomeSendExpired")
                .field("peer_id", &"[REDACTED]")
                .field("message_id", message_id)
                .field("attempt", attempt)
                .field("reason_code", reason_code)
                .finish(),
            Self::ConnectionRequestReceived {
                sender: _,
                sender_name: _,
                timestamp,
                key_package,
            } => f
                .debug_struct("ConnectionRequestReceived")
                .field("sender", &"[REDACTED]")
                .field("sender_name", &"[REDACTED]")
                .field("timestamp", timestamp)
                .field("has_key_package", &key_package.is_some())
                .finish(),
            Self::ConnectionAccepted {
                accepted_by: _,
                accepted_by_name: _,
                timestamp,
                key_package,
            } => f
                .debug_struct("ConnectionAccepted")
                .field("accepted_by", &"[REDACTED]")
                .field("accepted_by_name", &"[REDACTED]")
                .field("timestamp", timestamp)
                .field("has_key_package", &key_package.is_some())
                .finish(),
            Self::ConnectionRejected { rejected_by: _ } => f
                .debug_struct("ConnectionRejected")
                .field("rejected_by", &"[REDACTED]")
                .finish(),
            Self::ServiceDiscovered {
                query_id,
                service_id,
                version,
                provider_peer_id: _,
                capabilities,
                hop_count,
            } => f
                .debug_struct("ServiceDiscovered")
                .field("query_id", query_id)
                .field("service_id", service_id)
                .field("version", version)
                .field("provider_peer_id", &"[REDACTED]")
                .field("capabilities_count", &capabilities.len())
                .field("hop_count", hop_count)
                .finish(),
            Self::ServiceRequestReceived {
                request_id,
                service_id,
                method,
                body,
                sender: _,
            } => f
                .debug_struct("ServiceRequestReceived")
                .field("request_id", request_id)
                .field("service_id", service_id)
                .field("method", method)
                .field("body", &format!("[REDACTED {} bytes]", body.len()))
                .field("sender", &"[REDACTED]")
                .finish(),
            Self::ServiceResponseReceived {
                request_id,
                service_id,
                status,
                body,
                provider_peer_id: _,
            } => f
                .debug_struct("ServiceResponseReceived")
                .field("request_id", request_id)
                .field("service_id", service_id)
                .field("status", status)
                .field("body", &format!("[REDACTED {} bytes]", body.len()))
                .field("provider_peer_id", &"[REDACTED]")
                .finish(),
            Self::GroupCreated { group_id, name } => f
                .debug_struct("GroupCreated")
                .field("group_id", group_id)
                .field("name", name)
                .finish(),
            Self::GroupMessageReceived {
                group_id,
                sender: _,
                content,
                ..
            } => f
                .debug_struct("GroupMessageReceived")
                .field("group_id", group_id)
                .field("sender", &"[REDACTED]")
                .field("content", &format!("[REDACTED {} bytes]", content.len()))
                .finish(),
            Self::GroupMemberAdded {
                group_id,
                user_id: _,
                added_by: _,
                group_name,
            } => f
                .debug_struct("GroupMemberAdded")
                .field("group_id", group_id)
                .field("user_id", &"[REDACTED]")
                .field("added_by", &"[REDACTED]")
                .field("group_name", group_name)
                .finish(),
            Self::GroupMemberRemoved {
                group_id,
                user_id: _,
                removed_by: _,
            } => f
                .debug_struct("GroupMemberRemoved")
                .field("group_id", group_id)
                .field("user_id", &"[REDACTED]")
                .field("removed_by", &"[REDACTED]")
                .finish(),
            Self::GroupInfo {
                group_id,
                name,
                created_by: _,
                created_at,
                members,
            } => f
                .debug_struct("GroupInfo")
                .field("group_id", group_id)
                .field("name", name)
                .field("created_by", &"[REDACTED]")
                .field("created_at", created_at)
                .field("members_count", &members.len())
                .finish(),
            Self::UserGroups { groups } => f
                .debug_struct("UserGroups")
                .field("groups_count", &groups.len())
                .finish(),
            Self::GroupError { reason } => f
                .debug_struct("GroupError")
                .field("reason", reason)
                .finish(),
            Self::GroupMessageSent {
                group_id,
                message_ids,
                member_count,
            } => f
                .debug_struct("GroupMessageSent")
                .field("group_id", group_id)
                .field("message_count", &message_ids.len())
                .field("member_count", member_count)
                .finish(),
            Self::GroupMessagePartialFailure {
                group_id,
                failed_members,
                succeeded_members,
            } => f
                .debug_struct("GroupMessagePartialFailure")
                .field("group_id", group_id)
                .field("failed_count", &failed_members.len())
                .field("succeeded_count", &succeeded_members.len())
                .finish(),
            Self::GroupEpochForkDetected {
                group_id,
                local_epoch,
            } => f
                .debug_struct("GroupEpochForkDetected")
                .field("group_id", group_id)
                .field("local_epoch", local_epoch)
                .finish(),
            Self::GroupEpochForkResolved {
                group_id,
                resolved_epoch,
                failed_members,
            } => f
                .debug_struct("GroupEpochForkResolved")
                .field("group_id", group_id)
                .field("resolved_epoch", resolved_epoch)
                .field("failed_members", failed_members)
                .finish(),
            Self::PresenceUpdated {
                peer_id: _,
                status,
                timestamp,
            } => f
                .debug_struct("PresenceUpdated")
                .field("peer_id", &"[REDACTED]")
                .field("status", status)
                .field("timestamp", timestamp)
                .finish(),
            Self::TypingIndicatorReceived {
                sender: _,
                conversation_id: _,
                is_typing,
                timestamp,
            } => f
                .debug_struct("TypingIndicatorReceived")
                .field("sender", &"[REDACTED]")
                .field("conversation_id", &"[REDACTED]")
                .field("is_typing", is_typing)
                .field("timestamp", timestamp)
                .finish(),
            Self::ReadReceiptReceived {
                sender: _,
                message_ids,
                timestamp,
            } => f
                .debug_struct("ReadReceiptReceived")
                .field("sender", &"[REDACTED]")
                .field("message_count", &message_ids.len())
                .field("timestamp", timestamp)
                .finish(),
            Self::DorsScoreUpdated { scores } => f
                .debug_struct("DorsScoreUpdated")
                .field("scores", scores)
                .finish(),
            Self::DorsTransportSelected {
                from,
                transport,
                reason_code,
                score,
            } => f
                .debug_struct("DorsTransportSelected")
                .field("from", from)
                .field("transport", transport)
                .field("reason_code", reason_code)
                .field("score", score)
                .finish(),
            Self::DorsTransportSwitched {
                from,
                to,
                reason_code,
                reason_detail,
            } => f
                .debug_struct("DorsTransportSwitched")
                .field("from", from)
                .field("to", to)
                .field("reason_code", reason_code)
                .field("reason_detail", reason_detail)
                .finish(),
            Self::DorsEscalationTriggered {
                phase,
                from,
                to,
                reason_code,
                reason_detail,
            } => f
                .debug_struct("DorsEscalationTriggered")
                .field("phase", phase)
                .field("from", from)
                .field("to", to)
                .field("reason_code", reason_code)
                .field("reason_detail", reason_detail)
                .finish(),
            Self::SecurityWarning { peer_id, reason } => f
                .debug_struct("SecurityWarning")
                .field("peer_id", peer_id)
                .field("reason", reason)
                .finish(),
            Self::MessageRelayed {
                message_id,
                sender: _,
                recipient: _,
                hop_count,
                remaining_ttl,
            } => f
                .debug_struct("MessageRelayed")
                .field("message_id", message_id)
                .field("sender", &"[REDACTED]")
                .field("recipient", &"[REDACTED]")
                .field("hop_count", hop_count)
                .field("remaining_ttl", remaining_ttl)
                .finish(),
            Self::UserBlocked { user_id: _ } => f
                .debug_struct("UserBlocked")
                .field("user_id", &"[REDACTED]")
                .finish(),
            Self::UserUnblocked { user_id: _ } => f
                .debug_struct("UserUnblocked")
                .field("user_id", &"[REDACTED]")
                .finish(),
            Self::TofuReset { peer_id: _ } => f
                .debug_struct("TofuReset")
                .field("peer_id", &"[REDACTED]")
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, Message, MessagePriority, UserId};

    #[test]
    fn test_message_sent_event() {
        let message = Message::builder(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("app").unwrap(),
        )
        .content("Hello")
        .priority(MessagePriority::High)
        .build();

        let event = Event::message_sent(&message);

        match event {
            Event::MessageSent {
                message_id,
                sender,
                recipient,
                content,
                priority,
                requires_ack,
                timestamp,
                lamport_clock,
            } => {
                assert_eq!(message_id, message.id.as_str());
                assert_eq!(sender, message.sender.as_str());
                assert_eq!(recipient, message.recipient.as_str());
                assert_eq!(content, message.content);
                assert_eq!(priority, "high");
                assert!(requires_ack);
                assert_eq!(timestamp, message.timestamp.as_millis());
                assert_eq!(lamport_clock, message.lamport_clock.value());
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_message_delivered_event() {
        let msg_id = MessageId::new();
        let event = Event::message_delivered(msg_id.clone(), 100, 3, TransportType::BLE);

        match event {
            Event::MessageDelivered {
                message_id,
                latency_ms,
                hop_count,
                transport,
            } => {
                assert_eq!(message_id, msg_id.as_str());
                assert_eq!(latency_ms, 100);
                assert_eq!(hop_count, 3);
                assert_eq!(transport, "ble");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_transport_switched_event() {
        let event = Event::transport_switched(
            Some(TransportType::BLE),
            TransportType::WiFiDirect,
            "Poor signal".to_string(),
        );

        match event {
            Event::TransportSwitched { from, to, reason } => {
                assert_eq!(from.as_deref(), Some("ble"));
                assert_eq!(to, "wifiDirect");
                assert_eq!(reason, "Poor signal");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_relay_promoted_event() {
        let event = Event::relay_promoted(5, 80);

        match event {
            Event::RelayPromoted {
                connection_count,
                battery_level,
            } => {
                assert_eq!(connection_count, 5);
                assert_eq!(battery_level, 80);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_file_progress_event() {
        let event = Event::file_progress("file123".to_string(), 50, 100);

        match event {
            Event::FileProgress {
                file_id,
                chunks_sent,
                total_chunks,
                percentage,
            } => {
                assert_eq!(file_id, "file123");
                assert_eq!(chunks_sent, 50);
                assert_eq!(total_chunks, 100);
                assert_eq!(percentage, 50);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_event_serialization() {
        let event = Event::relay_promoted(5, 80);
        let json = event.to_json().unwrap();
        let deserialized = Event::from_json(&json).unwrap();

        match deserialized {
            Event::RelayPromoted {
                connection_count,
                battery_level,
            } => {
                assert_eq!(connection_count, 5);
                assert_eq!(battery_level, 80);
            }
            _ => panic!("Wrong event type after deserialization"),
        }
    }

    #[test]
    fn test_network_metrics_event() {
        let event = Event::network_metrics(10, 3, 0.95, 150);

        match event {
            Event::NetworkMetrics {
                neighbor_count,
                relay_count,
                delivery_ratio,
                avg_latency_ms,
            } => {
                assert_eq!(neighbor_count, 10);
                assert_eq!(relay_count, 3);
                assert_eq!(delivery_ratio, 0.95);
                assert_eq!(avg_latency_ms, 150);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_file_received_event() {
        let event = Event::file_received(
            "file123".to_string(),
            "photo.jpg".to_string(),
            1024000,
            "alice".to_string(),
            ContentType::Image,
            None,
            vec![1, 2, 3, 4],
        );

        match event {
            Event::FileReceived {
                file_id,
                file_name,
                file_size,
                sender,
                content_type,
                file_data,
                ..
            } => {
                assert_eq!(file_id, "file123");
                assert_eq!(file_name, "photo.jpg");
                assert_eq!(file_size, 1024000);
                assert_eq!(sender, "alice");
                assert_eq!(content_type, "image");
                assert!(!file_data.is_empty());
            }
            _ => panic!("Wrong event type"),
        }
    }
}
