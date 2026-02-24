//! Event types and callbacks.

use offline_protocol_core::{Message, MessageId};
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

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
    GroupCreated { group_id: String, name: String },

    /// A message was received in a group (from relay).
    GroupMessageReceived {
        group_id: String,
        sender: String,
        content: String,
        timestamp: String,
        message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reply_to_msg: Option<String>,
    },

    /// A member was added to a group (from relay).
    GroupMemberAdded {
        group_id: String,
        user_id: String,
        added_by: String,
    },

    /// A member was removed from a group (from relay).
    GroupMemberRemoved {
        group_id: String,
        user_id: String,
        removed_by: String,
    },

    /// Group info was received (from relay).
    GroupInfo {
        group_id: String,
        name: String,
        created_by: String,
        created_at: String,
        members: Vec<GroupInfoMember>,
    },

    /// User's groups list was received (from relay).
    UserGroups { groups: Vec<UserGroupSummary> },

    /// A group operation failed (from relay).
    GroupError { reason: String },
}

/// Member entry in GroupInfo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfoMember {
    pub user_id: String,
    pub role: String,
    pub joined_at: String,
}

/// Group summary in UserGroups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserGroupSummary {
    pub group_id: String,
    pub name: String,
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
    ) -> Self {
        Self::FileReceived {
            file_id,
            file_name,
            file_size,
            sender,
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
    pub fn group_member_added(group_id: String, user_id: String, added_by: String) -> Self {
        Self::GroupMemberAdded {
            group_id,
            user_id,
            added_by,
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

    /// Converts the event to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Parses an event from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
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
            } => f
                .debug_struct("FileReceived")
                .field("file_id", file_id)
                .field("file_name", file_name)
                .field("file_size", file_size)
                .field("sender", &"[REDACTED]")
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
            } => f
                .debug_struct("GroupMemberAdded")
                .field("group_id", group_id)
                .field("user_id", &"[REDACTED]")
                .field("added_by", &"[REDACTED]")
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
        );

        match event {
            Event::FileReceived {
                file_id,
                file_name,
                file_size,
                sender,
            } => {
                assert_eq!(file_id, "file123");
                assert_eq!(file_name, "photo.jpg");
                assert_eq!(file_size, 1024000);
                assert_eq!(sender, "alice");
            }
            _ => panic!("Wrong event type"),
        }
    }
}
