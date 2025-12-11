//! 1:1 session management using MLS groups.
//!
//! This module provides a higher-level abstraction for 1:1 encrypted messaging
//! by using 2-person MLS groups under the hood.

use crate::error::{MlsError, Result};
use crate::group::GroupManager;
use crate::provider::MlsProvider;
use crate::storage::MlsStorage;
use crate::types::{EncryptedMessage, GroupId, GroupInfo, MlsMessageType, WelcomeMessage};

use openmls::prelude::*;
use openmls::prelude::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls_traits::signatures::Signer;
use std::sync::Arc;
use tracing::{debug, info};

/// Manages 1:1 encrypted sessions.
pub struct SessionManager {
    /// The local user's ID.
    user_id: String,

    /// Group manager for underlying MLS operations.
    group_manager: GroupManager,

    /// Storage backend.
    #[allow(dead_code)]
    storage: Arc<dyn MlsStorage>,
}

impl SessionManager {
    /// Creates a new session manager.
    pub fn new(user_id: String, storage: Arc<dyn MlsStorage>, provider: MlsProvider) -> Self {
        let group_manager = GroupManager::new(user_id.clone(), storage.clone(), provider);
        Self {
            user_id,
            group_manager,
            storage,
        }
    }

    /// Returns a reference to the underlying group manager.
    pub fn group_manager(&self) -> &GroupManager {
        &self.group_manager
    }

    /// Gets the session ID for a 1:1 conversation with another user.
    pub fn get_session_id(&self, other_user_id: &str) -> GroupId {
        GroupId::for_session(&self.user_id, other_user_id)
    }

    /// Checks if a session exists with another user.
    pub fn has_session(&self, other_user_id: &str) -> Result<bool> {
        let session_id = self.get_session_id(other_user_id);
        let group = self.group_manager.load_group(&session_id)?;
        Ok(group.is_some())
    }

    /// Creates a new session with another user using their key package.
    pub fn create_session(
        &self,
        other_user_id: &str,
        their_key_package: KeyPackage,
        credential_with_key: &CredentialWithKey,
        signer: &impl Signer,
    ) -> Result<WelcomeMessage> {
        let session_id = self.get_session_id(other_user_id);

        if self.group_manager.load_group(&session_id)?.is_some() {
            return Err(MlsError::GroupCreation(format!(
                "Session already exists with user: {}",
                other_user_id
            )));
        }

        let mut group = self.group_manager.create_group(
            &session_id,
            credential_with_key,
            signer,
        )?;

        let (_commit, welcome) = self.group_manager.add_member(
            &mut group,
            their_key_package,
            signer,
        )?;

        self.group_manager.save_group(&session_id, &group)?;

        let welcome_bytes = welcome
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        info!(
            session_id = %session_id,
            other_user = %other_user_id,
            "Created new 1:1 session"
        );

        Ok(WelcomeMessage {
            group_id: session_id,
            welcome_data: welcome_bytes,
            inviter_id: self.user_id.clone(),
            group_name: None,
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        })
    }

    /// Joins a session using a Welcome message.
    pub fn join_session(&self, welcome_msg: &WelcomeMessage) -> Result<GroupInfo> {
        // Deserialize the Welcome from the MlsMessageOut bytes
        let mls_msg = MlsMessageIn::tls_deserialize_exact(&welcome_msg.welcome_data)
            .map_err(|e| MlsError::Deserialization(e.to_string()))?;

        // Extract the Welcome from the message
        let welcome = match mls_msg.extract() {
            MlsMessageBodyIn::Welcome(w) => w,
            _ => return Err(MlsError::WelcomeProcessing("Not a Welcome message".to_string())),
        };

        let group = self.group_manager.join_group(welcome, &welcome_msg.group_id)?;
        let info = self.group_manager.get_group_info(&group, &welcome_msg.group_id);

        info!(
            session_id = %welcome_msg.group_id,
            inviter = %welcome_msg.inviter_id,
            "Joined 1:1 session"
        );

        Ok(info)
    }

    /// Encrypts a message for the session partner.
    pub fn encrypt_message(
        &self,
        other_user_id: &str,
        plaintext: &[u8],
        signer: &impl Signer,
    ) -> Result<EncryptedMessage> {
        let session_id = self.get_session_id(other_user_id);

        let mut group = self
            .group_manager
            .load_group(&session_id)?
            .ok_or_else(|| MlsError::SessionNotFound(other_user_id.to_string()))?;

        let mls_message = self.group_manager.encrypt_message(&mut group, plaintext, signer)?;

        self.group_manager.save_group(&session_id, &group)?;

        let ciphertext = mls_message
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        Ok(EncryptedMessage {
            group_id: session_id,
            message_type: MlsMessageType::Application,
            epoch: group.epoch().as_u64(),
            ciphertext,
            sender_id: self.user_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        })
    }

    /// Decrypts a message from a session.
    pub fn decrypt_message(&self, encrypted: &EncryptedMessage) -> Result<Option<Vec<u8>>> {
        let mut group = self
            .group_manager
            .load_group(&encrypted.group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(encrypted.group_id.to_string()))?;

        let mls_message = MlsMessageIn::tls_deserialize_exact(&encrypted.ciphertext)
            .map_err(|e| MlsError::Deserialization(e.to_string()))?;

        let result = self.group_manager.decrypt_message(&mut group, mls_message)?;

        self.group_manager.save_group(&encrypted.group_id, &group)?;

        Ok(result)
    }

    /// Lists all active sessions.
    pub fn list_sessions(&self) -> Result<Vec<String>> {
        let groups = self.group_manager.list_groups()?;
        let sessions: Vec<String> = groups
            .into_iter()
            .filter(|g| g.as_str().starts_with("session:"))
            .filter_map(|g| {
                let parts: Vec<&str> = g.as_str().split(':').collect();
                if parts.len() == 3 {
                    let other = if parts[1] == self.user_id {
                        parts[2]
                    } else {
                        parts[1]
                    };
                    Some(other.to_string())
                } else {
                    None
                }
            })
            .collect();

        Ok(sessions)
    }

    /// Deletes a session.
    pub fn delete_session(&self, other_user_id: &str) -> Result<()> {
        let session_id = self.get_session_id(other_user_id);
        self.group_manager.delete_group(&session_id)?;
        debug!(session_id = %session_id, "Deleted session");
        Ok(())
    }

    /// Gets information about a session.
    pub fn get_session_info(&self, other_user_id: &str) -> Result<Option<GroupInfo>> {
        let session_id = self.get_session_id(other_user_id);
        let group = self.group_manager.load_group(&session_id)?;
        Ok(group.map(|g| self.group_manager.get_group_info(&g, &session_id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryStorage;
    use crate::storage_adapter::MlsStorageAdapter;

    fn create_test_session_manager(user_id: &str) -> SessionManager {
        let storage = Arc::new(InMemoryStorage::new());
        let adapter = MlsStorageAdapter::new(storage.clone());
        let provider = MlsProvider::new(adapter);
        SessionManager::new(user_id.to_string(), storage, provider)
    }

    #[test]
    fn test_session_id_generation() {
        let manager = create_test_session_manager("alice");
        let id1 = manager.get_session_id("bob");
        let id2 = GroupId::for_session("bob", "alice");
        assert_eq!(id1, id2);
        assert!(id1.as_str().starts_with("session:"));
    }

    #[test]
    fn test_no_session_initially() {
        let manager = create_test_session_manager("alice");
        assert!(!manager.has_session("bob").unwrap());
        assert!(manager.list_sessions().unwrap().is_empty());
    }
}
