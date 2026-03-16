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
use tracing::{debug, info, warn};

impl OfflineProtocol {
    /// Handles an incoming MLS key package message.
    pub(crate) fn handle_key_package_message(&mut self, sender: &str, data: &str) {
        if let Ok(payload) = serde_json::from_str::<KeyPackagePayload>(data) {
            debug!(sender = %sender, "Received key package");
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
                let _ = self.send_key_package_to(sender);
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
                        "Skipping probe confirmation until welcome delivery is sent"
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
                        "Skipping ack confirmation until welcome delivery is sent"
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
                                    warn!(error = %e, sender = %sender, "Failed to replace session");
                                    error_reason = Some(e.to_string());
                                }
                            }
                        } else {
                            info!(
                                sender = %sender,
                                local_id = %local_id,
                                "Welcome-wins tiebreaker: keeping local session (local < remote)"
                            );
                            should_flush = true;
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
                            let _ = self.send_key_package_to(&sender_owned);
                        }

                        // Emit secure session established event
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::secure_session_established(
                                sender_owned,
                                group_id,
                                is_session,
                                false, // initiated_by_local is false - we received the Welcome
                            ));
                        }
                    }
                    Err(e) => {
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::secure_session_failed(
                                sender_owned,
                                format!("Failed to persist confirmation: {}", e),
                            ));
                        }
                    }
                }
            } else if let Some(reason) = error_reason {
                // Emit secure session failed event
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::secure_session_failed(sender_owned, reason));
                }
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
                    if !self.can_confirm_from_source(&sender_owned, "decrypt_success") {
                        debug!(
                            sender = %sender_owned,
                            "Skipping decrypt-based confirmation until welcome delivery is sent"
                        );
                        Some(InternalMessageResult::Decrypted(text))
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
                        Some(InternalMessageResult::Decrypted(text))
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
    pub(crate) fn handle_group_relay_message(&mut self, _sender: &str, content: &str) {
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
                // Reconcile local member cache if we have MLS state for this group
                if let Some(members) = self.group_mesh.members.get_mut(&payload.group_id) {
                    members.retain(|m| m != &payload.user_id);
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
