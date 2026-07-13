//! MLS group management.
//!
//! This module handles creation, modification, and state management of MLS groups.

use crate::error::{MlsError, Result};
use crate::provider::MlsProvider;
use crate::storage::{MlsStorage, StorageError};
use crate::types::{GroupId, GroupInfo, StorageKeyType};

use openmls::prelude::*;
use openmls_traits::signatures::Signer;
use openmls_traits::OpenMlsProvider;
use std::sync::Arc;
use tracing::{debug, warn};

/// Default ciphersuite for MLS operations.
pub const DEFAULT_CIPHERSUITE: Ciphersuite =
    Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

/// How many generations behind the latest decrypted message a sender ratchet
/// key is kept, so late/reordered messages remain decryptable.
///
/// The OpenMLS default (5) is smaller than the windowed media transfer's
/// in-flight budget (up to 8 chunks on internet transports, interleaved with
/// text on the same 1:1 session ratchet), which would make a sufficiently
/// delayed chunk *permanently* undecryptable and stall the transfer. 32 gives
/// 4x headroom over the largest window at the cost of retaining up to 32
/// unused message keys per sender ratchet.
///
/// The protocol layer keeps the combined in-flight budget within this bound
/// by capping concurrent media transfers per peer
/// (`MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER` in `offline-protocol`); revisit
/// both together if either changes.
pub const SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE: u32 = 32;

/// How far ahead of the highest seen generation a sender ratchet may be
/// fast-forwarded when messages are lost (OpenMLS default).
pub const SENDER_RATCHET_MAXIMUM_FORWARD_DISTANCE: u32 = 1000;

/// Sender ratchet configuration applied to every created and joined group.
fn sender_ratchet_configuration() -> SenderRatchetConfiguration {
    SenderRatchetConfiguration::new(
        SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE,
        SENDER_RATCHET_MAXIMUM_FORWARD_DISTANCE,
    )
}

/// Manages MLS groups for encrypted messaging.
pub struct GroupManager {
    /// Storage backend for persisting group state.
    storage: Arc<dyn MlsStorage>,

    /// OpenMLS crypto provider.
    provider: MlsProvider,
}

impl GroupManager {
    /// Creates a new group manager.
    pub fn new(storage: Arc<dyn MlsStorage>, provider: MlsProvider) -> Self {
        Self { storage, provider }
    }

    /// Returns a reference to the crypto provider.
    pub fn provider(&self) -> &MlsProvider {
        &self.provider
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
            .sender_ratchet_configuration(sender_ratchet_configuration())
            .build();

        let mls_group_id = openmls::group::GroupId::from_slice(group_id.as_str().as_bytes());

        let group = MlsGroup::new_with_group_id(
            &self.provider,
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
                let mls_group_id =
                    openmls::group::GroupId::from_slice(group_id.as_str().as_bytes());

                // Try to load from the provider's storage
                match MlsGroup::load(self.provider.storage(), &mls_group_id) {
                    Ok(Some(group)) => Ok(Some(group)),
                    Ok(None) => {
                        debug!(group_id = %group_id, "Group not found in crypto storage");
                        Ok(None)
                    }
                    Err(e) => Err(MlsError::Deserialization(format!(
                        "Failed to load group: {:?}",
                        e
                    ))),
                }
            }
            None => Ok(None),
        }
    }

    /// Saves a group marker to storage.
    ///
    /// OpenMLS persists group state through the provider during mutations.
    /// This marker is kept in our storage layer for listing/enumeration.
    pub fn save_group(&self, group_id: &GroupId, group: &MlsGroup) -> Result<()> {
        let key_type = StorageKeyType::GroupState.as_str();
        let marker = group.epoch().as_u64().to_le_bytes();
        self.storage.store(key_type, group_id.as_str(), &marker)?;
        Ok(())
    }

    /// Deletes a group from storage, including all OpenMLS provider state.
    pub fn delete_group(&self, group_id: &GroupId) -> Result<()> {
        let mls_group_id = openmls::group::GroupId::from_slice(group_id.as_str().as_bytes());

        match MlsGroup::load(self.provider.storage(), &mls_group_id) {
            Ok(Some(mut group)) => {
                group.delete(self.provider.storage()).map_err(|e| {
                    MlsError::Storage(StorageError::DeleteFailed(format!(
                        "Failed to delete group from provider storage: {:?}",
                        e
                    )))
                })?;
            }
            // No group at this id — nothing to clean up.
            Ok(None) => {}
            // We could not load the group to delete its provider-side state
            // (epoch keypairs, secrets); that state may now leak. Warn rather
            // than abort and still drop the marker below: callers such as
            // `join_group_replacing` deliberately rely on `delete_group` not
            // failing here once the one-time key package is already consumed.
            Err(e) => {
                warn!(
                    group_id = %group_id,
                    error = ?e,
                    "delete_group: could not load group to clean provider state; dropping marker only"
                );
            }
        }

        let key_type = StorageKeyType::GroupState.as_str();
        self.storage.delete(key_type, group_id.as_str())?;

        Ok(())
    }

    /// Lists all groups.
    ///
    /// Stored keys that fail group-id validation (possible only for state
    /// written before validation was enforced) are skipped with a warning
    /// rather than resurrected as live group ids.
    pub fn list_groups(&self) -> Result<Vec<GroupId>> {
        let key_type = StorageKeyType::GroupState.as_str();
        let keys = self.storage.list_keys(key_type)?;
        Ok(keys
            .into_iter()
            .filter_map(|key| match GroupId::new(key) {
                Ok(group_id) => Some(group_id),
                Err(e) => {
                    warn!(error = %e, "Skipping stored group with invalid id");
                    None
                }
            })
            .collect())
    }

    /// Adds a member to a group using their key package.
    pub fn add_member(
        &self,
        group: &mut MlsGroup,
        key_package: KeyPackage,
        signer: &impl Signer,
    ) -> Result<(MlsMessageOut, MlsMessageOut)> {
        let (commit, welcome, _group_info) = group
            .add_members(&self.provider, signer, &[key_package])
            .map_err(|e| MlsError::AddMember(e.to_string()))?;

        // Merge the pending commit
        group
            .merge_pending_commit(&self.provider)
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
            .remove_members(&self.provider, signer, &[member_index])
            .map_err(|e| MlsError::RemoveMember(e.to_string()))?;

        group
            .merge_pending_commit(&self.provider)
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
            .create_message(&self.provider, signer, plaintext)
            .map_err(|e| MlsError::Encryption(e.to_string()))?;

        Ok(message)
    }

    /// Decrypts an incoming MLS message.
    ///
    /// Security (SEC-M1): `claimed_sender` is the identity the caller will
    /// attribute this message to (the transport-level sender). It is
    /// cross-checked against the MLS-authenticated credential of the
    /// processed message *before* any plaintext is surfaced or any commit
    /// is merged, so a group member cannot impersonate another member by
    /// lying in the wire envelope.
    pub fn decrypt_message(
        &self,
        group: &mut MlsGroup,
        message: MlsMessageIn,
        claimed_sender: &str,
    ) -> Result<Option<Vec<u8>>> {
        let protocol_message = message
            .try_into_protocol_message()
            .map_err(|e| MlsError::Decryption(format!("Invalid protocol message: {:?}", e)))?;

        let processed = group
            .process_message(&self.provider, protocol_message)
            .map_err(|e| MlsError::Decryption(e.to_string()))?;

        // Credentials in this SDK are basic credentials carrying the user id
        // as raw bytes (see `MlsManager::create_identity`).
        let authenticated = processed.credential().serialized_content();
        if authenticated != claimed_sender.as_bytes() {
            return Err(MlsError::SenderIdentityMismatch {
                claimed: claimed_sender.to_string(),
                authenticated: String::from_utf8_lossy(authenticated).into_owned(),
            });
        }

        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(app_msg) => Ok(Some(app_msg.into_bytes())),
            ProcessedMessageContent::ProposalMessage(_) => {
                debug!("Received proposal message");
                Ok(None)
            }
            ProcessedMessageContent::StagedCommitMessage(staged_commit) => {
                debug!("Received commit message, merging");
                group
                    .merge_staged_commit(&self.provider, *staged_commit)
                    .map_err(|e| MlsError::Decryption(format!("Failed to merge commit: {}", e)))?;
                Ok(None)
            }
            ProcessedMessageContent::ExternalJoinProposalMessage(_) => {
                warn!("Received external join proposal (not supported)");
                Ok(None)
            }
        }
    }

    /// Binds a staged Welcome to the `group_id` the caller authenticated.
    ///
    /// OpenMLS derives the id it persists the joined group under from the
    /// Welcome's embedded `GroupContext` — a value the inviter picks freely at
    /// group creation (`MlsGroup::new_with_group_id`). Our storage marker
    /// ([`save_group`]) and every [`load_group`]/`delete_group` lookup are
    /// instead keyed by the caller-supplied wire `group_id`, and every
    /// SEC-M5/M6 binding (`verify_welcome_slot`, the `session:`
    /// reserved-namespace check) validates only that wire field. If the two
    /// diverge, `into_group` writes the group under the *embedded* id, letting
    /// an authenticated inviter seed or overwrite an arbitrary slot the
    /// wire-id checks never inspected (e.g. embed `session:alice:bob` behind a
    /// benign wire id that passes `verify_welcome_slot`) and hijack the
    /// victim's session. Assert the two agree *before* `into_group` so no
    /// cross-slot state is ever persisted. Legitimate Welcomes always carry
    /// embedded == wire (both created via `new_with_group_id(group_id)`), so
    /// this only rejects forgeries.
    ///
    /// Staging has already consumed the one-time key package by this point;
    /// that only burns the package (inherent to any Welcome processing) and
    /// does not touch existing group state.
    fn verify_staged_group_id(staged: &StagedWelcome, group_id: &GroupId) -> Result<()> {
        let embedded = staged.group_context().group_id().as_slice();
        if embedded != group_id.as_str().as_bytes() {
            return Err(MlsError::WelcomeGroupIdMismatch {
                expected: group_id.to_string(),
                embedded: String::from_utf8_lossy(embedded).into_owned(),
            });
        }
        Ok(())
    }

    /// Joins a group using a Welcome message.
    pub fn join_group(&self, welcome: Welcome, group_id: &GroupId) -> Result<MlsGroup> {
        let group_config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .sender_ratchet_configuration(sender_ratchet_configuration())
            .build();

        let staged = StagedWelcome::new_from_welcome(&self.provider, &group_config, welcome, None)
            .map_err(|e| MlsError::WelcomeProcessing(format!("Failed to stage welcome: {}", e)))?;

        // Bind the Welcome's embedded group id to the authenticated wire id
        // before persisting — otherwise `into_group` would install under the
        // attacker-chosen embedded id. See `verify_staged_group_id`.
        Self::verify_staged_group_id(&staged, group_id)?;

        let group = staged
            .into_group(&self.provider)
            .map_err(|e| MlsError::WelcomeProcessing(format!("Failed to join group: {}", e)))?;

        self.save_group(group_id, &group)?;

        debug!(group_id = %group_id, "Joined group via Welcome");
        Ok(group)
    }

    /// Joins a group from a Welcome, replacing any existing group at the same
    /// `group_id`. **Non-destructive on the common failure (a duplicate
    /// Welcome); best-effort — NOT atomic — on the rare storage failure.**
    ///
    /// Staging the incoming Welcome (`StagedWelcome::new_from_welcome`) is the
    /// step that consumes the one-time key package, so it is the natural failure
    /// point: if the key package is unavailable — e.g. a retransmitted Welcome we
    /// already adopted, or a first-contact key-package race — staging returns
    /// `Err` **before** we touch the existing group, which is therefore left
    /// completely intact. This makes both-create adoption idempotent: a duplicate
    /// Welcome is a safe no-op rather than a re-brick, the case this method exists
    /// to handle.
    ///
    /// Caveat — once staging succeeds the swap is **not atomic**. We delete the
    /// prior group (to avoid orphaning its old-leaf epoch encryption keypairs,
    /// which are keyed by `(group_id, epoch, leaf_index)`; the adopted group
    /// installs at a different leaf and so would not overwrite them) and then call
    /// `into_group`, which performs several sequential, non-transactional storage
    /// writes. If one of those fails (e.g. a Keychain/Keystore write error), the
    /// old group is already gone and the new one is absent — a lost session. The
    /// key package is spent by then, so the same Welcome cannot simply be retried;
    /// recovery is a fresh key-package exchange. The caller distinguishes this
    /// "lost group" outcome from a benign duplicate by observing `has_session`
    /// (false here, true for a duplicate) and surfaces it as a session failure.
    pub fn join_group_replacing(&self, welcome: Welcome, group_id: &GroupId) -> Result<MlsGroup> {
        let group_config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .sender_ratchet_configuration(sender_ratchet_configuration())
            .build();

        // Stage first — non-destructive failure point (consumes the key package).
        // A failure here leaves the existing group untouched (benign duplicate).
        let staged = StagedWelcome::new_from_welcome(&self.provider, &group_config, welcome, None)
            .map_err(|e| MlsError::WelcomeProcessing(format!("Failed to stage welcome: {}", e)))?;

        // Bind the Welcome's embedded group id to the authenticated wire id
        // before the destructive replace below. A forged Welcome whose embedded
        // id differs from `group_id` would otherwise `into_group` under the
        // embedded id and hijack that slot; rejecting here also leaves the
        // existing group at `group_id` untouched (no `delete_group` runs). See
        // `verify_staged_group_id`.
        Self::verify_staged_group_id(&staged, group_id)?;

        // Staging consumed the key package; from here a failure is not
        // recoverable by retrying the same Welcome. Drop the prior group so its
        // old-leaf epoch keypairs are not orphaned. `delete_group` already
        // no-ops on absence and tolerates a corrupt prior group, so call it
        // unconditionally — a redundant `load_group` here would only add another
        // fallible storage read that could abort the join with the key package
        // already spent.
        self.delete_group(group_id)?;

        let group = staged.into_group(&self.provider).map_err(|e| {
            // Distinct message from the staging failure above: the prior group is
            // already deleted and the key package is spent, so this is the
            // non-atomic "lost group" window, not a benign duplicate.
            MlsError::WelcomeProcessing(format!(
                "Failed to install adopted group (prior group removed, key package consumed): {}",
                e
            ))
        })?;

        self.save_group(group_id, &group)?;

        debug!(group_id = %group_id, "Joined/adopted group via Welcome (non-destructive stage, best-effort swap)");
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
        let members_count = members.len() as u32;

        GroupInfo {
            group_id: group_id.clone(),
            name: None,
            members,
            members_count,
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
    use crate::storage_adapter::MlsStorageAdapter;

    fn create_test_group_manager() -> GroupManager {
        let storage = Arc::new(InMemoryStorage::new());
        let adapter = MlsStorageAdapter::new(storage.clone());
        let provider = MlsProvider::new(adapter);
        GroupManager::new(storage, provider)
    }

    #[test]
    fn test_group_manager_creation() {
        let manager = create_test_group_manager();
        assert!(manager.list_groups().unwrap().is_empty());
    }
}
