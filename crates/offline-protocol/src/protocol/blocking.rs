//! User blocking: silent block list that filters incoming messages, control
//! messages, discovery events, and connection requests from blocked users.

use super::{lock_shared_state, OfflineProtocol};
use crate::protocol::types::MAX_BLOCKED_USERS;
use crate::{Error, Event, Result};
use offline_protocol_core::UserId;
use tracing::{debug, info};

impl OfflineProtocol {
    /// Blocks a user. Messages from this user will be silently dropped
    /// (no ACK sent, no event emitted). The blocked user receives no
    /// notification.
    ///
    /// Blocking is idempotent — calling this for an already-blocked user
    /// succeeds silently.
    pub fn block_user(&mut self, user_id: &str) -> Result<()> {
        // Validate user_id format
        let _ = UserId::new(user_id)
            .map_err(|_| Error::InvalidConfiguration(format!("Invalid user ID: {}", user_id)))?;

        // Cannot block self
        if user_id == self.config.user_id {
            return Err(Error::InvalidConfiguration(
                "Cannot block own user ID".to_string(),
            ));
        }

        // Enforce capacity limit
        if self.blocked_users.len() >= MAX_BLOCKED_USERS && !self.blocked_users.contains(user_id) {
            return Err(Error::InvalidConfiguration(format!(
                "Blocked users limit reached ({})",
                MAX_BLOCKED_USERS
            )));
        }

        if !self.blocked_users.insert(user_id.to_string()) {
            // Already blocked — idempotent success
            debug!(user_id = %user_id, "User already blocked");
            return Ok(());
        }

        self.persist_blocked_user(user_id);

        info!(user_id = %user_id, "User blocked");

        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::user_blocked(user_id.to_string()));
        }

        Ok(())
    }

    /// Unblocks a previously blocked user.
    ///
    /// Clears any stale MLS session state, pending key packages, and queued
    /// messages for the peer so that a fresh key exchange can occur on
    /// re-discovery.
    ///
    /// Unblocking a non-blocked user succeeds silently.
    pub fn unblock_user(&mut self, user_id: &str) -> Result<()> {
        // Validate user_id format
        let _ = UserId::new(user_id)
            .map_err(|_| Error::InvalidConfiguration(format!("Invalid user ID: {}", user_id)))?;

        if !self.blocked_users.remove(user_id) {
            // Not blocked — idempotent success
            debug!(user_id = %user_id, "User was not blocked");
            return Ok(());
        }

        self.delete_blocked_user(user_id);
        self.cleanup_peer_session_state(user_id);

        // If the peer is currently nearby and MLS is active, proactively send a
        // fresh key package so both sides can re-establish the session without
        // waiting for a new neighbor discovery cycle.
        if self.known_peers.contains(user_id)
            && self.config.encryption.enabled
            && self.config.encryption.auto_key_exchange
            && self.mls_manager.is_some()
        {
            if let Err(e) = self.send_key_package_to(user_id, true) {
                debug!(user_id = %user_id, error = %e, "Failed to send key package after unblock (peer may reconnect later)");
            }
        }

        info!(user_id = %user_id, "User unblocked");

        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::user_unblocked(user_id.to_string()));
        }

        Ok(())
    }

    /// Removes all MLS session state for a peer so the next encounter starts
    /// with a clean slate.
    ///
    /// Best-effort: individual cleanup failures are logged but do not prevent
    /// the unblock from succeeding.
    fn cleanup_peer_session_state(&mut self, user_id: &str) {
        // 1. Delete the MLS session (+ confirmed_sessions, confirmation
        //    tracking, welcome lifecycles, persisted session/welcome state).
        //    Only needed when MLS is initialized; steps 2-5 operate on
        //    in-memory maps that always exist regardless of MLS state.
        if self.mls_manager.is_some() {
            if let Err(e) = self.manual_mls_delete_session(user_id) {
                // Not an error — there may simply be no session to delete.
                debug!(user_id = %user_id, error = %e, "No MLS session to clean up for unblocked user");
            }
        }

        // 2. Discard any received key package we were holding for them.
        if self.pending_key_packages.remove(user_id).is_some() {
            debug!(user_id = %user_id, "Cleared pending key package for unblocked user");
        }
        self.delete_peer_key_package_from_storage(user_id);

        // 3. Drop queued outbound messages that were waiting for session
        //    establishment with this peer.
        if self.pending_encrypted_messages.remove(user_id).is_some() {
            debug!(user_id = %user_id, "Cleared pending encrypted messages for unblocked user");
        }
        self.clear_pending_messages_from_storage(user_id);

        // 4. Drain any inbound messages sitting in the pending decryption
        //    queue (encrypted messages received before the session was ready).
        let drained = self
            .pending_queue
            .drain_for_peer(&self.config.encryption.pending_queue, user_id);
        if !drained.is_empty() {
            debug!(
                user_id = %user_id,
                count = drained.len(),
                "Drained pending decryption queue for unblocked user"
            );
        }

        // 5. Cancel any in-progress inbound file transfers from this peer
        //    and remove their pending media metadata.
        let file_ids_to_cancel: Vec<String> = self
            .pending_media_metadata
            .iter()
            .filter(|(_, entry)| entry.sender == user_id)
            .map(|(file_id, _)| file_id.clone())
            .collect();
        for file_id in &file_ids_to_cancel {
            self.file_transfer_manager.cancel_transfer(file_id);
            self.pending_media_metadata.remove(file_id);
        }
        if !file_ids_to_cancel.is_empty() {
            debug!(
                user_id = %user_id,
                count = file_ids_to_cancel.len(),
                "Cancelled in-progress file transfers for unblocked user"
            );
        }

        // 6. Allow a fresh key exchange on re-discovery.
        self.key_package_sent_to.remove(user_id);
    }

    /// Returns the list of currently blocked user IDs, sorted for stable output.
    pub fn get_blocked_users(&self) -> Vec<String> {
        let mut users: Vec<String> = self.blocked_users.iter().cloned().collect();
        users.sort();
        users
    }

    /// Checks whether a user is currently blocked.
    pub fn is_user_blocked(&self, user_id: &str) -> bool {
        self.blocked_users.contains(user_id)
    }
}

#[cfg(test)]
mod tests {
    use crate::mls::InMemoryStorage;
    use crate::ProtocolConfig;
    use offline_protocol_transport::Transport;
    use std::sync::Arc;

    fn make_protocol(user_id: &str) -> crate::OfflineProtocol {
        let mut config = ProtocolConfig::new("test-app", user_id);
        // These tests exercise blocking and inbound file-transfer machinery
        // over legacy plaintext chunks; opt out of the fail-closed default
        // (SEC-M3) so the receive gate accepts them.
        config.encryption.require_encryption = false;
        crate::OfflineProtocol::new(config).unwrap()
    }

    #[test]
    fn test_block_unblock_roundtrip() {
        let mut proto = make_protocol("alice");
        assert!(!proto.is_user_blocked("bob"));
        assert!(proto.get_blocked_users().is_empty());

        proto.block_user("bob").unwrap();
        assert!(proto.is_user_blocked("bob"));
        let blocked = proto.get_blocked_users();
        assert_eq!(blocked.len(), 1);
        assert!(blocked.contains(&"bob".to_string()));

        proto.unblock_user("bob").unwrap();
        assert!(!proto.is_user_blocked("bob"));
        assert!(proto.get_blocked_users().is_empty());
    }

    #[test]
    fn test_block_self_rejected() {
        let mut proto = make_protocol("alice");
        let result = proto.block_user("alice");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot block own"));
    }

    #[test]
    fn test_block_invalid_user_id() {
        let mut proto = make_protocol("alice");
        let result = proto.block_user("");
        assert!(result.is_err());
    }

    #[test]
    fn test_block_idempotent() {
        let mut proto = make_protocol("alice");
        proto.block_user("bob").unwrap();
        proto.block_user("bob").unwrap(); // should succeed silently
        assert!(proto.is_user_blocked("bob"));
    }

    #[test]
    fn test_unblock_not_blocked() {
        let mut proto = make_protocol("alice");
        proto.unblock_user("bob").unwrap(); // should succeed silently
    }

    #[test]
    fn test_blocked_user_persistence() {
        let mut proto = make_protocol("alice");
        let storage = Arc::new(InMemoryStorage::new());
        proto.enable_message_persistence(storage).unwrap();

        proto.block_user("bob").unwrap();
        assert!(proto.is_user_blocked("bob"));

        // Clear in-memory set, then restore from storage
        proto.blocked_users.clear();
        assert!(!proto.is_user_blocked("bob"));

        proto.restore_blocked_users();
        assert!(proto.is_user_blocked("bob"));
    }

    #[test]
    fn test_receive_drops_blocked_sender() {
        use offline_protocol_core::{AppId, Message, UserId};
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");
        proto.block_user("mallory").unwrap();

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();

        // Queue a message from the blocked user addressed to us
        let msg = Message::new(
            UserId::new("mallory").unwrap(),
            UserId::new("alice").unwrap(),
            AppId::new("test-app").unwrap(),
            "you shouldn't see this",
        );
        mock.queue_message(msg);

        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        // receive_message should return None — the blocked message is silently dropped
        assert!(proto.receive_message().is_none());
    }

    #[test]
    fn test_relay_continues_for_blocked_user() {
        use crate::events::Event;
        use offline_protocol_core::{AppId, Message, UserId};
        use offline_protocol_transport::{mock::MockTransport, TransportType};
        use std::sync::{Arc, Mutex};

        let mut proto = make_protocol("alice");
        proto.block_user("mallory").unwrap();

        let relay_events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let relay_events_clone = relay_events.clone();
        proto.on_event(move |event| {
            if matches!(event, Event::MessageRelayed { .. }) {
                relay_events_clone.lock().unwrap().push(event);
            }
        });

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();

        // Message from blocked user but addressed to a THIRD party — should NOT be blocked.
        // With relay enabled, the message is forwarded and NOT returned to the app layer.
        let msg = Message::new(
            UserId::new("mallory").unwrap(),
            UserId::new("charlie").unwrap(),
            AppId::new("test-app").unwrap(),
            "relay this",
        );
        mock.queue_message(msg);

        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        // The message is for a third party — it gets relayed (forwarded) and not
        // returned to the local app. receive_message returns None because the loop
        // calls `continue` after relaying.
        let received = proto.receive_message();
        assert!(
            received.is_none(),
            "Relay messages for third parties should be forwarded, not returned"
        );

        // Verify that a MessageRelayed event was emitted — blocking must not
        // suppress relay forwarding for messages addressed to third parties.
        let events = relay_events.lock().unwrap();
        assert_eq!(
            events.len(),
            1,
            "Blocked-user relay to third party must emit MessageRelayed"
        );
        match &events[0] {
            Event::MessageRelayed {
                sender, recipient, ..
            } => {
                assert_eq!(sender, "mallory");
                assert_eq!(recipient, "charlie");
            }
            _ => panic!("Expected MessageRelayed event"),
        }
    }

    #[test]
    fn test_unblock_clears_session_state() {
        use crate::mls::InMemoryStorage as MlsInMemoryStorage;
        use offline_protocol_core::{AppId, Message, UserId};

        let mut proto = make_protocol("alice");
        let storage = Arc::new(MlsInMemoryStorage::new());
        proto.initialize_mls(storage).unwrap();

        use crate::protocol::types::ReceivedKeyPackage;

        // Simulate receiving a key package from bob (pending state)
        proto.pending_key_packages.insert(
            "bob".to_string(),
            ReceivedKeyPackage {
                key_package_data: vec![0x01, 0x02],
                local_expires_at_ms: u64::MAX,
            },
        );
        proto.key_package_sent_to.insert("bob".to_string());

        // Queue a pending encrypted message for bob
        let pending_msg = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            "queued",
        );
        proto.enqueue_pending_decryption("bob", &pending_msg);

        // Block then unblock — should clear all state
        proto.block_user("bob").unwrap();
        proto.unblock_user("bob").unwrap();

        assert!(proto.pending_key_packages.get("bob").is_none());
        assert!(!proto.key_package_sent_to.contains("bob"));
        assert!(!proto.confirmed_sessions.contains("bob"));
        assert!(proto
            .pending_queue
            .drain_for_peer(&proto.config.encryption.pending_queue, "bob")
            .is_empty());
    }

    #[test]
    fn test_unblock_then_receive() {
        use offline_protocol_core::{AppId, Message, UserId};
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");
        proto.block_user("bob").unwrap();
        proto.unblock_user("bob").unwrap();

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();

        let msg = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("alice").unwrap(),
            AppId::new("test-app").unwrap(),
            "hello again",
        );
        let msg_id = msg.id.clone();
        mock.queue_message(msg);

        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        // After unblocking, messages should flow normally
        let received = proto.receive_message();
        assert!(
            received.is_some(),
            "Messages should be received after unblocking"
        );
        assert_eq!(received.unwrap().id, msg_id);
    }

    #[test]
    fn test_send_to_blocked_user_rejected() {
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();
        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        proto.block_user("mallory").unwrap();

        let result = proto.send_message("mallory", "hello", None, None::<String>);
        assert!(matches!(
            result,
            Err(crate::Error::UserBlocked(ref id)) if id == "mallory"
        ));
    }

    #[test]
    fn test_send_via_transport_to_blocked_user_rejected() {
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();
        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        proto.block_user("mallory").unwrap();

        let result = proto.send_message_via_transport(
            "mallory",
            "hello",
            None,
            TransportType::BLE,
            None::<String>,
        );
        assert!(matches!(
            result,
            Err(crate::Error::UserBlocked(ref id)) if id == "mallory"
        ));
    }

    #[test]
    fn test_send_media_to_blocked_user_rejected() {
        use offline_protocol_core::ContentType;
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();
        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        proto.block_user("mallory").unwrap();

        let result = proto.send_media(
            "mallory",
            vec![0u8; 100],
            "photo.jpg",
            ContentType::Image,
            None,
        );
        assert!(matches!(
            result,
            Err(crate::Error::UserBlocked(ref id)) if id == "mallory"
        ));
    }

    #[test]
    fn test_pending_decryption_drops_blocked_sender() {
        use offline_protocol_core::{AppId, Message, UserId};

        let mut proto = make_protocol("alice");

        // Enqueue a pending message from bob before blocking
        let msg = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("alice").unwrap(),
            AppId::new("test-app").unwrap(),
            "secret",
        );
        proto.enqueue_pending_decryption("bob", &msg);

        // Now block bob
        proto.block_user("bob").unwrap();

        // process_pending_decryption should silently discard bob's messages
        proto.process_pending_decryption("bob");

        // Verify nothing was surfaced to the app
        let state = crate::protocol::lock_shared_state(&proto.shared_state).unwrap();
        assert!(
            state.received_messages.is_empty(),
            "Pending messages from blocked user should not be surfaced"
        );
    }

    #[test]
    fn test_duplicate_from_blocked_user_no_ack() {
        use offline_protocol_core::{AppId, Message, UserId};
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();

        // First, receive a normal message from mallory (before blocking)
        let msg = Message::new(
            UserId::new("mallory").unwrap(),
            UserId::new("alice").unwrap(),
            AppId::new("test-app").unwrap(),
            "first message",
        );
        let msg_clone = msg.clone();
        mock.queue_message(msg);

        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        // Receive the first message normally
        let received = proto.receive_message();
        assert!(received.is_some());

        // Now block mallory
        proto.block_user("mallory").unwrap();

        // Re-send the same message (duplicate) — should be silently discarded.
        // The dedup path should NOT send a re-ACK because the sender is blocked.
        let mut mock2 = MockTransport::new(TransportType::BLE);
        mock2.start().unwrap();
        mock2.queue_message(msg_clone);
        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock2));

        // This should return None without sending any ACK
        assert!(proto.receive_message().is_none());
    }

    #[test]
    fn test_get_blocked_users_sorted() {
        let mut proto = make_protocol("alice");
        proto.block_user("charlie").unwrap();
        proto.block_user("bob").unwrap();
        proto.block_user("dave").unwrap();

        let blocked = proto.get_blocked_users();
        assert_eq!(blocked, vec!["bob", "charlie", "dave"]);
    }

    #[test]
    fn test_presence_to_blocked_user_rejected() {
        use crate::events::PresenceStatus;
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();
        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        proto.block_user("mallory").unwrap();

        let result = proto.send_presence_update("mallory", PresenceStatus::Online);
        assert!(matches!(
            result,
            Err(crate::Error::UserBlocked(ref id)) if id == "mallory"
        ));
    }

    #[test]
    fn test_typing_indicator_to_blocked_user_rejected() {
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();
        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        proto.block_user("mallory").unwrap();

        let result = proto.send_typing_indicator("mallory", "conv-1", true);
        assert!(matches!(
            result,
            Err(crate::Error::UserBlocked(ref id)) if id == "mallory"
        ));
    }

    #[test]
    fn test_read_receipt_to_blocked_user_rejected() {
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();
        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        proto.block_user("mallory").unwrap();

        let result = proto.send_read_receipt("mallory", vec!["msg-1".to_string()]);
        assert!(matches!(
            result,
            Err(crate::Error::UserBlocked(ref id)) if id == "mallory"
        ));
    }

    #[test]
    fn test_block_cap_enforced() {
        use crate::protocol::types::MAX_BLOCKED_USERS;

        let mut proto = make_protocol("alice");
        for i in 0..MAX_BLOCKED_USERS {
            proto.block_user(&format!("user-{i}")).unwrap();
        }
        assert_eq!(proto.blocked_users.len(), MAX_BLOCKED_USERS);

        // One more should fail
        let result = proto.block_user("one-too-many");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Blocked users limit reached"));

        // But re-blocking an existing user should still succeed (idempotent)
        proto.block_user("user-0").unwrap();
    }

    #[test]
    fn test_on_neighbor_discovered_skips_blocked_user() {
        let mut proto = make_protocol("alice");
        proto.block_user("mallory").unwrap();

        // Calling on_neighbor_discovered for a blocked peer should NOT
        // add them to known_peers or trigger key exchange tracking.
        proto.on_neighbor_discovered("mallory");
        assert!(
            !proto.known_peers.contains("mallory"),
            "Blocked user should not be added to known_peers"
        );
        assert!(
            !proto.key_package_sent_to.contains("mallory"),
            "Blocked user should not trigger key package exchange"
        );

        // Non-blocked peer should be tracked normally
        proto.on_neighbor_discovered("bob");
        assert!(
            proto.known_peers.contains("bob"),
            "Non-blocked peer should be tracked"
        );
    }

    #[test]
    fn test_cleanup_cancels_inbound_file_transfers() {
        use crate::file_transfer::FileChunk;
        use offline_protocol_core::ContentType;

        let mut proto = make_protocol("alice");

        // Simulate an in-progress inbound file transfer from bob by inserting
        // a partial assembly and its pending media metadata.
        let chunk = FileChunk {
            file_id: "file-from-bob".to_string(),
            file_name: "photo.jpg".to_string(),
            file_size: 1000,
            total_chunks: 10,
            chunk_index: 0,
            chunk_data: vec![0u8; 100],
            file_checksum: "abc123".to_string(),
        };
        proto
            .file_transfer_manager
            .process_chunk("bob", chunk)
            .unwrap();

        use crate::protocol::types::PendingMediaMetadataEntry;
        use std::time::Instant;
        proto.pending_media_metadata.insert(
            "file-from-bob".to_string(),
            PendingMediaMetadataEntry {
                content_type: ContentType::Image,
                media_metadata: None,
                last_updated_at: Instant::now(),
                sender: "bob".to_string(),
            },
        );

        // Block then unblock — cleanup should cancel the transfer
        proto.block_user("bob").unwrap();
        proto.unblock_user("bob").unwrap();

        assert!(
            proto
                .file_transfer_manager
                .get_progress("file-from-bob")
                .is_none(),
            "Inbound file transfer should be cancelled on unblock cleanup"
        );
        assert!(
            !proto.pending_media_metadata.contains_key("file-from-bob"),
            "Pending media metadata should be removed on unblock cleanup"
        );
    }

    #[test]
    fn test_rejected_file_chunk_leaves_no_state() {
        use crate::file_transfer::FileChunk;
        use offline_protocol_core::{AppId, Message, UserId};

        let mut proto = make_protocol("alice");

        let chunk_message = |chunk: &FileChunk| {
            Message::new(
                UserId::new("bob").unwrap(),
                UserId::new("alice").unwrap(),
                AppId::new("test-app").unwrap(),
                chunk.to_json().unwrap(),
            )
        };

        // Control: an in-bounds chunk 0 is accepted and records its metadata.
        let good = FileChunk {
            file_id: "good-file".to_string(),
            file_name: "photo.jpg".to_string(),
            file_size: 11,
            total_chunks: 2,
            chunk_index: 0,
            chunk_data: vec![1u8; 6],
            file_checksum: "abc".to_string(),
        };
        proto.handle_incoming_file_chunk(&chunk_message(&good));
        assert!(proto.pending_media_metadata.contains_key("good-file"));
        assert_eq!(proto.file_transfer_manager.active_transfer_count(), 1);

        // SEC-H2: a chunk with an absurd file_size claim is rejected and
        // leaves neither an assembly nor pending media metadata behind.
        let evil = FileChunk {
            file_id: "evil-file".to_string(),
            file_name: "evil.bin".to_string(),
            file_size: u64::MAX,
            total_chunks: 1,
            chunk_index: 0,
            chunk_data: vec![0u8; 16],
            file_checksum: "def".to_string(),
        };
        proto.handle_incoming_file_chunk(&chunk_message(&evil));
        assert!(!proto.pending_media_metadata.contains_key("evil-file"));
        assert_eq!(proto.file_transfer_manager.active_transfer_count(), 1);
    }

    #[test]
    fn test_resource_rejected_file_chunk_emits_receive_failed_event() {
        use crate::events::Event;
        use crate::file_transfer::FileChunk;
        use offline_protocol_core::{AppId, Message, UserId};
        use std::sync::{Arc, Mutex};

        let mut proto = make_protocol("alice");
        proto.file_transfer_manager = crate::file_transfer::FileTransferManager::with_config(
            crate::file_transfer::FileTransferConfig {
                max_concurrent_assemblies: 1,
                ..crate::file_transfer::FileTransferConfig::default()
            },
        );

        let failed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let failed_events_clone = failed_events.clone();
        proto.on_event(move |event| {
            if matches!(event, Event::FileReceiveFailed { .. }) {
                failed_events_clone.lock().unwrap().push(event);
            }
        });

        let chunk_message = |chunk: &FileChunk| {
            Message::new(
                UserId::new("bob").unwrap(),
                UserId::new("alice").unwrap(),
                AppId::new("test-app").unwrap(),
                chunk.to_json().unwrap(),
            )
        };
        let make_chunk = |file_id: &str| FileChunk {
            file_id: file_id.to_string(),
            file_name: "photo.jpg".to_string(),
            file_size: 1000,
            total_chunks: 10,
            chunk_index: 0,
            chunk_data: vec![0u8; 100],
            file_checksum: "abc".to_string(),
        };

        // First transfer occupies the single assembly slot.
        proto.handle_incoming_file_chunk(&chunk_message(&make_chunk("file-1")));
        assert!(failed_events.lock().unwrap().is_empty());

        // The second transfer hits the cap: the app must be told the
        // transfer is lost (the chunk was already ACKed — it will not be
        // retransmitted).
        proto.handle_incoming_file_chunk(&chunk_message(&make_chunk("file-2")));
        let events = failed_events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::FileReceiveFailed {
                file_id,
                file_name,
                sender,
                reason,
            } => {
                assert_eq!(file_id, "file-2");
                assert_eq!(file_name, "photo.jpg");
                assert_eq!(sender, "bob");
                assert_eq!(reason, "too_many_transfers");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn test_resource_rejected_transfer_emits_single_failed_event() {
        use crate::events::Event;
        use crate::file_transfer::FileChunk;
        use offline_protocol_core::{AppId, Message, UserId};
        use std::sync::{Arc, Mutex};

        let mut proto = make_protocol("alice");
        proto.file_transfer_manager = crate::file_transfer::FileTransferManager::with_config(
            crate::file_transfer::FileTransferConfig {
                max_concurrent_assemblies: 1,
                ..crate::file_transfer::FileTransferConfig::default()
            },
        );

        let failed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let failed_events_clone = failed_events.clone();
        proto.on_event(move |event| {
            if matches!(event, Event::FileReceiveFailed { .. }) {
                failed_events_clone.lock().unwrap().push(event);
            }
        });

        let chunk_message = |chunk: &FileChunk| {
            Message::new(
                UserId::new("bob").unwrap(),
                UserId::new("alice").unwrap(),
                AppId::new("test-app").unwrap(),
                chunk.to_json().unwrap(),
            )
        };
        let make_chunk = |file_id: &str, index: u32| FileChunk {
            file_id: file_id.to_string(),
            file_name: "photo.jpg".to_string(),
            file_size: 1000,
            total_chunks: 10,
            chunk_index: index,
            chunk_data: vec![0u8; 100],
            file_checksum: "abc".to_string(),
        };

        // First transfer occupies the single assembly slot.
        proto.handle_incoming_file_chunk(&chunk_message(&make_chunk("file-1", 0)));

        // Every chunk of the second transfer was already ACKed and keeps
        // streaming in after the rejection. The app must be told the
        // transfer is lost exactly once — not once per remaining chunk.
        for index in 0..6u32 {
            proto.handle_incoming_file_chunk(&chunk_message(&make_chunk("file-2", index)));
        }

        let events = failed_events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::FileReceiveFailed {
                file_id, reason, ..
            } => {
                assert_eq!(file_id, "file-2");
                assert_eq!(reason, "too_many_transfers");
            }
            other => panic!("unexpected event: {:?}", other),
        }
        // And no ghost assembly formed for the failed transfer.
        assert_eq!(proto.file_transfer_manager.active_transfer_count(), 1);
        assert!(proto.file_transfer_manager.get_progress("file-2").is_none());
    }

    #[test]
    fn test_budget_dropped_transfer_emits_no_second_stale_event() {
        use crate::events::Event;
        use crate::file_transfer::FileChunk;
        use crate::protocol::MEDIA_TRANSFER_STALE_TIMEOUT_SECS;
        use offline_protocol_core::{AppId, Message, UserId};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let mut proto = make_protocol("alice");
        proto.file_transfer_manager = crate::file_transfer::FileTransferManager::with_config(
            crate::file_transfer::FileTransferConfig {
                max_file_size: 1_000_000,
                max_total_buffered_bytes: 1_065_536,
                ..crate::file_transfer::FileTransferConfig::default()
            },
        );

        let failed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let failed_events_clone = failed_events.clone();
        proto.on_event(move |event| {
            if matches!(event, Event::FileReceiveFailed { .. }) {
                failed_events_clone.lock().unwrap().push(event);
            }
        });

        let chunk_message = |chunk: &FileChunk| {
            Message::new(
                UserId::new("bob").unwrap(),
                UserId::new("alice").unwrap(),
                AppId::new("test-app").unwrap(),
                chunk.to_json().unwrap(),
            )
        };
        let make_chunk =
            |file_id: &str, file_size: u64, total: u32, index: u32, len: usize| FileChunk {
                file_id: file_id.to_string(),
                file_name: "big.bin".to_string(),
                file_size,
                total_chunks: total,
                chunk_index: index,
                chunk_data: vec![0u8; len],
                file_checksum: "abc".to_string(),
            };

        // f1 and f2 fill most of the budget; f1's next chunk busts it and
        // drops f1 with a buffer_budget_exhausted event.
        proto.handle_incoming_file_chunk(&chunk_message(&make_chunk(
            "f1", 1_000_000, 3, 0, 600_000,
        )));
        proto.handle_incoming_file_chunk(&chunk_message(&make_chunk("f2", 400_000, 2, 0, 300_000)));
        proto.handle_incoming_file_chunk(&chunk_message(&make_chunk(
            "f1", 1_000_000, 3, 1, 300_000,
        )));
        assert_eq!(failed_events.lock().unwrap().len(), 1);

        // f1's last in-flight chunk must not resurrect it as a
        // never-completable assembly.
        proto.handle_incoming_file_chunk(&chunk_message(&make_chunk(
            "f1", 1_000_000, 3, 2, 100_000,
        )));
        assert_eq!(proto.file_transfer_manager.active_transfer_count(), 1);
        assert!(proto.file_transfer_manager.get_progress("f1").is_none());

        // The stale sweep must not report f1 a second time; only f2 (still
        // partially assembled and now stale) fails.
        proto
            .file_transfer_manager
            .backdate_transfers(Duration::from_secs(MEDIA_TRANSFER_STALE_TIMEOUT_SECS + 1));
        proto.cleanup_expired_entries();

        let events = failed_events.lock().unwrap();
        let f1_reasons: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::FileReceiveFailed {
                    file_id, reason, ..
                } if file_id == "f1" => Some(reason.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(f1_reasons, vec!["buffer_budget_exhausted"]);
        let f2_reasons: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::FileReceiveFailed {
                    file_id, reason, ..
                } if file_id == "f2" => Some(reason.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(f2_reasons, vec!["stale_timeout"]);
    }

    #[test]
    fn test_malformed_chunk_emits_no_failed_event() {
        use crate::events::Event;
        use crate::file_transfer::FileChunk;
        use offline_protocol_core::{AppId, Message, UserId};
        use std::sync::{Arc, Mutex};

        let mut proto = make_protocol("alice");

        let failed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let failed_events_clone = failed_events.clone();
        proto.on_event(move |event| {
            if matches!(event, Event::FileReceiveFailed { .. }) {
                failed_events_clone.lock().unwrap().push(event);
            }
        });

        // An old-format sender chunking a max-size file below the 1 KiB
        // payload floor claims more chunks than the derived cap. Such
        // attacker-shaped input is dropped with only a warning log — no
        // FileReceiveFailed, no state.
        let chunk = FileChunk {
            file_id: "fine-chunked".to_string(),
            file_name: "old.bin".to_string(),
            file_size: 100 * 1024 * 1024,
            total_chunks: 200_000,
            chunk_index: 0,
            chunk_data: vec![0u8; 512],
            file_checksum: "abc".to_string(),
        };
        let message = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("alice").unwrap(),
            AppId::new("test-app").unwrap(),
            chunk.to_json().unwrap(),
        );
        proto.handle_incoming_file_chunk(&message);

        assert!(failed_events.lock().unwrap().is_empty());
        assert_eq!(proto.file_transfer_manager.active_transfer_count(), 0);
        assert!(!proto.pending_media_metadata.contains_key("fine-chunked"));
    }

    #[test]
    fn test_finalize_failure_clears_metadata_and_emits_event() {
        use crate::events::Event;
        use crate::file_transfer::FileChunk;
        use offline_protocol_core::{AppId, Message, UserId};
        use sha2::{Digest, Sha256};
        use std::sync::{Arc, Mutex};

        let mut proto = make_protocol("alice");

        let failed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let failed_events_clone = failed_events.clone();
        proto.on_event(move |event| {
            if matches!(event, Event::FileReceiveFailed { .. }) {
                failed_events_clone.lock().unwrap().push(event);
            }
        });

        // A single chunk whose checksum matches its bytes, but whose claimed
        // file_size does not — the transfer completes, then fails the
        // finalize size check.
        let data = vec![3u8; 100];
        let checksum = format!("{:x}", Sha256::digest(&data));
        let chunk = FileChunk {
            file_id: "bad-file".to_string(),
            file_name: "evil.bin".to_string(),
            file_size: 2048,
            total_chunks: 1,
            chunk_index: 0,
            chunk_data: data,
            file_checksum: checksum,
        };
        let message = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("alice").unwrap(),
            AppId::new("test-app").unwrap(),
            chunk.to_json().unwrap(),
        );
        proto.handle_incoming_file_chunk(&message);

        // The failed transfer leaves no assembly and no pending metadata.
        assert_eq!(proto.file_transfer_manager.active_transfer_count(), 0);
        assert!(!proto.pending_media_metadata.contains_key("bad-file"));

        let events = failed_events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::FileReceiveFailed {
                file_id, reason, ..
            } => {
                assert_eq!(file_id, "bad-file");
                assert_eq!(reason, "integrity_check_failed");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn test_stale_transfer_sweep_emits_receive_failed_event() {
        use crate::events::Event;
        use crate::file_transfer::FileChunk;
        use crate::protocol::MEDIA_TRANSFER_STALE_TIMEOUT_SECS;
        use offline_protocol_core::{AppId, Message, UserId};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let mut proto = make_protocol("alice");

        let failed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let failed_events_clone = failed_events.clone();
        proto.on_event(move |event| {
            if matches!(event, Event::FileReceiveFailed { .. }) {
                failed_events_clone.lock().unwrap().push(event);
            }
        });

        // A partial inbound transfer from bob (1 of 2 chunks).
        let chunk = FileChunk {
            file_id: "stale-file".to_string(),
            file_name: "photo.jpg".to_string(),
            file_size: 200,
            total_chunks: 2,
            chunk_index: 0,
            chunk_data: vec![0u8; 100],
            file_checksum: "abc".to_string(),
        };
        let message = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("alice").unwrap(),
            AppId::new("test-app").unwrap(),
            chunk.to_json().unwrap(),
        );
        proto.handle_incoming_file_chunk(&message);
        assert_eq!(proto.file_transfer_manager.active_transfer_count(), 1);
        assert!(proto.pending_media_metadata.contains_key("stale-file"));

        // Age the assembly past the stale timeout and run the periodic
        // sweep — the app must hear about the dropped transfer (its chunks
        // were ACKed and will never be retransmitted).
        proto
            .file_transfer_manager
            .backdate_transfers(Duration::from_secs(MEDIA_TRANSFER_STALE_TIMEOUT_SECS + 1));
        proto.cleanup_expired_entries();

        assert_eq!(proto.file_transfer_manager.active_transfer_count(), 0);
        assert!(!proto.pending_media_metadata.contains_key("stale-file"));
        let events = failed_events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::FileReceiveFailed {
                file_id,
                file_name,
                sender,
                reason,
            } => {
                assert_eq!(file_id, "stale-file");
                assert_eq!(file_name, "photo.jpg");
                assert_eq!(sender, "bob");
                assert_eq!(reason, "stale_timeout");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn test_connection_request_to_blocked_user_rejected() {
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();
        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        proto.block_user("mallory").unwrap();

        let result = proto.send_connection_request("mallory", "Alice", None);
        assert!(matches!(
            result,
            Err(crate::Error::UserBlocked(ref id)) if id == "mallory"
        ));
    }

    #[test]
    fn test_accept_connection_request_to_blocked_user_rejected() {
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();
        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        proto.block_user("mallory").unwrap();

        let result = proto.accept_connection_request("mallory", "Alice", None);
        assert!(matches!(
            result,
            Err(crate::Error::UserBlocked(ref id)) if id == "mallory"
        ));
    }

    #[test]
    fn test_reject_connection_request_to_blocked_user_rejected() {
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();
        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        proto.block_user("mallory").unwrap();

        let result = proto.reject_connection_request("mallory");
        assert!(matches!(
            result,
            Err(crate::Error::UserBlocked(ref id)) if id == "mallory"
        ));
    }

    #[test]
    fn test_restore_blocked_users_skips_invalid_entries() {
        use crate::mls::InMemoryStorage;
        use offline_protocol_mls::MlsStorage;

        let mut proto = make_protocol("alice");
        let storage = Arc::new(InMemoryStorage::new());
        proto.enable_message_persistence(storage.clone()).unwrap();

        // Directly write valid and invalid entries into storage
        storage.store("blocked_users", "bob", &[]).unwrap();
        storage.store("blocked_users", "", &[]).unwrap(); // invalid: empty user ID

        proto.blocked_users.clear();
        proto.restore_blocked_users();

        // "bob" should be restored, "" should be skipped
        assert!(proto.is_user_blocked("bob"));
        assert!(!proto.is_user_blocked(""));
        assert_eq!(proto.blocked_users.len(), 1);
    }

    #[test]
    fn test_session_reset_key_package_clears_stale_session() {
        use crate::mls::InMemoryStorage as MlsInMemoryStorage;
        use crate::protocol::types::KeyPackagePayload;

        // Set up alice with MLS
        let mut alice = make_protocol("alice");
        let alice_storage = Arc::new(MlsInMemoryStorage::new());
        alice.initialize_mls(alice_storage).unwrap();

        // Set up bob with MLS
        let mut bob = make_protocol("bob");
        let bob_storage = Arc::new(MlsInMemoryStorage::new());
        bob.initialize_mls(bob_storage).unwrap();

        // Exchange key packages to establish a session
        let alice_key_pkg = {
            let mls = alice.mls_manager.as_ref().unwrap();
            let manager = mls.read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        // Bob imports Alice's key package and creates session
        {
            let mls = bob.mls_manager.as_ref().unwrap();
            let manager = mls.read().unwrap();
            manager
                .import_key_package("alice", &alice_key_pkg.key_package_data)
                .unwrap();
            let welcome = manager.create_session("alice").unwrap();
            // Alice joins via welcome
            let alice_mls = alice.mls_manager.as_ref().unwrap();
            let alice_manager = alice_mls.read().unwrap();
            alice_manager.join_session(&welcome).unwrap();
        }

        // Verify both have sessions
        {
            let mls = alice.mls_manager.as_ref().unwrap();
            assert!(mls.read().unwrap().has_session("bob").unwrap());
        }
        {
            let mls = bob.mls_manager.as_ref().unwrap();
            assert!(mls.read().unwrap().has_session("alice").unwrap());
        }

        // Now simulate: Alice blocks then unblocks Bob.
        // Alice's side deletes her session (done by unblock_user internally).
        alice.block_user("bob").unwrap();
        alice.unblock_user("bob").unwrap();

        // Verify Alice no longer has a session with Bob
        {
            let mls = alice.mls_manager.as_ref().unwrap();
            assert!(
                !mls.read().unwrap().has_session("bob").unwrap(),
                "Alice should have no session after unblock cleanup"
            );
        }

        // Bob still has the stale session
        {
            let mls = bob.mls_manager.as_ref().unwrap();
            assert!(
                mls.read().unwrap().has_session("alice").unwrap(),
                "Bob still has the old session before receiving reset"
            );
        }

        // Simulate Bob receiving Alice's key package with session_reset=true
        let alice_fresh_key_pkg = {
            let mls = alice.mls_manager.as_ref().unwrap();
            let manager = mls.read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        let reset_payload = KeyPackagePayload {
            user_id: "alice".to_string(),
            key_package_data: alice_fresh_key_pkg.key_package_data.clone(),
            remaining_lifetime_ms: alice_fresh_key_pkg.remaining_lifetime_ms(),
            timestamp_ms: 0,
            session_reset: true,
        };
        let content = serde_json::to_string(&reset_payload).unwrap();

        // Bob handles the key package with session_reset=true.
        // This deletes the stale session and auto-establishes a fresh one
        // using Alice's new key package.
        bob.handle_key_package_message("alice", &content);

        // Bob should have a session (the NEW one, auto-established from
        // Alice's fresh key package — NOT the stale orphaned session).
        {
            let mls = bob.mls_manager.as_ref().unwrap();
            let manager = mls.read().unwrap();
            assert!(
                manager.has_session("alice").unwrap(),
                "Bob should have a fresh session after session_reset + auto-establish"
            );
        }

        // The old confirmed_sessions entry should be gone (cleared during
        // session delete), proving the stale session was replaced.
        assert!(
            !bob.confirmed_sessions.contains("alice"),
            "Old confirmed session entry should be cleared"
        );

        // The pending key package should have been consumed by auto-establish
        assert!(
            !bob.pending_key_packages.contains_key("alice"),
            "Key package should be consumed after auto-establish"
        );
    }

    #[test]
    fn test_reset_tofu_for_peer_clears_mls_session() {
        use crate::mls::InMemoryStorage as MlsInMemoryStorage;

        // Establish an alice→bob MLS session (alice owns the group).
        let mut alice = make_protocol("alice");
        alice
            .initialize_mls(Arc::new(MlsInMemoryStorage::new()))
            .unwrap();
        let mut bob = make_protocol("bob");
        bob.initialize_mls(Arc::new(MlsInMemoryStorage::new()))
            .unwrap();

        let bob_key_pkg = {
            let mls = bob.mls_manager.as_ref().unwrap();
            let manager = mls.read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        {
            let mls = alice.mls_manager.as_ref().unwrap();
            let manager = mls.read().unwrap();
            manager
                .import_key_package("bob", &bob_key_pkg.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap();
            assert!(manager.has_session("bob").unwrap());
        }

        // Pin bob's TOFU key (real API) so the reset engages and returns true.
        alice.tofu_check_or_pin("bob", vec![7u8; 32]).unwrap();

        // Reset re-identifies bob: unpin the key AND drop the now-stale session.
        assert!(alice.reset_tofu_for_peer("bob"));
        assert!(!alice.known_peer_public_keys.contains_key("bob"));
        {
            let mls = alice.mls_manager.as_ref().unwrap();
            assert!(
                !mls.read().unwrap().has_session("bob").unwrap(),
                "reset_tofu_for_peer must drop the stale MLS session"
            );
        }
    }

    #[test]
    fn test_reset_tofu_for_peer_surfaces_session_delete_failure_but_still_unpins() {
        use crate::mls::{InMemoryStorage as MlsInMemoryStorage, MlsStorage};
        use offline_protocol_mls::storage::{StorageError, StorageResult};
        use std::sync::atomic::{AtomicBool, Ordering};

        // An MLS storage that delegates to an in-memory store but can be flipped
        // to reject every `delete`, simulating a secure-store backend (iOS
        // Keychain / Android Keystore) that fails to remove session state. This
        // is the *real-failure* arm of the session drop — distinct from the
        // benign "no session" / "MLS not initialized" no-ops, which return `Ok`
        // and `Err(MlsNotInitialized)` respectively.
        struct FailOnDeleteStorage {
            inner: MlsInMemoryStorage,
            fail_delete: AtomicBool,
        }
        impl MlsStorage for FailOnDeleteStorage {
            fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> StorageResult<()> {
                self.inner.store(key_type, key_id, data)
            }
            fn load(&self, key_type: &str, key_id: &str) -> StorageResult<Option<Vec<u8>>> {
                self.inner.load(key_type, key_id)
            }
            fn delete(&self, key_type: &str, key_id: &str) -> StorageResult<()> {
                if self.fail_delete.load(Ordering::SeqCst) {
                    return Err(StorageError::DeleteFailed(
                        "injected delete failure".to_string(),
                    ));
                }
                self.inner.delete(key_type, key_id)
            }
            fn list_keys(&self, key_type: &str) -> StorageResult<Vec<String>> {
                self.inner.list_keys(key_type)
            }
        }

        let storage = Arc::new(FailOnDeleteStorage {
            inner: MlsInMemoryStorage::new(),
            fail_delete: AtomicBool::new(false),
        });

        // Establish an alice→bob MLS session (alice owns the group), same shape
        // as the happy-path test above.
        let mut alice = make_protocol("alice");
        alice.initialize_mls(storage.clone()).unwrap();
        let mut bob = make_protocol("bob");
        bob.initialize_mls(Arc::new(MlsInMemoryStorage::new()))
            .unwrap();

        let bob_key_pkg = {
            let mls = bob.mls_manager.as_ref().unwrap();
            let manager = mls.read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        {
            let mls = alice.mls_manager.as_ref().unwrap();
            let manager = mls.read().unwrap();
            manager
                .import_key_package("bob", &bob_key_pkg.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap();
            assert!(manager.has_session("bob").unwrap());
        }
        alice.tofu_check_or_pin("bob", vec![7u8; 32]).unwrap();

        // Flip storage to reject deletes, then reset — the session drop now
        // genuinely fails.
        storage.fail_delete.store(true, Ordering::SeqCst);

        // Best-effort contract: the un-pin is committed before the drop is
        // attempted, so the reset still reports success and removes the pinned
        // key even though the session drop failed (surfaced via `warn!`, not
        // rolled back).
        assert!(
            alice.reset_tofu_for_peer("bob"),
            "reset commits the TOFU un-pin before the session drop, so it still returns true"
        );
        assert!(
            !alice.known_peer_public_keys.contains_key("bob"),
            "the pinned key must be removed even when the session drop fails"
        );
        // And the hazard the `warn!` flags is real: the stale session outlived
        // the un-pin because its deletion failed. `has_session` only reads, so
        // it is unaffected by the delete fault.
        {
            let mls = alice.mls_manager.as_ref().unwrap();
            assert!(
                mls.read().unwrap().has_session("bob").unwrap(),
                "the stale MLS session survives a failed drop — exactly what the warn! surfaces"
            );
        }
    }
}
