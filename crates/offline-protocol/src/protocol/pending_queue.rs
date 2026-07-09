//! Pending decryption queue: thin wrappers on [`PendingDecryptionQueue`] that
//! need access to the broader [`OfflineProtocol`] state (shared state, MLS
//! decryption, lamport clock).

use super::decryption_queue::DroppedPendingMessage;
use super::{lock_shared_state, InternalMessageResult, OfflineProtocol};
use crate::events::{DecryptionFailureCode, Event};
use chrono::Utc;
use offline_protocol_core::ContentType;
use std::time::Instant;
use tracing::{debug, info, warn};

impl OfflineProtocol {
    pub(super) fn enqueue_pending_decryption(
        &mut self,
        sender: &str,
        message: &offline_protocol_core::Message,
    ) {
        let config = &self.config.encryption.pending_queue;
        let dropped = self.pending_queue.enqueue(config, sender, message);
        self.report_dropped_pending_media(dropped);
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
        self.report_dropped_pending_media(expired);
        count
    }

    /// Surfaces pending-queue drops of encrypted media chunks. A dropped
    /// chunk is unrecoverable: it was ACKed and dedup-marked on receipt, so
    /// the sender will not retransmit it and already counts it delivered —
    /// the file transfer it belongs to can never complete. Emit a loud,
    /// machine-readable failure instead of stalling silently. (The chunk is
    /// still encrypted at this point, so the file_id cannot be named here.)
    ///
    /// Dropped text messages keep their existing metrics-only handling: the
    /// pending message queue was sized for them, and their loss is already
    /// tracked via `PendingQueueMetrics`.
    fn report_dropped_pending_media(&mut self, dropped: Vec<DroppedPendingMessage>) {
        for entry in dropped {
            if entry.message.content_type != ContentType::FileChunk {
                continue;
            }
            warn!(
                sender = %entry.message.sender,
                message_id = %entry.message.id,
                reason = entry.reason,
                "Encrypted media chunk dropped from pending queue; its file transfer cannot complete"
            );
            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::message_decryption_failed(
                    entry.message.id.clone(),
                    entry.message.sender.as_str().to_string(),
                    DecryptionFailureCode::PendingQueueDropped,
                    format!(
                        "encrypted media chunk dropped from pending queue ({}); its file transfer cannot complete",
                        entry.reason
                    ),
                ));
            }
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
        self.report_dropped_pending_media(expired);
        let drained = self.pending_queue.drain_for_peer(&config, sender);

        if drained.is_empty() {
            return;
        }

        info!(
            sender = %sender,
            count = drained.len(),
            "Processing pending encrypted messages"
        );

        for entry in drained {
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
                self.handle_incoming_file_chunk(&msg);
                continue;
            }

            if let Some(result) = self.process_internal_message(&msg) {
                match result {
                    InternalMessageResult::Decrypted(content) => {
                        let mut decrypted_msg = msg.clone();
                        decrypted_msg.content = content.clone();
                        decrypted_msg
                            .metadata
                            .insert("encrypted".to_string(), "true".to_string());
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
                                content,
                                hop_count: decrypted_msg.hop_count.value(),
                                transport: "delayed".to_string(),
                                timestamp: Utc::now().timestamp_millis(),
                                lamport_clock: decrypted_msg.lamport_clock.value(),
                                reply_to_msg: decrypted_msg
                                    .reply_to_msg
                                    .as_ref()
                                    .map(|id| id.as_str().to_string()),
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

                        debug!(message_id = %msg.id, "Processed delayed encrypted message");
                    }
                    InternalMessageResult::Consumed => {
                        debug!(message_id = %msg.id, "Delayed message was consumed internally");
                    }
                    InternalMessageResult::SecurityRejected => {
                        debug!(message_id = %msg.id, "Delayed message was rejected by security gate");
                    }
                }
            }
        }
    }
}
