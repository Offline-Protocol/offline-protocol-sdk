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

        // 5. Allow a fresh key exchange on re-discovery.
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
        let config = ProtocolConfig::new("test-app", user_id);
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
        use offline_protocol_core::{AppId, Message, UserId};
        use offline_protocol_transport::{mock::MockTransport, TransportType};

        let mut proto = make_protocol("alice");
        proto.block_user("mallory").unwrap();

        let mut mock = MockTransport::new(TransportType::BLE);
        mock.start().unwrap();

        // Message from blocked user but addressed to a THIRD party — should NOT be blocked
        let msg = Message::new(
            UserId::new("mallory").unwrap(),
            UserId::new("charlie").unwrap(),
            AppId::new("test-app").unwrap(),
            "relay this",
        );
        let msg_id = msg.id.clone();
        mock.queue_message(msg);

        proto
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        proto.start().unwrap();

        // receive_message should return the message since it's not addressed to us
        let received = proto.receive_message();
        assert!(
            received.is_some(),
            "Relay messages for third parties must not be blocked"
        );
        assert_eq!(received.unwrap().id, msg_id);
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
        assert!(proto.pending_queue.drain_for_peer(&proto.config.encryption.pending_queue, "bob").is_empty());
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
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Cannot send message to blocked user"),
        );
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
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Cannot send message to blocked user"),
        );
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
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot send presence to blocked user"));
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
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot send typing indicator to blocked user"));
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
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot send read receipt to blocked user"));
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
}
