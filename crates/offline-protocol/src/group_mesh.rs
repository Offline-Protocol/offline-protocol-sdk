//! Group messaging — MLS-encrypted, transport-agnostic group operations.
//!
//! This module implements group creation, member invite/remove/leave,
//! encrypted message fan-out, commit distribution, and pending commit
//! buffering for out-of-order delivery.
//!
//! Groups are transport-agnostic: messages route through whichever transport
//! DORS selects (BLE, WiFi Direct, or Internet). When Internet is available
//! and a relay server supports it, the protocol can send a single relay
//! broadcast instead of O(N) per-member fan-out.

use crate::protocol::{base64_decode, base64_encode, internal_prefixes, OfflineProtocol};
use crate::{Error, Event, Result};
use chrono::Utc;
use offline_protocol_core::{Message, MessageId, MessagePriority};
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration as StdDuration, Instant};
use tracing::{debug, error, info, warn};

/// TTL for group message dedup entries (5 minutes).
pub(super) const GROUP_MESSAGE_DEDUP_TTL_SECS: u64 = 300;
/// Maximum number of group message dedup entries before forced cleanup.
pub(super) const MAX_GROUP_MESSAGE_DEDUP_ENTRIES: usize = 10_000;
/// Maximum allowed base64-encoded payload size for incoming group messages (1 MB).
pub(crate) const MAX_BASE64_PAYLOAD_SIZE: usize = 1_048_576;
/// Maximum number of buffered out-of-order commits per group.
pub(super) const MAX_PENDING_COMMITS_PER_GROUP: usize = 8;
/// TTL for buffered pending commits (2 minutes).
pub(super) const PENDING_COMMIT_TTL_SECS: u64 = 120;
/// Timeout before the next eligible member re-elects itself for a leave
/// remove-commit when the original elected remover fails (30 seconds).
pub(super) const LEAVE_ELECTION_TIMEOUT_SECS: u64 = 30;
/// Delay after detecting a potential epoch fork before the leader issues a
/// resync commit (10 seconds — gives time for buffered commits to drain).
pub(super) const EPOCH_FORK_RESOLUTION_DELAY_SECS: u64 = 10;
/// Maximum number of tracked epoch fork states to prevent unbounded growth.
pub(super) const MAX_EPOCH_FORK_ENTRIES: usize = 32;
/// Maximum number of tracked pending leave elections to prevent unbounded growth.
pub(super) const MAX_PENDING_LEAVE_ELECTIONS: usize = 64;
/// Maximum lifetime for a leave election before it is abandoned (5 minutes).
/// After this, the election is cleaned up regardless of whether the member
/// was removed — prevents infinite retry loops when all candidates fail.
pub(super) const LEAVE_ELECTION_MAX_LIFETIME_SECS: u64 = 300;
/// Minimum cooldown between re-election attempts for the same leave election
/// to prevent spamming MLS operations on every process tick (5 seconds).
pub(super) const LEAVE_ELECTION_ATTEMPT_COOLDOWN_SECS: u64 = 5;

/// Bundled state for group messaging.
///
/// Groups together the cached member lists, dedup table, pending commit
/// buffer, and relay sync tracking so that `OfflineProtocol` doesn't carry
/// these as individual fields.
#[derive(Default)]
pub(crate) struct GroupMeshState {
    /// Cached group membership lists for fan-out without holding MLS lock.
    /// Maps group_id -> list of member user IDs.
    pub(crate) members: HashMap<String, Vec<String>>,

    /// Deduplication cache for group messages received via multiple paths.
    /// Key: message ID, Value: when first seen.
    pub(crate) message_dedup: HashMap<String, Instant>,

    /// Buffer for out-of-order MLS commits that failed to decrypt.
    /// Maps group_id -> deque of pending commits awaiting retry.
    /// When a commit succeeds for a group, buffered commits are drained and retried.
    pub(crate) pending_commits: HashMap<String, VecDeque<PendingCommit>>,

    /// Group IDs that have been successfully registered with the relay server.
    /// Cleared when Internet transport disconnects so that groups are re-synced
    /// when connectivity returns.
    pub(crate) relay_synced: HashSet<String>,

    /// Whether Internet transport was available on the last `process()` tick.
    /// Used for edge-detection: sync groups on 0→1 transition, clear on 1→0.
    pub(crate) internet_was_available: bool,

    /// Pending leave elections awaiting a remove-commit from the elected member.
    /// Key: (group_id, leaving_member_id) — uses a tuple to avoid ambiguity
    /// from string concatenation when IDs contain separator characters.
    pub(crate) pending_leave_elections: HashMap<(String, String), PendingLeaveElection>,

    /// Suspected epoch forks awaiting resolution. Key: group_id.
    pub(crate) epoch_forks: HashMap<String, EpochForkState>,
}

/// Outcome of attempting to process an MLS Commit.
enum CommitOutcome {
    /// Successfully processed; contains the group ID.
    Success(String),
    /// MLS decryption failed transiently (e.g., out-of-order delivery);
    /// the commit may succeed after a prior commit advances the epoch.
    /// Contains the group ID for buffering.
    Retriable(String),
    /// Permanently failed (parse error, bad ciphertext, MLS unavailable);
    /// should not be retried.
    Rejected,
}

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
    /// A key update commit (epoch advancement without membership change).
    /// Used for epoch fork resolution.
    KeyUpdate,
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

// --- Relay optimization payloads ---

/// Payload for registering/updating a group with the relay server.
///
/// Sent as a `__GRP_RELAY_REG__`-prefixed internal message to the user's
/// own ID via Internet transport.  The relay server intercepts the prefix
/// and records the group → member mapping so it can perform server-side
/// fan-out for future `__GRP_RELAY_BCAST__` messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RelayGroupRegistrationPayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// Human-readable group name (for display/search on relay).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) group_name: Option<String>,
    /// Current member user IDs.
    pub(crate) members: Vec<String>,
}

/// Payload for a relay-broadcast of an MLS-encrypted group message.
///
/// Sent as a `__GRP_RELAY_BCAST__`-prefixed internal message to the user's
/// own ID via Internet transport.  The relay server looks up the group
/// membership, wraps the ciphertext in individual `__GRP_MLS_MSG__`
/// messages, and delivers to each member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RelayGroupBroadcastPayload {
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

/// A commit that arrived out-of-order and is waiting to be processed.
///
/// In mesh networks, messages can arrive out of order. If a Commit arrives
/// before a prior Commit, MLS decryption will fail. We buffer it here and
/// retry after successfully processing a later commit for the same group.
#[derive(Debug, Clone)]
pub(crate) struct PendingCommit {
    /// The original sender of this commit.
    pub(crate) sender: String,
    /// The raw JSON data (after prefix strip) for replay.
    pub(crate) data: String,
    /// When this pending commit was first buffered.
    pub(crate) buffered_at: Instant,
    /// Number of times this commit has been retried and still failed.
    /// A commit that expires with `retry_count > 0` is a strong signal
    /// of epoch mismatch (true fork), not just slow delivery.
    pub(crate) retry_count: u32,
}

/// Tracks a leave election where we're waiting for the elected remover to
/// issue a remove-commit. If the timeout expires and the member is still
/// in the group, the next eligible member re-elects itself.
#[derive(Debug, Clone)]
pub(crate) struct PendingLeaveElection {
    /// Group the leave is for.
    pub(crate) group_id: String,
    /// User ID of the member who is leaving.
    pub(crate) leaving_member: String,
    /// When we received the leave notification.
    pub(crate) received_at: Instant,
    /// When we last attempted to issue a remove-commit for this election.
    /// Used as a cooldown to prevent spamming MLS operations on every
    /// process tick when the current candidate fails repeatedly.
    pub(crate) last_attempt_at: Option<Instant>,
}

/// Tracks a suspected epoch fork for a group.
///
/// An epoch fork occurs when two members issue concurrent commits at the
/// same epoch (e.g., two admins adding different members simultaneously).
/// Members who accepted commit A are on one branch, those who accepted
/// commit B are on another. Detected when buffered commits expire without
/// resolution.
#[derive(Debug, Clone)]
pub(crate) struct EpochForkState {
    /// Group with the suspected fork.
    pub(crate) group_id: String,
    /// Our local epoch when the fork was detected, or `None` if MLS was unavailable.
    pub(crate) local_epoch: Option<u64>,
    /// When the fork was first suspected.
    pub(crate) detected_at: Instant,
    /// Whether the leader has already attempted resolution.
    pub(crate) resolution_attempted: bool,
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
        if self.group_mesh.message_dedup.contains_key(&dedup_key) {
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
        self.group_mesh
            .message_dedup
            .insert(dedup_key, Instant::now());
        if self.group_mesh.message_dedup.len() > MAX_GROUP_MESSAGE_DEDUP_ENTRIES {
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
            self.emit_event(Event::group_message_received(
                payload.group_id,
                sender.to_string(),
                text,
                timestamp,
                msg_id,
                payload.reply_to,
            ));
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
            self.group_mesh.members.insert(group_id.clone(), members);
            self.emit_event(Event::group_member_added(
                group_id,
                self.config.user_id.clone(),
                sender.to_string(),
            ));
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
        match self.process_commit_core(sender, data) {
            CommitOutcome::Success(group_id) => {
                self.drain_pending_commits(&group_id);
            }
            CommitOutcome::Retriable(group_id) => {
                self.buffer_pending_commit(&group_id, sender, data);
            }
            CommitOutcome::Rejected => {}
        }
    }

    /// Core commit processing logic. Returns an outcome indicating success,
    /// retriable failure, or permanent rejection.
    ///
    /// Does **not** buffer on failure — callers decide whether to buffer based
    /// on the returned outcome.
    fn process_commit_core(&mut self, sender: &str, data: &str) -> CommitOutcome {
        let payload = match serde_json::from_str::<GroupMlsCommitPayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupMlsCommit payload");
                return CommitOutcome::Rejected;
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
            return CommitOutcome::Rejected;
        }

        let ciphertext_bytes = match base64_decode(&payload.ciphertext) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Failed to decode commit ciphertext");
                return CommitOutcome::Rejected;
            }
        };

        let mls_guard = match self.read_mls_guard() {
            Ok(guard) => guard,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "MLS unavailable, dropping group commit");
                return CommitOutcome::Rejected;
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
        let mls_result = mls_guard.decrypt_from_group(&encrypted);
        drop(mls_guard);

        match mls_result {
            Ok(_) => {}
            Err(ref e) => {
                // Classify MLS errors: permanent failures should not be retried,
                // only epoch-related decryption failures are worth buffering.
                let is_permanent = matches!(
                    e,
                    offline_protocol_mls::MlsError::GroupNotFound(_)
                        | offline_protocol_mls::MlsError::Deserialization(_)
                        | offline_protocol_mls::MlsError::InvalidMessage(_)
                        | offline_protocol_mls::MlsError::Storage(_)
                );
                if is_permanent {
                    error!(
                        group_id = %payload.group_id,
                        epoch = payload.epoch,
                        error = %e,
                        "Permanently failed to process group commit (not retriable)"
                    );
                    return CommitOutcome::Rejected;
                }
                error!(
                    group_id = %payload.group_id,
                    epoch = payload.epoch,
                    error = %e,
                    "Failed to process group commit (may be out-of-order, buffering)"
                );
                return CommitOutcome::Retriable(payload.group_id);
            }
        }

        // Refresh cache and compute actual membership delta
        let _ = self.refresh_group_members(&payload.group_id);
        let members_after: HashSet<String> = self
            .group_mesh
            .members
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
                GroupCommitType::KeyUpdate => true, // No membership change expected
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
        for member in &actual_added {
            self.emit_event(Event::group_member_added(
                payload.group_id.clone(),
                (*member).clone(),
                sender.to_string(),
            ));
        }
        for member in &actual_removed {
            self.emit_event(Event::group_member_removed(
                payload.group_id.clone(),
                (*member).clone(),
                sender.to_string(),
            ));
            // Clear any pending leave election for this member — the remove
            // has been committed successfully.
            self.group_mesh
                .pending_leave_elections
                .remove(&(payload.group_id.clone(), member.to_string()));
        }

        // Only clear fork tracking when a KeyUpdate commit succeeds — these
        // are the resolution commits issued by the leader. Regular Add/Remove
        // commits from the same branch succeeding doesn't prove the fork is
        // resolved; members on the other branch are still diverged.
        if matches!(payload.commit_type, GroupCommitType::KeyUpdate) {
            self.group_mesh.epoch_forks.remove(&payload.group_id);
        }

        CommitOutcome::Success(payload.group_id)
    }

    /// Buffers a failed commit for deferred retry.
    pub(crate) fn buffer_pending_commit(&mut self, group_id: &str, sender: &str, data: &str) {
        let pending = PendingCommit {
            sender: sender.to_string(),
            data: data.to_string(),
            buffered_at: Instant::now(),
            retry_count: 0,
        };
        let buf = self
            .group_mesh
            .pending_commits
            .entry(group_id.to_string())
            .or_default();
        if buf.len() < MAX_PENDING_COMMITS_PER_GROUP {
            buf.push_back(pending);
            debug!(
                group_id = %group_id,
                buffered_count = buf.len(),
                "Buffered out-of-order commit for deferred retry"
            );
        } else {
            warn!(
                group_id = %group_id,
                "Pending commit buffer full, dropping oldest"
            );
            buf.pop_front();
            buf.push_back(pending);
        }
    }

    /// Drains and retries buffered pending commits for a group after a
    /// successful commit advanced the epoch. Each successful retry triggers
    /// another drain pass (at most `MAX_PENDING_COMMITS_PER_GROUP` iterations).
    ///
    /// Uses `process_commit_core` (not `handle_group_mls_commit`) to avoid
    /// double-buffering: this method owns the retry lifecycle and re-buffers
    /// only via `still_pending`, never through the core processing path.
    pub(crate) fn drain_pending_commits(&mut self, group_id: &str) {
        // Track commits that were retried at least once and still expired —
        // these are strong evidence of epoch mismatch (true fork), not just
        // slow delivery from the mesh network.
        let mut retried_expired_count: usize = 0;

        // Limit iterations to avoid unbounded looping
        for _ in 0..MAX_PENDING_COMMITS_PER_GROUP {
            let pending = match self.group_mesh.pending_commits.get_mut(group_id) {
                Some(buf) if !buf.is_empty() => std::mem::take(buf),
                _ => break,
            };

            let mut any_succeeded = false;
            let mut still_pending = VecDeque::new();

            for entry in pending {
                // Drop expired entries — only count retried ones for fork detection
                if entry.buffered_at.elapsed() > StdDuration::from_secs(PENDING_COMMIT_TTL_SECS) {
                    debug!(
                        group_id = %group_id,
                        retry_count = entry.retry_count,
                        "Dropping expired pending commit"
                    );
                    if entry.retry_count > 0 {
                        retried_expired_count += 1;
                    }
                    continue;
                }
                match self.process_commit_core(&entry.sender, &entry.data) {
                    CommitOutcome::Success(_) => {
                        any_succeeded = true;
                    }
                    CommitOutcome::Retriable(_) => {
                        // Still out-of-order — keep for next pass with incremented retry count
                        let mut entry = entry;
                        entry.retry_count += 1;
                        still_pending.push_back(entry);
                    }
                    CommitOutcome::Rejected => {
                        // Permanently bad — drop silently
                    }
                }
            }

            // Re-buffer commits that still failed
            if !still_pending.is_empty() {
                self.group_mesh
                    .pending_commits
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
            .group_mesh
            .pending_commits
            .get(group_id)
            .map_or(true, |v| v.is_empty())
        {
            self.group_mesh.pending_commits.remove(group_id);
        }

        // If commits that were retried (and kept failing) expired, this is a
        // strong signal of epoch fork — the commits were valid but for a
        // different epoch branch. Commits that expired without any retries
        // are more likely just slow mesh delivery and are not flagged.
        if retried_expired_count > 0 && !self.group_mesh.epoch_forks.contains_key(group_id) {
            self.flag_potential_epoch_fork(group_id);
        }
    }

    /// Flags a group as having a potential epoch fork.
    ///
    /// Called when buffered commits expire without resolution, indicating
    /// the group may have diverged due to concurrent commits.
    fn flag_potential_epoch_fork(&mut self, group_id: &str) {
        if self.group_mesh.epoch_forks.len() >= MAX_EPOCH_FORK_ENTRIES {
            // Evict the oldest entry to prevent unbounded growth
            if let Some(oldest_key) = self
                .group_mesh
                .epoch_forks
                .values()
                .min_by_key(|f| f.detected_at)
                .map(|f| f.group_id.clone())
            {
                self.group_mesh.epoch_forks.remove(&oldest_key);
            }
        }

        let local_epoch = self.refresh_group_members(group_id).ok().and_then(|_| {
            let mls_guard = self.read_mls_guard().ok()?;
            let gid = offline_protocol_mls::GroupId::new(group_id);
            mls_guard
                .get_group_info(&gid)
                .ok()
                .flatten()
                .map(|i| i.epoch)
        });

        if local_epoch.is_none() {
            warn!(
                group_id = %group_id,
                "Could not read local MLS epoch during fork detection — MLS unavailable or group not found"
            );
        }

        warn!(
            group_id = %group_id,
            local_epoch = ?local_epoch,
            "Potential epoch fork detected — buffered commits expired without resolution"
        );

        self.group_mesh.epoch_forks.insert(
            group_id.to_string(),
            EpochForkState {
                group_id: group_id.to_string(),
                local_epoch,
                detected_at: Instant::now(),
                resolution_attempted: false,
            },
        );

        self.emit_event(Event::group_epoch_fork_detected(
            group_id.to_string(),
            local_epoch,
        ));
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
            .or_else(|| self.group_mesh.members.get(&payload.group_id).cloned())
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
        let mut remaining: Vec<String> = members
            .iter()
            .filter(|m| m.as_str() != sender)
            .cloned()
            .collect();
        remaining.sort();

        let should_remove = remaining
            .first()
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
            // Clear any pending election for this leave (we handled it)
            let key = (payload.group_id.clone(), sender.to_string());
            self.group_mesh.pending_leave_elections.remove(&key);
        } else {
            // We're not the elected remover — record the election so we can
            // take over if the elected member fails to issue the commit in time.
            if self.group_mesh.pending_leave_elections.len() >= MAX_PENDING_LEAVE_ELECTIONS {
                // Evict the oldest entry to prevent unbounded growth.
                if let Some(oldest_key) = self
                    .group_mesh
                    .pending_leave_elections
                    .iter()
                    .min_by_key(|(_, e)| e.received_at)
                    .map(|(k, _)| k.clone())
                {
                    self.group_mesh.pending_leave_elections.remove(&oldest_key);
                }
            }
            let key = (payload.group_id.clone(), sender.to_string());
            self.group_mesh.pending_leave_elections.insert(
                key,
                PendingLeaveElection {
                    group_id: payload.group_id.clone(),
                    leaving_member: sender.to_string(),
                    received_at: Instant::now(),
                    last_attempt_at: None,
                },
            );
        }

        // Do NOT emit GroupMemberRemoved here for non-elected nodes.
        // The elected node issues an MLS remove-commit which all members
        // (including us) will process via `handle_group_mls_commit` →
        // `process_commit_core`, which emits the event based on the actual
        // MLS membership delta. Emitting here would cause a duplicate event
        // because we'd emit once now (premature, before MLS state changes)
        // and once when the commit arrives.
    }

    /// Refreshes the cached member list for a group from MlsManager.
    pub(crate) fn refresh_group_members(&mut self, group_id: &str) -> Result<Vec<String>> {
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id);
        let info = mls_guard
            .get_group_info(&gid)?
            .ok_or_else(|| Error::Other(format!("Group not found: {}", group_id)))?;
        let members = info.members.clone();
        drop(mls_guard);
        self.group_mesh
            .members
            .insert(group_id.to_string(), members.clone());
        Ok(members)
    }

    /// Creates a new MLS group.
    ///
    /// The group is created locally via MLS. Members can be invited with
    /// `invite_to_group()`. Messages sent via `send_group_message()` are
    /// MLS-encrypted and route through whichever transport DORS selects.
    /// If Internet is available the group is also registered with the
    /// relay server for optimized fan-out.
    pub fn create_group(&mut self, group_name: &str) -> Result<offline_protocol_mls::GroupInfo> {
        let mls_guard = self.read_mls_guard()?;
        let group_info = mls_guard.create_group(group_name)?;
        let group_id = group_info.group_id.as_str().to_string();
        let members = group_info.members.clone();
        drop(mls_guard);

        self.group_mesh
            .members
            .insert(group_id.clone(), members.clone());

        self.emit_event(Event::group_created(
            group_id.clone(),
            group_name.to_string(),
        ));

        // Best-effort relay registration
        let _ = self.try_relay_register_group(&group_id, Some(group_name), &members);

        Ok(group_info)
    }

    /// Invites a user to an MLS group.
    ///
    /// Requires the invitee's key package to be available in `pending_key_packages`.
    /// Sends a Welcome to the invitee and a Commit to all existing members.
    pub fn invite_to_group(&mut self, group_id: &str, invitee_user_id: &str) -> Result<()> {
        // Check group member cap before adding
        let current_count = self
            .group_mesh
            .members
            .get(group_id)
            .map(|m| m.len())
            .or_else(|| {
                self.read_mls_guard().ok().and_then(|g| {
                    g.get_group_info(&offline_protocol_mls::GroupId::new(group_id))
                        .ok()
                        .flatten()
                        .map(|info| info.members.len())
                })
            })
            .unwrap_or(0);
        let max_members = self.config.group.max_group_members;
        if current_count >= max_members {
            return Err(Error::Other(format!(
                "Group has {} members, cannot exceed {} limit",
                current_count, max_members
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

        self.emit_event(Event::group_member_added(
            group_id.to_string(),
            invitee_user_id.to_string(),
            self_id,
        ));

        // Sync membership update to relay
        let _ = self.try_relay_register_group(group_id, None, &members);

        info!(group_id = %group_id, invitee = %invitee_user_id, "Invited member to group");
        Ok(())
    }

    /// Removes a member from an MLS group.
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

        self.emit_event(Event::group_member_removed(
            group_id.to_string(),
            member_id.to_string(),
            self_id,
        ));

        // Sync membership update to relay
        let _ = self.try_relay_register_group(group_id, None, &members);

        info!(group_id = %group_id, member = %member_id, "Removed member from group");
        Ok(())
    }

    /// Leaves an MLS group.
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
    pub fn leave_group(&mut self, group_id: &str) -> Result<()> {
        // Get members before leaving
        let members = self
            .group_mesh
            .members
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

        // Remove from caches
        self.group_mesh.members.remove(group_id);
        self.group_mesh.relay_synced.remove(group_id);

        info!(group_id = %group_id, "Left group");
        Ok(())
    }

    /// Sends a message to all members of an MLS group.
    ///
    /// If Internet transport is available and relay-enabled, sends a single
    /// relay broadcast (`__GRP_RELAY_BCAST__`). Otherwise falls back to
    /// per-member fan-out via `send_internal_message()` where each member's
    /// delivery goes through the full DORS/ACK/retry stack independently.
    pub fn send_group_message(
        &mut self,
        group_id: &str,
        content: &str,
        priority: Option<MessagePriority>,
        reply_to_msg: Option<&str>,
    ) -> Result<Vec<MessageId>> {
        let priority = priority.unwrap_or(MessagePriority::Medium);

        // Encrypt via MLS — release the guard immediately after encryption
        // to minimize lock contention during the fan-out phase.
        let encrypted = {
            let mls_guard = self.read_mls_guard()?;
            let gid = offline_protocol_mls::GroupId::new(group_id);
            mls_guard.encrypt_for_group(&gid, content.as_bytes())?
        };

        // Read member list from cache, falling back to MLS on cache miss.
        let members = match self.group_mesh.members.get(group_id) {
            Some(m) => m.clone(),
            None => {
                let mls_guard = self.read_mls_guard()?;
                let gid = offline_protocol_mls::GroupId::new(group_id);
                let info = mls_guard
                    .get_group_info(&gid)?
                    .ok_or_else(|| Error::Other(format!("Group not found: {}", group_id)))?;
                info.members.clone()
            }
        };

        // Update cache if it was a miss
        if !self.group_mesh.members.contains_key(group_id) {
            self.group_mesh
                .members
                .insert(group_id.to_string(), members.clone());
        }

        let ciphertext_b64 = base64_encode(&encrypted.ciphertext);
        let epoch = encrypted.epoch;

        let self_id = self.config.user_id.clone();

        // --- Attempt relay broadcast first ---
        // If Internet is the primary transport and the group is registered,
        // a single relay broadcast is O(1) instead of O(N) individual sends.
        if self.group_mesh.relay_synced.contains(group_id) {
            if let Ok(mid) =
                self.try_relay_broadcast(group_id, &ciphertext_b64, epoch, reply_to_msg)
            {
                let member_count = members.iter().filter(|m| m.as_str() != self_id).count() as u32;
                self.emit_event(Event::group_message_sent(
                    group_id.to_string(),
                    vec![mid.as_str().to_string()],
                    member_count,
                ));
                return Ok(vec![mid]);
            }
            // Relay broadcast failed — fall through to per-member fan-out
        }

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

        // Per-member fan-out (mesh or DORS-selected transport)
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
            self.emit_event(Event::group_message_partial_failure(
                group_id.to_string(),
                failed_members,
                succeeded_members,
            ));
            return Err(Error::Other("All group message sends failed".to_string()));
        }

        // Emit appropriate event
        if failed_members.is_empty() {
            self.emit_event(Event::group_message_sent(
                group_id.to_string(),
                message_ids.iter().map(|m| m.as_str().to_string()).collect(),
                member_count,
            ));
        } else {
            self.emit_event(Event::group_message_partial_failure(
                group_id.to_string(),
                failed_members,
                succeeded_members,
            ));
        }

        Ok(message_ids)
    }

    /// Lists all MLS groups (excluding 1:1 sessions).
    pub fn list_groups(&self) -> Result<Vec<String>> {
        let mls_guard = self.read_mls_guard()?;
        let groups = mls_guard.list_groups()?;
        Ok(groups.into_iter().map(|g| g.as_str().to_string()).collect())
    }

    /// Cleans up expired group message dedup entries and enforces size cap.
    pub(crate) fn cleanup_group_message_dedup(&mut self) {
        let cutoff = Instant::now() - StdDuration::from_secs(GROUP_MESSAGE_DEDUP_TTL_SECS);
        self.group_mesh
            .message_dedup
            .retain(|_, seen_at| *seen_at > cutoff);
        // If still over cap after TTL cleanup, drop oldest entries using O(N) selection
        let len = self.group_mesh.message_dedup.len();
        if len > MAX_GROUP_MESSAGE_DEDUP_ENTRIES {
            let mut entries: Vec<_> = self.group_mesh.message_dedup.drain().collect();
            // Partition so the newest MAX entries are in [..MAX]
            entries.select_nth_unstable_by_key(MAX_GROUP_MESSAGE_DEDUP_ENTRIES, |(_, ts)| {
                std::cmp::Reverse(*ts)
            });
            entries.truncate(MAX_GROUP_MESSAGE_DEDUP_ENTRIES);
            self.group_mesh.message_dedup = entries.into_iter().collect();
        }

        // Expire stale pending commits, tracking groups where retried commits
        // expired — these are strong epoch fork signals (same as drain_pending_commits).
        let commit_cutoff = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS);
        let mut fork_candidates: Vec<String> = Vec::new();
        self.group_mesh.pending_commits.retain(|group_id, commits| {
            let mut retried_expired = false;
            commits.retain(|c| {
                let alive = c.buffered_at > commit_cutoff;
                if !alive && c.retry_count > 0 {
                    retried_expired = true;
                }
                alive
            });
            if retried_expired {
                fork_candidates.push(group_id.clone());
            }
            !commits.is_empty()
        });
        // Flag forks for groups where retried commits expired during periodic
        // cleanup — this covers the case where drain_pending_commits is never
        // called because no new commits succeed for the group.
        for group_id in fork_candidates {
            if !self.group_mesh.epoch_forks.contains_key(&group_id) {
                self.flag_potential_epoch_fork(&group_id);
            }
        }
    }

    // ========================================================================
    // RELAY OPTIMIZATION
    // ========================================================================

    /// Attempts to register (or update) a group with the relay server.
    ///
    /// Sends a `__GRP_RELAY_REG__` message to the user's own ID via Internet
    /// transport. The relay server intercepts the prefix and records the
    /// group membership for server-side fan-out.
    ///
    /// Fire-and-forget: returns `Ok(true)` if sent, `Ok(false)` if Internet
    /// is unavailable, `Err` on serialization failure.
    fn try_relay_register_group(
        &mut self,
        group_id: &str,
        group_name: Option<&str>,
        members: &[String],
    ) -> Result<bool> {
        if !self.config.group.relay_enabled || !self.is_internet_available() {
            return Ok(false);
        }

        let payload = RelayGroupRegistrationPayload {
            group_id: group_id.to_string(),
            group_name: group_name.map(|s| s.to_string()),
            members: members.to_vec(),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_RELAY_REGISTER,
            serde_json::to_string(&payload)
                .map_err(|e| Error::Other(format!("Serialize relay registration: {}", e)))?
        );

        // Send to self — the relay server intercepts messages with this prefix
        let self_id = self.config.user_id.clone();
        match self.send_internal_message(&self_id, content, MessagePriority::Medium) {
            Ok(_) => {
                self.group_mesh.relay_synced.insert(group_id.to_string());
                debug!(group_id = %group_id, "Registered group with relay server");
                Ok(true)
            }
            Err(e) => {
                debug!(group_id = %group_id, error = %e, "Failed to register group with relay");
                Ok(false)
            }
        }
    }

    /// Attempts to send a group message via relay broadcast.
    ///
    /// Sends a single `__GRP_RELAY_BCAST__` message to the user's own ID.
    /// The relay server fans it out to all registered group members as
    /// individual `__GRP_MLS_MSG__` messages.
    ///
    /// Returns the broadcast `MessageId` on success, or an error if the
    /// relay is unreachable.
    fn try_relay_broadcast(
        &mut self,
        group_id: &str,
        ciphertext_b64: &str,
        epoch: u64,
        reply_to: Option<&str>,
    ) -> Result<MessageId> {
        let payload = RelayGroupBroadcastPayload {
            group_id: group_id.to_string(),
            ciphertext: ciphertext_b64.to_string(),
            epoch,
            reply_to: reply_to.map(|s| s.to_string()),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_RELAY_BROADCAST,
            serde_json::to_string(&payload)
                .map_err(|e| Error::Other(format!("Serialize relay broadcast: {}", e)))?
        );

        let self_id = self.config.user_id.clone();
        let mid = self.send_internal_message(&self_id, content, MessagePriority::Medium)?;
        debug!(group_id = %group_id, "Sent group message via relay broadcast");
        Ok(mid)
    }

    /// Returns `true` if the Internet transport is currently available.
    pub(crate) fn is_internet_available(&self) -> bool {
        self.transport_manager
            .get_available_transports()
            .contains_key(&TransportType::Internet)
    }

    // ========================================================================
    // EPOCH FORK RESOLUTION
    // ========================================================================

    /// Checks for pending epoch forks and attempts resolution.
    ///
    /// Called from the `process()` tick. After the resolution delay, the
    /// deterministic leader (lex-first member) issues an `update_keys` commit
    /// to re-establish a canonical epoch. Members on the same branch will
    /// process the commit normally. Members on a different branch will fail
    /// to process it, at which point they need a re-invite (the leader
    /// emits `GroupEpochForkResolved` and the application layer can trigger
    /// re-invites for unreachable members).
    ///
    /// ## Stuck fork recovery
    ///
    /// If `update_keys` fails (e.g., the leader's MLS state is corrupted),
    /// `resolution_attempted` is set to `true` and no further automatic
    /// resolution is tried. The fork entry remains until the 5-minute stale
    /// cleanup removes it, at which point the fork may be re-detected if
    /// the underlying issue persists (creating a detect → fail → stale-cleanup
    /// → re-detect cycle). This is intentional: a persistent `update_keys`
    /// failure indicates the group likely needs manual intervention (e.g.,
    /// re-creating the group or re-inviting all members). The application
    /// layer should monitor `GroupEpochForkDetected` events and escalate if
    /// the same group is repeatedly detected.
    pub(crate) fn check_epoch_forks(&mut self) {
        let delay = StdDuration::from_secs(EPOCH_FORK_RESOLUTION_DELAY_SECS);
        let self_id = self.config.user_id.clone();

        // Collect forks ready for resolution
        let ready: Vec<EpochForkState> = self
            .group_mesh
            .epoch_forks
            .values()
            .filter(|f| !f.resolution_attempted && f.detected_at.elapsed() > delay)
            .cloned()
            .collect();

        for fork in ready {
            // Check if we're the leader for this group
            let members = self
                .refresh_group_members(&fork.group_id)
                .ok()
                .or_else(|| self.group_mesh.members.get(&fork.group_id).cloned())
                .unwrap_or_default();

            let mut sorted = members.clone();
            sorted.sort();
            let am_leader = sorted.first().map(|f| f == &self_id).unwrap_or(false);

            // Mark as attempted regardless — only the leader acts, but all
            // members should stop re-checking.
            if let Some(state) = self.group_mesh.epoch_forks.get_mut(&fork.group_id) {
                state.resolution_attempted = true;
            }

            if !am_leader {
                continue;
            }

            info!(
                group_id = %fork.group_id,
                local_epoch = ?fork.local_epoch,
                "Attempting epoch fork resolution as elected leader"
            );

            // Issue a key update commit to establish a canonical next epoch.
            // Members on our branch will process this normally and advance.
            // Members on a forked branch will fail — the app layer should
            // handle re-inviting them.
            //
            // NOTE: We use `read_mls_guard` (RwLock read lock) even though
            // `update_keys` mutates MLS state. This is correct because
            // `MlsManager` uses interior mutability (internal Mutex on the
            // group store), so a read guard is sufficient for all operations.
            //
            // Acquire MLS lock, perform key update, and fully release the
            // guard before touching any other &mut self state.
            let update_result = {
                if let Ok(guard) = self.read_mls_guard() {
                    let gid = offline_protocol_mls::GroupId::new(&fork.group_id);
                    let r = guard.update_keys(&gid);
                    drop(guard);
                    Some(r)
                } else {
                    None
                }
            };
            // Guard is fully dropped here — safe to use &mut self.
            let update_result = match update_result {
                Some(r) => r,
                None => {
                    warn!(group_id = %fork.group_id, "MLS unavailable during epoch fork resolution");
                    if let Some(state) = self.group_mesh.epoch_forks.get_mut(&fork.group_id) {
                        state.resolution_attempted = false;
                    }
                    continue;
                }
            };

            match update_result {
                Ok(commit_msg) => {
                    // Distribute the key-update commit to all members
                    let commit_payload = GroupMlsCommitPayload {
                        group_id: fork.group_id.clone(),
                        commit_type: GroupCommitType::KeyUpdate,
                        ciphertext: base64_encode(&commit_msg.ciphertext),
                        epoch: commit_msg.epoch,
                        affected_member: None,
                    };
                    let commit_content = match serde_json::to_string(&commit_payload) {
                        Ok(json) => format!("{}{}", internal_prefixes::GROUP_MLS_COMMIT, json),
                        Err(e) => {
                            warn!(
                                group_id = %fork.group_id,
                                error = %e,
                                "Failed to serialize fork resolution commit"
                            );
                            continue;
                        }
                    };
                    let mut failed_members = Vec::new();
                    for member in &members {
                        if member == &self_id {
                            continue;
                        }
                        if let Err(e) = self.send_internal_message(
                            member,
                            commit_content.clone(),
                            MessagePriority::High,
                        ) {
                            warn!(
                                group_id = %fork.group_id,
                                member = %member,
                                error = %e,
                                "Failed to send fork resolution commit to member"
                            );
                            failed_members.push(member.clone());
                        }
                    }

                    info!(
                        group_id = %fork.group_id,
                        new_epoch = commit_msg.epoch,
                        failed_count = failed_members.len(),
                        "Epoch fork resolution commit distributed"
                    );

                    self.emit_event(Event::group_epoch_fork_resolved(
                        fork.group_id.clone(),
                        commit_msg.epoch,
                        failed_members,
                    ));

                    // Clean up the fork state
                    self.group_mesh.epoch_forks.remove(&fork.group_id);
                }
                Err(e) => {
                    warn!(
                        group_id = %fork.group_id,
                        error = %e,
                        "Failed to issue key update for epoch fork resolution"
                    );
                    // Leave resolution_attempted = true — a persistent failure
                    // means the group may need manual intervention.
                }
            }
        }

        // Clean up stale fork entries (older than 5 minutes)
        let stale_threshold = StdDuration::from_secs(300);
        self.group_mesh
            .epoch_forks
            .retain(|_, f| f.detected_at.elapsed() < stale_threshold);
    }

    // ========================================================================
    // LEAVE ELECTION FALLBACK
    // ========================================================================

    /// Checks for timed-out leave elections and re-elects the next eligible
    /// member to issue the MLS remove-commit.
    ///
    /// Called from the `process()` tick. If the originally elected remover
    /// hasn't issued a commit within `LEAVE_ELECTION_TIMEOUT_SECS`, we check
    /// whether the leaving member is still in the group (via MLS state). If
    /// so, the next eligible member in the sorted remaining list takes over.
    pub(crate) fn check_leave_election_timeouts(&mut self) {
        let timeout = StdDuration::from_secs(LEAVE_ELECTION_TIMEOUT_SECS);
        let max_lifetime = StdDuration::from_secs(LEAVE_ELECTION_MAX_LIFETIME_SECS);
        let attempt_cooldown = StdDuration::from_secs(LEAVE_ELECTION_ATTEMPT_COOLDOWN_SECS);
        let self_id = self.config.user_id.clone();

        // Collect timed-out elections (keys + values) for processing.
        let timed_out: Vec<((String, String), PendingLeaveElection)> = self
            .group_mesh
            .pending_leave_elections
            .iter()
            .filter(|(_, e)| e.received_at.elapsed() > timeout)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (election_key, election) in timed_out {
            // Circuit breaker: abandon the election after max lifetime to
            // prevent infinite per-tick retry loops when all candidates fail.
            if election.received_at.elapsed() > max_lifetime {
                warn!(
                    group_id = %election.group_id,
                    leaving_member = %election.leaving_member,
                    "Leave election exceeded max lifetime, abandoning — member may need manual removal"
                );
                self.group_mesh
                    .pending_leave_elections
                    .remove(&election_key);
                continue;
            }

            // Refresh membership from MLS (authoritative) to avoid using
            // stale election-time lists where members may have since left.
            let current_members = self
                .refresh_group_members(&election.group_id)
                .ok()
                .or_else(|| self.group_mesh.members.get(&election.group_id).cloned())
                .unwrap_or_default();

            let still_member = current_members
                .iter()
                .any(|id| id == &election.leaving_member);

            if !still_member {
                // Already removed — clean up
                self.group_mesh
                    .pending_leave_elections
                    .remove(&election_key);
                continue;
            }

            // Use current membership for candidate selection, not the stale
            // election-time list — members may have left since the election.
            let mut remaining: Vec<String> = current_members
                .iter()
                .filter(|m| m.as_str() != election.leaving_member)
                .cloned()
                .collect();
            remaining.sort();

            // Re-elect: walk the sorted remaining members. For each timeout
            // interval that has passed, advance one position in the list.
            // This staggers re-election so members don't all fire at once.
            // Interval 1 = first fallback (idx 0), interval 2 = second (idx 1), etc.
            // The original elected member (lex-first) already had their shot
            // during handle_group_mls_leave; this path only fires after timeout.
            let elapsed_intervals =
                (election.received_at.elapsed().as_secs() / LEAVE_ELECTION_TIMEOUT_SECS) as usize;
            let candidate_idx = elapsed_intervals.min(remaining.len().saturating_sub(1));

            if remaining
                .get(candidate_idx)
                .map(|c| c == &self_id)
                .unwrap_or(false)
            {
                // Rate-limit: skip if we already attempted within the cooldown window.
                if let Some(last) = election.last_attempt_at {
                    if last.elapsed() < attempt_cooldown {
                        continue;
                    }
                }

                info!(
                    group_id = %election.group_id,
                    leaving_member = %election.leaving_member,
                    attempt = candidate_idx + 1,
                    "Re-elected to issue MLS remove-commit (prior elected member timed out)"
                );

                // Record the attempt timestamp before trying, so the cooldown
                // applies even on failure.
                if let Some(state) = self
                    .group_mesh
                    .pending_leave_elections
                    .get_mut(&election_key)
                {
                    state.last_attempt_at = Some(Instant::now());
                }

                if let Err(e) = self.remove_from_group(&election.group_id, &election.leaving_member)
                {
                    warn!(
                        group_id = %election.group_id,
                        leaving_member = %election.leaving_member,
                        error = %e,
                        "Failed to issue MLS remove-commit during re-election"
                    );
                    // Don't remove from pending — cooldown will prevent spam,
                    // next interval will advance to the next candidate.
                } else {
                    self.group_mesh
                        .pending_leave_elections
                        .remove(&election_key);
                }
            }
            // If we're not the candidate at this interval, leave the election
            // in place — either the current candidate will handle it, or the
            // next interval will advance to us.
        }
    }

    // ========================================================================
    // TRANSPORT-RESILIENT SYNC
    // ========================================================================

    /// Checks for Internet availability transitions and syncs group state.
    ///
    /// Called from the `process()` tick. On a 0→1 transition (Internet just
    /// became available), all unsynced groups are registered with the relay.
    /// On a 1→0 transition (Internet lost), relay sync flags are cleared so
    /// groups are re-registered when connectivity returns.
    pub(crate) fn check_relay_group_sync(&mut self) {
        if !self.config.group.relay_enabled {
            return;
        }
        let internet_available = self.is_internet_available();
        let was_available = self.group_mesh.internet_was_available;
        self.group_mesh.internet_was_available = internet_available;

        if internet_available && !was_available {
            // Internet just became available — sync all groups
            self.sync_groups_to_relay();
        } else if !internet_available && was_available {
            // Internet just went down — clear sync state
            self.group_mesh.relay_synced.clear();
        }
    }

    /// Re-registers all unsynced groups with the relay server.
    ///
    /// Refreshes each group's member list from MLS state before syncing so
    /// that the relay receives authoritative membership, not a stale cache.
    fn sync_groups_to_relay(&mut self) {
        let group_ids: Vec<String> = self.group_mesh.members.keys().cloned().collect();
        for group_id in group_ids {
            if self.group_mesh.relay_synced.contains(&group_id) {
                continue;
            }
            // Refresh from MLS to avoid syncing stale membership to relay
            let members = self
                .refresh_group_members(&group_id)
                .ok()
                .or_else(|| self.group_mesh.members.get(&group_id).cloned());
            if let Some(members) = members {
                let _ = self.try_relay_register_group(&group_id, None, &members);
            }
        }
    }

    // ========================================================================
    // RELAY INBOUND — MLS-AWARE ROUTING
    // ========================================================================

    /// Handles an inbound relay group message by routing through MLS decryption
    /// when the group has local MLS state.
    ///
    /// Called from `process_internal_message` when a `__GROUP_MSG__` arrives
    /// from the relay server for a group we have MLS state for. If MLS
    /// decryption fails or the content is not ciphertext, falls back to
    /// emitting the raw content.
    pub(crate) fn handle_relay_group_message_with_mls(
        &mut self,
        group_id: &str,
        sender: &str,
        content: &str,
        timestamp: &str,
        message_id: &str,
        reply_to_msg: Option<String>,
    ) {
        // Attempt base64 decode — if it fails, the content is plaintext (legacy)
        let ciphertext_bytes = match base64_decode(content) {
            Ok(bytes) => bytes,
            Err(_) => {
                // Not ciphertext — emit as plaintext
                self.emit_event(Event::group_message_received(
                    group_id.to_string(),
                    sender.to_string(),
                    content.to_string(),
                    timestamp.to_string(),
                    message_id.to_string(),
                    reply_to_msg,
                ));
                return;
            }
        };

        // Try MLS decryption
        let plaintext = {
            let mls_guard = match self.read_mls_guard() {
                Ok(guard) => guard,
                Err(_) => {
                    // MLS unavailable — emit raw content
                    self.emit_event(Event::group_message_received(
                        group_id.to_string(),
                        sender.to_string(),
                        content.to_string(),
                        timestamp.to_string(),
                        message_id.to_string(),
                        reply_to_msg,
                    ));
                    return;
                }
            };
            let gid = offline_protocol_mls::GroupId::new(group_id);
            // The epoch field is not used by MLS for decryption — OpenMLS
            // determines the epoch from the ciphertext header itself. We pass
            // 0 here because the relay protocol does not carry epoch metadata.
            let encrypted = offline_protocol_mls::EncryptedMessage {
                group_id: gid,
                message_type: offline_protocol_mls::MlsMessageType::Application,
                epoch: 0,
                ciphertext: ciphertext_bytes,
                sender_id: sender.to_string(),
                timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
            };
            match mls_guard.decrypt_from_group(&encrypted) {
                Ok(Some(pt)) => Some(pt),
                _ => None,
            }
        };

        match plaintext {
            Some(pt) => {
                let text = String::from_utf8_lossy(&pt).to_string();
                self.emit_event(Event::group_message_received(
                    group_id.to_string(),
                    sender.to_string(),
                    text,
                    timestamp.to_string(),
                    message_id.to_string(),
                    reply_to_msg,
                ));
            }
            None => {
                warn!(
                    group_id = %group_id,
                    "Failed to decrypt relay group message via MLS, emitting raw"
                );
                self.emit_event(Event::group_message_received(
                    group_id.to_string(),
                    sender.to_string(),
                    content.to_string(),
                    timestamp.to_string(),
                    message_id.to_string(),
                    reply_to_msg,
                ));
            }
        }
    }
}
