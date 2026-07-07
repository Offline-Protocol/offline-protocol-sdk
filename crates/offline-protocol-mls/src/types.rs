//! Types for MLS operations.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Unique identifier for an MLS group.
///
/// Group ids flow from the wire into [`crate::storage::MlsStorage`] as raw
/// `key_id` storage keys, so they are validated at construction: one or more
/// non-empty colon-separated segments (`:` is the namespace separator used by
/// `session:<a>:<b>` and `group:<uuid>` ids), each segment subject to the
/// same storage-key policy as `UserId`/`AppId` (no path-traversal components,
/// control characters, `/`, or `\`), with a total length cap of
/// [`GroupId::MAX_LEN`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct GroupId(String);

impl GroupId {
    /// Maximum accepted group id length in bytes.
    ///
    /// Matches the `EncryptedMessage` compact wire format's string-field cap.
    pub const MAX_LEN: usize = 4096;

    /// Creates a new group ID, rejecting storage-hostile values.
    pub fn new(id: impl Into<String>) -> crate::error::Result<Self> {
        let id = id.into();
        Self::validate(&id)?;
        Ok(Self(id))
    }

    fn validate(id: &str) -> crate::error::Result<()> {
        if id.is_empty() {
            return Err(crate::MlsError::InvalidGroupId(
                "Group ID cannot be empty".to_string(),
            ));
        }
        if id.len() > Self::MAX_LEN {
            return Err(crate::MlsError::InvalidGroupId(format!(
                "Group ID length {} exceeds maximum {}",
                id.len(),
                Self::MAX_LEN
            )));
        }
        for segment in id.split(':') {
            offline_protocol_core::validate_id_chars(segment, "Group ID segment")
                .map_err(crate::MlsError::InvalidGroupId)?;
        }
        Ok(())
    }

    /// Returns the group ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Builds the deterministic group ID for a 1:1 session.
    ///
    /// # Precondition
    ///
    /// `user_a` and `user_b` must be validated user ids (see
    /// `offline_protocol_core::UserId`); this constructor does not re-validate
    /// and hostile characters in either input would produce a storage-hostile
    /// group id. All production callers pass ids that were validated at the
    /// wire or config boundary.
    pub fn for_session(user_a: &str, user_b: &str) -> Self {
        // Create deterministic session ID by sorting user IDs
        let mut users = [user_a, user_b];
        users.sort();
        Self(format!("session:{}:{}", users[0], users[1]))
    }
}

impl std::fmt::Display for GroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for GroupId {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        GroupId::new(s).map_err(serde::de::Error::custom)
    }
}

/// Role a user holds within a group.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupRole {
    /// Group administrator — can invite/remove members and change roles.
    Admin,
    /// Regular group member (also the fallback for unknown future variants).
    #[default]
    #[serde(other)]
    Member,
}

impl fmt::Display for GroupRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admin => write!(f, "admin"),
            Self::Member => write!(f, "member"),
        }
    }
}

impl FromStr for GroupRole {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            _ => Err(format!("Invalid role '{}', must be 'admin' or 'member'", s)),
        }
    }
}

/// A bundle containing a key package and metadata for distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPackageBundle {
    /// Unique identifier for this key package.
    pub package_id: String,

    /// User ID this key package belongs to.
    pub user_id: String,

    /// Serialized MLS KeyPackage bytes.
    pub key_package_data: Vec<u8>,

    /// Timestamp when the key package was created (milliseconds since epoch).
    pub created_at_ms: u64,

    /// Timestamp when the key package expires (milliseconds since epoch).
    pub expires_at_ms: u64,

    /// Whether this key package has been uploaded to a server.
    pub synced: bool,
}

impl KeyPackageBundle {
    /// Creates a new key package bundle.
    pub fn new(
        package_id: String,
        user_id: String,
        key_package_data: Vec<u8>,
        lifetime_secs: u64,
    ) -> Self {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        Self {
            package_id,
            user_id,
            key_package_data,
            created_at_ms: now_ms,
            expires_at_ms: now_ms + (lifetime_secs * 1000),
            synced: false,
        }
    }

    /// Checks if the key package has expired (local device's own packages only).
    ///
    /// This compares against the local clock and is valid because `created_at_ms`
    /// and `expires_at_ms` were set on this same device.
    pub fn is_expired(&self) -> bool {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        now_ms >= self.expires_at_ms
    }

    /// Returns the remaining valid lifetime in milliseconds.
    ///
    /// Used when transmitting key packages to peers: the receiver applies
    /// this duration relative to their own clock, eliminating cross-device
    /// clock skew from expiry calculations.
    pub fn remaining_lifetime_ms(&self) -> u64 {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        self.expires_at_ms.saturating_sub(now_ms)
    }

    /// Creates a bundle from received transfer data, computing local expiry
    /// from the sender-provided remaining lifetime.
    pub fn from_transfer(
        package_id: String,
        user_id: String,
        key_package_data: Vec<u8>,
        remaining_lifetime_ms: u64,
    ) -> Self {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        Self {
            package_id,
            user_id,
            key_package_data,
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(remaining_lifetime_ms),
            synced: false,
        }
    }
}

/// Information about an MLS group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfo {
    /// Unique group identifier.
    pub group_id: GroupId,

    /// Human-readable group name (for multi-party groups).
    pub name: Option<String>,

    /// List of member user IDs.
    pub members: Vec<String>,

    /// Count of members in the group.
    pub members_count: u32,

    /// Current epoch number.
    pub epoch: u64,

    /// Whether this is a 1:1 session (2-person group).
    pub is_session: bool,

    /// Timestamp when the group was created.
    pub created_at_ms: u64,

    /// Timestamp of the last activity.
    pub last_activity_ms: u64,
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

impl std::fmt::Display for MlsMessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    pub fn to_base64(&self) -> Result<String, crate::MlsError> {
        let json =
            serde_json::to_vec(self).map_err(|e| crate::MlsError::Serialization(e.to_string()))?;
        Ok(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &json,
        ))
    }

    /// Decodes an encrypted message from base64.
    pub fn from_base64(encoded: &str) -> Result<Self, crate::MlsError> {
        let json = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .map_err(|e| crate::MlsError::Deserialization(e.to_string()))?;
        serde_json::from_slice(&json).map_err(|e| crate::MlsError::Deserialization(e.to_string()))
    }

    /// Maximum allowed length for a string field (group_id, sender_id) in the
    /// compact binary wire format. Prevents allocation bombs from crafted payloads.
    const MAX_STRING_FIELD_LEN: usize = 4096;
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
    pub fn from_bytes(data: &[u8]) -> Result<Self, crate::MlsError> {
        let err = |msg: String| crate::MlsError::Deserialization(msg);
        let mut pos = 0;

        let read_u32 = |pos: &mut usize| -> Result<u32, crate::MlsError> {
            if *pos + 4 > data.len() {
                return Err(err("unexpected end of data reading u32".to_string()));
            }
            let val = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
            *pos += 4;
            Ok(val)
        };

        let read_u64 = |pos: &mut usize| -> Result<u64, crate::MlsError> {
            if *pos + 8 > data.len() {
                return Err(err("unexpected end of data reading u64".to_string()));
            }
            let val = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
            *pos += 8;
            Ok(val)
        };

        let read_bytes = |pos: &mut usize, len: usize| -> Result<Vec<u8>, crate::MlsError> {
            if *pos + len > data.len() {
                return Err(err("unexpected end of data reading bytes".to_string()));
            }
            let val = data[*pos..*pos + len].to_vec();
            *pos += len;
            Ok(val)
        };

        let group_id_len = read_u32(&mut pos)? as usize;
        if group_id_len > Self::MAX_STRING_FIELD_LEN {
            return Err(err(format!(
                "group_id_len {} exceeds maximum {}",
                group_id_len,
                Self::MAX_STRING_FIELD_LEN
            )));
        }
        let group_id = String::from_utf8(read_bytes(&mut pos, group_id_len)?)
            .map_err(|e| err(format!("invalid group_id UTF-8: {}", e)))?;

        let sender_len = read_u32(&mut pos)? as usize;
        if sender_len > Self::MAX_STRING_FIELD_LEN {
            return Err(err(format!(
                "sender_len {} exceeds maximum {}",
                sender_len,
                Self::MAX_STRING_FIELD_LEN
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
    pub fn to_base64(&self) -> Result<String, crate::MlsError> {
        let json =
            serde_json::to_vec(self).map_err(|e| crate::MlsError::Serialization(e.to_string()))?;
        Ok(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &json,
        ))
    }

    /// Decodes a welcome message from base64.
    pub fn from_base64(encoded: &str) -> Result<Self, crate::MlsError> {
        let json = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .map_err(|e| crate::MlsError::Deserialization(e.to_string()))?;
        serde_json::from_slice(&json).map_err(|e| crate::MlsError::Deserialization(e.to_string()))
    }
}

/// Metadata about an MLS group (stored separately from MLS state).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMetadata {
    /// Human-readable group name.
    pub name: Option<String>,

    /// Timestamp when the group was created (milliseconds since epoch).
    pub created_at_ms: u64,

    /// Timestamp of the last activity (milliseconds since epoch).
    pub last_activity_ms: u64,

    /// Custom application-specific metadata.
    #[serde(default)]
    pub custom: std::collections::HashMap<String, String>,

    /// User ID of the group creator.
    #[serde(default)]
    pub created_by: Option<String>,

    /// Per-member role assignments.
    /// Deserialization falls back to migrating legacy `"role:*"` keys from `custom`.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub roles: std::collections::HashMap<String, GroupRole>,
}

impl GroupMetadata {
    /// Prefix previously used for role entries in the `custom` map (legacy).
    pub const LEGACY_ROLE_KEY_PREFIX: &'static str = "role:";

    /// Creates new group metadata with the given name and creator.
    pub fn new(name: Option<String>) -> Self {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        Self {
            name,
            created_at_ms: now_ms,
            last_activity_ms: now_ms,
            custom: std::collections::HashMap::new(),
            created_by: None,
            roles: std::collections::HashMap::new(),
        }
    }

    /// Creates new group metadata with a creator.
    pub fn new_with_creator(name: Option<String>, creator_id: &str) -> Self {
        let mut meta = Self::new(name);
        meta.created_by = Some(creator_id.to_string());
        meta.set_role(creator_id, GroupRole::Admin);
        meta
    }

    /// Migrates any legacy `"role:*"` keys from the `custom` map into the
    /// dedicated `roles` field. Call after deserialization for
    /// backwards-compatibility with older group metadata.
    pub fn migrate_legacy_roles(&mut self) {
        let legacy_keys: Vec<String> = self
            .custom
            .keys()
            .filter(|k| k.starts_with(Self::LEGACY_ROLE_KEY_PREFIX))
            .cloned()
            .collect();
        for key in legacy_keys {
            if let Some(uid) = key.strip_prefix(Self::LEGACY_ROLE_KEY_PREFIX) {
                if !self.roles.contains_key(uid) {
                    let role = self
                        .custom
                        .get(&key)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or_default();
                    self.roles.insert(uid.to_string(), role);
                }
            }
            self.custom.remove(&key);
        }
    }

    /// Updates the last activity timestamp to now.
    pub fn touch(&mut self) {
        self.last_activity_ms = chrono::Utc::now().timestamp_millis() as u64;
    }

    /// Gets the role for a user, defaulting to [`GroupRole::Member`] if not set.
    pub fn get_role(&self, user_id: &str) -> GroupRole {
        self.roles.get(user_id).copied().unwrap_or_default()
    }

    /// Sets the role for a user.
    pub fn set_role(&mut self, user_id: &str, role: GroupRole) {
        self.roles.insert(user_id.to_string(), role);
    }

    /// Removes role metadata for a user (on removal from group).
    pub fn remove_role(&mut self, user_id: &str) {
        self.roles.remove(user_id);
    }

    /// Returns true if any admin role is stored in this metadata.
    pub fn has_any_admin(&self) -> bool {
        self.roles.values().any(|r| *r == GroupRole::Admin)
    }

    /// Returns all `user_id -> role` mappings.
    pub fn get_all_roles(&self) -> std::collections::HashMap<String, GroupRole> {
        self.roles.clone()
    }
}

/// Storage key types for organizing MLS data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKeyType {
    /// Identity/signature key pair.
    Identity,

    /// Key packages for receiving Welcome messages.
    KeyPackage,

    /// MLS group state.
    GroupState,

    /// Epoch secrets for a group.
    EpochSecrets,

    /// Credential data.
    Credential,

    /// Contact's key packages.
    ContactKeyPackage,

    /// Group metadata (name, timestamps).
    GroupMetadata,

    /// Pending welcome messages awaiting delivery.
    PendingWelcome,
}

impl StorageKeyType {
    /// Returns the string representation for storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::KeyPackage => "key_package",
            Self::GroupState => "group_state",
            Self::EpochSecrets => "epoch_secrets",
            Self::Credential => "credential",
            Self::ContactKeyPackage => "contact_key_package",
            Self::GroupMetadata => "group_metadata",
            Self::PendingWelcome => "pending_welcome",
        }
    }
}

impl std::fmt::Display for StorageKeyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_encrypted_message() -> EncryptedMessage {
        EncryptedMessage {
            group_id: GroupId::for_session("alice", "bob"),
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
        assert!(matches!(err, crate::MlsError::InvalidGroupId(_)));
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
