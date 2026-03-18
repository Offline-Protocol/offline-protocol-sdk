//! Pending decryption queue: thin wrappers on [`PendingDecryptionQueue`] that
//! need access to the broader [`OfflineProtocol`] state (shared state, MLS
//! decryption, lamport clock).

use super::{lock_shared_state, InternalMessageResult, OfflineProtocol};
use crate::events::Event;
use chrono::Utc;
use std::time::Instant;
use tracing::{debug, info};

impl OfflineProtocol {
    pub(super) fn enqueue_pending_decryption(
        &mut self,
        sender: &str,
        message: &offline_protocol_core::Message,
    ) {
        let config = &self.config.encryption.pending_queue;
        self.pending_queue.enqueue(config, sender, message);
    }

    pub(super) fn prune_expired_pending_global_front(
        &mut self,
        now: Instant,
        max_evictions: usize,
    ) -> usize {
        let config = &self.config.encryption.pending_queue;
        self.pending_queue
            .prune_expired_global_front(config, now, max_evictions)
    }

    /// Processes encrypted messages that were received before the session was established.
    ///
    /// This handles the case where encrypted messages arrive before the Welcome message.
    /// After the session is confirmed (via Welcome), we re-process these queued messages.
    pub(super) fn process_pending_decryption(&mut self, sender: &str) {
        let config = self.config.encryption.pending_queue.clone();
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
