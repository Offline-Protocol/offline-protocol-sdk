//! Types for MLS operations.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Unique identifier for an MLS group.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GroupId(pub String);

impl GroupId {
    /// Creates a new group ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the group ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Generates a new random group ID for 1:1 sessions.
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

impl From<String> for GroupId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for GroupId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
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
