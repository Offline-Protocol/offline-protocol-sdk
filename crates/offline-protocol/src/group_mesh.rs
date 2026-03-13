//! Mesh group messaging — MLS-encrypted, transport-agnostic group operations.
//!
//! This module implements group creation, member invite/remove/leave,
//! encrypted message fan-out, commit distribution, and pending commit
//! buffering for out-of-order delivery over mesh networks.

use crate::protocol::{
    base64_decode, base64_encode, internal_prefixes, lock_shared_state, OfflineProtocol,
};
use crate::{Error, Event, Result};
use chrono::Utc;
use offline_protocol_core::{Message, MessageId, MessagePriority};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{Duration as StdDuration, Instant};
use tracing::{debug, error, info, warn};

/// TTL for group message dedup entries (5 minutes).
pub(super) const GROUP_MESSAGE_DEDUP_TTL_SECS: u64 = 300;
/// Maximum number of group message dedup entries before forced cleanup.
pub(super) const MAX_GROUP_MESSAGE_DEDUP_ENTRIES: usize = 10_000;
/// Maximum allowed base64-encoded payload size for incoming group messages (1 MB).
pub(crate) const MAX_BASE64_PAYLOAD_SIZE: usize = 1_048_576;
/// Maximum number of buffered out-of-order commits per group.
const MAX_PENDING_COMMITS_PER_GROUP: usize = 8;
/// TTL for buffered pending commits (2 minutes).
const PENDING_COMMIT_TTL_SECS: u64 = 120;
/// Maximum plaintext content size for outgoing group messages (512 KB).
/// After MLS encryption + base64, the wire payload must stay within
/// `MAX_BASE64_PAYLOAD_SIZE` (1 MB). 512 KB plaintext leaves headroom for MLS
/// framing overhead and the base64 expansion (~33%).
pub(super) const MAX_GROUP_CONTENT_LENGTH: usize = 524_288;
/// Maximum number of members allowed in a single mesh group.
const MAX_GROUP_MEMBERS: usize = 256;

// --- Group (mesh/MLS) payloads ---

/// Payload for MLS-encrypted group messages sent via mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMlsMessagePayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// Base64-encoded MLS ciphertext.
    pub(crate) ciphertext: String,
    /// MLS epoch at which the message was encrypted.
    pub(crate) epoch: u64,
    /// Optional reply-to message ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reply_to: Option<String>,
}

/// Payload for MLS Welcome messages (group invites) sent via mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMlsWelcomePayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// Human-readable group name.
    pub(crate) group_name: Option<String>,
    /// Base64-encoded MLS Welcome data.
    pub(crate) welcome_data: String,
    /// Current member list (user IDs) at the time of invite.
    pub(crate) member_list: Vec<String>,
}

/// Type of group membership commit operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum GroupCommitType {
    /// A member was added to the group.
    Add,
    /// A member was removed from the group.
    Remove,
}

/// Payload for MLS Commit messages (membership changes) sent via mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMlsCommitPayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// Type of commit operation.
    pub(crate) commit_type: GroupCommitType,
    /// Base64-encoded MLS commit ciphertext.
    pub(crate) ciphertext: String,
    /// MLS epoch at which the commit was created.
    pub(crate) epoch: u64,
    /// User ID of the affected member (added or removed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) affected_member: Option<String>,
}

/// Payload for group leave notifications sent via mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMlsLeavePayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// User ID of the leaving member.
    pub(crate) leaving_member: String,
}

/// A commit that arrived out-of-order and is waiting to be processed.
///
/// In mesh networks, messages can arrive out of order. If a Commit arrives
/// before a prior Commit, MLS decryption will fail. We buffer it here and
/// retry after successfully processing a later commit for the same group.
#[derive(Debug, Clone)]
pub(crate) struct PendingCommit {
    /// The original sender of this commit.
    sender: String,
    /// The raw JSON data (after prefix strip) for replay.
    data: String,
    /// When this pending commit was first buffered.
    buffered_at: Instant,
}

impl OfflineProtocol {
    /// Handles an incoming MLS-encrypted group message.
    pub(crate) fn handle_group_mls_msg(&mut self, message: &Message, sender: &str, data: &str) {
        let payload = match serde_json::from_str::<GroupMlsMessagePayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupMlsMessage payload");
                return;
            }
        };

        // Dedup check using the unique message ID
        let dedup_key = message.id.as_str().to_string();
        if self.group_message_dedup.contains_key(&dedup_key) {
            debug!(
                group_id = %payload.group_id,
                msg_id = %dedup_key,
                "Duplicate group message, skipping"
            );
            return;
        }

        // Mark as seen BEFORE attempting decode/decrypt to prevent replay
        // amplification: an adversary replaying the same message (even with a bad
        // epoch) should only trigger one MLS crypto operation.
        self.group_message_dedup.insert(dedup_key, Instant::now());
        if self.group_message_dedup.len() > MAX_GROUP_MESSAGE_DEDUP_ENTRIES {
            self.cleanup_group_message_dedup();
        }

        // Size guard before base64 decode
        let ciphertext_bytes = match base64_decode(&payload.ciphertext) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Failed to decode group message ciphertext");
                return;
            }
        };

        // Decrypt via MLS
        let mls_guard = match self.read_mls_guard() {
            Ok(guard) => guard,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "MLS unavailable, dropping group message");
                return;
            }
        };
        let gid = offline_protocol_mls::GroupId::new(&payload.group_id);
        let encrypted = offline_protocol_mls::EncryptedMessage {
            group_id: gid,
            message_type: offline_protocol_mls::MlsMessageType::Application,
            epoch: payload.epoch,
            ciphertext: ciphertext_bytes,
            sender_id: sender.to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        };
        let decrypt_result = match mls_guard.decrypt_from_group(&encrypted) {
            Ok(Some(plaintext)) => Some(plaintext),
            Ok(None) => {
                // Ok(None) means MLS consumed a Commit or Proposal — not application
                // data. This is normal for non-application messages that arrive via the
                // group message channel (e.g., due to message reordering).
                debug!(
                    group_id = %payload.group_id,
                    "MLS returned no plaintext (commit/proposal consumed), not an application message"
                );
                None
            }
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Failed to decrypt group message");
                None
            }
        };
        drop(mls_guard);

        if let Some(plaintext) = decrypt_result {
            let text = String::from_utf8_lossy(&plaintext).to_string();
            let msg_id = message.id.as_str().to_string();
            let timestamp = chrono::Utc::now().to_rfc3339();
            info!(group_id = %payload.group_id, "Decrypted mesh group message");
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::group_message_received(
                    payload.group_id,
                    sender.to_string(),
                    text,
                    timestamp,
                    msg_id,
                    payload.reply_to,
                ));
            }
        }
    }

    /// Handles an incoming MLS Welcome message (group invite).
    pub(crate) fn handle_group_mls_welcome(&mut self, sender: &str, data: &str) {
        let payload = match serde_json::from_str::<GroupMlsWelcomePayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupMlsWelcome payload");
                return;
            }
        };

        info!(group_id = %payload.group_id, "Received mesh group Welcome");

        let welcome_bytes = match base64_decode(&payload.welcome_data) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Failed to decode welcome data");
                return;
            }
        };

        // Join group via MLS, then update cache
        let mls_guard = match self.read_mls_guard() {
            Ok(guard) => guard,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "MLS unavailable, dropping group welcome");
                return;
            }
        };
        let welcome = offline_protocol_mls::WelcomeMessage {
            group_id: offline_protocol_mls::GroupId::new(&payload.group_id),
            welcome_data: welcome_bytes,
            inviter_id: sender.to_string(),
            group_name: payload.group_name.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        };
        let join_result = match mls_guard.join_group(&welcome) {
            Ok(group_info) => Some(group_info.members.clone()),
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Failed to join mesh group");
                None
            }
        };
        drop(mls_guard);

        if let Some(members) = join_result {
            let group_id = payload.group_id.clone();
            self.group_members.insert(group_id.clone(), members);
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::group_member_added(
                    group_id,
                    self.config.user_id.clone(),
                    sender.to_string(),
                ));
            }
        }
    }

    /// Handles an incoming MLS Commit message (membership change).
    ///
    /// Validates the `affected_member` claim against actual MLS state delta
    /// to prevent forged membership events.
    ///
    /// If decryption fails (e.g., due to out-of-order delivery in a mesh network),
    /// the commit is buffered for deferred retry. When a subsequent commit for the
    /// same group succeeds, buffered commits are drained and retried.
    pub(crate) fn handle_group_mls_commit(&mut self, sender: &str, data: &str) {
        if let Some(group_id) = self.try_process_commit(sender, data) {
            // Success — drain any buffered commits for this group
            self.drain_pending_commits(&group_id);
        }
    }

    /// Attempts to process a single MLS Commit. Returns the group ID on success.
    ///
    /// On failure, buffers the commit for later retry unless the payload is
    /// unparseable or the ciphertext is invalid.
    fn try_process_commit(&mut self, sender: &str, data: &str) -> Option<String> {
        let payload = match serde_json::from_str::<GroupMlsCommitPayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupMlsCommit payload");
                return None;
            }
        };

        info!(
            group_id = %payload.group_id,
            commit_type = ?payload.commit_type,
            "Received mesh group Commit"
        );

        if payload.ciphertext.is_empty() {
            warn!(
                group_id = %payload.group_id,
                "Received Commit with empty ciphertext, cannot advance MLS epoch"
            );
            return None;
        }

        let ciphertext_bytes = match base64_decode(&payload.ciphertext) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Failed to decode commit ciphertext");
                return None;
            }
        };

        let mls_guard = match self.read_mls_guard() {
            Ok(guard) => guard,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "MLS unavailable, dropping group commit");
                return None;
            }
        };

        let gid = offline_protocol_mls::GroupId::new(&payload.group_id);

        // Capture members before commit for delta validation
        let members_before: HashSet<String> = mls_guard
            .get_group_info(&gid)
            .ok()
            .flatten()
            .map(|info| info.members.into_iter().collect())
            .unwrap_or_default();

        // Process Commit via MLS to advance epoch (single lock acquisition)
        let encrypted = offline_protocol_mls::EncryptedMessage {
            group_id: gid,
            message_type: offline_protocol_mls::MlsMessageType::Commit,
            epoch: payload.epoch,
            ciphertext: ciphertext_bytes,
            sender_id: sender.to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        };
        let mls_ok = match mls_guard.decrypt_from_group(&encrypted) {
            Ok(_) => true,
            Err(e) => {
                error!(
                    group_id = %payload.group_id,
                    epoch = payload.epoch,
                    error = %e,
                    "Failed to process group commit — buffering for deferred retry"
                );
                false
            }
        };
        drop(mls_guard);

        if !mls_ok {
            // Buffer for retry — the commit may have arrived before a prior commit
            let pending = PendingCommit {
                sender: sender.to_string(),
                data: data.to_string(),
                buffered_at: Instant::now(),
            };
            let buf = self
                .pending_commits
                .entry(payload.group_id.clone())
                .or_default();
            if buf.len() < MAX_PENDING_COMMITS_PER_GROUP {
                buf.push(pending);
                debug!(
                    group_id = %payload.group_id,
                    buffered_count = buf.len(),
                    "Buffered out-of-order commit for deferred retry"
                );
            } else {
                warn!(
                    group_id = %payload.group_id,
                    "Pending commit buffer full, dropping oldest"
                );
                buf.remove(0);
                buf.push(pending);
            }
            return None;
        }

        // Refresh cache and compute actual membership delta
        let _ = self.refresh_group_members(&payload.group_id);
        let members_after: HashSet<String> = self
            .group_members
            .get(&payload.group_id)
            .map(|m| m.iter().cloned().collect())
            .unwrap_or_default();

        let actual_added: HashSet<&String> = members_after.difference(&members_before).collect();
        let actual_removed: HashSet<&String> = members_before.difference(&members_after).collect();

        // Validate claimed affected_member against actual MLS delta.
        // A mismatch may indicate a forged commit metadata — log at error level.
        if let Some(claimed) = &payload.affected_member {
            let valid = match payload.commit_type {
                GroupCommitType::Add => actual_added.contains(claimed),
                GroupCommitType::Remove => actual_removed.contains(claimed),
            };
            if !valid && (!actual_added.is_empty() || !actual_removed.is_empty()) {
                error!(
                    group_id = %payload.group_id,
                    sender = %sender,
                    claimed = %claimed,
                    actual_added = ?actual_added,
                    actual_removed = ?actual_removed,
                    "SECURITY: Commit affected_member does not match actual MLS state delta — possible forgery"
                );
            }
        }

        // Emit events based on actual MLS membership changes, not claimed affected_member
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            for member in &actual_added {
                state.emit_event(Event::group_member_added(
                    payload.group_id.clone(),
                    (*member).clone(),
                    sender.to_string(),
                ));
            }
            for member in &actual_removed {
                state.emit_event(Event::group_member_removed(
                    payload.group_id.clone(),
                    (*member).clone(),
                    sender.to_string(),
                ));
            }
        }

        Some(payload.group_id)
    }

    /// Drains and retries buffered pending commits for a group after a
    /// successful commit advanced the epoch. Each successful retry triggers
    /// another drain pass (at most `MAX_PENDING_COMMITS_PER_GROUP` iterations).
    fn drain_pending_commits(&mut self, group_id: &str) {
        // Limit iterations to avoid unbounded looping
        for _ in 0..MAX_PENDING_COMMITS_PER_GROUP {
            let pending = match self.pending_commits.get_mut(group_id) {
                Some(buf) if !buf.is_empty() => std::mem::take(buf),
                _ => break,
            };

            let mut any_succeeded = false;
            let mut still_pending = Vec::new();

            for entry in pending {
                // Drop expired entries
                if entry.buffered_at.elapsed() > StdDuration::from_secs(PENDING_COMMIT_TTL_SECS) {
                    debug!(
                        group_id = %group_id,
                        "Dropping expired pending commit"
                    );
                    continue;
                }
                if self
                    .try_process_commit(&entry.sender, &entry.data)
                    .is_some()
                {
                    any_succeeded = true;
                } else {
                    still_pending.push(entry);
                }
            }

            // Re-buffer commits that still failed
            if !still_pending.is_empty() {
                self.pending_commits
                    .entry(group_id.to_string())
                    .or_default()
                    .extend(still_pending);
            }

            if !any_succeeded {
                break;
            }
            // Another commit succeeded — loop again in case it unblocked more
        }

        // Clean up empty entries
        if self
            .pending_commits
            .get(group_id)
            .map_or(true, |v| v.is_empty())
        {
            self.pending_commits.remove(group_id);
        }
    }

    /// Handles an incoming group leave notification.
    ///
    /// After verifying the sender, the lexicographically-first remaining member
    /// (deterministic election) issues an MLS remove-commit to advance the group
    /// epoch and revoke the leaving member's keys. Other members will receive
    /// the commit via `handle_group_mls_commit`.
    ///
    /// # Security limitations
    ///
    /// Leave notifications are **not** MLS-authenticated because a member cannot
    /// issue a self-removal Commit in MLS. The `sender` field comes from the
    /// `Message` envelope which is not cryptographically bound. In an adversarial
    /// mesh environment a relay node could forge a leave notification to force-
    /// remove a legitimate member. Mitigations:
    ///
    /// 1. `sender == leaving_member` check prevents cross-member impersonation
    ///    when the transport layer preserves sender identity.
    /// 2. Membership is verified against the **MLS group state** (authoritative),
    ///    not just the local cache.
    /// 3. The elected remover issues a real MLS Commit that all members verify
    ///    cryptographically.
    ///
    /// For fully adversarial environments, consider requiring admin-only removal
    /// or adding an MLS application-message-signed leave proof.
    pub(crate) fn handle_group_mls_leave(&mut self, sender: &str, data: &str) {
        let payload = match serde_json::from_str::<GroupMlsLeavePayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupMlsLeave payload");
                return;
            }
        };

        // Verify sender matches the claimed leaving member to prevent spoofing
        if payload.leaving_member != sender {
            error!(
                sender = %sender,
                claimed = %payload.leaving_member,
                group_id = %payload.group_id,
                "SECURITY: Leave notification sender mismatch — possible spoofing attempt, ignoring"
            );
            return;
        }

        // Verify sender is actually a member of the group using MLS state
        // (authoritative) rather than only the local cache.
        let self_id = self.config.user_id.clone();
        let members = self
            .refresh_group_members(&payload.group_id)
            .ok()
            .or_else(|| self.group_members.get(&payload.group_id).cloned())
            .unwrap_or_default();

        if !members.iter().any(|m| m == sender) {
            error!(
                sender = %sender,
                group_id = %payload.group_id,
                "SECURITY: Leave notification from non-member, ignoring"
            );
            return;
        }

        info!(
            group_id = %payload.group_id,
            leaving_member = %payload.leaving_member,
            "Received mesh group leave notification"
        );

        // Deterministic election: lexicographically-first remaining member
        // (excluding the leaver) issues the MLS remove-commit to advance epoch.
        let should_remove = members
            .iter()
            .filter(|m| m.as_str() != sender)
            .min()
            .map(|first| first == &self_id)
            .unwrap_or(false);

        if should_remove {
            debug!(
                group_id = %payload.group_id,
                leaving_member = %sender,
                "Elected to issue MLS remove-commit for leaving member"
            );
            // Issue MLS remove + distribute commit to advance epoch and revoke keys.
            // remove_from_group emits GroupMemberRemoved, so we skip the event below.
            if let Err(e) = self.remove_from_group(&payload.group_id, sender) {
                warn!(
                    group_id = %payload.group_id,
                    leaving_member = %sender,
                    error = %e,
                    "Failed to issue MLS remove-commit for leaving member"
                );
            }
        }

        // Only emit the leave event if we didn't already emit via remove_from_group
        if !should_remove {
            let _ = self.refresh_group_members(&payload.group_id);
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::group_member_removed(
                    payload.group_id,
                    payload.leaving_member.clone(),
                    payload.leaving_member,
                ));
            }
        }
    }

    /// Refreshes the cached member list for a group from MlsManager.
    fn refresh_group_members(&mut self, group_id: &str) -> Result<Vec<String>> {
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id);
        let info = mls_guard
            .get_group_info(&gid)?
            .ok_or_else(|| Error::Other(format!("Group not found: {}", group_id)))?;
        let members = info.members.clone();
        drop(mls_guard);
        self.group_members
            .insert(group_id.to_string(), members.clone());
        Ok(members)
    }

    /// Creates a new MLS group for mesh messaging.
    ///
    /// The group is created locally via MLS. Members can be invited with
    /// `invite_to_group()`. Messages sent via `send_group_message()` are
    /// MLS-encrypted and fan-out via DORS to each member individually.
    pub fn create_mesh_group(
        &mut self,
        group_name: &str,
    ) -> Result<offline_protocol_mls::GroupInfo> {
        let mls_guard = self.read_mls_guard()?;
        let group_info = mls_guard.create_group(group_name)?;
        let group_id = group_info.group_id.as_str().to_string();
        let members = group_info.members.clone();
        drop(mls_guard);

        self.group_members.insert(group_id.clone(), members);

        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::group_created(group_id, group_name.to_string()));
        }

        Ok(group_info)
    }

    /// Invites a user to an MLS mesh group.
    ///
    /// Requires the invitee's key package to be available in `pending_key_packages`.
    /// Sends a Welcome to the invitee and a Commit to all existing members.
    pub fn invite_to_group(&mut self, group_id: &str, invitee_user_id: &str) -> Result<()> {
        // Check group member cap before adding
        let current_count = self
            .group_members
            .get(group_id)
            .map(|m| m.len())
            .or_else(|| {
                self.read_mls_guard()
                    .ok()
                    .and_then(|g| {
                        g.get_group_info(&offline_protocol_mls::GroupId::new(group_id))
                            .ok()
                            .flatten()
                            .map(|info| info.members.len())
                    })
            })
            .unwrap_or(0);
        if current_count >= MAX_GROUP_MEMBERS {
            return Err(Error::Other(format!(
                "Group has {} members, cannot exceed {} limit",
                current_count, MAX_GROUP_MEMBERS
            )));
        }

        // Get the invitee's key package and check expiry
        let now_ms = Utc::now().timestamp_millis() as u64;
        let received_pkg = self
            .pending_key_packages
            .get(invitee_user_id)
            .ok_or_else(|| {
                Error::Other(format!("No key package available for {}", invitee_user_id))
            })?;
        if now_ms >= received_pkg.local_expires_at_ms {
            self.pending_key_packages.remove(invitee_user_id);
            return Err(Error::Other(format!(
                "Key package for {} has expired",
                invitee_user_id
            )));
        }
        let key_pkg = received_pkg.key_package_data.clone();

        // Add member via MLS — returns both Welcome (for invitee) and Commit (for existing members)
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id);
        let (welcome, commit) = mls_guard.add_group_member(&gid, &key_pkg)?;
        let group_name = welcome.group_name.clone();
        drop(mls_guard);

        // Refresh member list after add
        let members = self.refresh_group_members(group_id)?;

        // Send Welcome to invitee
        let welcome_payload = GroupMlsWelcomePayload {
            group_id: group_id.to_string(),
            group_name,
            welcome_data: base64_encode(&welcome.welcome_data),
            member_list: members.clone(),
        };
        let welcome_content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_WELCOME,
            serde_json::to_string(&welcome_payload)
                .map_err(|e| Error::Other(format!("Serialize welcome: {}", e)))?
        );
        self.send_internal_message(invitee_user_id, welcome_content, MessagePriority::High)?;

        // Send Commit to all existing members (excluding self and invitee)
        // so they can process it and advance their MLS epoch
        let self_id = self.config.user_id.clone();
        let commit_payload = GroupMlsCommitPayload {
            group_id: group_id.to_string(),
            commit_type: GroupCommitType::Add,
            ciphertext: base64_encode(&commit.ciphertext),
            epoch: commit.epoch,
            affected_member: Some(invitee_user_id.to_string()),
        };
        let commit_content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_COMMIT,
            serde_json::to_string(&commit_payload)
                .map_err(|e| Error::Other(format!("Serialize commit: {}", e)))?
        );
        for member in &members {
            if member == &self_id || member == invitee_user_id {
                continue;
            }
            if let Err(e) =
                self.send_internal_message(member, commit_content.clone(), MessagePriority::High)
            {
                warn!(
                    group_id = %group_id,
                    member = %member,
                    error = %e,
                    "Failed to send commit to group member during invite"
                );
            }
        }

        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::group_member_added(
                group_id.to_string(),
                invitee_user_id.to_string(),
                self_id,
            ));
        }

        info!(group_id = %group_id, invitee = %invitee_user_id, "Invited member to mesh group");
        Ok(())
    }

    /// Removes a member from an MLS mesh group.
    ///
    /// Sends a Commit to all remaining members.
    pub fn remove_from_group(&mut self, group_id: &str, member_id: &str) -> Result<()> {
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id);
        let commit_msg = mls_guard.remove_group_member(&gid, member_id)?;
        drop(mls_guard);

        // Refresh member list after removal
        let members = self.refresh_group_members(group_id)?;

        // Fan-out Commit to remaining members
        let self_id = self.config.user_id.clone();
        let commit_payload = GroupMlsCommitPayload {
            group_id: group_id.to_string(),
            commit_type: GroupCommitType::Remove,
            ciphertext: base64_encode(&commit_msg.ciphertext),
            epoch: commit_msg.epoch,
            affected_member: Some(member_id.to_string()),
        };
        let commit_content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_COMMIT,
            serde_json::to_string(&commit_payload)
                .map_err(|e| Error::Other(format!("Serialize commit: {}", e)))?
        );
        for member in &members {
            if member == &self_id {
                continue;
            }
            if let Err(e) =
                self.send_internal_message(member, commit_content.clone(), MessagePriority::High)
            {
                warn!(
                    group_id = %group_id,
                    member = %member,
                    error = %e,
                    "Failed to send commit to group member during remove"
                );
            }
        }

        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::group_member_removed(
                group_id.to_string(),
                member_id.to_string(),
                self_id,
            ));
        }

        info!(group_id = %group_id, member = %member_id, "Removed member from mesh group");
        Ok(())
    }

    /// Leaves an MLS mesh group.
    ///
    /// Notifies remaining members and then removes local group state.
    /// Note: The MLS layer does not support self-removal Commits, so remaining
    /// members receive a plaintext leave notification. The deterministic election
    /// in `handle_group_mls_leave` will select one member to issue the MLS
    /// remove-commit to properly advance the epoch.
    ///
    /// **Ordering note:** Leave notifications are sent *before* deleting local
    /// MLS state. If all notification sends fail, local state is preserved and
    /// the caller receives an error so the leave can be retried. This prevents
    /// orphaned membership where the leaver is gone locally but peers never
    /// learn about the departure.
    pub fn leave_mesh_group(&mut self, group_id: &str) -> Result<()> {
        // Get members before leaving
        let members = self
            .group_members
            .get(group_id)
            .cloned()
            .or_else(|| self.refresh_group_members(group_id).ok())
            .unwrap_or_default();

        let self_id = self.config.user_id.clone();

        // Build leave notification payload before touching MLS state
        let leave_payload = GroupMlsLeavePayload {
            group_id: group_id.to_string(),
            leaving_member: self_id.clone(),
        };
        let leave_content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_LEAVE,
            serde_json::to_string(&leave_payload)
                .map_err(|e| Error::Other(format!("Serialize leave: {}", e)))?
        );

        // Send notifications first — if all fail, keep local state intact for retry
        let mut any_sent = false;
        let mut had_recipients = false;
        for member in &members {
            if member == &self_id {
                continue;
            }
            had_recipients = true;
            match self.send_internal_message(member, leave_content.clone(), MessagePriority::Medium)
            {
                Ok(_) => {
                    any_sent = true;
                }
                Err(e) => {
                    warn!(
                        group_id = %group_id,
                        member = %member,
                        error = %e,
                        "Failed to send leave notification to group member"
                    );
                }
            }
        }

        // If there were other members but no notification succeeded, fail so the
        // caller can retry rather than silently orphaning the membership.
        if had_recipients && !any_sent {
            return Err(Error::Other(
                "All leave notifications failed — local state preserved for retry".to_string(),
            ));
        }

        // Now safe to delete local MLS state — at least one peer was notified
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id);
        mls_guard.leave_group(&gid)?;
        drop(mls_guard);

        // Remove from cache
        self.group_members.remove(group_id);

        info!(group_id = %group_id, "Left mesh group");
        Ok(())
    }

    /// Sends a message to all members of an MLS mesh group.
    ///
    /// The message is MLS-encrypted once, then fan-out as individual
    /// point-to-point messages via `send_internal_message()`. Each member's
    /// delivery goes through the full DORS/ACK/retry stack independently.
    pub fn send_group_message(
        &mut self,
        group_id: &str,
        content: &str,
        priority: Option<MessagePriority>,
        reply_to_msg: Option<&str>,
    ) -> Result<Vec<MessageId>> {
        if content.len() > MAX_GROUP_CONTENT_LENGTH {
            return Err(Error::Other(format!(
                "Group message content too large: {} bytes exceeds {} limit",
                content.len(),
                MAX_GROUP_CONTENT_LENGTH
            )));
        }

        let priority = priority.unwrap_or(MessagePriority::Medium);

        // Encrypt and read member list while holding &mut self to ensure
        // the member cache is consistent with the MLS encryption epoch.
        let (encrypted, members) = {
            let mls_guard = self.read_mls_guard()?;
            let gid = offline_protocol_mls::GroupId::new(group_id);
            let enc = mls_guard.encrypt_for_group(&gid, content.as_bytes())?;
            let members = match self.group_members.get(group_id) {
                Some(m) => m.clone(),
                None => {
                    let info = mls_guard
                        .get_group_info(&gid)?
                        .ok_or_else(|| Error::Other(format!("Group not found: {}", group_id)))?;
                    info.members.clone()
                }
            };
            (enc, members)
        };

        // Update cache if it was a miss
        if !self.group_members.contains_key(group_id) {
            self.group_members
                .insert(group_id.to_string(), members.clone());
        }

        let ciphertext_b64 = base64_encode(&encrypted.ciphertext);
        let epoch = encrypted.epoch;

        let self_id = self.config.user_id.clone();

        // Build the internal message payload with reply_to as a proper field
        let msg_payload = GroupMlsMessagePayload {
            group_id: group_id.to_string(),
            ciphertext: ciphertext_b64,
            epoch,
            reply_to: reply_to_msg.map(|s| s.to_string()),
        };
        let base_content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_MSG,
            serde_json::to_string(&msg_payload)
                .map_err(|e| Error::Other(format!("Serialize group message: {}", e)))?
        );

        // Fan-out to each member
        let mut message_ids = Vec::new();
        let mut failed_members = Vec::new();
        let mut succeeded_members = Vec::new();

        for member in &members {
            if member == &self_id {
                continue;
            }
            match self.send_internal_message(member, base_content.clone(), priority) {
                Ok(mid) => {
                    message_ids.push(mid);
                    succeeded_members.push(member.clone());
                }
                Err(e) => {
                    warn!(
                        group_id = %group_id,
                        member = %member,
                        error = %e,
                        "Failed to send group message to member"
                    );
                    failed_members.push(member.clone());
                }
            }
        }

        let member_count = succeeded_members.len() as u32;

        // Check for total delivery failure: there were members to send to but
        // every send failed. Return an error so callers don't confuse this with
        // a solo-group scenario (which legitimately returns Ok(vec![])).
        let had_recipients = members.iter().any(|m| m != &self_id);
        if had_recipients && message_ids.is_empty() {
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::group_message_partial_failure(
                    group_id.to_string(),
                    failed_members,
                    succeeded_members,
                ));
            }
            return Err(Error::Other("All group message sends failed".to_string()));
        }

        // Emit appropriate event
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            if failed_members.is_empty() {
                state.emit_event(Event::group_message_sent(
                    group_id.to_string(),
                    message_ids.iter().map(|m| m.as_str().to_string()).collect(),
                    member_count,
                ));
            } else {
                state.emit_event(Event::group_message_partial_failure(
                    group_id.to_string(),
                    failed_members,
                    succeeded_members,
                ));
            }
        }

        Ok(message_ids)
    }

    /// Lists all MLS mesh groups (excluding 1:1 sessions).
    pub fn list_mesh_groups(&self) -> Result<Vec<String>> {
        let mls_guard = self.read_mls_guard()?;
        let groups = mls_guard.list_groups()?;
        Ok(groups.into_iter().map(|g| g.as_str().to_string()).collect())
    }

    /// Cleans up expired group message dedup entries and enforces size cap.
    pub(crate) fn cleanup_group_message_dedup(&mut self) {
        let cutoff = Instant::now() - StdDuration::from_secs(GROUP_MESSAGE_DEDUP_TTL_SECS);
        self.group_message_dedup
            .retain(|_, seen_at| *seen_at > cutoff);
        // If still over cap after TTL cleanup, drop oldest entries using O(N) selection
        let len = self.group_message_dedup.len();
        if len > MAX_GROUP_MESSAGE_DEDUP_ENTRIES {
            let mut entries: Vec<_> = self.group_message_dedup.drain().collect();
            // Partition so the newest MAX entries are in [..MAX]
            entries.select_nth_unstable_by_key(MAX_GROUP_MESSAGE_DEDUP_ENTRIES, |(_, ts)| {
                std::cmp::Reverse(*ts)
            });
            entries.truncate(MAX_GROUP_MESSAGE_DEDUP_ENTRIES);
            self.group_message_dedup = entries.into_iter().collect();
        }

        // Expire stale pending commits
        let commit_cutoff = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS);
        self.pending_commits.retain(|_, commits| {
            commits.retain(|c| c.buffered_at > commit_cutoff);
            !commits.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::tests::{create_test_config, create_test_config_for_user};
    use crate::protocol::InternalMessageResult;
    use offline_protocol_core::{AppId, UserId};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    #[test]
    fn test_group_mls_create_mesh_group_requires_mls() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        // Without MLS initialization, create_mesh_group should fail
        let result = protocol.create_mesh_group("Test Group");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("MLS not initialized"),
            "Expected MLS not initialized error"
        );
    }

    #[test]
    fn test_group_mls_create_mesh_group_with_mls() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let group_info = protocol.create_mesh_group("Test Group").unwrap();
        assert_eq!(group_info.name, Some("Test Group".to_string()));
        assert!(group_info.group_id.as_str().starts_with("group:"));
        assert!(group_info.members.contains(&"user123".to_string()));

        // Verify group is cached
        let cached = protocol.group_members.get(group_info.group_id.as_str());
        assert!(cached.is_some());
        assert!(cached.unwrap().contains(&"user123".to_string()));
    }

    #[test]
    fn test_group_mls_list_mesh_groups() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();

        // Initially no groups
        let groups = protocol.list_mesh_groups().unwrap();
        assert!(groups.is_empty());

        // Create a group
        let info = protocol.create_mesh_group("My Group").unwrap();
        let groups = protocol.list_mesh_groups().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], info.group_id.as_str());
    }

    #[test]
    fn test_group_mls_send_message_requires_mls() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let result = protocol.send_group_message("some-group", "hello", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_group_mls_send_message_group_not_found() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();

        // Sending to non-existent group should fail
        let result = protocol.send_group_message("nonexistent-group", "hello", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_group_mls_send_message_solo_group() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        // Create group (only self is a member)
        let info = protocol.create_mesh_group("Solo Group").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Sending in a solo group should succeed but produce no message IDs
        // (no other members to fan out to)
        let result = protocol.send_group_message(&group_id, "hello", None, None);
        assert!(result.is_ok());
        let message_ids = result.unwrap();
        assert!(message_ids.is_empty(), "No messages should be sent to self");
    }

    #[test]
    fn test_group_mls_leave_mesh_group() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let info = protocol.create_mesh_group("Leave Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Verify group exists in cache
        assert!(protocol.group_members.contains_key(&group_id));

        // Leave the group
        protocol.leave_mesh_group(&group_id).unwrap();

        // Verify group removed from cache
        assert!(!protocol.group_members.contains_key(&group_id));
    }

    #[test]
    fn test_group_mls_invite_requires_key_package() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let info = protocol.create_mesh_group("Invite Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Inviting without key package should fail
        let result = protocol.invite_to_group(&group_id, "bob");
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("No key package"),
            "Expected no key package error"
        );
    }

    #[test]
    fn test_group_mls_dedup_cache() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        // Insert dedup entry using message ID as key
        let key = "msg-001".to_string();
        protocol
            .group_message_dedup
            .insert(key.clone(), Instant::now());
        assert!(protocol.group_message_dedup.contains_key(&key));

        // Cleanup should keep recent entries
        protocol.cleanup_group_message_dedup();
        assert!(protocol.group_message_dedup.contains_key(&key));

        // Insert old entry and verify cleanup removes it
        let old_key = "msg-002".to_string();
        protocol.group_message_dedup.insert(
            old_key.clone(),
            Instant::now() - StdDuration::from_secs(GROUP_MESSAGE_DEDUP_TTL_SECS + 1),
        );
        protocol.cleanup_group_message_dedup();
        assert!(!protocol.group_message_dedup.contains_key(&old_key));
        assert!(protocol.group_message_dedup.contains_key(&key));
    }

    #[test]
    fn test_group_mls_process_leave_message() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Pre-populate group member cache
        protocol.group_members.insert(
            "group:test-123".to_string(),
            vec![
                "user123".to_string(),
                "alice".to_string(),
                "bob".to_string(),
            ],
        );

        // Simulate receiving a leave message
        let leave_payload = GroupMlsLeavePayload {
            group_id: "group:test-123".to_string(),
            leaving_member: "alice".to_string(),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_LEAVE,
            serde_json::to_string(&leave_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // Check that GroupMemberRemoved event was emitted
        let events = events.lock().unwrap();
        let leave_event = events.iter().find(|e| {
            matches!(e, Event::GroupMemberRemoved { group_id, user_id, .. }
                if group_id == "group:test-123" && user_id == "alice")
        });
        assert!(leave_event.is_some(), "Expected GroupMemberRemoved event");
    }

    #[test]
    fn test_group_mls_process_commit_empty_ciphertext_no_event() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Create a group first so refresh_group_members can find it
        let info = protocol.create_mesh_group("Commit Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Simulate receiving a commit "add" message with empty ciphertext.
        // MLS processing will fail, so no membership event should be emitted.
        let commit_payload = GroupMlsCommitPayload {
            group_id: group_id.clone(),
            commit_type: GroupCommitType::Add,
            ciphertext: String::new(),
            epoch: 1,
            affected_member: Some("carol".to_string()),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_COMMIT,
            serde_json::to_string(&commit_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // No membership event should be emitted since MLS processing failed
        let events = events.lock().unwrap();
        let add_event = events.iter().find(|e| {
            matches!(e, Event::GroupMemberAdded { group_id: gid, user_id, .. }
                if gid == &group_id && user_id == "carol")
        });
        assert!(
            add_event.is_none(),
            "Should NOT emit GroupMemberAdded when MLS commit processing fails"
        );
    }

    #[test]
    fn test_group_mls_refresh_group_members() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();

        // Create group
        let info = protocol.create_mesh_group("Refresh Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // refresh_group_members should populate cache
        protocol.group_members.clear();
        let members = protocol.refresh_group_members(&group_id).unwrap();
        assert!(members.contains(&"user123".to_string()));
        assert!(protocol.group_members.contains_key(&group_id));
    }

    #[test]
    fn test_group_mls_group_events_emitted_on_create() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        protocol.create_mesh_group("Event Test").unwrap();

        let events = events.lock().unwrap();
        let created_event = events
            .iter()
            .find(|e| matches!(e, Event::GroupCreated { name, .. } if name == "Event Test"));
        assert!(created_event.is_some(), "Expected GroupCreated event");
    }

    #[test]
    fn test_group_mls_cleanup_includes_group_dedup() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        // Add an expired dedup entry
        protocol.group_message_dedup.insert(
            "msg-expired-001".to_string(),
            Instant::now() - StdDuration::from_secs(GROUP_MESSAGE_DEDUP_TTL_SECS + 1),
        );
        assert_eq!(protocol.group_message_dedup.len(), 1);

        // cleanup_expired_entries should clean it up
        protocol.cleanup_expired_entries();
        assert!(protocol.group_message_dedup.is_empty());
    }

    #[test]
    fn test_group_mls_internal_prefixes_defined() {
        // Verify all new prefixes are unique and well-formed
        assert!(internal_prefixes::GROUP_MLS_MSG.starts_with("__"));
        assert!(internal_prefixes::GROUP_MLS_MSG.ends_with("__"));
        assert!(internal_prefixes::GROUP_MLS_WELCOME.starts_with("__"));
        assert!(internal_prefixes::GROUP_MLS_COMMIT.starts_with("__"));
        assert!(internal_prefixes::GROUP_MLS_LEAVE.starts_with("__"));

        // All prefixes are distinct
        let prefixes = [
            internal_prefixes::GROUP_MLS_MSG,
            internal_prefixes::GROUP_MLS_WELCOME,
            internal_prefixes::GROUP_MLS_COMMIT,
            internal_prefixes::GROUP_MLS_LEAVE,
        ];
        for (i, a) in prefixes.iter().enumerate() {
            for (j, b) in prefixes.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "Prefixes must be unique");
                    assert!(!a.starts_with(b), "No prefix should be a prefix of another");
                }
            }
        }
    }

    #[test]
    fn test_group_mls_payload_serialization_roundtrip() {
        // Message payload with reply_to and epoch
        let msg_payload = GroupMlsMessagePayload {
            group_id: "group:abc".to_string(),
            ciphertext: "dGVzdA==".to_string(),
            epoch: 42,
            reply_to: Some("msg-123".to_string()),
        };
        let json = serde_json::to_string(&msg_payload).unwrap();
        let parsed: GroupMlsMessagePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.group_id, "group:abc");
        assert_eq!(parsed.ciphertext, "dGVzdA==");
        assert_eq!(parsed.epoch, 42);
        assert_eq!(parsed.reply_to, Some("msg-123".to_string()));

        // Message payload without reply_to (should not include field in JSON)
        let msg_no_reply = GroupMlsMessagePayload {
            group_id: "group:abc".to_string(),
            ciphertext: "dGVzdA==".to_string(),
            epoch: 1,
            reply_to: None,
        };
        let json_no_reply = serde_json::to_string(&msg_no_reply).unwrap();
        assert!(!json_no_reply.contains("reply_to"));
        let parsed_no_reply: GroupMlsMessagePayload = serde_json::from_str(&json_no_reply).unwrap();
        assert_eq!(parsed_no_reply.reply_to, None);
        assert_eq!(parsed_no_reply.epoch, 1);

        let welcome_payload = GroupMlsWelcomePayload {
            group_id: "group:def".to_string(),
            group_name: Some("Test Group".to_string()),
            welcome_data: "d2VsY29tZQ==".to_string(),
            member_list: vec!["alice".to_string(), "bob".to_string()],
        };
        let json = serde_json::to_string(&welcome_payload).unwrap();
        let parsed: GroupMlsWelcomePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.group_id, "group:def");
        assert_eq!(parsed.group_name, Some("Test Group".to_string()));
        assert_eq!(parsed.member_list.len(), 2);

        let commit_payload = GroupMlsCommitPayload {
            group_id: "group:ghi".to_string(),
            commit_type: GroupCommitType::Add,
            ciphertext: "Y29tbWl0".to_string(),
            epoch: 5,
            affected_member: Some("carol".to_string()),
        };
        let json = serde_json::to_string(&commit_payload).unwrap();
        let parsed: GroupMlsCommitPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.commit_type, GroupCommitType::Add);
        assert_eq!(parsed.affected_member, Some("carol".to_string()));
        assert_eq!(parsed.epoch, 5);

        // Verify enum serializes to lowercase
        assert!(json.contains("\"add\""));

        let leave_payload = GroupMlsLeavePayload {
            group_id: "group:jkl".to_string(),
            leaving_member: "dave".to_string(),
        };
        let json = serde_json::to_string(&leave_payload).unwrap();
        let parsed: GroupMlsLeavePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.leaving_member, "dave");
    }

    #[test]
    fn test_group_mls_leave_sender_mismatch_rejected() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Simulate a spoofed leave: sender is "bob" but claims "alice" left
        let leave_payload = GroupMlsLeavePayload {
            group_id: "group:test-123".to_string(),
            leaving_member: "alice".to_string(),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_LEAVE,
            serde_json::to_string(&leave_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("bob").unwrap(), // sender != leaving_member
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // No event should be emitted for spoofed leave
        let events = events.lock().unwrap();
        let leave_event = events
            .iter()
            .find(|e| matches!(e, Event::GroupMemberRemoved { .. }));
        assert!(leave_event.is_none(), "Spoofed leave should not emit event");
    }

    #[test]
    fn test_group_mls_full_lifecycle_create_invite_send_decrypt() {
        // Two protocol instances: Alice creates a group, invites Bob,
        // sends a message, and Bob decrypts it.
        let storage_a = Arc::new(crate::mls::InMemoryStorage::default());
        let storage_b = Arc::new(crate::mls::InMemoryStorage::default());
        let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
        alice.initialize_mls(storage_a).unwrap();
        bob.initialize_mls(storage_b).unwrap();
        alice.start().unwrap();
        bob.start().unwrap();

        // Alice creates a group
        let group_info = alice.create_mesh_group("Integration Test Group").unwrap();
        let group_id = group_info.group_id.as_str().to_string();

        // Bob generates a key package
        let bob_kp = {
            let bob_mls = bob.mls_manager.as_ref().unwrap().read().unwrap();
            bob_mls.generate_key_package().unwrap()
        };

        // Alice adds Bob to the group at the MLS layer directly
        let (welcome, _commit) = {
            let alice_mls = alice.mls_manager.as_ref().unwrap().read().unwrap();
            let gid = offline_protocol_mls::GroupId::new(&group_id);
            alice_mls
                .add_group_member(&gid, &bob_kp.key_package_data)
                .unwrap()
        };

        // Bob joins the group via the Welcome
        {
            let bob_mls = bob.mls_manager.as_ref().unwrap().read().unwrap();
            bob_mls.join_group(&welcome).unwrap();
        }

        // Update member caches
        alice.refresh_group_members(&group_id).unwrap();
        bob.group_members.insert(
            group_id.clone(),
            vec!["alice".to_string(), "bob".to_string()],
        );

        // Set up Bob's event capture
        let bob_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let bob_events_clone = bob_events.clone();
        bob.on_event(move |event| {
            bob_events_clone.lock().unwrap().push(event);
        });

        // Alice encrypts a message via MLS and constructs the wire payload
        let encrypted = {
            let alice_mls = alice.mls_manager.as_ref().unwrap().read().unwrap();
            let gid = offline_protocol_mls::GroupId::new(&group_id);
            alice_mls.encrypt_for_group(&gid, b"Hello group!").unwrap()
        };

        let msg_payload = GroupMlsMessagePayload {
            group_id: group_id.clone(),
            ciphertext: base64_encode(&encrypted.ciphertext),
            epoch: encrypted.epoch,
            reply_to: None,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_MSG,
            serde_json::to_string(&msg_payload).unwrap()
        );

        // Simulate Bob receiving this message from Alice
        let bob_message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );
        let result = bob.process_internal_message(&bob_message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // Verify Bob received the decrypted message
        let events = bob_events.lock().unwrap();
        let received = events.iter().find(|e| {
            matches!(e, Event::GroupMessageReceived { group_id: gid, content, .. }
                if gid == &group_id && content == "Hello group!")
        });
        assert!(
            received.is_some(),
            "Bob should have received 'Hello group!' via GroupMessageReceived event"
        );
    }

    #[test]
    fn test_group_mls_send_message_multiple_members() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        // Create group and pre-populate cache with multiple members
        let info = protocol.create_mesh_group("Multi-Member Group").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Manually set the member cache to include more members
        protocol.group_members.insert(
            group_id.clone(),
            vec![
                "user123".to_string(), // self
                "bob".to_string(),
                "carol".to_string(),
            ],
        );

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Send a group message
        let msg_ids = protocol
            .send_group_message(&group_id, "hello everyone", None, None)
            .unwrap();

        // Should have sent to bob and carol (not self)
        assert_eq!(msg_ids.len(), 2, "Should send to 2 members (bob, carol)");

        // Check GroupMessageSent event
        let events = events.lock().unwrap();
        let sent_event = events.iter().find(|e| {
            matches!(e, Event::GroupMessageSent { group_id: gid, member_count, .. }
                if gid == &group_id && *member_count == 2)
        });
        assert!(
            sent_event.is_some(),
            "Expected GroupMessageSent event with member_count=2"
        );
    }

    #[test]
    fn test_group_mls_dedup_cap_enforcement() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        // Insert more than MAX_GROUP_MESSAGE_DEDUP_ENTRIES
        let count = MAX_GROUP_MESSAGE_DEDUP_ENTRIES + 100;
        for i in 0..count {
            protocol
                .group_message_dedup
                .insert(format!("msg-{:06}", i), Instant::now());
        }
        assert_eq!(protocol.group_message_dedup.len(), count);

        // Cleanup should enforce the cap
        protocol.cleanup_group_message_dedup();
        assert!(
            protocol.group_message_dedup.len() <= MAX_GROUP_MESSAGE_DEDUP_ENTRIES,
            "Dedup cache should be capped at {}",
            MAX_GROUP_MESSAGE_DEDUP_ENTRIES
        );
    }

    #[test]
    fn test_group_mls_commit_unknown_group() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Simulate receiving a commit for a group we don't belong to
        let commit_payload = GroupMlsCommitPayload {
            group_id: "group:nonexistent".to_string(),
            commit_type: GroupCommitType::Add,
            ciphertext: base64_encode(b"fake-commit-data"),
            epoch: 1,
            affected_member: Some("carol".to_string()),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_COMMIT,
            serde_json::to_string(&commit_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // No membership event should be emitted
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, Event::GroupMemberAdded { .. })),
            "No GroupMemberAdded event for unknown group"
        );
    }

    #[test]
    fn test_group_mls_message_after_leaving() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        // Create and leave a group
        let info = protocol.create_mesh_group("Leave Test").unwrap();
        let group_id = info.group_id.as_str().to_string();
        protocol.leave_mesh_group(&group_id).unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Simulate receiving an encrypted group message for the left group
        let msg_payload = GroupMlsMessagePayload {
            group_id: group_id.clone(),
            ciphertext: base64_encode(b"encrypted-content"),
            epoch: 1,
            reply_to: None,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_MSG,
            serde_json::to_string(&msg_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        // Should not panic, just fail decryption gracefully
        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // No message received event
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, Event::GroupMessageReceived { .. })),
            "No message event after leaving group"
        );
    }

    #[test]
    fn test_group_mls_oversized_payload_rejected() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Create an oversized base64 payload
        let oversized = "A".repeat(MAX_BASE64_PAYLOAD_SIZE + 1);
        let msg_payload = GroupMlsMessagePayload {
            group_id: "group:test".to_string(),
            ciphertext: oversized,
            epoch: 1,
            reply_to: None,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_MSG,
            serde_json::to_string(&msg_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // No message received event — payload was rejected
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, Event::GroupMessageReceived { .. })),
            "Oversized payload should not produce a message event"
        );
    }

    #[test]
    fn test_group_mls_duplicate_message_rejected() {
        // Verifies that a duplicate group message (same message ID) is silently
        // dropped and does NOT emit a second GroupMessageReceived event.
        let storage_a = Arc::new(crate::mls::InMemoryStorage::default());
        let storage_b = Arc::new(crate::mls::InMemoryStorage::default());
        let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
        let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
        alice.initialize_mls(storage_a).unwrap();
        bob.initialize_mls(storage_b).unwrap();
        alice.start().unwrap();
        bob.start().unwrap();

        // Alice creates group, Bob joins
        let group_info = alice.create_mesh_group("Dedup Test Group").unwrap();
        let group_id = group_info.group_id.as_str().to_string();

        let bob_kp = {
            let bob_mls = bob.mls_manager.as_ref().unwrap().read().unwrap();
            bob_mls.generate_key_package().unwrap()
        };
        let (welcome, _commit) = {
            let alice_mls = alice.mls_manager.as_ref().unwrap().read().unwrap();
            let gid = offline_protocol_mls::GroupId::new(&group_id);
            alice_mls
                .add_group_member(&gid, &bob_kp.key_package_data)
                .unwrap()
        };
        {
            let bob_mls = bob.mls_manager.as_ref().unwrap().read().unwrap();
            bob_mls.join_group(&welcome).unwrap();
        }
        bob.group_members.insert(
            group_id.clone(),
            vec!["alice".to_string(), "bob".to_string()],
        );

        // Set up Bob's event capture
        let bob_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let bob_events_clone = bob_events.clone();
        bob.on_event(move |event| {
            bob_events_clone.lock().unwrap().push(event);
        });

        // Alice encrypts a message
        let encrypted = {
            let alice_mls = alice.mls_manager.as_ref().unwrap().read().unwrap();
            let gid = offline_protocol_mls::GroupId::new(&group_id);
            alice_mls.encrypt_for_group(&gid, b"Hello dedup!").unwrap()
        };
        let msg_payload = GroupMlsMessagePayload {
            group_id: group_id.clone(),
            ciphertext: base64_encode(&encrypted.ciphertext),
            epoch: encrypted.epoch,
            reply_to: None,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_MSG,
            serde_json::to_string(&msg_payload).unwrap()
        );

        // Build a message with a fixed ID so we can send the same one twice
        let bob_message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        // First delivery — should succeed and emit GroupMessageReceived
        let result1 = bob.process_internal_message(&bob_message);
        assert!(matches!(result1, Some(InternalMessageResult::Consumed)));

        // Second delivery of the SAME message — should be deduped (no event)
        let result2 = bob.process_internal_message(&bob_message);
        assert!(matches!(result2, Some(InternalMessageResult::Consumed)));

        // Verify exactly ONE GroupMessageReceived event (the duplicate was dropped)
        let events = bob_events.lock().unwrap();
        let received_count = events
            .iter()
            .filter(|e| matches!(e, Event::GroupMessageReceived { .. }))
            .count();
        assert_eq!(
            received_count, 1,
            "Expected exactly 1 GroupMessageReceived, got {} (duplicate should be dropped)",
            received_count
        );
    }

    #[test]
    fn test_group_mls_leave_non_member_rejected() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Pre-populate group member cache — "eve" is NOT in the group
        protocol.group_members.insert(
            "group:nonmember-test".to_string(),
            vec!["user123".to_string(), "alice".to_string()],
        );

        // "eve" sends a leave notification for herself (sender matches, but not a member)
        let leave_payload = GroupMlsLeavePayload {
            group_id: "group:nonmember-test".to_string(),
            leaving_member: "eve".to_string(),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_LEAVE,
            serde_json::to_string(&leave_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("eve").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // No removal event
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, Event::GroupMemberRemoved { .. })),
            "Non-member leave should not produce a removal event"
        );
    }

    #[test]
    fn test_group_mls_leave_deterministic_election() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        // user123 is our user_id from create_test_config
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        // Use a fake group_id that doesn't exist in MLS so refresh_group_members
        // fails and falls back to the local cache. This lets us test the
        // deterministic election logic without needing real MLS member state.
        let group_id = "group:election-test".to_string();

        // Manually set the member cache to include more members.
        // "alice" < "bob" < "user123" lexicographically.
        // When "bob" leaves, "alice" should be elected (lex-first remaining).
        // Since we are "user123", we should NOT be elected.
        protocol.group_members.insert(
            group_id.clone(),
            vec![
                "alice".to_string(),
                "bob".to_string(),
                "user123".to_string(),
            ],
        );

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // "bob" sends a leave notification
        let leave_payload = GroupMlsLeavePayload {
            group_id: group_id.clone(),
            leaving_member: "bob".to_string(),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_LEAVE,
            serde_json::to_string(&leave_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // Since "alice" < "user123", alice should be elected, not us.
        // We should still emit GroupMemberRemoved because we're not the remover.
        let events = events.lock().unwrap();
        let remove_event = events.iter().find(|e| {
            matches!(e, Event::GroupMemberRemoved { group_id: gid, user_id, removed_by }
                if gid == &group_id && user_id == "bob" && removed_by == "bob")
        });
        assert!(
            remove_event.is_some(),
            "Expected GroupMemberRemoved event for non-elected node"
        );
    }

    #[test]
    fn test_group_mls_leave_we_are_elected() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        // user123 is our user_id
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        // Use a fake group_id so refresh_group_members falls back to cache
        let group_id = "group:elected-test".to_string();

        // Members: "user123" < "zzz" lexicographically.
        // When "zzz" leaves, "user123" is the lex-first remaining → we should be elected.
        protocol.group_members.insert(
            group_id.clone(),
            vec!["user123".to_string(), "zzz".to_string()],
        );

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // "zzz" sends a leave notification
        let leave_payload = GroupMlsLeavePayload {
            group_id: group_id.clone(),
            leaving_member: "zzz".to_string(),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_LEAVE,
            serde_json::to_string(&leave_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("zzz").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // We were elected, so remove_from_group was attempted.
        // Since "zzz" is not a real MLS member, the remove will fail — but the
        // important thing is we didn't emit a duplicate event from the non-elected path.
        let events = events.lock().unwrap();
        // Verify no double-emit: at most one GroupMemberRemoved event
        let remove_count = events
            .iter()
            .filter(|e| matches!(e, Event::GroupMemberRemoved { user_id, .. } if user_id == "zzz"))
            .count();
        assert!(
            remove_count <= 1,
            "Expected at most 1 GroupMemberRemoved event, got {}",
            remove_count
        );
    }

    #[test]
    fn test_group_mls_remove_from_group() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        // Create a group
        let info = protocol.create_mesh_group("Remove Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Try to remove a non-existent member — MLS should error
        let result = protocol.remove_from_group(&group_id, "nonexistent-user");
        assert!(result.is_err(), "Removing non-member should fail");
    }

    #[test]
    fn test_group_mls_base64_decode_size_guard() {
        // Verify the base64_decode function rejects oversized payloads
        let oversized = "A".repeat(MAX_BASE64_PAYLOAD_SIZE + 1);
        let result = base64_decode(&oversized);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("payload too large"));

        // Verify normal-sized payloads pass
        let normal = base64_encode(b"hello world");
        let result = base64_decode(&normal);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"hello world");
    }

    #[test]
    fn test_group_mls_welcome_bad_data_no_panic() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Send a Welcome with invalid base64 welcome_data
        let welcome_payload = GroupMlsWelcomePayload {
            group_id: "group:bad-welcome".to_string(),
            group_name: Some("Bad Group".to_string()),
            welcome_data: "not-valid-base64!!!".to_string(),
            member_list: vec!["alice".to_string()],
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_WELCOME,
            serde_json::to_string(&welcome_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        // Should not panic
        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // No join event
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, Event::GroupMemberAdded { .. })),
            "Bad welcome data should not produce a join event"
        );
    }

    #[test]
    fn test_group_mls_welcome_valid_base64_bad_mls_no_panic() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Send a Welcome with valid base64 but garbage MLS data
        let welcome_payload = GroupMlsWelcomePayload {
            group_id: "group:garbage-mls".to_string(),
            group_name: Some("Garbage MLS".to_string()),
            welcome_data: base64_encode(b"this is not valid MLS data"),
            member_list: vec!["alice".to_string()],
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_WELCOME,
            serde_json::to_string(&welcome_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        // Should not panic — MLS join will fail gracefully
        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // No join event
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, Event::GroupMemberAdded { .. })),
            "Invalid MLS welcome should not produce a join event"
        );
    }

    #[test]
    fn test_group_mls_send_message_partial_failure() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        // NOTE: Do NOT call protocol.start() — send_internal_message will fail
        // for all members because the protocol is not running, simulating
        // total delivery failure.

        let info = protocol.create_mesh_group("Partial Failure Group").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Populate cache with multiple members
        protocol.group_members.insert(
            group_id.clone(),
            vec![
                "user123".to_string(),
                "bob".to_string(),
                "carol".to_string(),
            ],
        );

        // Sending should fail since protocol is not started
        let result = protocol.send_group_message(&group_id, "hello", None, None);
        assert!(result.is_err(), "Total send failure should return Err");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("All group message sends failed"),
            "Error should indicate total failure"
        );
    }

    #[test]
    fn test_group_mls_commit_oversized_ciphertext_rejected() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        // Commit with oversized base64 ciphertext
        let oversized = "A".repeat(MAX_BASE64_PAYLOAD_SIZE + 1);
        let commit_payload = GroupMlsCommitPayload {
            group_id: "group:oversized".to_string(),
            commit_type: GroupCommitType::Add,
            ciphertext: oversized,
            epoch: 1,
            affected_member: Some("carol".to_string()),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_COMMIT,
            serde_json::to_string(&commit_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));

        // No membership events
        let events = events.lock().unwrap();
        assert!(
            events.iter().all(|e| !matches!(
                e,
                Event::GroupMemberAdded { .. } | Event::GroupMemberRemoved { .. }
            )),
            "Oversized commit should not produce membership events"
        );
    }

    #[test]
    fn test_group_mls_malformed_json_payloads() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        // Malformed JSON for each message type — should all be consumed without panic
        let prefixes = [
            internal_prefixes::GROUP_MLS_MSG,
            internal_prefixes::GROUP_MLS_WELCOME,
            internal_prefixes::GROUP_MLS_COMMIT,
            internal_prefixes::GROUP_MLS_LEAVE,
        ];

        for prefix in &prefixes {
            let content = format!("{}{{not valid json!", prefix);
            let message = Message::new(
                UserId::new("alice").unwrap(),
                UserId::new("user123").unwrap(),
                AppId::new("test-app").unwrap(),
                &content,
            );
            let result = protocol.process_internal_message(&message);
            assert!(
                matches!(result, Some(InternalMessageResult::Consumed)),
                "Malformed JSON for prefix {} should be consumed",
                prefix
            );
        }
    }

    // ========================================================================
    // Dedup-before-decrypt (anti-replay amplification)
    // ========================================================================

    #[test]
    fn test_group_mls_dedup_inserted_before_decrypt_attempt() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        // Create a group so MLS is available for the group_id
        let info = protocol.create_mesh_group("Dedup Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Build a group message with bad ciphertext (will fail decryption)
        let msg_payload = GroupMlsMessagePayload {
            group_id: group_id.clone(),
            ciphertext: base64_encode(b"definitely-not-valid-mls-ciphertext"),
            epoch: 1,
            reply_to: None,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_MSG,
            serde_json::to_string(&msg_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );

        let msg_id = message.id.as_str().to_string();

        // Process the message — decryption will fail but dedup should be recorded
        let _ = protocol.process_internal_message(&message);
        assert!(
            protocol.group_message_dedup.contains_key(&msg_id),
            "Dedup entry must be inserted even when decryption fails"
        );

        // Second delivery of same message should be skipped entirely
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        protocol.on_event(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        let _ = protocol.process_internal_message(&message);
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, Event::GroupMessageReceived { .. })),
            "Replayed message after dedup must not produce any event"
        );
    }

    // ========================================================================
    // Leave notification ordering (send-before-delete)
    // ========================================================================

    #[test]
    fn test_group_mls_leave_preserves_state_on_total_send_failure() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        // Note: protocol NOT started — send_internal_message will fail with NotStarted

        let info = protocol.create_mesh_group("Leave Fail Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Inject a fake member so there are recipients
        protocol
            .group_members
            .get_mut(&group_id)
            .unwrap()
            .push("bob".to_string());

        // Attempt to leave — all sends should fail because protocol isn't started
        let result = protocol.leave_mesh_group(&group_id);
        assert!(
            result.is_err(),
            "Leave should fail when all notifications fail"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("local state preserved"),
            "Error should indicate state was preserved for retry"
        );

        // Verify local MLS state is still intact (group still exists)
        let groups = protocol.list_mesh_groups().unwrap();
        assert!(
            groups.contains(&group_id),
            "Group should still exist locally after failed leave"
        );

        // Verify cache is still intact
        assert!(
            protocol.group_members.contains_key(&group_id),
            "Group member cache should be preserved after failed leave"
        );
    }

    #[test]
    fn test_group_mls_leave_deletes_state_after_successful_notification() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let info = protocol.create_mesh_group("Leave OK Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Solo group (only self) — no recipients, leave should succeed directly
        let result = protocol.leave_mesh_group(&group_id);
        assert!(result.is_ok(), "Leave with no other members should succeed");

        // Verify local state was cleaned up
        let groups = protocol.list_mesh_groups().unwrap();
        assert!(
            !groups.contains(&group_id),
            "Group should be removed after successful leave"
        );
        assert!(
            !protocol.group_members.contains_key(&group_id),
            "Cache should be cleared after successful leave"
        );
    }

    // ========================================================================
    // Out-of-order commit buffering
    // ========================================================================

    #[test]
    fn test_group_mls_commit_failure_buffers_for_retry() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let info = protocol.create_mesh_group("Buffer Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Send a commit with bad ciphertext — should be buffered
        let commit_payload = GroupMlsCommitPayload {
            group_id: group_id.clone(),
            commit_type: GroupCommitType::Add,
            ciphertext: base64_encode(b"bad-commit-data"),
            epoch: 99,
            affected_member: Some("new-member".to_string()),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_COMMIT,
            serde_json::to_string(&commit_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );
        let _ = protocol.process_internal_message(&message);

        // Verify commit was buffered
        assert!(
            protocol.pending_commits.contains_key(&group_id),
            "Failed commit should be buffered"
        );
        assert_eq!(
            protocol.pending_commits.get(&group_id).unwrap().len(),
            1,
            "Exactly one commit should be buffered"
        );
    }

    #[test]
    fn test_group_mls_pending_commit_buffer_cap() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let info = protocol.create_mesh_group("Cap Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Fill the buffer beyond capacity
        for i in 0..(MAX_PENDING_COMMITS_PER_GROUP + 4) {
            let commit_payload = GroupMlsCommitPayload {
                group_id: group_id.clone(),
                commit_type: GroupCommitType::Add,
                ciphertext: base64_encode(format!("bad-commit-{}", i).as_bytes()),
                epoch: i as u64,
                affected_member: None,
            };
            let content = format!(
                "{}{}",
                internal_prefixes::GROUP_MLS_COMMIT,
                serde_json::to_string(&commit_payload).unwrap()
            );
            let message = Message::new(
                UserId::new("alice").unwrap(),
                UserId::new("user123").unwrap(),
                AppId::new("test-app").unwrap(),
                &content,
            );
            let _ = protocol.process_internal_message(&message);
        }

        // Buffer should be capped at MAX_PENDING_COMMITS_PER_GROUP
        let buffered = protocol.pending_commits.get(&group_id).unwrap();
        assert_eq!(
            buffered.len(),
            MAX_PENDING_COMMITS_PER_GROUP,
            "Buffer should not exceed cap"
        );
    }

    #[test]
    fn test_group_mls_pending_commit_expired_entries_cleaned_up() {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

        let group_id = "test-group".to_string();
        // Insert a pending commit with an expired timestamp
        let expired = PendingCommit {
            sender: "alice".to_string(),
            data: "{}".to_string(),
            buffered_at: Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10),
        };
        protocol
            .pending_commits
            .entry(group_id.clone())
            .or_default()
            .push(expired);

        // Insert a recent one
        let recent = PendingCommit {
            sender: "bob".to_string(),
            data: "{}".to_string(),
            buffered_at: Instant::now(),
        };
        protocol
            .pending_commits
            .entry(group_id.clone())
            .or_default()
            .push(recent);

        // Run cleanup
        protocol.cleanup_group_message_dedup();

        // Expired entry should be removed, recent one retained
        let buf = protocol.pending_commits.get(&group_id).unwrap();
        assert_eq!(buf.len(), 1, "Only recent pending commit should survive");
        assert_eq!(buf[0].sender, "bob");
    }

    #[test]
    fn test_group_mls_commit_empty_ciphertext_not_buffered() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let info = protocol.create_mesh_group("No Buffer Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Empty ciphertext — this is a malformed commit, not an ordering issue
        let commit_payload = GroupMlsCommitPayload {
            group_id: group_id.clone(),
            commit_type: GroupCommitType::Remove,
            ciphertext: String::new(),
            epoch: 1,
            affected_member: None,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_COMMIT,
            serde_json::to_string(&commit_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );
        let _ = protocol.process_internal_message(&message);

        // Empty ciphertext should NOT be buffered (it's not an ordering issue)
        assert!(
            !protocol.pending_commits.contains_key(&group_id)
                || protocol.pending_commits.get(&group_id).unwrap().is_empty(),
            "Empty ciphertext commits must not be buffered"
        );
    }

    #[test]
    fn test_group_mls_double_leave_is_idempotent() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let info = protocol.create_mesh_group("Double Leave").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // First leave should succeed
        let result = protocol.leave_mesh_group(&group_id);
        assert!(result.is_ok());

        // Second leave is idempotent — MLS delete_group silently succeeds when
        // the group doesn't exist, and there are no members to notify.
        let result = protocol.leave_mesh_group(&group_id);
        assert!(
            result.is_ok(),
            "Double leave should be idempotent (no error)"
        );

        // Verify state is clean
        assert!(!protocol.group_members.contains_key(&group_id));
        let groups = protocol.list_mesh_groups().unwrap();
        assert!(!groups.contains(&group_id));
    }

    #[test]
    fn test_group_mls_send_message_content_too_large() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let info = protocol.create_mesh_group("Large Content Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Content at the limit should not be rejected by the size guard
        // (it may still fail at MLS encrypt, but the size check passes)
        let at_limit = "A".repeat(MAX_GROUP_CONTENT_LENGTH);
        let result = protocol.send_group_message(&group_id, &at_limit, None, None);
        // This will succeed (solo group returns Ok([])) or fail for MLS reasons,
        // but NOT for the "content too large" reason
        if let Err(ref e) = result {
            assert!(
                !e.to_string().contains("too large"),
                "Content at limit should not be rejected as too large"
            );
        }

        // Content over the limit must be rejected
        let oversized = "A".repeat(MAX_GROUP_CONTENT_LENGTH + 1);
        let result = protocol.send_group_message(&group_id, &oversized, None, None);
        assert!(result.is_err(), "Oversized content should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("too large"),
            "Error should mention 'too large', got: {}",
            err_msg
        );
    }

    #[test]
    fn test_group_mls_invite_exceeds_max_members() {
        let storage = Arc::new(crate::mls::InMemoryStorage::default());
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage).unwrap();
        protocol.start().unwrap();

        let info = protocol.create_mesh_group("Cap Test").unwrap();
        let group_id = info.group_id.as_str().to_string();

        // Simulate a group at MAX_GROUP_MEMBERS by injecting fake members into cache
        let fake_members: Vec<String> = (0..MAX_GROUP_MEMBERS)
            .map(|i| format!("member-{}", i))
            .collect();
        protocol
            .group_members
            .insert(group_id.clone(), fake_members);

        // Attempting to invite should fail with the cap error
        let result = protocol.invite_to_group(&group_id, "new-invitee");
        assert!(result.is_err(), "Should reject invite when at member cap");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("cannot exceed"),
            "Error should mention cap, got: {}",
            err_msg
        );
    }
}

