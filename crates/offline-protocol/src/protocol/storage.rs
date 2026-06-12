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
