//! Message types and structures.

use crate::types::{AppId, HopCount, Timestamp, UserId, TTL};
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

/// Message priority levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessagePriority {
    /// Low priority - can be delayed or dropped under congestion.
    Low,
    /// Medium priority - default for most messages.
    Medium,
    /// High priority - important messages that should be delivered quickly.
    High,
    /// Critical priority - emergency messages, highest delivery guarantee.
    Critical,
}

impl Default for MessagePriority {
    fn default() -> Self {
        Self::Medium
    }
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

    /// Timestamp when the message was created.
    pub timestamp: Timestamp,

    /// Message content (text, JSON, etc.).
    pub content: String,

    /// Optional metadata for application-specific use.
    #[serde(default)]
    pub metadata: HashMap<String, String>,

    /// Whether this message requires an ACK.
    #[serde(default = "default_requires_ack")]
    pub requires_ack: bool,
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
            content: content.into(),
            metadata: HashMap::new(),
            requires_ack: true,
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
    content: String,
    priority: MessagePriority,
    ttl: TTL,
    metadata: HashMap<String, String>,
    requires_ack: bool,
}

impl MessageBuilder {
    /// Creates a new message builder.
    pub fn new(sender: UserId, recipient: UserId, app_id: AppId) -> Self {
        Self {
            sender,
            recipient,
            app_id,
            content: String::new(),
            priority: MessagePriority::default(),
            ttl: TTL::default(),
            metadata: HashMap::new(),
            requires_ack: true,
        }
    }

    /// Sets the message content.
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
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
            content: self.content,
            metadata: self.metadata,
            requires_ack: self.requires_ack,
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
}
