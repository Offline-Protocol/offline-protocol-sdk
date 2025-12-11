//! MLS group management.
//!
//! This module handles creation, modification, and state management of MLS groups.

use crate::error::{MlsError, Result};
use crate::storage::MlsStorage;
use crate::types::{GroupId, GroupInfo, StorageKeyType};

use openmls::prelude::*;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::signatures::Signer;
use std::sync::Arc;
use tracing::{debug, warn};

/// Default ciphersuite for MLS operations.
pub const DEFAULT_CIPHERSUITE: Ciphersuite =
    Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

/// Manages MLS groups for encrypted messaging.
pub struct GroupManager {
    /// User ID of the local user.
    #[allow(dead_code)]
    user_id: String,

    /// Storage backend for persisting group state.
    storage: Arc<dyn MlsStorage>,

    /// OpenMLS crypto provider.
    crypto: OpenMlsRustCrypto,
}

impl GroupManager {
    /// Creates a new group manager.
    pub fn new(user_id: String, storage: Arc<dyn MlsStorage>) -> Self {
        Self {
            user_id,
            storage,
            crypto: OpenMlsRustCrypto::default(),
        }
    }

    /// Returns a reference to the crypto provider.
    pub fn crypto(&self) -> &OpenMlsRustCrypto {
        &self.crypto
    }

    /// Creates a new MLS group.
    pub fn create_group(
        &self,
        group_id: &GroupId,
        credential_with_key: &CredentialWithKey,
        signer: &impl Signer,
    ) -> Result<MlsGroup> {
        let group_config = MlsGroupCreateConfig::builder()
            .ciphersuite(DEFAULT_CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .build();

        let mls_group_id = openmls::group::GroupId::from_slice(group_id.as_str().as_bytes());

        let group = MlsGroup::new_with_group_id(
            &self.crypto,
            signer,
            &group_config,
            mls_group_id,
            credential_with_key.clone(),
        )
        .map_err(|e| MlsError::GroupCreation(e.to_string()))?;

        // Persist group state
        self.save_group(group_id, &group)?;

        debug!(group_id = %group_id, "Created new MLS group");
        Ok(group)
    }

    /// Loads a group from storage.
    pub fn load_group(&self, group_id: &GroupId) -> Result<Option<MlsGroup>> {
        let key_type = StorageKeyType::GroupState.as_str();
        let data = self.storage.load(key_type, group_id.as_str())?;

        match data {
            Some(_bytes) => {
                // MlsGroup in OpenMLS 0.7 uses the storage provider pattern
                let mls_group_id = openmls::group::GroupId::from_slice(group_id.as_str().as_bytes());
                
                // Try to load from the crypto provider's storage
                match MlsGroup::load(self.crypto.storage(), &mls_group_id) {
                    Ok(Some(group)) => Ok(Some(group)),
                    Ok(None) => {
                        debug!(group_id = %group_id, "Group not found in crypto storage");
                        Ok(None)
                    }
                    Err(e) => Err(MlsError::Deserialization(format!("Failed to load group: {:?}", e))),
                }
            }
            None => Ok(None),
        }
    }

    /// Saves a group to storage.
    pub fn save_group(&self, group_id: &GroupId, group: &MlsGroup) -> Result<()> {
        // In OpenMLS 0.7, groups are stored via the storage provider
        // The group is automatically persisted to the crypto provider's storage
        // We also keep a marker in our storage for listing purposes
        let key_type = StorageKeyType::GroupState.as_str();
        
        // Serialize the group epoch as a marker
        let marker = group.epoch().as_u64().to_le_bytes();
        self.storage.store(key_type, group_id.as_str(), &marker)?;
        Ok(())
    }

    /// Deletes a group from storage.
    pub fn delete_group(&self, group_id: &GroupId) -> Result<()> {
        let key_type = StorageKeyType::GroupState.as_str();
        self.storage.delete(key_type, group_id.as_str())?;
        Ok(())
    }

    /// Lists all groups.
    pub fn list_groups(&self) -> Result<Vec<GroupId>> {
        let key_type = StorageKeyType::GroupState.as_str();
        let keys = self.storage.list_keys(key_type)?;
        Ok(keys.into_iter().map(GroupId::new).collect())
    }

    /// Adds a member to a group using their key package.
    pub fn add_member(
        &self,
        group: &mut MlsGroup,
        key_package: KeyPackage,
        signer: &impl Signer,
    ) -> Result<(MlsMessageOut, MlsMessageOut)> {
        let (commit, welcome, _group_info) = group
            .add_members(&self.crypto, signer, &[key_package])
            .map_err(|e| MlsError::AddMember(e.to_string()))?;

        // Merge the pending commit
        group
            .merge_pending_commit(&self.crypto)
            .map_err(|e| MlsError::AddMember(format!("Failed to merge commit: {}", e)))?;

        Ok((commit, welcome))
    }

    /// Removes a member from a group.
    pub fn remove_member(
        &self,
        group: &mut MlsGroup,
        member_index: LeafNodeIndex,
        signer: &impl Signer,
    ) -> Result<MlsMessageOut> {
        let (commit, _welcome, _group_info) = group
            .remove_members(&self.crypto, signer, &[member_index])
            .map_err(|e| MlsError::RemoveMember(e.to_string()))?;

        group
            .merge_pending_commit(&self.crypto)
            .map_err(|e| MlsError::RemoveMember(format!("Failed to merge commit: {}", e)))?;

        Ok(commit)
    }

    /// Creates an encrypted application message.
    pub fn encrypt_message(
        &self,
        group: &mut MlsGroup,
        plaintext: &[u8],
        signer: &impl Signer,
    ) -> Result<MlsMessageOut> {
        let message = group
            .create_message(&self.crypto, signer, plaintext)
            .map_err(|e| MlsError::Encryption(e.to_string()))?;

        Ok(message)
    }

    /// Decrypts an incoming MLS message.
    pub fn decrypt_message(
        &self,
        group: &mut MlsGroup,
        message: MlsMessageIn,
    ) -> Result<Option<Vec<u8>>> {
        let protocol_message = message
            .try_into_protocol_message()
            .map_err(|e| MlsError::Decryption(format!("Invalid protocol message: {:?}", e)))?;

        let processed = group
            .process_message(&self.crypto, protocol_message)
            .map_err(|e| MlsError::Decryption(e.to_string()))?;

        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(app_msg) => {
                Ok(Some(app_msg.into_bytes()))
            }
            ProcessedMessageContent::ProposalMessage(_) => {
                debug!("Received proposal message");
                Ok(None)
            }
            ProcessedMessageContent::StagedCommitMessage(staged_commit) => {
                debug!("Received commit message, merging");
                group
                    .merge_staged_commit(&self.crypto, *staged_commit)
                    .map_err(|e| MlsError::Decryption(format!("Failed to merge commit: {}", e)))?;
                Ok(None)
            }
            ProcessedMessageContent::ExternalJoinProposalMessage(_) => {
                warn!("Received external join proposal (not supported)");
                Ok(None)
            }
        }
    }

    /// Joins a group using a Welcome message.
    pub fn join_group(
        &self,
        welcome: Welcome,
        group_id: &GroupId,
    ) -> Result<MlsGroup> {
        let group_config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .build();

        let group = StagedWelcome::new_from_welcome(&self.crypto, &group_config, welcome, None)
            .map_err(|e| MlsError::WelcomeProcessing(format!("Failed to stage welcome: {}", e)))?
            .into_group(&self.crypto)
            .map_err(|e| MlsError::WelcomeProcessing(format!("Failed to join group: {}", e)))?;

        self.save_group(group_id, &group)?;

        debug!(group_id = %group_id, "Joined group via Welcome");
        Ok(group)
    }

    /// Gets information about a group.
    pub fn get_group_info(&self, group: &MlsGroup, group_id: &GroupId) -> GroupInfo {
        let members: Vec<String> = group
            .members()
            .filter_map(|m| {
                let credential = m.credential.serialized_content();
                String::from_utf8(credential.to_vec()).ok()
            })
            .collect();

        let is_session = group_id.as_str().starts_with("session:");

        GroupInfo {
            group_id: group_id.clone(),
            name: if is_session { None } else { Some(group_id.as_str().to_string()) },
            members,
            epoch: group.epoch().as_u64(),
            is_session,
            created_at_ms: 0,
            last_activity_ms: chrono::Utc::now().timestamp_millis() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryStorage;

    fn create_test_group_manager() -> GroupManager {
        let storage = Arc::new(InMemoryStorage::new());
        GroupManager::new("test_user".to_string(), storage)
    }

    #[test]
    fn test_group_manager_creation() {
        let manager = create_test_group_manager();
        assert!(manager.list_groups().unwrap().is_empty());
    }
}
