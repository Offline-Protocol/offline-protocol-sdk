//! Session confirmation, welcome lifecycle, and pending session reconciliation.

use super::{
    classify_transport_send_error, internal_prefixes, lock_shared_state, send_failure_token,
    OfflineProtocol, PresenceRescueThrottle, PruneAllowance, RestorableRecord, SessionState,
    WelcomeDeliveryState, WelcomeLifecycleRecord, CONFIRMATION_PROBE_INTERVAL_SECS,
    CONFIRMATION_RETRY_INTERVAL_SECS, MAX_REKEY_TRACKED_PEERS, RECONCILIATION_THROTTLE_MS,
    REKEY_INTERVAL_SECS, SEND_FAIL_REASON_CONFIRM_TIMEOUT, WELCOME_INTERNET_CONFIRM_TIMEOUT_SECS,
    WELCOME_LIFECYCLE_TTL_SECS, WELCOME_MESH_CONFIRM_TIMEOUT_SECS, WELCOME_NO_CARRIER_RETRY_SECS,
    WELCOME_PRESENCE_RESCUE_BASE_SECS, WELCOME_PRESENCE_RESCUE_MAX_SECS, WELCOME_RETRY_BATCH_SIZE,
    WELCOME_RETRY_JITTER_RATIO, WELCOME_UNREACHABLE_RETRY_CAP_SECS, WELCOME_WATCHLIST_MAX_AGE_SECS,
};
use crate::events::SecurityWarningCode;
use crate::mls_observability::MlsOperationContext;
use crate::protocol::reachability::{Claim, FactSource};
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
                // The MLS session is gone, so any both-create owner gate for this
                // peer is stale; clear it (memory + storage) or it would block
                // `welcome_received` confirmation on the next re-pairing.
                self.clear_both_create_awaiting_decrypt(peer_id);
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
            self.clear_both_create_awaiting_decrypt(peer_id);
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
        // Group-aware proof has arrived (decrypt), so we no longer need to gate
        // confirmation on decrypt for this both-create peer.
        self.clear_both_create_awaiting_decrypt(peer_id);
        // The peer has proved the session, so the outbound Welcome — kept
        // non-terminal so a lost fragment keeps being retried — is delivered.
        // Mark it Sent so process_welcome_retry_queue stops re-sending it. Mesh
        // has no transport-level delivery ack, so this session proof is the mesh
        // equivalent of on_transport_send_confirmed.
        self.mark_welcome_confirmed(peer_id, source_event);
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

    /// Marks a non-terminal outbound welcome lifecycle as `Sent` once the peer
    /// has proved the session (probe / ack / welcome / decrypt). The mesh
    /// equivalent of [`Self::on_transport_send_confirmed`], which only fires for
    /// Internet. No-op when there is no welcome lifecycle for `peer_id` (e.g. a
    /// confirmation keyed by a group id rather than a 1:1 peer) or it is already
    /// terminal, so it is safe to call from every confirmation path.
    fn mark_welcome_confirmed(&mut self, peer_id: &str, source_event: &str) {
        match self.welcome_lifecycles.get(peer_id).map(|r| r.state) {
            // In flight / retried — fall through and mark Sent below.
            Some(WelcomeDeliveryState::SendAttempted) | Some(WelcomeDeliveryState::Failed) => {}
            // Parked, never actually sent (no carrier at send time) for a peer
            // that converged over some other path. `Created -> Sent` is not a
            // legal transition and there is nothing in flight to confirm, so drop
            // the now-moot lifecycle (memory + storage) rather than leaving it to
            // linger until the session is deleted.
            Some(WelcomeDeliveryState::Created) => {
                self.welcome_lifecycles.remove(peer_id);
                if let Err(err) = self.clear_welcome_lifecycle_entry(peer_id) {
                    warn!(
                        peer_id = %peer_id,
                        error = %err,
                        "Failed to clear parked welcome lifecycle on confirmation"
                    );
                }
                return;
            }
            // Already terminal (Sent / Expired) or no lifecycle — nothing to do.
            _ => return,
        }
        if let Some(record) = self.welcome_lifecycles.get_mut(peer_id) {
            record.next_retry_at = None;
            record.last_reason_code = None;
            record.last_transport_error = None;
        }
        // transition_welcome_state persists the record and logs the transition.
        if self
            .transition_welcome_state(peer_id, WelcomeDeliveryState::Sent, source_event)
            .is_err()
        {
            return;
        }
        if let Some(snapshot) = self.welcome_lifecycles.get(peer_id).cloned() {
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::welcome_send_succeeded(
                    peer_id.to_string(),
                    snapshot.welcome_message.id.as_str().to_string(),
                    snapshot.group_id,
                    snapshot.attempt,
                ));
            }
        }
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
    /// `allowance` is the launch's shared advisory pool — this walk's deletes
    /// are advisory (a dropped record re-bootstraps to `Pending`, which is the
    /// safe answer), and they add up against the same allowance as the four
    /// category walks. See `storage::MAX_RESTORE_PRUNE_DELETES`.
    pub(super) fn restore_session_states_from_manager(
        &mut self,
        mls: Arc<RwLock<MlsManager>>,
        allowance: &mut PruneAllowance,
    ) -> Result<()> {
        self.confirmed_sessions.clear();

        let sessions = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.list_sessions()?
        };

        // The session list itself is deliberately not truncated the way a
        // container-listed walk is: dropping its tail would silently leave
        // confirmed peers out of `confirmed_sessions`. What needed bounding was
        // the *deletes* a bad store makes this walk issue — one device barrier
        // per peer, with nothing counting them until now.
        let mut budget = allowance.refusing();
        for peer_id in sessions {
            // Before anything that can `continue`: an MLS session with this
            // peer exists, which is the capability fact, and it holds whether
            // or not the peer's `session_states` record turns out to be
            // readable. Deriving it from the record instead would reintroduce
            // exactly the deletable dependency `encryption_capable_peers`
            // exists to escape.
            self.mark_encryption_capable(&peer_id);

            // Restore-path read: a record whose bytes will not decode is
            // dropped and re-bootstrapped as `Pending` rather than failing
            // initialization forever. The send path deliberately keeps the
            // strict loader — see `load_restorable_state_record`.
            let state = match self.load_session_state_for_restore(&peer_id, Some(&mut budget)) {
                RestorableRecord::Present(state) => state,
                RestorableRecord::Absent => self.bootstrap_missing_session_state(&peer_id),
                // A record is still on disk and could not be read *this*
                // session. Re-bootstrapping would persist `Pending` straight
                // over one that may say `Confirmed`, and propagating would fail
                // `initialize_mls` for as long as that single record stays
                // unreadable — on an install that then cannot send at all.
                // Skipping is what leaves the record recoverable: the peer is
                // simply absent from `confirmed_sessions`, and the send path's
                // own strict loader (`is_session_confirmed`) still fails closed
                // for it — and re-reads the record, so a transient failure
                // heals the moment the store answers again.
                //
                // Three consequences worth naming rather than rediscovering,
                // all of them following from the peer being absent from
                // `confirmed_sessions` for the run:
                //
                // 1. The Welcome machinery gates on
                //    `!confirmed_sessions.contains(peer)` *plus* a live
                //    `welcome_lifecycles` entry (`welcome_pending_peers`,
                //    `resend_unconfirmed_sent_welcome`, the retry ladder). So a
                //    peer whose lifecycle restored as `Sent` while its session
                //    state did not can draw a redundant Welcome re-send.
                // 2. `flush_restored_confirmed_pending_messages` will not flush
                //    this peer's queued pre-session messages this session. They
                //    stay queued — in memory and on disk — and flush once a
                //    launch can read the record, or once a live confirmation
                //    re-adds the peer.
                // 3. Tier-2 crypto recovery re-seals a resend only for a peer
                //    in `confirmed_sessions` (`reseal_resend_content`), so a
                //    resend to this peer replays its original ciphertext for
                //    the run rather than re-sealing to the current epoch.
                //
                // All three are deferrals on an already-degraded store, and all
                // three are strictly better than the alternative this replaced
                // — propagating, which failed `initialize_mls` on every launch
                // for as long as the one record stayed unreadable, on an
                // install that could then send nothing at all.
                RestorableRecord::Unavailable => {
                    warn!(
                        session_or_group_id = %peer_id,
                        "Session state could not be read this session; leaving the record in \
                         place and treating the session as unconfirmed"
                    );
                    continue;
                }
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

        if budget.exhausted {
            warn!(
                deleted = budget.spent,
                budget = super::storage::MAX_RESTORE_PRUNE_DELETES,
                "Session state restore hit its share of the launch delete budget; the \
                 unreadable records left behind are re-bootstrapped as Pending anyway and \
                 dropped on a later launch"
            );
        }

        Ok(())
    }

    /// The state a session with no readable record starts from.
    ///
    /// Infallible: the migration write is best effort, like every other
    /// persistence call on the restore path. It used to propagate, so one
    /// `StoreFailed` — a full disk, a container the OS has locked — failed
    /// `initialize_mls` outright and, with `require_encryption` on by default,
    /// left the install unable to send. `Pending` is the safe answer in memory
    /// either way, and a launch that can write re-derives the record.
    pub(super) fn bootstrap_missing_session_state(&self, peer_id: &str) -> SessionState {
        // Legacy session records without explicit state are treated as Pending.
        // Recovery is driven by probe/ack reconciliation, never by implicit inference.
        let restored_state = SessionState::Pending;
        if let Err(e) = self.persist_session_state(
            peer_id,
            restored_state,
            "initialize_mls_missing_state_migration",
        ) {
            warn!(
                session_or_group_id = %peer_id,
                error = %e,
                "Failed to persist the bootstrapped session state; the session is Pending for \
                 this run and the record is re-derived on the next launch"
            );
        }
        info!(
            event = "session_state_transition",
            session_or_group_id = %peer_id,
            previous_state = "Absent",
            new_state = %restored_state.as_str(),
            source_event = "initialize_mls_missing_state_migration",
            "session_state_transition"
        );

        restored_state
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

    /// Triggers a rate-limited re-key of the 1:1 session with `peer_id` after an
    /// epoch-desync decrypt failure.
    ///
    /// The self-healing primitive is a two-part move, matching the unblock
    /// `session_reset` flow: tear down our *own* stale session **and** advertise
    /// a `session_reset` key package. The peer, on receiving it, drops its stale
    /// session, auto-establishes a fresh one from our enclosed key package, and
    /// Welcomes us back — which we, now session-less, simply *join*.
    ///
    /// Tearing down our session alone (without the reset key package) would
    /// deadlock: the peer's original key package was consumed at first
    /// establishment, so nothing could rebuild the channel, and the peer (still
    /// believing its session is healthy) would never re-advertise one. Sending
    /// the reset key package is what breaks that deadlock.
    ///
    /// Deleting the local session is also what makes convergence **symmetric and
    /// single-round regardless of user-id ordering**. Because we keep no local
    /// session, the peer's returning Welcome is joined via the fresh-session
    /// path rather than the greater-id-adopts tiebreaker in
    /// `handle_welcome_message` — so the lexicographically-smaller detector is no
    /// longer stranded (the failure mode when the stale session was kept). Both
    /// orderings are covered end-to-end by
    /// `test_desync_dm_heals_end_to_end_when_detector_id_is_greater` and
    /// `test_desync_dm_heals_end_to_end_when_detector_id_is_smaller`.
    ///
    /// Rate-limited via `rekey_due_at` to at most one re-key per
    /// [`REKEY_INTERVAL_SECS`] per peer, so a peer replaying stale-epoch
    /// ciphertext (or an injected wrong-epoch frame) cannot drive a re-key storm.
    /// The interval is stamped before the send so a send error still counts
    /// against the limit, and it is **never reset early** — not even by a
    /// successful decrypt on the healed session. That is deliberate: a genuine
    /// re-fork and a replayed old-epoch frame are indistinguishable at this layer
    /// (both surface as `WrongEpoch`), so clearing the floor on heal would let an
    /// attacker who lands one legit decrypt between replays defeat the limit and
    /// force ~one teardown per inbound message. The floor lapses naturally after
    /// [`REKEY_INTERVAL_SECS`]; during that window Tier 1 (un-ACK + sender
    /// retries) keeps delivery honest, so the only cost of not resetting is that a
    /// genuine second desync within the window heals up to one interval later —
    /// an acceptable trade for a rare event against a bounded-churn guarantee.
    ///
    /// **SECURITY — this trigger is unauthenticated, and cannot be made
    /// otherwise.** `peer_id` is the *wire-claimed* sender. Three facts compose:
    /// `__MLS_ENC__` is a data-plane prefix, deliberately exempt from the
    /// Ed25519 + derivation control-message gate (see `DATA_PLANE_PREFIXES`);
    /// OpenMLS
    /// validates the framing header — group id, then epoch — *before* any AEAD,
    /// sender-data decryption or signature check, so `WrongEpoch` /
    /// `NoPastEpochData` is produced with the sender still entirely unverified;
    /// and a 1:1 slot id is `session:<a>:<b>` over two public user ids. So the
    /// classification that lands here is reachable by **anyone who can inject a
    /// frame** — no key material, no captured ciphertext, no session, no replay
    /// needed. `test_forged_frame_reaches_session_desync_without_any_key_material`
    /// builds such a frame from scratch to keep this statement honest. It is
    /// inherent to MLS framing, not an OpenMLS defect, so no upgrade changes it.
    ///
    /// The `SenderIdentityMismatch` gate cannot help: it compares the MLS
    /// credential to the claimed sender, and that credential only exists once
    /// `process_message` **succeeds**. Every pre-authentication verdict is
    /// structurally outside its reach.
    ///
    /// Because the trigger cannot be authenticated, the mitigation is that
    /// acting on it is **harmless**, not that it is trusted:
    /// - `SessionManager::decrypt_message` requires the envelope to name the
    ///   slot shared with the claimed sender, so one derivable session id
    ///   cannot be aimed at arbitrary peers, and `rekey_due_at` cannot be grown
    ///   with attacker-chosen keys.
    /// - [`REKEY_INTERVAL_SECS`] bounds this to one re-key per peer per window.
    /// - The heal destroys nothing: queued plaintext survives a reset (it seals
    ///   at flush time against the rebuilt session), and Tier 2 re-seals
    ///   in-flight resends.
    /// - Each re-key emits `SecurityWarningCode::SessionRekeyTriggered`, so a
    ///   sustained rate — the signature of injected frames rather than a real
    ///   fork — is visible to the app.
    ///
    /// **Residual:** an injector can still force bounded re-key churn on a pair
    /// (delayed delivery, never lost). Closing that needs a signed
    /// epoch-corroboration exchange before teardown; a liveness-only probe does
    /// not work, since a healthy peer answers and we would tear down anyway.
    /// Also see `docs/state-machines/session-lifecycle.md` ("Desync and heal")
    /// and `docs/security/threat-model.md` (residual risk R2).
    pub(super) fn schedule_session_rekey(&mut self, peer_id: &str) {
        let now = Utc::now();
        if let Some(due_at) = self.rekey_due_at.get(peer_id) {
            if *due_at > now {
                return;
            }
        }
        // Bounded like every other map keyed by a wire-claimed id: the desync
        // that gets us here is classified before MLS authenticates anything.
        if !self.rekey_due_at.contains_key(peer_id)
            && self.rekey_due_at.len() >= MAX_REKEY_TRACKED_PEERS
        {
            self.rekey_due_at.clear();
        }
        self.rekey_due_at.insert(
            peer_id.to_string(),
            now + ChronoDuration::seconds(REKEY_INTERVAL_SECS),
        );

        // A re-key is remotely triggerable and, until now, entirely silent to
        // the app. A genuine fork produces these occasionally; a sustained rate
        // for one peer is the signature of injected frames, which an operator
        // cannot otherwise distinguish.
        self.emit_security_warning(
            peer_id,
            SecurityWarningCode::SessionRekeyTriggered,
            "1:1 session torn down and re-advertised after an epoch desync",
        );
        // Tear down our own stale session before advertising the reset key
        // package, mirroring the unblock `session_reset` flow. This is what makes
        // convergence symmetric regardless of user-id ordering: with no local
        // session left, the peer's returning Welcome is *joined* (not gated by
        // the greater-id-adopts tiebreaker in `handle_welcome_message`), so both
        // sides rebuild from scratch and converge in a single round. Keeping the
        // stale session instead strands the smaller-id detector, which the
        // tiebreaker forbids from adopting. Best-effort: a missing session is a
        // no-op, and a failed delete still sends the reset (the peer rebuild plus
        // the auto key-package exchange re-arm establishment either way).
        if self.has_mls_session(peer_id).unwrap_or(false) {
            if let Err(err) = self.manual_mls_delete_session(peer_id) {
                debug!(
                    peer_id = %peer_id,
                    error = %err,
                    "rekey: failed to delete local stale session; sending reset anyway"
                );
            }
        }
        match self.send_key_package_to(peer_id, true) {
            Ok(()) => {
                info!(
                    event = "session_rekey_triggered",
                    peer_id = %peer_id,
                    "Triggered 1:1 session re-key after epoch desync"
                );
            }
            Err(err) => {
                warn!(
                    event = "session_rekey_send_failed",
                    peer_id = %peer_id,
                    error = %err,
                    "session_rekey_send_failed"
                );
            }
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
                    "Skipping confirmation retry until welcome send is at least attempted"
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

    /// Marks `peer_id` as a both-create owner gate — we kept our own group and
    /// must observe a group-aware decrypt before confirming — and persists it so
    /// an owner restart mid-convergence cannot let a stale plaintext probe/ack
    /// confirm prematurely. Only writes storage on a genuine insert.
    pub(super) fn mark_both_create_awaiting_decrypt(&mut self, peer_id: &str) {
        if self
            .both_create_awaiting_decrypt
            .insert(peer_id.to_string())
        {
            self.persist_both_create_awaiting_decrypt(peer_id);
        }
    }

    /// Clears the both-create owner gate for `peer_id` (it has converged) from
    /// both memory and storage. Only writes storage on a genuine removal.
    pub(super) fn clear_both_create_awaiting_decrypt(&mut self, peer_id: &str) {
        if self.both_create_awaiting_decrypt.remove(peer_id) {
            self.delete_both_create_awaiting_decrypt(peer_id);
        }
    }

    pub(super) fn can_confirm_from_source(&self, peer_id: &str, source_event: &str) -> bool {
        // Both-create owner: we kept our own group and are waiting for the peer to
        // prove it adopted *our* group. Only a successful decrypt is group-aware
        // proof of that; a plaintext probe/ack proves only that the peer holds
        // some session (possibly its own, pre-adoption group), so it must not
        // confirm us or we would stop retransmitting and strand the peer.
        if self.both_create_awaiting_decrypt.contains(peer_id) && source_event != "decrypt_success"
        {
            return false;
        }

        // A successful group decrypt is definitive, group-aware proof that the
        // peer adopted our 1:1 group. `decrypt_success` is raised only inside
        // `DecryptResult::Success` (see message_dispatch.rs): ciphertext on the
        // single stored `session:<a>:<b>` group actually decrypted, which is
        // possible only if the peer is a member. It must therefore confirm the
        // session regardless of our *local* Welcome's delivery state. Gating it on
        // a still-active local Welcome (the lifecycle match below) strands a
        // both-create owner whose own Welcome timed out to `Failed`/`Expired` on a
        // lossy or asymmetric BLE link (the Android↔iOS case): the owner decrypts
        // the peer's messages but never confirms, so every outbound send fails
        // `SessionNotReady` — it can receive but never reply. The both-create gate
        // above is preserved, so a both-create owner is still restricted to *only*
        // decrypt_success (a plaintext probe/ack can never confirm it).
        if source_event == "decrypt_success" {
            return true;
        }

        if !matches!(
            source_event,
            "confirmation_ack_received" | "confirmation_probe_received" | "confirmation_retry"
        ) {
            return true;
        }

        match self.welcome_lifecycles.get(peer_id) {
            Some(record) => matches!(
                record.state,
                WelcomeDeliveryState::Sent | WelcomeDeliveryState::SendAttempted
            ),
            None => matches!(
                source_event,
                // Compatibility path for sessions created before welcome lifecycle
                // persistence existed. A plaintext probe/ack/retry can confirm here
                // (it is the only evidence such a legacy session has); a group-aware
                // decrypt already returned true above and never reaches this arm.
                "confirmation_ack_received" | "confirmation_probe_received" | "confirmation_retry"
            ),
        }
    }

    pub(super) fn abort_pending_session_for_peer(
        &mut self,
        peer_id: &str,
        reason: crate::events::WelcomeReasonCode,
    ) {
        self.drop_pending_queue_for_peer(
            peer_id,
            &format!("Welcome delivery failed: {}", reason.as_str()),
        );
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

    /// Parks a Welcome that currently has no transport carrier: refresh the
    /// carrier-relative TTL and schedule a slow re-check, WITHOUT consuming a
    /// retry attempt, transitioning state, or emitting a `WelcomeSendAttempted`
    /// event. A no-carrier send is a guaranteed failure, so re-attempting it at
    /// the data-plane retry rate only churns storage I/O and app events while a
    /// device is offline. The lifecycle stays in its current non-terminal state
    /// (Created on the first send, Failed on a later retry) so the retry queue
    /// re-polls it on [`WELCOME_NO_CARRIER_RETRY_SECS`]; `on_neighbor_discovered`
    /// re-arms it immediately when a carrier surfaces the peer. Returns
    /// `Ok(false)` (not sent).
    ///
    /// **Invariant: the refreshed TTL always outlives the retry being
    /// scheduled.** [`Self::try_send_welcome`] checks `expires_at` *before*
    /// running a due retry, so a record whose TTL lapsed while it waited gets
    /// expired terminally (`welcome_send_expired` + `secure_session_failed`) by
    /// the very timer meant to recover it. That is not hypothetical at the far
    /// end of the unreachable ladder in
    /// [`Self::apply_recipient_unreachable_failure`]: it reaches 480s at six
    /// consecutive parks and [`WELCOME_UNREACHABLE_RETRY_CAP_SECS`] (600s)
    /// beyond that, both above the 300s [`WELCOME_LIFECYCLE_TTL_SECS`]. A full
    /// extra interval of slack keeps the probe's own outcome — a verdict
    /// re-park or a confirm timeout — inside the window too. No-op for the 15s
    /// no-carrier park, whose interval is far below the TTL.
    fn park_welcome_no_carrier(
        &mut self,
        peer_id: &str,
        reason: crate::events::WelcomeReasonCode,
        transport_error: Option<&'static str>,
        retry_in_secs: i64,
    ) -> Result<bool> {
        let snapshot = {
            let Some(record) = self.welcome_lifecycles.get_mut(peer_id) else {
                return Ok(false);
            };
            let now = Utc::now();
            record.next_retry_at = Some(now + ChronoDuration::seconds(retry_in_secs));
            record.expires_at = now
                + ChronoDuration::seconds(
                    WELCOME_LIFECYCLE_TTL_SECS.max(retry_in_secs.saturating_mul(2)),
                );
            record.last_reason_code = Some(reason);
            record.last_transport_error = transport_error.map(str::to_string);
            record.clone()
        };
        self.persist_welcome_lifecycle_entry(&snapshot)?;
        debug!(
            peer_id = %peer_id,
            state = snapshot.state.as_str(),
            retry_in_secs,
            "Welcome parked: no transport carrier, re-checking on slow interval"
        );
        Ok(false)
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

        // Only honor the TTL when a carrier actually exists: a Welcome that has
        // never had a deliverable transport must not age out.
        let carrier_available = !self.transport_manager.get_available_transports().is_empty();
        if carrier_available && record.expires_at <= now {
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

        // No carrier at all: a send is guaranteed to fail, so do NOT burn the
        // speculative attempt increment, the SendAttempted/Failed transitions
        // (two persisted state changes + two info logs), and a
        // `WelcomeSendAttempted` event on it every retry tick. Park the
        // lifecycle on a slow carrier-relative interval instead and re-check
        // later; `on_neighbor_discovered` re-arms it immediately when a carrier
        // surfaces the peer. This keeps an offline device quiet rather than
        // spinning storage I/O + app events at the data-plane retry rate.
        if !carrier_available {
            return self.park_welcome_no_carrier(
                peer_id,
                crate::events::WelcomeReasonCode::TransportUnavailable,
                None,
                WELCOME_NO_CARRIER_RETRY_SECS,
            );
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

                // No transport's send() proves the peer reassembled and joined
                // the group, so keep the lifecycle NON-TERMINAL until an explicit
                // confirmation arrives and let the retry queue re-send on timeout.
                //
                //   - Internet send() only enqueues for platform polling; the
                //     confirmation is on_transport_send_confirmed.
                //   - A BLE / WiFi-Direct GATT WRITE_TYPE_NO_RESPONSE returning Ok
                //     only means the local stack accepted the bytes. A single lost
                //     fragment of the multi-fragment Welcome would otherwise leave
                //     the sender believing the session exists while the peer holds
                //     undecryptable ciphertext, with NO retry — permanently
                //     bricking the pair. The mesh confirmation is the peer proving
                //     the session (its probe / ack / welcome / decrypt), which marks
                //     the welcome Sent via confirm_session_state.
                let confirm_timeout_secs =
                    if matches!(transport_used, Some(TransportType::Internet)) {
                        WELCOME_INTERNET_CONFIRM_TIMEOUT_SECS
                    } else {
                        WELCOME_MESH_CONFIRM_TIMEOUT_SECS
                    };
                updated.next_retry_at =
                    Some(Utc::now() + ChronoDuration::seconds(confirm_timeout_secs));
                updated.last_reason_code = None;
                updated.last_transport_error = None;
                self.welcome_lifecycles
                    .insert(peer_id.to_string(), updated.clone());
                self.persist_welcome_lifecycle_entry(&updated)?;

                // Seed the confirmation-probe scheduler for this pending peer so
                // the SENDER actively reconciles instead of waiting passively.
                //
                // The Welcome is now in flight but unconfirmed. The sender's only
                // built-in convergence path would otherwise be the receiver's
                // single proactive encrypted confirm on first Welcome receipt — a
                // single point of failure with no retry (a lost fragment, a
                // not-yet-ready encryptor, or the retransmit/owner_keep path on
                // the receiver all strand it). By marking a probe due now,
                // `run_throttled_reconciliation` (whose `has_pending_work` gate is
                // otherwise false on the sender, since it has no pending encrypted
                // messages) starts emitting `SESSION_CONFIRM_PROBE`s. The peer —
                // which holds the session — replies with `SESSION_CONFIRM_ACK`,
                // confirming us and marking this Welcome `Sent`. The entry is
                // dropped once confirmed (`clear_confirmation_recovery_tracking`)
                // or once the peer is no longer pending (`kick`'s retain), so this
                // adds no steady-state work after convergence.
                self.confirmation_probe_due_at
                    .insert(peer_id.to_string(), Utc::now());

                Ok(false)
            }
            Err(err) => {
                let no_carrier = Self::is_no_carrier_error(&err);
                let reason = Self::map_welcome_reason_code(&err);
                // Classified, not rendered: the transport layer interpolates
                // the peer into `PeerNotReachable`, and this value is both
                // persisted with the lifecycle record and emitted on
                // `WelcomeSendFailed.transport_error`, where the scrubber
                // hashes `peer_id` beside it. The full error is logged above.
                self.apply_welcome_send_failure(
                    peer_id,
                    reason,
                    Some(send_failure_token(&err)),
                    no_carrier,
                    source_event,
                )
            }
        }
    }

    /// Re-arms a stalled or expired outbound Welcome when a peer becomes
    /// reachable again, so a session that never converged while the peer was
    /// offline gets a fresh delivery attempt over the now-available carrier.
    ///
    /// No-op when the peer's session is already confirmed, when there is no
    /// Welcome lifecycle, or when the Welcome is already `Sent` or still
    /// in-flight (`SendAttempted`). An `Expired` lifecycle is rebuilt from its
    /// retained Welcome message — the MLS group is not torn down on expiry, so
    /// the stored Welcome is still valid to re-send — while a `Created`/`Failed`
    /// lifecycle has its budget and TTL reset and is retried immediately.
    pub(super) fn rearm_welcome_for_peer(&mut self, peer_id: &str, source_event: &str) {
        if self.confirmed_sessions.contains(peer_id) {
            return;
        }
        let Some(record) = self.welcome_lifecycles.get(peer_id).cloned() else {
            return;
        };
        match record.state {
            WelcomeDeliveryState::Sent | WelcomeDeliveryState::SendAttempted => {}
            WelcomeDeliveryState::Created | WelcomeDeliveryState::Failed => {
                if let Some(entry) = self.welcome_lifecycles.get_mut(peer_id) {
                    entry.attempt = 0;
                    // A reachability edge resets the unreachable-park
                    // escalation: the next relay verdict starts the interval
                    // ladder over from its base.
                    entry.unreachable_parks = 0;
                    entry.next_retry_at = Some(Utc::now());
                    entry.last_reason_code = None;
                    entry.last_transport_error = None;
                    entry.expires_at =
                        Utc::now() + ChronoDuration::seconds(WELCOME_LIFECYCLE_TTL_SECS);
                }
                if let Err(err) = self.try_send_welcome(peer_id, source_event) {
                    warn!(
                        peer_id = %peer_id,
                        error = %err,
                        "Failed to re-arm welcome on peer reachability"
                    );
                }
            }
            WelcomeDeliveryState::Expired => {
                // Expired is terminal in the lifecycle state machine, so rebuild
                // from the retained Welcome message (upsert permits overwrite
                // from Expired) before re-sending.
                let welcome_message = record.welcome_message.clone();
                let group_id = record.group_id.clone();
                if let Err(err) =
                    self.upsert_welcome_lifecycle(peer_id, &group_id, welcome_message, source_event)
                {
                    warn!(
                        peer_id = %peer_id,
                        error = %err,
                        "Failed to rebuild expired welcome on peer reachability"
                    );
                    return;
                }
                if let Err(err) = self.try_send_welcome(peer_id, source_event) {
                    warn!(
                        peer_id = %peer_id,
                        error = %err,
                        "Failed to re-arm expired welcome on peer reachability"
                    );
                }
            }
        }
    }

    /// Ingests an authoritative per-peer presence signal (the internet relay's
    /// `PresenceStatusWithLastSeen` answer, bridged by the platform layer).
    ///
    /// `online` drives the same reachability machinery as a transport-level
    /// discovery — outbox flush, welcome re-arm, auto key exchange — plus a
    /// rescue for the store-less-relay hole: a welcome whose wire send was
    /// confirmed (`Sent`) while the peer was actually offline was dropped by
    /// the relay (it forwards or pushes, never stores), and
    /// `Self::rearm_welcome_for_peer` deliberately never touches `Sent`.
    /// The peer being provably online is the safe moment to rebuild and
    /// re-send it: if the original did land, the receiver dedups by message
    /// id and the confirmation probe resolves the lifecycle.
    ///
    /// `offline` parks retry-pending welcomes pending a reachability edge,
    /// without burning budget. In-flight records (`SendAttempted`) are left
    /// to their confirm deadline — a `DeliveryError`-driven failure or the
    /// confirm timeout resolves them, after which the next offline tick parks.
    ///
    /// The welcome re-arm/re-send on the online edge is throttled per peer
    /// with exponential backoff (`WELCOME_PRESENCE_RESCUE_BASE_SECS` ..
    /// `WELCOME_PRESENCE_RESCUE_MAX_SECS`): presence answers arrive on the
    /// platform's ~20s watch tick, and a peer that is online but can never
    /// prove the session (stale key package after a reinstall, incompatible
    /// version) must not be re-sent the multi-frame MLS welcome on every
    /// tick forever. A throttled online tick still flushes queued data-plane
    /// traffic for the peer.
    ///
    /// Emits `presence_updated` with `source: Internet` — this function is
    /// the relay-sourced half of the unified stream and must only be fed
    /// relay-observed signals (bridged via `internet_peer_presence`);
    /// peer-sent `__PRESENCE__` self-reports are handled in
    /// `message_dispatch` and emit `source: Peer`. Self, blocked, or empty
    /// peer ids are dropped entirely: an app waiting on `presence_updated`
    /// for a blocked peer will never see it.
    pub fn on_peer_presence(&mut self, peer_id: &str, online: bool, last_seen_ms: Option<i64>) {
        if peer_id.is_empty() || peer_id == self.local_id || self.is_user_blocked(peer_id) {
            return;
        }
        // A presence answer is a claim about this recipient on the carrier
        // that answered, so it is recorded as one. It supersedes an earlier
        // verdict for that carrier: both answer the same question and this is
        // the later answer. Presence facts decay faster than verdicts, since
        // this is a third party's report about someone else's connection.
        self.reachability.record(
            peer_id,
            TransportType::Internet,
            if online {
                Claim::Reachable
            } else {
                Claim::Unreachable
            },
            FactSource::GatewayPresence,
            std::time::Instant::now(),
        );
        if online {
            // Relay-scoped reachability proof: un-park and re-drive the
            // peer's DMs over the internet transport before anything else
            // (see `flush_outbox_for_peer_via` — this also cancels
            // unanswerable mesh-probe ACKs that would otherwise hold the
            // messages hostage past this edge). The rescue branch's inner
            // flush then finds successfully re-driven entries awaiting
            // fresh ACKs and leaves them alone; entries whose forced send
            // *failed* are picked up again, so the inner flush keeps the
            // internet override — DORS must not re-route them into the
            // mesh void with the park counter already cleared.
            self.flush_outbox_for_peer_via(peer_id, Some(TransportType::Internet));
            if self.welcome_rescue_permitted(peer_id) {
                self.on_neighbor_discovered_via(peer_id, Some(TransportType::Internet));
                self.resend_unconfirmed_sent_welcome(peer_id, "peer_presence_online");
                self.note_welcome_rescue_attempt(peer_id);
            }
        } else {
            self.park_welcome_peer_unreachable(peer_id);
        }

        let status = if online {
            crate::events::PresenceStatus::Online
        } else {
            crate::events::PresenceStatus::Offline
        };
        self.emit_event(Event::presence_updated_with_last_seen(
            peer_id.to_string(),
            status,
            Utc::now().timestamp_millis(),
            last_seen_ms,
        ));
    }

    /// Peers with a welcome lifecycle whose session has not been proven —
    /// the internet presence watch set. Includes `Sent` records (wire-
    /// confirmed but never session-confirmed): only a presence signal can
    /// rescue those over a store-less relay.
    /// Lifecycles older than `WELCOME_WATCHLIST_MAX_AGE_SECS` are excluded:
    /// a permanently-dead peer must not occupy watch-rotation slots (and keep
    /// its parked record alive via `expires_at` pushes) forever.
    pub fn welcome_pending_peers(&self) -> Vec<String> {
        let watch_cutoff = Utc::now() - ChronoDuration::seconds(WELCOME_WATCHLIST_MAX_AGE_SECS);
        self.welcome_lifecycles
            .iter()
            .filter(|(peer_id, record)| {
                !self.confirmed_sessions.contains(*peer_id) && record.created_at > watch_cutoff
            })
            .map(|(peer_id, _)| peer_id.clone())
            .collect()
    }

    /// Rebuilds and re-sends a `Sent`-but-session-unconfirmed welcome (see
    /// [`Self::on_peer_presence`]). No-op for any other state.
    fn resend_unconfirmed_sent_welcome(&mut self, peer_id: &str, source_event: &str) {
        if self.confirmed_sessions.contains(peer_id) {
            return;
        }
        let Some(record) = self.welcome_lifecycles.get(peer_id) else {
            return;
        };
        if !matches!(record.state, WelcomeDeliveryState::Sent) {
            return;
        }
        let welcome_message = record.welcome_message.clone();
        let group_id = record.group_id.clone();
        if let Err(err) =
            self.upsert_welcome_lifecycle(peer_id, &group_id, welcome_message, source_event)
        {
            warn!(
                peer_id = %peer_id,
                error = %err,
                "Failed to rebuild unconfirmed sent welcome on presence"
            );
            return;
        }
        if let Err(err) = self.try_send_welcome(peer_id, source_event) {
            warn!(
                peer_id = %peer_id,
                error = %err,
                "Failed to re-send unconfirmed welcome on presence"
            );
        }
    }

    /// True when a presence-online rescue for this peer is currently allowed
    /// by the per-peer backoff. Trivially true (and the throttle entry is
    /// dropped) once the peer has no unconfirmed welcome — there is nothing
    /// left to throttle.
    fn welcome_rescue_permitted(&mut self, peer_id: &str) -> bool {
        let pending = self.welcome_lifecycles.contains_key(peer_id)
            && !self.confirmed_sessions.contains(peer_id);
        if !pending {
            self.welcome_presence_rescue.remove(peer_id);
            return true;
        }
        self.welcome_presence_rescue
            .get(peer_id)
            .is_none_or(|throttle| throttle.next_allowed_at <= Utc::now())
    }

    /// Records that a presence-online tick ran the rescue actions for a peer
    /// that still has an unconfirmed welcome, doubling the wait before the
    /// next one (base 40s, capped at 10 minutes). The counter is per
    /// convergence attempt: the entry is dropped (via
    /// [`Self::welcome_rescue_permitted`]) as soon as the session confirms
    /// or the lifecycle goes away.
    fn note_welcome_rescue_attempt(&mut self, peer_id: &str) {
        let pending = self.welcome_lifecycles.contains_key(peer_id)
            && !self.confirmed_sessions.contains(peer_id);
        if !pending {
            self.welcome_presence_rescue.remove(peer_id);
            return;
        }
        let throttle = self
            .welcome_presence_rescue
            .entry(peer_id.to_string())
            .or_insert(PresenceRescueThrottle {
                next_allowed_at: Utc::now(),
                rescues: 0,
            });
        // Shift is clamped to 8, so 40 << 8 = 10240 max before the cap —
        // no overflow risk.
        let backoff_secs = (WELCOME_PRESENCE_RESCUE_BASE_SECS << throttle.rescues.min(8))
            .min(WELCOME_PRESENCE_RESCUE_MAX_SECS);
        throttle.next_allowed_at = Utc::now() + ChronoDuration::seconds(backoff_secs);
        throttle.rescues = throttle.rescues.saturating_add(1);
    }

    /// Parks a welcome pending a peer-reachability edge: reason
    /// `PeerUnreachable`, **no timed retry**. The carrier (relay socket) is
    /// healthy — only this peer is unreachable on it — so the *data-plane*
    /// retry this cancels would re-send the welcome into another
    /// `DeliveryError` while burning the real-delivery budget that a
    /// carrier-backed failure already charged. Recovery is edge-driven
    /// instead: `on_peer_presence(online)` and `on_neighbor_discovered` re-arm
    /// via `rearm_welcome_for_peer`, and the peer stays on the presence
    /// watchlist (`welcome_pending_peers`) so the platform keeps polling for
    /// that edge. The TTL is pushed like a no-carrier park: an unreachable
    /// peer must not age the welcome out.
    ///
    /// Its caller is responsible for not applying this to a record already
    /// holding an escalating unreachable probe — see
    /// [`Self::park_welcome_peer_unreachable`], which is what distinguishes
    /// the budget-burning data-plane retry cancelled here from the
    /// budget-refunded probe that must survive.
    fn park_welcome_awaiting_peer(&mut self, peer_id: &str) -> Result<()> {
        let snapshot = {
            let Some(record) = self.welcome_lifecycles.get_mut(peer_id) else {
                return Ok(());
            };
            record.next_retry_at = None;
            record.expires_at = Utc::now() + ChronoDuration::seconds(WELCOME_LIFECYCLE_TTL_SECS);
            record.last_reason_code = Some(crate::events::WelcomeReasonCode::PeerUnreachable);
            record.clone()
        };
        self.persist_welcome_lifecycle_entry(&snapshot)?;
        debug!(
            peer_id = %peer_id,
            state = snapshot.state.as_str(),
            "Welcome parked: peer unreachable on carrier, awaiting reachability edge"
        );
        Ok(())
    }

    /// Parks a retry-pending welcome for a peer the relay reports offline.
    /// Only `Created`/`Failed` records park: `SendAttempted` keeps its
    /// confirm deadline, and `Sent`/`Expired` are handled on the online edge
    /// (or corrected by an attributed `DeliveryError`). Skipped entirely when
    /// a mesh carrier is also available — relay presence is only
    /// authoritative for the internet path, and the data-plane retry track
    /// may still deliver over BLE / WiFi-Direct.
    ///
    /// Never cancels an escalating unreachable probe: a record that already
    /// holds one was parked by [`Self::apply_recipient_unreachable_failure`],
    /// which already set the same reason code and pushed the same TTL — so this
    /// adds nothing and would only downgrade the probe to edge-only. That
    /// matters because on an internet-only device this runs once per presence
    /// rotation for as long as the peer is down, which would otherwise
    /// re-strand the welcome after every single verdict. The distinction is
    /// budget: the data-plane retry this *does* cancel was charged an attempt
    /// by a carrier-backed failure, while the probe refunds its attempt on
    /// every verdict.
    ///
    /// `last_reason_code` — not `unreachable_parks` alone — is what encodes
    /// that distinction. The counter is sticky (only `rearm_welcome_for_peer`
    /// clears it), so gating on it by itself would keep shielding the record
    /// after the probe track has stopped protecting it: past
    /// [`WELCOME_WATCHLIST_MAX_AGE_SECS`] a confirm timeout no longer re-parks
    /// (see [`Self::welcome_probe_repark_permitted`]) and instead charges an
    /// attempt and arms a plain data-plane retry — precisely the budget-burner
    /// this park exists to cancel, and left running it expires the welcome at
    /// `max_retries`. Note `park_welcome_awaiting_peer` also stamps
    /// `PeerUnreachable` but leaves `next_retry_at = None`, so the
    /// `next_retry_at.is_some()` conjunct below is load-bearing, not
    /// redundant: the reason code *and* a scheduled retry together are what
    /// identify a live probe. A carrier-backed failure stamps
    /// `Timeout`/`SendFailed` and a successful send clears the code entirely.
    fn park_welcome_peer_unreachable(&mut self, peer_id: &str) {
        if self.confirmed_sessions.contains(peer_id) {
            return;
        }
        let Some(record) = self.welcome_lifecycles.get(peer_id) else {
            return;
        };
        if !matches!(
            record.state,
            WelcomeDeliveryState::Created | WelcomeDeliveryState::Failed
        ) {
            return;
        }
        if record.last_reason_code == Some(crate::events::WelcomeReasonCode::PeerUnreachable)
            && record.unreachable_parks > 0
            && record.next_retry_at.is_some()
        {
            return;
        }
        // Local mesh carriers only (BLE / WiFi-Direct): Nostr and Reticulum
        // are internet-dependent, so their availability must not veto the
        // park — mirrors the carrier guard in
        // `apply_recipient_unreachable_failure`.
        if self
            .transport_manager
            .get_available_transports()
            .keys()
            .any(|transport| matches!(transport, TransportType::BLE | TransportType::WiFiDirect))
        {
            return;
        }
        let _ = self.park_welcome_awaiting_peer(peer_id);
    }

    /// True when an unconfirmed welcome send should resolve as another
    /// unreachable verdict ([`Self::apply_recipient_unreachable_failure`])
    /// rather than as a carrier-backed failure that ages the record.
    ///
    /// A live `unreachable_parks` counter means the relay declared the peer
    /// offline and no reachability edge has fired since (every edge clears it
    /// via `rearm_welcome_for_peer`), so the send that just went unconfirmed
    /// was the escalating probe, not a delivery attempt. Without this the
    /// probe destroys the very welcome it exists to recover: the relay
    /// accepts the frame whenever its push fallback succeeds, which returns
    /// *no* `DeliveryError` at all, so the probe resolves at the 10s confirm
    /// timeout instead. Treated as carrier-backed that charges the attempt
    /// and arms a plain data-plane retry, and since nothing in that ladder
    /// pushes `expires_at`, the record walks into `should_expire` within one
    /// TTL window — terminal `welcome_send_expired` + `secure_session_failed`
    /// for a peer that is merely offline. Re-parking instead refunds the
    /// attempt, escalates the interval and pushes the TTL, mirroring the DM
    /// path's [`Self::try_repark_exhausted_dm`]: settlement stays reserved
    /// for delivery or the record genuinely ageing out below.
    ///
    /// Only the *presence-offline* answer cancelled that retry before, which
    /// is not a defense at all for the case this whole probe exists to serve
    /// — a headless consumer that never polls presence.
    ///
    /// Bounded by [`WELCOME_WATCHLIST_MAX_AGE_SECS`] measured from
    /// `created_at`, the same threshold that stops watching a peer as
    /// permanently dead: past it the counter no longer shields the record and
    /// normal ageing expires it. This is the welcome's twin of the DM probe's
    /// absolute bound (`first_sent_at` vs the outbox absolute lifetime) —
    /// without it a peer that never returns keeps a welcome probing at the
    /// 600s cap forever.
    ///
    /// Deliberately narrow to `Timeout`: a `SendFailed`/`TransportUnavailable`
    /// failure is evidence about the *carrier*, not about this peer, and
    /// keeps ageing the record normally.
    fn welcome_probe_repark_permitted(&self, peer_id: &str) -> bool {
        let Some(record) = self.welcome_lifecycles.get(peer_id) else {
            return false;
        };
        record.unreachable_parks > 0
            && record.created_at
                > Utc::now() - ChronoDuration::seconds(WELCOME_WATCHLIST_MAX_AGE_SECS)
    }

    /// Applies the relay's authoritative "recipient offline" verdict
    /// (a `recipient_unreachable`-tagged transport failure) to a welcome
    /// lifecycle. Also reached *without* a relay verdict when a probe's
    /// confirm timeout resolves while the park counter is still live — the
    /// same conclusion inferred rather than relayed, see
    /// [`Self::welcome_probe_repark_permitted`].
    ///
    /// The bridge wire-confirms on socket-write success, so by the time the
    /// relay's `DeliveryError` arrives the record is normally already `Sent`
    /// — a *false* Sent: the store-less relay dropped the content. Correct
    /// it back to `Failed` (emitting `welcome_send_failed` so the app's
    /// earlier `welcome_send_succeeded` is superseded), refund the attempt
    /// (the peer never saw the frame), and park pending a reachability edge.
    /// `SendAttempted` (the failure raced ahead of the wire confirm) gets
    /// the same treatment; `Created`/`Failed` records just park. Terminal
    /// `Expired` records are left to the online edge, which rebuilds them.
    ///
    /// The record always keeps a timed retry whose interval escalates per
    /// consecutive park. Relay presence is authoritative only for the
    /// internet path — with a mesh carrier live the peer may still be
    /// reachable over BLE / WiFi-Direct (DORS can pick internet even with
    /// mesh present) — but an internet-only device needs the timed track just
    /// as much: parking it edge-only left the welcome dependent solely on a
    /// presence-online answer, i.e. on the platform's presence-polling
    /// cadence, which is the same dead end that stalled parked DMs (see
    /// [`Self::park_unreachable_dm`]). The escalation (15s → 600s cap) is
    /// what bounds the retry: DORS may keep routing to the internet path,
    /// where each round trips another budget-refunded `DeliveryError`, and
    /// without escalation that would be an unbounded resend loop into the
    /// relay. Every verdict refunds the attempt and pushes the TTL — past the
    /// scheduled probe, see the invariant on [`Self::park_welcome_no_carrier`]
    /// — so the probes never age the welcome out or burn its real-delivery
    /// budget, and the counter resets on any reachability edge
    /// (`rearm_welcome_for_peer`).
    pub(super) fn apply_recipient_unreachable_failure(
        &mut self,
        peer_id: &str,
        transport_error: Option<&'static str>,
    ) -> Result<()> {
        // A late relay verdict for a session that has since been proven (the
        // welcome was rescued over another path and the peer confirmed) must
        // not corrupt the converged lifecycle: without this guard the stale
        // DeliveryError flips the record back to Failed, refunds an attempt,
        // persists the stale state, and emits welcome_send_failed AFTER the
        // app already saw secure_session_established.
        if self.confirmed_sessions.contains(peer_id) {
            return Ok(());
        }
        let Some(state) = self.welcome_lifecycles.get(peer_id).map(|r| r.state) else {
            return Ok(());
        };
        match state {
            WelcomeDeliveryState::Sent | WelcomeDeliveryState::SendAttempted => {
                self.transition_welcome_state(
                    peer_id,
                    WelcomeDeliveryState::Failed,
                    "recipient_unreachable",
                )?;
                if let Some(record) = self.welcome_lifecycles.get_mut(peer_id) {
                    record.attempt = record.attempt.saturating_sub(1);
                    record.last_transport_error = transport_error.map(str::to_string);
                }
            }
            WelcomeDeliveryState::Created | WelcomeDeliveryState::Failed => {
                if let Some(record) = self.welcome_lifecycles.get_mut(peer_id) {
                    record.last_transport_error = transport_error.map(str::to_string);
                }
            }
            WelcomeDeliveryState::Expired => return Ok(()),
        }
        // Escalating timed retry on every carrier (see the doc comment): the
        // ladder, not a carrier guard, is what bounds resends into the relay.
        let parks = {
            let Some(record) = self.welcome_lifecycles.get_mut(peer_id) else {
                return Ok(());
            };
            record.unreachable_parks = record.unreachable_parks.saturating_add(1);
            record.unreachable_parks
        };
        // parks >= 1 here; shift clamped so 15 << 6 = 960 is the largest
        // pre-cap value — no overflow risk.
        let retry_in_secs = (WELCOME_NO_CARRIER_RETRY_SECS << (parks - 1).min(6))
            .min(WELCOME_UNREACHABLE_RETRY_CAP_SECS);
        self.park_welcome_no_carrier(
            peer_id,
            crate::events::WelcomeReasonCode::PeerUnreachable,
            transport_error,
            retry_in_secs,
        )?;

        let Some(snapshot) = self.welcome_lifecycles.get(peer_id).cloned() else {
            return Ok(());
        };
        if let Ok(shared) = lock_shared_state(&self.shared_state) {
            shared.emit_event(Event::welcome_send_failed(
                peer_id.to_string(),
                snapshot.welcome_message.id.as_str().to_string(),
                snapshot.group_id.clone(),
                snapshot.attempt,
                crate::events::WelcomeReasonCode::PeerUnreachable,
                snapshot
                    .last_transport_error
                    .as_deref()
                    .map(classify_transport_send_error),
                // Retryable, and now always with a scheduled time: the park
                // keeps an escalating timed probe on every carrier. The
                // reachability edges (presence online / peer discovery) still
                // re-arm it sooner when they fire.
                true,
                snapshot.next_retry_at.map(|at| at.timestamp_millis()),
            ));
        }
        Ok(())
    }

    /// Records a Welcome send failure and decides whether to retry or expire.
    ///
    /// `no_carrier` is `true` when the failure was simply that no transport
    /// carrier exists yet (vs. a present carrier that failed mid-send). A
    /// no-carrier failure must NOT age the Welcome: it neither counts toward
    /// the retry budget nor lets the TTL expire it, because the peer is merely
    /// unreachable for now and will be retried once a carrier appears. The
    /// speculative attempt increment from [`Self::try_send_welcome`] is rolled
    /// back so a long offline period leaves the real-delivery budget intact.
    pub(super) fn apply_welcome_send_failure(
        &mut self,
        peer_id: &str,
        reason: crate::events::WelcomeReasonCode,
        transport_error: Option<&'static str>,
        no_carrier: bool,
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

        // A confirm timeout on a welcome that still holds a live
        // unreachable-park counter is a *probe* resolving, not a delivery
        // failing — re-park it as another unreachable verdict instead of
        // ageing it (see [`Self::welcome_probe_repark_permitted`]).
        if !no_carrier
            && matches!(reason, crate::events::WelcomeReasonCode::Timeout)
            && self.welcome_probe_repark_permitted(peer_id)
        {
            self.apply_recipient_unreachable_failure(peer_id, transport_error)?;
            return Ok(false);
        }

        let max_attempts = self.config.reliability.retry.max_retries.max(1);
        // A no-carrier failure never expires the Welcome: neither the retry
        // budget nor the TTL applies while the peer is simply unreachable. Only
        // a carrier-backed failure (SendFailed / Timeout) ages it toward expiry.
        let should_expire =
            !no_carrier && (updated.attempt >= max_attempts || updated.expires_at <= Utc::now());
        if should_expire {
            let terminal_reason = crate::events::WelcomeReasonCode::RetryExhausted;
            {
                let record = self.welcome_lifecycles.get_mut(peer_id).ok_or_else(|| {
                    Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
                })?;
                record.last_reason_code = Some(terminal_reason);
                record.last_transport_error = transport_error.map(str::to_string);
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
                    expired_snapshot
                        .last_transport_error
                        .as_deref()
                        .map(classify_transport_send_error),
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

        // A no-carrier failure (the carrier raced away after the pre-send check,
        // or an async transport-failed callback with no carrier) re-checks on the
        // slow no-carrier interval rather than the data-plane backoff: there is
        // nothing to deliver over until a carrier returns, and `try_send_welcome`
        // parks subsequent no-carrier ticks cheaply anyway.
        let retry_at = if no_carrier {
            Utc::now() + ChronoDuration::seconds(WELCOME_NO_CARRIER_RETRY_SECS)
        } else {
            let delay_ms = self.compute_welcome_retry_delay_ms(peer_id, updated.attempt);
            Utc::now() + ChronoDuration::milliseconds(delay_ms as i64)
        };

        {
            let record = self.welcome_lifecycles.get_mut(peer_id).ok_or_else(|| {
                Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
            })?;
            // Roll back the speculative attempt increment from try_send_welcome
            // for a no-carrier failure so the retry budget stays reserved for
            // real delivery attempts over an available carrier, and push the TTL
            // forward so a never-deliverable Welcome does not age out — the TTL
            // clock is carrier-relative, not creation-relative.
            if no_carrier {
                record.attempt = record.attempt.saturating_sub(1);
                record.expires_at =
                    Utc::now() + ChronoDuration::seconds(WELCOME_LIFECYCLE_TTL_SECS);
            }
            record.last_reason_code = Some(reason);
            record.last_transport_error = transport_error.map(str::to_string);
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
                updated
                    .last_transport_error
                    .as_deref()
                    .map(classify_transport_send_error),
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
                // Sent is normally a sink, but the relay's DeliveryError is
                // authoritative proof that a wire-confirmed frame was dropped
                // (store-less relay, recipient offline). That correction is
                // the one legal way back out of Sent — see
                // `on_transport_send_failed`.
                | (WelcomeDeliveryState::Sent, WelcomeDeliveryState::Failed)
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
            unreachable_parks: 0,
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

    /// True when a send failed because no transport carrier exists yet, as
    /// opposed to a present-but-unhealthy carrier (`SendFailed`) or a
    /// sent-but-unconfirmed timeout. A no-carrier failure means the peer is
    /// simply unreachable right now, so the Welcome must be kept alive and
    /// retried rather than counted against its budget — see
    /// [`Self::apply_welcome_send_failure`].
    pub(super) fn is_no_carrier_error(error: &Error) -> bool {
        matches!(
            error,
            Error::Transport(offline_protocol_transport::Error::TransportNotAvailable(_))
        )
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
                // Skip peers whose session is already confirmed: the welcome is
                // delivered, mark_welcome_confirmed should have marked it Sent,
                // and re-sending would be wasted bandwidth (defensive against any
                // keying mismatch that left the lifecycle non-terminal).
                if self.confirmed_sessions.contains(peer_id) {
                    return None;
                }
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
                Some(SEND_FAIL_REASON_CONFIRM_TIMEOUT),
                // A confirm timeout means the Welcome was sent over a carrier;
                // this is not a no-carrier failure, so it ages normally.
                false,
                "welcome_confirm_timeout",
            )?;
        }

        let due_peers: Vec<String> = self
            .welcome_lifecycles
            .iter()
            .filter_map(|(peer_id, record)| {
                if self.confirmed_sessions.contains(peer_id) {
                    return None;
                }
                // Failed = a normal retry-due Welcome. Created with a due
                // next_retry_at = a no-carrier-parked Welcome (see
                // `park_welcome_no_carrier`): a fresh Created entry has
                // next_retry_at == None, so only parked ones match here.
                if matches!(
                    record.state,
                    WelcomeDeliveryState::Failed | WelcomeDeliveryState::Created
                ) && record.next_retry_at.is_some_and(|retry_at| retry_at <= now)
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
