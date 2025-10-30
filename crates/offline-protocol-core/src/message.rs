//! Message types and envelope structures

use crate::types::{DeviceId, MessageId, Priority, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Type of message content
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum MessageType {
    Text = 0,
    File = 1,
    FileChunk = 2,
    Control = 3,
}

/// Text message content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextMessage {
    pub text: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// File message metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMessage {
    pub name: String,
    pub size: u64,
    pub mime_type: String,
    pub total_chunks: u32,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Individual file chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    pub file_id: MessageId,
    pub chunk_index: u32,
    pub total_chunks: u32,
    pub data: Vec<u8>,
    pub checksum: u32,
}

/// Control message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Acknowledgment of message receipt
    Ack {
        message_id: MessageId,
        hop_count: u8,
    },
    /// Beacon for neighbor discovery
    Beacon {
        device_id: DeviceId,
        username: UserId,
        is_relay: bool,
        connection_count: u8,
    },
    /// Request for missing file chunks
    ChunkRequest {
        file_id: MessageId,
        chunk_indices: Vec<u32>,
    },
}

/// Message content variants
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Text(TextMessage),
    File(FileMessage),
    FileChunk(FileChunk),
    Control(ControlMessage),
}

impl Message {
    /// Get the message type
    pub fn message_type(&self) -> MessageType {
        match self {
            Message::Text(_) => MessageType::Text,
            Message::File(_) => MessageType::File,
            Message::FileChunk(_) => MessageType::FileChunk,
            Message::Control(_) => MessageType::Control,
        }
    }

    /// Check if this is a control message
    pub fn is_control(&self) -> bool {
        matches!(self, Message::Control(_))
    }
}

/// Message envelope containing routing and metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEnvelope {
    /// Unique message identifier
    pub message_id: MessageId,
    
    /// Sender's device ID
    pub sender_device_id: DeviceId,
    
    /// Sender's user ID
    pub sender_user_id: UserId,
    
    /// Recipient's user ID (None for broadcast)
    pub recipient_user_id: Option<UserId>,
    
    /// Time-to-live (decremented at each hop)
    pub ttl: u8,
    
    /// Number of hops taken
    pub hop_count: u8,
    
    /// Message priority
    pub priority: Priority,
    
    /// Timestamp when message was created
    pub timestamp: DateTime<Utc>,
    
    /// The actual message content
    pub message: Message,
}

impl MessageEnvelope {
    /// Create a new message envelope
    pub fn new(
        sender_device_id: DeviceId,
        sender_user_id: UserId,
        recipient_user_id: Option<UserId>,
        message: Message,
        priority: Priority,
        ttl: u8,
    ) -> Self {
        Self {
            message_id: MessageId::new(),
            sender_device_id,
            sender_user_id,
            recipient_user_id,
            ttl,
            hop_count: 0,
            priority,
            timestamp: Utc::now(),
            message,
        }
    }

    /// Serialize the envelope to MessagePack bytes
    pub fn to_bytes(&self) -> crate::Result<Vec<u8>> {
        rmp_serde::to_vec(self).map_err(Into::into)
    }

    /// Deserialize an envelope from MessagePack bytes
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        rmp_serde::from_slice(bytes).map_err(Into::into)
    }

    /// Decrement TTL and increment hop count for forwarding
    pub fn forward(&mut self) -> crate::Result<()> {
        if self.ttl == 0 {
            return Err(crate::Error::TtlExceeded);
        }
        self.ttl -= 1;
        self.hop_count += 1;
        Ok(())
    }

    /// Check if TTL has expired
    pub fn is_expired(&self) -> bool {
        self.ttl == 0
    }

    /// Get the size of the serialized envelope in bytes
    pub fn size(&self) -> crate::Result<usize> {
        Ok(self.to_bytes()?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_envelope_serialization() {
        let envelope = MessageEnvelope::new(
            DeviceId::new(),
            UserId::new("user-123"),
            Some(UserId::new("user-456")),
            Message::Text(TextMessage {
                text: "Hello, World!".to_string(),
                metadata: HashMap::new(),
            }),
            Priority::High,
            8,
        );

        let bytes = envelope.to_bytes().unwrap();
        let deserialized = MessageEnvelope::from_bytes(&bytes).unwrap();

        assert_eq!(envelope.message_id, deserialized.message_id);
        assert_eq!(envelope.sender_device_id, deserialized.sender_device_id);
        assert_eq!(envelope.priority, deserialized.priority);
    }

    #[test]
    fn test_ttl_decrement() {
        let mut envelope = MessageEnvelope::new(
            DeviceId::new(),
            UserId::new("user-123"),
            Some(UserId::new("user-456")),
            Message::Text(TextMessage {
                text: "Test".to_string(),
                metadata: HashMap::new(),
            }),
            Priority::Medium,
            2,
        );

        assert_eq!(envelope.ttl, 2);
        assert_eq!(envelope.hop_count, 0);

        envelope.forward().unwrap();
        assert_eq!(envelope.ttl, 1);
        assert_eq!(envelope.hop_count, 1);

        envelope.forward().unwrap();
        assert_eq!(envelope.ttl, 0);
        assert_eq!(envelope.hop_count, 2);

        assert!(envelope.forward().is_err());
    }
}

