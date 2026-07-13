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
    ///
    /// Security (SEC-M5): the caller-supplied `user_id` becomes both the
    /// storage key and — at session creation — the identity this key
    /// package is trusted for. Reject storage-hostile ids (the id is a raw
    /// `key_id`), cryptographically validate the key package, and require
    /// the embedded credential identity to equal `user_id` so a key
    /// package generated by one user can never be imported under
    /// another's name.
    pub fn import_key_package(&self, user_id: &str, key_package_data: &[u8]) -> Result<()> {
        offline_protocol_core::validate_id_chars(user_id, "User ID")
            .map_err(|e| MlsError::InvalidUserId(e.to_string()))?;

        let key_package_in = KeyPackageIn::tls_deserialize_exact(key_package_data)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;
        let key_package = key_package_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;

        Self::verify_credential_identity(&key_package, user_id)?;

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
        let key_package = key_package_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;

        // Defense in depth: entries stored before identity binding was
        // enforced at import time (or written to storage out-of-band) must
        // not come back attributed to the wrong user.
        Self::verify_credential_identity(&key_package, user_id)?;

        Ok(key_package)
    }

    /// Requires the key package's leaf credential identity to equal
    /// `user_id`. Credentials in this SDK are basic credentials carrying
    /// the owner's user id as raw bytes (see `IdentityManager`).
    fn verify_credential_identity(key_package: &KeyPackage, user_id: &str) -> Result<()> {
        let identity = key_package.leaf_node().credential().serialized_content();
        if identity != user_id.as_bytes() {
            return Err(MlsError::CredentialIdentityMismatch {
                expected: user_id.to_string(),
                found: String::from_utf8_lossy(identity).into_owned(),
            });
        }
        Ok(())
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

        // Security: `inviter_id` arrives on the wire and is used below as a
        // raw storage key for deletes — reject storage-hostile values just
        // like `import_key_package` does for its user id.
        offline_protocol_core::validate_id_chars(other_user_id, "User ID")
            .map_err(|e| MlsError::InvalidUserId(e.to_string()))?;

        // SEC-M6: reject a mismatched session slot BEFORE the best-effort
        // deletes below, so a forged Welcome that squats a third party's slot
        // performs no mutation. `join_session` re-checks this as its own
        // boundary; hoisting it here keeps the reject side effect-free.
        self.session_manager.verify_welcome_slot(welcome)?;

        // Clear any pending welcome we were about to send
        let _ = self.clear_pending_welcome(other_user_id);

        // Delete conflicting contact key package (we no longer need it)
        let key_type = StorageKeyType::ContactKeyPackage.as_str();
        let _ = self.storage.delete(key_type, other_user_id);

        // Join using their Welcome. `join_session` adopts non-destructively
        // (stage-then-swap), so a retransmitted Welcome that re-stages is a safe
        // no-op rather than deleting and re-creating our existing session.
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
    ///
    /// `claimed_sender` is the transport-level sender this message will be
    /// attributed to; decryption fails with
    /// [`MlsError::SenderIdentityMismatch`] if it does not match the
    /// MLS-authenticated credential (SEC-M1).
    pub fn decrypt_from_user(
        &self,
        encrypted: &EncryptedMessage,
        claimed_sender: &str,
    ) -> Result<Option<Vec<u8>>> {
        self.session_manager
            .decrypt_message(encrypted, claimed_sender)
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
        let group_id = GroupId::new(format!("group:{}", Uuid::new_v4()))?;
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
    ///
    /// `claimed_sender` is the transport-level sender this message will be
    /// attributed to; decryption fails with
    /// [`MlsError::SenderIdentityMismatch`] if it does not match the
    /// MLS-authenticated credential (SEC-M1). The check runs before any
    /// commit is merged, so a spoofed commit cannot advance group state.
    pub fn decrypt_from_group(
        &self,
        encrypted: &EncryptedMessage,
        claimed_sender: &str,
    ) -> Result<Option<Vec<u8>>> {
        let mut group = self
            .group_manager
            .load_group(&encrypted.group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(encrypted.group_id.to_string()))?;

        let mls_message = MlsMessageIn::tls_deserialize_exact(&encrypted.ciphertext)
            .map_err(|e| MlsError::Deserialization(e.to_string()))?;

        let result = self
            .group_manager
            .decrypt_message(&mut group, mls_message, claimed_sender)?;

        self.group_manager.save_group(&encrypted.group_id, &group)?;

        Ok(result)
    }

    /// Joins a group using a Welcome message.
    pub fn join_group(&self, welcome: &WelcomeMessage) -> Result<GroupInfo> {
        // SEC-M6 (group-Welcome side): the `session:` namespace is owned
        // exclusively by identity-bound 1:1 sessions installed via
        // `join_session`. A group Welcome carries an attacker-controllable
        // `group_id` with no (self, inviter) binding and writes the same
        // storage/OpenMLS keyspace, so one naming a `session:*` slot would seed
        // or overwrite a third party's 1:1 session and hijack the victim's
        // outbound encryption — the exact hijack SEC-M6 blocks on the
        // session-Welcome path. Reject before staging. Enforced here (rather
        // than only in the mesh handler) so *every* caller of `join_group` is
        // covered. Legitimate mesh groups are always `group:<uuid>`
        // (see `create_group`), so this rejects only forged Welcomes.
        if welcome.group_id.as_str().starts_with("session:") {
            return Err(MlsError::ReservedSessionNamespace {
                group_id: welcome.group_id.to_string(),
            });
        }

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

    /// Returns `true` if local MLS group state exists for `group_id`.
    ///
    /// This is the authoritative test for "do we actually participate in this
    /// group via MLS" — it gates on the stored group marker, not the member
    /// send-cache (which relay reconciliation can populate without any MLS
    /// state). Callers use it to distinguish a genuine legacy relay-only
    /// (unencrypted) group, which has no MLS state, from an unauthenticated
    /// plaintext frame spoofed against a group the node secures with MLS.
    pub fn has_group(&self, group_id: &GroupId) -> Result<bool> {
        Ok(self.group_manager.load_group(group_id)?.is_some())
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
    ///
    /// `claimed_sender` is the transport-level sender this message will be
    /// attributed to; it must match the MLS-authenticated credential
    /// (SEC-M1).
    pub fn decrypt(
        &self,
        encrypted: &EncryptedMessage,
        claimed_sender: &str,
    ) -> Result<Option<Vec<u8>>> {
        if encrypted.group_id.as_str().starts_with("session:") {
            self.decrypt_from_user(encrypted, claimed_sender)
        } else {
            self.decrypt_from_group(encrypted, claimed_sender)
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

/// Returns `true` when `bytes` parse as a well-formed MLS wire message
/// (`MlsMessageIn` TLS framing, consuming the input exactly).
///
/// Inbound routing uses this to distinguish MLS ciphertext from legacy
/// plaintext that merely happens to be valid base64: the strict TLS framing
/// (protocol version, wire format, exact-length body) makes an accidental
/// match against non-MLS bytes vanishingly unlikely. This is a framing
/// check only — it says nothing about whether the message can be decrypted.
pub fn is_mls_framed(bytes: &[u8]) -> bool {
    MlsMessageIn::tls_deserialize_exact(bytes).is_ok()
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
    fn test_import_key_package_binds_credential_identity() {
        let alice = create_test_manager("alice");
        let mallory = create_test_manager("mallory");
        let mallory_kp = mallory.generate_key_package().unwrap();

        // Importing mallory's key package under "bob" must be rejected —
        // otherwise a session "with bob" would encrypt to mallory's keys.
        let err = alice
            .import_key_package("bob", &mallory_kp.key_package_data)
            .unwrap_err();
        assert!(matches!(err, MlsError::CredentialIdentityMismatch { .. }));

        // Importing under the matching identity succeeds.
        alice
            .import_key_package("mallory", &mallory_kp.key_package_data)
            .unwrap();
    }

    #[test]
    fn test_import_key_package_rejects_hostile_user_id() {
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let bob_kp = bob.generate_key_package().unwrap();

        for hostile in ["", "..", "bob/evil", "bob\\evil", "bob\0evil"] {
            let err = alice
                .import_key_package(hostile, &bob_kp.key_package_data)
                .unwrap_err();
            assert!(
                matches!(err, MlsError::InvalidUserId(_)),
                "expected InvalidUserId for {:?}, got {:?}",
                hostile,
                err
            );
        }
    }

    #[test]
    fn test_get_contact_key_package_rejects_poisoned_store() {
        // A key package written to storage out-of-band (bypassing import
        // validation, e.g. persisted before this fix) under the wrong user
        // id must fail identity verification at use time.
        let storage = Arc::new(InMemoryStorage::new());
        let alice = MlsManager::new("alice", storage.clone()).unwrap();
        let mallory = create_test_manager("mallory");
        let mallory_kp = mallory.generate_key_package().unwrap();

        storage
            .store(
                StorageKeyType::ContactKeyPackage.as_str(),
                "bob",
                &mallory_kp.key_package_data,
            )
            .unwrap();

        // create_session("bob") loads the poisoned package and must refuse.
        let err = alice.create_session("bob").unwrap_err();
        assert!(matches!(err, MlsError::CredentialIdentityMismatch { .. }));
    }

    #[test]
    fn test_replace_session_with_welcome_rejects_hostile_inviter_id() {
        // `inviter_id` arrives on the wire and is used as a raw storage key
        // for deletes — hostile values must be rejected before any storage
        // operation runs.
        let alice = create_test_manager("alice");
        let welcome = WelcomeMessage {
            group_id: GroupId::new("session:alice:bob").unwrap(),
            welcome_data: vec![],
            inviter_id: "../../etc".to_string(),
            group_name: None,
            timestamp_ms: 0,
        };
        let err = alice.replace_session_with_welcome(&welcome).unwrap_err();
        assert!(matches!(err, MlsError::InvalidUserId(_)));
    }

    /// Builds a converged alice/bob 1:1 session for sender-binding tests.
    fn create_test_session() -> (MlsManager, MlsManager) {
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        let welcome = alice.create_session("bob").unwrap();
        bob.join_session(&welcome).unwrap();
        (alice, bob)
    }

    #[test]
    fn test_session_decrypt_rejects_spoofed_sender() {
        let (alice, bob) = create_test_session();

        // A message authenticated as alice must not be attributable to a
        // different wire sender (SEC-M1).
        let ct = alice.encrypt_for_user("bob", b"hello").unwrap();
        let err = bob.decrypt_from_user(&ct, "mallory").unwrap_err();
        assert!(matches!(err, MlsError::SenderIdentityMismatch { .. }));

        // With the correct claimed sender, a fresh message decrypts. (The
        // spoofed attempt above consumed its ratchet generation — that
        // message is burned, which is fine for a forgery.)
        let ct2 = alice.encrypt_for_user("bob", b"hello again").unwrap();
        let pt = bob.decrypt_from_user(&ct2, "alice").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"hello again"[..]));
    }

    #[test]
    fn test_join_session_binds_slot_to_inviter() {
        // Regression (session-hijack): a Welcome from an authenticated inviter
        // may only install the 1:1 session slot for (self, inviter). An inviter
        // that names a *third* party's slot must be rejected before any group
        // state is touched — otherwise the inviter overwrites/seeds the
        // victim's session with that third party, so the victim's outbound
        // messages to that party encrypt to the attacker's group.
        let bob = create_test_manager("bob");

        // mallory, authenticating honestly as herself, sends bob a Welcome whose
        // group_id squats alice+bob's session slot.
        let hijack = WelcomeMessage {
            group_id: GroupId::new("session:alice:bob").unwrap(),
            welcome_data: vec![], // rejected before deserialization
            inviter_id: "mallory".to_string(),
            group_name: None,
            timestamp_ms: 0,
        };

        // Both join entry points must reject the mismatched slot.
        let err = bob.join_session(&hijack).unwrap_err();
        assert!(
            matches!(err, MlsError::WelcomeIdentityMismatch { .. }),
            "join_session: expected WelcomeIdentityMismatch, got {:?}",
            err
        );
        let err = bob.replace_session_with_welcome(&hijack).unwrap_err();
        assert!(
            matches!(err, MlsError::WelcomeIdentityMismatch { .. }),
            "replace_session_with_welcome: expected WelcomeIdentityMismatch, got {:?}",
            err
        );

        // Nothing was installed: bob has no session with either identity.
        assert!(!bob.has_session("alice").unwrap());
        assert!(!bob.has_session("mallory").unwrap());

        // The check is precise, not over-broad: a correctly-slotted Welcome from
        // mallory (group_id == session:bob:mallory, inviter_id == mallory) still
        // joins normally.
        let bob_kp = bob.generate_key_package().unwrap();
        let mallory = create_test_manager("mallory");
        mallory
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        let legit = mallory.create_session("bob").unwrap();
        bob.join_session(&legit).unwrap();
        assert!(bob.has_session("mallory").unwrap());
    }

    #[test]
    fn test_join_group_rejects_session_namespace() {
        // Regression (session-hijack via the group-Welcome path): the identity
        // binding on `join_session` only guards the session-Welcome path. A
        // group Welcome carries an attacker-controllable `group_id` and writes
        // the SAME storage/OpenMLS keyspace, so `join_group` must refuse the
        // reserved `session:` namespace outright — otherwise an authenticated
        // peer could seed/overwrite a third party's 1:1 session slot and
        // hijack the victim's outbound encryption.
        let bob = create_test_manager("bob");

        let squat = WelcomeMessage {
            group_id: GroupId::new("session:alice:bob").unwrap(),
            welcome_data: vec![], // rejected before deserialization
            inviter_id: "mallory".to_string(),
            group_name: None,
            timestamp_ms: 0,
        };

        let err = bob.join_group(&squat).unwrap_err();
        assert!(
            matches!(err, MlsError::ReservedSessionNamespace { .. }),
            "join_group: expected ReservedSessionNamespace, got {:?}",
            err
        );

        // The squatted 1:1 slot was never installed.
        assert!(!bob.has_session("alice").unwrap());

        // Precision: a legitimately-namespaced group Welcome is not caught by
        // this guard (it fails later at deserialization of the empty blob, not
        // with ReservedSessionNamespace).
        let legit_ns = WelcomeMessage {
            group_id: GroupId::new("group:0bd6e5f2-3a70-4a4e-9c3f-1c1f2a3b4c5d").unwrap(),
            welcome_data: vec![],
            inviter_id: "mallory".to_string(),
            group_name: None,
            timestamp_ms: 0,
        };
        let err = bob.join_group(&legit_ns).unwrap_err();
        assert!(
            !matches!(err, MlsError::ReservedSessionNamespace { .. }),
            "a group:-namespaced Welcome must not trip the session-namespace guard, got {:?}",
            err
        );
    }

    #[test]
    fn test_join_group_rejects_embedded_group_id_mismatch() {
        // Regression (HIGH-1): OpenMLS persists a joined group under the group
        // id embedded in the Welcome's GroupContext — a value the inviter picks
        // freely at creation (`new_with_group_id`) — while our storage marker
        // and every load/delete lookup key off the *wire* `group_id`, which is
        // all the SEC-M5/M6 bindings validate. If the two diverge, `into_group`
        // would install the group under the attacker's embedded id: an
        // arbitrary slot the wire-id checks never inspected. The join must bind
        // embedded == wire and reject before any state is written.
        let bob = create_test_manager("bob");
        let mallory = create_test_manager("mallory");

        // Mallory builds a real group whose EMBEDDED id is one value...
        let bob_kp = bob.generate_key_package().unwrap();
        let embedded = GroupId::new("group:11111111-1111-4111-8111-111111111111").unwrap();
        let cred = mallory.get_credential().unwrap();
        let signer = mallory.get_signer().unwrap();
        mallory
            .group_manager
            .create_group(&embedded, &cred, &signer)
            .unwrap();
        let (welcome, _commit) = mallory
            .add_group_member(&embedded, &bob_kp.key_package_data)
            .unwrap();
        assert_eq!(welcome.group_id, embedded);

        // ...but presents it under a DIFFERENT wire group_id. The wire id clears
        // the reserved-namespace guard (not `session:`), so only the embedded-id
        // binding stands between the attacker and an arbitrary slot.
        let wire = GroupId::new("group:22222222-2222-4222-8222-222222222222").unwrap();
        let tampered = WelcomeMessage {
            group_id: wire.clone(),
            ..welcome
        };

        let err = bob.join_group(&tampered).unwrap_err();
        assert!(
            matches!(err, MlsError::WelcomeGroupIdMismatch { .. }),
            "expected WelcomeGroupIdMismatch, got {:?}",
            err
        );

        // Nothing was installed under EITHER id — the reject precedes into_group.
        assert!(bob.group_manager.load_group(&wire).unwrap().is_none());
        assert!(bob.group_manager.load_group(&embedded).unwrap().is_none());
    }

    #[test]
    fn test_welcome_embedded_id_cannot_hijack_session_slot() {
        // Regression (HIGH-1, the SEC-M6 hijack reached through the embedded id).
        // Bob has a live 1:1 session with Alice at `session:alice:bob`. Mallory,
        // authenticating honestly as herself with a wire `group_id` that PASSES
        // `verify_welcome_slot` (`session:bob:mallory`), sends a Welcome whose
        // *embedded* GroupContext id squats `session:alice:bob`. Without the
        // embedded-id binding, `into_group` overwrites Bob's Alice session so his
        // next `encrypt_for_user("alice")` encrypts to Mallory's group. The
        // binding must reject it and leave Bob's Alice session intact.
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        let welcome = alice.create_session("bob").unwrap();
        bob.join_session(&welcome).unwrap();
        assert!(bob.has_session("alice").unwrap());

        // Mallory crafts a group whose embedded id squats bob's alice slot.
        let mallory = create_test_manager("mallory");
        let bob_kp2 = bob.generate_key_package().unwrap();
        let squat = GroupId::new("session:alice:bob").unwrap();
        let cred = mallory.get_credential().unwrap();
        let signer = mallory.get_signer().unwrap();
        mallory
            .group_manager
            .create_group(&squat, &cred, &signer)
            .unwrap();
        let (mal_welcome, _commit) = mallory
            .add_group_member(&squat, &bob_kp2.key_package_data)
            .unwrap();

        // Present it under a wire slot that passes verify_welcome_slot for mallory.
        let attack = WelcomeMessage {
            group_id: GroupId::new("session:bob:mallory").unwrap(),
            welcome_data: mal_welcome.welcome_data,
            inviter_id: "mallory".to_string(),
            group_name: None,
            timestamp_ms: 0,
        };

        let err = bob.join_session(&attack).unwrap_err();
        assert!(
            matches!(err, MlsError::WelcomeGroupIdMismatch { .. }),
            "expected WelcomeGroupIdMismatch, got {:?}",
            err
        );

        // Bob's Alice session survived and still encrypts to Alice's group.
        assert!(bob.has_session("alice").unwrap());
        let ct = bob.encrypt_for_user("alice", b"still private").unwrap();
        let pt = alice.decrypt_from_user(&ct, "bob").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"still private"[..]));
    }

    /// Builds a two-member group (alice admin, bob member) for group
    /// sender-binding tests. Returns (alice, bob, group_id).
    fn create_test_group_with_bob() -> (MlsManager, MlsManager, GroupId) {
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        let info = alice.create_group("Test Group").unwrap();
        let gid = info.group_id.clone();
        let (welcome, _commit) = alice
            .add_group_member(&gid, &bob_kp.key_package_data)
            .unwrap();
        bob.join_group(&welcome).unwrap();
        (alice, bob, gid)
    }

    #[test]
    fn test_group_decrypt_rejects_spoofed_sender() {
        let (alice, bob, gid) = create_test_group_with_bob();

        let ct = alice.encrypt_for_group(&gid, b"group message").unwrap();
        let err = bob.decrypt_from_group(&ct, "mallory").unwrap_err();
        assert!(matches!(err, MlsError::SenderIdentityMismatch { .. }));

        let ct2 = alice.encrypt_for_group(&gid, b"another one").unwrap();
        let pt = bob.decrypt_from_group(&ct2, "alice").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"another one"[..]));
    }

    #[test]
    fn test_group_commit_with_spoofed_sender_rejected_before_merge() {
        let (alice, bob, gid) = create_test_group_with_bob();

        // Alice issues a key-update commit; bob receives it with a spoofed
        // wire sender. The mismatch must be detected BEFORE the staged
        // commit is merged — bob's epoch must not advance.
        let commit = alice.update_keys(&gid).unwrap();
        let epoch_before = bob.get_group_info(&gid).unwrap().unwrap().epoch;

        let err = bob.decrypt_from_group(&commit, "mallory").unwrap_err();
        assert!(matches!(err, MlsError::SenderIdentityMismatch { .. }));

        let epoch_after = bob.get_group_info(&gid).unwrap().unwrap().epoch;
        assert_eq!(
            epoch_before, epoch_after,
            "spoofed commit must not advance group state"
        );
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

    /// Regression test for the both-create split-brain re-brick.
    ///
    /// With auto key exchange, both peers create a `session:a:b` group and the
    /// higher-id peer adopts the lower-id "owner"'s Welcome. The owner keeps
    /// retransmitting its Welcome until it sees a group-aware proof, so the
    /// adopter receives the SAME Welcome again *after* it already adopted. MLS
    /// key packages are one-time, so re-staging that Welcome must fail — but it
    /// must fail NON-DESTRUCTIVELY, leaving the converged group intact. The old
    /// delete-then-stage path deleted the good group first and then could not
    /// re-stage, permanently bricking a working session.
    #[test]
    fn test_both_create_adopt_is_non_destructive_on_welcome_retransmit() {
        // alice = lexicographically-lower "owner"; bob = higher "adopter".
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");

        // Exchange key packages both ways (both sides will auto-create).
        let alice_kp = alice.generate_key_package().unwrap();
        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        bob.import_key_package("alice", &alice_kp.key_package_data)
            .unwrap();

        // Both-create race: each peer creates its own session group + Welcome.
        let alice_welcome = alice.create_session("bob").unwrap(); // owner's Welcome
        let _bob_welcome = bob.create_session("alice").unwrap(); // adopter's own group
        assert!(bob.has_session("alice").unwrap());

        // Adopter adopts the owner's Welcome (first delivery): replaces its own
        // group with alice's, consuming bob's one-time key package.
        bob.join_session(&alice_welcome).unwrap();
        assert!(bob.has_session("alice").unwrap());

        // Convergence: alice encrypts and bob decrypts on the shared group.
        let ct = alice
            .encrypt_for_user("bob", b"hello over the converged group")
            .unwrap();
        let pt = bob.decrypt_from_user(&ct, "alice").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"hello over the converged group"[..]));

        // Owner retransmits the SAME Welcome (its periodic retry). Re-staging
        // MUST fail (bob's key package is consumed) but MUST be non-destructive.
        let retransmit = bob.join_session(&alice_welcome);
        assert!(
            retransmit.is_err(),
            "re-staging a consumed key package should fail"
        );

        // The converged group MUST survive the failed retransmit. This is the
        // regression: the old delete-then-stage path left bob with no session.
        assert!(
            bob.has_session("alice").unwrap(),
            "duplicate Welcome must not brick the converged session"
        );

        // ...and it must still be functional after the failed retransmit.
        let ct2 = alice
            .encrypt_for_user("bob", b"still converged after retransmit")
            .unwrap();
        let pt2 = bob.decrypt_from_user(&ct2, "alice").unwrap();
        assert_eq!(
            pt2.as_deref(),
            Some(&b"still converged after retransmit"[..])
        );
    }

    /// Windowed media transfers keep up to 8 encrypted chunks in flight
    /// (interleaved with text on the same session ratchet), so a delayed chunk
    /// can arrive many generations behind the newest decrypted message. The
    /// OpenMLS default tolerance (5) would delete its key and permanently stall
    /// the transfer; `SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE` (32) must cover it.
    #[test]
    fn test_out_of_order_decryption_within_sender_ratchet_tolerance() {
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");

        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        let welcome = alice.create_session("bob").unwrap();
        bob.join_session(&welcome).unwrap();

        let ciphertexts: Vec<_> = (0..40)
            .map(|i| {
                alice
                    .encrypt_for_user("bob", format!("chunk {}", i).as_bytes())
                    .unwrap()
            })
            .collect();

        // Decrypt the newest message first, ratcheting bob's receive state far
        // ahead of every earlier generation.
        let pt = bob.decrypt_from_user(&ciphertexts[39], "alice").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"chunk 39"[..]));

        // 29 generations behind: far beyond the OpenMLS default of 5, but
        // within our tolerance of 32 — must still decrypt.
        let pt = bob.decrypt_from_user(&ciphertexts[10], "alice").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"chunk 10"[..]));

        // 39 generations behind: beyond the tolerance, the key is deleted and
        // the message must NOT decrypt (proves the configured bound applies).
        let res = bob.decrypt_from_user(&ciphertexts[0], "alice");
        assert!(
            !matches!(res, Ok(Some(_))),
            "generation beyond the tolerance must not decrypt"
        );
    }
}
