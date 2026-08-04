//! Message dispatch handlers for internal protocol messages.

use super::{
    base64_decode, internal_prefixes, lock_shared_state, ConnectionAcceptedPayload,
    ConnectionRequestPayload, GroupCreatedPayload, GroupErrorPayload, GroupInfoPayload,
    GroupMemberAddedPayload, GroupMemberRemovedPayload, GroupMessageReceivedPayload,
    InternalMessageResult, KeyPackagePayload, OfflineProtocol, PeerCapabilities, PresencePayload,
    ReadReceiptPayload, ReceivedKeyPackage, TypingIndicatorPayload, UserGroupsPayload,
    MAX_KEY_PACKAGE_LIFETIME_MS, MAX_KEY_PACKAGE_SENT_TO, MAX_PENDING_KEY_PACKAGES,
    MAX_READ_RECEIPT_IDS, MLS_ENVELOPE_COMPACT_V1, RICH_PAYLOAD_V1,
};
use crate::events::{DecryptionFailureCode, Event};
use crate::mls_observability::{DecryptionFailureKind, MlsErrorCategory, MlsOperationContext};
use crate::SessionStateError;
use chrono::Utc;
use offline_protocol_core::{Message, MessagePriority};
use offline_protocol_mls::{EncryptedMessage, WelcomeMessage};
use offline_protocol_services::ServiceAction;
use offline_protocol_transport::TransportType;
use tracing::{debug, error, info, warn};

impl OfflineProtocol {
    /// Handles an incoming MLS key package message.
    pub(crate) fn handle_key_package_message(&mut self, sender: &str, data: &str) {
        if let Ok(payload) = serde_json::from_str::<KeyPackagePayload>(data) {
            debug!(sender = %sender, session_reset = %payload.session_reset, "Received key package");

            // Record whether this peer can decode our binary wire frames, so the
            // transport manager may stamp binary for messages addressed to them.
            // Gated by our own config: with the codec disabled we neither
            // advertise nor record capability, so both directions stay on JSON.
            if self.config.transport.binary_wire_enabled {
                self.transport_manager.mark_peer_binary_wire(
                    sender,
                    payload
                        .wire_versions
                        .contains(&offline_protocol_core::WIRE_VERSION_V1),
                );
            }

            // Record whether this peer parses the compact MLS envelope, so
            // `seal_encrypted_content` may emit it for messages encrypted to
            // them. Same shape as the binary-wire capability above: gated by
            // our own config so a kill switch stops both directions, and
            // removed when a fresh key package no longer advertises it (peer
            // downgrade). Bounded like `key_package_sent_to` — the set is
            // keyed by the wire-claimed sender, and forgetting a peer only
            // costs a fallback to the JSON envelope.
            if self.config.encryption.compact_envelope_enabled
                && payload.env_versions.contains(&MLS_ENVELOPE_COMPACT_V1)
            {
                if !self.peer_compact_envelope.contains(sender)
                    && self.peer_compact_envelope.len() >= MAX_KEY_PACKAGE_SENT_TO
                {
                    self.peer_compact_envelope.clear();
                }
                self.peer_compact_envelope.insert(sender.to_string());
            } else {
                self.peer_compact_envelope.remove(sender);
            }

            // Record whether this peer parses the sealed rich payload, so the
            // send path may seal rich extras for messages encrypted to them.
            // Same shape as the compact-envelope capability above: gated by
            // our own config so the kill switch stops both directions,
            // removed when a fresh key package no longer advertises it (peer
            // downgrade), and bounded like `key_package_sent_to`. Forgetting
            // a peer only costs silently dropped rich extras — never a
            // cleartext fallback.
            if self.config.encryption.rich_payload_enabled
                && payload.rich_versions.contains(&RICH_PAYLOAD_V1)
            {
                if !self.peer_rich_payload.contains(sender)
                    && self.peer_rich_payload.len() >= MAX_KEY_PACKAGE_SENT_TO
                {
                    self.peer_rich_payload.clear();
                }
                self.peer_rich_payload.insert(sender.to_string());
            } else {
                self.peer_rich_payload.remove(sender);
            }

            // A direct key package is authoritative for this peer: drop any
            // inviter-attested rich entry (the durable side happens below —
            // `from_advertised` never carries the attested field, and an
            // all-empty advertisement deletes the record outright), so a
            // stale or forged attestation cannot outlive real contact.
            self.peer_rich_attested.remove(sender);

            // Persist the raw advertised end-to-end capabilities so they
            // survive restarts: the cached key package persisted below is
            // deleted once a session is established, but the capability must
            // outlive it — a rich send right after relaunch would otherwise
            // silently drop its extras until the next live exchange. Raw
            // versions rather than the config-gated subset (the kill
            // switches gate the recording above, restore, and send — not
            // knowledge). A package advertising nothing deletes the record:
            // the durable side of the downgrade semantics above.
            let caps =
                PeerCapabilities::from_advertised(&payload.env_versions, &payload.rich_versions);
            if caps.is_any() {
                self.persist_peer_capabilities(sender, &caps);
            } else {
                self.delete_peer_capabilities_from_storage(sender);
            }

            // If the sender has reset their session (e.g. after unblocking us),
            // we must discard our stale local session so both sides converge on
            // a fresh MLS group.
            if payload.session_reset {
                if let Some(mls) = self.mls_manager.clone() {
                    if let Ok(manager) = mls.read() {
                        if manager.has_session(sender).unwrap_or(false) {
                            drop(manager); // release lock before mutating
                            info!(sender = %sender, "Session reset requested — deleting stale local session");
                            if let Err(e) = self.manual_mls_delete_session(sender) {
                                debug!(sender = %sender, error = %e, "No MLS session to clean up for session reset");
                            }
                            // Clear outbound pending messages (encrypted for the old session)
                            self.drop_pending_queue_for_peer(sender);
                            // Drain inbound pending decryption queue (old ciphertexts)
                            self.pending_queue
                                .drain_for_peer(&self.config.encryption.pending_queue, sender);
                            // Allow fresh key exchange
                            self.key_package_sent_to.remove(sender);
                        }
                    }
                }
            }

            let now_ms = Utc::now().timestamp_millis() as u64;
            // Clamp the peer-supplied lifetime to `MAX_KEY_PACKAGE_LIFETIME_MS`.
            // It is an unauthenticated wire field that becomes the eviction sort
            // key (soonest-to-expire) for `pending_key_packages`; without a
            // ceiling a flood of forged senders claiming a maximal lifetime
            // would pin their entries as latest-to-expire and preferentially
            // evict legitimate peers. A legacy sender omitting the field (0)
            // falls back to the same default. This only bounds the *cached*
            // expiry — OpenMLS enforces real key-package validity at use time.
            let lifetime_ms = if payload.remaining_lifetime_ms > 0 {
                payload
                    .remaining_lifetime_ms
                    .min(MAX_KEY_PACKAGE_LIFETIME_MS)
            } else {
                MAX_KEY_PACKAGE_LIFETIME_MS
            };
            let local_expires_at_ms = now_ms.saturating_add(lifetime_ms);
            let pkg = ReceivedKeyPackage {
                key_package_data: payload.key_package_data,
                local_expires_at_ms,
            };
            // SECURITY (resource exhaustion): `pending_key_packages` is keyed by
            // the wire-claimed `sender`, and every insert also writes a durable
            // entry via `persist_peer_key_package` (iOS Keychain / Android
            // Keystore). Under the default config an unpinned peer can flood
            // distinct forged senders, so bound the map exactly like
            // `known_peers`: at capacity, evict the soonest-to-expire entry and
            // drop its persisted copy before inserting, so neither memory nor
            // durable storage grows without bound or survives a reboot
            // re-inflated. A refreshed existing peer (already present) never
            // triggers eviction.
            if !self.pending_key_packages.contains_key(sender)
                && self.pending_key_packages.len() >= MAX_PENDING_KEY_PACKAGES
            {
                if let Some(victim) = self
                    .pending_key_packages
                    .iter()
                    .min_by_key(|(_, p)| p.local_expires_at_ms)
                    .map(|(id, _)| id.clone())
                {
                    debug!(
                        peer_id = %sender,
                        evicted = %victim,
                        cap = MAX_PENDING_KEY_PACKAGES,
                        "Pending key packages at capacity, evicting soonest-to-expire"
                    );
                    self.pending_key_packages.remove(&victim);
                    self.delete_peer_key_package_from_storage(&victim);
                    // Evict the victim's capability record with its key
                    // package, tying the durable capability count to the same
                    // flood bound — but only for victims without an
                    // established session. Session peers DO land in this map:
                    // a package re-advertised by a peer that restarted (its
                    // in-memory key_package_sent_to cleared) is inserted
                    // above but never consumed, because auto-establish sees
                    // the existing session and leaves it. They are exactly
                    // the population whose capabilities must survive
                    // restarts, so letting a forged-sender flood delete their
                    // records would reopen the silent rich-degrade window
                    // (#200) after the next relaunch. A non-session victim's
                    // record is as recoverable as its key package: the peer
                    // re-advertises on the next exchange.
                    let victim_has_session = self
                        .mls_manager
                        .as_ref()
                        .and_then(|mls| mls.read().ok())
                        .map(|manager| manager.has_session(&victim).unwrap_or(false))
                        .unwrap_or(false);
                    if !victim_has_session {
                        self.delete_peer_capabilities_from_storage(&victim);
                    }
                }
            }
            self.pending_key_packages
                .insert(sender.to_string(), pkg.clone());
            self.persist_peer_key_package(sender, &pkg);

            // Send our key package back if auto_key_exchange is enabled
            if self.config.encryption.auto_key_exchange
                && self.config.encryption.enabled
                && !self.key_package_sent_to.contains(sender)
            {
                let _ = self.send_key_package_to(sender, false);
            }

            // Auto-establish the session now that we have the peer's key
            // package. This avoids waiting until the first send attempt.
            if self.config.encryption.auto_key_exchange && self.mls_manager.is_some() {
                match self.establish_secure_session(sender) {
                    Ok(Some(_)) => {
                        info!(sender = %sender, "Auto-established secure session after key package exchange");
                    }
                    Ok(None) => {
                        // Session already exists — nothing to do.
                    }
                    Err(e) => {
                        debug!(sender = %sender, error = %e, "Auto-establish deferred (session not ready yet)");
                    }
                }
            }
        }
    }

    /// Handles a session confirmation probe message.
    pub(crate) fn handle_session_confirm_probe(&mut self, sender: &str, _content: &str) {
        let sender_owned = sender.to_string();
        match self.has_mls_session(&sender_owned) {
            Ok(true) => {
                if !self.can_confirm_from_source(&sender_owned, "confirmation_probe_received") {
                    debug!(
                        sender = %sender_owned,
                        "Skipping probe confirmation until welcome send is at least attempted"
                    );
                } else {
                    match self.confirm_session_state(&sender_owned, "confirmation_probe_received") {
                        Ok(_) => {
                            let _ = self.flush_pending_messages(&sender_owned);
                            self.process_pending_decryption(&sender_owned);
                        }
                        Err(err) => {
                            warn!(
                                sender = %sender_owned,
                                error = %err,
                                "Failed to persist session confirmation after probe"
                            );
                        }
                    }
                }

                if let Err(err) = self.send_internal_message(
                    &sender_owned,
                    internal_prefixes::SESSION_CONFIRM_ACK.to_string(),
                    MessagePriority::High,
                ) {
                    warn!(
                        sender = %sender_owned,
                        error = %err,
                        "Failed to send session confirmation ack"
                    );
                }
            }
            Ok(false) => {
                debug!(
                    sender = %sender_owned,
                    "Ignoring confirmation probe without local MLS session"
                );
            }
            Err(err) => {
                warn!(
                    sender = %sender_owned,
                    error = %err,
                    "Failed to validate local MLS session for confirmation probe"
                );
            }
        }
    }

    /// Handles a session confirmation acknowledgment message.
    pub(crate) fn handle_session_confirm_ack(&mut self, sender: &str, _content: &str) {
        let sender_owned = sender.to_string();
        match self.has_mls_session(&sender_owned) {
            Ok(true) => {
                if !self.can_confirm_from_source(&sender_owned, "confirmation_ack_received") {
                    debug!(
                        sender = %sender_owned,
                        "Skipping ack confirmation until welcome send is at least attempted"
                    );
                } else {
                    match self.confirm_session_state(&sender_owned, "confirmation_ack_received") {
                        Ok(_) => {
                            let _ = self.flush_pending_messages(&sender_owned);
                            self.process_pending_decryption(&sender_owned);
                        }
                        Err(err) => {
                            warn!(
                                sender = %sender_owned,
                                error = %err,
                                "Failed to persist session confirmation after ack"
                            );
                        }
                    }
                }
            }
            Ok(false) => {
                debug!(
                    sender = %sender_owned,
                    "Ignoring confirmation ack without local MLS session"
                );
            }
            Err(err) => {
                warn!(
                    sender = %sender_owned,
                    error = %err,
                    "Failed to validate local MLS session for confirmation ack"
                );
            }
        }
    }

    /// Handles an MLS welcome message (session invitation).
    pub(crate) fn handle_welcome_message(&mut self, sender: &str, data: &str) {
        if let Ok(welcome) = serde_json::from_str::<WelcomeMessage>(data) {
            // Security: honest peers always set `inviter_id` to their own id
            // (see `SessionManager::create_session`), and the payload field is
            // used downstream as a storage key. Reject Welcomes whose claimed
            // inviter disagrees with the transport-level sender.
            if welcome.inviter_id != sender {
                error!(
                    sender = %sender,
                    inviter_id = %welcome.inviter_id,
                    "SECURITY: Welcome inviter_id does not match transport sender, dropping"
                );
                return;
            }
            debug!(sender = %sender, group_id = %welcome.group_id, "Received welcome message");

            // Track if we need to flush pending messages and process pending decryption
            let mut should_flush = false;
            // Owner side of a both-create race kept its own group; it must await a
            // group-aware decrypt before confirming (see below).
            let mut owner_keep = false;
            // Adopter side: on a Welcome RETRANSMIT for a group we already adopted,
            // re-send the encrypted confirm so a lost first confirm is retried in
            // lockstep with the owner's retransmission until it converges.
            let mut resend_confirm_on_retransmit = false;
            let sender_owned = sender.to_string();
            let group_id = welcome.group_id.as_str().to_string();
            let is_session = group_id.starts_with("session:");
            let mut error_reason: Option<String> = None;
            // Receiver-side convergence instrumentation: prove the Welcome
            // actually reassembled and reached MLS handling on THIS device.
            // (Absent in logs => the Welcome never fully arrived — a transport
            // problem; present => the problem is in adoption/confirm, not
            // transport.) Emitted before taking the MLS lock.
            let mut had_existing = false;
            // NB: keep `detail` free of raw identifiers — the telemetry scrubber
            // only hashes the named `peer_id` field, so anything embedded here
            // ships in cleartext. `group_id` is `session:<localId>:<peerId>`, so
            // it is deliberately omitted; the (hashed) peer_id already identifies
            // the pair and `is_session` is the only non-identifying bit of value.
            self.emit_event(Event::convergence_diag(
                "welcome_received".to_string(),
                sender_owned.clone(),
                format!("is_session={}", is_session),
            ));

            if let Some(mls) = self.mls_manager.clone() {
                if let Ok(manager) = mls.read() {
                    let has_existing = manager.has_session(sender).unwrap_or(false);
                    had_existing = has_existing;

                    if has_existing {
                        // Both sides created a session and exchanged Welcomes.
                        // Deterministic tiebreaker: the device whose user_id is
                        // lexicographically *greater* adopts the remote Welcome;
                        // the other keeps its own session.  This guarantees both
                        // devices converge on the same MLS group.
                        let local_id: &str = &self.config.user_id;
                        let remote_id: &str = sender;
                        if local_id > remote_id {
                            info!(
                                sender = %sender,
                                local_id = %local_id,
                                "Welcome-wins tiebreaker: adopting remote Welcome (local > remote)"
                            );
                            match manager.replace_session_with_welcome(&welcome) {
                                Ok(_) => {
                                    info!(sender = %sender, "Replaced session with remote Welcome");
                                    should_flush = true;
                                }
                                Err(e) => {
                                    // Non-destructive adopt: if our session survived, the
                                    // staging failure is a retransmitted Welcome we already
                                    // adopted (the one-time key package is consumed). It is a
                                    // harmless duplicate — drop it instead of erroring/bricking.
                                    if manager.has_session(sender).unwrap_or(false) {
                                        debug!(
                                            error = %e,
                                            sender = %sender,
                                            "Duplicate Welcome after adopt; already converged, ignoring"
                                        );
                                        // Owner is still retransmitting → it hasn't
                                        // decrypted our adoption proof yet. Re-send it.
                                        resend_confirm_on_retransmit = true;
                                    } else {
                                        warn!(error = %e, sender = %sender, "Failed to replace session");
                                        error_reason = Some(e.to_string());
                                    }
                                }
                            }
                        } else if self.welcome_lifecycles.contains_key(sender)
                            && !self.confirmed_sessions.contains(sender)
                        {
                            info!(
                                sender = %sender,
                                local_id = %local_id,
                                "Welcome-wins tiebreaker: keeping local session (local < remote); \
                                 awaiting group-aware decrypt before confirming our Welcome"
                            );
                            // Genuine both-create owner that is NOT yet converged: we
                            // created our OWN group for this peer AND sent a Welcome (hence
                            // an outbound welcome lifecycle exists) and have not yet
                            // confirmed. Keep our group, but do NOT confirm our outbound
                            // Welcome merely because we received the peer's — that is no
                            // proof they adopted ours. Keep retransmitting until a decrypt
                            // proves they adopted our group. The `!confirmed` guard is
                            // load-bearing: once converged, a late Welcome retransmit must
                            // NOT re-arm (and re-persist) `both_create_awaiting_decrypt` on
                            // an already-confirmed session — that stale, persisted gate
                            // would later block confirmation on a re-pair where this device
                            // adopts, stranding it. A converged owner falls through to the
                            // resend-confirm branch instead.
                            owner_keep = true;
                        } else {
                            // Either (a) the adopter: we have no outbound welcome lifecycle
                            // for this peer, so we never created our own group — we joined
                            // THEIR group via an earlier Welcome, and this is the owner
                            // re-sending its Welcome because it has not yet observed our
                            // confirm; or (b) an already-converged owner whose `!confirmed`
                            // guard above sent it here. In both cases re-send the encrypted
                            // confirm so a lost confirm self-heals in lockstep with the
                            // owner's retransmission. Critically, do NOT enter owner_keep:
                            // for the adopter, gating on decrypt here (plus poisoning
                            // both_create_awaiting_decrypt) would suppress our confirm and
                            // strand the owner in Pending forever — the convergence bug.
                            debug!(
                                sender = %sender,
                                local_id = %local_id,
                                "Welcome retransmit for an already-adopted/converged session; re-sending encrypted confirm"
                            );
                            resend_confirm_on_retransmit = true;
                        }
                    } else {
                        match manager.join_session(&welcome) {
                            Ok(_) => {
                                info!(sender = %sender, "Joined MLS session via Welcome");
                                should_flush = true;
                            }
                            Err(e) => {
                                warn!(error = %e, sender = %sender, "Failed to join MLS session");
                                error_reason = Some(e.to_string());
                            }
                        }
                    }
                }
            }

            // Owner side of a both-create race: record that this peer must prove
            // it adopted our group via a group-aware decrypt before we confirm
            // (and stop retransmitting). A plaintext probe/ack is not sufficient.
            if owner_keep {
                self.mark_both_create_awaiting_decrypt(&sender_owned);
            }

            // Receiver-side convergence instrumentation: which branch did this
            // device take, and did adoption succeed? Decisive for the owner
            // (iOS) side, which otherwise emits NOTHING on the owner_keep /
            // resend_confirm / no-mls paths. Emitted after the MLS lock release.
            {
                let branch = if should_flush {
                    "adopted"
                } else if owner_keep {
                    "owner_keep"
                } else if resend_confirm_on_retransmit {
                    "resend_confirm"
                } else if error_reason.is_some() {
                    "join_failed"
                } else {
                    "no_mls"
                };
                // `err_present` is a bool, not the raw error string: MLS error
                // text can carry group/peer identifiers and `detail` is not
                // scrubbed. The `join_failed` branch already flags the failure,
                // and the scrubbed `SecureSessionFailed` event carries the reason.
                self.emit_event(Event::convergence_diag(
                    "welcome_branch".to_string(),
                    sender_owned.clone(),
                    format!(
                        "had_existing={} branch={} err_present={}",
                        had_existing,
                        branch,
                        error_reason.is_some()
                    ),
                ));
            }

            // Confirm session and process queued items after releasing the MLS lock
            if should_flush {
                match self.confirm_session_state(&sender_owned, "welcome_received") {
                    Ok(_) => {
                        // Flush pending outgoing messages
                        let _ = self.flush_pending_messages(&sender_owned);

                        // Process any encrypted messages that arrived before the Welcome
                        self.process_pending_decryption(&sender_owned);

                        self.emit_mls_session_ready(
                            &sender_owned,
                            &group_id,
                            MlsOperationContext::Welcome,
                        );

                        // Send a fresh key package so the peer has one available
                        // for group invites (the original was consumed during
                        // session establishment on their side).
                        if self.config.encryption.enabled {
                            let _ = self.send_key_package_to(&sender_owned, false);
                        }

                        // Proactively prove to the peer that we adopted ITS group.
                        // The peer may be the both-create "owner", which confirms ONLY
                        // on a group-aware decrypt from us (a plaintext probe/ack is
                        // rejected by `can_confirm_from_source`); our session is
                        // confirmed locally just above, so we can encrypt now. Without
                        // this, a passive owner with no traffic to send never decrypts
                        // anything from us, stays Pending, and the 1:1 connection never
                        // completes. The marker is consumed on receipt (never shown).
                        if is_session && self.config.encryption.enabled {
                            self.send_session_confirm_encrypted(&sender_owned);
                        }

                        // Emit secure session established event
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::secure_session_established(
                                sender_owned.clone(),
                                group_id,
                                is_session,
                                false, // initiated_by_local is false - we received the Welcome
                            ));
                        }
                    }
                    Err(e) => {
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::secure_session_failed(
                                sender_owned.clone(),
                                format!("Failed to persist confirmation: {}", e),
                            ));
                        }
                    }
                }
            } else if let Some(reason) = error_reason {
                // Emit secure session failed event
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::secure_session_failed(sender_owned.clone(), reason));
                }
            }

            // Retransmit case: we already adopted the owner's group (session
            // confirmed earlier), but it is still retransmitting its Welcome
            // because it has not decrypted our adoption proof. Re-send the
            // encrypted confirm in lockstep so a lost confirm self-heals. (The
            // owner stops retransmitting — and we stop re-sending — once it
            // confirms via decrypt.)
            if resend_confirm_on_retransmit && is_session && self.config.encryption.enabled {
                self.send_session_confirm_encrypted(&sender_owned);
            }
        }
    }

    /// Parses an `__MLS_ENC__` payload in either envelope form.
    ///
    /// The legacy envelope is a JSON object; the compact envelope (negotiated
    /// via `env_versions` in the key package) is base64 of
    /// [`EncryptedMessage::to_bytes`]. base64's alphabet has no `{`, so the
    /// first byte disambiguates. Parsing is never capability-gated: whatever a
    /// peer chose to send, we try to read. The compact branch keeps a JSON
    /// fallback after decoding, covering any sender that base64-wrapped the
    /// JSON form (the `EncryptedMessage::from_base64` layout). Trying compact
    /// first is safe: base64-wrapped JSON starts with `{"`, whose bytes read
    /// as a group_id length far above `from_bytes`'s 4 KB cap, so it is
    /// rejected deterministically and falls through to the JSON parse.
    pub(super) fn parse_encrypted_payload(data: &str) -> Option<EncryptedMessage> {
        if data.starts_with('{') {
            return serde_json::from_str::<EncryptedMessage>(data).ok();
        }
        let bytes = base64_decode(data).ok()?;
        EncryptedMessage::from_bytes(&bytes)
            .ok()
            .or_else(|| serde_json::from_slice::<EncryptedMessage>(&bytes).ok())
    }

    /// Handles an encrypted MLS message, returning the decrypted result.
    pub(crate) fn handle_encrypted_message(
        &mut self,
        sender: &str,
        data: &str,
        message: &Message,
        arrival_transport: Option<TransportType>,
    ) -> Option<InternalMessageResult> {
        if let Some(encrypted) = Self::parse_encrypted_payload(data) {
            // Track state to update after releasing MLS lock
            enum DecryptResult {
                Success {
                    text: String,
                    sender: String,
                    group_id: String,
                },
                Empty,
                NonUtf8Plaintext,
                SessionNotReady {
                    sender: String,
                },
                SessionDesync {
                    sender: String,
                },
                Failed {
                    sender: String,
                    group_id: String,
                    kind: DecryptionFailureKind,
                },
                SecurityRejected,
                MlsNotInitialized,
            }

            let result = if let Some(mls) = self.mls_manager.clone() {
                if let Ok(manager) = mls.read() {
                    match manager.decrypt(&encrypted, sender) {
                        Ok(Some(plaintext)) => match String::from_utf8(plaintext) {
                            Ok(text) => {
                                debug!(sender = %sender, "Decrypted message successfully");
                                DecryptResult::Success {
                                    text,
                                    sender: sender.to_string(),
                                    group_id: encrypted.group_id.as_str().to_string(),
                                }
                            }
                            Err(_) => {
                                warn!(
                                    sender = %sender,
                                    "Decrypted payload is not valid UTF-8, rejecting"
                                );
                                DecryptResult::NonUtf8Plaintext
                            }
                        },
                        Ok(None) => {
                            warn!(sender = %sender, "Decryption returned empty");
                            DecryptResult::Empty
                        }
                        Err(offline_protocol_mls::MlsError::SenderIdentityMismatch {
                            claimed,
                            authenticated,
                        }) => {
                            error!(
                                sender = %sender,
                                claimed = %claimed,
                                authenticated = %authenticated,
                                "SECURITY: wire sender does not match MLS-authenticated sender, rejecting message"
                            );
                            DecryptResult::SecurityRejected
                        }
                        Err(e) => {
                            let session_state_error = SessionStateError::from(&e);
                            match session_state_error {
                                SessionStateError::SessionNotReady
                                | SessionStateError::GroupNotFound => {
                                    info!(
                                        sender = %sender,
                                        error_code = session_state_error.code(),
                                        "Encrypted message received before session ready, queuing"
                                    );
                                    debug!(
                                        sender = %sender,
                                        error = %e,
                                        error_code = session_state_error.code(),
                                        "Queued encrypted message due to session state classification"
                                    );
                                    DecryptResult::SessionNotReady {
                                        sender: sender.to_string(),
                                    }
                                }
                                SessionStateError::NotInitialized => {
                                    warn!(
                                        sender = %sender,
                                        error = %e,
                                        error_code = session_state_error.code(),
                                        "MLS decrypt attempted before initialization"
                                    );
                                    DecryptResult::MlsNotInitialized
                                }
                                SessionStateError::SessionDesync
                                    if self.config.encryption.crypto_recovery_enabled =>
                                {
                                    info!(
                                        sender = %sender,
                                        error_code = session_state_error.code(),
                                        "Encrypted message failed to decrypt due to epoch desync, re-keying"
                                    );
                                    DecryptResult::SessionDesync {
                                        sender: sender.to_string(),
                                    }
                                }
                                // Epoch desync with recovery disabled, or any
                                // genuine crypto/transport failure: fall through
                                // to the legacy drop-and-ACK behavior.
                                SessionStateError::SessionDesync
                                | SessionStateError::TransportFailure
                                | SessionStateError::CryptoFailure
                                | SessionStateError::Unknown => {
                                    let kind = DecryptionFailureKind::from_mls_error(&e);
                                    warn!(
                                        sender = %sender,
                                        error = %e,
                                        error_code = session_state_error.code(),
                                        "Failed to decrypt message"
                                    );
                                    DecryptResult::Failed {
                                        sender: sender.to_string(),
                                        group_id: encrypted.group_id.as_str().to_string(),
                                        kind,
                                    }
                                }
                            }
                        }
                    }
                } else {
                    DecryptResult::MlsNotInitialized
                }
            } else {
                DecryptResult::MlsNotInitialized
            };

            // Now handle the result without holding the MLS lock
            match result {
                DecryptResult::Success {
                    text,
                    sender: sender_owned,
                    group_id,
                } => {
                    // A proactive adopter confirm carries no user payload — its only
                    // job is to BE a group-aware decrypt so we (the owner) can confirm.
                    // Confirm as normal below, then consume it so it never surfaces as
                    // a chat message.
                    let is_session_confirm = text == internal_prefixes::SESSION_CONFIRM_ENCRYPTED;
                    // Convergence instrumentation: a group-aware decrypt landed.
                    // This is exactly what the both-create owner waits on to
                    // confirm its own Welcome — seeing it (with is_confirm=true)
                    // means the adopter's proof arrived and decrypted.
                    // `group_id` (= `session:<localId>:<peerId>`) is omitted: it
                    // would leak both ids in cleartext through the unscrubbed
                    // `detail`. The hashed `peer_id` already identifies the pair.
                    self.emit_event(Event::convergence_diag(
                        "decrypt_success".to_string(),
                        sender_owned.clone(),
                        format!("is_confirm={}", is_session_confirm),
                    ));
                    let surfaced = if is_session_confirm {
                        Some(InternalMessageResult::Consumed)
                    } else {
                        Some(InternalMessageResult::Decrypted(text))
                    };
                    self.confirm_session_from_successful_decrypt(&sender_owned, &group_id);
                    surfaced
                }
                DecryptResult::Empty => {
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::message_decryption_failed(
                            message.id.clone(),
                            sender.to_string(),
                            DecryptionFailureCode::InvalidCiphertext,
                            "Failed to decrypt MLS message (empty plaintext)".to_string(),
                        ));
                    }
                    Some(InternalMessageResult::Consumed)
                }
                DecryptResult::NonUtf8Plaintext => {
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::message_decryption_failed(
                            message.id.clone(),
                            sender.to_string(),
                            DecryptionFailureCode::InvalidPayload,
                            "Decrypted payload is not valid UTF-8".to_string(),
                        ));
                    }
                    Some(InternalMessageResult::Consumed)
                }
                DecryptResult::SessionNotReady {
                    sender: sender_owned,
                } => {
                    self.emit_mls_session_missing(
                        Some(&sender_owned),
                        Some(encrypted.group_id.as_str()),
                        MlsOperationContext::SessionLookup,
                        MlsErrorCategory::SessionStateMissing,
                    );
                    self.enqueue_pending_decryption_via(&sender_owned, message, arrival_transport);
                    // Deferred, NOT Consumed: the message is queued but not
                    // delivered, so the receive loop must skip the ACK and
                    // unmark the id — otherwise the sender counts it delivered
                    // and never retries, and a queue eviction becomes silent
                    // loss. The queued copy is surfaced — and the ACK sent on the
                    // recorded arrival transport — when the session confirms and
                    // `process_pending_decryption` drains it.
                    Some(InternalMessageResult::Deferred)
                }
                DecryptResult::SessionDesync {
                    sender: sender_owned,
                } => {
                    // The session exists but is out of epoch sync (the two sides
                    // diverged). Trigger a rate-limited re-key to heal the channel
                    // for future traffic, and return Deferred so the receive loop
                    // withholds the ACK and unmarks the id — the sender's ACK was
                    // a lie before (message dropped, sender told "delivered").
                    //
                    // We deliberately do NOT enqueue: unlike the not-yet-ready
                    // case, this ciphertext is sealed to the now-dead epoch and
                    // can never decrypt after the re-key, so queuing it would only
                    // waste memory until TTL. Recovery of *this* message depends
                    // on the sender re-sending it re-sealed against the rebuilt
                    // session (a sender-side change); until then the sender's
                    // retries surface an honest undeliverable instead of silent
                    // loss, which is strictly better than the lying ACK.
                    self.schedule_session_rekey(&sender_owned);
                    Some(InternalMessageResult::Deferred)
                }
                DecryptResult::Failed {
                    sender: sender_owned,
                    group_id,
                    kind,
                } => {
                    self.emit_mls_decryption_failed(
                        &sender_owned,
                        Some(&group_id),
                        kind,
                        MlsOperationContext::Receive,
                    );
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::message_decryption_failed(
                            message.id.clone(),
                            sender_owned.clone(),
                            Self::decryption_failure_code_from_kind(kind),
                            format!("Failed to decrypt MLS message ({kind:?})"),
                        ));
                    }
                    Some(InternalMessageResult::Consumed)
                }
                DecryptResult::SecurityRejected => {
                    // Spoofed sender: the MLS credential proves the message came
                    // from a different member than the wire envelope claims. Do
                    // not surface, do not ACK (handled by SecurityRejected).
                    Some(InternalMessageResult::SecurityRejected)
                }
                DecryptResult::MlsNotInitialized => {
                    self.emit_mls_decryption_failed(
                        sender,
                        Some(encrypted.group_id.as_str()),
                        DecryptionFailureKind::NotInitialized,
                        MlsOperationContext::Receive,
                    );
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::message_decryption_failed(
                            message.id.clone(),
                            sender.to_string(),
                            DecryptionFailureCode::NotInitialized,
                            "Failed to decrypt MLS message (not initialized)".to_string(),
                        ));
                    }
                    Some(InternalMessageResult::Consumed)
                }
            }
        } else {
            warn!(sender = %sender, "Invalid encrypted payload");
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::message_decryption_failed(
                    message.id.clone(),
                    sender.to_string(),
                    DecryptionFailureCode::InvalidPayload,
                    "Invalid encrypted payload".to_string(),
                ));
            }
            Some(InternalMessageResult::Consumed)
        }
    }

    /// Attempts to confirm the sender's session off the back of a successful
    /// group-aware decrypt (text or media chunk). No-op when confirmation is
    /// gated or already recorded.
    pub(super) fn confirm_session_from_successful_decrypt(&mut self, sender: &str, group_id: &str) {
        if !self.can_confirm_from_source(sender, "decrypt_success") {
            debug!(
                sender = %sender,
                "Skipping decrypt-based confirmation until welcome send is at least attempted"
            );
            return;
        }
        match self.confirm_session_state(sender, "decrypt_success") {
            Ok(true) => {
                info!(sender = %sender, "Session confirmed via successful decryption");
                // NOTE: we deliberately do NOT clear the re-key rate limit here.
                // A genuine re-fork and a replayed old-epoch frame are
                // indistinguishable at this layer, so resetting the floor on a
                // healed decrypt would let an attacker landing one legit decrypt
                // between replays force ~one teardown per inbound message. The
                // floor lapses on its own after REKEY_INTERVAL_SECS; see
                // `schedule_session_rekey`.
                let _ = self.flush_pending_messages(sender);
                // Drain any messages that were queued while the session was not
                // ready. Historically the pending-decryption queue was only
                // drained on explicit session-confirmation events (Welcome,
                // confirm probe/ack); a session that became usable purely via a
                // live decrypt — the both-create owner, or an in-band
                // `__MLS_ENC__` that decrypted first — left earlier queued
                // messages stranded until TTL eviction. Draining here makes any
                // successful decrypt a drain trigger.
                self.process_pending_decryption(sender);
                self.emit_mls_session_ready(sender, group_id, MlsOperationContext::Receive);

                // Surface the app-facing established event for the
                // both-create owner. `can_confirm_from_source` restricts a
                // `both_create_awaiting_decrypt` owner to confirm ONLY via a
                // group-aware decrypt (a plaintext probe/ack is rejected), so
                // this is its sole convergence path — and the one place it can
                // tell the app the session exists. `confirm_session_state`
                // deliberately skips emission for `decrypt_success` (it does
                // not know the group id), deferring to this call site exactly
                // as the Welcome-receive path above does for the adopter.
                // Without this the owner has a fully working 1:1 session (it
                // sends and receives) but the app never receives
                // `secure_session_established`, so UI gated on a known secure
                // session — e.g. the demo's group-creation contact list —
                // silently excludes the peer. Gate on `session:` so multi-party
                // group decrypts (which also reach this arm) are not reported as
                // 1:1 sessions. Reaching `Ok(true)` here implies we own the 1:1
                // group (an adopter would already be Confirmed via
                // `welcome_received` and return `Ok(false)`), so
                // `initiated_by_local` is true.
                if group_id.starts_with("session:") {
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::secure_session_established(
                            sender.to_string(),
                            group_id.to_string(),
                            true,
                            true,
                        ));
                    }
                }
            }
            Ok(false) => {}
            Err(e) => {
                warn!(
                    sender = %sender,
                    error = %e,
                    "Failed to persist session confirmation after decrypt"
                );
            }
        }
    }

    /// Handles a connection request message.
    pub(crate) fn handle_connection_request(&mut self, sender: &str, data: &str) {
        if let Ok(payload) = serde_json::from_str::<ConnectionRequestPayload>(data) {
            info!(sender = %sender, sender_name = %payload.sender_name, "Received connection request");
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::connection_request_received(
                    sender.to_string(),
                    payload.sender_name,
                    payload.timestamp_ms,
                    payload.key_package,
                    payload.initial_message,
                ));
            }
        } else {
            warn!(sender = %sender, "Failed to parse connection request payload");
        }
    }

    /// Handles a connection accepted message.
    pub(crate) fn handle_connection_accepted(&mut self, sender: &str, data: &str) {
        // The peer answered, so any request we still track toward them is
        // settled — drop it (keyed by recipient: the frame carries no
        // request id) so a later stale unreachable signal cannot fire a
        // false ConnectionRequestUndeliverable. Before the parse: the
        // authenticated frame itself is the proof, even if malformed.
        self.pending_connection_requests
            .retain(|_, p| p.recipient != sender);
        if let Ok(payload) = serde_json::from_str::<ConnectionAcceptedPayload>(data) {
            info!(sender = %sender, accepted_by_name = %payload.accepted_by_name, "Connection request accepted");
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::connection_accepted(
                    sender.to_string(),
                    payload.accepted_by_name,
                    payload.timestamp_ms,
                    payload.key_package,
                ));
            }
        } else {
            warn!(sender = %sender, "Failed to parse connection accepted payload");
        }
    }

    /// Handles a connection rejected message.
    pub(crate) fn handle_connection_rejected(&mut self, sender: &str) {
        // Same settlement rule as handle_connection_accepted: the peer
        // answered, so the tracked request toward them is resolved.
        self.pending_connection_requests
            .retain(|_, p| p.recipient != sender);
        info!(sender = %sender, "Connection request rejected");
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::connection_rejected(sender.to_string()));
        }
    }

    /// Handles a connection cancelled message.
    pub(crate) fn handle_connection_cancelled(&mut self, sender: &str) {
        info!(sender = %sender, "Connection request cancelled");
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::connection_request_cancelled(sender.to_string()));
        }
    }

    /// Handles a presence update message.
    pub(crate) fn handle_presence_message(&mut self, sender: &str, data: &str) {
        if let Ok(payload) = serde_json::from_str::<PresencePayload>(data) {
            if payload.timestamp_ms < 0 {
                warn!("Dropping presence update with negative timestamp");
            } else {
                debug!(sender = %sender, status = ?payload.status, "Received presence update");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::presence_updated(
                        sender.to_string(),
                        payload.status,
                        payload.timestamp_ms,
                    ));
                }
            }
        } else {
            warn!("Failed to parse Presence payload");
        }
    }

    /// Handles a typing indicator message.
    pub(crate) fn handle_typing_indicator(&mut self, sender: &str, data: &str) {
        if let Ok(payload) = serde_json::from_str::<TypingIndicatorPayload>(data) {
            if payload.timestamp_ms < 0 {
                warn!("Dropping typing indicator with negative timestamp");
            } else if payload.conversation_id.is_empty() {
                warn!("Dropping typing indicator with empty conversation_id");
            } else {
                debug!(sender = %sender, is_typing = %payload.is_typing, "Received typing indicator");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::typing_indicator_received(
                        sender.to_string(),
                        payload.conversation_id,
                        payload.is_typing,
                        payload.timestamp_ms,
                    ));
                }
            }
        } else {
            warn!("Failed to parse TypingIndicator payload");
        }
    }

    /// Handles a read receipt message.
    pub(crate) fn handle_read_receipt(&mut self, sender: &str, data: &str) {
        if let Ok(payload) = serde_json::from_str::<ReadReceiptPayload>(data) {
            if payload.timestamp_ms < 0 {
                warn!("Dropping read receipt with negative timestamp");
            } else if payload.message_ids.is_empty() {
                warn!("Dropping read receipt with empty message_ids");
            } else if payload.message_ids.len() > MAX_READ_RECEIPT_IDS {
                warn!(
                    count = payload.message_ids.len(),
                    "Dropping read receipt exceeding max message_ids"
                );
            } else {
                debug!(sender = %sender, count = %payload.message_ids.len(), "Received read receipt");
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::read_receipt_received(
                        sender.to_string(),
                        payload.message_ids,
                        payload.timestamp_ms,
                    ));
                }
            }
        } else {
            warn!("Failed to parse ReadReceipt payload");
        }
    }

    /// Handles group relay messages (GROUP_CREATED through GROUP_ERROR).
    ///
    /// `arrival_transport` is the transport the frame was received on, when
    /// known. Relay-server answers are only trusted from the Internet path.
    pub(crate) fn handle_group_relay_message(
        &mut self,
        sender: &str,
        content: &str,
        arrival_transport: Option<TransportType>,
    ) {
        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_CREATED) {
            if let Ok(payload) = serde_json::from_str::<GroupCreatedPayload>(data) {
                info!(group_id = %payload.group_id, "Group created");
                // The relay only answers GroupCreated on the connection that
                // sent CreateGroup, so for a locally-tracked group this is the
                // positive registration acknowledgment that `relay_synced`
                // requires (enqueueing the registration frame proves nothing —
                // see try_relay_register_group). Only now may
                // send_group_message take the O(1) relay-broadcast path.
                //
                // The ack must also have arrived over the Internet transport
                // AND answer a registration we actually sent: any mesh peer
                // can craft a `__GROUP_CREATED__` frame, and the relay
                // forwards peer message content verbatim, so an internet
                // peer can too. A spoofed sync flag would route broadcasts
                // into a relay that never registered the group —
                // unrecoverable content loss on a store-less relay. The
                // pending-registration correlation narrows acceptance to the
                // window where the relay's genuine answer is due. (The
                // group_created app event below is unchanged: spoofing it is
                // cosmetic, gating it would hide legitimate relay answers
                // from apps on unusual topologies.)
                if arrival_transport == Some(TransportType::Internet)
                    && self.group_mesh.members.contains_key(&payload.group_id)
                    && self
                        .group_mesh
                        .relay_register_pending
                        .remove(&payload.group_id)
                        .is_some()
                {
                    self.group_mesh
                        .relay_synced
                        .insert(payload.group_id.clone());
                    // Emitted on the pending-consumed transition, NOT on the
                    // set insert: an idempotent re-registration ack (post
                    // membership change) finds the group already in
                    // `relay_synced`, and apps awaiting that re-sync ack
                    // (`ensure_group_registered`) must still hear it.
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::group_relay_sync_changed(
                            payload.group_id.clone(),
                            true,
                            "registered",
                        ));
                    }
                }
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_created(payload.group_id, payload.name));
                }
            } else {
                warn!("Failed to parse GroupCreated payload");
            }
            return;
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MSG) {
            if let Ok(payload) = serde_json::from_str::<GroupMessageReceivedPayload>(data) {
                info!(group_id = %payload.group_id, message_id = %payload.message_id, "Group message received");
                // Route through MLS decryption whenever MLS is available —
                // not just when we already have state for the group. A relay
                // group message can outrun its Welcome (we are a member on
                // the relay before the join lands locally), and gating on
                // the members cache would emit its ciphertext raw instead of
                // buffering it for the post-join drain. The MLS path itself
                // falls back to a raw emit for legacy non-MLS content.
                if self.group_mesh.members.contains_key(&payload.group_id)
                    || self.is_mls_initialized()
                {
                    self.handle_relay_group_message_with_mls(
                        &payload.group_id,
                        &payload.sender,
                        &payload.content,
                        &payload.timestamp,
                        &payload.message_id,
                        payload.reply_to_msg,
                        payload.forward_info,
                    );
                } else {
                    // Legacy relay-only group — emit raw content
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::group_message_received(
                            payload.group_id,
                            payload.sender,
                            payload.content,
                            payload.timestamp,
                            payload.message_id,
                            payload.reply_to_msg,
                            None,
                            None,
                            None,
                        ));
                    }
                }
            } else {
                warn!("Failed to parse GroupMessageReceived payload");
            }
            return;
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MEMBER_ADDED) {
            if let Ok(payload) = serde_json::from_str::<GroupMemberAddedPayload>(data) {
                // SECURITY: `__GROUP_MEMBER_ADDED__` is a relay reconciliation
                // frame — the mobile bindings inject it from a relay
                // notification that arrives over the Internet transport. It has
                // no in-SDK mesh producer (a real MLS add is surfaced from the
                // authenticated roster by `refresh_group_members`, not here).
                // The mutation below splices `payload.user_id` into
                // `group_mesh.members`, the group fan-out send cache that
                // `send_group_message_inner` reads verbatim, so an accepted
                // forgery makes us deliver every subsequent group MLS ciphertext
                // to an attacker-chosen id (a silent recipient — they cannot
                // decrypt, but it leaks membership/activity metadata) and forges
                // a roster event. Gate on Internet arrival exactly like
                // `__GROUP_CREATED__` above: a BLE/WiFi-mesh attacker can never
                // present `arrival_transport == Internet`, which drops the
                // forgery while preserving legitimate relay reconciliation.
                //
                // Residual (accepted): this does NOT authenticate a malicious
                // *Internet* peer who can address us through the store-and-
                // forward relay (which forwards peer content verbatim). We do
                // not additionally gate on the wire `sender` being an admin —
                // as the sibling `__GROUP_MEMBER_REMOVED__` path does — because
                // this frame has no signed sender to check: the mobile bindings
                // synthesize it from a relay notification with `sender =
                // added_by`, falling back to the literal `"relay"` when the
                // notification omits `added_by` (see InternetManager.{swift,kt}),
                // so an admin check would drop those legitimate reconciliations.
                // The residual is bounded: a spliced phantom cannot decrypt
                // (it is never in the MLS group), so it leaks only membership/
                // activity metadata, the `members.get_mut` guard below limits
                // the splice to groups we already track, and crypto membership
                // stays MLS-authoritative via `refresh_group_members`.
                if arrival_transport != Some(TransportType::Internet) {
                    warn!(
                        group_id = %payload.group_id,
                        user_id = %payload.user_id,
                        sender = %sender,
                        "SECURITY: dropping __GROUP_MEMBER_ADDED__ not delivered over the Internet relay path (mesh forgery)"
                    );
                    return;
                }
                info!(group_id = %payload.group_id, user_id = %payload.user_id, "Group member added");
                // Reconcile local member cache if we have MLS state for this group
                if let Some(members) = self.group_mesh.members.get_mut(&payload.group_id) {
                    if !members.contains(&payload.user_id) {
                        members.push(payload.user_id.clone());
                    }
                }
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_member_added(
                        payload.group_id,
                        payload.user_id,
                        payload.added_by,
                        payload.group_name,
                        // Relay reconciliation frame, gated on Internet arrival
                        // above; not an MLS commit.
                        true,
                    ));
                }
            } else {
                warn!("Failed to parse GroupMemberAdded payload");
            }
            return;
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_MEMBER_REMOVED) {
            if let Ok(payload) = serde_json::from_str::<GroupMemberRemovedPayload>(data) {
                info!(group_id = %payload.group_id, user_id = %payload.user_id, "Group member removed");

                // If WE are the removed member, clean up local MLS group state
                // so we don't retain a stale group that can't encrypt/decrypt.
                let self_removed = payload.user_id == self.config.user_id;
                if self_removed {
                    // SECURITY (HIGH-2): authorize this destructive local
                    // teardown off the authenticated wire `sender`, never the
                    // attacker-controlled `payload.removed_by`. A legitimate
                    // removal notification is sent directly by the removing admin
                    // (`remove_member` → `send_internal_message(member_id, …)`),
                    // so its `sender` IS that admin. Requiring the sender to be
                    // an admin matches every sibling membership handler (role
                    // change, rename, commit) and forces a forger to impersonate
                    // an admin identity — which the control-frame signature +
                    // TOFU gate rejects for any pinned admin. The prior
                    // `removed_by` fallback authenticated nothing: `removed_by`
                    // is an unauthenticated payload field, so naming any real
                    // admin passed the check and let an unpinned non-member
                    // force-evict the victim (dropping local MLS group state).
                    match self.check_is_admin(&payload.group_id, sender) {
                        Ok(true) => {}
                        Ok(false) => {
                            error!(
                                sender = %sender,
                                group_id = %payload.group_id,
                                "SECURITY: Group removal notification from non-admin sender, ignoring"
                            );
                            return;
                        }
                        Err(e) => {
                            warn!(
                                sender = %sender,
                                group_id = %payload.group_id,
                                error = %e,
                                "Failed to verify admin status for removal notification"
                            );
                            return;
                        }
                    }
                    info!(
                        group_id = %payload.group_id,
                        "We were removed from the group — cleaning up local state"
                    );
                    if let Some(mls) = self.mls_manager.clone() {
                        if let (Ok(mls_guard), Ok(gid)) = (
                            mls.read(),
                            offline_protocol_mls::GroupId::new(&payload.group_id),
                        ) {
                            if let Err(e) = mls_guard.leave_group(&gid) {
                                debug!(
                                    group_id = %payload.group_id,
                                    error = %e,
                                    "MLS leave_group cleanup after removal (may already be gone)"
                                );
                            }
                        }
                    }
                    self.group_mesh.members.remove(&payload.group_id);
                    let was_synced = self.group_mesh.relay_synced.remove(&payload.group_id);
                    // The outstanding-registration correlation goes too, as
                    // in every other membership-teardown path: a pending
                    // entry surviving our removal could otherwise be claimed
                    // by a stale or forged __GROUP_CREATED__ after a re-join
                    // repopulates the member cache.
                    let was_pending = self
                        .group_mesh
                        .relay_register_pending
                        .remove(&payload.group_id)
                        .is_some();
                    if was_synced || was_pending {
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::group_relay_sync_changed(
                                payload.group_id.clone(),
                                false,
                                "removed",
                            ));
                        }
                    }
                } else {
                    // Another member was removed. This mutates the group
                    // fan-out send cache (`group_mesh.members`), which
                    // `send_group_message_inner` reads verbatim — dropping a
                    // real member here silently denies them our group messages
                    // until the next commit's `refresh_group_members`. So
                    // authorize it off the authenticated wire `sender`, never
                    // the payload-named `removed_by`, exactly like the
                    // self-removal branch above and every sibling membership
                    // handler. `removed_by` is unauthenticated payload content;
                    // naming a real admin in it is free. The SDK only sends
                    // `__GROUP_MEMBER_REMOVED__` directly to the removed member
                    // (see `remove_from_group`), so a frame naming a third
                    // party is an admin's relay reconciliation or a forgery —
                    // requiring the sender to be an admin drops the forgeries.
                    match self.check_is_admin(&payload.group_id, sender) {
                        Ok(true) => {
                            if let Some(members) =
                                self.group_mesh.members.get_mut(&payload.group_id)
                            {
                                members.retain(|m| m != &payload.user_id);
                            }
                        }
                        Ok(false) => {
                            error!(
                                sender = %sender,
                                group_id = %payload.group_id,
                                removed = %payload.user_id,
                                "SECURITY: Group member-removal from non-admin sender, ignoring"
                            );
                            return;
                        }
                        Err(e) => {
                            warn!(
                                sender = %sender,
                                group_id = %payload.group_id,
                                error = %e,
                                "Failed to verify admin status for member-removal notification"
                            );
                            return;
                        }
                    }
                }

                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_member_removed(
                        payload.group_id,
                        payload.user_id,
                        payload.removed_by,
                        // Reached only through the admin gate above; not an
                        // MLS commit.
                        true,
                    ));
                }
            } else {
                warn!("Failed to parse GroupMemberRemoved payload");
            }
            return;
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_INFO) {
            if let Ok(payload) = serde_json::from_str::<GroupInfoPayload>(data) {
                info!(group_id = %payload.group_id, "Group info received");
                let members: Vec<crate::events::GroupInfoMember> = payload
                    .members
                    .into_iter()
                    .map(|m| crate::events::GroupInfoMember {
                        user_id: m.user_id,
                        role: m.role,
                        joined_at: m.joined_at,
                    })
                    .collect();
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_info(
                        payload.group_id,
                        payload.name,
                        payload.created_by,
                        payload.created_at,
                        members,
                    ));
                }
            } else {
                warn!("Failed to parse GroupInfo payload");
            }
            return;
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::USER_GROUPS) {
            if let Ok(payload) = serde_json::from_str::<UserGroupsPayload>(data) {
                info!(count = payload.groups.len(), "User groups received");
                let groups: Vec<crate::events::UserGroupSummary> = payload
                    .groups
                    .into_iter()
                    .map(|g| crate::events::UserGroupSummary {
                        group_id: g.group_id,
                        name: g.name,
                        created_at: g.created_at,
                    })
                    .collect();
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::user_groups(groups));
                }
            } else {
                warn!("Failed to parse UserGroups payload");
            }
            return;
        }

        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_ERROR) {
            if let Ok(payload) = serde_json::from_str::<GroupErrorPayload>(data) {
                warn!(reason = %payload.reason, group_id = ?payload.group_id, "Group error");
                // A group-scoped relay error (registration denied, not a
                // member, ...) means relay-side fan-out cannot be trusted for
                // this group: drop the sync flag so sends fall back to the
                // always-correct per-member path. Deliberately NOT gated on
                // the Internet transport (unlike the GROUP_CREATED ack): a
                // mesh-spoofed revocation only downgrades performance, while
                // a real one missed on an unusual arrival path would keep
                // routing content into a relay that disowned the group.
                if let Some(group_id) = &payload.group_id {
                    let was_synced = self.group_mesh.relay_synced.remove(group_id);
                    // The denial also answers any outstanding registration:
                    // drop the pending correlation so a later forged
                    // `__GROUP_CREATED__` cannot claim it.
                    let was_pending = self
                        .group_mesh
                        .relay_register_pending
                        .remove(group_id)
                        .is_some();
                    // Only a revocation of state we actually tracked is a
                    // sync change — a GroupError about a group the relay was
                    // never asked to register is app-plane noise.
                    if was_synced || was_pending {
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::group_relay_sync_changed(
                                group_id.clone(),
                                false,
                                "error",
                            ));
                        }
                    }
                }
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_error(payload.reason));
                }
            } else {
                warn!("Failed to parse GroupError payload");
            }
        }
    }

    /// Handles service discovery and request/response messages.
    pub(crate) fn handle_service_message(
        &mut self,
        sender: &str,
        content: &str,
        message: &Message,
    ) {
        let peers: Vec<String> = self
            .known_peers
            .keys()
            .filter(|p| p.as_str() != sender)
            .cloned()
            .collect();
        match self.mesh_services.handle_incoming_message(
            content,
            sender,
            message.hop_count.value(),
            &self.config.user_id,
            &peers,
        ) {
            ServiceAction::NotHandled => {
                warn!(sender = %sender, "Received unknown service message prefix, consuming");
            }
            ServiceAction::Consumed {
                messages_to_send,
                events_to_emit,
            } => {
                for msg in messages_to_send {
                    let _ = self.send_internal_message(&msg.recipient, msg.content, msg.priority);
                }
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    for svc_event in events_to_emit {
                        state.emit_event(Event::from(svc_event));
                    }
                }
            }
        }
    }
}
