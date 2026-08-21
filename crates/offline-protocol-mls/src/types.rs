//! Types for MLS operations.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The sealed-envelope types, re-exported so that every path into them keeps
/// working and there is still exactly one definition of each.
///
/// They live in `offline-protocol-sealed` because a leaf node encodes and
/// decodes the same envelope this crate does, and cannot link this crate:
/// this one is built on OpenMLS, which needs `std`. See ADR 0022.
pub use offline_protocol_sealed::{EncryptedMessage, GroupId, MlsMessageType, WelcomeMessage};

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

/// Error returned when parsing a [`GroupRole`] from a string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid role '{0}', must be 'admin' or 'member'")]
pub struct ParseGroupRoleError(pub String);

impl FromStr for GroupRole {
    type Err = ParseGroupRoleError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            _ => Err(ParseGroupRoleError(s.to_string())),
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

    /// Whether this package is reserved for a publication slot.
    ///
    /// An MLS key package's init key is consumed by the first peer who uses it,
    /// so a package standing in a published record must not also be handed to a
    /// peer over the push path: the second user of it builds a Welcome that can
    /// never be processed. Reserved packages are therefore invisible to both
    /// entry points that hand packages out —
    /// [`MlsManager::take_push_key_package`](crate::MlsManager::take_push_key_package)
    /// (the push path) and
    /// [`MlsManager::get_or_create_key_package`](crate::MlsManager::get_or_create_key_package)
    /// (the peer-less one).
    ///
    /// Additive and defaulted, so packages written before this field existed
    /// deserialize as unreserved — which is what they are.
    #[serde(default)]
    pub reserved_for_publication: bool,

    /// The peer this package has been handed to over the push path, if any.
    ///
    /// This is what keeps the push path from advertising one init key to
    /// everybody: a package is claimed by the first peer it is pushed to and
    /// afterwards only ever re-handed to *that* peer, so a repeat push costs no
    /// new key material while two peers never share one. RFC 9420 §16.8 asks
    /// for a key package to be rotated as soon as possible after use; the
    /// rotation trigger is consumption, which
    /// [`MlsManager::key_package_by_id`](crate::MlsManager::key_package_by_id)
    /// already reports, and a consumed package's successor is minted on the
    /// next push to that peer.
    ///
    /// `None` means unclaimed — either minted outside the push path (the
    /// peer-less FFI entry point) or written before this field existed. Those
    /// are claimable, so upgrading does not strand the package already in
    /// storage.
    ///
    /// Local bookkeeping only: never serialized onto the wire, and never part
    /// of the FFI record.
    #[serde(default)]
    pub assigned_peer: Option<String>,

    /// The TLS-serialized OpenMLS hash reference this package's private init
    /// key is stored under in the provider, stamped at mint time.
    ///
    /// A cache: deriving the reference from `key_package_data` costs a TLS
    /// parse plus a signature validation, and the push path's pool scan checks
    /// every stored package's usability on every push — without this it pays
    /// that per package per push. Records written before the field existed
    /// deserialize as `None` and are backfilled on their first load.
    ///
    /// Local bookkeeping only, like [`Self::assigned_peer`]: never on the
    /// wire, never in the FFI record.
    #[serde(default)]
    pub provider_hash_ref: Option<Vec<u8>>,
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
            reserved_for_publication: false,
            assigned_peer: None,
            provider_hash_ref: None,
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

    /// Whether the package expired more than `grace_secs` ago.
    ///
    /// Expiry and the destruction of the private init key are deliberately two
    /// different moments. An expired package is no longer advertised, but a peer
    /// holding a copy handed out just before expiry may still Welcome us — and
    /// that Welcome is only processable while the init key is in provider
    /// storage. The grace window is how long that stays true; past it the key
    /// material is destroyed rather than left resident for the life of the
    /// install.
    pub fn expired_past_grace(&self, grace_secs: u64) -> bool {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        now_ms
            >= self
                .expires_at_ms
                .saturating_add(grace_secs.saturating_mul(1000))
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
            // A peer's package, never one of our publication slots.
            reserved_for_publication: false,
            // Nor one of ours to hand out — the push-path assignment is about
            // packages this device minted.
            assigned_peer: None,
            // Provider refs describe our own private material; a peer's
            // package has none here.
            provider_hash_ref: None,
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

    /// How many leaves the roster read *skipped* because they do not carry the
    /// address their own signature key derives to.
    ///
    /// Normally zero: the Welcome and commit gates refuse such a leaf before it
    /// can enter local group state, so a non-zero count means the state was
    /// written behind the SDK's back — a direct write to the install-scoped
    /// provider store, or a group joined by a build predating those gates.
    ///
    /// A count, not the identities. The claimed credentials are attacker-chosen
    /// strings, and the whole reason [`crate::group::GroupManager::get_group_info`]
    /// filters them out is that the roster must not carry an identity nobody
    /// proved; handing the same string back through a second field would return
    /// it by another door. It is also not what an app should act on: the leaf
    /// holds live group secrets and reads everything sent to the group, so the
    /// remedy is to abandon the group, not to evict one member of it.
    ///
    /// Additive with `#[serde(default)]` so records written before this field
    /// existed still load.
    #[serde(default)]
    pub unproven_members: u32,
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
