//! Message types and structures.

use crate::types::{AppId, HopCount, LamportClock, Timestamp, UserId, TTL};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use uuid::Uuid;

/// Unique message identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(Uuid);

impl MessageId {
    /// Generates a new random message ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Creates a message ID from a string representation.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(id: &str) -> crate::Result<Self> {
        let uuid = Uuid::parse_str(id)
            .map_err(|e| crate::Error::InvalidMessage(format!("Invalid message id: {}", e)))?;
        Ok(Self::from_uuid(uuid))
    }

    /// Creates a message ID from a UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Returns the message ID as a string.
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The type of content carried in a message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    /// Plain text message.
    #[default]
    Text,
    /// Image attachment (JPEG, PNG, etc.).
    Image,
    /// Video attachment.
    Video,
    /// Audio attachment.
    Audio,
    /// Short voice recording.
    VoiceNote,
    /// Short video recording.
    VideoNote,
    /// Generic file attachment.
    File,
    /// Internal: a chunk belonging to a multi-part file transfer.
    FileChunk,
}

impl ContentType {
    /// Returns `true` for types that carry binary media data.
    pub fn is_media(&self) -> bool {
        !matches!(self, Self::Text)
    }

    /// Parses a content type from its string representation.
    ///
    /// Falls back to `File` for unrecognised strings.
    pub fn parse(s: &str) -> Self {
        match s {
            "text" => Self::Text,
            "image" => Self::Image,
            "video" => Self::Video,
            "audio" => Self::Audio,
            "voice_note" => Self::VoiceNote,
            "video_note" => Self::VideoNote,
            "file" => Self::File,
            "file_chunk" => Self::FileChunk,
            _ => Self::File,
        }
    }
}

impl fmt::Display for ContentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => write!(f, "text"),
            Self::Image => write!(f, "image"),
            Self::Video => write!(f, "video"),
            Self::Audio => write!(f, "audio"),
            Self::VoiceNote => write!(f, "voice_note"),
            Self::VideoNote => write!(f, "video_note"),
            Self::File => write!(f, "file"),
            Self::FileChunk => write!(f, "file_chunk"),
        }
    }
}

/// Metadata describing a media attachment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaMetadata {
    /// MIME type (e.g. "image/jpeg", "video/mp4").
    pub mime_type: String,

    /// Original file name.
    pub file_name: String,

    /// File size in bytes.
    pub file_size: u64,

    /// Duration in milliseconds (audio/video/voice-note/video-note).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Width in pixels (images and video).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,

    /// Height in pixels (images and video).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,

    /// Small base64-encoded thumbnail (< 2 KB) for preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_base64: Option<String>,
}

/// Information about a forwarded message.
///
/// **Trust model:** `ForwardInfo` is an unverified attribution hint, not a
/// cryptographic proof. A malicious client can forge the `original_sender` or
/// reset `forward_count`. UI layers should treat this as a display-level hint
/// and must not rely on it for access-control or security decisions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForwardInfo {
    /// The original sender of the message.
    pub original_sender: UserId,
    /// The original message ID.
    pub original_message_id: MessageId,
    /// The original timestamp (wall-clock, for display).
    pub original_timestamp: Timestamp,
    /// Number of times this message has been forwarded.
    pub forward_count: u32,
}

impl ForwardInfo {
    /// Creates `ForwardInfo` from a message being forwarded.
    ///
    /// If the message was already forwarded, the original attribution is preserved
    /// and `forward_count` is incremented. Otherwise, the message's sender becomes
    /// the original sender with `forward_count = 1`.
    pub fn from_message(message: &Message) -> Self {
        match &message.forwarded_from {
            Some(existing) => ForwardInfo {
                original_sender: existing.original_sender.clone(),
                original_message_id: existing.original_message_id.clone(),
                original_timestamp: existing.original_timestamp,
                forward_count: existing.forward_count + 1,
            },
            None => ForwardInfo {
                original_sender: message.sender.clone(),
                original_message_id: message.id.clone(),
                original_timestamp: message.timestamp,
                forward_count: 1,
            },
        }
    }
}

/// Message priority levels.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum MessagePriority {
    /// Low priority - can be delayed or dropped under congestion.
    Low,
    /// Medium priority - default for most messages.
    #[default]
    Medium,
    /// High priority - important messages that should be delivered quickly.
    High,
    /// Critical priority - emergency messages, highest delivery guarantee.
    Critical,
}

impl MessagePriority {
    /// Returns a numeric score for the priority (higher = more important).
    pub fn score(&self) -> u8 {
        match self {
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
            Self::Critical => 4,
        }
    }
}

/// A message in the Offline Protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique message identifier.
    pub id: MessageId,

    /// Sender's user ID.
    pub sender: UserId,

    /// Recipient's user ID.
    pub recipient: UserId,

    /// Application ID that this message belongs to.
    pub app_id: AppId,

    /// Message priority.
    pub priority: MessagePriority,

    /// Time-to-live: maximum hops remaining.
    pub ttl: TTL,

    /// Number of hops this message has traversed.
    pub hop_count: HopCount,

    /// Timestamp when the message was created (wall-clock, for display only).
    pub timestamp: Timestamp,

    /// Lamport logical clock for causal ordering across devices.
    #[serde(default)]
    pub lamport_clock: LamportClock,

    /// The type of content this message carries.
    #[serde(default)]
    pub content_type: ContentType,

    /// Message content (text, JSON-serialized file chunk metadata, etc.).
    pub content: String,

    /// Raw payload for file chunk data carried by the message envelope.
    /// This avoids wrapping chunk bytes in `FileChunk` JSON/base64 content.
    /// Note: current transport serialization still uses JSON for `Message`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_content: Option<Vec<u8>>,

    /// Media metadata (present for non-text content types).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_metadata: Option<MediaMetadata>,

    /// Optional metadata for application-specific use.
    #[serde(default)]
    pub metadata: HashMap<String, String>,

    /// Whether this message requires an ACK.
    #[serde(default = "default_requires_ack")]
    pub requires_ack: bool,

    /// ID of the message this is replying to (optional).
    #[serde(default)]
    pub reply_to_msg: Option<MessageId>,

    /// Forwarding attribution (present when this message was forwarded from another user).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_from: Option<ForwardInfo>,

    /// Transport-verified peer identity.
    ///
    /// Set by the transport layer when a message is received, binding the message
    /// to the physical peer that delivered it. This field is **never serialized**
    /// over the wire — it exists only in-process to prevent sender spoofing.
    ///
    /// When `Some`, the protocol layer validates that `sender` matches this value
    /// before processing security-sensitive control messages.
    ///
    /// # Security
    ///
    /// This field is private to prevent application code from bypassing sender
    /// verification. Use [`set_transport_peer_id`](Self::set_transport_peer_id)
    /// (transport layers only) and [`transport_peer_id`](Self::transport_peer_id)
    /// to interact with it.
    #[serde(skip)]
    transport_peer_id: Option<String>,
}

fn default_requires_ack() -> bool {
    true
}

impl Message {
    /// Creates a new message.
    ///
    /// # Arguments
    ///
    /// * `sender` - Sender's user ID
    /// * `recipient` - Recipient's user ID
    /// * `app_id` - Application ID
    /// * `content` - Message content
    pub fn new(
        sender: UserId,
        recipient: UserId,
        app_id: AppId,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: MessageId::new(),
            sender,
            recipient,
            app_id,
            priority: MessagePriority::default(),
            ttl: TTL::default(),
            hop_count: HopCount::new(),
            timestamp: Timestamp::now(),
            lamport_clock: LamportClock::default(),
            content_type: ContentType::default(),
            content: content.into(),
            binary_content: None,
            media_metadata: None,
            metadata: HashMap::new(),
            requires_ack: true,
            reply_to_msg: None,
            forwarded_from: None,
            transport_peer_id: None,
        }
    }

    /// Creates a message builder for more control over message creation.
    pub fn builder(sender: UserId, recipient: UserId, app_id: AppId) -> MessageBuilder {
        MessageBuilder::new(sender, recipient, app_id)
    }

    /// Increments the hop count.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if successful, `Err` if hop count overflows.
    pub fn increment_hop(&mut self) -> crate::Result<()> {
        self.hop_count = self
            .hop_count
            .increment()
            .ok_or(crate::Error::InvalidHopCount(u8::MAX))?;
        Ok(())
    }

    /// Decrements the TTL.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if successful, `Err` if TTL is exhausted.
    pub fn decrement_ttl(&mut self) -> crate::Result<()> {
        self.ttl = self.ttl.decrement().ok_or(crate::Error::InvalidTTL(0))?;
        Ok(())
    }

    /// Checks if the message's TTL is exhausted.
    pub fn is_ttl_exhausted(&self) -> bool {
        self.ttl.is_exhausted()
    }

    /// Serializes the message to JSON.
    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string(self).map_err(|e| crate::Error::SerializationError(e.to_string()))
    }

    /// Deserializes a message from JSON.
    pub fn from_json(json: &str) -> crate::Result<Self> {
        serde_json::from_str(json).map_err(|e| crate::Error::DeserializationError(e.to_string()))
    }

    /// Sets the transport-verified peer identity.
    ///
    /// # Security
    ///
    /// This method must only be called by transport layer implementations
    /// (e.g., `BleTransport::on_message_received_from`) to bind a message to
    /// the physical peer that delivered it. The protocol layer uses this value
    /// to reject control messages where the claimed `sender` does not match the
    /// transport-authenticated peer.
    ///
    /// This method is `pub` because transport implementations live in a
    /// separate crate (`offline-protocol-transport`). It is **not** exposed
    /// through UniFFI bindings, and the underlying field is private +
    /// `#[serde(skip)]`, so application code cannot set it via deserialization
    /// or direct field access.
    ///
    /// **Do not call from application code or expose via FFI.**
    ///
    /// # Errors
    ///
    /// Returns an error if `peer_id` is empty, since an empty transport peer
    /// identity cannot meaningfully authenticate a sender.
    #[doc(hidden)]
    pub fn set_transport_peer_id(&mut self, peer_id: String) -> crate::Result<()> {
        if peer_id.is_empty() {
            return Err(crate::Error::InvalidMessage(
                "transport_peer_id must not be empty".to_string(),
            ));
        }
        self.transport_peer_id = Some(peer_id);
        Ok(())
    }

    /// Returns the transport-verified peer identity, if set.
    pub fn transport_peer_id(&self) -> Option<&str> {
        self.transport_peer_id.as_deref()
    }

    /// Serializes the message to binary (MessagePack-like JSON bytes).
    pub fn to_bytes(&self) -> crate::Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| crate::Error::SerializationError(e.to_string()))
    }

    /// Deserializes a message from binary.
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        serde_json::from_slice(bytes).map_err(|e| crate::Error::DeserializationError(e.to_string()))
    }
}

/// Builder for creating messages with custom settings.
pub struct MessageBuilder {
    sender: UserId,
    recipient: UserId,
    app_id: AppId,
    content_type: ContentType,
    content: String,
    media_metadata: Option<MediaMetadata>,
    priority: MessagePriority,
    ttl: TTL,
    lamport_clock: LamportClock,
    metadata: HashMap<String, String>,
    requires_ack: bool,
    reply_to_msg: Option<MessageId>,
    forwarded_from: Option<ForwardInfo>,
}

impl MessageBuilder {
    /// Creates a new message builder.
    pub fn new(sender: UserId, recipient: UserId, app_id: AppId) -> Self {
        Self {
            sender,
            recipient,
            app_id,
            content_type: ContentType::default(),
            content: String::new(),
            media_metadata: None,
            priority: MessagePriority::default(),
            ttl: TTL::default(),
            lamport_clock: LamportClock::default(),
            metadata: HashMap::new(),
            requires_ack: true,
            reply_to_msg: None,
            forwarded_from: None,
        }
    }

    /// Sets the content type.
    pub fn content_type(mut self, content_type: ContentType) -> Self {
        self.content_type = content_type;
        self
    }

    /// Sets the message content.
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }

    /// Sets the media metadata.
    pub fn media_metadata(mut self, meta: MediaMetadata) -> Self {
        self.media_metadata = Some(meta);
        self
    }

    /// Sets the message priority.
    pub fn priority(mut self, priority: MessagePriority) -> Self {
        self.priority = priority;
        self
    }

    /// Sets the TTL.
    pub fn ttl(mut self, ttl: TTL) -> Self {
        self.ttl = ttl;
        self
    }

    /// Adds metadata to the message.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Sets whether the message requires an ACK.
    pub fn requires_ack(mut self, requires_ack: bool) -> Self {
        self.requires_ack = requires_ack;
        self
    }

    /// Sets the message this is replying to.
    pub fn reply_to_msg(mut self, reply_to: MessageId) -> Self {
        self.reply_to_msg = Some(reply_to);
        self
    }

    /// Sets forwarding attribution on the message.
    pub fn forwarded_from(mut self, info: ForwardInfo) -> Self {
        self.forwarded_from = Some(info);
        self
    }

    /// Sets the Lamport clock value for this message.
    pub fn lamport_clock(mut self, clock: LamportClock) -> Self {
        self.lamport_clock = clock;
        self
    }

    /// Builds the message.
    pub fn build(self) -> Message {
        Message {
            id: MessageId::new(),
            sender: self.sender,
            recipient: self.recipient,
            app_id: self.app_id,
            priority: self.priority,
            ttl: self.ttl,
            hop_count: HopCount::new(),
            timestamp: Timestamp::now(),
            lamport_clock: self.lamport_clock,
            content_type: self.content_type,
            content: self.content,
            binary_content: None,
            media_metadata: self.media_metadata,
            metadata: self.metadata,
            requires_ack: self.requires_ack,
            reply_to_msg: self.reply_to_msg,
            forwarded_from: self.forwarded_from,
            transport_peer_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            "Hello, Bob!",
        )
    }

    #[test]
    fn test_message_creation() {
        let msg = create_test_message();
        assert_eq!(msg.sender.as_str(), "alice");
        assert_eq!(msg.recipient.as_str(), "bob");
        assert_eq!(msg.content, "Hello, Bob!");
        assert_eq!(msg.priority, MessagePriority::Medium);
        assert_eq!(msg.hop_count.value(), 0);
    }

    #[test]
    fn test_message_builder() {
        let msg = Message::builder(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
        )
        .content("Test message")
        .priority(MessagePriority::High)
        .metadata("key1", "value1")
        .requires_ack(false)
        .build();

        assert_eq!(msg.content, "Test message");
        assert_eq!(msg.priority, MessagePriority::High);
        assert_eq!(msg.metadata.get("key1").unwrap(), "value1");
        assert!(!msg.requires_ack);
    }

    #[test]
    fn test_message_hop_operations() {
        let mut msg = create_test_message();
        assert_eq!(msg.hop_count.value(), 0);

        msg.increment_hop().unwrap();
        assert_eq!(msg.hop_count.value(), 1);
    }

    #[test]
    fn test_message_ttl_operations() {
        let mut msg = create_test_message();
        let initial_ttl = msg.ttl.value();

        msg.decrement_ttl().unwrap();
        assert_eq!(msg.ttl.value(), initial_ttl - 1);
    }

    #[test]
    fn test_message_serialization() {
        let msg = create_test_message();

        // JSON serialization
        let json = msg.to_json().unwrap();
        let deserialized = Message::from_json(&json).unwrap();
        assert_eq!(msg.id, deserialized.id);
        assert_eq!(msg.sender, deserialized.sender);
        assert_eq!(msg.content, deserialized.content);

        // Binary serialization
        let bytes = msg.to_bytes().unwrap();
        let deserialized = Message::from_bytes(&bytes).unwrap();
        assert_eq!(msg.id, deserialized.id);
    }

    #[test]
    fn test_message_priority_score() {
        assert_eq!(MessagePriority::Low.score(), 1);
        assert_eq!(MessagePriority::Medium.score(), 2);
        assert_eq!(MessagePriority::High.score(), 3);
        assert_eq!(MessagePriority::Critical.score(), 4);
    }

    #[test]
    fn test_forward_info_serialization() {
        use crate::types::Timestamp;

        let info = ForwardInfo {
            original_sender: UserId::new("alice").unwrap(),
            original_message_id: MessageId::new(),
            original_timestamp: Timestamp::now(),
            forward_count: 1,
        };

        let msg = Message::builder(
            UserId::new("bob").unwrap(),
            UserId::new("charlie").unwrap(),
            AppId::new("test-app").unwrap(),
        )
        .content("Forwarded message")
        .forwarded_from(info.clone())
        .build();

        assert!(msg.forwarded_from.is_some());
        let fwd = msg.forwarded_from.as_ref().unwrap();
        assert_eq!(fwd.original_sender.as_str(), "alice");
        assert_eq!(fwd.forward_count, 1);

        // Round-trip through JSON
        let json = msg.to_json().unwrap();
        let deserialized = Message::from_json(&json).unwrap();
        let fwd2 = deserialized.forwarded_from.as_ref().unwrap();
        assert_eq!(fwd2.original_sender.as_str(), "alice");
        assert_eq!(fwd2.forward_count, 1);
    }

    #[test]
    fn test_backward_compat_no_forwarded_from() {
        // Old messages without forwarded_from should deserialize with None
        let msg = create_test_message();
        let json = msg.to_json().unwrap();

        // Verify forwarded_from is not in the JSON (skip_serializing_if = None)
        assert!(!json.contains("forwarded_from"));

        let deserialized = Message::from_json(&json).unwrap();
        assert!(deserialized.forwarded_from.is_none());
    }

    #[test]
    fn test_forward_chain_increment() {
        // Simulate: Alice sends original → Bob forwards (count=1) → Charlie forwards (count=2)
        let original_msg = Message::builder(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
        )
        .content("Hello from Alice")
        .build();

        // Bob forwards to Charlie — ForwardInfo::from_message creates attribution
        let first_forward = ForwardInfo::from_message(&original_msg);
        assert_eq!(first_forward.original_sender.as_str(), "alice");
        assert_eq!(first_forward.original_message_id, original_msg.id);
        assert_eq!(first_forward.forward_count, 1);

        // Charlie forwards to Dave — should carry original_sender and increment count
        let forwarded_msg = Message::builder(
            UserId::new("bob").unwrap(),
            UserId::new("charlie").unwrap(),
            AppId::new("test-app").unwrap(),
        )
        .content("Hello from Alice")
        .forwarded_from(first_forward)
        .build();

        let second_forward = ForwardInfo::from_message(&forwarded_msg);
        assert_eq!(second_forward.original_sender.as_str(), "alice");
        assert_eq!(second_forward.forward_count, 2);
        assert_eq!(second_forward.original_message_id, original_msg.id);
    }
}
