//! Message dispatch handlers for internal protocol messages.

use super::{
    internal_prefixes, lock_shared_state, ConnectionAcceptedPayload, ConnectionRequestPayload,
    GroupCreatedPayload, GroupErrorPayload, GroupInfoPayload, GroupMemberAddedPayload,
    GroupMemberRemovedPayload, GroupMessageReceivedPayload, InternalMessageResult,
    KeyPackagePayload, OfflineProtocol, PresencePayload, ReadReceiptPayload, ReceivedKeyPackage,
    TypingIndicatorPayload, UserGroupsPayload, MAX_READ_RECEIPT_IDS,
};
use crate::events::{DecryptionFailureCode, Event};
use crate::mls_observability::{DecryptionFailureKind, MlsErrorCategory, MlsOperationContext};
use crate::SessionStateError;
use chrono::Utc;
use offline_protocol_core::{Message, MessagePriority};
use offline_protocol_mls::{EncryptedMessage, WelcomeMessage};
use offline_protocol_services::ServiceAction;
use tracing::{debug, error, info, warn};

impl OfflineProtocol {
    /// Handles an incoming MLS key package message.
    pub(crate) fn handle_key_package_message(&mut self, sender: &str, data: &str) {
        if let Ok(payload) = serde_json::from_str::<KeyPackagePayload>(data) {
            debug!(sender = %sender, session_reset = %payload.session_reset, "Received key package");

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
                            if self.pending_encrypted_messages.remove(sender).is_some() {
                                self.clear_pending_messages_from_storage(sender);
                            }
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
            let local_expires_at_ms = if payload.remaining_lifetime_ms > 0 {
                now_ms.saturating_add(payload.remaining_lifetime_ms)
            } else {
                // Legacy sender didn't include remaining_lifetime_ms;
                // assume 30-day default lifetime.
                now_ms.saturating_add(30 * 24 * 60 * 60 * 1000)
            };
            let pkg = ReceivedKeyPackage {
                key_package_data: payload.key_package_data,
                local_expires_at_ms,
            };
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

            if let Some(mls) = self.mls_manager.clone() {
                if let Ok(manager) = mls.read() {
                    let has_existing = manager.has_session(sender).unwrap_or(false);

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
                        } else {
                            info!(
                                sender = %sender,
                                local_id = %local_id,
                                "Welcome-wins tiebreaker: keeping local session (local < remote); \
                                 awaiting group-aware decrypt before confirming our Welcome"
                            );
                            // Owner side of a both-create race: keep our own group, but do NOT
                            // confirm our outbound Welcome merely because we received the peer's.
                            // Receiving their Welcome is no proof they received ours, so confirming
                            // here would mark our Welcome Sent and stop retransmission — the
                            // convergence bug. Keep retransmitting until a decrypt proves they
                            // adopted our group.
                            owner_keep = true;
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
                self.both_create_awaiting_decrypt.insert(sender_owned.clone());
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

    /// Handles an encrypted MLS message, returning the decrypted result.
    pub(crate) fn handle_encrypted_message(
        &mut self,
        sender: &str,
        data: &str,
        message: &Message,
    ) -> Option<InternalMessageResult> {
        if let Ok(encrypted) = serde_json::from_str::<EncryptedMessage>(data) {
            // Track state to update after releasing MLS lock
            enum DecryptResult {
                Success {
                    text: String,
                    sender: String,
                    group_id: String,
                },
                Empty,
                SessionNotReady {
                    sender: String,
                },
                Failed {
                    sender: String,
                    group_id: String,
                    kind: DecryptionFailureKind,
                },
                MlsNotInitialized,
            }

            let result = if let Some(mls) = self.mls_manager.clone() {
                if let Ok(manager) = mls.read() {
                    match manager.decrypt(&encrypted) {
                        Ok(Some(plaintext)) => {
                            let text = String::from_utf8_lossy(&plaintext).to_string();
                            debug!(sender = %sender, "Decrypted message successfully");
                            DecryptResult::Success {
                                text,
                                sender: sender.to_string(),
                                group_id: encrypted.group_id.as_str().to_string(),
                            }
                        }
                        Ok(None) => {
                            warn!(sender = %sender, "Decryption returned empty");
                            DecryptResult::Empty
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
                                SessionStateError::TransportFailure
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
                    let surfaced = if is_session_confirm {
                        Some(InternalMessageResult::Consumed)
                    } else {
                        Some(InternalMessageResult::Decrypted(text))
                    };
                    if !self.can_confirm_from_source(&sender_owned, "decrypt_success") {
                        debug!(
                            sender = %sender_owned,
                            "Skipping decrypt-based confirmation until welcome send is at least attempted"
                        );
                        surfaced
                    } else {
                        match self.confirm_session_state(&sender_owned, "decrypt_success") {
                            Ok(true) => {
                                info!(sender = %sender_owned, "Session confirmed via successful decryption");
                                let _ = self.flush_pending_messages(&sender_owned);
                                self.emit_mls_session_ready(
                                    &sender_owned,
                                    &group_id,
                                    MlsOperationContext::Receive,
                                );
                            }
                            Ok(false) => {}
                            Err(e) => {
                                warn!(
                                    sender = %sender_owned,
                                    error = %e,
                                    "Failed to persist session confirmation after decrypt"
                                );
                            }
                        }
                        surfaced
                    }
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
                DecryptResult::SessionNotReady {
                    sender: sender_owned,
                } => {
                    self.emit_mls_session_missing(
                        Some(&sender_owned),
                        Some(encrypted.group_id.as_str()),
                        MlsOperationContext::SessionLookup,
                        MlsErrorCategory::SessionStateMissing,
                    );
                    self.enqueue_pending_decryption(&sender_owned, message);
                    Some(InternalMessageResult::Consumed)
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
                ));
            }
        } else {
            warn!(sender = %sender, "Failed to parse connection request payload");
        }
    }

    /// Handles a connection accepted message.
    pub(crate) fn handle_connection_accepted(&mut self, sender: &str, data: &str) {
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
    pub(crate) fn handle_group_relay_message(&mut self, sender: &str, content: &str) {
        if let Some(data) = content.strip_prefix(internal_prefixes::GROUP_CREATED) {
            if let Ok(payload) = serde_json::from_str::<GroupCreatedPayload>(data) {
                info!(group_id = %payload.group_id, "Group created");
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
                // If we have local MLS state for this group, route through MLS decryption
                if self.group_mesh.members.contains_key(&payload.group_id) {
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
                    // SECURITY: Verify the sender is authorized to remove us.
                    // If the sender is a known group member, they must be an admin.
                    // If the sender is not a member (e.g. relay server), verify
                    // that removed_by in the payload is a known admin to prevent
                    // arbitrary non-member senders from forging removals.
                    let sender_is_member = self
                        .group_mesh
                        .members
                        .get(&payload.group_id)
                        .map(|m| m.contains(&sender.to_string()))
                        .unwrap_or(false);
                    let admin_to_verify = if sender_is_member {
                        sender
                    } else {
                        &payload.removed_by
                    };
                    match self.check_is_admin(&payload.group_id, admin_to_verify) {
                        Ok(true) => {}
                        Ok(false) => {
                            error!(
                                sender = %sender,
                                verified_id = %admin_to_verify,
                                group_id = %payload.group_id,
                                "SECURITY: Group removal notification with unverifiable admin, ignoring"
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
                        if let Ok(mls_guard) = mls.read() {
                            let gid = offline_protocol_mls::GroupId::new(&payload.group_id);
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
                    self.group_mesh.relay_synced.remove(&payload.group_id);
                } else {
                    // Another member was removed — just update the cache
                    if let Some(members) = self.group_mesh.members.get_mut(&payload.group_id) {
                        members.retain(|m| m != &payload.user_id);
                    }
                }

                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::group_member_removed(
                        payload.group_id,
                        payload.user_id,
                        payload.removed_by,
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
                warn!(reason = %payload.reason, "Group error");
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
            .iter()
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
