//! The sealed envelope: what a payload looks like on the wire, both ways.
//!
//! Every encrypted payload in the protocol is an [`EncryptedMessage`], in one
//! of two interchangeable encodings. The compact binary codec
//! ([`EncryptedMessage::to_bytes`]) is what peers that negotiated it use; JSON
//! is the permanent floor, because a peer that advertised no capability at all
//! must still be able to read a frame.
//!
//! Both encodings are here, in a crate a leaf node can link, because the phone
//! and the leaf are the two ends of the same envelope: a second
//! implementation of this codec is a second wire format the moment either side
//! is edited alone.

use crate::{Result, SealedError};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use serde::{Deserialize, Serialize};

/// Maximum length of a string field (`group_id`, `sender_id`) in the compact
/// binary wire format.
///
/// It bounds allocation from a crafted payload, but its load-bearing job is
/// disambiguation. A JSON envelope starts with `{"`, which the compact parser
/// reads as the little-endian `u32` 0x0000_227B (8827): above this cap, so a
/// JSON body is deterministically rejected by
/// [`EncryptedMessage::from_bytes`] and falls through to the JSON parser
/// rather than being half-decoded. Raising it past 8827 would make the two
/// encodings ambiguous.
const MAX_STRING_FIELD_LEN: usize = 4096;

/// Unique identifier for an MLS group.
///
/// Group ids flow from the wire into `MlsStorage` as raw `key_id` storage
/// keys, so they are validated at construction: one or more non-empty
/// colon-separated segments (`:` is the namespace separator used by
/// `session:<a>:<b>` and `group:<uuid>` ids), each segment subject to the
/// same storage-key policy as `UserId`/`AppId` (no path-traversal components,
/// control characters, `/`, or `\`), with a total length cap of
/// [`GroupId::MAX_LEN`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct GroupId(String);

impl GroupId {
    /// Maximum accepted group id length in bytes.
    ///
    /// This *is* the compact wire format's string-field cap, not a copy of it:
    /// a group id that could not survive a round trip through the envelope
    /// must not be constructible in the first place.
    pub const MAX_LEN: usize = MAX_STRING_FIELD_LEN;

    /// Creates a new group ID, rejecting storage-hostile values.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        Self::validate(&id)?;
        Ok(Self(id))
    }

    fn validate(id: &str) -> Result<()> {
        if id.is_empty() {
            return Err(SealedError::InvalidGroupId(
                "Group ID cannot be empty".to_string(),
            ));
        }
        if id.len() > Self::MAX_LEN {
            return Err(SealedError::InvalidGroupId(format!(
                "Group ID length {} exceeds maximum {}",
                id.len(),
                Self::MAX_LEN
            )));
        }
        for segment in id.split(':') {
            offline_protocol_core::validate_id_chars(segment, "Group ID segment")
                .map_err(|e| SealedError::InvalidGroupId(e.to_string()))?;
        }
        Ok(())
    }

    /// Returns the group ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Builds the deterministic group ID for a 1:1 session, rejecting
    /// storage-hostile user ids (the id becomes a raw storage key).
    ///
    /// Both peers must land on the same string, so the two ids are put in a
    /// canonical order. When both are addresses that order is
    /// [`Address`](offline_protocol_core::Address) order, the underlying hash
    /// bytes, because the bech32 charset is not ASCII-monotonic (value 4
    /// renders as `y` 0x79, value 5 as `9` 0x39), so sorting the rendered
    /// strings would disagree with every other address comparison in the
    /// protocol. Ids that are not both addresses fall back to string order,
    /// which is all a nickname id has.
    ///
    /// Either way the result is symmetric in its arguments, which is the
    /// property peers actually depend on.
    pub fn for_session(user_a: &str, user_b: &str) -> Result<Self> {
        use offline_protocol_core::Address;

        let ordered = match (user_a.parse::<Address>(), user_b.parse::<Address>()) {
            (Ok(addr_a), Ok(addr_b)) if addr_b < addr_a => [user_b, user_a],
            (Ok(_), Ok(_)) => [user_a, user_b],
            _ => {
                let mut users = [user_a, user_b];
                users.sort();
                users
            }
        };

        Self::new(format!("session:{}:{}", ordered[0], ordered[1]))
    }
}

impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for GroupId {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> core::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        GroupId::new(s).map_err(serde::de::Error::custom)
    }
}

/// Type of MLS message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MlsMessageType {
    /// Application message (encrypted content).
    Application,

    /// Welcome message for new group members.
    Welcome,

    /// Commit message for group state changes.
    Commit,

    /// Proposal message.
    Proposal,
}

impl MlsMessageType {
    /// Returns a stable string representation for serialization/FFI.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Application => "Application",
            Self::Welcome => "Welcome",
            Self::Commit => "Commit",
            Self::Proposal => "Proposal",
        }
    }

    /// Parses a string into an MlsMessageType, returning None for unknown values.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "Application" => Some(Self::Application),
            "Welcome" => Some(Self::Welcome),
            "Commit" => Some(Self::Commit),
            "Proposal" => Some(Self::Proposal),
            _ => None,
        }
    }

    /// Returns a stable single-byte tag for the compact binary wire format.
    pub fn as_u8(&self) -> u8 {
        match self {
            Self::Application => 0,
            Self::Welcome => 1,
            Self::Commit => 2,
            Self::Proposal => 3,
        }
    }

    /// Parses a wire-format tag, returning None for unknown values.
    pub fn from_u8(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Application),
            1 => Some(Self::Welcome),
            2 => Some(Self::Commit),
            3 => Some(Self::Proposal),
            _ => None,
        }
    }
}

impl fmt::Display for MlsMessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An encrypted MLS message ready for transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedMessage {
    /// The group ID this message belongs to.
    pub group_id: GroupId,

    /// Type of MLS message.
    pub message_type: MlsMessageType,

    /// Current epoch when the message was created.
    pub epoch: u64,

    /// Serialized MLS message bytes.
    pub ciphertext: Vec<u8>,

    /// Sender's user ID.
    pub sender_id: String,

    /// Timestamp when the message was created.
    pub timestamp_ms: u64,
}

impl EncryptedMessage {
    /// Encodes the encrypted message to base64 for transport.
    pub fn to_base64(&self) -> Result<String> {
        let json =
            serde_json::to_vec(self).map_err(|e| SealedError::Serialization(e.to_string()))?;
        Ok(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &json,
        ))
    }

    /// Decodes an encrypted message from base64.
    pub fn from_base64(encoded: &str) -> Result<Self> {
        let json = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .map_err(|e| SealedError::Deserialization(e.to_string()))?;
        serde_json::from_slice(&json).map_err(|e| SealedError::Deserialization(e.to_string()))
    }

    /// Maximum allowed ciphertext length in the compact binary wire format.
    const MAX_CIPHERTEXT_LEN: usize = 128 * 1024 * 1024; // 128 MB

    /// Serializes to a compact binary format, avoiding base64/JSON overhead.
    ///
    /// JSON renders `ciphertext` as an integer array (~3.7x inflation), which
    /// is unacceptable for large payloads such as encrypted file chunks.
    ///
    /// Wire format (all multi-byte integers are little-endian):
    /// ```text
    /// [group_id_len:4][group_id][sender_len:4][sender_id]
    /// [message_type:1][epoch:8][timestamp_ms:8]
    /// [ciphertext_len:4][ciphertext]
    /// ```
    pub fn to_bytes(&self) -> Vec<u8> {
        let group_id = self.group_id.as_str().as_bytes();
        let sender_id = self.sender_id.as_bytes();

        let capacity =
            4 + group_id.len() + 4 + sender_id.len() + 1 + 8 + 8 + 4 + self.ciphertext.len();
        let mut buf = Vec::with_capacity(capacity);

        buf.extend_from_slice(&(group_id.len() as u32).to_le_bytes());
        buf.extend_from_slice(group_id);
        buf.extend_from_slice(&(sender_id.len() as u32).to_le_bytes());
        buf.extend_from_slice(sender_id);
        buf.push(self.message_type.as_u8());
        buf.extend_from_slice(&self.epoch.to_le_bytes());
        buf.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        buf.extend_from_slice(&(self.ciphertext.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.ciphertext);

        buf
    }

    /// Deserializes from the compact binary format produced by `to_bytes`.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let err = SealedError::Deserialization;
        let mut pos = 0;

        let read_u32 = |pos: &mut usize| -> Result<u32> {
            if *pos + 4 > data.len() {
                return Err(err("unexpected end of data reading u32".to_string()));
            }
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&data[*pos..*pos + 4]);
            let val = u32::from_le_bytes(bytes);
            *pos += 4;
            Ok(val)
        };

        let read_u64 = |pos: &mut usize| -> Result<u64> {
            if *pos + 8 > data.len() {
                return Err(err("unexpected end of data reading u64".to_string()));
            }
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&data[*pos..*pos + 8]);
            let val = u64::from_le_bytes(bytes);
            *pos += 8;
            Ok(val)
        };

        let read_bytes = |pos: &mut usize, len: usize| -> Result<Vec<u8>> {
            if *pos + len > data.len() {
                return Err(err("unexpected end of data reading bytes".to_string()));
            }
            let val = data[*pos..*pos + len].to_vec();
            *pos += len;
            Ok(val)
        };

        let group_id_len = read_u32(&mut pos)? as usize;
        if group_id_len > MAX_STRING_FIELD_LEN {
            return Err(err(format!(
                "group_id_len {} exceeds maximum {}",
                group_id_len, MAX_STRING_FIELD_LEN
            )));
        }
        let group_id = String::from_utf8(read_bytes(&mut pos, group_id_len)?)
            .map_err(|e| err(format!("invalid group_id UTF-8: {}", e)))?;

        let sender_len = read_u32(&mut pos)? as usize;
        if sender_len > MAX_STRING_FIELD_LEN {
            return Err(err(format!(
                "sender_len {} exceeds maximum {}",
                sender_len, MAX_STRING_FIELD_LEN
            )));
        }
        let sender_id = String::from_utf8(read_bytes(&mut pos, sender_len)?)
            .map_err(|e| err(format!("invalid sender_id UTF-8: {}", e)))?;

        if pos + 1 > data.len() {
            return Err(err(
                "unexpected end of data reading message_type".to_string()
            ));
        }
        let message_type = MlsMessageType::from_u8(data[pos])
            .ok_or_else(|| err(format!("unknown message_type tag {}", data[pos])))?;
        pos += 1;

        let epoch = read_u64(&mut pos)?;
        let timestamp_ms = read_u64(&mut pos)?;

        let ciphertext_len = read_u32(&mut pos)? as usize;
        if ciphertext_len > Self::MAX_CIPHERTEXT_LEN {
            return Err(err(format!(
                "ciphertext_len {} exceeds maximum {}",
                ciphertext_len,
                Self::MAX_CIPHERTEXT_LEN
            )));
        }
        let ciphertext = read_bytes(&mut pos, ciphertext_len)?;

        Ok(Self {
            group_id: GroupId::new(group_id)?,
            message_type,
            epoch,
            ciphertext,
            sender_id,
            timestamp_ms,
        })
    }
}

/// A Welcome message for inviting users to a group.
///
/// A leaf node joins its pair from one of these, which is why it lives beside
/// the envelope rather than with the group-management types in the MLS crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WelcomeMessage {
    /// The group ID the user is being invited to.
    pub group_id: GroupId,

    /// Serialized MLS Welcome message bytes.
    pub welcome_data: Vec<u8>,

    /// The inviter's user ID.
    pub inviter_id: String,

    /// Optional group name for display.
    pub group_name: Option<String>,

    /// Timestamp when the welcome was created.
    pub timestamp_ms: u64,
}

impl WelcomeMessage {
    /// Encodes the welcome message to base64 for transport.
    pub fn to_base64(&self) -> Result<String> {
        let json =
            serde_json::to_vec(self).map_err(|e| SealedError::Serialization(e.to_string()))?;
        Ok(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &json,
        ))
    }

    /// Decodes a welcome message from base64.
    pub fn from_base64(encoded: &str) -> Result<Self> {
        let json = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .map_err(|e| SealedError::Deserialization(e.to_string()))?;
        serde_json::from_slice(&json).map_err(|e| SealedError::Deserialization(e.to_string()))
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use offline_protocol_core::Address;

    /// Two addresses whose hash-byte order is the reverse of their rendered
    /// string order.
    ///
    /// Hand-constructed rather than searched for, so the disagreement is
    /// checkable by eye: the data part's second character encodes the version
    /// byte's low three bits (`0b001`) followed by the hash's top two bits, so
    /// `hash[0] = 0x00` yields charset value 4 to `y` (0x79) and
    /// `hash[0] = 0x40` yields value 5 to `9` (0x39). Byte order puts the
    /// `0x00` address first; ASCII order puts it second.
    fn disagreeing_addresses() -> (Address, Address) {
        let mut low_bytes = [0u8; Address::HASH_LEN];
        low_bytes[0] = 0x00;
        let mut high_bytes = [0u8; Address::HASH_LEN];
        high_bytes[0] = 0x40;
        (
            Address::from_hash_bytes(low_bytes),
            Address::from_hash_bytes(high_bytes),
        )
    }

    #[test]
    fn session_id_orders_addresses_by_hash_not_by_string() {
        let (low, high) = disagreeing_addresses();

        // The premise: the two orders genuinely disagree for this pair.
        assert!(low < high, "hash-byte order");
        assert!(
            low.to_string() > high.to_string(),
            "rendered-string order must be the opposite, or this test proves nothing"
        );

        let expected = format!("session:{}:{}", low, high);
        assert_eq!(
            GroupId::for_session(&low.to_string(), &high.to_string())
                .unwrap()
                .as_str(),
            expected,
            "address slots must follow Address order, not string order"
        );
    }

    #[test]
    fn session_id_is_symmetric_for_addresses() {
        let (low, high) = disagreeing_addresses();
        let (a, b) = (low.to_string(), high.to_string());

        assert_eq!(
            GroupId::for_session(&a, &b).unwrap(),
            GroupId::for_session(&b, &a).unwrap(),
            "both peers must derive the same slot regardless of argument order"
        );
    }

    /// A pair that is not two addresses has no hash order to use, so it keeps
    /// the string ordering it has always had.
    #[test]
    fn session_id_falls_back_to_string_order_for_non_addresses() {
        assert_eq!(
            GroupId::for_session("bob", "alice").unwrap().as_str(),
            "session:alice:bob"
        );

        let (address, _) = disagreeing_addresses();
        let mixed = GroupId::for_session("alice", &address.to_string()).unwrap();
        let mixed_reversed = GroupId::for_session(&address.to_string(), "alice").unwrap();
        assert_eq!(mixed, mixed_reversed, "mixed pairs must stay symmetric");
    }

    fn sample_encrypted_message() -> EncryptedMessage {
        EncryptedMessage {
            group_id: GroupId::for_session("alice", "bob").unwrap(),
            message_type: MlsMessageType::Application,
            epoch: 42,
            ciphertext: vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01],
            sender_id: "alice".to_string(),
            timestamp_ms: 1_700_000_000_123,
        }
    }

    #[test]
    fn test_encrypted_message_bytes_roundtrip() {
        let msg = sample_encrypted_message();
        let bytes = msg.to_bytes();
        let decoded = EncryptedMessage::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.group_id, msg.group_id);
        assert_eq!(decoded.message_type, msg.message_type);
        assert_eq!(decoded.epoch, msg.epoch);
        assert_eq!(decoded.ciphertext, msg.ciphertext);
        assert_eq!(decoded.sender_id, msg.sender_id);
        assert_eq!(decoded.timestamp_ms, msg.timestamp_ms);
    }

    #[test]
    fn test_encrypted_message_bytes_roundtrip_empty_ciphertext() {
        let mut msg = sample_encrypted_message();
        msg.ciphertext = Vec::new();
        let decoded = EncryptedMessage::from_bytes(&msg.to_bytes()).unwrap();
        assert!(decoded.ciphertext.is_empty());
    }

    #[test]
    fn test_encrypted_message_from_bytes_truncated() {
        let bytes = sample_encrypted_message().to_bytes();
        // Every strict prefix must fail cleanly, never panic.
        for len in 0..bytes.len() {
            assert!(
                EncryptedMessage::from_bytes(&bytes[..len]).is_err(),
                "truncation at {} unexpectedly succeeded",
                len
            );
        }
    }

    #[test]
    fn test_encrypted_message_from_bytes_oversized_group_id_rejected() {
        // Claim a group_id length far beyond the cap without supplying data.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(u32::MAX).to_le_bytes());
        let err = EncryptedMessage::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("group_id_len"));
    }

    /// The sibling of the group-id cap, which had no test of its own before
    /// this codec moved: a crafted `sender_len` must be refused by the cap
    /// rather than by running off the end of the buffer.
    #[test]
    fn test_encrypted_message_from_bytes_oversized_sender_id_rejected() {
        let mut bytes = Vec::new();
        let group_id = b"session:alice:bob";
        bytes.extend_from_slice(&(group_id.len() as u32).to_le_bytes());
        bytes.extend_from_slice(group_id);
        bytes.extend_from_slice(&(u32::MAX).to_le_bytes());
        let err = EncryptedMessage::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("sender_len"),
            "expected the sender_len cap to refuse it, got: {err}"
        );
    }

    #[test]
    fn test_encrypted_message_from_bytes_oversized_ciphertext_rejected() {
        let mut msg = sample_encrypted_message();
        msg.ciphertext = vec![0u8; 4];
        let mut bytes = msg.to_bytes();
        // Overwrite the ciphertext length prefix (last 4 + 4 bytes from the end).
        let ct_len_offset = bytes.len() - 4 - 4;
        bytes[ct_len_offset..ct_len_offset + 4].copy_from_slice(&(u32::MAX).to_le_bytes());
        let err = EncryptedMessage::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("ciphertext_len"));
    }

    #[test]
    fn test_encrypted_message_from_bytes_unknown_message_type_rejected() {
        let msg = sample_encrypted_message();
        let mut bytes = msg.to_bytes();
        let tag_offset = 4 + msg.group_id.as_str().len() + 4 + msg.sender_id.len();
        bytes[tag_offset] = 0xFF;
        let err = EncryptedMessage::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("message_type"));
    }

    /// The property that lets one parser accept both encodings: a JSON body
    /// must be refused by the compact parser rather than half-decoded, so the
    /// caller can fall through to JSON. `{"` reads as the little-endian u32
    /// 8827, which is over the cap.
    ///
    /// The engine pins the fall-through itself; this pins the half of it that
    /// lives in the codec, where the cap that makes it work is defined.
    #[test]
    fn json_bodies_are_refused_by_the_compact_parser() {
        let json = serde_json::to_vec(&sample_encrypted_message()).unwrap();
        assert!(json.starts_with(b"{\""), "premise: JSON starts with {{\"");

        let err = EncryptedMessage::from_bytes(&json).unwrap_err();
        assert!(
            err.to_string().contains("group_id_len"),
            "a JSON body must be refused by the string-field cap, got: {err}"
        );

        let claimed = u32::from_le_bytes([json[0], json[1], json[2], json[3]]) as usize;
        assert!(
            claimed > MAX_STRING_FIELD_LEN,
            "the cap only disambiguates while it stays below {claimed}"
        );
    }

    #[test]
    fn test_group_id_accepts_legitimate_formats() {
        assert!(GroupId::new("group:0bd6e5f2-3a70-4a4e-9c3f-1c1f2a3b4c5d").is_ok());
        assert!(GroupId::new("session:alice:bob").is_ok());
        assert!(GroupId::new("plain-segment_1.x@y").is_ok());
    }

    #[test]
    fn test_group_id_rejects_storage_hostile_chars() {
        assert!(GroupId::new("").is_err());
        assert!(GroupId::new("group/evil").is_err());
        assert!(GroupId::new("group\\evil").is_err());
        assert!(GroupId::new("group\0evil").is_err());
        assert!(GroupId::new("group\nevil").is_err());
        assert!(GroupId::new("group\x7Fevil").is_err()); // DEL
        assert!(GroupId::new(".").is_err());
        assert!(GroupId::new("..").is_err());
        // Path traversal hidden inside a segment
        assert!(GroupId::new("session:..:bob").is_err());
        assert!(GroupId::new("group:../../etc").is_err());
        // Empty segments (leading/trailing/doubled colons)
        assert!(GroupId::new("session:").is_err());
        assert!(GroupId::new(":session").is_err());
        assert!(GroupId::new("a::b").is_err());
    }

    #[test]
    fn test_group_id_length_cap() {
        assert!(GroupId::new("a".repeat(GroupId::MAX_LEN)).is_ok());
        assert!(GroupId::new("a".repeat(GroupId::MAX_LEN + 1)).is_err());
    }

    #[test]
    fn test_group_id_deserialize_rejects_hostile() {
        assert!(serde_json::from_str::<GroupId>(r#""evil/path""#).is_err());
        assert!(serde_json::from_str::<GroupId>(r#""..""#).is_err());
        assert!(serde_json::from_str::<GroupId>(r#""session:alice:bob""#).is_ok());
    }

    #[test]
    fn test_encrypted_message_from_bytes_rejects_hostile_group_id() {
        // Hand-craft wire bytes: the typed constructor can no longer produce
        // an invalid group id, but a malicious peer can still send one.
        let mut bytes = Vec::new();
        let hostile = b"evil/../path";
        bytes.extend_from_slice(&(hostile.len() as u32).to_le_bytes());
        bytes.extend_from_slice(hostile);
        let sender = b"alice";
        bytes.extend_from_slice(&(sender.len() as u32).to_le_bytes());
        bytes.extend_from_slice(sender);
        bytes.push(0); // Application
        bytes.extend_from_slice(&42u64.to_le_bytes()); // epoch
        bytes.extend_from_slice(&1u64.to_le_bytes()); // timestamp_ms
        bytes.extend_from_slice(&0u32.to_le_bytes()); // empty ciphertext
        let err = EncryptedMessage::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, SealedError::InvalidGroupId(_)));
    }

    #[test]
    fn test_mls_message_type_u8_roundtrip() {
        for msg_type in [
            MlsMessageType::Application,
            MlsMessageType::Welcome,
            MlsMessageType::Commit,
            MlsMessageType::Proposal,
        ] {
            assert_eq!(MlsMessageType::from_u8(msg_type.as_u8()), Some(msg_type));
        }
        assert_eq!(MlsMessageType::from_u8(4), None);
    }
}
