//! 1:1 session management using MLS groups.
//!
//! This module provides a higher-level abstraction for 1:1 encrypted messaging
//! by using 2-person MLS groups under the hood.

use crate::error::{MlsError, Result};
use crate::group::GroupManager;
use crate::provider::MlsProvider;
use crate::storage::MlsStorage;
use crate::types::{EncryptedMessage, GroupId, GroupInfo, MlsMessageType, WelcomeMessage};

use openmls::prelude::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls::prelude::*;
use openmls_traits::signatures::Signer;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

/// Where one sealed frame sits in its session's sending order: the
/// `index`-th frame this device sealed in that session, and which session
/// (`incarnation`) that was.
///
/// The incarnation is what keeps an acknowledgement for a frame of a session
/// that has since been rebuilt from vouching for the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealMark {
    /// Which session the frame was sealed in. New on every create, join and
    /// delete.
    pub incarnation: u64,
    /// The frame's position among this session's seals, from 1.
    pub index: u64,
}

/// How far this device's sealing has run ahead of what the peer has shown
/// it received, per session.
#[derive(Debug, Clone, Copy)]
struct SealProgress {
    incarnation: u64,
    sealed: u64,
    acknowledged: u64,
}

/// Manages 1:1 encrypted sessions.
pub struct SessionManager {
    /// The local user's ID.
    user_id: String,

    /// Group manager for underlying MLS operations.
    group_manager: GroupManager,

    /// Seals per session against the highest seal the peer has acknowledged.
    /// In memory only: after a restart the count starts again from nothing,
    /// which reads as "no gap", the permissive direction, until the first
    /// acknowledgement.
    seal_progress: Mutex<HashMap<String, SealProgress>>,

    /// Source of session incarnations.
    next_incarnation: AtomicU64,
}

impl SessionManager {
    /// Creates a new session manager.
    pub fn new(user_id: String, storage: Arc<dyn MlsStorage>, provider: MlsProvider) -> Self {
        let group_manager = GroupManager::new(storage, provider);
        Self {
            user_id,
            group_manager,
            seal_progress: Mutex::new(HashMap::new()),
            next_incarnation: AtomicU64::new(1),
        }
    }

    fn progress(&self) -> std::sync::MutexGuard<'_, HashMap<String, SealProgress>> {
        self.seal_progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn fresh_progress(&self) -> SealProgress {
        SealProgress {
            incarnation: self.next_incarnation.fetch_add(1, Ordering::Relaxed),
            sealed: 0,
            acknowledged: 0,
        }
    }

    /// Starts a session's seal count from nothing: it was created, joined or
    /// deleted, so nothing sealed before can vouch for it.
    fn restart_seal_progress(&self, session_id: &GroupId) {
        let fresh = self.fresh_progress();
        self.progress().insert(session_id.to_string(), fresh);
    }

    /// The position of the most recent frame sealed for `other_user_id`, or
    /// `None` when nothing has been sealed in this session since it started
    /// (or since this process did).
    pub fn last_seal_mark(&self, other_user_id: &str) -> Option<SealMark> {
        let session_id = self.get_session_id(other_user_id).ok()?;
        let progress = self.progress();
        let entry = progress.get(session_id.as_str())?;
        (entry.sealed > 0).then_some(SealMark {
            incarnation: entry.incarnation,
            index: entry.sealed,
        })
    }

    /// Records that the peer received the frame at `mark`, so it has
    /// decrypted at least that far into the session. Ignored for a frame of
    /// an earlier incarnation of the session.
    pub fn note_acknowledged_seal(&self, other_user_id: &str, mark: SealMark) {
        let Ok(session_id) = self.get_session_id(other_user_id) else {
            return;
        };
        if let Some(entry) = self.progress().get_mut(session_id.as_str()) {
            if entry.incarnation == mark.incarnation {
                entry.acknowledged = entry.acknowledged.max(mark.index);
            }
        }
    }

    /// How many frames this device has sealed for `other_user_id` past the
    /// last one the peer acknowledged.
    ///
    /// A receiver cannot decrypt a frame more than the forward window past
    /// the newest it has decrypted, and an acknowledged frame is one it has,
    /// so keeping this below the window keeps every frame decryptable. The
    /// sender is what controls it: each re-sealed resend adds one.
    pub fn unacknowledged_seals(&self, other_user_id: &str) -> u64 {
        let Ok(session_id) = self.get_session_id(other_user_id) else {
            return 0;
        };
        self.progress()
            .get(session_id.as_str())
            .map_or(0, |entry| entry.sealed.saturating_sub(entry.acknowledged))
    }

    /// Returns a reference to the underlying group manager.
    pub fn group_manager(&self) -> &GroupManager {
        &self.group_manager
    }

    /// Gets the session ID for a 1:1 conversation with another user.
    ///
    /// Fails with [`crate::MlsError::InvalidGroupId`] if `other_user_id`
    /// contains storage-hostile characters (the session id is a raw storage
    /// key).
    pub fn get_session_id(&self, other_user_id: &str) -> Result<GroupId> {
        GroupId::for_session(&self.user_id, other_user_id).map_err(MlsError::from)
    }

    /// Checks if a session exists with another user.
    pub fn has_session(&self, other_user_id: &str) -> Result<bool> {
        let session_id = self.get_session_id(other_user_id)?;
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
        let session_id = self.get_session_id(other_user_id)?;

        if self.group_manager.load_group(&session_id)?.is_some() {
            return Err(MlsError::GroupCreation(format!(
                "Session already exists with user: {}",
                other_user_id
            )));
        }

        let mut group =
            self.group_manager
                .create_group(&session_id, credential_with_key, signer)?;

        let (_commit, welcome) =
            self.group_manager
                .add_member(&mut group, their_key_package, signer)?;

        self.group_manager.save_group(&session_id, &group)?;

        let welcome_bytes = welcome
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        info!(
            session_id = %session_id,
            other_user = %other_user_id,
            "Created new 1:1 session"
        );

        self.restart_seal_progress(&session_id);
        Ok(WelcomeMessage {
            group_id: session_id,
            welcome_data: welcome_bytes,
            inviter_id: self.user_id.clone(),
            group_name: None,
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        })
    }

    /// Verifies a Welcome's `group_id` names the `(self, inviter)` session slot
    /// — the only 1:1 slot an authenticated inviter may install (SEC-M6) — and
    /// returns the mismatch error **without touching any state**, so callers
    /// can reject a forged/tampered slot before performing any mutation.
    ///
    /// `welcome_msg.group_id` is an attacker-controllable wire field that
    /// becomes the raw storage key for the adopted group, and the inviter is
    /// authenticated one layer up (`handle_welcome_message` drops any Welcome
    /// whose `inviter_id` != the transport-authenticated sender). The only 1:1
    /// slot this Welcome may legitimately install is therefore
    /// `session:<self>:<inviter>`. Without this check an authenticated peer
    /// could name a *third* party's slot (e.g. "session:alice:bob") and, via
    /// the stage-then-replace adopt in `join_group_replacing`, overwrite or
    /// seed the victim's session with that party — hijacking it so the victim's
    /// `encrypt_for_user` output goes to the attacker's group. This mirrors the
    /// invite-side (SEC-M5) and decrypt-side (SEC-M1) identity bindings on the
    /// previously-unguarded join side. The sibling group-Welcome path is
    /// guarded separately by the reserved-namespace check in
    /// [`crate::MlsManager::join_group`], which refuses any `session:`-prefixed
    /// `group_id`. Checked before deserialization so a tampered slot is
    /// rejected regardless of the Welcome blob.
    pub(crate) fn verify_welcome_slot(&self, welcome_msg: &WelcomeMessage) -> Result<()> {
        let expected_id = GroupId::for_session(&self.user_id, &welcome_msg.inviter_id)?;
        if welcome_msg.group_id != expected_id {
            return Err(MlsError::WelcomeIdentityMismatch {
                expected: expected_id.to_string(),
                found: welcome_msg.group_id.to_string(),
            });
        }
        Ok(())
    }

    /// Joins a session using a Welcome message.
    ///
    /// Implements a **non-destructive** "welcome-wins" strategy: if we already
    /// created a session for this peer (both-create race), the incoming Welcome
    /// is staged first and only replaces our group once staging succeeds (see
    /// [`crate::group::GroupManager::join_group_replacing`]). A retransmitted
    /// Welcome we already adopted, or a first-contact key-package race, fails to
    /// stage and leaves the existing converged group intact — so adoption is
    /// idempotent and never bricks a working session.
    pub fn join_session(&self, welcome_msg: &WelcomeMessage) -> Result<GroupInfo> {
        // SEC-M6: bind the Welcome's session slot to the (self, inviter) pair
        // before touching any group state (see `verify_welcome_slot`).
        self.verify_welcome_slot(welcome_msg)?;

        // Deserialize the Welcome from the MlsMessageOut bytes
        let mls_msg = MlsMessageIn::tls_deserialize_exact(&welcome_msg.welcome_data)
            .map_err(|e| MlsError::Deserialization(e.to_string()))?;

        // Extract the Welcome from the message
        let welcome = match mls_msg.extract() {
            MlsMessageBodyIn::Welcome(w) => w,
            _ => {
                return Err(MlsError::WelcomeProcessing(
                    "Not a Welcome message".to_string(),
                ))
            }
        };

        // Stage-then-swap: replaces any existing group only after the incoming
        // Welcome stages successfully, so a failed stage is a safe no-op.
        let group = self
            .group_manager
            .join_group_replacing(welcome, &welcome_msg.group_id)?;
        let info = self
            .group_manager
            .get_group_info(&group, &welcome_msg.group_id);

        info!(
            session_id = %welcome_msg.group_id,
            inviter = %welcome_msg.inviter_id,
            "Joined/adopted 1:1 session (non-destructive)"
        );

        self.restart_seal_progress(&welcome_msg.group_id);
        Ok(info)
    }

    /// Encrypts a message for the session partner.
    pub fn encrypt_message(
        &self,
        other_user_id: &str,
        plaintext: &[u8],
        signer: &impl Signer,
    ) -> Result<EncryptedMessage> {
        let session_id = self.get_session_id(other_user_id)?;

        let mut group = self
            .group_manager
            .load_group(&session_id)?
            .ok_or_else(|| MlsError::SessionNotFound(other_user_id.to_string()))?;

        let mls_message = self
            .group_manager
            .encrypt_message(&mut group, plaintext, signer)?;

        self.group_manager.save_group(&session_id, &group)?;
        {
            let mut progress = self.progress();
            let entry = progress
                .entry(session_id.to_string())
                .or_insert_with(|| self.fresh_progress());
            entry.sealed = entry.sealed.saturating_add(1);
        }

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
    ///
    /// `claimed_sender` is the transport-level sender this message will be
    /// attributed to; it must match the MLS-authenticated credential.
    ///
    /// The envelope's `group_id` is bound to `claimed_sender` before the group
    /// is loaded, because every failure OpenMLS raises below happens *before*
    /// it authenticates anything — see [`MlsError::SessionIdentityMismatch`].
    /// This is the single chokepoint for the binding: `decrypt_from_user` is
    /// reachable both from the receive path and from the public
    /// `manual_mls_decrypt_from_user` API, so checking it in either caller
    /// alone leaves the other as a bypass.
    pub fn decrypt_message(
        &self,
        encrypted: &EncryptedMessage,
        claimed_sender: &str,
    ) -> Result<Option<Vec<u8>>> {
        let expected_id = GroupId::for_session(&self.user_id, claimed_sender)?;
        if encrypted.group_id != expected_id {
            return Err(MlsError::SessionIdentityMismatch {
                expected: expected_id.to_string(),
                found: encrypted.group_id.to_string(),
            });
        }

        let mut group = self
            .group_manager
            .load_group(&encrypted.group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(encrypted.group_id.to_string()))?;

        let mls_message = MlsMessageIn::tls_deserialize_exact(&encrypted.ciphertext)
            .map_err(|e| MlsError::Deserialization(e.to_string()))?;

        let result = self.group_manager.decrypt_message(
            &mut group,
            &encrypted.group_id,
            mls_message,
            claimed_sender,
        )?;

        self.group_manager.save_group(&encrypted.group_id, &group)?;

        Ok(result)
    }

    /// Lists the peers this device has a 1:1 session with.
    ///
    /// A session id is `session:<a>:<b>` where exactly one of the two halves
    /// is us — ids cannot contain `:` (`GroupId::validate` runs the storage-key
    /// charset check per segment), so the split is unambiguous. Anything that
    /// does not have that exact shape is skipped rather than guessed at: a slot
    /// naming two other parties has no "other peer" to report, and returning
    /// one half of it anyway would surface a peer this device has no session
    /// with.
    pub fn list_sessions(&self) -> Result<Vec<String>> {
        let sessions: Vec<String> = self
            .group_manager
            .list_groups()?
            .into_iter()
            .filter_map(|g| {
                let parts: Vec<&str> = g.as_str().split(':').collect();
                match parts.as_slice() {
                    ["session", a, b] if *a == self.user_id => Some(b.to_string()),
                    ["session", a, b] if *b == self.user_id => Some(a.to_string()),
                    _ => None,
                }
            })
            .collect();

        Ok(sessions)
    }

    /// Deletes a session.
    pub fn delete_session(&self, other_user_id: &str) -> Result<()> {
        let session_id = self.get_session_id(other_user_id)?;
        self.group_manager.delete_group(&session_id)?;
        self.restart_seal_progress(&session_id);
        debug!(session_id = %session_id, "Deleted session");
        Ok(())
    }

    /// Gets information about a session.
    pub fn get_session_info(&self, other_user_id: &str) -> Result<Option<GroupInfo>> {
        let session_id = self.get_session_id(other_user_id)?;
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
        let id1 = manager.get_session_id("bob").unwrap();
        let id2 = GroupId::for_session("bob", "alice").unwrap();
        assert_eq!(id1, id2);
        assert!(id1.as_str().starts_with("session:"));
    }

    #[test]
    fn test_no_session_initially() {
        let manager = create_test_session_manager("alice");
        assert!(!manager.has_session("bob").unwrap());
        assert!(manager.list_sessions().unwrap().is_empty());
    }

    #[test]
    fn test_session_id_is_deterministic() {
        // Session IDs should be the same regardless of which user creates them
        let alice_manager = create_test_session_manager("alice");
        let bob_manager = create_test_session_manager("bob");

        let alice_to_bob = alice_manager.get_session_id("bob").unwrap();
        let bob_to_alice = bob_manager.get_session_id("alice").unwrap();

        // Both should produce the same session ID
        assert_eq!(alice_to_bob, bob_to_alice);

        // The session ID should have a consistent format with sorted user IDs
        assert_eq!(alice_to_bob.as_str(), "session:alice:bob");
    }

    #[test]
    fn test_delete_session() {
        let manager = create_test_session_manager("alice");

        // Initially no session
        assert!(!manager.has_session("bob").unwrap());

        // Delete non-existent session should not panic (delete_group handles this)
        // This is important for welcome-wins where we delete before knowing if exists
        let result = manager.delete_session("bob");
        // Should succeed even if nothing to delete
        assert!(result.is_ok());
    }
}
