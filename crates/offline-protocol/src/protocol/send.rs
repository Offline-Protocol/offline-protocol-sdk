//! Send pipeline, outbox management, and delivery tracking.

use super::{
    internal_prefixes, lock_shared_state, ConnectionAcceptedPayload, ConnectionRequestPayload,
    KeyPackagePayload, OfflineProtocol, OutboundMediaTransfer, OutboundSendPreparation,
    OutboxEntry, PendingMessage, PresencePayload, ProtocolState, ReadReceiptPayload,
    TypingIndicatorPayload, WelcomeDeliveryState, MAX_READ_RECEIPT_IDS,
};
use crate::constants::{ACK_FOR_KEY, ACK_HOP_COUNT_KEY, ACK_TRANSPORT_KEY, MAX_OUTBOX_ENTRIES};
use crate::events::{DecryptionFailureCode, Event, PresenceStatus};
use crate::file_transfer::{FileChunk, OutboundTransferState};
use crate::mls_observability::{DecryptionFailureKind, MlsErrorCategory, MlsOperationContext};
use crate::{Error, Result};
use chrono::{Duration as ChronoDuration, Utc};
use offline_protocol_core::{
    AppId, ContentType, MediaMetadata, Message, MessageId, MessagePriority, UserId, TTL,
};
use offline_protocol_mls::MlsManager;
use offline_protocol_transport::TransportType;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use std::time::{Duration as StdDuration, Instant};
use tracing::{debug, error, info, warn};

impl OfflineProtocol {
    // ========================================================================
    // CORE SEND
    // ========================================================================

    /// Sends a message.
    ///
    /// # Arguments
    ///
    /// * `recipient` - Recipient's user ID
    /// * `content` - Message content
    /// * `priority` - Message priority (optional, defaults to Medium)
    /// * `reply_to_msg` - ID of the message this is replying to (optional)
    ///
    /// # Returns
    ///
    /// Returns the message ID if successful.
    ///
    /// # Auto-Encryption
    ///
    /// When encryption is enabled and MLS is initialized, messages are automatically
    /// encrypted before sending. If no session exists with the recipient but we have
    /// their key package, a session is created automatically. If no key package is
    /// available and `store_pending` is enabled, the message is queued until a key
    /// package is received.
    pub fn send_message(
        &mut self,
        recipient: impl Into<String>,
        content: impl Into<String>,
        priority: Option<MessagePriority>,
        reply_to_msg: Option<impl Into<String>>,
    ) -> Result<MessageId> {
        // Check if protocol is running
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let recipient_str: String = recipient.into();
        let content_str: String = content.into();
        let priority = priority.unwrap_or(MessagePriority::Medium);

        // Prevent sending messages to blocked users. Blocking is bidirectional:
        // we neither receive from nor send to a blocked peer.
        if self.is_user_blocked(&recipient_str) {
            return Err(Error::UserBlocked(recipient_str));
        }

        // Reject content that starts with an internal control prefix to prevent
        // injection of protocol-level messages through the public API.
        if Self::is_internal_prefix(&content_str) {
            return Err(Error::Other(
                "Message content must not start with a reserved internal prefix".to_string(),
            ));
        }

        // Parse reply_to_msg if provided
        let reply_to_msg_id = reply_to_msg
            .map(|r| MessageId::from_str(&r.into()))
            .transpose()
            .map_err(|e| Error::Other(format!("Invalid reply_to_msg: {}", e)))?;

        let final_content = match self.prepare_outbound_content(
            &recipient_str,
            &content_str,
            priority,
            reply_to_msg_id.clone(),
            "send_message_session_pending",
        )? {
            OutboundSendPreparation::Ready(content) => content,
            OutboundSendPreparation::Queued(message_id) => return Ok(message_id),
        };

        // Create message with potentially encrypted content
        let message = self.create_message(
            &recipient_str,
            final_content,
            Some(priority),
            reply_to_msg_id,
        )?;
        let message_id = message.id.clone();

        // Check for duplicates
        if self.deduplicator.is_duplicate(&message_id) {
            return Err(crate::Error::Other("Duplicate message".to_string()));
        }

        // Mark as seen
        self.deduplicator.mark_seen(message_id.clone());

        // Track previous transport before sending
        let previous_transport = self.transport_manager.current_transport();

        // Attempt to send via transport manager (DORS will select best transport)
        let send_result = self.transport_manager.send(&message);
        let current_transport = self.transport_manager.current_transport();

        // Handle send result
        match send_result {
            Ok(()) => {
                self.handle_send_success(&message, current_transport)?;
                self.emit_transport_switch_event(previous_transport, current_transport)?;
                self.emit_message_sent_event(&message)?;
                Ok(message_id)
            }
            Err(err) => {
                self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                warn!(
                    message_id = %message.id,
                    error = %err,
                    "Send failed, message deferred"
                );
                Err(Error::Other(format!(
                    "Send failed (message {} deferred for retry): {}",
                    message.id, err
                )))
            }
        }
    }

    /// Sends an internal protocol message (connection requests, etc.) via DORS.
    ///
    /// Handles the full send orchestration: state check, deduplication, transport send,
    /// success/failure handling, and transport switch events. Does NOT emit a
    /// `MessageSent` event — internal messages are not user-visible content.
    pub(crate) fn send_internal_message(
        &mut self,
        recipient: &str,
        content: String,
        priority: MessagePriority,
    ) -> Result<MessageId> {
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let mut message = self.create_message(recipient, content, Some(priority), None)?;
        self.sign_control_message(&mut message)?;
        let message_id = message.id.clone();

        if self.deduplicator.is_duplicate(&message_id) {
            return Err(crate::Error::Other("Duplicate message".to_string()));
        }

        self.deduplicator.mark_seen(message_id.clone());

        let previous_transport = self.transport_manager.current_transport();
        let send_result = self.transport_manager.send(&message);
        let current_transport = self.transport_manager.current_transport();

        match send_result {
            Ok(()) => {
                self.handle_send_success(&message, current_transport)?;
            }
            Err(err) => {
                self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                warn!(
                    message_id = %message.id,
                    recipient = %recipient,
                    error = %err,
                    "Internal message send failed, message deferred"
                );
            }
        }

        self.emit_transport_switch_event(previous_transport, current_transport)?;
        Ok(message_id)
    }

    /// Sends a message via a specific transport type.
    pub fn send_message_via_transport(
        &mut self,
        recipient: impl Into<String>,
        content: impl Into<String>,
        priority: Option<MessagePriority>,
        transport: TransportType,
        reply_to_msg: Option<impl Into<String>>,
    ) -> Result<MessageId> {
        // Check if protocol is running
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let recipient_str: String = recipient.into();
        let content_str: String = content.into();
        let priority = priority.unwrap_or(MessagePriority::Medium);

        // Prevent sending messages to blocked users. Blocking is bidirectional:
        // we neither receive from nor send to a blocked peer.
        if self.is_user_blocked(&recipient_str) {
            return Err(Error::UserBlocked(recipient_str));
        }

        // Reject content that starts with an internal control prefix to prevent
        // injection of protocol-level messages through the public API.
        if Self::is_internal_prefix(&content_str) {
            return Err(Error::Other(
                "Message content must not start with a reserved internal prefix".to_string(),
            ));
        }

        // Parse reply_to_msg if provided
        let reply_to_msg_id = reply_to_msg
            .map(|r| MessageId::from_str(&r.into()))
            .transpose()
            .map_err(|e| Error::Other(format!("Invalid reply_to_msg: {}", e)))?;

        let final_content = match self.prepare_outbound_content(
            &recipient_str,
            &content_str,
            priority,
            reply_to_msg_id.clone(),
            "send_message_via_transport_session_pending",
        )? {
            OutboundSendPreparation::Ready(content) => content,
            OutboundSendPreparation::Queued(message_id) => return Ok(message_id),
        };

        // Create message
        let message = self.create_message(
            &recipient_str,
            final_content,
            Some(priority),
            reply_to_msg_id,
        )?;
        let message_id = message.id.clone();

        // Check for duplicates
        if self.deduplicator.is_duplicate(&message_id) {
            return Err(crate::Error::Other("Duplicate message".to_string()));
        }

        // Mark as seen
        self.deduplicator.mark_seen(message_id.clone());

        // Track previous transport before sending
        let previous_transport = self.transport_manager.current_transport();

        // Attempt to send via the specified transport (bypassing DORS)
        let send_result = self
            .transport_manager
            .send_via_transport(&message, transport);
        let current_transport = Some(transport);

        // Handle send result
        match send_result {
            Ok(()) => {
                self.handle_send_success(&message, current_transport)?;
                self.emit_transport_switch_event(previous_transport, current_transport)?;
                self.emit_message_sent_event(&message)?;
                Ok(message_id)
            }
            Err(err) => {
                self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                // send_via_transport does not record retry failures internally
                // (unlike TransportManager::send), so record explicitly here.
                self.transport_manager.record_retry_failure(transport);
                warn!(
                    message_id = %message.id,
                    transport = ?transport,
                    error = %err,
                    "Send via forced transport failed, message deferred"
                );
                Err(Error::Other(format!(
                    "Send via {:?} failed (message {} deferred for retry): {}",
                    transport, message.id, err
                )))
            }
        }
    }

    /// Creates a new message from the given parameters.
    pub(super) fn create_message(
        &mut self,
        recipient: impl Into<String>,
        content: impl Into<String>,
        priority: Option<MessagePriority>,
        reply_to_msg: Option<MessageId>,
    ) -> Result<Message> {
        let sender = UserId::new(&self.config.user_id)?;
        let recipient = UserId::new(recipient)?;
        let app_id = AppId::new(&self.config.app_id)?;

        let clock_value = self.lamport_clock.tick();
        self.persist_lamport_clock();

        let mut builder = Message::builder(sender, recipient, app_id)
            .content(content)
            .priority(priority.unwrap_or(MessagePriority::Medium))
            .ttl(TTL::new(self.config.initial_ttl)?)
            .lamport_clock(clock_value);

        if let Some(reply_to) = reply_to_msg {
            builder = builder.reply_to_msg(reply_to);
        }

        Ok(builder.build())
    }

    // ========================================================================
    // ENCRYPTION
    // ========================================================================

    pub(super) fn encrypt_content_for_recipient(
        &mut self,
        recipient: &str,
        content: &str,
        _priority: MessagePriority,
    ) -> Result<String> {
        // Clone the Arc to avoid borrow issues
        let mls = self.mls_manager.clone().ok_or_else(|| {
            self.emit_mls_session_missing(
                Some(recipient),
                None,
                MlsOperationContext::SessionLookup,
                MlsErrorCategory::NotInitialized,
            );
            Error::MlsNotInitialized
        })?;

        // Fast path: if the session is already confirmed (in-memory cache hit),
        // skip the has_session() storage call — we know the session exists.
        if self.confirmed_sessions.contains(recipient) {
            return self.encrypt_confirmed_session(&mls, recipient, content);
        }

        // Check for existing session (requires storage I/O via load_group)
        let has_session = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.has_session(recipient)?
        };

        if !has_session {
            // Try loading key package from storage (e.g. after restart) then create session from memory
            self.try_load_key_package_from_storage_into_memory(recipient);
            // Try to create session from stored key package
            // Clone first, only remove after all operations succeed to avoid losing the key package on failure
            if let Some(received_pkg) = self.pending_key_packages.get(recipient).cloned() {
                // Check if key package has expired (using local clock)
                let now_ms = Utc::now().timestamp_millis() as u64;
                if now_ms >= received_pkg.local_expires_at_ms {
                    warn!(recipient = %recipient, "Received key package has expired, discarding");
                    self.pending_key_packages.remove(recipient);
                    self.delete_peer_key_package_from_storage(recipient);
                } else {
                    {
                        let manager = mls
                            .read()
                            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                        manager.import_key_package(recipient, &received_pkg.key_package_data)?;
                    }

                    // Create session and send welcome message
                    let welcome = {
                        let manager = mls
                            .read()
                            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                        manager.create_session(recipient)?
                    };

                    // All operations succeeded, now safe to remove the key package
                    self.pending_key_packages.remove(recipient);
                    self.delete_peer_key_package_from_storage(recipient);

                    let group_id = welcome.group_id.as_str().to_string();
                    let is_session = group_id.starts_with("session:");

                    if let Err(err) =
                        self.ensure_session_state_entry(recipient, "session_created_local")
                    {
                        warn!(
                            recipient = %recipient,
                            error = %err,
                            "Failed to persist pending session state"
                        );
                    }

                    let welcome_sent = self.send_welcome_message(recipient, &welcome)?;

                    debug!(
                        recipient = %recipient,
                        group_id = %group_id,
                        welcome_sent = welcome_sent,
                        "Created MLS session and scheduled welcome lifecycle"
                    );

                    if welcome_sent {
                        debug!(recipient = %recipient, group_id = %group_id, is_session, "Welcome synchronously sent");
                    }

                    // Don't encrypt immediately after creating session.
                    // Queue message until session is confirmed (peer processes our Welcome
                    // and we successfully decrypt their first message, or we receive their Welcome).
                    // This avoids race conditions where both peers create sessions.
                    if self.config.encryption.store_pending {
                        return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
                    }
                }
            } else {
                // No key package available (memory nor storage)
                self.emit_mls_session_missing(
                    Some(recipient),
                    None,
                    MlsOperationContext::SessionLookup,
                    MlsErrorCategory::SessionStateMissing,
                );
                return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
            }
        }

        // Only encrypt if session is confirmed (Welcome processed or successful decrypt).
        // Confirmation truth comes from persisted session state.
        if !self.is_session_confirmed(recipient)? {
            debug!(recipient = %recipient, "Session exists but not confirmed, queuing message");
            return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
        }

        // Encrypt the message
        let encrypted = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager
                .encrypt_for_user(recipient, content.as_bytes())
                .map_err(|_| Error::EncryptFailed("encryption operation failed".to_string()))?
        };

        // Serialize encrypted message with prefix
        let serialized =
            serde_json::to_string(&encrypted).map_err(|e| Error::Serialization(e.to_string()))?;

        self.emit_mls_encryption_used(recipient);
        Ok(format!("{}{}", internal_prefixes::ENCRYPTED, serialized))
    }

    /// Encrypts content for a recipient whose session is known to be confirmed
    /// (in-memory cache hit). Uses `encrypt_for_existing_session` to skip both
    /// the external `has_session()` and the internal one inside `encrypt_for_user`,
    /// reducing storage I/O from 2 round-trips to 1 (`load_group` for encrypt).
    ///
    /// Only evicts the cache on `SessionNotFound` (session deleted externally).
    /// Transient errors (crypto, storage I/O) propagate without cache eviction
    /// so the fast path is preserved for the next attempt.
    pub(super) fn encrypt_confirmed_session(
        &mut self,
        mls: &Arc<RwLock<MlsManager>>,
        recipient: &str,
        content: &str,
    ) -> Result<String> {
        let encrypt_result = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.encrypt_for_existing_session(recipient, content.as_bytes())
        };

        let encrypted = match encrypt_result {
            Ok(enc) => enc,
            Err(offline_protocol_mls::MlsError::SessionNotFound(_)) => {
                // Session was deleted externally — evict stale cache entry and
                // return SessionNotReady so the send pipeline can queue the
                // message (when store_pending is enabled) rather than dropping it.
                warn!(
                    recipient = %recipient,
                    "Confirmed session missing from MLS storage, evicting cache"
                );
                self.confirmed_sessions.remove(recipient);
                let state = self.establishment_state(recipient)
                    .unwrap_or(crate::EstablishmentState::NoKeyPackage);
                return Err(Error::SessionNotReady(state));
            }
            Err(e) => {
                // Transient error (crypto, storage I/O, etc.) — do NOT evict
                // cache, the session likely still exists.
                return Err(Error::EncryptFailed(format!(
                    "encryption operation failed: {}",
                    e
                )));
            }
        };

        let serialized =
            serde_json::to_string(&encrypted).map_err(|e| Error::Serialization(e.to_string()))?;
        self.emit_mls_encryption_used(recipient);
        Ok(format!("{}{}", internal_prefixes::ENCRYPTED, serialized))
    }

    pub(super) fn encrypt_content_for_recipient_strict(
        &mut self,
        recipient: &str,
        content: &str,
    ) -> Result<String> {
        let mls = self.mls_manager.clone().ok_or_else(|| {
            self.emit_mls_session_missing(
                Some(recipient),
                None,
                MlsOperationContext::SessionLookup,
                MlsErrorCategory::NotInitialized,
            );
            Error::MlsNotInitialized
        })?;

        // Fast path: if session is confirmed (in-memory cache), skip has_session() I/O
        if self.confirmed_sessions.contains(recipient) {
            return self.encrypt_confirmed_session(&mls, recipient, content);
        }

        let has_session = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.has_session(recipient)?
        };

        if !has_session {
            self.try_load_key_package_from_storage_into_memory(recipient);
            let now_ms = Utc::now().timestamp_millis() as u64;
            let has_valid_key_package = match self.pending_key_packages.get(recipient) {
                Some(pkg) if now_ms < pkg.local_expires_at_ms => true,
                Some(_) => {
                    self.pending_key_packages.remove(recipient);
                    self.delete_peer_key_package_from_storage(recipient);
                    false
                }
                None => false,
            };

            if has_valid_key_package {
                return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
            }

            self.emit_mls_session_missing(
                Some(recipient),
                None,
                MlsOperationContext::SessionLookup,
                MlsErrorCategory::SessionStateMissing,
            );
            return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
        }

        if !self.is_session_confirmed(recipient)? {
            return Err(Error::SessionNotReady(self.establishment_state(recipient)?));
        }

        let encrypted = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager
                .encrypt_for_user(recipient, content.as_bytes())
                .map_err(|_| Error::EncryptFailed("encryption operation failed".to_string()))?
        };

        let serialized =
            serde_json::to_string(&encrypted).map_err(|e| Error::Serialization(e.to_string()))?;
        self.emit_mls_encryption_used(recipient);
        Ok(format!("{}{}", internal_prefixes::ENCRYPTED, serialized))
    }

    pub(super) fn prepare_outbound_content(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        reply_to_msg_id: Option<MessageId>,
        reconciliation_reason: &'static str,
    ) -> Result<OutboundSendPreparation> {
        if self.should_auto_encrypt() {
            if self.config.encryption.require_encryption {
                match self.encrypt_content_for_recipient_strict(recipient, content) {
                    Ok(encrypted) => return Ok(OutboundSendPreparation::Ready(encrypted)),
                    Err(Error::SessionNotReady(_)) if self.config.encryption.store_pending => {
                        // Session not ready but store_pending is enabled — queue
                        // the message so it gets encrypted and sent once the
                        // session is confirmed, rather than dropping it.
                        let queued_id = self.queue_message_for_session_establishment(
                            recipient,
                            content,
                            priority,
                            reply_to_msg_id,
                            reconciliation_reason,
                        )?;
                        return Ok(OutboundSendPreparation::Queued(queued_id));
                    }
                    Err(e) => return Err(e),
                }
            }

            match self.encrypt_content_for_recipient(recipient, content, priority) {
                Ok(encrypted) => Ok(OutboundSendPreparation::Ready(encrypted)),
                Err(Error::SessionNotReady(state)) => {
                    if !self.config.encryption.store_pending {
                        return Err(Error::SessionNotReady(state));
                    }

                    let queued_id = self.queue_message_for_session_establishment(
                        recipient,
                        content,
                        priority,
                        reply_to_msg_id,
                        reconciliation_reason,
                    )?;
                    Ok(OutboundSendPreparation::Queued(queued_id))
                }
                Err(e) => Err(e),
            }
        } else if self.config.encryption.require_encryption {
            Err(Error::EncryptFailed(
                "MLS encryption is required but MLS is not initialized".to_string(),
            ))
        } else {
            Ok(OutboundSendPreparation::Ready(content.to_string()))
        }
    }

    // ========================================================================
    // PENDING / FLUSH
    // ========================================================================

    pub(super) fn queue_message_for_session_establishment(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        reply_to_msg_id: Option<MessageId>,
        reconciliation_reason: &'static str,
    ) -> Result<MessageId> {
        // Generate an ID without ticking the Lamport clock.
        // The real tick happens when flush_pending_messages re-sends
        // via send_message after the session is established.
        let message_id = MessageId::new();

        debug!(
            recipient = %recipient,
            message_id = %message_id,
            "Message queued pending session establishment"
        );
        self.queue_pending_message(
            recipient,
            content,
            priority,
            message_id.clone(),
            reply_to_msg_id,
        );
        self.kick_pending_session_reconciliation(reconciliation_reason);
        if self.has_terminal_welcome_failure(recipient) {
            self.abort_pending_session_for_peer(
                recipient,
                crate::events::WelcomeReasonCode::RetryExhausted,
            );
            return Err(Error::Other(format!(
                "Welcome delivery failed for {}",
                recipient
            )));
        }

        Ok(message_id)
    }

    /// Queues a message with a specific message ID for later sending when session is established.
    pub(super) fn queue_pending_message(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        message_id: MessageId,
        reply_to_msg: Option<MessageId>,
    ) {
        let message_id_str = message_id.as_str().to_string();
        let pending = PendingMessage {
            content: content.to_string(),
            priority,
            message_id,
            reply_to_msg,
            queued_at: Utc::now(),
        };

        // Push to in-memory queue first, then persist (the in-memory queue
        // is the source of truth; storage is a crash-recovery backup).
        self.pending_encrypted_messages
            .entry(recipient.to_string())
            .or_default()
            .push(pending);

        self.persist_pending_messages_for_recipient(recipient);

        debug!(recipient = %recipient, message_id = %message_id_str, "Queued message pending session establishment");
    }

    /// Flushes pending messages for a recipient after session is established.
    pub(super) fn flush_pending_messages(&mut self, recipient: &str) -> Result<()> {
        if let Some(pending) = self.pending_encrypted_messages.remove(recipient) {
            info!(recipient = %recipient, count = pending.len(), "Flushing pending messages");
            let mut remaining = Vec::new();

            for msg in pending {
                // Re-attempt to send each pending message
                // Use the stored message ID by passing reply_to_msg if it exists
                let reply_to_str = msg.reply_to_msg.as_ref().map(|id| id.as_str().to_string());
                match self.send_message(
                    recipient,
                    msg.content.clone(),
                    Some(msg.priority),
                    reply_to_str,
                ) {
                    Ok(id) => {
                        // Note: The new message will have a new ID, but the original ID was already returned to the caller
                        debug!(original_id = %msg.message_id, new_id = %id, "Sent pending message");
                    }
                    Err(e) => {
                        warn!(original_id = %msg.message_id, error = %e, "Failed to send pending message");
                        remaining.push(msg);
                    }
                }
            }

            if remaining.is_empty() {
                self.clear_pending_messages_from_storage(recipient);
            } else {
                self.persist_pending_messages_snapshot(recipient, &remaining);
                self.pending_encrypted_messages
                    .insert(recipient.to_string(), remaining);
            }
        }
        Ok(())
    }

    pub(super) fn flush_restored_confirmed_pending_messages(&mut self) {
        let recipients: Vec<String> = self.pending_encrypted_messages.keys().cloned().collect();

        for recipient in recipients {
            match self.is_session_confirmed(&recipient) {
                Ok(true) => {
                    if let Err(err) = self.flush_pending_messages(&recipient) {
                        warn!(
                            recipient = %recipient,
                            error = %err,
                            "Failed to flush restored pending messages for confirmed session"
                        );
                    }
                }
                Ok(false) => {}
                Err(err) => {
                    warn!(
                        recipient = %recipient,
                        error = %err,
                        "Failed to read session confirmation state while restoring pending messages"
                    );
                }
            }
        }
    }

    // ========================================================================
    // MEDIA
    // ========================================================================

    /// Sends a media attachment (image, video, audio, file, etc.) to a recipient.
    ///
    /// The file data is chunked and each chunk is sent as an individual message
    /// with `content_type: FileChunk`. The first chunk carries the full
    /// `MediaMetadata` so the receiver can display a preview before all chunks
    /// arrive. Individual chunk messages require ACKs and participate in retry
    /// logic so delivery is tracked and recoverable per chunk. `MediaSent` is
    /// emitted only after all chunks are ACKed.
    ///
    /// Returns a `file_id` that can be used to track progress or cancel.
    pub fn send_media(
        &mut self,
        recipient: impl Into<String>,
        file_data: Vec<u8>,
        file_name: impl Into<String>,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
    ) -> Result<String> {
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let recipient_str: String = recipient.into();
        let file_name_str: String = file_name.into();

        // Prevent sending media to blocked users. Blocking is bidirectional:
        // we neither receive from nor send to a blocked peer.
        if self.is_user_blocked(&recipient_str) {
            return Err(Error::UserBlocked(recipient_str));
        }

        let file_id = format!("file_{}", MessageId::new().as_str());
        let pinned_transport = self.select_media_transport()?;

        let (chunk_size, window_size) = match pinned_transport {
            TransportType::BLE => {
                use crate::constants::{CHUNK_SIZE_BLE, MEDIA_WINDOW_SIZE_BLE};
                (CHUNK_SIZE_BLE, MEDIA_WINDOW_SIZE_BLE)
            }
            TransportType::Internet => {
                use crate::constants::{CHUNK_SIZE_INTERNET, MEDIA_WINDOW_SIZE_INTERNET};
                (CHUNK_SIZE_INTERNET, MEDIA_WINDOW_SIZE_INTERNET)
            }
            TransportType::WiFiDirect => {
                use crate::constants::{DEFAULT_CHUNK_SIZE, DEFAULT_MEDIA_WINDOW_SIZE};
                (DEFAULT_CHUNK_SIZE, DEFAULT_MEDIA_WINDOW_SIZE)
            }
        };
        let chunks = self.file_transfer_manager.chunk_file(
            file_id.clone(),
            file_name_str,
            file_data,
            Some(chunk_size),
        )?;

        let total_chunks = chunks.len() as u32;
        self.outbound_media_transfers.insert(
            file_id.clone(),
            OutboundMediaTransfer {
                content_type,
                recipient: recipient_str.clone(),
                pinned_transport,
                total_chunks,
                delivered_chunks: HashSet::new(),
                last_updated_at: Instant::now(),
                media_metadata: media_metadata.clone(),
            },
        );

        let mut window_state = OutboundTransferState::new(chunks, window_size);
        let initial_batch = window_state.next_chunks_to_send();
        self.outbound_media_windows
            .insert(file_id.clone(), window_state);

        let state = lock_shared_state(&self.shared_state)?;
        state.emit_event(Event::file_progress(file_id.clone(), 0, total_chunks));
        drop(state);

        self.send_media_chunk_batch(
            &file_id,
            initial_batch,
            &recipient_str,
            pinned_transport,
            content_type,
            media_metadata.as_ref(),
        )?;

        Ok(file_id)
    }

    /// Sends a batch of file chunks, wiring each into the outbox and media tracking.
    pub(super) fn send_media_chunk_batch(
        &mut self,
        file_id: &str,
        chunks: Vec<FileChunk>,
        recipient: &str,
        pinned_transport: TransportType,
        content_type: ContentType,
        media_metadata: Option<&MediaMetadata>,
    ) -> Result<()> {
        for chunk in chunks {
            let chunk_index = chunk.chunk_index;
            let binary_payload = chunk.to_bytes();

            let meta_for_chunk = if chunk_index == 0 {
                media_metadata.cloned()
            } else {
                None
            };

            let mut message = self.create_media_message(
                recipient,
                String::new(),
                ContentType::FileChunk,
                meta_for_chunk,
            )?;
            message.binary_content = Some(binary_payload);

            if chunk_index == 0 {
                use crate::constants::ORIGINAL_CONTENT_TYPE_KEY;
                message.metadata.insert(
                    ORIGINAL_CONTENT_TYPE_KEY.to_string(),
                    content_type.to_string(),
                );
            }
            self.outbound_media_chunks
                .insert(message.id.clone(), (file_id.to_string(), chunk_index));

            let previous_transport = self.transport_manager.current_transport();
            let send_result = self
                .transport_manager
                .send_via_transport(&message, pinned_transport);
            let current_transport = Some(pinned_transport);

            match send_result {
                Ok(()) => {
                    self.handle_send_success(&message, current_transport)?;
                    self.emit_transport_switch_event(previous_transport, current_transport)?;
                }
                Err(err) => {
                    self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                    // send_via_transport does not record retry failures internally.
                    self.transport_manager
                        .record_retry_failure(pinned_transport);
                    warn!(
                        file_id = %file_id,
                        chunk_index = chunk_index,
                        transport = ?pinned_transport,
                        error = %err,
                        "File chunk send failed, message deferred"
                    );
                }
            }
            if !self.outbound_media_transfers.contains_key(file_id) {
                return Err(Error::Other(format!(
                    "Media transfer {} could not be scheduled for reliable delivery",
                    file_id
                )));
            }
        }
        Ok(())
    }

    /// Pumps all active windowed media transfers, sending the next batch of
    /// chunks for any transfer whose window has capacity (previous chunks ACKed).
    /// Should be called from the periodic tick/poll loop.
    pub(super) fn pump_media_transfers(&mut self) {
        let file_ids: Vec<String> = self.outbound_media_windows.keys().cloned().collect();

        for file_id in file_ids {
            let transfer = match self.outbound_media_transfers.get(&file_id) {
                Some(t) => t.clone(),
                None => {
                    self.outbound_media_windows.remove(&file_id);
                    continue;
                }
            };

            let window = match self.outbound_media_windows.get_mut(&file_id) {
                Some(w) => w,
                None => continue,
            };

            if !window.has_capacity() {
                continue;
            }

            let batch = window.next_chunks_to_send();
            if batch.is_empty() {
                continue;
            }

            if let Err(err) = self.send_media_chunk_batch(
                &file_id,
                batch,
                &transfer.recipient,
                transfer.pinned_transport,
                transfer.content_type,
                transfer.media_metadata.as_ref(),
            ) {
                warn!(
                    file_id = %file_id,
                    error = %err,
                    "Failed to pump media transfer chunks"
                );
            }
        }
    }

    /// Creates a message carrying media content (file chunks, etc.).
    ///
    /// Like `create_message` but sets `content_type`, `media_metadata`, marks
    /// the message as internet-preferred via metadata, and requires per-chunk ACKs.
    pub(super) fn create_media_message(
        &mut self,
        recipient: &str,
        content: impl Into<String>,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
    ) -> Result<Message> {
        use crate::constants::{TRANSPORT_PREFERENCE_INTERNET, TRANSPORT_PREFERENCE_KEY};

        let sender = UserId::new(&self.config.user_id)?;
        let recipient = UserId::new(recipient)?;
        let app_id = AppId::new(&self.config.app_id)?;

        let clock_value = self.lamport_clock.tick();
        self.persist_lamport_clock();

        let mut builder = Message::builder(sender, recipient, app_id)
            .content(content)
            .content_type(content_type)
            .priority(MessagePriority::Medium)
            .ttl(TTL::new(self.config.initial_ttl)?)
            .lamport_clock(clock_value)
            .metadata(TRANSPORT_PREFERENCE_KEY, TRANSPORT_PREFERENCE_INTERNET)
            .requires_ack(true);

        if let Some(meta) = media_metadata {
            builder = builder.media_metadata(meta);
        }

        Ok(builder.build())
    }

    // ========================================================================
    // OUTBOX
    // ========================================================================

    pub(super) fn ensure_outbox_entry(&mut self, message: &Message) {
        if !message.requires_ack {
            return;
        }

        let is_media = Self::is_media_outbox_message(message);
        let (outbox, capacity) = if is_media {
            use crate::constants::MAX_MEDIA_OUTBOX_ENTRIES;
            (&mut self.media_outbox, MAX_MEDIA_OUTBOX_ENTRIES)
        } else {
            (&mut self.outbox, MAX_OUTBOX_ENTRIES)
        };

        if !outbox.contains_key(&message.id) && outbox.len() >= capacity {
            if let Some((oldest_id, last_transport)) = outbox
                .iter()
                .min_by_key(|(_, entry)| entry.last_sent_at)
                .map(|(id, entry)| (id.clone(), entry.last_transport))
            {
                if let Some(transport) = last_transport {
                    self.transport_manager.record_delivery_failure(transport);
                }
                let outbox = if is_media {
                    &mut self.media_outbox
                } else {
                    &mut self.outbox
                };
                outbox.remove(&oldest_id);
                self.handle_outbound_media_chunk_failed(&oldest_id, "outbox eviction");
            }
        }

        let outbox = if is_media {
            &mut self.media_outbox
        } else {
            &mut self.outbox
        };
        outbox
            .entry(message.id.clone())
            .or_insert_with(|| OutboxEntry {
                message: message.clone(),
                attempt_count: 0,
                first_sent_at: Utc::now(),
                last_sent_at: Utc::now(),
                last_transport: None,
            });
    }

    pub(super) fn mark_message_sent(
        &mut self,
        message: &Message,
        transport: Option<TransportType>,
        attempt_hint: Option<u32>,
    ) {
        if !message.requires_ack {
            return;
        }

        let now = Utc::now();
        let outbox = if Self::is_media_outbox_message(message) {
            &mut self.media_outbox
        } else {
            &mut self.outbox
        };
        let entry = outbox
            .entry(message.id.clone())
            .or_insert_with(|| OutboxEntry {
                message: message.clone(),
                attempt_count: 0,
                first_sent_at: now,
                last_sent_at: now,
                last_transport: transport,
            });

        entry.message = message.clone();
        if entry.attempt_count == 0 {
            entry.first_sent_at = now;
        }
        entry.attempt_count = attempt_hint.unwrap_or(entry.attempt_count.saturating_add(1));
        entry.last_sent_at = now;
        entry.last_transport = transport;
    }

    pub(super) fn remove_outbox_entry(&mut self, message_id: &MessageId) -> Option<OutboxEntry> {
        self.outbox
            .remove(message_id)
            .or_else(|| self.media_outbox.remove(message_id))
    }

    pub(super) fn cleanup_outbox(&mut self) {
        let cutoff = Utc::now()
            - ChronoDuration::milliseconds(
                self.config.reliability.retry.outbox_max_lifetime_ms as i64,
            );

        let mut expired_from_outbox = Vec::new();
        for (message_id, entry) in &self.outbox {
            if entry.last_sent_at >= cutoff {
                continue;
            }
            if entry.message.requires_ack && self.ack_manager.is_waiting_for_ack(&entry.message.id)
            {
                continue;
            }
            expired_from_outbox.push((message_id.clone(), entry.last_transport));
        }
        for (message_id, last_transport) in expired_from_outbox {
            if let Some(transport) = last_transport {
                self.transport_manager.record_delivery_failure(transport);
            }
            self.outbox.remove(&message_id);
            self.handle_outbound_media_chunk_failed(&message_id, "outbox lifetime exceeded");
        }

        let mut expired_from_media = Vec::new();
        for (message_id, entry) in &self.media_outbox {
            if entry.last_sent_at >= cutoff {
                continue;
            }
            if entry.message.requires_ack && self.ack_manager.is_waiting_for_ack(&entry.message.id)
            {
                continue;
            }
            expired_from_media.push((message_id.clone(), entry.last_transport));
        }
        for (message_id, last_transport) in expired_from_media {
            if let Some(transport) = last_transport {
                self.transport_manager.record_delivery_failure(transport);
            }
            self.media_outbox.remove(&message_id);
            self.handle_outbound_media_chunk_failed(&message_id, "outbox lifetime exceeded");
        }
    }

    pub(super) fn ensure_ack_registration(&mut self, message: &Message) -> Result<bool> {
        if !message.requires_ack {
            return Ok(false);
        }

        if self.ack_manager.is_waiting_for_ack(&message.id) {
            Ok(false)
        } else {
            self.ack_manager
                .register_pending_ack(message.id.clone(), None)?;
            Ok(true)
        }
    }

    pub(super) fn is_media_outbox_message(message: &Message) -> bool {
        message.content_type == ContentType::FileChunk
    }

    // ========================================================================
    // DELIVERY TRACKING
    // ========================================================================

    /// Handles successful message send.
    pub(super) fn handle_send_success(
        &mut self,
        message: &Message,
        transport: Option<TransportType>,
    ) -> Result<()> {
        self.mark_message_sent(message, transport, Some(1));
        self.ensure_ack_registration(message)?;
        Ok(())
    }

    /// Handles failed message send by persisting to outbox and scheduling retry.
    ///
    /// EDGE CASE HANDLING:
    /// - Ensures message is persisted to outbox for recovery
    /// - Schedules retry with exponential backoff
    /// - Handles case where all transports are unavailable
    ///
    /// NOTE: This does NOT call `record_retry_failure` — callers that need it
    /// (e.g. `send_via_forced_transport`) must record the failure themselves.
    /// `TransportManager::send()` already records failures internally, so
    /// calling it here would double-count.
    pub(super) fn handle_send_failure(
        &mut self,
        message: &Message,
        transport: Option<TransportType>,
    ) -> Result<()> {
        // Ensure message is persisted to outbox for recovery
        self.ensure_outbox_entry(message);

        // Schedule retry. If queuing fails, treat this as a terminal failure.
        if let Err(e) = self.retry_queue.enqueue(message.clone(), 0) {
            warn!(
                message_id = %message.id,
                error = %e,
                "Failed to enqueue message for retry"
            );
            if message.content_type == ContentType::FileChunk {
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::message_failed(
                        message.id.clone(),
                        "Retry queue unavailable".to_string(),
                        0,
                    ));
                }
                self.handle_outbound_media_chunk_failed(&message.id, "retry queue unavailable");
                self.remove_outbox_entry(&message.id);
            }
        }

        warn!(
            message_id = %message.id,
            transport = ?transport,
            "Deferred message due to send error"
        );
        Ok(())
    }

    /// Confirms that a message was successfully sent by the transport layer.
    pub fn on_transport_send_confirmed(&mut self, message_id: &str) -> Result<()> {
        let Some(peer_id) = self.find_welcome_peer_by_message_id(message_id) else {
            return Ok(());
        };

        let updated = self
            .welcome_lifecycles
            .get(&peer_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("Missing welcome lifecycle for {}", peer_id)))?;

        if matches!(
            updated.state,
            WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
        ) {
            return Ok(());
        }

        {
            let record = self.welcome_lifecycles.get_mut(&peer_id).ok_or_else(|| {
                Error::Other(format!("Missing welcome lifecycle for {}", peer_id))
            })?;
            record.next_retry_at = None;
            record.last_reason_code = None;
            record.last_transport_error = None;
        }
        self.transition_welcome_state(&peer_id, WelcomeDeliveryState::Sent, "transport_confirmed")?;

        let sent_snapshot = self
            .welcome_lifecycles
            .get(&peer_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("Missing welcome lifecycle for {}", peer_id)))?;
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::welcome_send_succeeded(
                peer_id,
                sent_snapshot.welcome_message.id.as_str().to_string(),
                sent_snapshot.group_id,
                sent_snapshot.attempt,
            ));
        }
        Ok(())
    }

    /// Handles asynchronous transport send failures for pending welcome sends.
    pub fn on_transport_send_failed(
        &mut self,
        message_id: &str,
        transport_error: Option<String>,
    ) -> Result<()> {
        let Some(peer_id) = self.find_welcome_peer_by_message_id(message_id) else {
            return Ok(());
        };
        let reason = crate::events::WelcomeReasonCode::TransportUnavailable;
        let _ =
            self.apply_welcome_send_failure(&peer_id, reason, transport_error, "transport_failed")?;
        Ok(())
    }

    pub(super) fn handle_outbound_media_chunk_delivered(&mut self, message_id: &MessageId) {
        let Some((file_id, chunk_index)) = self.outbound_media_chunks.remove(message_id) else {
            return;
        };

        if let Some(window) = self.outbound_media_windows.get_mut(&file_id) {
            window.on_chunk_ack(chunk_index);
        }

        let Some(transfer) = self.outbound_media_transfers.get_mut(&file_id) else {
            return;
        };

        transfer.delivered_chunks.insert(chunk_index);
        transfer.last_updated_at = Instant::now();

        let delivered_chunks = transfer.delivered_chunks.len() as u32;
        let total_chunks = transfer.total_chunks;
        let content_type = transfer.content_type;
        let recipient = transfer.recipient.clone();
        let completed = delivered_chunks == total_chunks;

        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::file_progress(
                file_id.clone(),
                delivered_chunks,
                total_chunks,
            ));
            if completed {
                state.emit_event(Event::media_sent(file_id.clone(), content_type, recipient));
            }
        }

        if completed {
            self.outbound_media_transfers.remove(&file_id);
            self.outbound_media_windows.remove(&file_id);
        }
    }

    pub(super) fn handle_outbound_media_chunk_failed(
        &mut self,
        message_id: &MessageId,
        reason: &str,
    ) {
        let Some((file_id, _chunk_index)) = self.outbound_media_chunks.remove(message_id) else {
            return;
        };

        if self.outbound_media_transfers.remove(&file_id).is_some() {
            self.outbound_media_chunks
                .retain(|_, (candidate_file_id, _)| candidate_file_id != &file_id);
            self.outbound_media_windows.remove(&file_id);
            warn!(
                file_id = %file_id,
                message_id = %message_id,
                reason = %reason,
                "Aborting outbound media transfer after terminal chunk failure"
            );
        }
    }

    pub(super) fn cleanup_stale_media_state(&mut self, max_age: StdDuration) {
        let now = Instant::now();

        self.pending_media_metadata
            .retain(|_, metadata| now.duration_since(metadata.last_updated_at) <= max_age);

        let stale_outbound_file_ids: HashSet<String> = self
            .outbound_media_transfers
            .iter()
            .filter_map(|(file_id, transfer)| {
                if now.duration_since(transfer.last_updated_at) > max_age {
                    return Some(file_id.clone());
                }
                None
            })
            .collect();

        if stale_outbound_file_ids.is_empty() {
            return;
        }

        self.outbound_media_transfers
            .retain(|file_id, _| !stale_outbound_file_ids.contains(file_id));
        self.outbound_media_chunks
            .retain(|_, (file_id, _)| !stale_outbound_file_ids.contains(file_id));
        self.outbound_media_windows
            .retain(|file_id, _| !stale_outbound_file_ids.contains(file_id));
    }

    // ========================================================================
    // ACK
    // ========================================================================

    pub(super) fn send_delivery_ack(
        &mut self,
        message: &Message,
        inbound_transport: TransportType,
    ) -> Result<()> {
        let sender = UserId::new(&self.config.user_id)?;
        let recipient = message.sender.clone();
        let app_id = AppId::new(&self.config.app_id)?;
        let ttl = TTL::new(self.config.initial_ttl).unwrap_or_else(|_| TTL::default());

        let ack_message = Message::builder(sender, recipient, app_id)
            .content(String::new())
            .priority(MessagePriority::Low)
            .ttl(ttl)
            .requires_ack(false)
            .metadata(ACK_FOR_KEY, message.id.as_str())
            .metadata(ACK_HOP_COUNT_KEY, message.hop_count.value().to_string())
            .metadata(ACK_TRANSPORT_KEY, Self::transport_label(inbound_transport))
            .build();

        // Try sending ACK via the same transport that received the message first.
        // This is the preferred path as it's known to work for this peer.
        // If the inbound transport is no longer available (e.g., internet disconnected),
        // fall back to DORS selection to try any available transport.
        if self
            .transport_manager
            .send_via_transport(&ack_message, inbound_transport)
            .is_ok()
        {
            return Ok(());
        }

        // Fallback: try any available transport via DORS
        debug!(
            message_id = %message.id,
            inbound_transport = ?inbound_transport,
            "Inbound transport unavailable for ACK, falling back to DORS selection"
        );
        self.transport_manager.send(&ack_message)
    }

    pub(super) fn handle_ack_message(&mut self, message: &Message) {
        if let Some(ack_for) = message.metadata.get(ACK_FOR_KEY) {
            if let Ok(message_id) = MessageId::from_str(ack_for) {
                if let Some(pending) = self.ack_manager.remove_ack(&message_id) {
                    let latency = Utc::now()
                        .signed_duration_since(pending.sent_at)
                        .num_milliseconds()
                        .max(0) as u64;

                    let hop_count = message
                        .metadata
                        .get(ACK_HOP_COUNT_KEY)
                        .and_then(|v| v.parse::<u8>().ok())
                        .unwrap_or(0);

                    let transport = message
                        .metadata
                        .get(ACK_TRANSPORT_KEY)
                        .map(|label| Self::transport_from_label(label))
                        .unwrap_or(TransportType::BLE);

                    // Emit event if we can lock the state, but don't fail if we can't
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::message_delivered(
                            message_id.clone(),
                            latency,
                            hop_count,
                            transport,
                        ));
                        drop(state);
                    } else {
                        error!(
                            "Failed to lock shared state for ACK event, skipping event emission"
                        );
                    }

                    self.transport_manager.reset_retry_count(transport);
                    self.transport_manager.record_delivery_success(
                        transport,
                        latency.min(u32::MAX as u64) as u32,
                        hop_count,
                    );
                    self.handle_outbound_media_chunk_delivered(&message_id);
                    self.remove_outbox_entry(&message_id);
                }
            }
        }
    }

    pub(super) fn handle_missing_outbox_entry(
        &mut self,
        message_id: &MessageId,
        retry_count: u32,
    ) -> Result<()> {
        let state = lock_shared_state(&self.shared_state).map_err(|e| {
            error!(
                "Failed to lock shared state for missing outbox entry event: {}",
                e
            );
            e
        })?;
        state.emit_event(Event::message_failed(
            message_id.clone(),
            "Message missing from outbox (cannot retry)".to_string(),
            retry_count,
        ));
        drop(state);

        self.handle_outbound_media_chunk_failed(message_id, "missing outbox entry");
        self.ack_manager.remove_ack(message_id);
        Ok(())
    }

    // ========================================================================
    // EVENTS
    // ========================================================================

    /// Emits a message sent event.
    pub(super) fn emit_message_sent_event(&self, message: &Message) -> Result<()> {
        let state = lock_shared_state(&self.shared_state).map_err(|e| {
            error!("Failed to lock shared state for message sent event: {}", e);
            e
        })?;
        state.emit_event(Event::message_sent(message));
        drop(state);
        Ok(())
    }

    /// Emits a transport switched event if the transport changed.
    pub(super) fn emit_transport_switch_event(
        &self,
        previous_transport: Option<TransportType>,
        current_transport: Option<TransportType>,
    ) -> Result<()> {
        if current_transport != previous_transport {
            if let Some(new_transport) = current_transport {
                let state = lock_shared_state(&self.shared_state).map_err(|e| {
                    error!(
                        "Failed to lock shared state for transport switch event: {}",
                        e
                    );
                    e
                })?;
                state.emit_event(Event::transport_switched(
                    previous_transport,
                    new_transport,
                    "DORS selected better transport".to_string(),
                ));
                drop(state);
            }
        }
        Ok(())
    }

    // ========================================================================
    // SOCIAL
    // ========================================================================

    /// Sends a connection request to another user via any available transport.
    ///
    /// The request is routed through DORS, so it works over Internet, BLE, or WiFi Direct.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The user ID of the recipient
    /// * `sender_name` - Display name of the sender
    /// * `key_package` - Optional MLS key package for encrypted session setup
    ///
    /// # Encryption
    ///
    /// Connection requests are internal control messages sent in plaintext,
    /// exempt from `require_encryption` (same as key packages and welcome messages).
    pub fn send_connection_request(
        &mut self,
        recipient: &str,
        sender_name: &str,
        key_package: Option<Vec<u8>>,
    ) -> Result<MessageId> {
        // Connection requests are internal control messages (not user content),
        // so they are exempt from require_encryption — same as key packages.
        if self.is_user_blocked(recipient) {
            return Err(Error::UserBlocked(recipient.to_string()));
        }

        let payload = ConnectionRequestPayload {
            sender_name: sender_name.to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            key_package,
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::CONN_REQUEST, serialized);

        let message_id = self.send_internal_message(recipient, content, MessagePriority::High)?;
        info!(recipient = %recipient, "Sent connection request");
        Ok(message_id)
    }

    /// Accepts a connection request from another user via any available transport.
    ///
    /// The response is routed through DORS, so it works over Internet, BLE, or WiFi Direct.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The user ID of the original requester
    /// * `accepter_name` - Display name of the accepting party
    /// * `key_package` - Optional MLS key package for encrypted session setup
    ///
    /// # Encryption
    ///
    /// Connection accepts are internal control messages sent in plaintext,
    /// exempt from `require_encryption` (same as key packages and welcome messages).
    pub fn accept_connection_request(
        &mut self,
        recipient: &str,
        accepter_name: &str,
        key_package: Option<Vec<u8>>,
    ) -> Result<MessageId> {
        // Accept messages are internal control messages (not user content),
        // so they are exempt from require_encryption — same as key packages.
        if self.is_user_blocked(recipient) {
            return Err(Error::UserBlocked(recipient.to_string()));
        }

        let payload = ConnectionAcceptedPayload {
            accepted_by_name: accepter_name.to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            key_package,
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::CONN_ACCEPT, serialized);

        let message_id = self.send_internal_message(recipient, content, MessagePriority::High)?;
        info!(recipient = %recipient, "Accepted connection request");
        Ok(message_id)
    }

    /// Rejects a connection request from another user via any available transport.
    ///
    /// The response is routed through DORS, so it works over Internet, BLE, or WiFi Direct.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The user ID of the original requester
    ///
    /// # Encryption
    ///
    /// Connection rejects are internal control messages sent in plaintext,
    /// exempt from `require_encryption` (same as key packages and welcome messages).
    pub fn reject_connection_request(&mut self, recipient: &str) -> Result<MessageId> {
        // Reject messages are internal control messages (not user content),
        // so they are exempt from require_encryption — same as key packages.
        if self.is_user_blocked(recipient) {
            return Err(Error::UserBlocked(recipient.to_string()));
        }

        let content = internal_prefixes::CONN_REJECT.to_string();

        let message_id = self.send_internal_message(recipient, content, MessagePriority::High)?;
        info!(recipient = %recipient, "Rejected connection request");
        Ok(message_id)
    }

    /// Sends a presence update to a single peer (unicast).
    ///
    /// This sends to one recipient at a time. To broadcast presence to multiple
    /// peers, the caller must invoke this method once per peer.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The user ID of the peer to notify
    /// * `status` - Presence status
    pub fn send_presence_update(
        &mut self,
        recipient: &str,
        status: PresenceStatus,
    ) -> Result<MessageId> {
        if recipient.is_empty() {
            return Err(Error::Other("recipient must not be empty".to_string()));
        }
        if self.is_user_blocked(recipient) {
            return Err(Error::UserBlocked(recipient.to_string()));
        }

        let payload = PresencePayload {
            status,
            timestamp_ms: Utc::now().timestamp_millis(),
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::PRESENCE, serialized);

        let message_id = self.send_internal_message(recipient, content, MessagePriority::Low)?;
        debug!(recipient = %recipient, status = ?status, "Sent presence update");
        Ok(message_id)
    }

    /// Sends a typing indicator to a peer.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The user ID of the peer to notify
    /// * `conversation_id` - Conversation identifier (recipient's username for DMs, group_id for groups)
    /// * `is_typing` - Whether the user started or stopped typing
    pub fn send_typing_indicator(
        &mut self,
        recipient: &str,
        conversation_id: &str,
        is_typing: bool,
    ) -> Result<MessageId> {
        if recipient.is_empty() {
            return Err(Error::Other("recipient must not be empty".to_string()));
        }
        if self.is_user_blocked(recipient) {
            return Err(Error::UserBlocked(recipient.to_string()));
        }
        if conversation_id.is_empty() {
            return Err(Error::Other(
                "conversation_id must not be empty".to_string(),
            ));
        }

        let payload = TypingIndicatorPayload {
            conversation_id: conversation_id.to_string(),
            is_typing,
            timestamp_ms: Utc::now().timestamp_millis(),
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::TYPING_INDICATOR, serialized);

        let message_id = self.send_internal_message(recipient, content, MessagePriority::Low)?;
        debug!(recipient = %recipient, is_typing = %is_typing, "Sent typing indicator");
        Ok(message_id)
    }

    /// Sends a read receipt to a peer, indicating that the given messages have been read.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The user ID of the peer who sent the messages
    /// * `message_ids` - IDs of the messages that were read (max 256)
    pub fn send_read_receipt(
        &mut self,
        recipient: &str,
        message_ids: Vec<String>,
    ) -> Result<MessageId> {
        if recipient.is_empty() {
            return Err(Error::Other("recipient must not be empty".to_string()));
        }
        if self.is_user_blocked(recipient) {
            return Err(Error::UserBlocked(recipient.to_string()));
        }
        if message_ids.is_empty() {
            return Err(Error::Other("message_ids must not be empty".to_string()));
        }
        if message_ids.len() > MAX_READ_RECEIPT_IDS {
            return Err(Error::Other(format!(
                "message_ids exceeds maximum of {MAX_READ_RECEIPT_IDS}"
            )));
        }

        let payload = ReadReceiptPayload {
            message_ids,
            timestamp_ms: Utc::now().timestamp_millis(),
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::READ_RECEIPT, serialized);

        let message_id = self.send_internal_message(recipient, content, MessagePriority::Low)?;
        debug!(recipient = %recipient, "Sent read receipt");
        Ok(message_id)
    }

    pub(crate) fn send_key_package_to(&mut self, peer_id: &str, session_reset: bool) -> Result<()> {
        let mls = self.mls_manager.as_ref().ok_or(Error::MlsNotInitialized)?;

        let key_pkg = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.get_or_create_key_package()?
        };

        let payload = KeyPackagePayload {
            user_id: self.config.user_id.clone(),
            key_package_data: key_pkg.key_package_data.clone(),
            remaining_lifetime_ms: key_pkg.remaining_lifetime_ms(),
            timestamp_ms: Utc::now().timestamp_millis() as u64,
            session_reset,
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::KEY_PACKAGE, serialized);

        let mut message =
            self.create_message(peer_id, content, Some(MessagePriority::Low), None)?;
        self.sign_control_message(&mut message)?;

        match self.transport_manager.send(&message) {
            Ok(()) => {
                self.key_package_sent_to.insert(peer_id.to_string());
                debug!(peer_id = %peer_id, message_id = %message.id, "Sent key package");
                Ok(())
            }
            Err(err) => {
                // Don't mark as sent and don't enqueue for retry -- if the peer is
                // unreachable now, on_neighbor_discovered will fire again when they
                // reconnect, generating a fresh exchange.
                debug!(peer_id = %peer_id, error = %err, "Key package send deferred");
                Err(err)
            }
        }
    }

    // ========================================================================
    // TRANSPORT HELPERS
    // ========================================================================

    pub(super) fn transport_from_label(label: &str) -> TransportType {
        TransportType::from_label(label)
    }

    pub(super) fn transport_label(transport: TransportType) -> &'static str {
        transport.label()
    }

    pub(super) fn select_media_transport(&self) -> Result<TransportType> {
        let available = self.transport_manager.get_available_transports();

        if let Some(current) = self.transport_manager.current_transport() {
            if available.contains_key(&current) {
                return Ok(current);
            }
        }

        for preferred in [
            TransportType::Internet,
            TransportType::WiFiDirect,
            TransportType::BLE,
        ] {
            if available.contains_key(&preferred) {
                return Ok(preferred);
            }
        }

        Err(Error::Other(
            "No available transport for media transfer".to_string(),
        ))
    }

    pub(super) fn pinned_media_transport_for_message(
        &self,
        message_id: &MessageId,
    ) -> Option<TransportType> {
        let (file_id, _) = self.outbound_media_chunks.get(message_id)?;
        self.outbound_media_transfers
            .get(file_id)
            .map(|transfer| transfer.pinned_transport)
    }

    pub(super) fn decryption_failure_code_from_kind(
        kind: DecryptionFailureKind,
    ) -> DecryptionFailureCode {
        match kind {
            DecryptionFailureKind::NotInitialized => DecryptionFailureCode::NotInitialized,
            DecryptionFailureKind::InvalidCiphertext => DecryptionFailureCode::InvalidCiphertext,
            DecryptionFailureKind::IdentityMismatch => DecryptionFailureCode::IdentityMismatch,
            DecryptionFailureKind::CryptoFailure => DecryptionFailureCode::CryptoFailure,
            DecryptionFailureKind::SessionNotFound | DecryptionFailureKind::Unknown => {
                DecryptionFailureCode::Unknown
            }
        }
    }
}
