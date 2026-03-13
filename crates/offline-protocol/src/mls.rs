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

use crate::{Error, Result};
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
