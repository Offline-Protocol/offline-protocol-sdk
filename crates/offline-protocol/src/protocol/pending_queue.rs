//! Pending decryption queue: thin wrappers on [`PendingDecryptionQueue`] that
//! need access to the broader [`OfflineProtocol`] state (shared state, MLS
//! decryption, lamport clock).

use super::decryption_queue::{DroppedPendingMessage, PendingDecryptMessage};
use super::{lock_shared_state, ChunkOutcome, InternalMessageResult, OfflineProtocol};
use crate::events::{DecryptionFailureCode, Event};
use chrono::Utc;
use offline_protocol_core::{ContentType, Message};
use offline_protocol_transport::TransportType;
use std::time::Instant;
use tracing::{debug, error, info, warn};

impl OfflineProtocol {
    /// Transport-less convenience wrapper over
    /// [`Self::enqueue_pending_decryption_via`], used only by tests; production
    /// always knows the arrival transport.
    #[cfg(test)]
    pub(super) fn enqueue_pending_decryption(
        &mut self,
        sender: &str,
        message: &offline_protocol_core::Message,
    ) {
        self.enqueue_pending_decryption_via(sender, message, None);
    }

    /// [`Self::enqueue_pending_decryption`] with the transport the frame arrived
    /// on, when the caller knows it. The transport is recorded on the pending
    /// entry so the drain can send the deferred delivery ACK directly instead of
    /// relying on the sender's next resend (see the deferred-acknowledgement
    /// atom in `docs/state-machines/delivery-and-acks.md`).
    ///
    /// An admitted frame is also written to protocol-state storage
    /// (`PendingDecryptRecord`), stamped with its first receipt, so it survives
    /// a restart; see [`Self::enqueue_restored_pending_decryption`] for the
    /// way back in.
    pub(super) fn enqueue_pending_decryption_via(
        &mut self,
        sender: &str,
        message: &offline_protocol_core::Message,
        arrival_transport: Option<TransportType>,
    ) {
        if self.enqueue_pending_decryption_in_memory(sender, message, arrival_transport) {
            self.persist_pending_decrypt_entry(
                sender,
                message,
                Utc::now().timestamp_millis(),
                arrival_transport,
            );
        }
    }

    /// Re-admits a frame read back from storage: the in-memory enqueue with
    /// the same overflow handling as the live path, but **no** write — the
    /// record on disk is already the durable copy and keeps its first-receipt
    /// timestamp. A restored frame the queue refuses (its cap was lowered, or
    /// the store holds more than memory admits) has its record deleted like
    /// any other overflow drop.
    pub(crate) fn enqueue_restored_pending_decryption(
        &mut self,
        sender: &str,
        message: &offline_protocol_core::Message,
        received_via: Option<TransportType>,
    ) {
        self.enqueue_pending_decryption_in_memory(sender, message, received_via);
    }

    /// The shared half of the two enqueue paths: admits into the in-memory
    /// queue, reports every drop and deletes each dropped frame's record.
    /// Returns whether the incoming frame was admitted.
    fn enqueue_pending_decryption_in_memory(
        &mut self,
        sender: &str,
        message: &offline_protocol_core::Message,
        arrival_transport: Option<TransportType>,
    ) -> bool {
        let config = &self.config.encryption.pending_queue;
        let outcome = self
            .pending_queue
            .enqueue_via(config, sender, message, arrival_transport);
        self.report_dropped_pending(outcome.dropped);
        outcome.admitted
    }

    pub(super) fn prune_expired_pending_global_front(
        &mut self,
        now: Instant,
        max_evictions: usize,
    ) -> usize {
        let config = &self.config.encryption.pending_queue;
        let expired = self
            .pending_queue
            .prune_expired_global_front(config, now, max_evictions);
        let count = expired.len();
        self.report_dropped_pending(expired);
        count
    }

    /// Drains a peer's parked frames without processing them, deleting each
    /// one's persisted record. For the paths where every queued frame from a
    /// peer is being discarded at once — today, the session reset on unblock.
    /// Returns how many were discarded.
    pub(super) fn discard_pending_decryption_for_peer(&mut self, peer_id: &str) -> usize {
        let config = self.config.encryption.pending_queue.clone();
        let drained = self.pending_queue.drain_for_peer(&config, peer_id);
        self.delete_pending_decrypt_entries_from_storage(drained.iter().map(|e| &e.message.id));
        drained.len()
    }

    /// Takes a peer's parked frames out of the queue for processing, deleting
    /// their persisted records first: whatever the drain does with a frame —
    /// surface it, consume it, or drop it as undecryptable — the queue no
    /// longer holds it, and a record that outlived its entry would be
    /// restored and re-drained on the next launch, where a frame whose ratchet
    /// generation was spent by this drain surfaces as a spurious decrypt
    /// failure. A frame the handler re-queues during the drain (still
    /// session-not-ready) goes through the live enqueue and is re-persisted
    /// there.
    fn take_pending_decryption_for_drain(&mut self, sender: &str) -> Vec<PendingDecryptMessage> {
        let config = self.config.encryption.pending_queue.clone();
        let drained = self.pending_queue.drain_for_peer(&config, sender);
        self.delete_pending_decrypt_entries_from_storage(drained.iter().map(|e| &e.message.id));
        drained
    }

    /// Surfaces every pending-queue eviction as a `PendingQueueDropped`
    /// decryption failure so an app can react — a stalled media transfer, or a
    /// text message that will only arrive if the sender resends — instead of
    /// watching the gap silently. (The frame is still encrypted at this point,
    /// so neither the file_id nor the text can be named here.)
    ///
    /// Under the deferred-ACK model this is **advisory, not terminal**: an
    /// evicted frame was never ACKed, so a sender still retrying resends it, and
    /// the resend re-enters the queue and can still complete once the session
    /// confirms. The event says the message is *at risk*, not that it has
    /// failed — for media the terminal signal is `FileReceiveFailed`. It is
    /// emitted for text as well as for chunks: text drops used to be
    /// metrics-only, which left an app with no way to distinguish "the sender
    /// went quiet" from "the SDK evicted their message", and a relay that
    /// pushes ciphertext without store-and-forward has no second copy to fall
    /// back on, so silence there was a lost message.
    fn report_dropped_pending(&mut self, dropped: Vec<DroppedPendingMessage>) {
        // A dropped frame's record goes with it: the on-disk copy exists only
        // to mirror what the queue holds, and a record that outlived its entry
        // would be restored — and re-dropped, and re-reported — on the next
        // launch. Deleting a record that was never written (a refused frame)
        // is a no-op.
        self.delete_pending_decrypt_entries_from_storage(dropped.iter().map(|e| &e.message.id));
        for entry in dropped {
            let is_media_chunk = entry.message.content_type == ContentType::FileChunk;
            let reason = if is_media_chunk {
                format!(
                    "encrypted media chunk evicted from pending queue ({}); its file transfer is stalled until the sender resends",
                    entry.reason
                )
            } else {
                format!(
                    "encrypted message evicted from pending queue ({}); recoverable only if the sender resends",
                    entry.reason
                )
            };
            warn!(
                sender = %entry.message.sender,
                message_id = %entry.message.id,
                content_type = %entry.message.content_type,
                reason = entry.reason,
                "Encrypted message evicted from pending queue; recoverable only if the sender resends"
            );
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::message_decryption_failed(
                    entry.message.id.clone(),
                    entry.message.sender.as_str().to_string(),
                    DecryptionFailureCode::PendingQueueDropped,
                    reason,
                ));
            }
        }
    }

    /// Sends the deferred delivery ACK for a message surfaced by the drain,
    /// using the transport it originally arrived on.
    ///
    /// This closes the ACK-latency window: without it the receiver would surface
    /// the message locally but only ACK it when the sender's *next* resend hit
    /// the duplicate re-ACK path — so a sender that exhausted its retry budget
    /// before the session confirmed would mark a locally-delivered message
    /// undeliverable. `received_via` is `None` only when the entry was queued
    /// from a transport-less context (tests, or a handler re-queue during a
    /// drain); the drain then falls back to the resend-driven ACK as before.
    fn ack_drained_message(&mut self, message: &Message, received_via: Option<TransportType>) {
        if !message.requires_ack {
            return;
        }
        let Some(transport) = received_via else {
            return;
        };
        if let Err(err) = self.send_delivery_ack(message, transport) {
            error!(
                message_id = %message.id,
                error = %err,
                "Failed to send deferred delivery ACK for drained message"
            );
        }
    }

    /// Processes encrypted messages that were received before the session was established.
    ///
    /// This handles the case where encrypted messages arrive before the Welcome message.
    /// After the session is confirmed (via Welcome), we re-process these queued messages.
    pub(super) fn process_pending_decryption(&mut self, sender: &str) {
        let config = self.config.encryption.pending_queue.clone();
        // Report TTL-expired entries before draining: expired media chunks are
        // unrecoverable and would otherwise be discarded silently inside
        // `drain_for_peer`'s pre-prune.
        let expired = self
            .pending_queue
            .prune_expired_for_peer(&config, sender, Instant::now());
        self.report_dropped_pending(expired);
        let drained = self.take_pending_decryption_for_drain(sender);

        if drained.is_empty() {
            return;
        }

        info!(
            sender = %sender,
            count = drained.len(),
            "Processing pending encrypted messages"
        );

        for entry in drained {
            // Capture the arrival transport before consuming the entry: the
            // drain now ACKs a surfaced message directly on it (closing the
            // ACK-latency window) rather than waiting for the sender's next
            // resend to hit the duplicate re-ACK path.
            let received_via = entry.received_via;
            let msg = entry.message;

            // Block filter: skip messages from blocked users that were queued
            // before the block was applied.
            if self.is_user_blocked(msg.sender.as_str()) {
                debug!(
                    sender = %msg.sender,
                    message_id = %msg.id,
                    "Dropping pending message from blocked user"
                );
                continue;
            }

            // Encrypted media chunks don't go through process_internal_message
            // (their payload is in binary_content, not a content prefix) —
            // route them back through the chunk handler now that the session
            // is ready.
            if msg.content_type == ContentType::FileChunk {
                match self.handle_incoming_file_chunk_via(&msg, received_via) {
                    // Delivered/assembled or terminally dropped: re-mark the id
                    // (the deferred path unmarked it on receipt) so a later
                    // resend is deduped rather than re-processed, and ACK it
                    // now on its arrival transport so the sender can stop
                    // retrying without a further resend.
                    ChunkOutcome::Handled => {
                        self.mark_seen_persisted(msg.id.clone());
                        self.ack_drained_message(&msg, received_via);
                    }
                    // Not delivered, recoverable by a resend. Two shapes, both
                    // leaving the id unmarked: still session-not-ready
                    // (unexpected post-confirmation — the handler re-queued the
                    // chunk itself), or a hard decrypt failure, which is
                    // deliberately *not* re-queued because its generation is
                    // spent. Either way recovery is the sender's resend, which
                    // for media means re-encoded bytes via `MediaResendRequired`
                    // rather than a replay of these.
                    ChunkOutcome::Deferred => {}
                    // Refused as illegitimate: never ACK (don't confirm
                    // processing to an injector) and leave the id unmarked.
                    //
                    // The plaintext-policy shape cannot occur here (queued
                    // chunks are always encrypted), but the identity-binding
                    // shape can: a chunk queued while the session was missing
                    // now decrypts far enough for the MLS credential to be
                    // checked, and that check can fail where the pre-decrypt
                    // slot check passed. Never re-queued — the frame can never
                    // become legitimate.
                    ChunkOutcome::Rejected => {}
                }
                continue;
            }

            if let Some(result) = self.process_internal_message(&msg) {
                match result {
                    InternalMessageResult::Decrypted(content) => {
                        let mut decrypted_msg = msg.clone();
                        // Shared with the live receive path: swaps in the
                        // plaintext, drops the relay-writable outer
                        // `reply_context`, and restores rich fields from a
                        // sealed `__RICH_V1__` body (see
                        // `apply_decrypted_content`) — so the event below
                        // must read content/rich fields from the message,
                        // not the raw decrypted string.
                        Self::apply_decrypted_content(&mut decrypted_msg, content);
                        decrypted_msg
                            .metadata
                            .insert("delayed_decrypt".to_string(), "true".to_string());

                        self.lamport_clock.merge(decrypted_msg.lamport_clock);
                        self.persist_lamport_clock();

                        if let Ok(mut state) = lock_shared_state(&self.shared_state) {
                            state.received_messages.push_back(decrypted_msg.clone());
                            let event = Event::MessageReceived {
                                message_id: decrypted_msg.id.as_str().to_string(),
                                sender: decrypted_msg.sender.as_str().to_string(),
                                recipient: decrypted_msg.recipient.as_str().to_string(),
                                content: decrypted_msg.content.clone(),
                                hop_count: decrypted_msg.hop_count.value(),
                                transport: "delayed".to_string(),
                                timestamp: Utc::now().timestamp_millis(),
                                lamport_clock: decrypted_msg.lamport_clock.value(),
                                reply_to_msg: decrypted_msg
                                    .reply_to_msg
                                    .as_ref()
                                    .map(|id| id.as_str().to_string()),
                                reply_context: decrypted_msg
                                    .reply_context
                                    .as_ref()
                                    .map(|rc| Box::new(crate::events::ReplyContextEvent::from(rc))),
                                content_type: decrypted_msg.content_type.to_string(),
                                media_metadata: decrypted_msg.media_metadata.clone(),
                                forward_info: decrypted_msg
                                    .forwarded_from
                                    .as_ref()
                                    .map(crate::events::ForwardInfoEvent::from),
                                encrypted: true,
                            };
                            state.emit_event(event);
                        }

                        // Re-mark the id as seen now that it is delivered. On
                        // first receipt the deferred path unmarked it (so the
                        // sender's resends would re-enter processing); with the
                        // message now surfaced, a subsequent resend must be
                        // deduped + re-ACKed rather than re-surfaced (double
                        // delivery) or re-decrypted (an MLS replay the ratchet
                        // would reject). This is the counterpart to the unmark
                        // in the receive loop's `Deferred` arm.
                        self.mark_seen_persisted(msg.id.clone());

                        // ACK on drain: the message is delivered locally now, so
                        // send the deferred delivery ACK directly on its arrival
                        // transport instead of waiting for the sender's next
                        // resend. A later resend still hits the duplicate re-ACK
                        // path (harmless second ACK), but the sender no longer
                        // has to resend at all to learn of delivery.
                        self.ack_drained_message(&msg, received_via);

                        debug!(message_id = %msg.id, "Processed delayed encrypted message");
                    }
                    InternalMessageResult::Consumed => {
                        // Internal control message (e.g. session-confirm) that
                        // decrypted on drain. It was unmarked on the deferred
                        // path; re-mark so a resend is deduped rather than
                        // reprocessed, and ACK it (control messages are
                        // delivery-sensitive, exactly like the live path).
                        self.mark_seen_persisted(msg.id.clone());
                        self.ack_drained_message(&msg, received_via);
                        debug!(message_id = %msg.id, "Delayed message was consumed internally");
                    }
                    InternalMessageResult::Deferred => {
                        // Still undecryptable during a drain. Not ACKed and the
                        // id is not re-marked, so the sender's resend still
                        // recovers the message instead of being told
                        // "delivered" for a frame we dropped.
                        //
                        // Deliberately does NOT re-queue here, because each of
                        // the three ways a drained frame reaches this arm has
                        // already settled the question:
                        //
                        // - Session not ready (unexpected post-confirmation):
                        //   `handle_encrypted_message` re-queued it itself
                        //   before returning `Deferred`, so a re-queue here is
                        //   redundant — `enqueue` is idempotent by id.
                        // - Epoch desync, and a hard crypto/transport failure
                        //   (the routine case): both refuse to enqueue on the
                        //   live path for a reason that holds just as well
                        //   here — the ciphertext is sealed to a dead epoch, or
                        //   the failed attempt already spent its ratchet
                        //   generation, so no number of later drains can make
                        //   this frame decrypt.
                        //
                        // Re-queuing the latter two used to look free, but the
                        // drain *removes* the entry before processing, so the
                        // re-enqueue missed `enqueue`'s idempotency check and
                        // re-stamped `received_at`. A frame that can never
                        // decrypt then had its TTL restarted by every drain,
                        // outliving its budget and re-reporting an advisory
                        // decrypt failure each time — including after the
                        // sender's re-sealed resend had already delivered the
                        // same id. Dropping it costs nothing the un-ACK does
                        // not already cover.
                        debug!(
                            message_id = %msg.id,
                            "Delayed message still undecryptable during drain; dropping the queued copy (recovery is the sender's resend)"
                        );
                    }
                    InternalMessageResult::SecurityRejected => {
                        debug!(message_id = %msg.id, "Delayed message was rejected by security gate");
                    }
                }
            }
        }
    }
}
