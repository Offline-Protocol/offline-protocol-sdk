//! MLS (Message Layer Security) integration for end-to-end encryption.
//!
//! This module provides end-to-end encryption support using the MLS protocol.
//! Messages can be encrypted before sending and decrypted after receiving,
//! providing forward secrecy and post-compromise security.
//!
//! # Architecture
//!
//! The MLS integration uses a storage-agnostic design:
//! - Apps provide a storage implementation via the [`MlsStorage`] trait
//! - The SDK handles all MLS protocol operations
//! - Encrypted messages are transported using the existing transport layer
//!
//! # Usage
//!
//! ```ignore
//! use offline_protocol::mls::{MlsManager, MlsStorage};
//!
//! // Create storage (apps implement this for platform-native secure storage)
//! let storage = MySecureStorage::new();
//!
//! // Create the MLS manager
//! let mls = MlsManager::new("user123", storage)?;
//!
//! // Generate a key package to share with others
//! let key_package = mls.generate_key_package()?;
//!
//! // Encrypt a message for another user
//! let encrypted = mls.encrypt_for_user("bob", b"Hello!")?;
//! ```

// Re-export MLS types from the MLS crate
pub use offline_protocol_mls::{
    EncryptedMessage, GroupId, GroupInfo, GroupMetadata, KeyPackageBundle, MlsError, MlsManager,
    MlsMessageType, MlsStorage, Result as MlsResult, StorageError, WelcomeMessage,
};

// Re-export the in-memory storage for testing
pub use offline_protocol_mls::storage::InMemoryStorage;

use crate::{Error, OfflineProtocol, Result};
use offline_protocol_core::MessagePriority;
use std::sync::Arc;

/// Metadata keys for MLS-encrypted messages.
pub mod metadata_keys {
    /// Key indicating the message is encrypted.
    pub const ENCRYPTED: &str = "mls_encrypted";

    /// Key for the MLS group ID.
    pub const GROUP_ID: &str = "mls_group_id";

    /// Key for the MLS epoch.
    pub const EPOCH: &str = "mls_epoch";

    /// Key for the message type (application, welcome, commit).
    pub const MESSAGE_TYPE: &str = "mls_message_type";
}

/// Extension trait for integrating MLS with the protocol.
pub trait MlsProtocolExt {
    /// Encrypts a plaintext message for a recipient.
    ///
    /// This wraps the plaintext in an MLS ciphertext suitable for transport.
    fn encrypt_message(
        &self,
        mls_manager: &MlsManager,
        recipient: &str,
        plaintext: &[u8],
    ) -> Result<String>;

    /// Decrypts an encrypted message.
    ///
    /// Returns the decrypted plaintext if successful.
    fn decrypt_message(
        &self,
        mls_manager: &MlsManager,
        encrypted_content: &str,
    ) -> Result<Option<Vec<u8>>>;
}

/// Converts an MLS encrypted message to a base64-encoded string for transport.
pub fn encode_for_transport(encrypted: &EncryptedMessage) -> Result<String> {
    encrypted
        .to_base64()
        .map_err(|e| Error::Other(format!("Failed to encode encrypted message: {}", e)))
}

/// Decodes an encrypted message from a base64-encoded string.
pub fn decode_from_transport(encoded: &str) -> Result<EncryptedMessage> {
    EncryptedMessage::from_base64(encoded)
        .map_err(|e| Error::Other(format!("Failed to decode encrypted message: {}", e)))
}

/// Creates an MLS manager with the given storage.
///
/// This is a convenience function for creating an MLS manager.
pub fn create_manager(
    user_id: impl Into<String>,
    storage: Arc<dyn MlsStorage>,
) -> Result<MlsManager> {
    MlsManager::new(user_id, storage)
        .map_err(|e| Error::Other(format!("Failed to create MLS manager: {}", e)))
}

// ============================================================================
// COMMIT DISTRIBUTION HELPERS
// ============================================================================

/// Result of distributing an MLS commit to group members.
#[derive(Debug)]
pub struct CommitDistributionResult {
    /// Number of members the commit was sent to.
    pub sent_count: usize,

    /// User IDs of members that failed to receive the commit.
    pub failed_recipients: Vec<String>,
}

/// Distributes an MLS commit message to all group members.
///
/// This sends the commit to all members of the group (excluding the sender)
/// via the internal message path (bypassing MLS 1:1 encryption) since the
/// commit is already MLS group ciphertext.
///
/// # Arguments
///
/// * `protocol` - The protocol instance to send messages through
/// * `mls_manager` - The MLS manager for group info
/// * `commit` - The commit message to distribute
///
/// # Returns
///
/// Returns a result indicating how many members received the commit.
pub fn distribute_commit(
    protocol: &mut OfflineProtocol,
    mls_manager: &MlsManager,
    commit: &EncryptedMessage,
) -> Result<CommitDistributionResult> {
    let group_info = mls_manager
        .get_group_info(&commit.group_id)
        .map_err(|e| Error::Other(format!("Failed to get group info: {}", e)))?
        .ok_or_else(|| Error::Other("Group not found".to_string()))?;

    let sender_id = mls_manager.user_id();

    // Build a __GRP_MLS_COMMIT__ internal message so the commit bypasses
    // the MLS 1:1 encryption layer (it is already MLS group ciphertext).
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    let commit_payload = serde_json::json!({
        "group_id": commit.group_id.as_str(),
        "commit_type": "add",
        "ciphertext": BASE64.encode(&commit.ciphertext),
        "epoch": commit.epoch,
    });
    let commit_content = format!(
        "__GRP_MLS_COMMIT__{}",
        serde_json::to_string(&commit_payload)
            .map_err(|e| Error::Other(format!("Serialize commit: {}", e)))?
    );

    let mut sent_count = 0;
    let mut failed_recipients = Vec::new();

    for member_id in &group_info.members {
        // Skip self
        if member_id == sender_id {
            continue;
        }

        match protocol.send_internal_message(
            member_id,
            commit_content.clone(),
            MessagePriority::High,
        ) {
            Ok(_) => sent_count += 1,
            Err(_) => failed_recipients.push(member_id.clone()),
        }
    }

    Ok(CommitDistributionResult {
        sent_count,
        failed_recipients,
    })
}

/// Distributes an MLS Welcome message to a new group member.
///
/// This sends via the internal message path (bypassing MLS 1:1 encryption)
/// since the welcome is already MLS group ciphertext.
///
/// # Arguments
///
/// * `protocol` - The protocol instance to send messages through
/// * `welcome` - The welcome message to send
/// * `recipient` - The user ID of the new member
pub fn send_welcome(
    protocol: &mut OfflineProtocol,
    welcome: &WelcomeMessage,
    recipient: &str,
) -> Result<()> {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    let welcome_payload = serde_json::json!({
        "group_id": welcome.group_id.as_str(),
        "group_name": welcome.group_name,
        "welcome_data": BASE64.encode(&welcome.welcome_data),
        "member_list": [],
    });
    let welcome_content = format!(
        "__GRP_MLS_WELCOME__{}",
        serde_json::to_string(&welcome_payload)
            .map_err(|e| Error::Other(format!("Serialize welcome: {}", e)))?
    );

    protocol.send_internal_message(recipient, welcome_content, MessagePriority::High)?;
    Ok(())
}

/// Removes a member from a group and distributes the commit to remaining members.
///
/// This is a convenience function that combines `remove_group_member` with
/// automatic commit distribution.
///
/// # Arguments
///
/// * `protocol` - The protocol instance to send messages through
/// * `mls_manager` - The MLS manager
/// * `group_id` - The group to remove the member from
/// * `member_id` - The user ID of the member to remove
pub fn remove_member_and_distribute(
    protocol: &mut OfflineProtocol,
    mls_manager: &MlsManager,
    group_id: &GroupId,
    member_id: &str,
) -> Result<CommitDistributionResult> {
    let commit = mls_manager
        .remove_group_member(group_id, member_id)
        .map_err(|e| Error::Other(format!("Failed to remove member: {}", e)))?;

    distribute_commit(protocol, mls_manager, &commit)
}

/// Updates group keys and distributes the commit to all members.
///
/// This is a convenience function that combines `update_keys` with
/// automatic commit distribution.
///
/// # Arguments
///
/// * `protocol` - The protocol instance to send messages through
/// * `mls_manager` - The MLS manager
/// * `group_id` - The group to update keys for
pub fn update_keys_and_distribute(
    protocol: &mut OfflineProtocol,
    mls_manager: &MlsManager,
    group_id: &GroupId,
) -> Result<CommitDistributionResult> {
    let commit = mls_manager
        .update_keys(group_id)
        .map_err(|e| Error::Other(format!("Failed to update keys: {}", e)))?;

    distribute_commit(protocol, mls_manager, &commit)
}

/// Adds a member to a group and sends the welcome message.
///
/// This is a convenience function that combines `add_group_member` with
/// automatic welcome message delivery.
///
/// # Arguments
///
/// * `protocol` - The protocol instance to send messages through
/// * `mls_manager` - The MLS manager
/// * `group_id` - The group to add the member to
/// * `member_key_package` - The new member's key package
/// * `member_id` - The user ID of the new member
pub fn add_member_and_send_welcome(
    protocol: &mut OfflineProtocol,
    mls_manager: &MlsManager,
    group_id: &GroupId,
    member_key_package: &[u8],
    member_id: &str,
) -> Result<()> {
    let (welcome, commit) = mls_manager
        .add_group_member(group_id, member_key_package)
        .map_err(|e| Error::Other(format!("Failed to add member: {}", e)))?;

    send_welcome(protocol, &welcome, member_id)?;
    distribute_commit(protocol, mls_manager, &commit)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_manager() {
        let storage = Arc::new(InMemoryStorage::new());
        let manager = create_manager("test_user", storage).unwrap();
        assert_eq!(manager.user_id(), "test_user");
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let encrypted = EncryptedMessage {
            group_id: GroupId::new("test-group"),
            message_type: MlsMessageType::Application,
            epoch: 1,
            ciphertext: vec![1, 2, 3, 4],
            sender_id: "alice".to_string(),
            timestamp_ms: 1234567890,
        };

        let encoded = encode_for_transport(&encrypted).unwrap();
        let decoded = decode_from_transport(&encoded).unwrap();

        assert_eq!(encrypted.group_id, decoded.group_id);
        assert_eq!(encrypted.epoch, decoded.epoch);
        assert_eq!(encrypted.ciphertext, decoded.ciphertext);
    }
}
