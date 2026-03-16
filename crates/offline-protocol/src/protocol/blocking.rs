//! User blocking: silent block list that filters incoming messages, control
//! messages, discovery events, and connection requests from blocked users.

use super::{lock_shared_state, OfflineProtocol};
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

        info!(user_id = %user_id, "User unblocked");

        if let Ok(state) = lock_shared_state(&self.shared_state) {
            state.emit_event(Event::user_unblocked(user_id.to_string()));
        }

        Ok(())
    }

    /// Returns the list of currently blocked user IDs.
    pub fn get_blocked_users(&self) -> Vec<String> {
        self.blocked_users.iter().cloned().collect()
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
}
