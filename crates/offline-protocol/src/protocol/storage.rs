//! Storage persistence methods for protocol state.

use super::{
    storage_keys, OfflineProtocol, PendingMessage, ReceivedKeyPackage, SessionState,
    WelcomeDeliveryState, WelcomeLifecycleRecord,
};
use crate::{Error, Result};
use chrono::Utc;
use offline_protocol_core::LamportClock;
use offline_protocol_mls::MlsManager;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

impl OfflineProtocol {
    // ========================================================================
    // PENDING MESSAGES PERSISTENCE
    // ========================================================================

    /// Persists a pending message for a recipient to storage.
    ///
    /// This ensures messages survive app crashes/restarts.
    pub(crate) fn persist_pending_message(&self, recipient: &str, pending: &PendingMessage) {
        // Load existing messages for this recipient
        let mut messages: Vec<PendingMessage> = self
            .load_pending_messages_from_storage(recipient)
            .unwrap_or_default();

        // Add the new message
        messages.push(pending.clone());

        self.persist_pending_messages_snapshot(recipient, &messages);
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

        for peer_id in peer_ids {
            if session_set.contains(&peer_id) {
                continue;
            }
            if let Some(pkg) = self.load_peer_key_package_from_storage(&peer_id) {
                info!(peer_id = %peer_id, "Restored peer key package from storage");
                self.pending_key_packages.insert(peer_id, pkg);
            }
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

    /// Persists session state atomically for a single peer key.
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
        let persisted_data = storage
            .load(storage_keys::SESSION_STATES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to verify persisted session state for {}: {}",
                    peer_id, e
                ))
            })?
            .ok_or_else(|| {
                Error::Other(format!(
                    "Persisted session state missing immediately after write for {}",
                    peer_id
                ))
            })?;
        let persisted_state =
            serde_json::from_slice::<SessionState>(&persisted_data).map_err(|e| {
                Error::Other(format!(
                    "Failed to deserialize verified session state for {}: {}",
                    peer_id, e
                ))
            })?;
        if persisted_state != new_state {
            return Err(Error::Other(format!(
                "Session state verification mismatch for {}: expected {}, got {}",
                peer_id,
                new_state.as_str(),
                persisted_state.as_str()
            )));
        }

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
                    ) || record.expires_at <= Utc::now()
                    {
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
    // LAMPORT CLOCK PERSISTENCE
    // ========================================================================

    pub(crate) fn persist_lamport_clock(&self) {
        let Some(storage) = &self.message_storage else {
            return;
        };
        let value = self.lamport_clock.value().to_le_bytes();
        if let Err(e) = storage.store(
            storage_keys::LAMPORT_CLOCK,
            storage_keys::LAMPORT_CLOCK_ID,
            &value,
        ) {
            warn!(error = %e, "Failed to persist Lamport clock");
        }
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
