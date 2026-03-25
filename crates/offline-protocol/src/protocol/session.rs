//! Session confirmation, welcome lifecycle, and pending session reconciliation.

use super::{
    internal_prefixes, lock_shared_state, OfflineProtocol, SessionState, WelcomeDeliveryState,
    WelcomeLifecycleRecord, CONFIRMATION_PROBE_INTERVAL_SECS, CONFIRMATION_RETRY_INTERVAL_SECS,
    RECONCILIATION_THROTTLE_MS, WELCOME_INTERNET_CONFIRM_TIMEOUT_SECS, WELCOME_LIFECYCLE_TTL_SECS,
    WELCOME_RETRY_BATCH_SIZE, WELCOME_RETRY_JITTER_RATIO,
};
use crate::mls_observability::MlsOperationContext;
use crate::{Error, EstablishmentState, Event, Result, SessionStateError};
use chrono::{Duration as ChronoDuration, Utc};
use offline_protocol_core::{Message, MessagePriority};
use offline_protocol_mls::{MlsManager, WelcomeMessage};
use offline_protocol_transport::TransportType;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, RwLock};
use std::time::{Duration as StdDuration, Instant};
use tracing::{debug, info, warn};

impl OfflineProtocol {
    // ========================================================================
    // SESSION STATE
    // ========================================================================

    /// Returns true when persisted state marks this peer session as confirmed.
    ///
    /// Uses the in-memory `confirmed_sessions` cache as a fast path to avoid
    /// hitting persistent storage on every message send. The cache is populated
    /// when sessions are confirmed and cleared when sessions are invalidated.
    pub(super) fn is_session_confirmed(&mut self, peer_id: &str) -> Result<bool> {
        // Fast path: check in-memory cache only — no I/O. The cache is kept in
        // sync by confirm_session_state() and all invalidation paths (blocking,
        // stale cleanup, etc.). We deliberately skip the has_mls_session() guard
        // here because it calls load_group() which hits MlsStorage (10-100ms on
        // mobile Keychain/Keystore). If the session was wiped externally, the
        // encrypt call will fail and we handle it there.
        if self.confirmed_sessions.contains(peer_id) {
            return Ok(true);
        }

        let persisted = self
            .load_session_state_entry(peer_id)?
            .unwrap_or(SessionState::Pending);
        if matches!(persisted, SessionState::Confirmed) {
            if !self.has_mls_session(peer_id)? {
                warn!(
                    peer_id = %peer_id,
                    "Persisted confirmed state has no matching MLS session; clearing stale state"
                );
                self.confirmed_sessions.remove(peer_id);
                self.clear_confirmation_recovery_tracking(peer_id);
                self.welcome_lifecycles.remove(peer_id);
                if let Err(err) = self.clear_session_state_entry(peer_id) {
                    warn!(
                        peer_id = %peer_id,
                        error = %err,
                        "Failed to clear stale persisted session state"
                    );
                }
                if let Err(err) = self.clear_welcome_lifecycle_entry(peer_id) {
                    warn!(
                        peer_id = %peer_id,
                        error = %err,
                        "Failed to clear stale persisted welcome lifecycle"
                    );
                }
                return Ok(false);
            }
            self.confirmed_sessions.insert(peer_id.to_string());
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn has_mls_session(&self, peer_id: &str) -> Result<bool> {
        let Some(mls) = self.mls_manager.clone() else {
            return Ok(false);
        };

        let manager = mls
            .read()
            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
        manager.has_session(peer_id).map_err(Error::Mls)
    }

    /// Returns the current establishment state for a peer (for API and error reporting).
    pub(super) fn establishment_state(&self, peer_id: &str) -> Result<EstablishmentState> {
        if let Some(mls) = &self.mls_manager {
            let has_session = {
                let manager = mls
                    .read()
                    .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                manager.has_session(peer_id).map_err(Error::Mls)?
            };
            if has_session {
                let state = self
                    .load_session_state_entry(peer_id)?
                    .unwrap_or(SessionState::Pending);
                return Ok(if matches!(state, SessionState::Confirmed) {
                    EstablishmentState::SessionConfirmed
                } else {
                    EstablishmentState::SessionPending
                });
            }
        }

        let now_ms = Utc::now().timestamp_millis() as u64;
        if let Some(pkg) = self.pending_key_packages.get(peer_id) {
            if now_ms < pkg.local_expires_at_ms {
                return Ok(EstablishmentState::HaveKeyPackage);
            }
        }
        if self.load_peer_key_package_from_storage(peer_id).is_some() {
            return Ok(EstablishmentState::HaveKeyPackage);
        }
        Ok(EstablishmentState::NoKeyPackage)
    }

    /// Monotonic state transition helper: Pending -> Confirmed only.
    pub(super) fn confirm_session_state(
        &mut self,
        peer_id: &str,
        source_event: &str,
    ) -> Result<bool> {
        if !self.can_confirm_from_source(peer_id, source_event) {
            warn!(
                event = "session_confirmation_blocked",
                session_or_group_id = %peer_id,
                source_event = %source_event,
                "session_confirmation_blocked"
            );
            return Ok(false);
        }

        let previous = self.ensure_session_state_entry(peer_id, source_event)?;

        if matches!(previous, SessionState::Confirmed) {
            self.confirmed_sessions.insert(peer_id.to_string());
            self.clear_confirmation_recovery_tracking(peer_id);
            info!(
                event = "session_state_transition",
                session_or_group_id = %peer_id,
                previous_state = "Confirmed",
                new_state = "Confirmed",
                source_event = %source_event,
                "session_state_transition"
            );
            return Ok(false);
        }

        // Persist first, then publish in-memory view.
        if let Err(err) = self.persist_session_state(peer_id, SessionState::Confirmed, source_event)
        {
            self.schedule_confirmation_retry(peer_id, source_event);
            return Err(err);
        }

        self.confirmed_sessions.insert(peer_id.to_string());
        self.clear_confirmation_recovery_tracking(peer_id);
        if source_event != "welcome_received" && source_event != "decrypt_success" {
            self.maybe_emit_local_session_established(
                peer_id,
                Self::session_ready_context_for_source(source_event),
            );
        }
        info!(
            event = "session_state_transition",
            session_or_group_id = %peer_id,
            previous_state = previous.as_str(),
            new_state = "Confirmed",
            source_event = %source_event,
            "session_state_transition"
        );
        Ok(true)
    }

    /// Ensures a session has an explicit persisted state entry.
    pub(super) fn ensure_session_state_entry(
        &self,
        peer_id: &str,
        source_event: &str,
    ) -> Result<SessionState> {
        let existing = self.load_session_state_entry(peer_id)?;
        if let Some(state) = existing {
            return Ok(state);
        }

        self.persist_session_state(peer_id, SessionState::Pending, source_event)?;
        info!(
            event = "session_state_transition",
            session_or_group_id = %peer_id,
            previous_state = "Absent",
            new_state = "Pending",
            source_event = %source_event,
            "session_state_transition"
        );
        Ok(SessionState::Pending)
    }

    /// Reconstructs runtime confirmation cache from persisted session states.
    pub(super) fn restore_session_states_from_manager(
        &mut self,
        mls: Arc<RwLock<MlsManager>>,
    ) -> Result<()> {
        self.confirmed_sessions.clear();

        let sessions = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.list_sessions()?
        };

        for peer_id in sessions {
            let state = match self.load_session_state_entry(&peer_id)? {
                Some(state) => state,
                None => self.bootstrap_missing_session_state(&peer_id)?,
            };
            if matches!(state, SessionState::Confirmed) {
                self.confirmed_sessions.insert(peer_id.clone());
            }
            info!(
                event = "session_state_restored",
                session_or_group_id = %peer_id,
                previous_state = "Pending",
                new_state = %state.as_str(),
                source_event = "initialize_mls",
                "session_state_restored"
            );
        }

        Ok(())
    }

    pub(super) fn bootstrap_missing_session_state(&self, peer_id: &str) -> Result<SessionState> {
        // Legacy session records without explicit state are treated as Pending.
        // Recovery is driven by probe/ack reconciliation, never by implicit inference.
        let restored_state = SessionState::Pending;
        self.persist_session_state(
            peer_id,
            restored_state,
            "initialize_mls_missing_state_migration",
        )?;
        info!(
            event = "session_state_transition",
            session_or_group_id = %peer_id,
            previous_state = "Absent",
            new_state = %restored_state.as_str(),
            source_event = "initialize_mls_missing_state_migration",
            "session_state_transition"
        );

        Ok(restored_state)
    }

    // ========================================================================
    // CONFIRMATION RECOVERY
    // ========================================================================

    pub(super) fn schedule_confirmation_retry(&mut self, peer_id: &str, source_event: &str) {
        self.confirmation_retry_due_at
            .insert(peer_id.to_string(), Utc::now());
        warn!(
            event = "session_confirmation_retry_scheduled",
            session_or_group_id = %peer_id,
            source_event = %source_event,
            "session_confirmation_retry_scheduled"
        );
    }

    pub(super) fn clear_confirmation_recovery_tracking(&mut self, peer_id: &str) {
        self.confirmation_retry_due_at.remove(peer_id);
        self.confirmation_probe_due_at.remove(peer_id);
    }

    pub(super) fn collect_pending_session_peers(&mut self) -> Result<Vec<String>> {
        let Some(mls) = self.mls_manager.clone() else {
            return Ok(Vec::new());
        };

        let sessions = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.list_sessions()?
        };

        let mut pending = Vec::new();
        for peer_id in sessions {
            if !self.is_session_confirmed(&peer_id)? {
                pending.push(peer_id);
            }
        }

        Ok(pending)
    }

    pub(super) fn send_session_confirmation_probe(&mut self, peer_id: &str, source_event: &str) {
        match self.send_internal_message(
            peer_id,
            internal_prefixes::SESSION_CONFIRM_PROBE.to_string(),
            MessagePriority::High,
        ) {
            Ok(_) => {
                info!(
                    event = "session_confirmation_probe_sent",
                    session_or_group_id = %peer_id,
                    source_event = %source_event,
                    "session_confirmation_probe_sent"
                );
            }
            Err(err) => {
                warn!(
                    event = "session_confirmation_probe_failed",
                    session_or_group_id = %peer_id,
                    source_event = %source_event,
                    error = %err,
                    "session_confirmation_probe_failed"
                );
            }
        }
    }

    /// Throttled wrapper around session reconciliation. Skips the expensive
    /// `list_sessions()` storage I/O when there is no pending work or when the
    /// last scan ran recently (within `RECONCILIATION_THROTTLE_MS`).
    ///
    /// This is the entry-point called from `process()` and `receive_message()`
    /// on every tick/poll. Without throttling, the storage call holds the
    /// protocol Mutex and blocks `sendMessage()` on mobile platforms.
    pub(super) fn run_throttled_reconciliation(&mut self, source_event: &str) {
        // Fast path: nothing pending → skip entirely (no storage I/O)
        let has_pending_work = !self.pending_encrypted_messages.is_empty()
            || !self.confirmation_probe_due_at.is_empty()
            || !self.confirmation_retry_due_at.is_empty();

        if !has_pending_work {
            return;
        }

        // Throttle: only run the full scan every RECONCILIATION_THROTTLE_MS
        let now = Instant::now();
        let throttle = StdDuration::from_millis(RECONCILIATION_THROTTLE_MS);
        if let Some(last) = self.last_reconciliation_at {
            if now.duration_since(last) < throttle {
                return;
            }
        }

        self.last_reconciliation_at = Some(now);
        self.retry_pending_session_confirmations();
        self.kick_pending_session_reconciliation(source_event);
    }

    pub(super) fn kick_pending_session_reconciliation(&mut self, source_event: &str) {
        let now = Utc::now();
        let pending_peers = match self.collect_pending_session_peers() {
            Ok(peers) => peers,
            Err(err) => {
                warn!(
                    event = "session_confirmation_probe_scan_failed",
                    source_event = %source_event,
                    error = %err,
                    "session_confirmation_probe_scan_failed"
                );
                return;
            }
        };

        let pending_set: HashSet<String> = pending_peers.iter().cloned().collect();
        self.confirmation_probe_due_at
            .retain(|peer, _| pending_set.contains(peer));

        for peer_id in pending_peers {
            let due_at = self
                .confirmation_probe_due_at
                .get(&peer_id)
                .copied()
                .unwrap_or(now);
            if due_at > now {
                continue;
            }

            self.send_session_confirmation_probe(&peer_id, source_event);
            self.confirmation_probe_due_at.insert(
                peer_id,
                now + ChronoDuration::seconds(CONFIRMATION_PROBE_INTERVAL_SECS),
            );
        }
    }

    pub(super) fn retry_pending_session_confirmations(&mut self) {
        let now = Utc::now();
        let due_peers: Vec<String> = self
            .confirmation_retry_due_at
            .iter()
            .filter_map(|(peer_id, due_at)| {
                if *due_at <= now {
                    Some(peer_id.clone())
                } else {
                    None
                }
            })
            .collect();

        for peer_id in due_peers {
            match self.has_mls_session(&peer_id) {
                Ok(true) => {}
                Ok(false) => {
                    self.confirmation_retry_due_at.remove(&peer_id);
                    continue;
                }
                Err(err) => {
                    warn!(
                        event = "session_confirmation_retry_scan_failed",
                        session_or_group_id = %peer_id,
                        error = %err,
                        "session_confirmation_retry_scan_failed"
                    );
                    self.confirmation_retry_due_at.insert(
                        peer_id,
                        now + ChronoDuration::seconds(CONFIRMATION_RETRY_INTERVAL_SECS),
                    );
                    continue;
                }
            }

            if !self.can_confirm_from_source(&peer_id, "confirmation_retry") {
                debug!(
                    peer_id = %peer_id,
                    "Skipping confirmation retry until welcome delivery is sent"
                );
                continue;
            }

            match self.confirm_session_state(&peer_id, "confirmation_retry") {
                Ok(_) => {
                    let _ = self.flush_pending_messages(&peer_id);
                    self.process_pending_decryption(&peer_id);
                }
                Err(err) => {
                    warn!(
                        event = "session_confirmation_retry_failed",
                        session_or_group_id = %peer_id,
                        error = %err,
                        "session_confirmation_retry_failed"
                    );
                    self.confirmation_retry_due_at.insert(
                        peer_id,
                        now + ChronoDuration::seconds(CONFIRMATION_RETRY_INTERVAL_SECS),
                    );
                }
            }
        }
    }

    pub(super) fn can_confirm_from_source(&self, peer_id: &str, source_event: &str) -> bool {
        if !matches!(
            source_event,
            "decrypt_success"
                | "confirmation_ack_received"
                | "confirmation_probe_received"
                | "confirmation_retry"
        ) {
            return true;
        }

        match self.welcome_lifecycles.get(peer_id) {
            Some(record) => matches!(record.state, WelcomeDeliveryState::Sent | WelcomeDeliveryState::SendAttempted),
            None => matches!(
                source_event,
                // Compatibility path for sessions created before welcome lifecycle
                // persistence existed. Decrypt-based confirmation stays blocked
                // until we have explicit local welcome delivery evidence.
                "confirmation_ack_received" | "confirmation_probe_received" | "confirmation_retry"
            ),
        }
    }

    pub(super) fn abort_pending_session_for_peer(
        &mut self,
        peer_id: &str,
        reason: crate::events::WelcomeReasonCode,
    ) {
        self.pending_encrypted_messages.remove(peer_id);
        self.clear_pending_messages_from_storage(peer_id);
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::secure_session_failed(
                peer_id.to_string(),
                format!("Welcome delivery failed: {}", reason.as_str()),
            ));
        }
    }

    pub(super) fn maybe_emit_local_session_established(
        &self,
        peer_id: &str,
        context: MlsOperationContext,
    ) {
        let Some(record) = self.welcome_lifecycles.get(peer_id) else {
            return;
        };
        if !matches!(record.state, WelcomeDeliveryState::Sent) {
            return;
        }
        self.emit_mls_session_ready(peer_id, &record.group_id, context);
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::secure_session_established(
                peer_id.to_string(),
                record.group_id.clone(),
                record.group_id.starts_with("session:"),
                true,
            ));
        }
    }

    // ========================================================================
    // WELCOME LIFECYCLE
    // ========================================================================

    pub(super) fn send_welcome_message(
        &mut self,
        recipient: &str,
        welcome: &WelcomeMessage,
    ) -> Result<bool> {
        let serialized =
            serde_json::to_string(welcome).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::WELCOME, serialized);
        let mut message =
            self.create_message(recipient, content, Some(MessagePriority::High), None)?;
        self.sign_control_message(&mut message)?;
        let group_id = welcome.group_id.as_str().to_string();

        self.upsert_welcome_lifecycle(recipient, &group_id, message, "welcome_created")?;
        self.try_send_welcome(recipient, "welcome_initial_send")
    }

    pub(super) fn try_send_welcome(&mut self, peer_id: &str, source_event: &str) -> Result<bool> {
        let now = Utc::now();
        let mut record = self
            .welcome_lifecycles
            .get(peer_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("Missing welcome lifecycle for {}", peer_id)))?;

        if matches!(record.state, WelcomeDeliveryState::Sent) {
            return Ok(true);
        }
        if matches!(record.state, WelcomeDeliveryState::Expired) {
            return Ok(false);
        }

        if record.expires_at <= now {
            self.transition_welcome_state(peer_id, WelcomeDeliveryState::Expired, source_event)?;
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::welcome_send_expired(
                    peer_id.to_string(),
                    record.welcome_message.id.as_str().to_string(),
                    record.attempt,
                    crate::events::WelcomeReasonCode::RetryExhausted,
                ));
            }
            self.abort_pending_session_for_peer(
                peer_id,
                crate::events::WelcomeReasonCode::RetryExhausted,
            );
            return Ok(false);
        }

        record.attempt = record.attempt.saturating_add(1);
        self.welcome_lifecycles
            .insert(peer_id.to_string(), record.clone());
        self.persist_welcome_lifecycle_entry(&record)?;
        self.transition_welcome_state(peer_id, WelcomeDeliveryState::SendAttempted, source_event)?;

        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::welcome_send_attempted(
                peer_id.to_string(),
                record.welcome_message.id.as_str().to_string(),
                record.group_id.clone(),
                record.attempt,
            ));
        }

        match self.transport_manager.send(&record.welcome_message) {
            Ok(()) => {
                let transport_used = self.transport_manager.current_transport();
                let mut updated =
                    self.welcome_lifecycles
                        .get(peer_id)
                        .cloned()
                        .ok_or_else(|| {
                            Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
                        })?;

                if matches!(transport_used, Some(TransportType::Internet)) {
                    // Internet send() only enqueues for platform polling. Keep lifecycle
                    // non-terminal until explicit platform confirmation arrives.
                    updated.next_retry_at = Some(
                        Utc::now() + ChronoDuration::seconds(WELCOME_INTERNET_CONFIRM_TIMEOUT_SECS),
                    );
                    updated.last_reason_code = None;
                    updated.last_transport_error = None;
                    self.welcome_lifecycles
                        .insert(peer_id.to_string(), updated.clone());
                    self.persist_welcome_lifecycle_entry(&updated)?;
                    return Ok(false);
                }

                updated.next_retry_at = None;
                updated.last_reason_code = None;
                updated.last_transport_error = None;
                self.welcome_lifecycles
                    .insert(peer_id.to_string(), updated.clone());
                self.persist_welcome_lifecycle_entry(&updated)?;
                self.transition_welcome_state(peer_id, WelcomeDeliveryState::Sent, source_event)?;
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::welcome_send_succeeded(
                        peer_id.to_string(),
                        updated.welcome_message.id.as_str().to_string(),
                        updated.group_id,
                        updated.attempt,
                    ));
                }
                Ok(true)
            }
            Err(err) => {
                let reason = Self::map_welcome_reason_code(&err);
                self.apply_welcome_send_failure(
                    peer_id,
                    reason,
                    Some(err.to_string()),
                    source_event,
                )
            }
        }
    }

    pub(super) fn apply_welcome_send_failure(
        &mut self,
        peer_id: &str,
        reason: crate::events::WelcomeReasonCode,
        transport_error: Option<String>,
        source_event: &str,
    ) -> Result<bool> {
        let mut updated = self
            .welcome_lifecycles
            .get(peer_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("Missing welcome lifecycle for {}", peer_id)))?;

        if matches!(
            updated.state,
            WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
        ) {
            return Ok(matches!(updated.state, WelcomeDeliveryState::Sent));
        }

        let max_attempts = self.config.reliability.retry.max_retries.max(1);
        let should_expire = updated.attempt >= max_attempts || updated.expires_at <= Utc::now();
        if should_expire {
            let terminal_reason = crate::events::WelcomeReasonCode::RetryExhausted;
            {
                let record = self.welcome_lifecycles.get_mut(peer_id).ok_or_else(|| {
                    Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
                })?;
                record.last_reason_code = Some(terminal_reason);
                record.last_transport_error = transport_error.clone();
                record.next_retry_at = None;
            }

            if !matches!(updated.state, WelcomeDeliveryState::Failed) {
                self.transition_welcome_state(peer_id, WelcomeDeliveryState::Failed, source_event)?;
            }
            self.transition_welcome_state(
                peer_id,
                WelcomeDeliveryState::Expired,
                "welcome_retry_exhausted",
            )?;

            let expired_snapshot =
                self.welcome_lifecycles
                    .get(peer_id)
                    .cloned()
                    .ok_or_else(|| {
                        Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
                    })?;
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::welcome_send_failed(
                    peer_id.to_string(),
                    expired_snapshot.welcome_message.id.as_str().to_string(),
                    expired_snapshot.group_id.clone(),
                    expired_snapshot.attempt,
                    terminal_reason,
                    expired_snapshot.last_transport_error.clone(),
                    false,
                    None,
                ));
                state.emit_event(Event::welcome_send_expired(
                    peer_id.to_string(),
                    expired_snapshot.welcome_message.id.as_str().to_string(),
                    expired_snapshot.attempt,
                    terminal_reason,
                ));
            }
            self.abort_pending_session_for_peer(peer_id, terminal_reason);
            return Ok(false);
        }

        let delay_ms = self.compute_welcome_retry_delay_ms(peer_id, updated.attempt);
        let retry_at = Utc::now() + ChronoDuration::milliseconds(delay_ms as i64);

        {
            let record = self.welcome_lifecycles.get_mut(peer_id).ok_or_else(|| {
                Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
            })?;
            record.last_reason_code = Some(reason);
            record.last_transport_error = transport_error;
            record.next_retry_at = Some(retry_at);
        }

        if !matches!(updated.state, WelcomeDeliveryState::Failed) {
            self.transition_welcome_state(peer_id, WelcomeDeliveryState::Failed, source_event)?;
        } else if let Some(record) = self.welcome_lifecycles.get(peer_id).cloned() {
            self.persist_welcome_lifecycle_entry(&record)?;
        }

        updated = self
            .welcome_lifecycles
            .get(peer_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("Missing welcome lifecycle for {}", peer_id)))?;
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::welcome_send_failed(
                peer_id.to_string(),
                updated.welcome_message.id.as_str().to_string(),
                updated.group_id,
                updated.attempt,
                reason,
                updated.last_transport_error.clone(),
                true,
                Some(retry_at.timestamp_millis()),
            ));
        }
        Ok(false)
    }

    pub(super) fn compute_welcome_retry_delay_ms(&self, peer_id: &str, attempt: u32) -> u64 {
        let config = &self.config.reliability.retry;
        let capped_attempt = attempt.saturating_sub(1);
        let base_ms = if capped_attempt == 0 {
            config.initial_delay_ms
        } else {
            let multiplier = config.backoff_multiplier.powi(capped_attempt as i32);
            (config.initial_delay_ms as f64 * multiplier as f64) as u64
        }
        .min(config.max_delay_ms);

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        peer_id.hash(&mut hasher);
        attempt.hash(&mut hasher);
        Utc::now().timestamp_millis().hash(&mut hasher);
        let bucket = (hasher.finish() % 10_000) as f64 / 10_000.0;
        let jitter_factor = 1.0 + ((bucket * 2.0 - 1.0) * WELCOME_RETRY_JITTER_RATIO);
        let jittered = (base_ms as f64 * jitter_factor).round() as i64;
        jittered.max(1) as u64
    }

    pub(super) fn can_transition_welcome_state(
        current: WelcomeDeliveryState,
        next: WelcomeDeliveryState,
    ) -> bool {
        matches!(
            (current, next),
            (
                WelcomeDeliveryState::Created,
                WelcomeDeliveryState::SendAttempted
            ) | (WelcomeDeliveryState::Created, WelcomeDeliveryState::Expired)
                | (
                    WelcomeDeliveryState::SendAttempted,
                    WelcomeDeliveryState::Sent
                )
                | (
                    WelcomeDeliveryState::SendAttempted,
                    WelcomeDeliveryState::Failed
                )
                | (
                    WelcomeDeliveryState::Failed,
                    WelcomeDeliveryState::SendAttempted
                )
                | (WelcomeDeliveryState::Failed, WelcomeDeliveryState::Sent)
                | (WelcomeDeliveryState::Failed, WelcomeDeliveryState::Expired)
        )
    }

    pub(super) fn transition_welcome_state(
        &mut self,
        peer_id: &str,
        next_state: WelcomeDeliveryState,
        source_event: &str,
    ) -> Result<()> {
        let (previous_state, record_snapshot) = {
            let record = self.welcome_lifecycles.get_mut(peer_id).ok_or_else(|| {
                Error::Other(format!(
                    "Missing welcome lifecycle for transition: {}",
                    peer_id
                ))
            })?;

            if record.state == next_state {
                return Ok(());
            }

            if !Self::can_transition_welcome_state(record.state, next_state) {
                return Err(Error::Other(format!(
                    "Illegal welcome lifecycle transition for {}: {} -> {}",
                    peer_id,
                    record.state.as_str(),
                    next_state.as_str()
                )));
            }

            let previous = record.state;
            record.state = next_state;
            if matches!(
                next_state,
                WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
            ) {
                record.next_retry_at = None;
            }
            (previous, record.clone())
        };

        self.persist_welcome_lifecycle_entry(&record_snapshot)?;
        info!(
            event = "welcome_lifecycle_transition",
            session_or_group_id = %peer_id,
            previous_state = previous_state.as_str(),
            new_state = next_state.as_str(),
            source_event = %source_event,
            attempt = record_snapshot.attempt,
            "welcome_lifecycle_transition"
        );
        Ok(())
    }

    pub(super) fn upsert_welcome_lifecycle(
        &mut self,
        peer_id: &str,
        group_id: &str,
        welcome_message: Message,
        source_event: &str,
    ) -> Result<()> {
        if let Some(existing) = self.welcome_lifecycles.get(peer_id) {
            if !matches!(
                existing.state,
                WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
            ) {
                return Err(Error::Other(format!(
                    "Refusing to overwrite active welcome lifecycle for {} in state {}",
                    peer_id,
                    existing.state.as_str()
                )));
            }
        }

        let now = Utc::now();
        let record = WelcomeLifecycleRecord {
            peer_id: peer_id.to_string(),
            group_id: group_id.to_string(),
            state: WelcomeDeliveryState::Created,
            attempt: 0,
            welcome_message,
            next_retry_at: None,
            last_reason_code: None,
            last_transport_error: None,
            created_at: now,
            expires_at: now + ChronoDuration::seconds(WELCOME_LIFECYCLE_TTL_SECS),
        };
        self.welcome_lifecycles
            .insert(peer_id.to_string(), record.clone());
        self.persist_welcome_lifecycle_entry(&record)?;
        info!(
            event = "welcome_lifecycle_transition",
            session_or_group_id = %peer_id,
            previous_state = "Absent",
            new_state = WelcomeDeliveryState::Created.as_str(),
            source_event = %source_event,
            attempt = 0,
            "welcome_lifecycle_transition"
        );
        Ok(())
    }

    pub(super) fn map_welcome_reason_code(error: &Error) -> crate::events::WelcomeReasonCode {
        SessionStateError::classify(error).to_welcome_reason_code()
    }

    pub(super) fn session_ready_context_for_source(source_event: &str) -> MlsOperationContext {
        match source_event {
            "confirmation_ack_received" | "confirmation_probe_received" | "decrypt_success" => {
                MlsOperationContext::Receive
            }
            "welcome_received" => MlsOperationContext::Welcome,
            _ => MlsOperationContext::Send,
        }
    }

    pub(super) fn find_welcome_peer_by_message_id(&self, message_id: &str) -> Option<String> {
        self.welcome_lifecycles
            .iter()
            .find_map(|(peer_id, record)| {
                if record.welcome_message.id.as_str() == message_id {
                    return Some(peer_id.clone());
                }
                None
            })
    }

    pub(super) fn process_welcome_retry_queue(&mut self) -> Result<()> {
        let now = Utc::now();
        let timed_out_attempts: Vec<String> = self
            .welcome_lifecycles
            .iter()
            .filter_map(|(peer_id, record)| {
                if matches!(record.state, WelcomeDeliveryState::SendAttempted)
                    && record.next_retry_at.is_some_and(|retry_at| retry_at <= now)
                {
                    return Some(peer_id.clone());
                }
                None
            })
            .take(WELCOME_RETRY_BATCH_SIZE)
            .collect();

        for peer_id in timed_out_attempts {
            let _ = self.apply_welcome_send_failure(
                &peer_id,
                crate::events::WelcomeReasonCode::Timeout,
                Some("Welcome send confirmation timed out".to_string()),
                "welcome_confirm_timeout",
            )?;
        }

        let due_peers: Vec<String> = self
            .welcome_lifecycles
            .iter()
            .filter_map(|(peer_id, record)| {
                if matches!(record.state, WelcomeDeliveryState::Failed)
                    && record.next_retry_at.is_some_and(|retry_at| retry_at <= now)
                {
                    return Some(peer_id.clone());
                }
                None
            })
            .take(WELCOME_RETRY_BATCH_SIZE)
            .collect();

        for peer_id in due_peers {
            if let Err(err) = self.try_send_welcome(&peer_id, "welcome_retry") {
                warn!(
                    peer_id = %peer_id,
                    error = %err,
                    "Failed to process welcome retry"
                );
            }
        }

        Ok(())
    }

    pub(super) fn has_terminal_welcome_failure(&self, peer_id: &str) -> bool {
        self.welcome_lifecycles
            .get(peer_id)
            .is_some_and(|record| matches!(record.state, WelcomeDeliveryState::Expired))
    }
}
