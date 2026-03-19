//! Main MLS manager - the primary interface for MLS operations.

use crate::error::{MlsError, Result};
use crate::group::{GroupManager, DEFAULT_CIPHERSUITE};
use crate::provider::MlsProvider;
use crate::session::SessionManager;
use crate::storage::MlsStorage;
use crate::storage_adapter::MlsStorageAdapter;
use crate::types::{
    EncryptedMessage, GroupId, GroupInfo, GroupMetadata, GroupRole, KeyPackageBundle,
    MlsMessageType, StorageKeyType, WelcomeMessage,
};

use openmls::prelude::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::OpenMlsProvider;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Default lifetime for key packages (30 days in seconds).
const DEFAULT_KEY_PACKAGE_LIFETIME_SECS: u64 = 30 * 24 * 60 * 60;

/// Main MLS manager for end-to-end encryption.
pub struct MlsManager {
    /// The local user's ID.
    user_id: String,

    /// Storage backend for persisting MLS state.
    storage: Arc<dyn MlsStorage>,

    /// OpenMLS provider.
    provider: MlsProvider,

    /// Cached credential.
    credential: RwLock<Option<CredentialWithKey>>,

    /// Cached signature key pair (avoids re-reading storage on every crypto op).
    cached_signer: RwLock<Option<SignatureKeyPair>>,

    /// Session manager for 1:1 messaging.
    session_manager: SessionManager,

    /// Group manager for multi-party groups.
    group_manager: GroupManager,
}

impl MlsManager {
    /// Creates a new MLS manager.
    pub fn new(user_id: impl Into<String>, storage: Arc<dyn MlsStorage>) -> Result<Self> {
        let user_id = user_id.into();

        let adapter = MlsStorageAdapter::new(storage.clone());
        let provider = MlsProvider::new(adapter);

        let session_manager =
            SessionManager::new(user_id.clone(), storage.clone(), provider.clone());
        let group_manager = GroupManager::new(storage.clone(), provider.clone());

        let manager = Self {
            user_id,
            storage,
            provider,
            credential: RwLock::new(None),
            cached_signer: RwLock::new(None),
            session_manager,
            group_manager,
        };

        manager.ensure_identity()?;

        Ok(manager)
    }

    /// Returns the local user's ID.
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// Ensures the user has an identity.
    fn ensure_identity(&self) -> Result<()> {
        if self.load_identity()? {
            return Ok(());
        }
        self.create_identity()?;
        info!(user_id = %self.user_id, "Created new MLS identity");
        Ok(())
    }

    /// Loads the identity from storage.
    fn load_identity(&self) -> Result<bool> {
        let key_type = StorageKeyType::Identity.as_str();
        let keys_data = self.storage.load(key_type, "key_pair")?;

        match keys_data {
            Some(json) => {
                let signature_keys: SignatureKeyPair =
                    serde_json::from_slice(&json).map_err(|e| {
                        MlsError::Deserialization(format!(
                            "Failed to deserialize signature keys: {}",
                            e
                        ))
                    })?;

                let public_key = signature_keys.public();

                let credential =
                    Credential::new(CredentialType::Basic, self.user_id.as_bytes().to_vec());

                let credential_with_key = CredentialWithKey {
                    credential,
                    signature_key: public_key.into(),
                };

                {
                    let mut guard = self
                        .credential
                        .write()
                        .map_err(|_| MlsError::NotInitialized)?;
                    *guard = Some(credential_with_key);
                }
                {
                    let mut guard = self
                        .cached_signer
                        .write()
                        .map_err(|_| MlsError::NotInitialized)?;
                    *guard = Some(signature_keys);
                }

                debug!(user_id = %self.user_id, "Loaded existing MLS identity");
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Creates a new identity.
    fn create_identity(&self) -> Result<()> {
        let signature_keys = SignatureKeyPair::new(DEFAULT_CIPHERSUITE.signature_algorithm())
            .map_err(|e| MlsError::CryptoGeneration(format!("{:?}", e)))?;

        let keys_json = serde_json::to_vec(&signature_keys)
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        let key_type = StorageKeyType::Identity.as_str();
        self.storage.store(key_type, "key_pair", &keys_json)?;

        let public_key = signature_keys.public();

        let credential = Credential::new(CredentialType::Basic, self.user_id.as_bytes().to_vec());

        let credential_with_key = CredentialWithKey {
            credential,
            signature_key: public_key.into(),
        };

        {
            let mut guard = self
                .credential
                .write()
                .map_err(|_| MlsError::NotInitialized)?;
            *guard = Some(credential_with_key);
        }
        {
            let mut guard = self
                .cached_signer
                .write()
                .map_err(|_| MlsError::NotInitialized)?;
            *guard = Some(signature_keys);
        }

        Ok(())
    }

    /// Gets the credential with key.
    fn get_credential(&self) -> Result<CredentialWithKey> {
        let guard = self
            .credential
            .read()
            .map_err(|_| MlsError::NotInitialized)?;
        guard.clone().ok_or(MlsError::NotInitialized)
    }

    /// Gets a signer for MLS operations, using the in-memory cache.
    ///
    /// `SignatureKeyPair` doesn't implement `Clone`, so we serialize from
    /// the cached copy to avoid hitting storage on every crypto operation.
    fn get_signer(&self) -> Result<SignatureKeyPair> {
        let guard = self
            .cached_signer
            .read()
            .map_err(|_| MlsError::NotInitialized)?;

        let cached = guard.as_ref().ok_or(MlsError::NotInitialized)?;
        let bytes =
            serde_json::to_vec(cached).map_err(|e| MlsError::Serialization(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| MlsError::Deserialization(e.to_string()))
    }

    // ========================================================================
    // KEY PACKAGE MANAGEMENT
    // ========================================================================

    /// Generates a new key package for distribution.
    pub fn generate_key_package(&self) -> Result<KeyPackageBundle> {
        let credential = self.get_credential()?;
        let signature_keys = self.get_signer()?;

        let key_package_bundle = KeyPackage::builder()
            .build(
                DEFAULT_CIPHERSUITE,
                &self.provider,
                &signature_keys,
                credential,
            )
            .map_err(|e| MlsError::KeyPackageCreation(e.to_string()))?;

        let key_package_data = key_package_bundle
            .key_package()
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        let package_id = Uuid::new_v4().to_string();

        let key_type = StorageKeyType::KeyPackage.as_str();
        let bundle = KeyPackageBundle::new(
            package_id,
            self.user_id.clone(),
            key_package_data,
            DEFAULT_KEY_PACKAGE_LIFETIME_SECS,
        );
        let serialized =
            serde_json::to_vec(&bundle).map_err(|e| MlsError::Serialization(e.to_string()))?;
        self.storage
            .store(key_type, &bundle.package_id, &serialized)?;

        debug!(package_id = %bundle.package_id, "Generated new key package");
        Ok(bundle)
    }

    /// Gets an existing key package or generates a new one.
    pub fn get_or_create_key_package(&self) -> Result<KeyPackageBundle> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let packages = self.storage.list_keys(key_type)?;

        for package_id in packages {
            if let Some(bundle) = self.load_stored_key_package(&package_id)? {
                return Ok(bundle);
            }
        }

        self.generate_key_package()
    }

    /// Imports a contact's key package for later use.
    pub fn import_key_package(&self, user_id: &str, key_package_data: &[u8]) -> Result<()> {
        // Validate by attempting to deserialize
        let _key_package = KeyPackageIn::tls_deserialize_exact(key_package_data)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;

        let key_type = StorageKeyType::ContactKeyPackage.as_str();
        self.storage.store(key_type, user_id, key_package_data)?;

        debug!(user_id = %user_id, "Imported key package for contact");
        Ok(())
    }

    /// Gets a contact's key package.
    fn get_contact_key_package(&self, user_id: &str) -> Result<KeyPackage> {
        let key_type = StorageKeyType::ContactKeyPackage.as_str();
        let data = self
            .storage
            .load(key_type, user_id)?
            .ok_or_else(|| MlsError::NoKeyPackage(user_id.to_string()))?;

        let key_package_in = KeyPackageIn::tls_deserialize_exact(&data)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;

        // Validate using the crypto backend
        key_package_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))
    }

    /// Gets pending key packages.
    pub fn get_pending_key_packages(&self) -> Result<Vec<KeyPackageBundle>> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let package_ids = self.storage.list_keys(key_type)?;

        let mut bundles = Vec::new();
        for package_id in package_ids {
            if let Some(bundle) = self.load_stored_key_package(&package_id)? {
                bundles.push(bundle);
            }
        }

        Ok(bundles)
    }

    /// Marks a key package as synced.
    pub fn mark_key_package_synced(&self, package_id: &str) -> Result<()> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        self.storage.delete(key_type, package_id)?;
        debug!(package_id = %package_id, "Marked key package as synced");
        Ok(())
    }

    // ========================================================================
    // 1:1 ENCRYPTED MESSAGING
    // ========================================================================

    /// Checks if a session exists with another user.
    pub fn has_session(&self, other_user_id: &str) -> Result<bool> {
        self.session_manager.has_session(other_user_id)
    }

    /// Creates a new 1:1 session with another user.
    pub fn create_session(&self, other_user_id: &str) -> Result<WelcomeMessage> {
        let their_key_package = self.get_contact_key_package(other_user_id)?;
        let credential = self.get_credential()?;
        let signature_keys = self.get_signer()?;

        let welcome = self.session_manager.create_session(
            other_user_id,
            their_key_package,
            &credential,
            &signature_keys,
        )?;

        let key_type = StorageKeyType::ContactKeyPackage.as_str();
        self.storage.delete(key_type, other_user_id)?;

        Ok(welcome)
    }

    /// Joins a session using a Welcome message.
    pub fn join_session(&self, welcome: &WelcomeMessage) -> Result<GroupInfo> {
        self.session_manager.join_session(welcome)
    }

    /// Replaces an existing session with an incoming Welcome message.
    ///
    /// This implements the "welcome-wins" strategy for race condition resolution.
    /// When both peers simultaneously create a session, this method allows one peer
    /// to replace their own session with the other peer's Welcome, ensuring both
    /// end up with the same cryptographic state.
    pub fn replace_session_with_welcome(&self, welcome: &WelcomeMessage) -> Result<GroupInfo> {
        let other_user_id = &welcome.inviter_id;

        // Clear any pending welcome we were about to send
        let _ = self.clear_pending_welcome(other_user_id);

        // Delete conflicting contact key package (we no longer need it)
        let key_type = StorageKeyType::ContactKeyPackage.as_str();
        let _ = self.storage.delete(key_type, other_user_id);

        // Join using their Welcome (session.join_session handles deleting our existing session)
        self.session_manager.join_session(welcome)
    }

    /// Encrypts a message for a 1:1 session.
    pub fn encrypt_for_user(
        &self,
        other_user_id: &str,
        plaintext: &[u8],
    ) -> Result<EncryptedMessage> {
        if !self.has_session(other_user_id)? {
            let welcome = self.create_session(other_user_id)?;
            warn!(
                other_user_id = %other_user_id,
                "Created new session - Welcome message needs to be sent"
            );
            let key_type = StorageKeyType::PendingWelcome.as_str();
            let welcome_data =
                serde_json::to_vec(&welcome).map_err(|e| MlsError::Serialization(e.to_string()))?;
            self.storage.store(key_type, other_user_id, &welcome_data)?;
        }

        let signature_keys = self.get_signer()?;
        self.session_manager
            .encrypt_message(other_user_id, plaintext, &signature_keys)
    }

    /// Encrypts a message for a 1:1 session that is known to exist.
    ///
    /// Unlike `encrypt_for_user`, this skips the `has_session()` storage check
    /// and goes directly to `encrypt_message()`. The caller must guarantee the
    /// session exists (e.g., via an in-memory cache). If the session was deleted
    /// externally, `SessionNotFound` is returned.
    pub fn encrypt_for_existing_session(
        &self,
        other_user_id: &str,
        plaintext: &[u8],
    ) -> Result<EncryptedMessage> {
        let signature_keys = self.get_signer()?;
        self.session_manager
            .encrypt_message(other_user_id, plaintext, &signature_keys)
    }

    /// Gets a pending Welcome message.
    pub fn get_pending_welcome(&self, other_user_id: &str) -> Result<Option<WelcomeMessage>> {
        let key_type = StorageKeyType::PendingWelcome.as_str();
        match self.storage.load(key_type, other_user_id)? {
            Some(data) => {
                let welcome: WelcomeMessage = serde_json::from_slice(&data)
                    .map_err(|e| MlsError::Deserialization(e.to_string()))?;
                Ok(Some(welcome))
            }
            None => Ok(None),
        }
    }

    /// Clears a pending Welcome message.
    pub fn clear_pending_welcome(&self, other_user_id: &str) -> Result<()> {
        let key_type = StorageKeyType::PendingWelcome.as_str();
        self.storage.delete(key_type, other_user_id)?;
        Ok(())
    }

    /// Decrypts a message from a 1:1 session.
    pub fn decrypt_from_user(&self, encrypted: &EncryptedMessage) -> Result<Option<Vec<u8>>> {
        self.session_manager.decrypt_message(encrypted)
    }

    /// Lists all active 1:1 sessions.
    pub fn list_sessions(&self) -> Result<Vec<String>> {
        self.session_manager.list_sessions()
    }

    /// Deletes a 1:1 session.
    pub fn delete_session(&self, other_user_id: &str) -> Result<()> {
        self.session_manager.delete_session(other_user_id)
    }

    // ========================================================================
    // GROUP MESSAGING
    // ========================================================================

    /// Creates a new group.
    pub fn create_group(&self, group_name: &str) -> Result<GroupInfo> {
        let group_id = GroupId::new(format!("group:{}", Uuid::new_v4()));
        let credential = self.get_credential()?;
        let signature_keys = self.get_signer()?;

        let group = self
            .group_manager
            .create_group(&group_id, &credential, &signature_keys)?;

        // Store group metadata with creator as admin
        let metadata = GroupMetadata::new_with_creator(Some(group_name.to_string()), &self.user_id);
        self.save_group_metadata(&group_id, &metadata)?;

        let mut info = self.group_manager.get_group_info(&group, &group_id);
        info.name = metadata.name;
        info.created_at_ms = metadata.created_at_ms;
        info.last_activity_ms = metadata.last_activity_ms;

        info!(group_id = %group_id, name = %group_name, "Created new group");
        Ok(info)
    }

    /// Adds a member to a group.
    ///
    /// Returns a tuple of (WelcomeMessage, EncryptedMessage) where the
    /// WelcomeMessage should be sent to the invitee and the EncryptedMessage
    /// (Commit) should be distributed to all existing group members so they
    /// can advance their MLS epoch.
    pub fn add_group_member(
        &self,
        group_id: &GroupId,
        member_key_package: &[u8],
    ) -> Result<(WelcomeMessage, EncryptedMessage)> {
        let key_package = KeyPackageIn::tls_deserialize_exact(member_key_package)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;

        let mut group = self
            .group_manager
            .load_group(group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(group_id.to_string()))?;

        let signature_keys = self.get_signer()?;
        let (commit, welcome) =
            self.group_manager
                .add_member(&mut group, key_package, &signature_keys)?;

        self.group_manager.save_group(group_id, &group)?;

        let welcome_bytes = welcome
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        let commit_bytes = commit
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        // Include group name in welcome for the invitee
        let group_name = self.load_group_metadata(group_id)?.and_then(|m| m.name);

        let now_ms = chrono::Utc::now().timestamp_millis() as u64;

        let welcome_msg = WelcomeMessage {
            group_id: group_id.clone(),
            welcome_data: welcome_bytes,
            inviter_id: self.user_id.clone(),
            group_name,
            timestamp_ms: now_ms,
        };

        let commit_msg = EncryptedMessage {
            group_id: group_id.clone(),
            message_type: MlsMessageType::Commit,
            epoch: group.epoch().as_u64(),
            ciphertext: commit_bytes,
            sender_id: self.user_id.clone(),
            timestamp_ms: now_ms,
        };

        Ok((welcome_msg, commit_msg))
    }

    /// Removes a member from a group.
    pub fn remove_group_member(
        &self,
        group_id: &GroupId,
        member_id: &str,
    ) -> Result<EncryptedMessage> {
        let mut group = self
            .group_manager
            .load_group(group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(group_id.to_string()))?;

        let member_index = group
            .members()
            .find_map(|m| {
                let cred_data = m.credential.serialized_content();
                if cred_data == member_id.as_bytes() {
                    Some(m.index)
                } else {
                    None
                }
            })
            .ok_or_else(|| MlsError::UserNotInGroup(member_id.to_string()))?;

        let signature_keys = self.get_signer()?;
        let commit = self
            .group_manager
            .remove_member(&mut group, member_index, &signature_keys)?;

        self.group_manager.save_group(group_id, &group)?;

        let ciphertext = commit
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        Ok(EncryptedMessage {
            group_id: group_id.clone(),
            message_type: MlsMessageType::Commit,
            epoch: group.epoch().as_u64(),
            ciphertext,
            sender_id: self.user_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        })
    }

    /// Leaves a group.
    pub fn leave_group(&self, group_id: &GroupId) -> Result<()> {
        self.group_manager.delete_group(group_id)?;
        info!(group_id = %group_id, "Left group");
        Ok(())
    }

    /// Encrypts a message for a group.
    pub fn encrypt_for_group(
        &self,
        group_id: &GroupId,
        plaintext: &[u8],
    ) -> Result<EncryptedMessage> {
        let mut group = self
            .group_manager
            .load_group(group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(group_id.to_string()))?;

        let signature_keys = self.get_signer()?;
        let mls_message =
            self.group_manager
                .encrypt_message(&mut group, plaintext, &signature_keys)?;

        self.group_manager.save_group(group_id, &group)?;

        let ciphertext = mls_message
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        Ok(EncryptedMessage {
            group_id: group_id.clone(),
            message_type: MlsMessageType::Application,
            epoch: group.epoch().as_u64(),
            ciphertext,
            sender_id: self.user_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        })
    }

    /// Decrypts a message from a group.
    pub fn decrypt_from_group(&self, encrypted: &EncryptedMessage) -> Result<Option<Vec<u8>>> {
        let mut group = self
            .group_manager
            .load_group(&encrypted.group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(encrypted.group_id.to_string()))?;

        let mls_message = MlsMessageIn::tls_deserialize_exact(&encrypted.ciphertext)
            .map_err(|e| MlsError::Deserialization(e.to_string()))?;

        let result = self
            .group_manager
            .decrypt_message(&mut group, mls_message)?;

        self.group_manager.save_group(&encrypted.group_id, &group)?;

        Ok(result)
    }

    /// Joins a group using a Welcome message.
    pub fn join_group(&self, welcome: &WelcomeMessage) -> Result<GroupInfo> {
        let mls_msg = MlsMessageIn::tls_deserialize_exact(&welcome.welcome_data)
            .map_err(|e| MlsError::Deserialization(e.to_string()))?;

        let welcome_msg = match mls_msg.extract() {
            MlsMessageBodyIn::Welcome(w) => w,
            _ => {
                return Err(MlsError::WelcomeProcessing(
                    "Not a Welcome message".to_string(),
                ))
            }
        };

        let group = self
            .group_manager
            .join_group(welcome_msg, &welcome.group_id)?;
        let mut info = self.group_manager.get_group_info(&group, &welcome.group_id);
        info.name = welcome.group_name.clone();

        info!(group_id = %welcome.group_id, "Joined group");
        Ok(info)
    }

    /// Lists all groups.
    pub fn list_groups(&self) -> Result<Vec<GroupId>> {
        let all_groups = self.group_manager.list_groups()?;
        Ok(all_groups
            .into_iter()
            .filter(|g| !g.as_str().starts_with("session:"))
            .collect())
    }

    /// Gets information about a group.
    pub fn get_group_info(&self, group_id: &GroupId) -> Result<Option<GroupInfo>> {
        let group = match self.group_manager.load_group(group_id)? {
            Some(g) => g,
            None => return Ok(None),
        };

        let mut info = self.group_manager.get_group_info(&group, group_id);

        // Merge stored metadata
        if let Some(metadata) = self.load_group_metadata(group_id)? {
            info.name = metadata.name;
            info.created_at_ms = metadata.created_at_ms;
            info.last_activity_ms = metadata.last_activity_ms;
        }

        Ok(Some(info))
    }

    /// Updates the group name.
    pub fn set_group_name(&self, group_id: &GroupId, name: &str) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        metadata.name = Some(name.to_string());
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    /// Gets group metadata.
    pub fn get_group_metadata(&self, group_id: &GroupId) -> Result<Option<GroupMetadata>> {
        self.load_group_metadata(group_id)
    }

    /// Sets custom metadata for a group.
    pub fn set_group_custom_metadata(
        &self,
        group_id: &GroupId,
        key: &str,
        value: &str,
    ) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        metadata.custom.insert(key.to_string(), value.to_string());
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    /// Removes a custom metadata key for a group.
    pub fn remove_group_custom_metadata(&self, group_id: &GroupId, key: &str) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        metadata.custom.remove(key);
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    /// Sets a member's role in a group.
    pub fn set_member_role(
        &self,
        group_id: &GroupId,
        user_id: &str,
        role: GroupRole,
    ) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        metadata.set_role(user_id, role);
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    /// Removes a member's role metadata from a group.
    pub fn remove_member_role(&self, group_id: &GroupId, user_id: &str) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        metadata.remove_role(user_id);
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    // ========================================================================
    // GENERIC MESSAGE HANDLING
    // ========================================================================

    /// Decrypts any incoming encrypted message.
    pub fn decrypt(&self, encrypted: &EncryptedMessage) -> Result<Option<Vec<u8>>> {
        if encrypted.group_id.as_str().starts_with("session:") {
            self.decrypt_from_user(encrypted)
        } else {
            self.decrypt_from_group(encrypted)
        }
    }

    /// Processes a Welcome message.
    pub fn process_welcome(&self, welcome: &WelcomeMessage) -> Result<GroupInfo> {
        if welcome.group_id.as_str().starts_with("session:") {
            self.join_session(welcome)
        } else {
            self.join_group(welcome)
        }
    }
}

impl MlsManager {
    /// Loads a stored key package bundle, handling legacy raw storage and expiration.
    ///
    /// Also validates that the key package's private key still exists in the
    /// OpenMLS provider storage. Key packages whose private keys have been
    /// consumed (e.g. by a previous Welcome processing) are pruned.
    fn load_stored_key_package(&self, package_id: &str) -> Result<Option<KeyPackageBundle>> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let data = match self.storage.load(key_type, package_id)? {
            Some(data) => data,
            None => return Ok(None),
        };

        let bundle = match serde_json::from_slice::<KeyPackageBundle>(&data) {
            Ok(bundle) => bundle,
            Err(_) => {
                let legacy_bundle = KeyPackageBundle::new(
                    package_id.to_string(),
                    self.user_id.clone(),
                    data,
                    DEFAULT_KEY_PACKAGE_LIFETIME_SECS,
                );
                let serialized = serde_json::to_vec(&legacy_bundle)
                    .map_err(|e| MlsError::Serialization(e.to_string()))?;
                self.storage.store(key_type, package_id, &serialized)?;
                legacy_bundle
            }
        };

        if bundle.is_expired() {
            self.storage.delete(key_type, package_id)?;
            return Ok(None);
        }

        if !self.is_key_package_usable(&bundle.key_package_data) {
            warn!(
                package_id = %package_id,
                "Key package private key no longer in provider storage, pruning stale entry"
            );
            self.storage.delete(key_type, package_id)?;
            return Ok(None);
        }

        let serialized =
            serde_json::to_vec(&bundle).map_err(|e| MlsError::Serialization(e.to_string()))?;
        self.storage.store(key_type, package_id, &serialized)?;

        Ok(Some(bundle))
    }

    /// Checks whether a key package's private init key is still present in
    /// the OpenMLS provider storage. Returns `false` when the key material
    /// has been consumed (e.g. by processing a Welcome) or was never stored.
    fn is_key_package_usable(&self, key_package_data: &[u8]) -> bool {
        let kp_in = match KeyPackageIn::tls_deserialize_exact(key_package_data) {
            Ok(kp) => kp,
            Err(_) => return false,
        };
        let kp: KeyPackage = match kp_in.validate(self.provider.crypto(), ProtocolVersion::Mls10) {
            Ok(kp) => kp,
            Err(_) => return false,
        };
        let hash_ref = match kp.hash_ref(self.provider.crypto()) {
            Ok(hr) => hr,
            Err(_) => return false,
        };
        use openmls_traits::storage::StorageProvider;
        let found: std::result::Result<Option<openmls::key_packages::KeyPackageBundle>, _> =
            self.provider.storage().key_package(&hash_ref);
        matches!(found, Ok(Some(_)))
    }

    /// Loads group metadata from storage.
    fn load_group_metadata(&self, group_id: &GroupId) -> Result<Option<GroupMetadata>> {
        let key_type = StorageKeyType::GroupMetadata.as_str();
        match self.storage.load(key_type, group_id.as_str())? {
            Some(data) => {
                let mut metadata: GroupMetadata = serde_json::from_slice(&data)
                    .map_err(|e| MlsError::Deserialization(e.to_string()))?;
                // Migrate legacy "role:*" keys from `custom` into `roles`
                if metadata.roles.is_empty()
                    && metadata
                        .custom
                        .keys()
                        .any(|k| k.starts_with(GroupMetadata::LEGACY_ROLE_KEY_PREFIX))
                {
                    metadata.migrate_legacy_roles();
                    // Persist the migration so it only runs once
                    if let Err(e) = self.save_group_metadata(group_id, &metadata) {
                        warn!(group_id = %group_id.as_str(), error = %e, "Failed to persist legacy role migration");
                    }
                }
                Ok(Some(metadata))
            }
            None => Ok(None),
        }
    }

    /// Saves group metadata to storage.
    fn save_group_metadata(&self, group_id: &GroupId, metadata: &GroupMetadata) -> Result<()> {
        let key_type = StorageKeyType::GroupMetadata.as_str();
        let data =
            serde_json::to_vec(metadata).map_err(|e| MlsError::Serialization(e.to_string()))?;
        self.storage.store(key_type, group_id.as_str(), &data)?;
        Ok(())
    }
}

// ============================================================================
// KEY ROTATION AND KEY PACKAGE MANAGEMENT
// ============================================================================

impl MlsManager {
    /// Updates the cryptographic keys for a group (triggers MLS self-update).
    ///
    /// This provides post-compromise security by rotating keys. The returned
    /// commit message must be sent to all other group members.
    ///
    /// # Returns
    ///
    /// Returns the commit message that must be distributed to all group members.
    pub fn update_keys(&self, group_id: &GroupId) -> Result<EncryptedMessage> {
        use openmls::treesync::LeafNodeParameters;

        let mut group = self
            .group_manager
            .load_group(group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(group_id.to_string()))?;

        let signature_keys = self.get_signer()?;

        let bundle = group
            .self_update(
                &self.provider,
                &signature_keys,
                LeafNodeParameters::default(),
            )
            .map_err(|e| MlsError::OpenMls(format!("Self-update failed: {}", e)))?;

        let (commit, _welcome, _group_info) = bundle.into_contents();

        group
            .merge_pending_commit(&self.provider)
            .map_err(|e| MlsError::OpenMls(format!("Failed to merge self-update commit: {}", e)))?;

        self.group_manager.save_group(group_id, &group)?;

        // Update metadata last activity
        if let Some(mut metadata) = self.load_group_metadata(group_id)? {
            metadata.touch();
            self.save_group_metadata(group_id, &metadata)?;
        }

        let ciphertext = commit
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        debug!(group_id = %group_id, epoch = %group.epoch().as_u64(), "Updated group keys");

        Ok(EncryptedMessage {
            group_id: group_id.clone(),
            message_type: MlsMessageType::Commit,
            epoch: group.epoch().as_u64(),
            ciphertext,
            sender_id: self.user_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        })
    }

    /// Ensures at least `min` valid key packages are available.
    ///
    /// Generates new key packages if the current count is below the minimum.
    /// This is useful for offline scenarios where multiple key packages should
    /// be pre-generated for distribution.
    ///
    /// # Arguments
    ///
    /// * `min` - Minimum number of key packages to maintain
    ///
    /// # Returns
    ///
    /// Returns the total number of valid key packages after ensuring minimum.
    pub fn ensure_min_key_packages(&self, min: usize) -> Result<usize> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let package_ids = self.storage.list_keys(key_type)?;

        // Count valid (non-expired) packages
        let mut valid_count = 0;
        for package_id in &package_ids {
            if self.load_stored_key_package(package_id)?.is_some() {
                valid_count += 1;
            }
        }

        // Generate more if needed
        let to_generate = min.saturating_sub(valid_count);
        for _ in 0..to_generate {
            self.generate_key_package()?;
            valid_count += 1;
        }

        debug!(
            valid_count = valid_count,
            generated = to_generate,
            "Ensured minimum key packages"
        );

        Ok(valid_count)
    }

    /// Returns the number of valid (non-expired) key packages available.
    pub fn count_valid_key_packages(&self) -> Result<usize> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let package_ids = self.storage.list_keys(key_type)?;

        let mut count = 0;
        for package_id in package_ids {
            if self.load_stored_key_package(&package_id)?.is_some() {
                count += 1;
            }
        }

        Ok(count)
    }

    // ========================================================================
    // IDENTITY AND SIGNING OPERATIONS
    // ========================================================================

    /// Returns the identity public key as raw bytes.
    ///
    /// This is the Ed25519 public key used for MLS operations. It can be shared
    /// with others to establish your identity and verify signatures.
    pub fn get_identity_public_key(&self) -> Result<Vec<u8>> {
        let credential = self.get_credential()?;
        Ok(credential.signature_key.as_slice().to_vec())
    }

    /// Signs arbitrary data with the identity private key.
    ///
    /// Uses Ed25519 signatures (the same algorithm used for MLS operations).
    /// The signature can be verified by anyone with the corresponding public key.
    ///
    /// # Arguments
    ///
    /// * `data` - The data to sign
    ///
    /// # Returns
    ///
    /// Returns the signature as raw bytes.
    pub fn sign_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        use openmls_traits::signatures::Signer;

        let signer = self.get_signer()?;
        let signature = signer
            .sign(data)
            .map_err(|e| MlsError::Signing(format!("Failed to sign data: {:?}", e)))?;
        Ok(signature.as_slice().to_vec())
    }

    /// Verifies a signature against a public key.
    ///
    /// # Arguments
    ///
    /// * `public_key` - The Ed25519 public key bytes (32 bytes)
    /// * `data` - The original data that was signed
    /// * `signature` - The signature to verify (64 bytes)
    ///
    /// # Returns
    ///
    /// Returns `true` if the signature is valid, `false` otherwise.
    pub fn verify_signature(public_key: &[u8], data: &[u8], signature: &[u8]) -> Result<bool> {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};

        // Parse the public key (Ed25519 public keys are 32 bytes)
        let verifying_key = VerifyingKey::try_from(public_key).map_err(|e| {
            MlsError::InvalidPublicKey(format!("Invalid Ed25519 public key: {}", e))
        })?;

        // Parse the signature (Ed25519 signatures are 64 bytes)
        let sig = Signature::try_from(signature).map_err(|e| {
            MlsError::VerificationFailed(format!("Invalid signature format: {}", e))
        })?;

        // Verify the signature
        match verifying_key.verify(data, &sig) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Derives a deterministic user ID from a public key.
    ///
    /// The user ID is derived by taking the SHA-256 hash of the public key
    /// and encoding the first 20 bytes as base58. This produces a short,
    /// human-readable identifier that is collision-resistant.
    ///
    /// # Arguments
    ///
    /// * `public_key` - The Ed25519 public key bytes
    ///
    /// # Returns
    ///
    /// Returns a base58-encoded string derived from the public key.
    pub fn derive_user_id_from_public_key(public_key: &[u8]) -> String {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(public_key);
        let hash = hasher.finalize();

        // Take first 20 bytes and encode as base58
        bs58::encode(&hash[..20]).into_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryStorage;

    fn create_test_manager(user_id: &str) -> MlsManager {
        let storage = Arc::new(InMemoryStorage::new());
        MlsManager::new(user_id, storage).unwrap()
    }

    #[test]
    fn test_manager_creation() {
        let manager = create_test_manager("alice");
        assert_eq!(manager.user_id(), "alice");
    }

    #[test]
    fn test_key_package_generation() {
        let manager = create_test_manager("alice");
        let package = manager.generate_key_package().unwrap();

        assert_eq!(package.user_id, "alice");
        assert!(!package.key_package_data.is_empty());
        assert!(!package.is_expired());
    }

    #[test]
    fn test_get_or_create_key_package() {
        let manager = create_test_manager("alice");
        let pkg1 = manager.get_or_create_key_package().unwrap();
        let pkg2 = manager.get_or_create_key_package().unwrap();
        // Since we are now properly persisting keys, pkg2 should be the same as pkg1
        // IF the logic reuses existing key packages.
        // get_or_create_key_package logic iterates list_keys.
        assert_eq!(pkg1.package_id, pkg2.package_id);
    }

    #[test]
    fn test_no_session_initially() {
        let manager = create_test_manager("alice");
        assert!(!manager.has_session("bob").unwrap());
        assert!(manager.list_sessions().unwrap().is_empty());
    }

    #[test]
    fn test_no_groups_initially() {
        let manager = create_test_manager("alice");
        assert!(manager.list_groups().unwrap().is_empty());
    }

    #[test]
    fn test_group_creation() {
        let manager = create_test_manager("alice");
        let info = manager.create_group("Test Group").unwrap();

        assert_eq!(info.name, Some("Test Group".to_string()));
        assert!(!info.is_session);
        assert_eq!(info.members.len(), 1);

        let groups = manager.list_groups().unwrap();
        assert_eq!(groups.len(), 1);
    }
}
