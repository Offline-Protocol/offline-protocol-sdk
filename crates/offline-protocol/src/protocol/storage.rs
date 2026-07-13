//! Storage persistence methods for protocol state.

use super::{
    storage_keys, OfflineProtocol, OutboxEntry, PendingMessage, ReceivedKeyPackage, SessionState,
    WelcomeDeliveryState, WelcomeLifecycleRecord, MAX_PENDING_KEY_PACKAGES,
    WELCOME_LIFECYCLE_TTL_SECS,
};
use crate::constants::MAX_OUTBOX_ENTRIES;
use crate::{Error, Result};
use chrono::{Duration as ChronoDuration, Utc};
use offline_protocol_core::{LamportClock, MessageId};
use offline_protocol_mls::MlsManager;
use offline_protocol_transport::{NostrKeypair, NostrTransport, TransportType};
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

impl OfflineProtocol {
    // ========================================================================
    // PENDING MESSAGES PERSISTENCE
    // ========================================================================

    /// Persists the in-memory pending message queue for a recipient to storage.
    ///
    /// Uses `pending_encrypted_messages` as the source of truth rather than
    /// loading from storage first. The caller must push to the in-memory
    /// queue before calling this method.
    pub(crate) fn persist_pending_messages_for_recipient(&self, recipient: &str) {
        if let Some(messages) = self.pending_encrypted_messages.get(recipient) {
            self.persist_pending_messages_snapshot(recipient, messages);
        }
    }

    pub(crate) fn persist_pending_messages_snapshot(
        &self,
        recipient: &str,
        messages: &[PendingMessage],
    ) {
        let Some(storage) = &self.message_storage else {
            return;
        };

        if messages.is_empty() {
            if let Err(e) = storage.delete(storage_keys::PENDING_MESSAGES, recipient) {
                warn!(
                    recipient = %recipient,
                    error = %e,
                    "Failed to clear persisted pending messages"
                );
            }
            return;
        }

        match serde_json::to_vec(messages) {
            Ok(data) => {
                if let Err(e) = storage.store(storage_keys::PENDING_MESSAGES, recipient, &data) {
                    warn!(recipient = %recipient, error = %e, "Failed to persist pending messages");
                }
            }
            Err(e) => {
                warn!(recipient = %recipient, error = %e, "Failed to serialize pending messages");
            }
        }
    }

    /// Loads pending messages for a recipient from storage.
    pub(crate) fn load_pending_messages_from_storage(
        &self,
        recipient: &str,
    ) -> Option<Vec<PendingMessage>> {
        let storage = self.message_storage.as_ref()?;
        let data = storage
            .load(storage_keys::PENDING_MESSAGES, recipient)
            .ok()??;
        serde_json::from_slice(&data).ok()
    }

    /// Removes pending messages for a recipient from storage.
    pub(crate) fn clear_pending_messages_from_storage(&self, recipient: &str) {
        if let Some(storage) = &self.message_storage {
            let _ = storage.delete(storage_keys::PENDING_MESSAGES, recipient);
        }
    }

    /// Restores all pending messages from storage on startup.
    ///
    /// This should be called after initializing storage to recover
    /// any messages that were pending when the app was terminated.
    pub(crate) fn restore_pending_messages(&mut self) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };

        let recipients = storage
            .list_keys(storage_keys::PENDING_MESSAGES)
            .map_err(|e| Error::Other(format!("Failed to list pending messages: {}", e)))?;

        for recipient in recipients {
            if let Some(messages) = self.load_pending_messages_from_storage(&recipient) {
                if !messages.is_empty() {
                    info!(recipient = %recipient, count = messages.len(), "Restored pending messages from storage");
                    self.pending_encrypted_messages.insert(recipient, messages);
                }
            }
        }

        Ok(())
    }

    // ========================================================================
    // PEER KEY PACKAGES PERSISTENCE
    // ========================================================================

    /// Persists a received key package for a peer so it survives restart.
    pub(crate) fn persist_peer_key_package(&self, peer_id: &str, pkg: &ReceivedKeyPackage) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        match serde_json::to_vec(pkg) {
            Ok(data) => {
                if let Err(e) = storage.store(storage_keys::PEER_KEY_PACKAGES, peer_id, &data) {
                    warn!(peer_id = %peer_id, error = %e, "Failed to persist peer key package");
                }
            }
            Err(e) => {
                warn!(peer_id = %peer_id, error = %e, "Failed to serialize peer key package");
            }
        }
    }

    /// Loads a persisted key package for a peer (if present and not expired).
    pub(crate) fn load_peer_key_package_from_storage(
        &self,
        peer_id: &str,
    ) -> Option<ReceivedKeyPackage> {
        let storage = self.message_storage.as_ref()?;
        let data = storage
            .load(storage_keys::PEER_KEY_PACKAGES, peer_id)
            .ok()??;
        let pkg: ReceivedKeyPackage = serde_json::from_slice(&data).ok()?;
        let now_ms = Utc::now().timestamp_millis() as u64;
        if now_ms >= pkg.local_expires_at_ms {
            let _ = storage.delete(storage_keys::PEER_KEY_PACKAGES, peer_id);
            return None;
        }
        Some(pkg)
    }

    /// Removes persisted key package for a peer (e.g. after session created).
    pub(crate) fn delete_peer_key_package_from_storage(&self, peer_id: &str) {
        if let Some(storage) = &self.message_storage {
            let _ = storage.delete(storage_keys::PEER_KEY_PACKAGES, peer_id);
        }
    }

    /// Loads key package from storage into memory if not already present. Returns true if we now have one in memory.
    pub(crate) fn try_load_key_package_from_storage_into_memory(&mut self, peer_id: &str) -> bool {
        if self.pending_key_packages.contains_key(peer_id) {
            return true;
        }
        if let Some(pkg) = self.load_peer_key_package_from_storage(peer_id) {
            self.pending_key_packages.insert(peer_id.to_string(), pkg);
            return true;
        }
        false
    }

    /// Restores peer key packages from storage for peers that have no MLS session.
    pub(crate) fn restore_peer_key_packages(
        &mut self,
        mls: &Arc<RwLock<MlsManager>>,
    ) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };

        let peer_ids = storage
            .list_keys(storage_keys::PEER_KEY_PACKAGES)
            .map_err(|e| Error::Other(format!("Failed to list peer key packages: {}", e)))?;

        let sessions = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.list_sessions().map_err(Error::Mls)?
        };
        let session_set: std::collections::HashSet<_> = sessions.into_iter().collect();

        let mut pruned = 0usize;
        for peer_id in peer_ids {
            if session_set.contains(&peer_id) {
                continue;
            }
            // Bound restore to the same cap as the live insert path so a
            // pre-existing over-cap durable store (e.g. a flood that landed
            // before the cap existed) cannot re-inflate memory on boot. Rather
            // than leaving the overflow to linger on disk forever — where it
            // would re-inflate memory on a future boot and waste durable
            // storage — prune it so the store shrinks to the cap in a single
            // boot. Dropping a cached package only costs a recoverable
            // re-exchange, exactly like the live eviction path. Overflow is
            // deleted without loading it, so peak memory stays cap-bounded.
            if self.pending_key_packages.len() >= MAX_PENDING_KEY_PACKAGES {
                self.delete_peer_key_package_from_storage(&peer_id);
                pruned += 1;
                continue;
            }
            if let Some(pkg) = self.load_peer_key_package_from_storage(&peer_id) {
                info!(peer_id = %peer_id, "Restored peer key package from storage");
                self.pending_key_packages.insert(peer_id, pkg);
            }
        }
        if pruned > 0 {
            warn!(
                cap = MAX_PENDING_KEY_PACKAGES,
                pruned,
                "Peer key package store exceeded the cap on restore; pruned overflow from durable storage"
            );
        }

        Ok(())
    }

    // ========================================================================
    // SESSION STATE PERSISTENCE
    // ========================================================================

    /// Loads a persisted session state entry (if present).
    pub(crate) fn load_session_state_entry(&self, peer_id: &str) -> Result<Option<SessionState>> {
        let Some(storage) = &self.message_storage else {
            return Ok(None);
        };

        let Some(data) = storage
            .load(storage_keys::SESSION_STATES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to load session state for {}: {}",
                    peer_id, e
                ))
            })?
        else {
            return Ok(None);
        };

        let state = serde_json::from_slice::<SessionState>(&data).map_err(|e| {
            Error::Other(format!(
                "Failed to deserialize session state for {}: {}",
                peer_id, e
            ))
        })?;

        Ok(Some(state))
    }

    /// Persists session state for a single peer key.
    pub(crate) fn persist_session_state(
        &self,
        peer_id: &str,
        new_state: SessionState,
        source_event: &str,
    ) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Err(Error::MlsNotInitialized);
        };

        let encoded = serde_json::to_vec(&new_state).map_err(|e| {
            Error::Serialization(format!("Failed to serialize session state: {}", e))
        })?;
        storage
            .store(storage_keys::SESSION_STATES, peer_id, &encoded)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to persist session state for {}: {}",
                    peer_id, e
                ))
            })?;

        if matches!(new_state, SessionState::Confirmed) {
            info!(
                event = "confirmation_persisted",
                session_or_group_id = %peer_id,
                previous_state = "Pending",
                new_state = "Confirmed",
                source_event = %source_event,
                "confirmation_persisted"
            );
        }

        Ok(())
    }

    pub(crate) fn clear_session_state_entry(&self, peer_id: &str) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };
        storage
            .delete(storage_keys::SESSION_STATES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to clear session state for {}: {}",
                    peer_id, e
                ))
            })
    }

    // ========================================================================
    // WELCOME LIFECYCLE PERSISTENCE
    // ========================================================================

    pub(crate) fn load_welcome_lifecycle_entry(
        &self,
        peer_id: &str,
    ) -> Result<Option<WelcomeLifecycleRecord>> {
        let Some(storage) = &self.message_storage else {
            return Ok(None);
        };

        let Some(data) = storage
            .load(storage_keys::WELCOME_LIFECYCLES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to load welcome lifecycle for {}: {}",
                    peer_id, e
                ))
            })?
        else {
            return Ok(None);
        };

        let record = serde_json::from_slice::<WelcomeLifecycleRecord>(&data).map_err(|e| {
            Error::Other(format!(
                "Failed to deserialize welcome lifecycle for {}: {}",
                peer_id, e
            ))
        })?;
        Ok(Some(record))
    }

    pub(crate) fn persist_welcome_lifecycle_entry(
        &self,
        record: &WelcomeLifecycleRecord,
    ) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Err(Error::MlsNotInitialized);
        };

        let encoded = serde_json::to_vec(record).map_err(|e| {
            Error::Serialization(format!("Failed to serialize welcome lifecycle: {}", e))
        })?;
        storage
            .store(storage_keys::WELCOME_LIFECYCLES, &record.peer_id, &encoded)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to persist welcome lifecycle for {}: {}",
                    record.peer_id, e
                ))
            })
    }

    pub(crate) fn clear_welcome_lifecycle_entry(&self, peer_id: &str) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };
        storage
            .delete(storage_keys::WELCOME_LIFECYCLES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to clear welcome lifecycle for {}: {}",
                    peer_id, e
                ))
            })
    }

    pub(crate) fn restore_welcome_lifecycles(&mut self) -> Result<()> {
        self.welcome_lifecycles.clear();
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };

        let peers = storage
            .list_keys(storage_keys::WELCOME_LIFECYCLES)
            .map_err(|e| Error::Other(format!("Failed to list welcome lifecycles: {}", e)))?;

        for peer_id in peers {
            if let Some(mut record) = self.load_welcome_lifecycle_entry(&peer_id)? {
                if matches!(
                    record.state,
                    WelcomeDeliveryState::Created | WelcomeDeliveryState::SendAttempted
                ) {
                    record.state = WelcomeDeliveryState::Failed;
                    record.next_retry_at = Some(Utc::now());
                    self.persist_welcome_lifecycle_entry(&record)?;
                    warn!(
                        event = "welcome_lifecycle_repaired",
                        session_or_group_id = %peer_id,
                        repair_action = "in_flight_to_failed_retry_now",
                        state = record.state.as_str(),
                        attempt = record.attempt,
                        "welcome_lifecycle_repaired"
                    );
                }
                if matches!(record.state, WelcomeDeliveryState::Failed)
                    && record.next_retry_at.is_none()
                {
                    if matches!(
                        record.last_reason_code,
                        Some(crate::events::WelcomeReasonCode::RetryExhausted)
                    ) {
                        // Only genuine retry exhaustion (a present carrier that
                        // kept failing) is terminal here. A stale TTL alone must
                        // NOT expire a no-carrier Welcome on restart — the TTL
                        // clock is carrier-relative and is refreshed below.
                        record.state = WelcomeDeliveryState::Expired;
                        warn!(
                            event = "welcome_lifecycle_repaired",
                            session_or_group_id = %peer_id,
                            repair_action = "failed_no_retry_to_expired",
                            state = record.state.as_str(),
                            attempt = record.attempt,
                            "welcome_lifecycle_repaired"
                        );
                    } else {
                        // Recover from partial-crash write where Failed was persisted
                        // without a retry schedule.
                        record.next_retry_at = Some(Utc::now());
                        warn!(
                            event = "welcome_lifecycle_repaired",
                            session_or_group_id = %peer_id,
                            repair_action = "failed_no_retry_to_failed_retry_now",
                            state = record.state.as_str(),
                            attempt = record.attempt,
                            "welcome_lifecycle_repaired"
                        );
                    }
                    self.persist_welcome_lifecycle_entry(&record)?;
                }
                // The TTL clock is carrier-relative: a Welcome must not be
                // restored already-expired after an offline period. Restart the
                // window for any non-terminal lifecycle whose TTL has lapsed so
                // it gets a fresh chance once a carrier (or the peer) reappears.
                if matches!(record.state, WelcomeDeliveryState::Failed)
                    && record.expires_at <= Utc::now()
                {
                    record.expires_at =
                        Utc::now() + ChronoDuration::seconds(WELCOME_LIFECYCLE_TTL_SECS);
                    self.persist_welcome_lifecycle_entry(&record)?;
                    warn!(
                        event = "welcome_lifecycle_repaired",
                        session_or_group_id = %peer_id,
                        repair_action = "ttl_refreshed_carrier_relative",
                        state = record.state.as_str(),
                        attempt = record.attempt,
                        "welcome_lifecycle_repaired"
                    );
                }
                if matches!(
                    record.state,
                    WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
                ) && record.next_retry_at.is_some()
                {
                    record.next_retry_at = None;
                    self.persist_welcome_lifecycle_entry(&record)?;
                    warn!(
                        event = "welcome_lifecycle_repaired",
                        session_or_group_id = %peer_id,
                        repair_action = "terminal_clear_retry_schedule",
                        state = record.state.as_str(),
                        attempt = record.attempt,
                        "welcome_lifecycle_repaired"
                    );
                }
                self.welcome_lifecycles.insert(peer_id.clone(), record);
                info!(
                    event = "welcome_lifecycle_restored",
                    session_or_group_id = %peer_id,
                    "welcome_lifecycle_restored"
                );
            }
        }

        Ok(())
    }

    // ========================================================================
    // OUTBOX PERSISTENCE
    // ========================================================================

    /// Persists a single outbox entry to storage, keyed by message id.
    ///
    /// Best-effort and infallible: the send path cannot propagate storage
    /// errors, so a failed write is logged and swallowed (the message still
    /// lives in the in-memory outbox and will retry; it just won't survive a
    /// restart). No-ops when persistence is not configured or when the entry
    /// belongs to the media outbox — file transfers are not persisted and
    /// resurrected chunks could never complete, so we never write them.
    pub(crate) fn persist_outbox_entry(&self, entry: &OutboxEntry) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        if Self::is_media_outbox_message(&entry.message) {
            return;
        }
        match serde_json::to_vec(entry) {
            Ok(data) => {
                if let Err(e) =
                    storage.store(storage_keys::OUTBOX, &entry.message.id.as_str(), &data)
                {
                    warn!(message_id = %entry.message.id, error = %e, "Failed to persist outbox entry");
                }
            }
            Err(e) => {
                warn!(message_id = %entry.message.id, error = %e, "Failed to serialize outbox entry");
            }
        }
    }

    /// Removes a persisted outbox entry from storage. Best-effort: a media
    /// message id is never persisted, so deleting it is a harmless no-op.
    pub(crate) fn clear_outbox_entry_from_storage(&self, message_id: &MessageId) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        if let Err(e) = storage.delete(storage_keys::OUTBOX, &message_id.as_str()) {
            warn!(message_id = %message_id, error = %e, "Failed to clear persisted outbox entry");
        }
    }

    /// Restores the store-and-forward outbox from storage on startup.
    ///
    /// Merges persisted entries into `self.outbox` — it is *not* cleared first.
    /// An entry queued before persistence was enabled lives only in memory and,
    /// if it succeeded at the transport but is awaiting an ACK, is not in the
    /// retry queue either; clearing would strand it with no recovery path (the
    /// ACK-timeout path drops a message that is missing from the outbox). Where
    /// an id exists in both, storage is authoritative and overwrites.
    ///
    /// The retry queue and ACK manager start empty, so every restored entry
    /// lands in the "stranded" state that [`Self::flush_outbox_all`] already
    /// recovers — a flush on `start()` re-drives delivery.
    ///
    /// Recovery rules, mirroring the other restore paths:
    /// - corrupted entries are dropped from storage and skipped;
    /// - any stray media entry is dropped (they must never be resurrected);
    /// - the total is pruned to `MAX_OUTBOX_ENTRIES`, keeping the newest by
    ///   `last_sent_at`;
    /// - the TTL clock is carrier-relative: an entry whose `last_sent_at` has
    ///   already lapsed the outbox lifetime is refreshed rather than restored
    ///   already-expired, so it gets a fresh delivery window once a carrier
    ///   reappears (mirrors the Welcome lifecycle repair). This runs *after*
    ///   the prune, so a refreshed (now-stamped) clock can't sort as the newest
    ///   and crowd genuinely-fresh entries out of the kept set;
    /// - pre-existing in-memory entries not yet in storage are persisted, so
    ///   memory and storage are consistent once restore returns.
    pub(crate) fn restore_outbox(&mut self) -> Result<()> {
        let Some(storage) = &self.message_storage else {
            return Ok(());
        };

        let message_ids = storage
            .list_keys(storage_keys::OUTBOX)
            .map_err(|e| Error::Other(format!("Failed to list outbox entries: {}", e)))?;

        let lifetime = ChronoDuration::milliseconds(
            self.config.reliability.retry.outbox_max_lifetime_ms as i64,
        );

        let mut restored: Vec<OutboxEntry> = Vec::new();
        for message_id in message_ids {
            let loaded = self
                .message_storage
                .as_ref()
                .and_then(|s| s.load(storage_keys::OUTBOX, &message_id).ok().flatten());
            let Some(data) = loaded else {
                continue;
            };

            let entry = match serde_json::from_slice::<OutboxEntry>(&data) {
                Ok(entry) => entry,
                Err(e) => {
                    warn!(message_id = %message_id, error = %e, "Dropping corrupted outbox entry");
                    self.delete_outbox_key(&message_id);
                    continue;
                }
            };

            // A media entry should never have been persisted; drop any that
            // slipped in (e.g. from an older build) so it can't be resurrected.
            if Self::is_media_outbox_message(&entry.message) {
                warn!(message_id = %message_id, "Dropping persisted media outbox entry");
                self.delete_outbox_key(&message_id);
                continue;
            }

            restored.push(entry);
        }

        // Prune to capacity BEFORE refreshing TTLs, keeping the newest by
        // last_sent_at. Delete the pruned overflow from storage so it can't
        // linger and be re-restored. Ordering matters: refreshing a lapsed
        // clock stamps it with `now`, which would otherwise sort it as the
        // newest and crowd genuinely-fresh entries out of the kept set.
        if restored.len() > MAX_OUTBOX_ENTRIES {
            restored.sort_by_key(|e| std::cmp::Reverse(e.last_sent_at));
            for entry in restored.drain(MAX_OUTBOX_ENTRIES..) {
                self.delete_outbox_key(&entry.message.id.as_str());
            }
        }

        // Carrier-relative TTL: refresh any entry already past the outbox
        // lifetime so it survives the first cleanup tick and gets a fresh chance
        // once a carrier appears. Collect the refreshed clones so the repair can
        // be re-persisted below, once the mutable borrow of `restored` is gone.
        let now = Utc::now();
        let mut refreshed: Vec<OutboxEntry> = Vec::new();
        for entry in &mut restored {
            if entry.last_sent_at + lifetime <= now {
                entry.last_sent_at = now;
                refreshed.push(entry.clone());
                info!(
                    event = "outbox_entry_restored",
                    message_id = %entry.message.id,
                    repair_action = "ttl_refreshed_carrier_relative",
                    "outbox_entry_restored"
                );
            }
        }

        // Pre-existing in-memory entries not backed by storage (queued before
        // persistence was enabled) must be persisted too, so they survive the
        // next restart and memory/storage stay consistent after restore.
        let restored_ids: std::collections::HashSet<String> =
            restored.iter().map(|e| e.message.id.as_str()).collect();
        let orphans: Vec<OutboxEntry> = self
            .outbox
            .values()
            .filter(|e| !restored_ids.contains(&e.message.id.as_str()))
            .cloned()
            .collect();

        let count = restored.len();
        for entry in restored {
            self.outbox.insert(entry.message.id.clone(), entry);
        }
        for entry in &refreshed {
            self.persist_outbox_entry(entry);
        }
        for entry in &orphans {
            self.persist_outbox_entry(entry);
        }
        if count > 0 {
            info!(count = count, "Restored outbox entries from storage");
        }

        Ok(())
    }

    /// Deletes an outbox key from storage without the media/no-storage guards
    /// of [`Self::clear_outbox_entry_from_storage`] — used inside
    /// [`Self::restore_outbox`], which already holds a storage handle and
    /// operates on raw persisted keys.
    fn delete_outbox_key(&self, message_id: &str) {
        if let Some(storage) = &self.message_storage {
            if let Err(e) = storage.delete(storage_keys::OUTBOX, message_id) {
                warn!(message_id = %message_id, error = %e, "Failed to delete outbox key");
            }
        }
    }

    // ========================================================================
    // BLOCKED USERS PERSISTENCE
    // ========================================================================

    /// Persists a blocked user entry to storage.
    pub(crate) fn persist_blocked_user(&self, user_id: &str) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        if let Err(e) = storage.store(storage_keys::BLOCKED_USERS, user_id, &[]) {
            warn!(user_id = %user_id, error = %e, "Failed to persist blocked user");
        }
    }

    /// Deletes a blocked user entry from storage.
    pub(crate) fn delete_blocked_user(&self, user_id: &str) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        if let Err(e) = storage.delete(storage_keys::BLOCKED_USERS, user_id) {
            warn!(user_id = %user_id, error = %e, "Failed to delete blocked user from storage");
        }
    }

    /// Restores blocked users from persistent storage.
    ///
    /// Skips entries with invalid user IDs (best-effort restore).
    pub(crate) fn restore_blocked_users(&mut self) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        let user_ids = match storage.list_keys(storage_keys::BLOCKED_USERS) {
            Ok(keys) => keys,
            Err(e) => {
                warn!(error = %e, "Failed to list blocked users from storage");
                return;
            }
        };
        for user_id in &user_ids {
            if offline_protocol_core::UserId::new(user_id).is_err() {
                warn!(user_id = %user_id, "Skipping blocked user entry with invalid user ID");
                continue;
            }
            self.blocked_users.insert(user_id.clone());
        }
        if !self.blocked_users.is_empty() {
            info!(
                count = self.blocked_users.len(),
                "Restored blocked users from storage"
            );
        }
    }

    // ========================================================================
    // BOTH-CREATE OWNER GATE PERSISTENCE
    // ========================================================================

    /// Persists a both-create owner-gate entry (value-less; the key is the peer).
    pub(crate) fn persist_both_create_awaiting_decrypt(&self, peer_id: &str) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        if let Err(e) = storage.store(storage_keys::BOTH_CREATE_AWAITING_DECRYPT, peer_id, &[]) {
            warn!(peer_id = %peer_id, error = %e, "Failed to persist both-create owner gate");
        }
    }

    /// Deletes a both-create owner-gate entry once the peer has converged.
    pub(crate) fn delete_both_create_awaiting_decrypt(&self, peer_id: &str) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        if let Err(e) = storage.delete(storage_keys::BOTH_CREATE_AWAITING_DECRYPT, peer_id) {
            warn!(peer_id = %peer_id, error = %e, "Failed to delete both-create owner gate");
        }
    }

    /// Restores the both-create owner gate from storage on startup, so an owner
    /// that restarted mid-convergence keeps requiring a group-aware decrypt
    /// before confirming a still-pending peer. Stale entries for already-confirmed
    /// peers are harmless (confirmation short-circuits) and are cleared on the
    /// next confirm.
    pub(crate) fn restore_both_create_awaiting_decrypt(&mut self) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        let peer_ids = match storage.list_keys(storage_keys::BOTH_CREATE_AWAITING_DECRYPT) {
            Ok(keys) => keys,
            Err(e) => {
                warn!(error = %e, "Failed to list both-create owner gate from storage");
                return;
            }
        };
        for peer_id in &peer_ids {
            self.both_create_awaiting_decrypt.insert(peer_id.clone());
        }
        if !self.both_create_awaiting_decrypt.is_empty() {
            info!(
                count = self.both_create_awaiting_decrypt.len(),
                "Restored both-create owner gate from storage"
            );
        }
    }

    // ========================================================================
    // TELEMETRY SCRUB-SECRET PERSISTENCE
    // ========================================================================

    /// Loads (or, on first run, generates and persists) the per-install
    /// telemetry scrub secret, then installs it as the fallback secret used
    /// for opaque-identifier hashing when no explicit `scrub_secret` is set on
    /// the installed `TelemetryConfig`.
    ///
    /// This makes opaque identifiers stable across process restarts so backend
    /// telemetry can count distinct devices: the same device hashes to the same
    /// opaque id every session. Until storage is available (or if storage is
    /// never provided), the SDK keeps using the random per-instance fallback
    /// generated at construction — so this is purely an upgrade over the
    /// random fallback, never a regression.
    ///
    /// Secret precedence is unchanged: an explicit
    /// [`crate::telemetry::TelemetryConfig::with_scrub_secret`] still wins over
    /// this persistent fallback (see [`crate::telemetry::Scrubber::from_config`]).
    ///
    /// Idempotent across the two storage-entry paths (`initialize_mls` and
    /// `enable_message_persistence`) via `telemetry_secret_persisted`. All
    /// storage failures degrade gracefully to the in-memory random fallback —
    /// telemetry pseudonymization must never block protocol initialization.
    pub(crate) fn restore_or_init_scrub_secret(&mut self) {
        if self.telemetry_secret_persisted {
            return;
        }
        let Some(storage) = &self.message_storage else {
            return;
        };

        let secret: [u8; 16] = match storage
            .load(storage_keys::SCRUB_SECRET, storage_keys::SCRUB_SECRET_ID)
        {
            Ok(Some(bytes)) if bytes.len() == 16 => {
                let mut secret = [0u8; 16];
                secret.copy_from_slice(&bytes);
                debug!("Restored persistent telemetry scrub secret from storage");
                secret
            }
            Ok(other) => {
                // Absent, or present but corrupt/wrong-length: generate a fresh
                // secret and persist it. A wrong-length blob is overwritten so a
                // single corrupt write does not pin every future session to the
                // random fallback.
                if other.is_some() {
                    warn!("Persisted scrub secret had unexpected length; regenerating");
                }
                let fresh = *uuid::Uuid::new_v4().as_bytes();
                if let Err(e) = storage.store(
                    storage_keys::SCRUB_SECRET,
                    storage_keys::SCRUB_SECRET_ID,
                    &fresh,
                ) {
                    // Keep the in-memory secret for this session; next launch
                    // will retry persistence. Opaque ids stay stable within
                    // this process but may differ next session — strictly no
                    // worse than the legacy random fallback.
                    warn!(error = %e, "Failed to persist telemetry scrub secret; using session-local secret");
                    return self.adopt_fallback_secret(fresh);
                }
                info!("Generated and persisted per-install telemetry scrub secret");
                fresh
            }
            Err(e) => {
                warn!(error = %e, "Failed to load telemetry scrub secret; keeping random fallback");
                return;
            }
        };

        self.telemetry_secret_persisted = true;
        self.adopt_fallback_secret(secret);
    }

    // ========================================================================
    // NOSTR SIGNING-SECRET PERSISTENCE
    // ========================================================================

    /// Loads (or, on first run, generates and persists) the per-install Nostr
    /// signing secret and installs the derived signing key into the Nostr
    /// transport, replacing the ephemeral key it was constructed with.
    ///
    /// This gives the install a stable Nostr identity (event signatures,
    /// relay-visible pubkey) across restarts. The signing key is intentionally
    /// not derivable from any public identifier (SEC-M4); message addressing
    /// is unaffected because it uses the separate routing tag, which remains
    /// derived from the device ID.
    ///
    /// Idempotent across the two storage-entry paths (`initialize_mls` and
    /// `enable_message_persistence`) via `nostr_secret_persisted`. All
    /// failures degrade gracefully to the construction-time ephemeral key,
    /// which is equally unforgeable but rotates per process — transport
    /// keying must never block protocol initialization. A secret that was
    /// installed but could not be persisted is kept in
    /// `nostr_unpersisted_secret` so a later attempt retries persisting the
    /// same identity instead of rotating it.
    pub(crate) fn restore_or_init_nostr_signing_secret(&mut self) {
        if self.nostr_secret_persisted {
            return;
        }
        let Some(storage) = self.message_storage.clone() else {
            return;
        };
        let Some(nostr_arc) = self.transport_manager.get_transport(TransportType::Nostr) else {
            // Nostr transport not registered — nothing to key.
            return;
        };

        let (secret, persisted): (Zeroizing<[u8; 32]>, bool) = match storage.load(
            storage_keys::NOSTR_SIGNING_SECRET,
            storage_keys::NOSTR_SIGNING_SECRET_ID,
        ) {
            Ok(Some(bytes)) if bytes.len() == 32 => {
                let bytes = Zeroizing::new(bytes);
                let mut secret = Zeroizing::new([0u8; 32]);
                secret.copy_from_slice(&bytes);
                // A stored secret supersedes anything a previous attempt
                // failed to persist.
                self.nostr_unpersisted_secret = None;
                debug!("Restored persistent Nostr signing secret from storage");
                (secret, true)
            }
            Ok(other) => {
                // Absent, or present but corrupt/wrong-length: persist a
                // fresh secret (a wrong-length blob is overwritten so a
                // single corrupt write does not pin every future session to
                // the ephemeral key). Prefer a secret a previous attempt
                // installed but failed to persist, so the retry keeps the
                // identity already in use instead of rotating it again.
                if other.is_some() {
                    warn!("Persisted Nostr signing secret had unexpected length; regenerating");
                }
                let fresh = match self.nostr_unpersisted_secret.take() {
                    Some(unpersisted) => unpersisted,
                    None => match NostrKeypair::generate_install_secret() {
                        Ok(fresh) => fresh,
                        Err(e) => {
                            warn!(error = %e, "Failed to generate Nostr signing secret; keeping ephemeral key");
                            return;
                        }
                    },
                };
                match storage.store(
                    storage_keys::NOSTR_SIGNING_SECRET,
                    storage_keys::NOSTR_SIGNING_SECRET_ID,
                    &*fresh,
                ) {
                    Ok(()) => {
                        info!("Generated and persisted per-install Nostr signing secret");
                        (fresh, true)
                    }
                    Err(e) => {
                        // Install the unpersisted secret anyway: the identity
                        // is stable for this session and the next entry path
                        // or launch retries persistence — strictly no worse
                        // than the ephemeral key.
                        warn!(error = %e, "Failed to persist Nostr signing secret; Nostr identity is session-local");
                        (fresh, false)
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to load Nostr signing secret; keeping ephemeral key");
                return;
            }
        };

        let installed = match nostr_arc.as_any().downcast_ref::<NostrTransport>() {
            Some(nostr) => match nostr.install_signing_secret(&*secret) {
                Ok(()) => true,
                Err(e) => {
                    warn!(error = %e, "Failed to install Nostr signing key; keeping ephemeral key");
                    false
                }
            },
            None => {
                warn!("Transport registered as Nostr is not a NostrTransport; cannot install signing key");
                false
            }
        };

        if !persisted {
            // Keep the secret so the next entry path retries persisting this
            // same identity rather than generating a new one.
            self.nostr_unpersisted_secret = Some(secret);
        }
        self.nostr_secret_persisted = installed && persisted;
    }

    /// Installs `secret` as the telemetry fallback secret and rebuilds the
    /// pre-install scrubber so the legacy MLS observability path also hashes
    /// with the stable secret. Does not touch an already-installed
    /// [`crate::telemetry::TelemetryContext`] (rebuilding a live context would
    /// rotate opaque ids mid-run); apps that need stable ids should provide
    /// storage before installing a telemetry sink.
    fn adopt_fallback_secret(&mut self, secret: [u8; 16]) {
        self.telemetry_fallback_secret = secret;
        self.telemetry_scrubber = crate::telemetry::Scrubber::from_config(
            &crate::telemetry::TelemetryConfig::default(),
            secret,
        );
    }

    /// Returns a stable, opaque per-install telemetry identifier, or `None`
    /// until the persistent scrub secret is available.
    ///
    /// The id is `SHA-256(secret || domain)` truncated to a 32-character hex
    /// string, where `secret` is the per-install scrub secret managed by
    /// [`Self::restore_or_init_scrub_secret`]. The secret cannot be recovered
    /// from the id, and the fixed domain string keeps the id un-correlatable
    /// with opaque identifiers the scrubber produces for telemetry records:
    /// the domain contains `:`, which id validation
    /// (`offline_protocol_core::types::validate_id_chars`) rejects in every
    /// `UserId`/`AppId`, so no validated identifier reaching the scrubber can
    /// ever equal the domain and collide with the install id.
    ///
    /// Returns `None` while the SDK is still on the random per-instance
    /// fallback secret — i.e. before storage is provided via
    /// [`super::OfflineProtocol::initialize_mls`] /
    /// [`super::OfflineProtocol::enable_message_persistence`], or when
    /// persistence failed this session. In that state the id would not be
    /// stable across launches, so none is exposed.
    ///
    /// Deliberately derived from the persistent fallback secret, not from an
    /// installed [`crate::telemetry::TelemetryConfig::with_scrub_secret`]
    /// override: the install id must not rotate when a sink is (re)installed,
    /// and must not be computable from an app-chosen secret.
    ///
    /// The domain string is part of the public contract: changing it would
    /// silently rotate every device's install id. Frozen — do not edit.
    pub fn telemetry_install_id(&self) -> Option<String> {
        const TELEMETRY_INSTALL_ID_DOMAIN: &str = "telemetry:install-id";
        self.telemetry_secret_persisted.then(|| {
            crate::telemetry::scrubber::opaque_id(
                TELEMETRY_INSTALL_ID_DOMAIN,
                &self.telemetry_fallback_secret,
            )
        })
    }

    // ========================================================================
    // LAMPORT CLOCK PERSISTENCE
    // ========================================================================

    /// Debounced Lamport clock persistence. Only writes to storage when the
    /// in-memory value has advanced past `last_persisted_lamport` by at least
    /// `LAMPORT_PERSIST_INTERVAL` ticks. This avoids a Keychain/Keystore
    /// write on every sent and received message.
    pub(crate) fn persist_lamport_clock(&mut self) {
        let current = self.lamport_clock.value();
        if current.wrapping_sub(self.last_persisted_lamport) < super::LAMPORT_PERSIST_INTERVAL {
            return;
        }
        self.write_lamport_clock_to_storage(current);
    }

    /// Forces the Lamport clock to storage regardless of debounce state.
    /// Called on shutdown to avoid losing any un-flushed ticks.
    pub(crate) fn flush_lamport_clock(&mut self) {
        self.write_lamport_clock_to_storage(self.lamport_clock.value());
    }

    fn write_lamport_clock_to_storage(&mut self, value: u64) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        let bytes = value.to_le_bytes();
        if let Err(e) = storage.store(
            storage_keys::LAMPORT_CLOCK,
            storage_keys::LAMPORT_CLOCK_ID,
            &bytes,
        ) {
            warn!(error = %e, "Failed to persist Lamport clock");
            return;
        }
        self.last_persisted_lamport = value;
    }

    /// Restores the Lamport clock from storage.
    ///
    /// Uses `max(current, restored)` so the clock never goes backward even
    /// if the in-memory value has advanced before storage was attached.
    pub(crate) fn restore_lamport_clock(&mut self) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        if let Ok(Some(data)) =
            storage.load(storage_keys::LAMPORT_CLOCK, storage_keys::LAMPORT_CLOCK_ID)
        {
            if data.len() == 8 {
                let restored = u64::from_le_bytes(data.try_into().expect("verified length is 8"));
                let restored_clock = LamportClock::from_value(restored);
                if restored_clock > self.lamport_clock {
                    self.lamport_clock = restored_clock;
                }
                self.last_persisted_lamport = self.lamport_clock.value();
                debug!(clock = %self.lamport_clock, "Restored Lamport clock from storage");
            } else {
                warn!(
                    len = data.len(),
                    "Corrupted Lamport clock in storage (expected 8 bytes), starting fresh"
                );
            }
        }
    }
}
