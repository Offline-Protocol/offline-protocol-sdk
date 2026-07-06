//! Message receive loop and file chunk handling.

use super::{
    lock_shared_state, InternalMessageResult, OfflineProtocol, PendingMediaMetadataEntry,
    ProtocolState,
};
use crate::constants::{ACK_FOR_KEY, RELAY_LEARNED_ROUTE_QUALITY};
use crate::events::{Event, SecurityWarningCode};
use crate::file_transfer::FileChunk;
use crate::media_envelope::{decode_media_envelope, is_media_envelope, MediaChunkPlaintext};
use crate::mls_observability::{DecryptionFailureKind, MlsErrorCategory, MlsOperationContext};
use crate::SessionStateError;
use offline_protocol_core::{ContentType, MediaMetadata, Message};
use offline_protocol_mls::{EncryptedMessage, GroupId};
use offline_protocol_router::relay::RelayPriority;
use std::time::Instant;
use tracing::{debug, error, info, warn};

impl OfflineProtocol {
    /// Receives the next available message.
    pub fn receive_message(&mut self) -> Option<Message> {
        let Ok(mut state) = lock_shared_state(&self.shared_state) else {
            error!("Failed to lock shared state in receive_message");
            return None;
        };
        let protocol_running = state.state == ProtocolState::Running;

        if !state.received_messages.is_empty() {
            return state.received_messages.pop_front();
        }

        drop(state);

        // Drive confirmation maintenance from receive polling as an additional
        // liveness source when the app does not call process() on a timer.
        // Uses the same throttle as process() to avoid redundant storage I/O.
        if protocol_running {
            self.run_throttled_reconciliation("receive_message_poll");
        }

        loop {
            match self.transport_manager.receive() {
                Ok(Some((transport_used, mut message))) => {
                    // Block filter (early): check before any side-effects so
                    // that blocked users cannot advance our Lamport clock,
                    // leak our presence via re-ACK, or trigger any processing.
                    // Relay messages for third parties pass through.
                    let sender_blocked = self.is_user_blocked(message.sender.as_str())
                        && message.recipient.as_str() == self.config.user_id;

                    // Merge Lamport clock for every non-blocked received message
                    // — including duplicates, ACKs, and internal protocol
                    // messages — so the local clock always advances past any
                    // observed peer value.
                    if !sender_blocked && message.lamport_clock.value() > 0 {
                        self.lamport_clock.merge(message.lamport_clock);
                        self.persist_lamport_clock();
                    }

                    if message.metadata.contains_key(ACK_FOR_KEY) {
                        if !sender_blocked {
                            self.handle_ack_message(&message);
                        }
                        continue;
                    }

                    if self.deduplicator.is_duplicate_mut(&message.id) {
                        // Re-ACK duplicate packets so the sender can stop
                        // retrying — but NOT for blocked users, to avoid
                        // leaking presence information.
                        if !sender_blocked && message.requires_ack {
                            if let Err(err) = self.send_delivery_ack(&message, transport_used) {
                                error!(
                                    message_id = %message.id,
                                    error = %err,
                                    "Failed to send delivery ACK for duplicate message"
                                );
                            }
                        }
                        continue;
                    }

                    // Block filter: silently drop messages from blocked users
                    // addressed to us. No ACK, no event, no side-effects.
                    // Checked before mark_seen so that if the user is later
                    // unblocked, retransmissions can still be delivered.
                    if sender_blocked {
                        debug!(
                            sender = %message.sender,
                            message_id = %message.id,
                            "Dropping message from blocked user"
                        );
                        continue;
                    }

                    self.deduplicator.mark_seen(message.id.clone());

                    // Relay: if this message is not for us, forward it
                    if message.recipient.as_str() != self.config.user_id {
                        self.try_relay_message(&message);
                        continue;
                    }

                    // Handle internal MLS messages
                    if let Some(result) = self.process_internal_message(&message) {
                        match result {
                            InternalMessageResult::Consumed => {
                                // Internal control messages are still delivery-sensitive for
                                // the sender (invites/accept/welcome). ACK before consume.
                                if message.requires_ack {
                                    if let Err(err) =
                                        self.send_delivery_ack(&message, transport_used)
                                    {
                                        error!(
                                            message_id = %message.id,
                                            error = %err,
                                            "Failed to send delivery ACK for internal message"
                                        );
                                    }
                                }
                                // Internal message handled, don't surface to app
                                continue;
                            }
                            InternalMessageResult::SecurityRejected => {
                                // Security gate rejected this message (spoofed sender,
                                // bad signature, TOFU violation). Do NOT send a delivery
                                // ACK — acknowledging would confirm to the attacker that
                                // the target peer is online and processing messages.
                                continue;
                            }
                            InternalMessageResult::Decrypted(plaintext) => {
                                // Replace content with decrypted plaintext
                                message.content = plaintext;
                                message
                                    .metadata
                                    .insert("encrypted".to_string(), "true".to_string());
                            }
                        }
                    }

                    if message.requires_ack {
                        if let Err(err) = self.send_delivery_ack(&message, transport_used) {
                            error!(
                                message_id = %message.id,
                                error = %err,
                                "Failed to send delivery ACK"
                            );
                        }
                    }

                    // Route file-chunk messages to the transfer manager instead
                    // of surfacing them to the app as regular messages.
                    if message.content_type == ContentType::FileChunk {
                        self.handle_incoming_file_chunk(&message);
                        continue;
                    }

                    let forward_info = message
                        .forwarded_from
                        .as_ref()
                        .map(crate::events::ForwardInfoEvent::from);

                    let event = Event::MessageReceived {
                        message_id: message.id.as_str(),
                        sender: message.sender.as_str().to_string(),
                        recipient: message.recipient.as_str().to_string(),
                        content: message.content.clone(),
                        hop_count: message.hop_count.value(),
                        transport: transport_used.to_string(),
                        timestamp: message.timestamp.as_millis(),
                        lamport_clock: message.lamport_clock.value(),
                        reply_to_msg: message
                            .reply_to_msg
                            .as_ref()
                            .map(|id| id.as_str().to_string()),
                        content_type: message.content_type.to_string(),
                        media_metadata: message.media_metadata.clone(),
                        forward_info,
                    };

                    let Ok(state) = lock_shared_state(&self.shared_state) else {
                        error!("Failed to lock shared state for message received event");
                        return None;
                    };
                    state.emit_event(event);
                    drop(state);

                    return Some(message);
                }
                Ok(None) => return None,
                Err(err) => {
                    error!(error = %err, "Transport receive error");
                    return None;
                }
            }
        }
    }

    /// Attempts to relay (forward) a message destined for a third party.
    ///
    /// Learns a route back to the sender, checks relay configuration and TTL,
    /// then forwards the message with hop count incremented and TTL decremented.
    /// Emits a `MessageRelayed` event on success.
    fn try_relay_message(&mut self, message: &Message) {
        // Learn route back to sender through whoever gave us this message.
        // Note: in multi-hop scenarios this records sender as both destination
        // and next_hop, which is correct for 1-hop but conservative for deeper
        // chains (the transport layer does not expose the immediate peer ID).
        self.path_selector.learn_route_from_message(
            message,
            message.sender.as_str(),
            RELAY_LEARNED_ROUTE_QUALITY,
        );

        let relay_allowed = self.config.relay.allow_relay
            && self.config.relay.relay_priority != RelayPriority::Never;

        if !relay_allowed {
            debug!(
                message_id = %message.id,
                "Dropping relay message: relay disabled"
            );
            return;
        }

        if message.is_ttl_exhausted() {
            debug!(
                message_id = %message.id,
                sender = %message.sender,
                recipient = %message.recipient,
                "Dropping relay message: TTL exhausted"
            );
            return;
        }

        let mut relay_msg = message.clone();
        let _ = relay_msg.decrement_ttl();
        let _ = relay_msg.increment_hop();

        let hop_count = relay_msg.hop_count.value();
        let remaining_ttl = relay_msg.ttl.value();

        match self.transport_manager.send(&relay_msg) {
            Ok(()) => {
                debug!(
                    message_id = %relay_msg.id,
                    sender = %relay_msg.sender,
                    recipient = %relay_msg.recipient,
                    hop_count,
                    remaining_ttl,
                    "Relayed message for third party"
                );
                self.emit_event(Event::message_relayed(
                    relay_msg.id.as_str(),
                    relay_msg.sender.as_str().to_string(),
                    relay_msg.recipient.as_str().to_string(),
                    hop_count,
                    remaining_ttl,
                ));
            }
            Err(err) => {
                warn!(
                    message_id = %relay_msg.id,
                    error = %err,
                    "Failed to relay message"
                );
            }
        }
    }

    pub(super) fn handle_incoming_file_chunk(&mut self, message: &Message) {
        let sender = message.sender.as_str().to_string();

        // SEC-H1: encrypted chunks carry the chunk bytes, media metadata, and
        // original content type inside the MLS ciphertext; legacy plaintext
        // chunks carry them on the wire Message and are accepted only where
        // policy allows.
        let (chunk, media_metadata, original_content_type) = if let Some(ref binary) =
            message.binary_content
        {
            if is_media_envelope(binary) {
                let encrypted = match decode_media_envelope(binary) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!(
                            message_id = %message.id,
                            error = %e,
                            "Failed to decode encrypted media envelope, dropping"
                        );
                        return;
                    }
                };
                let Some(plaintext) = self.decrypt_media_chunk(&sender, &encrypted, message) else {
                    // Queued for delayed decryption or dropped — handled inside.
                    return;
                };
                let inner = match MediaChunkPlaintext::decode(&plaintext) {
                    Ok(i) => i,
                    Err(e) => {
                        warn!(
                            message_id = %message.id,
                            error = %e,
                            "Failed to parse decrypted media chunk plaintext, dropping"
                        );
                        return;
                    }
                };
                let chunk = match FileChunk::from_bytes(&inner.chunk_bytes) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(
                            message_id = %message.id,
                            error = %e,
                            "Failed to deserialize decrypted file chunk, dropping"
                        );
                        return;
                    }
                };
                (chunk, inner.media_metadata, inner.original_content_type)
            } else {
                if !self.accept_plaintext_media(&sender) {
                    warn!(
                        message_id = %message.id,
                        sender = %sender,
                        "Rejecting unencrypted media chunk (encryption policy)"
                    );
                    return;
                }
                let chunk = match FileChunk::from_bytes(binary) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(
                            message_id = %message.id,
                            error = %e,
                            "Failed to deserialize binary file chunk, dropping"
                        );
                        return;
                    }
                };
                let (meta, oct) = Self::wire_media_metadata(message);
                (chunk, meta, oct)
            }
        } else {
            if !self.accept_plaintext_media(&sender) {
                warn!(
                    message_id = %message.id,
                    sender = %sender,
                    "Rejecting unencrypted media chunk (encryption policy)"
                );
                return;
            }
            let chunk = match FileChunk::from_json(&message.content) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        message_id = %message.id,
                        error = %e,
                        "Failed to deserialize file chunk, dropping"
                    );
                    return;
                }
            };
            let (meta, oct) = Self::wire_media_metadata(message);
            (chunk, meta, oct)
        };

        let file_id = chunk.file_id.clone();
        let file_name = chunk.file_name.clone();
        let file_size = chunk.file_size;
        let is_first_chunk = chunk.chunk_index == 0;

        // Metadata is recorded only for chunks the manager accepts — a
        // rejected chunk must leave no state behind (SEC-H2).
        match self.file_transfer_manager.process_chunk(&sender, chunk) {
            Ok(progress) => {
                if is_first_chunk {
                    self.pending_media_metadata.insert(
                        file_id.clone(),
                        PendingMediaMetadataEntry {
                            content_type: original_content_type.unwrap_or(ContentType::File),
                            media_metadata,
                            last_updated_at: Instant::now(),
                            sender: sender.clone(),
                        },
                    );
                } else if let Some(entry) = self.pending_media_metadata.get_mut(&file_id) {
                    entry.last_updated_at = Instant::now();
                }
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::file_progress(
                        file_id.clone(),
                        progress.chunks_completed,
                        progress.total_chunks,
                    ));
                }
            }
            Err(rejection) if rejection.is_resource_exhaustion() => {
                // A well-formed transfer was dropped by a receiver-side
                // resource limit. The chunk was already ACKed and will not
                // be retransmitted, so the transfer is unrecoverable —
                // surface that to the application instead of going silent.
                self.pending_media_metadata.remove(&file_id);
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::file_receive_failed(
                        file_id,
                        file_name,
                        sender,
                        rejection.as_str().to_string(),
                    ));
                }
                return;
            }
            // Malformed or mismatched chunks are logged by the manager; the
            // assembly they targeted (if any) stays intact. Chunks of an
            // already-failed transfer land here too (`previously_failed`) —
            // its FileReceiveFailed event already fired exactly once.
            Err(_) => return,
        }

        if self.file_transfer_manager.is_complete(&file_id) {
            let Some(file_data) = self.file_transfer_manager.finalize_file(&file_id) else {
                warn!(
                    file_id = %file_id,
                    "File transfer marked complete but reassembly or integrity checks failed"
                );
                self.pending_media_metadata.remove(&file_id);
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::file_receive_failed(
                        file_id,
                        file_name,
                        sender,
                        "integrity_check_failed".to_string(),
                    ));
                }
                return;
            };
            let metadata_entry = self.pending_media_metadata.remove(&file_id);
            let (content_type, media_metadata) = metadata_entry
                .map(|entry| (entry.content_type, entry.media_metadata))
                .unwrap_or((ContentType::File, None));

            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::file_received(
                    file_id,
                    file_name,
                    file_size,
                    sender,
                    content_type,
                    media_metadata,
                    file_data,
                ));
            }
        }
    }

    /// Extracts the chunk-0 metadata a legacy (unencrypted) chunk carries on
    /// the wire `Message`.
    fn wire_media_metadata(message: &Message) -> (Option<MediaMetadata>, Option<ContentType>) {
        use crate::constants::ORIGINAL_CONTENT_TYPE_KEY;
        let original_ct = message
            .metadata
            .get(ORIGINAL_CONTENT_TYPE_KEY)
            .map(|s| ContentType::parse(s));
        (message.media_metadata.clone(), original_ct)
    }

    /// Policy gate for legacy (unencrypted) media chunks: rejected when this
    /// node requires encryption, and rejected once a confirmed MLS session
    /// exists with the sender — an encryption-capable peer sending plaintext
    /// media is a downgrade/forgery attempt (plaintext chunks carry no sender
    /// authentication, so anyone could inject them under a contact's name).
    fn accept_plaintext_media(&mut self, sender: &str) -> bool {
        if self.config.encryption.require_encryption {
            return false;
        }
        // Fail closed: if the confirmation lookup errors (storage failure),
        // treat the session as confirmed and reject — accepting plaintext on
        // error would let a storage fault disable the downgrade gate.
        if self.should_auto_encrypt() && self.is_session_confirmed(sender).unwrap_or(true) {
            return false;
        }
        true
    }

    /// Decrypts an encrypted media chunk envelope, returning the plaintext on
    /// success. On session-not-ready the whole message is queued for delayed
    /// decryption (mirroring the text path — the deduplicator has already
    /// marked this message seen, so dropping it would strand the transfer);
    /// other failures emit decryption telemetry and drop. A successful decrypt
    /// doubles as a session confirmation signal, exactly like text decrypts.
    fn decrypt_media_chunk(
        &mut self,
        sender: &str,
        encrypted: &EncryptedMessage,
        message: &Message,
    ) -> Option<Vec<u8>> {
        let group_id = encrypted.group_id.as_str().to_string();

        // Media envelopes are only ever produced for the sender's 1:1 session,
        // whose MLS group id is deterministic. Enforce that binding before
        // decrypting: MLS authenticates group membership, not the wire sender
        // claim, so without this check any peer holding a valid session with
        // us could deliver its own ciphertext under an arbitrary
        // `message.sender` and have the file attributed to that identity.
        let expected_group = GroupId::for_session(&self.config.user_id, sender);
        if encrypted.group_id != expected_group {
            warn!(
                sender = %sender,
                group_id = %group_id,
                "Encrypted media chunk MLS group does not match the sender's 1:1 session, dropping"
            );
            self.emit_security_warning(
                sender,
                SecurityWarningCode::MediaSenderGroupMismatch,
                "Encrypted media chunk MLS group does not match the claimed sender",
            );
            return None;
        }

        let Some(mls) = self.mls_manager.clone() else {
            warn!(
                sender = %sender,
                "Encrypted media chunk received but MLS is not initialized, dropping"
            );
            self.emit_mls_decryption_failed(
                sender,
                Some(&group_id),
                DecryptionFailureKind::NotInitialized,
                MlsOperationContext::Receive,
            );
            return None;
        };

        let decrypt_result = {
            let manager = match mls.read() {
                Ok(m) => m,
                Err(_) => {
                    error!("MLS lock poisoned while decrypting media chunk");
                    return None;
                }
            };
            manager.decrypt(encrypted)
        };

        match decrypt_result {
            Ok(Some(plaintext)) => {
                // A group-aware decrypt is the same confirmation signal the
                // text path uses; skip when already cache-confirmed to avoid
                // per-chunk storage I/O.
                if !self.confirmed_sessions.contains(sender) {
                    self.confirm_session_from_successful_decrypt(sender, &group_id);
                }
                Some(plaintext)
            }
            Ok(None) => {
                warn!(sender = %sender, "Media chunk decryption returned empty, dropping");
                None
            }
            Err(e) => {
                let classification = SessionStateError::from(&e);
                match classification {
                    SessionStateError::SessionNotReady | SessionStateError::GroupNotFound => {
                        info!(
                            sender = %sender,
                            error_code = classification.code(),
                            "Encrypted media chunk received before session ready, queuing"
                        );
                        self.emit_mls_session_missing(
                            Some(sender),
                            Some(&group_id),
                            MlsOperationContext::SessionLookup,
                            MlsErrorCategory::SessionStateMissing,
                        );
                        self.enqueue_pending_decryption(sender, message);
                        None
                    }
                    _ => {
                        warn!(
                            sender = %sender,
                            error = %e,
                            error_code = classification.code(),
                            "Failed to decrypt media chunk, dropping"
                        );
                        let kind = DecryptionFailureKind::from_mls_error(&e);
                        self.emit_mls_decryption_failed(
                            sender,
                            Some(&group_id),
                            kind,
                            MlsOperationContext::Receive,
                        );
                        // The chunk was already ACKed and dedup-marked on
                        // receipt, so this loss is permanent — surface it to
                        // the app like a pending-queue drop, not just as MLS
                        // telemetry.
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::message_decryption_failed(
                                message.id.clone(),
                                sender.to_string(),
                                Self::decryption_failure_code_from_kind(kind),
                                format!(
                                    "encrypted media chunk failed to decrypt ({}); its file transfer cannot complete",
                                    classification.code()
                                ),
                            ));
                        }
                        None
                    }
                }
            }
        }
    }
}
