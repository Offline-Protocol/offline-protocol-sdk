//! Send pipeline, outbox management, and delivery tracking.

use super::{
    base64_encode, internal_prefixes, lifetime_expired, lock_shared_state,
    ConnectionAcceptedPayload, ConnectionRequestPayload, KeyPackagePayload, MediaSendOptions,
    MediaTransferDescriptor, OfflineProtocol, OutboundMediaTransfer, OutboundSendPreparation,
    OutboxEntry, OutboxReseal, PendingConnectionRequest, PendingMessage, PendingProvenance,
    PresencePayload, ProtocolState, ReadReceiptPayload, RichPayloadV1, RichSendExtras,
    SendMessageOptions, TypingIndicatorPayload, WelcomeDeliveryState, MAX_INITIAL_MESSAGE_BYTES,
    MAX_KEY_PACKAGE_SENT_TO, MAX_MESSAGE_CONTENT_BYTES, MAX_PENDING_CONNECTION_REQUESTS,
    MAX_PENDING_EXPIRIES_PER_PASS, MAX_PENDING_MESSAGES_GLOBAL, MAX_PENDING_MESSAGES_PER_PEER,
    MAX_PENDING_MESSAGE_BYTES_GLOBAL, MAX_PENDING_MESSAGE_BYTES_PER_PEER, MAX_READ_RECEIPT_IDS,
    MAX_RICH_EXTRAS_BYTES, MLS_ENVELOPE_COMPACT_V1, PENDING_CONNECTION_REQUEST_TTL,
    RICH_PAYLOAD_V1, SEND_FAIL_REASON_RECIPIENT_UNREACHABLE, WELCOME_NO_CARRIER_RETRY_SECS,
    WELCOME_UNREACHABLE_RETRY_CAP_SECS,
};
use crate::constants::{
    ACK_FOR_KEY, ACK_HOP_COUNT_KEY, ACK_TRANSPORT_KEY, MAX_FORWARD_COUNT, MAX_OUTBOX_ENTRIES,
};
use crate::events::{DecryptionFailureCode, Event, PresenceStatus};
use crate::file_transfer::{FileChunk, OutboundTransferState};
use crate::media_envelope::{encode_media_envelope, MediaChunkPlaintext, MediaRichExtras};
use crate::mls_observability::{DecryptionFailureKind, MlsErrorCategory, MlsOperationContext};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use offline_protocol_core::{
    AppId, ContentType, ForwardInfo, MediaMetadata, Message, MessageId, MessagePriority, UserId,
    TTL,
};
use offline_protocol_mls::{EncryptedMessage, MlsManager};
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
        self.send_message_with(
            recipient,
            content,
            SendMessageOptions {
                priority,
                reply_to_msg: reply_to_msg.map(Into::into),
                ..Default::default()
            },
        )
    }

    /// Sends a message with rich options: priority and reply threading (as
    /// on [`Self::send_message`]), plus quoted-reply context, rich media
    /// metadata, and forward attribution.
    ///
    /// The rich fields only ever travel inside the MLS-sealed `__RICH_V1__`
    /// body, and only toward recipients whose key package advertised
    /// `rich_versions` support (gated by
    /// `EncryptionConfig::rich_payload_enabled`). Toward anyone else they
    /// are silently dropped — never sent cleartext — so the message degrades
    /// to plain text with `reply_to_msg` threading intact.
    ///
    /// Returns `InvalidArgument` for `ContentType::FileChunk` (an internal
    /// transport content type — the receiver would swallow the message into
    /// its file-transfer manager) and for rich extras whose serialized size
    /// exceeds 32 KiB (oversized bodies inflate the MLS plaintext into heavy
    /// transport fragmentation).
    pub fn send_message_with(
        &mut self,
        recipient: impl Into<String>,
        content: impl Into<String>,
        options: SendMessageOptions,
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
        let priority = options.priority.unwrap_or(MessagePriority::Medium);
        Self::validate_outbound_recipient(&recipient_str)?;

        // Prevent sending messages to blocked users. Blocking is bidirectional:
        // we neither receive from nor send to a blocked peer.
        if self.is_user_blocked(&recipient_str) {
            return Err(Error::UserBlocked(recipient_str));
        }

        Self::check_content_size(&content_str)?;

        // Reject content that starts with an internal control prefix to prevent
        // injection of protocol-level messages through the public API.
        if Self::is_internal_prefix(&content_str) {
            return Err(Error::InvalidArgument(
                "Message content must not start with a reserved internal prefix".to_string(),
            ));
        }

        // FileChunk is an internal transport content type: the receiver
        // routes a message stamped with it into handle_incoming_file_chunk
        // (after ACKing delivery) where a non-chunk fails to parse and is
        // dropped without surfacing. Reject rather than silently lose it.
        if options.content_type == Some(ContentType::FileChunk) {
            return Err(Error::InvalidArgument(
                "FileChunk is an internal content type and cannot be sent directly".to_string(),
            ));
        }

        // Parse reply_to_msg if provided
        let reply_to_msg_id = options
            .reply_to_msg
            .as_deref()
            .map(MessageId::from_str)
            .transpose()
            .map_err(|e| Error::InvalidArgument(format!("Invalid reply_to_msg: {}", e)))?;

        let rich = RichSendExtras {
            reply_context: options.reply_context,
            media_metadata: options.media_metadata,
            forward_info: options.forward_info,
        };

        // Bound the extras here at the boundary — not at seal time — so the
        // pending queue only ever holds extras that are known to seal: the
        // flush path re-seals against current capability, and a seal-time
        // failure there would re-queue the message forever. The cap keeps an
        // oversized quote or thumbnail from inflating the MLS plaintext into
        // heavy transport fragmentation.
        rich.check_size()?;

        let final_content = match self.prepare_outbound_content(
            &recipient_str,
            &content_str,
            priority,
            reply_to_msg_id.clone(),
            None,
            options.content_type.unwrap_or_default(),
            None,
            Some(&rich),
            None,
            "send_message_session_pending",
        )? {
            OutboundSendPreparation::Ready(content) => content,
            OutboundSendPreparation::Queued(message_id) => return Ok(message_id),
        };

        // Create message with potentially encrypted content. Rich extras are
        // deliberately NOT copied onto the outer message — they are either
        // inside the sealed body by now or dropped; only the coarse
        // content_type rendering hint rides outer.
        let reseal_reply_to = reply_to_msg_id.clone();
        let mut message = self.create_message(
            &recipient_str,
            final_content,
            Some(priority),
            reply_to_msg_id,
        )?;
        if let Some(content_type) = options.content_type {
            message.content_type = content_type;
        }

        // Tier 2: if the send was encrypted, keep the plaintext provenance so
        // each resend re-seals against the peer's current session instead of
        // replaying dead-epoch ciphertext (see `OutboxReseal`).
        if message.content.starts_with(internal_prefixes::ENCRYPTED) {
            self.stage_outbox_reseal(
                &message.id,
                OutboxReseal {
                    content: content_str,
                    priority,
                    reply_to_msg: reseal_reply_to,
                    forwarded_from: None,
                    content_type: message.content_type,
                    media_metadata: None,
                    rich: Some(rich).filter(|r| r.is_any()),
                },
            );
        }

        match options.via_transport {
            Some(transport) => self.dispatch_prepared_message_via(message, transport),
            None => self.dispatch_prepared_message(message),
        }
    }

    /// Forwards a message to a new recipient with attribution to the original sender.
    ///
    /// Creates a new message with the original content and attaches `ForwardInfo`
    /// tracking the original sender, message ID, timestamp, and forward count.
    /// If the message was already forwarded, the original attribution is preserved
    /// and `forward_count` is incremented.
    ///
    /// Toward a recipient that advertised the sealed rich payload, the
    /// attribution and the original `media_metadata` — including any
    /// cloud-media `encryption_key`/`iv` secrets — travel inside the MLS-sealed
    /// `__RICH_V1__` body, so forwarded cloud media stays openable. Toward
    /// anyone else they ride the legacy cleartext outer fields, from which
    /// the transport chokepoint strips the secrets — attribution and display
    /// hints survive, but the media key cannot be delivered.
    ///
    /// # Arguments
    ///
    /// * `original_message` - The message to forward
    /// * `new_recipient` - Recipient's user ID
    /// * `priority` - Message priority (optional, defaults to Medium)
    ///
    /// # Returns
    ///
    /// Returns the new message ID if successful.
    pub fn forward_message(
        &mut self,
        original_message: &Message,
        new_recipient: impl Into<String>,
        priority: Option<MessagePriority>,
    ) -> Result<MessageId> {
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let recipient_str: String = new_recipient.into();
        let priority = priority.unwrap_or(MessagePriority::Medium);
        Self::validate_outbound_recipient(&recipient_str)?;

        if self.is_user_blocked(&recipient_str) {
            return Err(Error::UserBlocked(recipient_str));
        }

        // Reject content that starts with an internal control prefix to prevent
        // injection of protocol-level messages through the forwarding API.
        if Self::is_internal_prefix(&original_message.content) {
            return Err(Error::InvalidArgument(
                "Cannot forward a message with reserved internal prefix content".to_string(),
            ));
        }

        // FileChunk is an internal transport content type (see
        // `send_message_with`): a forwarded message stamped with it would be
        // ACKed and then dropped by the receiver's file-transfer manager.
        if original_message.content_type == ContentType::FileChunk {
            return Err(Error::InvalidArgument(
                "FileChunk is an internal content type and cannot be forwarded".to_string(),
            ));
        }

        // Build ForwardInfo: preserve original attribution, increment count
        let forward_info = ForwardInfo::from_message(original_message);

        if forward_info.forward_count > MAX_FORWARD_COUNT {
            return Err(Error::InvalidArgument(format!(
                "Forward count {} exceeds maximum of {}",
                forward_info.forward_count, MAX_FORWARD_COUNT,
            )));
        }

        // Sealed-path extras: toward a rich-capable recipient the attribution
        // and the original media metadata (including cloud-media key/iv
        // secrets, which only the sealed body may carry — the wire chokepoint
        // strips them from the cleartext outer field) travel inside the
        // `__RICH_V1__` body. The outer copies set below remain the fallback
        // for non-capable recipients; the receiver's sealed-body restore is
        // authoritative and overwrites them when the seal happened.
        let rich = RichSendExtras {
            reply_context: None,
            media_metadata: original_message.media_metadata.clone(),
            forward_info: Some(forward_info.clone()),
        };
        // Same boundary caps as `send_message_with` — enforced here, not at
        // seal time, so a forward queued behind session establishment is
        // always known to seal at flush and always fits the queue's budget.
        Self::check_content_size(&original_message.content)?;
        rich.check_size()?;

        // Prepare content (may encrypt). ForwardInfo is threaded through so
        // it survives the pending-message queue if the session isn't ready.
        let final_content = match self.prepare_outbound_content(
            &recipient_str,
            &original_message.content,
            priority,
            None,
            Some(forward_info.clone()),
            original_message.content_type,
            original_message.media_metadata.clone(),
            Some(&rich),
            None,
            "forward_message_session_pending",
        )? {
            OutboundSendPreparation::Ready(content) => content,
            OutboundSendPreparation::Queued(message_id) => return Ok(message_id),
        };

        // Create and dispatch the forwarded message
        let mut message =
            self.create_message(&recipient_str, final_content, Some(priority), None)?;
        message.forwarded_from = Some(forward_info);
        message.content_type = original_message.content_type;
        message.media_metadata = original_message.media_metadata.clone();

        // Tier 2 provenance (see `send_message_with`): re-seal forwarded encrypted
        // DMs against the peer's current session on resend.
        if message.content.starts_with(internal_prefixes::ENCRYPTED) {
            self.stage_outbox_reseal(
                &message.id,
                OutboxReseal {
                    content: original_message.content.clone(),
                    priority,
                    reply_to_msg: None,
                    forwarded_from: message.forwarded_from.clone(),
                    content_type: message.content_type,
                    media_metadata: message.media_metadata.clone(),
                    rich: Some(rich).filter(|r| r.is_any()),
                },
            );
        }

        self.dispatch_prepared_message(message)
    }

    /// Shared send path: dedup, transport send, success/failure handling, event emission.
    ///
    /// Used by `forward_message`, `flush_pending_messages` (for forwarded messages),
    /// and any path that has a fully-constructed `Message` ready to send.
    fn dispatch_prepared_message(&mut self, message: Message) -> Result<MessageId> {
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
                self.emit_transport_switch_event(previous_transport, current_transport)?;
                self.emit_message_sent_event(&message)?;
                Ok(message_id)
            }
            Err(err) => {
                let next_retry_at =
                    self.handle_send_failure(&message, current_transport.or(previous_transport))?;
                warn!(
                    message_id = %message.id,
                    error = %err,
                    "Send failed, message deferred"
                );
                self.emit_event(Event::message_deferred(
                    message.id.clone(),
                    format!("Transport send failed: {}", err),
                    0,
                    next_retry_at.map(|at| at.timestamp_millis()),
                ));
                Ok(message_id)
            }
        }
    }

    /// [`Self::dispatch_prepared_message`] variant that sends via a specific
    /// transport (bypassing DORS) — the shared tail for
    /// `send_message_via_transport` and rich sends carrying `via_transport`.
    /// Unlike `TransportManager::send`, `send_via_transport` does not record
    /// retry failures internally, so the failure arm records one explicitly.
    fn dispatch_prepared_message_via(
        &mut self,
        message: Message,
        transport: TransportType,
    ) -> Result<MessageId> {
        let message_id = message.id.clone();

        if self.deduplicator.is_duplicate(&message_id) {
            return Err(crate::Error::Other("Duplicate message".to_string()));
        }

        self.deduplicator.mark_seen(message_id.clone());

        let previous_transport = self.transport_manager.current_transport();
        let send_result = self
            .transport_manager
            .send_via_transport(&message, transport);
        let current_transport = Some(transport);

        match send_result {
            Ok(()) => {
                self.handle_send_success(&message, current_transport)?;
                self.emit_transport_switch_event(previous_transport, current_transport)?;
                self.emit_message_sent_event(&message)?;
                Ok(message_id)
            }
            Err(err) => {
                let next_retry_at =
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
                self.emit_event(Event::message_deferred(
                    message.id.clone(),
                    format!("Send via {:?} failed: {}", transport, err),
                    0,
                    next_retry_at.map(|at| at.timestamp_millis()),
                ));
                Ok(message_id)
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
                let _ =
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

    /// Sends a self-addressed relay *hint* frame — `__GRP_RELAY_REG__` or
    /// `__GRP_RELAY_BCAST__` — pinned to the Internet transport, without the
    /// ACK ladder.
    ///
    /// A hint frame is not traffic. The platform bridge recognizes it via
    /// [`Self::internet_control_op`] and **replaces** it with a relay-native
    /// frame (`CreateGroup` + member deltas, or `SendGroupMessage`), so the
    /// frame itself never reaches a peer and no delivery ACK can ever come
    /// back for it. Two properties follow, and both are load-bearing:
    ///
    /// 1. **`requires_ack = false`.** On the ordinary ladder an un-ACKable
    ///    frame is retransmitted `DEFAULT_MAX_RETRIES` times over ~800s, each
    ///    resend producing another full relay fan-out (under a fresh
    ///    relay-minted id, so receiver dedup misses it), and terminates in a
    ///    `MessageFailed` for an id the app was never told about plus a
    ///    `record_delivery_failure` that degrades DORS' score for the very
    ///    transport the frame requires. Opting out of the ladder is what makes
    ///    these frames honest one-shots; retry policy for them lives at the
    ///    application layer instead (`RelayRegisterPending` for registration,
    ///    the caller's per-member fallback for broadcast).
    /// 2. **Pinned to [`TransportType::Internet`].** These frames are
    ///    self-addressed, and DORS demotes Internet below every mesh transport
    ///    (`INTERNET_FALLBACK_DEMOTION`), so ordinary routing hands them to
    ///    BLE/Wi-Fi Direct first. BLE fails closed (self is never a connected
    ///    peer), but Wi-Fi Direct and Reticulum enqueue unconditionally and
    ///    return `Ok` — swallowing the frame while reporting success, which on
    ///    the broadcast path means the group message is delivered to nobody.
    ///
    /// Errors propagate to the caller rather than routing through
    /// [`Self::handle_send_failure`]: a hint frame has no outbox entry to
    /// defer into, and its callers already have a better recovery than a
    /// retry (fall back to per-member fan-out; leave the group unregistered
    /// for the next sync tick). The failure is also deliberately *not*
    /// recorded against Internet's DORS metrics — "the relay bridge isn't up"
    /// is not evidence about the transport's delivery quality.
    ///
    /// No `TransportChanged` event is emitted, matching
    /// [`Self::send_delivery_ack`] — the other fire-and-forget control frame
    /// sent over a forced transport. A hint frame is not user traffic, and
    /// announcing a switch for one would report a transport change the app's
    /// messages did not take.
    pub(crate) fn send_relay_hint_message(
        &mut self,
        content: String,
        priority: MessagePriority,
    ) -> Result<MessageId> {
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let self_id = self.config.user_id.clone();
        let mut message = self.create_message(&self_id, content, Some(priority), None)?;
        // Set before signing for clarity; the canonical payload covers only
        // sender/id/recipient/content, so ordering is not security-relevant.
        message.requires_ack = false;
        self.sign_control_message(&mut message)?;
        let message_id = message.id.clone();

        if self.deduplicator.is_duplicate(&message_id) {
            return Err(crate::Error::Other("Duplicate message".to_string()));
        }

        // Mark seen so a bridge that passes the frame through verbatim (an
        // adapter without a translator) cannot have the relay echo it back
        // into our own receive path.
        self.deduplicator.mark_seen(message_id.clone());

        self.transport_manager
            .send_via_transport(&message, TransportType::Internet)?;

        Ok(message_id)
    }

    /// Sends one MLS-encrypted session-confirm marker to `peer_id`.
    ///
    /// Used by the Welcome-adopt path: after we adopt the peer's `session:` group
    /// (and confirm our own side), the peer may be the both-create "owner" that
    /// kept its own group and confirms ONLY on a group-aware decrypt from us. A
    /// plaintext probe/ack does not count, and we may have no user traffic to
    /// send — so without this the owner stays Pending forever and the 1:1
    /// connection never completes. The marker is `SESSION_CONFIRM_ENCRYPTED`
    /// wrapped in the normal `ENCRYPTED` envelope; the peer consumes it on decrypt
    /// (never surfaced). Mirrors [`Self::send_internal_message`] but encrypts
    /// (data-plane, so NOT signed) and never propagates errors — it is re-sent on
    /// every received Welcome while still adopting, so a lost confirm is retried
    /// in lockstep with the owner's Welcome retransmission.
    pub(super) fn send_session_confirm_encrypted(&mut self, peer_id: &str) {
        let encrypted = match self.encrypt_content_for_recipient(
            peer_id,
            internal_prefixes::SESSION_CONFIRM_ENCRYPTED,
            MessagePriority::High,
        ) {
            Ok(content) => content,
            Err(err) => {
                debug!(
                    peer = %peer_id,
                    error = %err,
                    "Skipped proactive session-confirm (encryption not ready); welcome retry remains"
                );
                return;
            }
        };

        let message =
            match self.create_message(peer_id, encrypted, Some(MessagePriority::High), None) {
                Ok(message) => message,
                Err(err) => {
                    warn!(peer = %peer_id, error = %err, "Failed to build session-confirm message");
                    return;
                }
            };

        if self.deduplicator.is_duplicate(&message.id) {
            return;
        }
        self.deduplicator.mark_seen(message.id.clone());

        let previous_transport = self.transport_manager.current_transport();
        match self.transport_manager.send(&message) {
            Ok(()) => {
                let current_transport = self.transport_manager.current_transport();
                let _ = self.handle_send_success(&message, current_transport);
                info!(peer = %peer_id, "Sent proactive encrypted session-confirm to owner");
            }
            Err(err) => {
                let current_transport = self.transport_manager.current_transport();
                let _ =
                    self.handle_send_failure(&message, current_transport.or(previous_transport));
                debug!(
                    peer = %peer_id,
                    error = %err,
                    "Proactive session-confirm deferred (will retry with welcome)"
                );
            }
        }
    }

    /// Sends a message via a specific transport type (bypassing DORS).
    pub fn send_message_via_transport(
        &mut self,
        recipient: impl Into<String>,
        content: impl Into<String>,
        priority: Option<MessagePriority>,
        transport: TransportType,
        reply_to_msg: Option<impl Into<String>>,
    ) -> Result<MessageId> {
        self.send_message_with(
            recipient,
            content,
            SendMessageOptions {
                priority,
                reply_to_msg: reply_to_msg.map(Into::into),
                via_transport: Some(transport),
                ..Default::default()
            },
        )
    }

    /// Rejects a recipient token that is not a well-formed [`UserId`].
    ///
    /// Enforced at every outbound boundary so an app-owned placeholder (an
    /// `unresolved:token`, an empty string) cannot become durable protocol
    /// state — an outbox entry or pending-queue record that is retried, stored,
    /// and restored forever against an address no transport can ever resolve.
    pub(crate) fn validate_outbound_recipient(recipient: &str) -> Result<()> {
        UserId::new(recipient)
            .map(|_| ())
            .map_err(|error| Error::InvalidArgument(format!("Invalid recipient user ID: {error}")))
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
        let encrypted = self.encrypt_bytes_for_recipient(recipient, content.as_bytes())?;
        self.seal_encrypted_content(recipient, &encrypted)
    }

    /// Bytes-level core of [`Self::encrypt_content_for_recipient`]: initiates
    /// session establishment when a key package is available and encrypts once
    /// the session is confirmed. Text content and media chunk plaintexts share
    /// this path.
    pub(super) fn encrypt_bytes_for_recipient(
        &mut self,
        recipient: &str,
        plaintext: &[u8],
    ) -> Result<EncryptedMessage> {
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
            return self.encrypt_bytes_confirmed_session(&mls, recipient, plaintext);
        }

        self.ensure_session_establishment(&mls, recipient)?;

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
                .encrypt_for_user(recipient, plaintext)
                .map_err(|_| Error::EncryptFailed("encryption operation failed".to_string()))?
        };

        self.emit_mls_encryption_used(recipient);
        Ok(encrypted)
    }

    /// Ensures MLS session establishment with `recipient` has at least been
    /// initiated: when no session exists in storage, imports a stored/pending
    /// key package, creates the session, and sends the Welcome.
    ///
    /// Returns `Ok(())` when a session already exists (possibly unconfirmed —
    /// callers must still gate on `is_session_confirmed`) or when no usable
    /// key package remains (expired). Returns `Err(SessionNotReady)` when no
    /// key package is available, or right after creating a session with
    /// `store_pending` enabled (the session cannot be confirmed yet).
    pub(super) fn ensure_session_establishment(
        &mut self,
        mls: &Arc<RwLock<MlsManager>>,
        recipient: &str,
    ) -> Result<()> {
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
                        let trust = self.key_package_trust(recipient);
                        let manager = mls
                            .read()
                            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                        manager.import_key_package(
                            recipient,
                            &received_pkg.key_package_data,
                            trust,
                        )?;
                    }

                    // Create session and send welcome message
                    let welcome = {
                        let manager = mls
                            .read()
                            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                        manager.create_session(recipient)?
                    };
                    self.mark_encryption_capable(recipient);

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

        Ok(())
    }

    /// Serializes an [`EncryptedMessage`] into the prefixed string form used
    /// by the text path.
    ///
    /// Recipients that advertised [`MLS_ENVELOPE_COMPACT_V1`] in their key
    /// package (with our own `compact_envelope_enabled` on) get the compact
    /// envelope — `__MLS_ENC__` + base64 of [`EncryptedMessage::to_bytes`] —
    /// roughly 2.7x smaller than the legacy JSON form, whose `ciphertext`
    /// field renders as an integer array. Everyone else gets `__MLS_ENC__` +
    /// JSON, the permanent floor. Receivers sniff the byte after the prefix
    /// (`{` = JSON), so no per-message signaling is needed, and the choice is
    /// per-recipient end-to-end state, valid however the message is later
    /// routed or retried.
    pub(super) fn seal_encrypted_content(
        &self,
        recipient: &str,
        encrypted: &EncryptedMessage,
    ) -> Result<String> {
        if self.config.encryption.compact_envelope_enabled
            && self.peer_compact_envelope.contains(recipient)
        {
            return Ok(format!(
                "{}{}",
                internal_prefixes::ENCRYPTED,
                base64_encode(&encrypted.to_bytes())
            ));
        }
        let serialized =
            serde_json::to_string(encrypted).map_err(|e| Error::Serialization(e.to_string()))?;
        Ok(format!("{}{}", internal_prefixes::ENCRYPTED, serialized))
    }

    /// Whether rich extras seal for `recipient`: our own kill switch is on
    /// and the peer advertised [`RICH_PAYLOAD_V1`] in their key package.
    pub(super) fn rich_seal_active(&self, recipient: &str) -> bool {
        self.config.encryption.rich_payload_enabled && self.peer_rich_payload.contains(recipient)
    }

    /// Whether rich extras may seal into a group message: our own kill
    /// switch is on and every non-self member is known to parse
    /// [`RICH_PAYLOAD_V1`] — either self-advertised in a directly received
    /// key package (`peer_rich_payload`) or attested by a group inviter on
    /// the Add commit / Welcome (`peer_rich_attested`; members added by
    /// someone else never exchange key packages with us directly). Group
    /// MLS encryption produces a single ciphertext for all members, so one
    /// non-capable or capability-unknown member forces the extras to drop
    /// for the whole group. Conservative by design: an unknown member must
    /// never receive a sealed body their SDK would render as literal
    /// `__RICH_V1__` JSON.
    ///
    /// Attestation is third-party and may be stale or forged by a rogue
    /// admin; the worst case is exactly the degraded display below, never a
    /// leak, and direct contact evicts the attested entry. Best-effort
    /// beyond that: callers pass the `group_mesh.members` fan-out cache,
    /// which trails MLS membership until the add/remove notification
    /// lands. In that window a just-added non-capable member can receive
    /// one sealed body and render it as literal JSON — degraded display,
    /// not a leak (every group member is entitled to the sealed contents).
    pub(crate) fn group_rich_seal_active(&self, members: &[String]) -> bool {
        self.config.encryption.rich_payload_enabled
            && members
                .iter()
                .filter(|m| m.as_str() != self.config.user_id)
                .all(|m| {
                    self.peer_rich_payload.contains(m.as_str())
                        || self.peer_rich_attested.contains(m.as_str())
                })
    }

    /// Group members (non-self) not known to parse the sealed rich payload
    /// — neither directly advertised nor inviter-attested. These are the
    /// members that hold `group_rich_seal_active` closed; the set absence
    /// cannot distinguish "never heard from" from "known not capable".
    pub(crate) fn group_rich_unknown_members(&self, members: &[String]) -> Vec<String> {
        members
            .iter()
            .filter(|m| {
                m.as_str() != self.config.user_id
                    && !self.peer_rich_payload.contains(m.as_str())
                    && !self.peer_rich_attested.contains(m.as_str())
            })
            .cloned()
            .collect()
    }

    /// The rich-payload versions we can attest for a peer when inviting it
    /// into (or welcoming it to) a group: [`RICH_PAYLOAD_V1`] when the peer
    /// is known capable (directly or itself attested — attestation chains
    /// transitively, which is how knowledge reaches members several adds
    /// removed from any direct exchange), `None` when unknown. `None` is
    /// deliberately not a downgrade signal: absence of knowledge must never
    /// evict what another member learned first-hand.
    pub(crate) fn attestable_rich_versions(&self, peer_id: &str) -> Option<Vec<u8>> {
        (self.peer_rich_payload.contains(peer_id) || self.peer_rich_attested.contains(peer_id))
            .then(|| vec![RICH_PAYLOAD_V1])
    }

    /// Best-effort capability backfill for group members whose rich support
    /// is unknown (pre-attestation groups, or an attestation chain broken by
    /// an old-SDK inviter): send them our key package so their
    /// `auto_key_exchange` reply teaches us theirs. Guarded by
    /// `key_package_sent_to`, so repeated gate-failing group sends don't
    /// re-probe the same peer; a peer that never replies is either
    /// unreachable, old-SDK, or opted out — all of which correctly leave
    /// the gate closed.
    pub(crate) fn backfill_group_rich_capabilities(&mut self, unknown_members: &[String]) {
        for member in unknown_members {
            if self.key_package_sent_to.contains(member.as_str()) {
                continue;
            }
            if let Err(e) = self.send_key_package_to(member, false) {
                debug!(
                    member = %member,
                    error = %e,
                    "Rich-capability backfill key package deferred (peer unreachable)"
                );
            }
        }
    }

    /// Wraps plaintext content and rich extras into the sealed
    /// `__RICH_V1__`-prefixed JSON body ([`RichPayloadV1`]). Callers must
    /// only invoke this on a path that MLS-encrypts the result — the sealed
    /// body is the sole carrier for reply context and media secrets and must
    /// never leave as cleartext.
    ///
    /// The outer `content_type` hint is always copied into the body (not
    /// just when non-Text) so the receiver can treat the sealed copy as
    /// authoritative and a relay cannot rewrite the hint in either
    /// direction on a rich message.
    pub(crate) fn seal_rich_payload(
        content: &str,
        extras: &RichSendExtras,
        content_type: ContentType,
    ) -> Result<String> {
        let payload = RichPayloadV1 {
            text: content.to_string(),
            reply_context: extras.reply_context.clone(),
            media_metadata: extras.media_metadata.clone(),
            forward_info: extras.forward_info.clone(),
            content_type: Some(content_type),
        };
        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        Ok(format!("{}{}", internal_prefixes::RICH_V1, serialized))
    }

    /// Encrypts a plaintext for a recipient whose session is known to be
    /// confirmed (in-memory cache hit). Uses `encrypt_for_existing_session` to
    /// skip both the external `has_session()` and the internal one inside
    /// `encrypt_for_user`, reducing storage I/O from 2 round-trips to 1
    /// (`load_group` for encrypt).
    ///
    /// Only evicts the cache on `SessionNotFound` (session deleted externally).
    /// Transient errors (crypto, storage I/O) propagate without cache eviction
    /// so the fast path is preserved for the next attempt.
    pub(super) fn encrypt_bytes_confirmed_session(
        &mut self,
        mls: &Arc<RwLock<MlsManager>>,
        recipient: &str,
        plaintext: &[u8],
    ) -> Result<EncryptedMessage> {
        let encrypt_result = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.encrypt_for_existing_session(recipient, plaintext)
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
                let state = self
                    .establishment_state(recipient)
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

        self.emit_mls_encryption_used(recipient);
        Ok(encrypted)
    }

    pub(super) fn encrypt_content_for_recipient_strict(
        &mut self,
        recipient: &str,
        content: &str,
    ) -> Result<String> {
        let encrypted = self.encrypt_bytes_for_recipient_strict(recipient, content.as_bytes())?;
        self.seal_encrypted_content(recipient, &encrypted)
    }

    /// Bytes-level core of [`Self::encrypt_content_for_recipient_strict`]:
    /// requires an existing confirmed session and never initiates
    /// establishment.
    pub(super) fn encrypt_bytes_for_recipient_strict(
        &mut self,
        recipient: &str,
        plaintext: &[u8],
    ) -> Result<EncryptedMessage> {
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
            return self.encrypt_bytes_confirmed_session(&mls, recipient, plaintext);
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
                .encrypt_for_user(recipient, plaintext)
                .map_err(|_| Error::EncryptFailed("encryption operation failed".to_string()))?
        };

        self.emit_mls_encryption_used(recipient);
        Ok(encrypted)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_outbound_content(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        reply_to_msg_id: Option<MessageId>,
        forwarded_from: Option<ForwardInfo>,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
        rich: Option<&RichSendExtras>,
        provenance: Option<PendingProvenance>,
        reconciliation_reason: &'static str,
    ) -> Result<OutboundSendPreparation> {
        if self.should_auto_encrypt() {
            // Rich extras travel only inside the sealed body: seal for a
            // capable recipient, drop for everyone else — never cleartext.
            // The decision is made here, at actual-send time; a message
            // queued behind session establishment stores the raw extras and
            // re-evaluates capability at flush (the key package that
            // confirms the session may be the one that advertised it).
            let rich_extras = rich.filter(|extras| extras.is_any());
            let sealed_body;
            let outbound: &str = match rich_extras {
                Some(extras) if self.rich_seal_active(recipient) => {
                    sealed_body = Self::seal_rich_payload(content, extras, content_type)?;
                    &sealed_body
                }
                Some(_) => {
                    debug!(
                        recipient = %recipient,
                        "Recipient lacks sealed rich payload capability, dropping rich extras"
                    );
                    content
                }
                // No rich extras, but a non-Text content_type toward a
                // capable recipient still seals a hint-only body: the outer
                // hint is relay-writable, and restamping it FileChunk gets
                // the decrypted message ACKed and then dropped by the
                // file-transfer manager — worse than a plain relay drop,
                // which at least fails the ACK and retries. Only when
                // nothing rides outer (`forwarded_from`/`media_metadata`
                // both absent): fresh forwards thread their attribution and
                // media metadata through `rich` above, so this outer-only
                // shape survives solely in messages queued by an older build
                // (persisted `PendingMessage`s with no `rich`), where the
                // receiver's wholesale sealed-body restore would wipe the
                // outer copies a hint-only seal can't replicate. Bare Text
                // seals nothing: no hint to protect, and the unsealed form
                // keeps the plaintext floor maximal.
                None if content_type != ContentType::Text
                    && forwarded_from.is_none()
                    && media_metadata.is_none()
                    && self.rich_seal_active(recipient) =>
                {
                    sealed_body =
                        Self::seal_rich_payload(content, &RichSendExtras::default(), content_type)?;
                    &sealed_body
                }
                None => content,
            };
            if self.config.encryption.require_encryption {
                match self.encrypt_content_for_recipient_strict(recipient, outbound) {
                    Ok(encrypted) => return Ok(OutboundSendPreparation::Ready(encrypted)),
                    Err(Error::SessionNotReady(_)) if self.config.encryption.store_pending => {
                        // Session not ready but store_pending is enabled — queue
                        // the message so it gets encrypted and sent once the
                        // session is confirmed, rather than dropping it.
                        //
                        // Kick establishment first (import a stored key package,
                        // create the session, send the Welcome), matching the
                        // non-strict and media paths: without this a peer whose
                        // key package arrived but whose arrival-time
                        // auto-establish failed would stall until the peer
                        // initiates. SessionNotReady from the kick is the
                        // expected "created, awaiting confirmation" outcome;
                        // the message queues regardless.
                        if let Some(mls) = self.mls_manager.clone() {
                            match self.ensure_session_establishment(&mls, recipient) {
                                Ok(()) | Err(Error::SessionNotReady(_)) => {}
                                Err(err) => {
                                    warn!(
                                        recipient = %recipient,
                                        error = %err,
                                        "Session establishment kick failed; message queued anyway"
                                    );
                                }
                            }
                        }
                        let queued_id = self.queue_message_for_session_establishment(
                            recipient,
                            content,
                            priority,
                            reply_to_msg_id,
                            forwarded_from,
                            content_type,
                            media_metadata,
                            rich_extras.cloned(),
                            provenance,
                            reconciliation_reason,
                        )?;
                        return Ok(OutboundSendPreparation::Queued(queued_id));
                    }
                    Err(e) => return Err(e),
                }
            }

            match self.encrypt_content_for_recipient(recipient, outbound, priority) {
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
                        forwarded_from,
                        content_type,
                        media_metadata,
                        rich_extras.cloned(),
                        provenance,
                        reconciliation_reason,
                    )?;
                    Ok(OutboundSendPreparation::Queued(queued_id))
                }
                Err(e) => Err(e),
            }
        } else if self.config.encryption.require_encryption {
            Err(Error::EncryptFailed(
                "MLS encryption is required (the default) but MLS is not initialized — \
                 call initialize_mls() with secure and protocol-state storage, or explicitly opt out with \
                 require_encryption=false to send plaintext"
                    .to_string(),
            ))
        } else {
            // Explicit opt-out: require_encryption=false with encryption
            // disabled or MLS uninitialized. The message leaves as plaintext;
            // surface that loudly (once per peer) instead of a debug log.
            // Rich extras are dropped here unconditionally — they only ever
            // travel inside the sealed body, never as cleartext.
            if rich.is_some_and(|extras| extras.is_any()) {
                debug!(
                    recipient = %recipient,
                    "Plaintext send path, dropping rich extras (sealed-only)"
                );
            }
            self.warn_plaintext_send(recipient);
            Ok(OutboundSendPreparation::Ready(content.to_string()))
        }
    }

    // ========================================================================
    // PENDING / FLUSH
    // ========================================================================

    /// Enforces [`MAX_MESSAGE_CONTENT_BYTES`] on application-supplied content.
    ///
    /// Enforced at the public send boundary rather than at transmit time,
    /// because a message that has to wait for MLS session establishment is
    /// queued — in memory and on disk — long before it ever reaches the
    /// transport's own size check. Without this cap a handful of very large
    /// sends could exhaust mobile memory and protocol-state storage while
    /// still satisfying the pending queue's entry-count limits.
    ///
    /// Applies to the plaintext the app hands us. MLS ciphertext, base64, and
    /// the JSON wire envelope expand it further, which is why the cap sits
    /// well under the transport's 1 MiB ceiling: everything accepted here can
    /// actually be delivered.
    ///
    /// Deliberately *not* applied to the group surface: a group send has no
    /// durable pre-session queue behind it (there is no per-recipient MLS
    /// session to wait on), so it is bounded by the transport alone — the
    /// documented "no send-side limit" behavior that
    /// `test_group_mls_send_large_content_no_send_side_limit` pins.
    pub(crate) fn check_content_size(content: &str) -> Result<()> {
        if content.len() > MAX_MESSAGE_CONTENT_BYTES {
            return Err(Error::InvalidArgument(format!(
                "Message content too large: {} bytes (max {}); use send_media for large payloads",
                content.len(),
                MAX_MESSAGE_CONTENT_BYTES
            )));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn queue_message_for_session_establishment(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        reply_to_msg_id: Option<MessageId>,
        forwarded_from: Option<ForwardInfo>,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
        rich: Option<RichSendExtras>,
        provenance: Option<PendingProvenance>,
        reconciliation_reason: &'static str,
    ) -> Result<MessageId> {
        // Fresh sends mint an ID without ticking the Lamport clock — the
        // real tick happens when flush_pending_messages re-sends after the
        // session is established. A flush-time re-queue passes the id the
        // caller already holds so Deferred/Sent/Delivered stay correlatable,
        // and the timestamp it first entered the queue so the absolute
        // pending lifetime is not renewed by the round trip.
        let (message_id, first_queued_at) = match provenance {
            Some(provenance) => (provenance.message_id, provenance.first_queued_at),
            None => (MessageId::default(), None),
        };

        debug!(
            recipient = %recipient,
            message_id = %message_id,
            "Message queued pending session establishment"
        );
        self.queue_pending_message_at(
            recipient,
            content,
            priority,
            message_id.clone(),
            reply_to_msg_id,
            forwarded_from,
            content_type,
            media_metadata,
            rich,
            first_queued_at.unwrap_or_else(Utc::now),
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

    /// Queues a message with a specific message ID for later sending when
    /// session is established, starting a fresh pending lifetime.
    ///
    /// A re-queue must go through [`Self::queue_pending_message_at`] with the
    /// entry's original timestamp instead — which is why production code always
    /// calls that one directly (through
    /// `queue_message_for_session_establishment`, where the distinction is
    /// made) and this convenience exists only for tests.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn queue_pending_message(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        message_id: MessageId,
        reply_to_msg: Option<MessageId>,
        forwarded_from: Option<ForwardInfo>,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
        rich: Option<RichSendExtras>,
    ) {
        self.queue_pending_message_at(
            recipient,
            content,
            priority,
            message_id,
            reply_to_msg,
            forwarded_from,
            content_type,
            media_metadata,
            rich,
            Utc::now(),
        );
    }

    /// Queues a message with an explicit first-queued timestamp.
    ///
    /// `queued_at` is the instant the message *first* entered the pending
    /// queue, which is what `pending_message_max_lifetime_ms` is measured from.
    /// Re-queueing an entry (a flush that found the session still unavailable)
    /// must pass the original value: stamping `now()` there would hand the
    /// message a fresh window on every failed reconciliation, so a message
    /// could outlive an "absolute" lifetime indefinitely.
    ///
    /// Admission is bounded on two axes — entry count and serialized bytes,
    /// per peer and globally — because message content is
    /// application-supplied and a count alone bounds neither memory nor
    /// durable storage. Both axes evict oldest-first and settle each evicted
    /// message as failed.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn queue_pending_message_at(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        message_id: MessageId,
        reply_to_msg: Option<MessageId>,
        forwarded_from: Option<ForwardInfo>,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
        rich: Option<RichSendExtras>,
        queued_at: DateTime<Utc>,
    ) {
        let message_id_str = message_id.as_str().to_string();
        let mut pending = PendingMessage {
            content: content.to_string(),
            priority,
            message_id,
            reply_to_msg,
            forwarded_from,
            content_type,
            media_metadata,
            rich,
            queued_at,
            serialized_bytes: 0,
        };
        pending.measure();
        let incoming_bytes = pending.serialized_bytes;

        let mut evicted = Vec::new();

        while self
            .pending_encrypted_messages
            .get(recipient)
            .is_some_and(|messages| messages.len() >= MAX_PENDING_MESSAGES_PER_PEER)
        {
            if let Some(message) = self.evict_pending_message_at(recipient, 0) {
                evicted.push(message);
            } else {
                break;
            }
        }

        // The running totals are decremented as entries are evicted rather
        // than recomputed each turn, so a queue at capacity does not make
        // admission quadratic.
        let mut peer_bytes = self.pending_message_bytes_for(recipient);
        while peer_bytes.saturating_add(incoming_bytes) > MAX_PENDING_MESSAGE_BYTES_PER_PEER {
            let Some(message) = self.evict_pending_message_at(recipient, 0) else {
                break;
            };
            peer_bytes = peer_bytes.saturating_sub(message.serialized_bytes);
            evicted.push(message);
        }

        while self.total_pending_message_count() >= MAX_PENDING_MESSAGES_GLOBAL {
            let Some((_, message)) = self.evict_oldest_pending_message() else {
                break;
            };
            evicted.push(message);
        }

        let mut global_bytes = self.total_pending_message_bytes();
        while global_bytes.saturating_add(incoming_bytes) > MAX_PENDING_MESSAGE_BYTES_GLOBAL {
            let Some((_, message)) = self.evict_oldest_pending_message() else {
                break;
            };
            global_bytes = global_bytes.saturating_sub(message.serialized_bytes);
            evicted.push(message);
        }

        // Push to in-memory queue first, then persist (the in-memory queue
        // is the source of truth; storage is a crash-recovery backup).
        //
        // One record per message, so this writes the incoming entry and nothing
        // else — the queue it joins is already on disk, entry by entry, and the
        // entries the caps just evicted are deleted by id rather than rewritten
        // around. Which recipients were touched no longer matters.
        self.pending_encrypted_messages
            .entry(recipient.to_string())
            .or_default()
            .push(pending);
        self.note_pending_message_expiry(queued_at);

        if let Some(queued) = self
            .pending_encrypted_messages
            .get(recipient)
            .and_then(|messages| messages.last())
        {
            self.persist_pending_message(recipient, queued);
        }

        for message in evicted {
            self.delete_pending_message_from_storage(&message.message_id);
            self.emit_event(Event::message_failed(
                message.message_id,
                "Pending session queue capacity exceeded".to_string(),
                0,
            ));
        }

        debug!(recipient = %recipient, message_id = %message_id_str, "Queued message pending session establishment");
    }

    pub(super) fn total_pending_message_count(&self) -> usize {
        self.pending_encrypted_messages.values().map(Vec::len).sum()
    }

    /// Total serialized footprint of the pending-session queue across all
    /// peers. Summing precomputed per-entry sizes, so this stays a cheap
    /// integer walk rather than re-serializing the queue.
    pub(super) fn total_pending_message_bytes(&self) -> usize {
        self.pending_encrypted_messages
            .values()
            .flatten()
            .map(|message| message.serialized_bytes)
            .sum()
    }

    /// Serialized footprint of one peer's pending-session queue.
    pub(super) fn pending_message_bytes_for(&self, recipient: &str) -> usize {
        self.pending_encrypted_messages
            .get(recipient)
            .map(|messages| {
                messages
                    .iter()
                    .map(|message| message.serialized_bytes)
                    .sum()
            })
            .unwrap_or(0)
    }

    fn evict_pending_message_at(
        &mut self,
        recipient: &str,
        index: usize,
    ) -> Option<PendingMessage> {
        let (message, remove_recipient) = {
            let messages = self.pending_encrypted_messages.get_mut(recipient)?;
            if index >= messages.len() {
                return None;
            }
            let message = messages.remove(index);
            (message, messages.is_empty())
        };
        if remove_recipient {
            self.pending_encrypted_messages.remove(recipient);
        }
        Some(message)
    }

    pub(super) fn evict_oldest_pending_message(&mut self) -> Option<(String, PendingMessage)> {
        let (recipient, index) = self
            .pending_encrypted_messages
            .iter()
            .flat_map(|(recipient, messages)| {
                messages
                    .iter()
                    .enumerate()
                    .map(move |(index, message)| (recipient, index, message))
            })
            .min_by(|left, right| {
                left.2
                    .queued_at
                    .cmp(&right.2.queued_at)
                    .then_with(|| left.2.message_id.as_str().cmp(&right.2.message_id.as_str()))
            })
            .map(|(recipient, index, _)| (recipient.clone(), index))?;
        let message = self.evict_pending_message_at(&recipient, index)?;
        Some((recipient, message))
    }

    /// Drops a recipient's whole queue from memory *and* from storage, settling
    /// every dropped entry as `MessageFailed`.
    ///
    /// The storage half is a delete per entry rather than one whole-queue
    /// delete, which is the cost side of keying records by message id. Callers
    /// are the paths where every queued message for a peer becomes undeliverable
    /// at once — blocking, an aborted session.
    ///
    /// Settlement happens *here* rather than in the callers on purpose. The
    /// caller holds ids handed to the app by `send_message*` at queue time, so
    /// a silent drop leaves the app waiting forever on ids that will never
    /// resolve — the same contract the blocked-recipient branch of
    /// [`Self::flush_pending_messages`] and the pending-expiry pass already
    /// honour. Returning the messages for the caller to settle is what let all
    /// three call sites discard them instead, so there is nothing to discard
    /// any more.
    pub(super) fn drop_pending_queue_for_peer(&mut self, recipient: &str, reason: &str) -> usize {
        let messages = self
            .pending_encrypted_messages
            .remove(recipient)
            .unwrap_or_default();
        self.delete_pending_messages_from_storage(messages.iter().map(|m| &m.message_id));
        for message in &messages {
            self.emit_event(Event::message_failed(
                message.message_id.clone(),
                reason.to_string(),
                0,
            ));
        }
        self.recompute_next_pending_message_expiry();
        messages.len()
    }

    fn pending_message_expiry(queued_at: DateTime<Utc>, lifetime_ms: u64) -> Option<DateTime<Utc>> {
        let lifetime_ms = i64::try_from(lifetime_ms).ok()?;
        queued_at.checked_add_signed(chrono::Duration::milliseconds(lifetime_ms))
    }

    fn note_pending_message_expiry(&mut self, queued_at: DateTime<Utc>) {
        let Some(expiry) = Self::pending_message_expiry(
            queued_at,
            self.config
                .reliability
                .retry
                .pending_message_max_lifetime_ms,
        ) else {
            return;
        };
        self.next_pending_message_expiry = Some(
            self.next_pending_message_expiry
                .map_or(expiry, |current| current.min(expiry)),
        );
    }

    pub(super) fn recompute_next_pending_message_expiry(&mut self) {
        let lifetime_ms = self
            .config
            .reliability
            .retry
            .pending_message_max_lifetime_ms;
        self.next_pending_message_expiry = self
            .pending_encrypted_messages
            .values()
            .flat_map(|messages| messages.iter())
            .filter_map(|message| Self::pending_message_expiry(message.queued_at, lifetime_ms))
            .min();
    }

    /// Flushes pending messages for a recipient after session is established.
    ///
    /// Every message flushes through one unified path — prepare (re-making
    /// the seal-or-drop decision for rich extras against the recipient's
    /// *current* capability; the key package that confirmed this session may
    /// have advertised it), create, restore outer fields, then dispatch —
    /// and keeps the `message_id` the caller received from `send_message*`
    /// at queue time (no event fires on queueing — the returned id is the
    /// correlation anchor), so that id matches the eventual
    /// `MessageSent`/`MessageDelivered`/`MessageFailed`. A re-queue (session
    /// flapped back to not-ready) keeps the id too.
    pub(super) fn flush_pending_messages(&mut self, recipient: &str) -> Result<()> {
        if let Some(pending) = self.pending_encrypted_messages.remove(recipient) {
            // Blocking is bidirectional (we neither receive from nor send
            // to): a recipient blocked after messages were queued drops the
            // queue outright — retrying would fail with UserBlocked forever.
            if self.is_user_blocked(recipient) {
                info!(
                    recipient = %recipient,
                    count = pending.len(),
                    "Dropping pending messages for blocked recipient"
                );
                // The caller holds these ids (returned by `send_message*`
                // at queue time) — settle each one so the app isn't left
                // waiting on ids that will never resolve.
                for msg in &pending {
                    self.delete_pending_message_from_storage(&msg.message_id);
                    self.emit_event(Event::message_failed(
                        msg.message_id.clone(),
                        "Recipient blocked".to_string(),
                        0,
                    ));
                }
                return Ok(());
            }

            info!(recipient = %recipient, count = pending.len(), "Flushing pending messages");
            let original_order: Vec<String> =
                pending.iter().map(|m| m.message_id.as_str()).collect();
            let mut remaining = Vec::new();
            let now = Utc::now();
            let lifetime_ms = self
                .config
                .reliability
                .retry
                .pending_message_max_lifetime_ms;

            for msg in pending {
                // The absolute pending lifetime is enforced here as well as in
                // the expiry pass, because that pass is bounded per tick
                // (`MAX_PENDING_EXPIRIES_PER_PASS`) — a restore can hand this
                // flush entries already past their deadline, and dispatching
                // one would settle it `MessageSent` after the lifetime promised
                // `message_failed`. Back into `remaining` instead: the entry
                // keeps its record, and the next expiry pass settles and
                // deletes it as a pair.
                if lifetime_expired(now, msg.queued_at, lifetime_ms) {
                    remaining.push(msg);
                    continue;
                }

                // With stable ids the deduplicator now guards against a
                // double flush (e.g. a stale storage snapshot restored after
                // the message already went out): a dedup hit means this id
                // was dispatched before — drop it, don't retry forever.
                if self.deduplicator.is_duplicate(&msg.message_id) {
                    if self.deduplicator.is_exact() {
                        // Exact mode: the hit is authoritative — this id was
                        // already dispatched and (barring an internal error
                        // after the transport accepted it) settled then via
                        // MessageSent or MessageDeferred; re-dispatching
                        // would duplicate on the wire.
                        debug!(
                            message_id = %msg.message_id,
                            "Pending message already dispatched (dedup hit), dropping"
                        );
                    } else {
                        // Bloom mode: the hit may be a false positive for a
                        // message that never went out, and dispatching anyway
                        // would error-loop on the same filter. Settle loudly
                        // so the app can resend under a fresh id instead of
                        // losing the message silently.
                        warn!(
                            message_id = %msg.message_id,
                            "Pending message dropped on probabilistic dedup hit"
                        );
                        self.emit_event(Event::message_failed(
                            msg.message_id.clone(),
                            "Duplicate suppressed at flush (probabilistic dedup; possible false positive)".to_string(),
                            0,
                        ));
                    }
                    // Leaves the queue for good either way, so its record goes
                    // with it. Only the entries that end up back in `remaining`
                    // below keep theirs — those were never deleted, and their
                    // contents have not changed, so re-queuing them costs no
                    // write at all.
                    self.delete_pending_message_from_storage(&msg.message_id);
                    continue;
                }

                let final_content = match self.prepare_outbound_content(
                    recipient,
                    &msg.content,
                    msg.priority,
                    msg.reply_to_msg.clone(),
                    msg.forwarded_from.clone(),
                    msg.content_type,
                    msg.media_metadata.clone(),
                    msg.rich.as_ref(),
                    Some(PendingProvenance::requeued(&msg)),
                    "flush_pending",
                ) {
                    Ok(OutboundSendPreparation::Ready(c)) => c,
                    Ok(OutboundSendPreparation::Queued(message_id)) => {
                        // Re-queued for later (session still not ready),
                        // keeping the original id.
                        debug!(message_id = %message_id, "Pending message re-queued");
                        continue;
                    }
                    Err(e) => {
                        warn!(message_id = %msg.message_id, error = %e, "Failed to prepare pending message");
                        remaining.push(msg);
                        continue;
                    }
                };

                let message = match self.create_message(
                    recipient,
                    final_content,
                    Some(msg.priority),
                    msg.reply_to_msg.clone(),
                ) {
                    Ok(mut message) => {
                        // Restore outer fields: forward attribution and its
                        // media_metadata ride outer as the fallback for
                        // non-capable recipients (secrets stripped at the
                        // wire); when `msg.rich` sealed above, the receiver's
                        // sealed-body restore overwrites these wholesale.
                        // Plain sends store None for both — their rich
                        // copies are sealed or dropped by prepare above.
                        message.forwarded_from = msg.forwarded_from.clone();
                        message.content_type = msg.content_type;
                        message.media_metadata = msg.media_metadata.clone();
                        message.id = msg.message_id.clone();
                        message
                    }
                    Err(e) => {
                        warn!(message_id = %msg.message_id, error = %e, "Failed to create pending message");
                        remaining.push(msg);
                        continue;
                    }
                };

                // Tier 2 provenance: a flushed pending message that sealed
                // carries its plaintext forward so later resends re-seal against
                // the peer's current session (see `send_message_with`).
                if message.content.starts_with(internal_prefixes::ENCRYPTED) {
                    self.stage_outbox_reseal(
                        &message.id,
                        OutboxReseal {
                            content: msg.content.clone(),
                            priority: msg.priority,
                            reply_to_msg: msg.reply_to_msg.clone(),
                            forwarded_from: msg.forwarded_from.clone(),
                            content_type: msg.content_type,
                            media_metadata: msg.media_metadata.clone(),
                            rich: msg.rich.clone(),
                        },
                    );
                }

                match self.dispatch_prepared_message(message) {
                    Ok(id) => {
                        debug!(message_id = %id, "Sent pending message");
                        // Handed off to the outbox, which persists its own
                        // record under the same id — so the pending copy has to
                        // go, or a restart would restore and re-flush it.
                        self.delete_pending_message_from_storage(&id);
                    }
                    Err(e) => {
                        warn!(message_id = %msg.message_id, error = %e, "Failed to send pending message");
                        remaining.push(msg);
                    }
                }
            }

            // Messages that re-queued above re-inserted themselves into the
            // map (and rewrote their own record) — merge failures in without
            // clobbering the entry. Nothing to clear when neither survives: each
            // entry that left the queue already took its own record with it.
            if remaining.is_empty() {
                return Ok(());
            }
            if self.has_terminal_welcome_failure(recipient) {
                // A prepare failure above came from a terminal Welcome
                // failure: abort_pending_session_for_peer already cleared
                // the queue and its storage and settled the peer with
                // secure_session_failed — re-inserting `remaining` would
                // resurrect messages the abort just settled, on every
                // future flush, forever.
                //
                // The abort could only delete the records of entries still in
                // the map, and this flush took the queue out of it at the top,
                // so these records are still on disk and have to go here or the
                // next launch restores exactly what the abort settled.
                debug!(
                    recipient = %recipient,
                    count = remaining.len(),
                    "Dropping flush failures for aborted pending session"
                );
                for msg in &remaining {
                    self.delete_pending_message_from_storage(&msg.message_id);
                }
            } else {
                // Every other path into `pending_encrypted_messages` notes the
                // entry's deadline. This one is a *re*-insertion of entries the
                // queue already held, so today the cached minimum is still
                // correct without it — but only because nothing recomputes that
                // cache while a flush is in progress. Noting them keeps
                // "`next_pending_message_expiry` is never later than the true
                // minimum" an invariant of the code rather than of the call
                // graph: if it ever broke, these entries would stop being
                // scanned and would never expire, silently, on the one queue the
                // absolute pending lifetime exists to bound.
                let merged_deadlines: Vec<DateTime<Utc>> =
                    remaining.iter().map(|m| m.queued_at).collect();
                let queue = self
                    .pending_encrypted_messages
                    .entry(recipient.to_string())
                    .or_default();
                queue.extend(remaining);
                // A failure that preceded a mid-flush re-queue would land
                // behind it — restore the original queue order (stable
                // sort; every id here came from this flush's queue).
                queue.sort_by_key(|m| {
                    original_order
                        .iter()
                        .position(|id| *id == m.message_id.as_str())
                        .unwrap_or(usize::MAX)
                });
                for queued_at in merged_deadlines {
                    self.note_pending_message_expiry(queued_at);
                }
                // No write: these entries never left the queue durably. Their
                // records were written at enqueue, were not deleted above, and
                // their contents have not changed — only their position in the
                // in-memory queue, which the restore ordering rebuilds from
                // `queued_at` rather than from storage order.
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
    /// # Encryption
    ///
    /// With auto-encryption active (encryption enabled and MLS initialized),
    /// chunk bytes — and the chunk-0 media metadata and original content type —
    /// travel inside MLS ciphertext, using the same session the text path uses.
    /// The session must already be confirmed: media is never queued pending
    /// establishment and never falls back to plaintext. When the session is
    /// not ready this returns [`Error::SessionNotReady`] (after kicking
    /// establishment if a key package is available); retry after the
    /// `secure_session_established` event. When encryption is required but MLS
    /// is not initialized this returns [`Error::EncryptFailed`]. Plaintext
    /// chunks are sent only when auto-encryption is inactive AND
    /// `require_encryption` was explicitly set to `false` (it defaults to
    /// `true`): the explicit encryption opt-out, or encryption enabled but
    /// MLS never initialized (matching the text path). Every plaintext
    /// transfer emits a
    /// [`crate::events::SecurityWarningCode::PlaintextSend`] warning, once
    /// per peer.
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
        self.send_media_with(
            recipient,
            file_data,
            file_name,
            content_type,
            MediaSendOptions {
                media_metadata,
                ..Default::default()
            },
        )
    }

    /// Sends a media attachment with rich options: the chunk-0 media
    /// metadata (as on [`Self::send_media`]), plus caption, reply threading,
    /// quoted-reply context, forward attribution, and an optional
    /// caller-supplied `file_id`.
    ///
    /// The rich fields only ever travel inside the MLS-sealed chunk-0
    /// plaintext (under the v2 media envelope), and only toward recipients
    /// whose key package advertised `rich_versions` support (gated by
    /// `EncryptionConfig::rich_payload_enabled`). Toward anyone else —
    /// including every plaintext (encryption opt-out) transfer — they are
    /// silently dropped, never sent cleartext.
    ///
    /// Returns `InvalidArgument` for an unparsable `reply_to_msg`, rich
    /// extras whose serialized size exceeds 32 KiB, or a `file_id` that is
    /// empty, over the wire field bound, or already carried by an active
    /// outbound transfer. Encryption preconditions match
    /// [`Self::send_media`].
    pub fn send_media_with(
        &mut self,
        recipient: impl Into<String>,
        file_data: Vec<u8>,
        file_name: impl Into<String>,
        content_type: ContentType,
        options: MediaSendOptions,
    ) -> Result<String> {
        {
            let state = lock_shared_state(&self.shared_state)?;
            if state.state != ProtocolState::Running {
                return Err(Error::NotStarted);
            }
        }

        let recipient_str: String = recipient.into();
        let file_name_str: String = file_name.into();
        let media_metadata = options.media_metadata;
        Self::validate_outbound_recipient(&recipient_str)?;

        // Validate the reply id like send_message_with does — a malformed id
        // fails the call instead of riding sealed to the receiver.
        if let Some(reply_to) = options.reply_to_msg.as_deref() {
            MessageId::from_str(reply_to)
                .map_err(|e| Error::InvalidArgument(format!("Invalid reply_to_msg: {}", e)))?;
        }

        let rich_extras = MediaRichExtras {
            caption: options.caption,
            reply_to_msg: options.reply_to_msg,
            reply_context: options.reply_context,
            forward_info: options.forward_info,
        };
        // Same boundary cap as send_message_with: an oversized quote or
        // caption would inflate the sealed chunk-0 plaintext into heavy
        // fragmentation. Enforced before the capability gate so the caller
        // hears about it even when the extras would be dropped.
        if rich_extras.is_any() {
            let extras_len = serde_json::to_vec(&rich_extras)
                .map_err(|e| Error::Serialization(e.to_string()))?
                .len();
            if extras_len > MAX_RICH_EXTRAS_BYTES {
                return Err(Error::InvalidArgument(format!(
                    "Rich extras too large: {} bytes serialized (max {})",
                    extras_len, MAX_RICH_EXTRAS_BYTES
                )));
            }
        }

        if let Some(file_id) = options.file_id.as_deref() {
            if file_id.is_empty() || file_id.len() > FileChunk::MAX_STRING_FIELD_LEN {
                return Err(Error::InvalidArgument(format!(
                    "file_id length must be 1..={} bytes",
                    FileChunk::MAX_STRING_FIELD_LEN
                )));
            }
            if self.outbound_media_transfers.contains_key(file_id) {
                return Err(Error::InvalidArgument(format!(
                    "file_id {} already has an active outbound transfer",
                    file_id
                )));
            }
            // A restored descriptor under this file_id means this send is
            // the app answering MediaResendRequired: it must target the
            // original recipient (a redirect would consume the descriptor
            // and orphan the interrupted transfer's resend forever) and
            // re-supply the original bytes — the receiver may already hold
            // chunks of the interrupted attempt, and a silent content swap
            // under the same id would fail its integrity check at best.
            if let Some(descriptor) = self.restored_media_descriptors.get(file_id) {
                if descriptor.recipient != recipient_str {
                    return Err(Error::InvalidArgument(format!(
                        "file_id {} belongs to an interrupted transfer to a different recipient",
                        file_id
                    )));
                }
                use sha2::{Digest, Sha256};
                let resupplied_checksum = format!("{:x}", Sha256::digest(&file_data));
                if resupplied_checksum != descriptor.file_checksum {
                    return Err(Error::InvalidArgument(format!(
                        "file_id {} resend bytes do not match the interrupted transfer's checksum",
                        file_id
                    )));
                }
            }
        }

        // Prevent sending media to blocked users. Blocking is bidirectional:
        // we neither receive from nor send to a blocked peer.
        if self.is_user_blocked(&recipient_str) {
            return Err(Error::UserBlocked(recipient_str));
        }

        // SEC-H1: media rides the same MLS session machinery as text. With
        // auto-encryption active the session must already be confirmed —
        // files are too large for the pending-message queue, so media is
        // never queued pending establishment and never falls back to
        // plaintext. Callers should retry after `secure_session_established`.
        if self.should_auto_encrypt() {
            if !self.is_session_confirmed(&recipient_str)? {
                // Kick session establishment exactly like the text path
                // (import a stored key package, create the session, send the
                // Welcome), then report not-ready: the session cannot be
                // confirmed synchronously, so callers retry after
                // `secure_session_established`.
                if let Some(mls) = self.mls_manager.clone() {
                    self.ensure_session_establishment(&mls, &recipient_str)?;
                }
                return Err(Error::SessionNotReady(
                    self.establishment_state(&recipient_str)?,
                ));
            }

            // Encrypted chunks share the recipient's session ratchet with
            // text, and the receiver keeps out-of-order message keys for a
            // bounded number of generations — cap concurrent transfers so
            // the combined in-flight windows cannot push a delayed chunk
            // beyond that tolerance and permanently stall it.
            use crate::constants::MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER;
            let active_transfers = self
                .outbound_media_transfers
                .values()
                .filter(|transfer| transfer.recipient == recipient_str)
                .count();
            if active_transfers >= MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER {
                return Err(Error::MediaTransferLimit(recipient_str));
            }
        } else if self.config.encryption.require_encryption {
            return Err(Error::EncryptFailed(
                "MLS encryption is required (the default) but MLS is not initialized — \
                 call initialize_mls() with secure and protocol-state storage, or explicitly opt out with \
                 require_encryption=false to send plaintext media"
                    .to_string(),
            ));
        } else {
            // Explicit opt-out: the whole transfer leaves as legacy plaintext
            // chunks. Warn once per peer, not per chunk.
            self.warn_plaintext_send(&recipient_str);
        }

        // Rich extras seal only toward recipients that advertised the rich
        // payload, and only on the encrypted path; everyone else — including
        // every plaintext (opt-out) transfer — gets the plain transfer.
        // Dropped here, never sent cleartext.
        let rich_extras = if !rich_extras.is_any() {
            None
        } else if self.should_auto_encrypt() && self.rich_seal_active(&recipient_str) {
            Some(rich_extras)
        } else {
            debug!(
                recipient = %recipient_str,
                "Dropping rich media extras: recipient did not advertise sealed rich payload support"
            );
            None
        };

        let file_id = options
            .file_id
            .unwrap_or_else(|| format!("file_{}", MessageId::new().as_str()));
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
            TransportType::Reticulum => {
                // Reticulum is low-bandwidth (LoRa); use BLE-like chunk sizes.
                use crate::constants::{CHUNK_SIZE_BLE, MEDIA_WINDOW_SIZE_BLE};
                (CHUNK_SIZE_BLE, MEDIA_WINDOW_SIZE_BLE)
            }
            TransportType::Nostr => {
                // Nostr relays have decent bandwidth; use Internet-like chunk sizes.
                use crate::constants::{CHUNK_SIZE_INTERNET, MEDIA_WINDOW_SIZE_INTERNET};
                (CHUNK_SIZE_INTERNET, MEDIA_WINDOW_SIZE_INTERNET)
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
                rich_extras: rich_extras.clone(),
            },
        );

        // Persist the crash-recovery descriptor (no chunk bytes) so a
        // restart mid-transfer can tell the app to re-initiate via
        // MediaResendRequired. A same-file_id resend consumes the restored
        // copy here — its checksum was validated at the boundary above.
        if let Some(first_chunk) = chunks.first() {
            let descriptor = MediaTransferDescriptor {
                file_id: file_id.clone(),
                recipient: recipient_str.clone(),
                file_name: first_chunk.file_name.clone(),
                file_size: first_chunk.file_size,
                file_checksum: first_chunk.file_checksum.clone(),
                content_type,
                queued_at: Utc::now(),
            };
            self.restored_media_descriptors.remove(&file_id);
            self.persist_media_descriptor(&descriptor);
        }

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
            rich_extras.as_ref(),
        )?;

        Ok(file_id)
    }

    /// Sends a batch of file chunks, wiring each into the outbox and media tracking.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_media_chunk_batch(
        &mut self,
        file_id: &str,
        chunks: Vec<FileChunk>,
        recipient: &str,
        pinned_transport: TransportType,
        content_type: ContentType,
        media_metadata: Option<&MediaMetadata>,
        rich_extras: Option<&MediaRichExtras>,
    ) -> Result<()> {
        for chunk in chunks {
            let chunk_index = chunk.chunk_index;

            let meta_for_chunk = if chunk_index == 0 {
                media_metadata.cloned()
            } else {
                None
            };

            // SEC-H1: with auto-encryption active, chunk bytes AND the chunk-0
            // metadata (file name, preview thumbnail, original content type)
            // travel inside the MLS ciphertext — the wire Message carries none
            // of them. The plaintext fields are only populated when
            // auto-encryption is inactive. Rich extras (caption, reply,
            // forward) exist only on this sealed path, chunk 0 only; their
            // presence bumps the envelope to v2 so a pre-rich receiver
            // rejects cleanly instead of misparsing.
            let (binary_payload, wire_metadata, wire_original_ct) = if self.should_auto_encrypt() {
                let inner = MediaChunkPlaintext {
                    chunk_bytes: chunk.to_bytes(),
                    media_metadata: meta_for_chunk,
                    original_content_type: (chunk_index == 0).then_some(content_type),
                    rich_extras: if chunk_index == 0 {
                        rich_extras.cloned()
                    } else {
                        None
                    },
                };
                let envelope_version = inner.envelope_version();
                let sealed = inner
                    .encode()
                    .map_err(Error::Serialization)
                    .and_then(|plaintext| {
                        self.encrypt_bytes_for_recipient_strict(recipient, &plaintext)
                    });
                let encrypted = match sealed {
                    Ok(encrypted) => encrypted,
                    Err(err) => {
                        // The chunks this batch popped from the window are
                        // marked in-flight with no outbox entry — nothing
                        // would ever ACK or retry them, so the transfer can
                        // only wedge while holding a per-peer transfer slot.
                        // Encryption failure is not per-chunk transient:
                        // abort the whole transfer loudly.
                        self.abort_outbound_media_transfer(file_id, "chunk encryption failed");
                        return Err(err);
                    }
                };
                (
                    encode_media_envelope(&encrypted, envelope_version),
                    None,
                    false,
                )
            } else {
                (chunk.to_bytes(), meta_for_chunk, chunk_index == 0)
            };

            let mut message = self.create_media_message(
                recipient,
                String::new(),
                ContentType::FileChunk,
                wire_metadata,
            )?;
            message.binary_content = Some(binary_payload);

            if wire_original_ct {
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
                    let _ = self
                        .handle_send_failure(&message, current_transport.or(previous_transport))?;
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
                transfer.rich_extras.as_ref(),
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
            if let Some((oldest_id, last_transport, attempt_count)) = outbox
                .iter()
                .min_by_key(|(_, entry)| entry.last_sent_at)
                .map(|(id, entry)| (id.clone(), entry.last_transport, entry.attempt_count))
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
                if !is_media {
                    self.clear_outbox_entry_from_storage(&oldest_id);
                }
                self.handle_outbound_media_chunk_failed(&oldest_id, "outbox eviction");
                if !is_media {
                    // Capacity eviction is as terminal as expiry: the entry
                    // and its persisted copy are gone, so without an event
                    // the app shows the message as pending forever. (Media
                    // chunks surface through the transfer abort above.)
                    self.emit_event(Event::message_failed(
                        oldest_id.clone(),
                        "Outbox capacity exceeded".to_string(),
                        attempt_count,
                    ));
                    if let Some(recipient) = self.take_undeliverable_connection_request(&oldest_id)
                    {
                        warn!(
                            recipient = %recipient,
                            message_id = %oldest_id,
                            "Connection request undeliverable: outbox capacity exceeded"
                        );
                        self.emit_event(Event::connection_request_undeliverable(
                            recipient,
                            oldest_id.as_str(),
                            "outbox_capacity_exceeded".to_string(),
                        ));
                    }
                }
            }
        }

        // Consume staged re-seal provenance only when first creating a
        // (non-media) entry here (the failure/retry path); later calls find the
        // entry already present and leave its `reseal` intact.
        let staged_reseal = self.take_staged_reseal(message, is_media);
        let outbox = if is_media {
            &mut self.media_outbox
        } else {
            &mut self.outbox
        };
        let is_new = !outbox.contains_key(&message.id);
        outbox
            .entry(message.id.clone())
            .or_insert_with(|| OutboxEntry {
                message: message.clone(),
                attempt_count: 0,
                first_sent_at: Utc::now(),
                last_sent_at: Utc::now(),
                last_transport: None,
                reseal: staged_reseal,
            });
        // Persist newly-created main-outbox entries so they survive a restart.
        // media entries are intentionally not persisted.
        if is_new && !is_media {
            if let Some(entry) = self.outbox.get(&message.id) {
                self.persist_outbox_entry(entry);
            }
        }
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
        let is_media = Self::is_media_outbox_message(message);
        // Consume the staged re-seal provenance only when first creating a
        // (non-media) outbox entry; on later attempts the entry already carries
        // it. Staging for media is never populated.
        let staged_reseal = self.take_staged_reseal(message, is_media);
        let outbox = if is_media {
            &mut self.media_outbox
        } else {
            &mut self.outbox
        };
        let is_new = !outbox.contains_key(&message.id);
        let entry = outbox
            .entry(message.id.clone())
            .or_insert_with(|| OutboxEntry {
                message: message.clone(),
                attempt_count: 0,
                first_sent_at: now,
                last_sent_at: now,
                last_transport: transport,
                reseal: staged_reseal,
            });

        entry.message = message.clone();
        if entry.attempt_count == 0 {
            entry.first_sent_at = now;
        }
        entry.attempt_count = attempt_hint.unwrap_or(entry.attempt_count.saturating_add(1));
        entry.last_sent_at = now;
        entry.last_transport = transport;

        // Persist only when the entry is newly created here. The success path
        // never calls ensure_outbox_entry, so this is the one point a
        // successfully-sent message first enters the durable outbox before its
        // ACK. Later attempts don't re-persist: the durable copy already exists
        // and attempt/timestamp churn isn't restore-critical (the TTL is
        // refreshed on restore anyway), which keeps secure-storage writes off
        // the retry hot path. Media entries are never persisted.
        if is_new && !is_media {
            if let Some(entry) = self.outbox.get(&message.id) {
                self.persist_outbox_entry(entry);
            }
        }
    }

    /// Stages re-seal provenance for a message about to be dispatched, so the
    /// outbox entry created during dispatch carries it. Callers gate on the
    /// content being a sealed encrypted DM (`__MLS_ENC__`); media and plaintext
    /// never stage and thus always replay verbatim.
    pub(super) fn stage_outbox_reseal(&mut self, id: &MessageId, reseal: OutboxReseal) {
        self.pending_reseal.insert(id.clone(), reseal);
    }

    /// Consumes any staged re-seal provenance for `message`, attaching it to a
    /// (non-media) main-outbox entry being created for the first time. On later
    /// attempts the entry already exists and carries its `reseal`, so this
    /// returns `None`. Media never stages.
    ///
    /// **Always removes the staged copy** (it does not leave it behind when it
    /// returns `None`): the staging map is strictly transient and must never
    /// outlive its message, or a message that stages but is then dropped before
    /// an outbox entry is created (e.g. `dispatch_prepared_message` early-returns
    /// on a duplicate id) would strand plaintext in `pending_reseal` for the
    /// process lifetime. Removing unconditionally here — plus the belt-and-braces
    /// clear in [`Self::remove_outbox_entry`] — bounds the map to genuinely
    /// in-flight sends. Kept as one helper so the two outbox-entry creation sites
    /// ([`Self::ensure_outbox_entry`] and [`Self::mark_message_sent`]) stay in
    /// lockstep on the consume condition.
    fn take_staged_reseal(&mut self, message: &Message, is_media: bool) -> Option<OutboxReseal> {
        let staged = self.pending_reseal.remove(&message.id);
        if is_media || self.outbox.contains_key(&message.id) {
            return None;
        }
        staged
    }

    /// Re-seals an outbound encrypted DM against the recipient's *current* MLS
    /// session for a resend, returning fresh sealed content, or `None` to replay
    /// verbatim. Returns `None` when it is not a re-sealable encrypted DM, when
    /// there is no stored provenance (plaintext, media, or a pre-upgrade
    /// persisted entry), when the session is not currently confirmed, or when
    /// the re-seal produced identical bytes.
    ///
    /// This is the Tier 2 core: after a desync + re-key the recipient's session
    /// is rebuilt at a new epoch, so replaying the ciphertext sealed at the old
    /// epoch is undecryptable forever. Re-sealing here rebuilds only the
    /// ciphertext `content`; the outer `Message.id` is preserved so the
    /// receiver's dedup and ACK correlation are unaffected. Gating on a
    /// confirmed session guarantees `prepare_outbound_content` takes the
    /// encrypt-now path and never the `store_pending` queueing path.
    fn reseal_resend_content(&mut self, message: &Message) -> Option<String> {
        if !message.content.starts_with(internal_prefixes::ENCRYPTED) {
            return None;
        }
        let recipient = message.recipient.as_str().to_string();
        if !self.confirmed_sessions.contains(&recipient) {
            return None;
        }
        let reseal = self.outbox.get(&message.id)?.reseal.as_ref()?.clone();
        match self.prepare_outbound_content(
            &recipient,
            &reseal.content,
            reseal.priority,
            reseal.reply_to_msg.clone(),
            reseal.forwarded_from.clone(),
            reseal.content_type,
            reseal.media_metadata.clone(),
            reseal.rich.as_ref(),
            Some(PendingProvenance::for_id(message.id.clone())),
            "resend_reseal",
        ) {
            Ok(OutboundSendPreparation::Ready(sealed)) if sealed != message.content => Some(sealed),
            // Identical bytes, or an error: replay verbatim this round. Tier 1
            // keeps the sender retrying until the session heals.
            //
            // The `Queued` arm is all but unreachable — the `confirmed_sessions`
            // gate above runs synchronously with no yield before the encrypt, so
            // the session cannot flip to not-ready in between. If it ever does
            // (e.g. `confirmed_sessions` outlived an externally-deleted MLS
            // session), `prepare_outbound_content` has enqueued a pending message
            // as a side effect, keyed by our SAME `existing_id`. That is benign:
            // this round still replays the verbatim outbox bytes, and when the
            // pending copy later flushes, the sender's own `dispatch_prepared_message`
            // dedup drops it as a duplicate id. So no double-delivery, just a
            // transient pending entry that self-clears on flush.
            _ => None,
        }
    }

    /// Re-seals `message` in place against the current session if it is a
    /// re-sealable encrypted DM (see [`Self::reseal_resend_content`]). Used at
    /// the resend transmit points so the bytes that go out — and the outbox copy
    /// `mark_message_sent` then stores — are freshly sealed.
    pub(super) fn reseal_for_resend_in_place(&mut self, message: &mut Message) {
        if let Some(fresh) = self.reseal_resend_content(message) {
            message.content = fresh;
        }
    }

    pub(super) fn remove_outbox_entry(&mut self, message_id: &MessageId) -> Option<OutboxEntry> {
        // Belt-and-braces: drop any still-staged re-seal provenance for this id
        // so it never outlives the message (see `take_staged_reseal`). Normally
        // staging is consumed at entry creation; this covers the id being torn
        // down before that ever happened.
        self.pending_reseal.remove(message_id);
        if let Some(entry) = self.outbox.remove(message_id) {
            self.clear_outbox_entry_from_storage(message_id);
            return Some(entry);
        }
        self.media_outbox.remove(message_id)
    }

    /// Settles the pending connection-request entry for `message_id` when a
    /// terminal drop occurs (max retries, outbox expiry, capacity eviction),
    /// returning the recipient the caller should emit
    /// `connection_request_undeliverable` for. The TTL gates emission — a
    /// signal that stale belongs to a request the app has long stopped
    /// waiting on — but the entry is removed either way.
    pub(super) fn take_undeliverable_connection_request(
        &mut self,
        message_id: &MessageId,
    ) -> Option<String> {
        self.pending_connection_requests
            .remove(&message_id.as_str())
            .filter(|pending| pending.sent_at.elapsed() <= PENDING_CONNECTION_REQUEST_TTL)
            .map(|pending| pending.recipient)
    }

    /// Removes messages that have waited too long for MLS session
    /// establishment. This gives the pre-outbox queue the same bounded
    /// lifecycle guarantee as dispatched messages.
    pub(super) fn cleanup_expired_pending_messages(&mut self) {
        self.cleanup_expired_pending_messages_at(Utc::now());
    }

    /// Runs pending-message expiry only when the earliest queued deadline is
    /// due. Bindings call `process()` every 100 ms, so blindly scanning the
    /// entire queue there wastes CPU and battery for almost the whole default
    /// seven-day lifetime.
    pub(super) fn cleanup_expired_pending_messages_if_due(&mut self) {
        let now = Utc::now();
        if self
            .next_pending_message_expiry
            .is_some_and(|expiry| now >= expiry)
        {
            self.cleanup_expired_pending_messages_at(now);
        }
    }

    fn cleanup_expired_pending_messages_at(&mut self, now: DateTime<Utc>) {
        let lifetime_ms = self
            .config
            .reliability
            .retry
            .pending_message_max_lifetime_ms;
        let mut changed_recipients = Vec::new();
        let mut expired_ids = Vec::new();

        for (recipient, messages) in &mut self.pending_encrypted_messages {
            let previous_len = messages.len();
            messages.retain(|pending| {
                // Bounded per pass: each expiry costs a durable delete, and a
                // burst of entries queued together comes due together. The
                // remainder stays queued past its deadline and drains on the
                // next tick — see `MAX_PENDING_EXPIRIES_PER_PASS`.
                if expired_ids.len() < MAX_PENDING_EXPIRIES_PER_PASS
                    && lifetime_expired(now, pending.queued_at, lifetime_ms)
                {
                    expired_ids.push(pending.message_id.clone());
                    false
                } else {
                    true
                }
            });
            if messages.len() != previous_len {
                changed_recipients.push(recipient.clone());
            }
        }

        for recipient in changed_recipients {
            if self
                .pending_encrypted_messages
                .get(&recipient)
                .is_some_and(Vec::is_empty)
            {
                self.pending_encrypted_messages.remove(&recipient);
            }
        }
        // The survivors' records are untouched, so only the expired entries are
        // written to storage — one delete each, paired with the settlement
        // below.
        self.delete_pending_messages_from_storage(expired_ids.iter());
        self.recompute_next_pending_message_expiry();

        for message_id in expired_ids {
            // Reached both from `process()` (emits immediately) and from
            // restore, which runs before the event pipeline is live.
            self.settle_restored_message_failure(Event::message_failed(
                message_id,
                "Pending session lifetime exceeded".to_string(),
                0,
            ));
        }
    }

    pub(super) fn cleanup_outbox(&mut self) {
        let now = Utc::now();
        let lifetime_ms = self.config.reliability.retry.outbox_max_lifetime_ms;
        // In-process twin of the restore path's absolute cap
        // (`restore_outbox`): the carrier-relative window slides on every
        // send, and an unreachable-parked DM's mesh reachability probe keeps
        // sending — without an absolute bound from `first_sent_at`, a
        // long-lived process would keep such an entry alive forever.
        let absolute_lifetime_ms =
            lifetime_ms.saturating_mul(crate::constants::OUTBOX_ABSOLUTE_LIFETIME_FACTOR as u64);

        let mut expired_from_outbox = Vec::new();
        for (message_id, entry) in &self.outbox {
            if !lifetime_expired(now, entry.last_sent_at, lifetime_ms)
                && !lifetime_expired(now, entry.first_sent_at, absolute_lifetime_ms)
            {
                continue;
            }
            if entry.message.requires_ack && self.ack_manager.is_waiting_for_ack(&entry.message.id)
            {
                continue;
            }
            expired_from_outbox.push((
                message_id.clone(),
                entry.last_transport,
                entry.attempt_count,
            ));
        }
        for (message_id, last_transport, attempt_count) in expired_from_outbox {
            if let Some(transport) = last_transport {
                self.transport_manager.record_delivery_failure(transport);
            }
            self.retry_queue.remove(&message_id.as_str());
            self.outbox.remove(&message_id);
            self.clear_outbox_entry_from_storage(&message_id);
            self.handle_outbound_media_chunk_failed(&message_id, "outbox lifetime exceeded");

            // Expiry is terminal: without an event the app shows the message
            // as pending forever. Mirrors handle_max_retries_exceeded, which
            // settles the same way when the retry budget runs out.
            self.emit_event(Event::message_failed(
                message_id.clone(),
                "Outbox lifetime exceeded".to_string(),
                attempt_count,
            ));
            if let Some(recipient) = self.take_undeliverable_connection_request(&message_id) {
                warn!(
                    recipient = %recipient,
                    message_id = %message_id,
                    "Connection request undeliverable: outbox lifetime exceeded"
                );
                self.emit_event(Event::connection_request_undeliverable(
                    recipient,
                    message_id.as_str(),
                    "outbox_lifetime_exceeded".to_string(),
                ));
            }
        }

        // Only peers that still hold outbox entries can retain an
        // unreachable-park counter (bounds the map at the outbox cap).
        if !self.dm_unreachable_parks.is_empty() {
            let active: HashSet<&str> = self
                .outbox
                .values()
                .map(|entry| entry.message.recipient.as_str())
                .collect();
            self.dm_unreachable_parks
                .retain(|peer, _| active.contains(peer.as_str()));
        }

        let mut expired_from_media = Vec::new();
        for (message_id, entry) in &self.media_outbox {
            if !lifetime_expired(now, entry.last_sent_at, lifetime_ms) {
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
            self.retry_queue.remove(&message_id.as_str());
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
            if let Some(eviction) = self
                .ack_manager
                .register_pending_ack(message.id.clone(), None)?
            {
                self.emit_event(Event::ack_evicted(
                    eviction.message_id,
                    eviction.priority.as_str(),
                    "capacity".to_string(),
                ));
            }
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
    /// Returns the absolute time the retry was scheduled for (`None` when the
    /// message was already queued for retry), so callers can surface it in
    /// `MessageDeferred` instead of guessing.
    pub(super) fn handle_send_failure(
        &mut self,
        message: &Message,
        transport: Option<TransportType>,
    ) -> Result<Option<DateTime<Utc>>> {
        // Ensure message is persisted to outbox for recovery
        self.ensure_outbox_entry(message);

        // No transport could reach the recipient directly — but a neighbor
        // might be able to, or know someone who can. Offer the frame to the
        // mesh before settling for a retry: this is the case the whole
        // forwarding path exists for, someone out of our range but inside the
        // crowd's.
        //
        // The outbox entry and retry stay in place regardless. Handing a frame
        // to neighbors is not proof it arrived, and the acknowledgement coming
        // back is what settles it.
        let handed_to_mesh = self.offer_to_mesh(message);

        // Schedule retry (enqueue is infallible — no attempt limit)
        let next_retry_at = self.retry_queue.enqueue(message.clone(), 0);

        warn!(
            message_id = %message.id,
            transport = ?transport,
            handed_to_mesh,
            "Deferred message due to send error"
        );
        Ok(next_retry_at)
    }

    /// Hands a locally-originated frame to nearby devices so it can travel
    /// toward a recipient we cannot reach ourselves.
    ///
    /// Returns how many neighbors took a copy. Zero means we are alone, or
    /// every neighbor refused the link — in both cases the retry ladder and the
    /// outbox remain the recovery path, exactly as before.
    ///
    /// Unlike a forwarded frame, this one is not held back: the delay before
    /// forwarding exists so neighbors holding the *same* frame do not all
    /// transmit at once, and nobody else is holding this one yet.
    pub(super) fn offer_to_mesh(&mut self, message: &Message) -> usize {
        // Deliberately not gated on `allow_relay`. That setting is about what
        // this device does for *other people* — whether it spends its battery
        // carrying their traffic. This is the device's own message, and the
        // ones it must be able to send include the acknowledgement for a
        // message it just received. Gating it here would leave a
        // relay-declining device unable to answer anything that reached it
        // across the mesh, so its sender would retransmit to exhaustion and
        // report a failure for a message that was delivered and read.
        let neighbors = self.transport_manager.mesh_neighbors();
        if neighbors.is_empty() {
            return 0;
        }

        // Ours to originate: record it as handled so a copy coming back to us
        // through the mesh is recognized and not forwarded again.
        self.mesh_relay.mark_handled(&message.id.as_str());

        // If the recipient is standing right there, hand it to them. Reached
        // here it means the ordinary send could not use that link, but trying
        // it costs one call and beats routing around the destination.
        let recipient = message.recipient.as_str();
        let targets = if neighbors.iter().any(|n| n.peer_id == recipient) {
            vec![recipient.to_string()]
        } else {
            self.mesh_relay.select_targets(
                neighbors
                    .iter()
                    .map(|n| (n.peer_id.as_str(), n.link_quality())),
                &[],
                &message.id.as_str(),
            )
        };

        let mut handed_to = 0usize;
        for target in targets {
            // Metered against the same ceiling as carrying other people's
            // traffic: it is the same radio. Without this a large transfer,
            // which offers every chunk separately, would put an unbounded burst
            // on the air in one call.
            if !self.mesh_relay.take_send_token() {
                debug!(
                    message_id = %message.id,
                    "At the forwarding limit; not handing this to further neighbors"
                );
                break;
            }

            match self.transport_manager.send_to_neighbor(&target, message) {
                Ok(transport) => {
                    handed_to += 1;
                    debug!(
                        message_id = %message.id,
                        recipient = %message.recipient,
                        next_hop = %target,
                        transport = ?transport,
                        "Handed message to a neighbor to carry"
                    );
                }
                Err(err) => {
                    debug!(
                        message_id = %message.id,
                        next_hop = %target,
                        error = %err,
                        "Neighbor could not take the message"
                    );
                }
            }
        }

        handed_to
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

        // A relay DeliveryError verdict outranks a late wire confirm: the
        // socket write succeeded, but the relay saw the frame and dropped it
        // (recipient offline). When the verdict raced ahead of this confirm,
        // the record is already parked `Failed`/`PeerUnreachable`; letting
        // the confirm proceed would resurrect the false `Sent` the verdict
        // just corrected — and emit welcome_send_succeeded *after* the
        // corrective welcome_send_failed. Any legitimate new send attempt
        // moves the record to `SendAttempted` first, so `Failed` here means
        // this confirm belongs to the very send the relay already failed.
        if updated.state == WelcomeDeliveryState::Failed
            && updated.last_reason_code == Some(crate::events::WelcomeReasonCode::PeerUnreachable)
        {
            return Ok(());
        }

        // A wire confirm for a reachability probe must not upgrade the
        // record to `Sent`. With a live unreachable-park counter the send in
        // flight IS the escalating probe — every reachability edge clears the
        // counter before sending anything else — and the socket write
        // succeeding adds nothing the park didn't already know. Whenever the
        // relay's push fallback succeeds it returns no `DeliveryError` at
        // all, so a probe marked `Sent` here would go quiet forever:
        // `next_retry_at` cleared below, `Sent` outside every retry scan, and
        // `rearm_welcome_for_peer` a no-op on it — the exact edge-only dead
        // end the probe exists to close, reachable only through this bridge
        // confirm (mesh carriers never wire-confirm). Leaving the record
        // `SendAttempted` keeps the confirm deadline armed: a relay verdict
        // re-parks it, and an accepted-but-unanswered frame resolves at the
        // confirm timeout, which re-parks via
        // `welcome_probe_repark_permitted`. Genuine delivery still converges
        // regardless — the peer proving the session marks the welcome `Sent`
        // through `confirm_session_state`. This also withholds the
        // `welcome_send_succeeded` a probe's socket write would otherwise
        // emit, which the next verdict or timeout would only have to correct.
        if matches!(updated.state, WelcomeDeliveryState::SendAttempted)
            && updated.unreachable_parks > 0
        {
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
                peer_id.clone(),
                sent_snapshot.welcome_message.id.as_str().to_string(),
                sent_snapshot.group_id,
                sent_snapshot.attempt,
            ));
        }

        // The welcome is now confirmed as sent. Send a confirmation probe
        // immediately so the session can be confirmed without waiting for the
        // next process() tick — an optimistic fast-path for the common case
        // where the transport confirms promptly.
        self.send_session_confirmation_probe(&peer_id, "transport_confirmed");

        Ok(())
    }

    /// Handles asynchronous transport send failures for pending welcome
    /// sends and outbound connection requests.
    pub fn on_transport_send_failed(
        &mut self,
        message_id: &str,
        transport_error: Option<String>,
    ) -> Result<()> {
        // Connection requests first: the relay's "recipient offline"
        // DeliveryError is the only fast, authoritative failure signal a
        // request gets (there is no ACK from an offline peer — the app
        // would otherwise wait out the full retry budget for a generic
        // message_failed). Non-unreachable errors stay with the generic
        // retry machinery: the request is queued/retried and may still
        // deliver.
        if let Some(pending) = self.pending_connection_requests.get(message_id) {
            // The TTL must hold at read time too: the insert-path prune only
            // runs on the next `send_connection_request`, so an idle map can
            // hold an entry far past the correlation window — a failure
            // signal that stale belongs to a request the app has long
            // stopped waiting on and must not fire the event.
            if pending.sent_at.elapsed() > PENDING_CONNECTION_REQUEST_TTL {
                self.pending_connection_requests.remove(message_id);
            } else {
                let unreachable = transport_error
                    .as_deref()
                    .is_some_and(|r| r.starts_with(SEND_FAIL_REASON_RECIPIENT_UNREACHABLE));
                if unreachable {
                    let recipient = pending.recipient.clone();
                    self.pending_connection_requests.remove(message_id);
                    warn!(
                        recipient = %recipient,
                        message_id = %message_id,
                        "Connection request undeliverable: recipient unreachable"
                    );
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::connection_request_undeliverable(
                            recipient,
                            message_id.to_string(),
                            transport_error.unwrap_or_default(),
                        ));
                    }
                    return Ok(());
                }
            }
        }

        let Some(peer_id) = self.find_welcome_peer_by_message_id(message_id) else {
            // Not a connection request, not a welcome: a plain DM or media
            // chunk. Surface the relay's verdict and park plain DMs so the
            // ACK retry budget stops burning against an offline peer.
            self.handle_recipient_unreachable_for_message(message_id, transport_error.as_deref());
            return Ok(());
        };
        // A reason tagged "recipient_unreachable" (the internet bridge's
        // translation of the relay's DeliveryError) is authoritative proof
        // the frame was dropped: the carrier is up but *this peer* is not on
        // it. It must be handled even for records already wire-confirmed
        // (`Sent`) — the bridge confirms on socket-write success, before the
        // relay can answer, so the DeliveryError normally arrives when the
        // record is already Sent and the plain failure path below would
        // no-op on it, stranding a false `Sent`.
        let peer_unreachable = transport_error
            .as_deref()
            .is_some_and(|r| r.starts_with(SEND_FAIL_REASON_RECIPIENT_UNREACHABLE));
        if peer_unreachable {
            return self.apply_recipient_unreachable_failure(&peer_id, transport_error);
        }
        let reason = crate::events::WelcomeReasonCode::TransportUnavailable;
        // No raw error is available on this async path, so infer no-carrier
        // from live connectivity: with no transport currently available the
        // peer is simply unreachable and the Welcome must be kept alive, not
        // aged.
        let no_carrier = self.transport_manager.get_available_transports().is_empty();
        let _ = self.apply_welcome_send_failure(
            &peer_id,
            reason,
            transport_error,
            no_carrier,
            "transport_failed",
        )?;
        Ok(())
    }

    /// Handles a relay `recipient_unreachable` verdict for a plain DM or
    /// media chunk: emits the non-terminal [`Event::MessageUndeliverable`],
    /// then *parks* plain DMs.
    ///
    /// Parking drops the pending ACK and the retry-queue entry while the
    /// outbox entry stays put — exactly the "stranded" state every re-drive
    /// edge already flushes (`flush_outbox_all` on reconnect/start,
    /// `flush_outbox_for_peer_via` on discovery and presence-online, the latter
    /// fed by [`Self::presence_watch_peers`]). Without the park, the offline
    /// peer's missing ACK burns the full `max_retries` budget in minutes
    /// (`process_timed_out_acks`) and settles a 7-day outbox message
    /// terminally; a re-driven send re-registers a fresh ACK, so the budget
    /// normally burns only against a peer believed reachable. The exception
    /// is the reachability probe below: its send re-enters the ACK machinery
    /// but may never earn a relay verdict (a mesh carrier cannot produce one
    /// at all), so exhaustion with a live park counter *re-parks* instead of
    /// settling ([`Self::try_repark_exhausted_dm`]). Settlement is reserved
    /// for delivery or outbox-lifetime expiry.
    ///
    /// The park always keeps a timed reachability probe whose interval
    /// escalates per consecutive park (15s → 600s cap, per-peer counter reset
    /// on any reachability edge). It is deliberately carrier-agnostic: with a
    /// local mesh carrier the peer may be a room away — possibly already a
    /// discovered neighbor, so no future edge would fire for it — and on an
    /// internet-only device the external edges (presence rotation, reconnect)
    /// are the *only* other recovery, which leaves delivery hostage to a
    /// bridge's polling cadence. See [`Self::park_unreachable_dm`] for why
    /// probing over the relay is self-limiting.
    ///
    /// Media chunks are never parked: their offline story is retry
    /// exhaustion → transfer abort → persisted descriptor →
    /// `MediaResendRequired` with app-resupplied bytes. Messages without an
    /// outbox entry (nothing to attribute a recipient from) are skipped.
    fn handle_recipient_unreachable_for_message(
        &mut self,
        message_id: &str,
        transport_error: Option<&str>,
    ) {
        let Some(reason) =
            transport_error.filter(|r| r.starts_with(SEND_FAIL_REASON_RECIPIENT_UNREACHABLE))
        else {
            return;
        };
        let Ok(parsed_id) = MessageId::from_str(message_id) else {
            return;
        };
        let (entry, is_media) = match self.outbox.get(&parsed_id) {
            Some(entry) => (entry, false),
            None => match self.media_outbox.get(&parsed_id) {
                Some(entry) => (entry, true),
                None => {
                    debug!(
                        message_id = %message_id,
                        "Unreachable verdict for message without outbox entry, dropping"
                    );
                    return;
                }
            },
        };
        let recipient = entry.message.recipient.as_str().to_string();
        let attempt_count = entry.attempt_count;
        let file_id = self
            .outbound_media_chunks
            .get(&parsed_id)
            .map(|(file_id, _)| file_id.clone());
        warn!(
            message_id = %message_id,
            file_id = ?file_id,
            parked = !is_media,
            "Recipient unreachable for in-flight message (non-terminal)"
        );
        self.emit_event(Event::message_undeliverable(
            parsed_id.clone(),
            recipient.clone(),
            reason.to_string(),
            file_id,
        ));
        if is_media {
            return;
        }

        self.park_unreachable_dm(&parsed_id, &recipient, attempt_count);
    }

    /// The park action shared by the relay-verdict path
    /// ([`Self::handle_recipient_unreachable_for_message`]) and the
    /// exhausted-probe path ([`Self::try_repark_exhausted_dm`]): drops the
    /// pending ACK and any scheduled retry so nothing burns budget against
    /// the offline peer, then schedules the escalating reachability probe.
    ///
    /// The probe is carrier-agnostic — DORS picks its transport like any
    /// other send — because an internet-only device is the common
    /// configuration, not an exotic one, and parking it edge-only leaves the
    /// message with *no* self-recovery at all: no pending ACK to time out, no
    /// retry entry, nothing on a timer. Recovery then depends entirely on an
    /// external reachability edge (`flush_outbox_all` on reconnect/start, or
    /// a presence-online answer), i.e. on a bridge's presence-polling cadence
    /// — tens of seconds to minutes, and nothing at all for a headless
    /// consumer that never polls presence.
    ///
    /// Probing over the relay is self-limiting in every outcome, which is why
    /// no carrier guard is needed:
    /// - the peer is still offline and the relay's push fallback fails → a
    ///   fresh `DeliveryError` re-enters this park, escalating the interval
    ///   (15s → 600s cap) and re-emitting the non-terminal
    ///   [`Event::MessageUndeliverable`];
    /// - the relay's push fallback succeeds → no verdict is returned at all
    ///   and the probe becomes an ordinary in-flight send on the ACK ladder;
    /// - the peer is back → the probe *is* the delivery, which beats waiting
    ///   for any presence edge.
    ///
    /// The counter is per-*peer* while the probes are per-*message*: a burst
    /// of N DMs to one offline peer escalates the shared ladder once per
    /// park, so no individual message climbs 15s → 600s — the Nth already
    /// starts high on it. The compensating edge is delivery: an ACK for any
    /// one of the peer's DMs re-drives the rest immediately
    /// ([`Self::handle_ack_message`]) instead of leaving them on their
    /// escalated timers.
    ///
    /// Relay traffic is bounded differently in each of those branches, and
    /// the difference matters for capacity planning now that the whole
    /// internet-only fleet is on this path:
    /// - verdict branch: the escalation is the bound — one frame per interval
    ///   per parked *message*, settling at one per 600s;
    /// - accepted branch: the probe registers a fresh ACK at `retry_count` 0,
    ///   so it rides the ordinary ACK ladder (up to `max_retries` sends,
    ///   1s → 300s backoff, ~800s cumulative on the defaults) before
    ///   `try_repark_exhausted_dm` re-parks it at the escalated interval.
    ///
    /// The outbox lifetime bounds the entry itself — and note the probe
    /// refreshes `last_sent_at` on every send, so the sliding 7-day window
    /// stops binding and settlement moves out to the absolute cap
    /// ([`crate::constants::OUTBOX_ABSOLUTE_LIFETIME_FACTOR`] × the lifetime).
    /// Note also that a probe can be routed onto the relay when a mesh
    /// carrier is up — DORS has always been free to choose it — so this path
    /// is not new behavior for the relay, only newly reachable on
    /// internet-only devices.
    ///
    /// The counter increment is unconditional for the same reason: it is what
    /// arms [`Self::try_repark_exhausted_dm`], so a probe that exhausts its
    /// ACK budget re-parks instead of settling terminally.
    fn park_unreachable_dm(&mut self, message_id: &MessageId, recipient: &str, attempt_count: u32) {
        // Park: no pending ACK to time out, no backoff-scheduled resend.
        self.ack_manager.remove_ack(message_id);
        self.retry_queue.remove(&message_id.as_str());

        let parks = {
            let count = self
                .dm_unreachable_parks
                .entry(recipient.to_string())
                .or_insert(0);
            *count = count.saturating_add(1);
            *count
        };
        // parks >= 1 here; shift clamped so 15 << 6 = 960 is the largest
        // pre-cap value — no overflow risk.
        let retry_in_secs = (WELCOME_NO_CARRIER_RETRY_SECS << (parks - 1).min(6))
            .min(WELCOME_UNREACHABLE_RETRY_CAP_SECS);
        if let Some(entry) = self.outbox.get(message_id) {
            let message = entry.message.clone();
            let _ = self.retry_queue.enqueue_with_delay(
                message,
                attempt_count,
                (retry_in_secs * 1000) as u64,
            );
        }
        debug!(
            message_id = %message_id,
            recipient = %recipient,
            retry_in_secs = retry_in_secs,
            parks = parks,
            "Parked unreachable DM with escalating reachability probe"
        );
    }

    /// Re-parks a plain DM whose ACK retry budget just exhausted while its
    /// recipient still holds a live unreachable-park counter, returning
    /// `true` when the caller (`handle_max_retries_exceeded`) must skip
    /// terminal settlement.
    ///
    /// A live counter means the relay declared the peer offline and no
    /// reachability edge has fired since (every edge clears it), so the
    /// exhausted budget was burnt by reachability probes — sends that succeed
    /// locally without proving the peer is back — not by a peer believed
    /// reachable. Over a mesh carrier such a probe can never earn a relay
    /// verdict at all; over the relay a verdict normally re-parks the message
    /// (resetting its ACK budget) well before exhaustion, so reaching here on
    /// an internet-only device means the probes were accepted for delivery
    /// but never answered. Settling terminally here would
    /// reintroduce the ~15-minute `message_failed` the park exists to
    /// prevent; re-parking keeps settlement reserved for delivery or
    /// outbox-lifetime expiry (the escalation cap bounds probe traffic, the
    /// outbox lifetime + absolute cap bound the entry itself).
    ///
    /// Deliberately narrow: pending connection requests keep their typed
    /// terminal path, welcomes keep their own lifecycle, and media chunks
    /// (never in `outbox`, and never counted) keep retry exhaustion →
    /// transfer abort → `MediaResendRequired`.
    pub(super) fn try_repark_exhausted_dm(&mut self, message_id: &MessageId) -> bool {
        if !self.is_parkable_plain_dm(message_id) {
            return false;
        }
        let Some(entry) = self.outbox.get(message_id) else {
            return false;
        };
        let recipient = entry.message.recipient.as_str();
        if !self.dm_unreachable_parks.contains_key(recipient) {
            return false;
        }
        let recipient = recipient.to_string();
        let attempt_count = entry.attempt_count;
        debug!(
            message_id = %message_id,
            recipient = %recipient,
            "ACK budget exhausted by reachability probes; re-parking instead of settling"
        );
        self.park_unreachable_dm(message_id, &recipient, attempt_count);
        true
    }

    /// True when `message_id` is a plain DM eligible for the
    /// unreachable-park machinery: not a pending connection request (typed
    /// terminal path), not an MLS welcome (own lifecycle), and still present
    /// in the outbox. Media chunks live in `media_outbox`, so they never
    /// pass the outbox check.
    pub(super) fn is_parkable_plain_dm(&self, message_id: &MessageId) -> bool {
        if self
            .pending_connection_requests
            .contains_key(&message_id.as_str())
        {
            return false;
        }
        if self
            .find_welcome_peer_by_message_id(&message_id.as_str())
            .is_some()
        {
            return false;
        }
        self.outbox.contains_key(message_id)
    }

    /// Cancels the in-flight probe state (pending ACK + scheduled retry) of
    /// every parkable plain DM addressed to `peer_id`, so a
    /// reachability-edge flush can re-drive them.
    ///
    /// Only acts when the peer holds a live unreachable-park counter: a
    /// live counter means the relay declared the peer offline and no edge
    /// has fired since, so a pending ACK on the peer's DMs belongs to a
    /// reachability probe — a send that locally succeeded but has not been
    /// answered — not to a delivery in good standing.
    /// Without this cancel, the flush edges' awaiting-ACK filters skip
    /// exactly the probing messages while still clearing the counter,
    /// stranding them on minutes-scale ACK backoff after the relay already
    /// reported the peer online — and, with the counter gone, exposing them
    /// to terminal settlement on exhaustion (`try_repark_exhausted_dm`
    /// finds nothing). Canceling an ACK that was genuinely about to arrive
    /// is safe: receivers dedupe and re-ACK duplicates.
    ///
    /// Does NOT clear the counter itself — the flush edge owns that reset,
    /// after this runs.
    pub(super) fn cancel_probe_state_for_parked_peer(&mut self, peer_id: &str) {
        if !self.dm_unreachable_parks.contains_key(peer_id) {
            return;
        }
        let probing: Vec<MessageId> = self
            .outbox
            .values()
            .filter(|entry| entry.message.recipient.as_str() == peer_id)
            .map(|entry| entry.message.id.clone())
            .collect();
        for message_id in probing {
            if !self.is_parkable_plain_dm(&message_id) {
                continue;
            }
            self.ack_manager.remove_ack(&message_id);
            self.retry_queue.remove(&message_id.as_str());
        }
    }

    /// Classifies an outbound internet frame as a server-plane control op the
    /// platform bridge should translate to (or mirror as) a relay-native
    /// message instead of an opaque `SendMessage`.
    ///
    /// Returns `(op, payload)` where `payload` is the JSON after the prefix
    /// (empty string for payload-less ops). `None` means normal traffic —
    /// send verbatim.
    ///
    /// Ops and their bridge semantics:
    /// - `group_relay_register` / `group_relay_broadcast` — self-addressed
    ///   relay hints (`send_group_message` optimization); REPLACE with
    ///   relay-native `CreateGroup`+member deltas / `SendGroupMessage`. The
    ///   relay does not intercept content prefixes, so untranslated frames
    ///   would be echoed back uselessly.
    /// - `group_mls_leave` — TAP, don't replace: the per-member leave
    ///   notification must still be delivered verbatim, but the bridge also
    ///   sends one relay-native `LeaveGroup` so the relay's group registry
    ///   (which feeds invite links and broadcast fan-out) doesn't go stale.
    ///
    /// Connection ops (`__CONN_REQ__`/`__CONN_ACC__`/`__CONN_REJ__`/
    /// `__CONN_CAN__`) deliberately do NOT classify: they ship verbatim as
    /// opaque `SendMessage` frames so the Ed25519 control signature in the
    /// message metadata survives to the receiver's security gate. The former
    /// relay-native translation (`SendConnectionRequest` & co.) rebuilt them
    /// unsigned on the receiving bridge, which the gate rejects as a
    /// signature downgrade once the sender's key is TOFU-pinned. Verbatim
    /// also gains the relay's push-notification fallback for offline
    /// recipients, which the relay-native connection frames never had. The
    /// cost is that pre-SDK relay clients (which only speak the relay-native
    /// frames) no longer interoperate with connection ops — an accepted
    /// trade: the SDK's internet transport is the only supported relay
    /// client.
    ///
    /// Only frames *originated by this device* classify. The mesh relays
    /// third-party messages verbatim (`try_relay_message`). The relay-hint
    /// ops additionally require the self-addressed recipient (that is how
    /// the core marks them as hints rather than traffic), so they classify
    /// only when sender AND recipient are both this device.
    ///
    /// Self-origination is proven by `hop_count == 0`, not by the `sender`
    /// field: `sender` is an unauthenticated wire field, so a mesh peer
    /// could forge `sender == self` on a frame we then relay — but
    /// `try_relay_message` increments the hop before re-sending, while every
    /// locally-originated frame leaves `send_internal_message` at hop 0.
    /// (Defense in depth: the receive loop also drops inbound frames
    /// claiming our own origin before they can reach the relay path.)
    pub fn internet_control_op(&self, message: &Message) -> Option<(&'static str, String)> {
        if message.hop_count.value() != 0 {
            return None;
        }
        let content = message.content.as_str();
        if message.sender.as_str() == self.config.user_id {
            if let Some(payload) = content.strip_prefix(internal_prefixes::GROUP_MLS_LEAVE) {
                return Some(("group_mls_leave", payload.to_string()));
            }
        }
        if message.sender.as_str() == self.config.user_id
            && message.recipient.as_str() == self.config.user_id
        {
            if let Some(payload) = content.strip_prefix(internal_prefixes::GROUP_RELAY_REGISTER) {
                return Some(("group_relay_register", payload.to_string()));
            }
            if let Some(payload) = content.strip_prefix(internal_prefixes::GROUP_RELAY_BROADCAST) {
                return Some(("group_relay_broadcast", payload.to_string()));
            }
        }
        None
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
            self.remove_media_descriptor(&file_id);
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
        self.abort_outbound_media_transfer(&file_id, reason);
    }

    /// Aborts an active outbound media transfer: removes all transfer
    /// tracking (freeing its per-peer transfer slot) and emits
    /// [`Event::MediaSendFailed`] so the app learns the transfer will never
    /// complete. Idempotent — a second call for the same `file_id` is a no-op.
    pub(super) fn abort_outbound_media_transfer(&mut self, file_id: &str, reason: &str) {
        let Some(transfer) = self.outbound_media_transfers.remove(file_id) else {
            return;
        };
        self.outbound_media_chunks
            .retain(|_, (candidate_file_id, _)| candidate_file_id.as_str() != file_id);
        self.outbound_media_windows.remove(file_id);
        // A terminal abort is settled state — the app hears MediaSendFailed
        // now, so a MediaResendRequired for it after a restart would be
        // stale noise.
        self.remove_media_descriptor(file_id);
        warn!(
            file_id = %file_id,
            reason = %reason,
            "Aborting outbound media transfer"
        );
        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::media_send_failed(
                file_id.to_string(),
                transfer.recipient,
                reason.to_string(),
            ));
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
        for file_id in &stale_outbound_file_ids {
            self.remove_media_descriptor(file_id);
        }
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
        if self.transport_manager.send(&ack_message).is_ok() {
            return Ok(());
        }

        // The sender is not somewhere we can reach directly. A message that
        // arrived across several devices has an answer that must travel the
        // same way back — without this, multi-hop delivery would look like
        // failure to the sender, who would retransmit a message we already
        // have.
        if self.offer_to_mesh(&ack_message) > 0 {
            return Ok(());
        }

        Err(Error::Other(format!(
            "no route back to {} for delivery acknowledgement",
            ack_message.recipient
        )))
    }

    /// Builds and sends a delivery ACK from the group-message drain, which
    /// holds only the acked id and the original wire sender (not the full
    /// inbound [`Message`]). Mirrors [`Self::send_delivery_ack`]; the hop count
    /// is unknown at drain time and reported as 0 (sender-side metrics only).
    pub(crate) fn send_group_delivery_ack(
        &mut self,
        ack_to: &str,
        acked_message_id: &str,
        inbound_transport: TransportType,
    ) -> Result<()> {
        let sender = UserId::new(&self.config.user_id)?;
        let recipient = UserId::new(ack_to)?;
        let app_id = AppId::new(&self.config.app_id)?;
        let ttl = TTL::new(self.config.initial_ttl).unwrap_or_else(|_| TTL::default());

        let ack_message = Message::builder(sender, recipient, app_id)
            .content(String::new())
            .priority(MessagePriority::Low)
            .ttl(ttl)
            .requires_ack(false)
            .metadata(ACK_FOR_KEY, acked_message_id)
            .metadata(ACK_HOP_COUNT_KEY, "0")
            .metadata(ACK_TRANSPORT_KEY, Self::transport_label(inbound_transport))
            .build();

        // Prefer the transport the message arrived on; fall back to DORS.
        if self
            .transport_manager
            .send_via_transport(&ack_message, inbound_transport)
            .is_ok()
        {
            return Ok(());
        }
        debug!(
            message_id = %acked_message_id,
            inbound_transport = ?inbound_transport,
            "Inbound transport unavailable for group ACK, falling back to DORS selection"
        );
        self.transport_manager.send(&ack_message)
    }

    pub(super) fn handle_ack_message(&mut self, message: &Message) {
        if let Some(ack_for) = message.metadata.get(ACK_FOR_KEY) {
            if let Ok(message_id) = MessageId::from_str(ack_for) {
                // A delivery ack settles an outbound connection request:
                // the recipient provably received it, so a later stale
                // recipient_unreachable signal must not fire a false
                // ConnectionRequestUndeliverable. Outside the ack_manager
                // branch — a duplicate ack (second transport) or one
                // arriving after retry exhaustion is still proof.
                self.pending_connection_requests
                    .remove(&message_id.as_str());
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
                    self.retry_queue.remove(&message_id.as_str());
                    if let Some(entry) = self.remove_outbox_entry(&message_id) {
                        // Delivery proves reachability, so the escalating
                        // unreachable-probe interval starts over. Clearing the
                        // counter is not enough on its own: it is per-peer
                        // while the probes are per-message, so a burst of DMs
                        // to an offline peer escalates the shared ladder once
                        // per park and leaves the later ones scheduled minutes
                        // out (at the 600s cap after seven). Delivery of any
                        // one of them proves the rest can go now, and on a
                        // consumer that never polls presence this ACK is the
                        // only edge that will say so. Flushing owns the
                        // counter reset — including restoring it when every
                        // re-drive fails — so this must not pre-clear it.
                        //
                        // The override is our own record of the transport the
                        // delivered send went out on (`last_transport`), which
                        // this ACK just proved reaches the peer — genuine
                        // per-transport reachability, pinning the re-drive the
                        // way presence-online pins Internet. Deliberately NOT
                        // the ACK's `transport` above: ACK_TRANSPORT_KEY is
                        // peer-supplied, and `from_label` maps any absent,
                        // stale, or hostile label to BLE — pinning a burst of
                        // sibling sends to a carrier chosen by the peer.
                        // Filtered on local availability (the transport may
                        // have dropped since the send); `None` falls back to
                        // DORS, so a missing carrier costs routing freedom,
                        // never the re-drive itself.
                        let recipient = entry.message.recipient.as_str().to_string();
                        if self.dm_unreachable_parks.contains_key(&recipient) {
                            let redrive_via = entry.last_transport.filter(|t| {
                                self.transport_manager
                                    .get_available_transports()
                                    .contains_key(t)
                            });
                            self.flush_outbox_for_peer_via(&recipient, redrive_via);
                        }
                    }
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
    /// * `initial_message` - Optional first message shown with the request
    ///   (surfaced verbatim in the recipient's `ConnectionRequestReceived`
    ///   event; apps typically seed the conversation with it on accept).
    ///   At most 4096 UTF-8 bytes (`MAX_INITIAL_MESSAGE_BYTES`, crate-private)
    ///   — longer input is rejected with `Error::InvalidArgument`.
    ///
    /// # Encryption
    ///
    /// Connection requests are internal control messages sent in plaintext,
    /// exempt from `require_encryption` (same as key packages and welcome
    /// messages) — an `initial_message` is therefore NOT end-to-end
    /// encrypted and should be treated like the sender display name.
    pub fn send_connection_request(
        &mut self,
        recipient: &str,
        sender_name: &str,
        key_package: Option<Vec<u8>>,
        initial_message: Option<String>,
    ) -> Result<MessageId> {
        Self::validate_outbound_recipient(recipient)?;
        if let Some(msg) = initial_message.as_deref() {
            if msg.len() > MAX_INITIAL_MESSAGE_BYTES {
                return Err(Error::InvalidArgument(format!(
                    "initial_message exceeds {} bytes (got {})",
                    MAX_INITIAL_MESSAGE_BYTES,
                    msg.len()
                )));
            }
        }

        // Connection requests are internal control messages (not user content),
        // so they are exempt from require_encryption — same as key packages.
        if self.is_user_blocked(recipient) {
            return Err(Error::UserBlocked(recipient.to_string()));
        }

        let payload = ConnectionRequestPayload {
            sender_name: sender_name.to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            key_package,
            initial_message,
        };

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::CONN_REQUEST, serialized);

        let message_id = self.send_internal_message(recipient, content, MessagePriority::High)?;
        self.track_pending_connection_request(&message_id, recipient);
        info!(recipient = %recipient, "Sent connection request");
        Ok(message_id)
    }

    /// Records an outbound connection request so a later transport-level
    /// "recipient offline" verdict can be surfaced as a typed event
    /// (see `on_transport_send_failed`). TTL-prunes and caps the map here,
    /// on the write path, so it needs no periodic sweep.
    fn track_pending_connection_request(&mut self, message_id: &MessageId, recipient: &str) {
        let now = std::time::Instant::now();
        self.pending_connection_requests
            .retain(|_, p| now.duration_since(p.sent_at) <= PENDING_CONNECTION_REQUEST_TTL);
        if self.pending_connection_requests.len() >= MAX_PENDING_CONNECTION_REQUESTS {
            if let Some(oldest) = self
                .pending_connection_requests
                .iter()
                .min_by_key(|(_, p)| p.sent_at)
                .map(|(id, _)| id.clone())
            {
                self.pending_connection_requests.remove(&oldest);
            }
        }
        self.pending_connection_requests.insert(
            message_id.as_str(),
            PendingConnectionRequest {
                recipient: recipient.to_string(),
                sent_at: now,
            },
        );
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
        Self::validate_outbound_recipient(recipient)?;
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
        Self::validate_outbound_recipient(recipient)?;
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

    /// Cancels a previously sent connection request via any available transport.
    ///
    /// The cancellation is routed through DORS, so it works over Internet, BLE, or WiFi Direct.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The user ID of the original request recipient
    ///
    /// # Encryption
    ///
    /// Connection cancellations are internal control messages sent in plaintext,
    /// exempt from `require_encryption` (same as key packages and welcome messages).
    pub fn cancel_connection_request(&mut self, recipient: &str) -> Result<MessageId> {
        Self::validate_outbound_recipient(recipient)?;
        if self.is_user_blocked(recipient) {
            return Err(Error::UserBlocked(recipient.to_string()));
        }

        let content = internal_prefixes::CONN_CANCEL.to_string();

        let message_id = self.send_internal_message(recipient, content, MessagePriority::High)?;
        info!(recipient = %recipient, "Cancelled connection request");
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
        Self::validate_outbound_recipient(recipient)?;
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
        Self::validate_outbound_recipient(recipient)?;
        if self.is_user_blocked(recipient) {
            return Err(Error::UserBlocked(recipient.to_string()));
        }
        if conversation_id.is_empty() {
            return Err(Error::InvalidArgument(
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
        Self::validate_outbound_recipient(recipient)?;
        if self.is_user_blocked(recipient) {
            return Err(Error::UserBlocked(recipient.to_string()));
        }
        if message_ids.is_empty() {
            return Err(Error::InvalidArgument(
                "message_ids must not be empty".to_string(),
            ));
        }
        if message_ids.len() > MAX_READ_RECEIPT_IDS {
            return Err(Error::InvalidArgument(format!(
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

    /// Builds the advertisement body carried by a key package, whether it is
    /// pushed to a peer or published for a stranger to fetch.
    ///
    /// Shared so the two paths cannot drift: a published record that advertised
    /// a different capability set from a pushed one would make a peer's
    /// behaviour depend on which way they first met us.
    pub(crate) fn build_key_package_payload(
        &self,
        key_pkg: &offline_protocol_mls::KeyPackageBundle,
        session_reset: bool,
    ) -> KeyPackagePayload {
        KeyPackagePayload {
            user_id: self.config.user_id.clone(),
            key_package_data: key_pkg.key_package_data.clone(),
            remaining_lifetime_ms: key_pkg.remaining_lifetime_ms(),
            timestamp_ms: Utc::now().timestamp_millis() as u64,
            session_reset,
            wire_versions: if self.config.transport.binary_wire_enabled {
                vec![offline_protocol_core::WIRE_VERSION_V1]
            } else {
                Vec::new()
            },
            env_versions: if self.config.encryption.compact_envelope_enabled {
                vec![MLS_ENVELOPE_COMPACT_V1]
            } else {
                Vec::new()
            },
            rich_versions: if self.config.encryption.rich_payload_enabled {
                vec![RICH_PAYLOAD_V1]
            } else {
                Vec::new()
            },
            // Present only when the Nostr transport is installed. Advertised
            // regardless of the sealing kill switch, which gates what *we*
            // publish, not what a peer may seal to us: withholding it would
            // downgrade the peer's traffic to the bootstrap key as a side
            // effect of a local setting.
            nostr_pubkey: self.transport_manager.nostr_public_key(),
        }
    }

    /// Advertises this device's key package to `peer_id`.
    ///
    /// The package comes from the peer-keyed pool
    /// ([`MlsManager::take_push_key_package`]): `peer_id` gets its own init key,
    /// re-handed on every push until a Welcome consumes it. Before that, this
    /// path handed the *same* package to every peer until somebody used it,
    /// which both weakened forward secrecy at session establishment (one
    /// compromised init key opens every Welcome built against it) and made the
    /// second peer's Welcome permanently unprocessable.
    pub(crate) fn send_key_package_to(&mut self, peer_id: &str, session_reset: bool) -> Result<()> {
        let mls = self.mls_manager.as_ref().ok_or(Error::MlsNotInitialized)?;

        let push = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.take_push_key_package(peer_id)?
        };

        if push.pool_exhausted {
            self.warn_push_key_package_pool_exhausted(peer_id);
        }

        let key_pkg = push.bundle;
        let payload = self.build_key_package_payload(&key_pkg, session_reset);

        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::KEY_PACKAGE, serialized);

        let mut message =
            self.create_message(peer_id, content, Some(MessagePriority::Low), None)?;
        self.sign_control_message(&mut message)?;

        match self.transport_manager.send(&message) {
            Ok(()) => {
                // SECURITY (resource exhaustion): `key_package_sent_to` is keyed
                // by the wire-claimed peer id, so a forged-sender key-package
                // flood (each reply routes through here) would grow it without
                // bound. Reset at capacity like `plaintext_receive_warned` — the
                // only cost of forgetting a peer is one idempotent re-send.
                if !self.key_package_sent_to.contains(peer_id)
                    && self.key_package_sent_to.len() >= MAX_KEY_PACKAGE_SENT_TO
                {
                    self.key_package_sent_to.clear();
                }
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

        // Reticulum excluded: LoRa bandwidth (~0.7 KB/s typical) is unsuitable for media transfer.
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
