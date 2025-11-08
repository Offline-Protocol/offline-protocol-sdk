//! ACK (acknowledgment) management for tracking pending acknowledgments.

use chrono::{DateTime, Utc};
use offline_protocol_core::MessageId;
use std::collections::HashMap;
use std::time::Duration;

/// Status of an ACK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckStatus {
    /// Waiting for ACK.
    Pending,
    /// ACK received.
    Received,
    /// ACK timed out.
    TimedOut,
}

/// Information about a pending ACK.
#[derive(Debug, Clone)]
pub struct PendingAck {
    /// Message ID waiting for ACK.
    pub message_id: MessageId,

    /// When the message was sent.
    pub sent_at: DateTime<Utc>,

    /// Timeout for this ACK (in milliseconds).
    pub timeout_ms: u64,

    /// Current status.
    pub status: AckStatus,

    /// Number of times the message has been retried.
    pub retry_count: u32,
}

impl PendingAck {
    /// Checks if this ACK has timed out.
    pub fn is_timed_out(&self) -> bool {
        if self.status != AckStatus::Pending {
            return false;
        }

        let elapsed = Utc::now().signed_duration_since(self.sent_at);
        elapsed.num_milliseconds() >= self.timeout_ms as i64
    }
}

/// Configuration for ACK management.
#[derive(Debug, Clone)]
pub struct AckConfig {
    /// Default ACK timeout in milliseconds.
    pub default_timeout_ms: u64,

    /// Maximum number of pending ACKs to track.
    pub max_pending_acks: usize,
}

impl Default for AckConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 5000, // 5 seconds
            max_pending_acks: 1000,
        }
    }
}

/// ACK manager for tracking pending acknowledgments.
pub struct AckManager {
    config: AckConfig,
    pending_acks: HashMap<MessageId, PendingAck>,
}

impl AckManager {
    /// Creates a new ACK manager with default configuration.
    pub fn new() -> Self {
        Self::with_config(AckConfig::default())
    }

    /// Creates a new ACK manager with custom configuration.
    pub fn with_config(config: AckConfig) -> Self {
        Self {
            config,
            pending_acks: HashMap::new(),
        }
    }

    /// Registers a message as waiting for ACK.
    ///
    /// # Arguments
    ///
    /// * `message_id` - ID of the message waiting for ACK
    /// * `timeout_ms` - Optional custom timeout (uses default if None)
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if registered, `Err` if ACK limit exceeded.
    pub fn register_pending_ack(
        &mut self,
        message_id: MessageId,
        timeout_ms: Option<u64>,
    ) -> crate::Result<()> {
        // Check if we've hit the limit
        if self.pending_acks.len() >= self.config.max_pending_acks {
            // Clean up timed out ACKs first
            self.cleanup_timed_out();

            // If still at limit, return error
            if self.pending_acks.len() >= self.config.max_pending_acks {
                return Err(crate::Error::Other(
                    "Maximum pending ACKs limit reached".to_string(),
                ));
            }
        }

        let pending = PendingAck {
            message_id: message_id.clone(),
            sent_at: Utc::now(),
            timeout_ms: timeout_ms.unwrap_or(self.config.default_timeout_ms),
            status: AckStatus::Pending,
            retry_count: 0,
        };

        self.pending_acks.insert(message_id, pending);
        Ok(())
    }

    /// Records that an ACK was received for a message.
    ///
    /// # Arguments
    ///
    /// * `message_id` - ID of the message that was acknowledged
    ///
    /// # Returns
    ///
    /// Returns `true` if the ACK was expected and recorded, `false` if not found.
    pub fn record_ack_received(&mut self, message_id: &MessageId) -> bool {
        if let Some(pending) = self.pending_acks.get_mut(message_id) {
            pending.status = AckStatus::Received;
            true
        } else {
            false
        }
    }

    /// Drains all ACKs that have timed out and marks them as timed out.
    pub fn drain_timed_out(&mut self) -> Vec<PendingAck> {
        let mut timed_out = Vec::new();
        for pending in self.pending_acks.values_mut() {
            if pending.is_timed_out() {
                pending.status = AckStatus::TimedOut;
                timed_out.push(pending.clone());
            }
        }
        timed_out
    }

    /// Removes an ACK from tracking (e.g., after successful delivery).
    pub fn remove_ack(&mut self, message_id: &MessageId) -> Option<PendingAck> {
        self.pending_acks.remove(message_id)
    }

    /// Gets all timed out ACKs.
    ///
    /// # Returns
    ///
    /// Returns a vector of message IDs that have timed out.
    pub fn get_timed_out_acks(&self) -> Vec<MessageId> {
        self.pending_acks
            .values()
            .filter(|ack| ack.is_timed_out())
            .map(|ack| ack.message_id.clone())
            .collect()
    }

    /// Updates the retry count for a message.
    pub fn increment_retry_count(&mut self, message_id: &MessageId) {
        if let Some(pending) = self.pending_acks.get_mut(message_id) {
            pending.retry_count += 1;
            pending.sent_at = Utc::now(); // Reset timer for retry
            pending.status = AckStatus::Pending;
        }
    }

    /// Gets information about a pending ACK.
    pub fn get_pending_ack(&self, message_id: &MessageId) -> Option<&PendingAck> {
        self.pending_acks.get(message_id)
    }

    /// Gets the number of pending ACKs.
    pub fn pending_count(&self) -> usize {
        self.pending_acks.len()
    }

    /// Checks if a message is waiting for ACK.
    pub fn is_waiting_for_ack(&self, message_id: &MessageId) -> bool {
        self.pending_acks.contains_key(message_id)
    }

    /// Cleans up timed out ACKs by marking them as timed out.
    fn cleanup_timed_out(&mut self) {
        self.drain_timed_out();
    }

    /// Removes all ACKs with TimedOut status that are older than a given duration.
    ///
    /// This helps prevent unbounded memory growth.
    pub fn prune_old_timeouts(&mut self, older_than: Duration) {
        let now = Utc::now();
        let cutoff = now - chrono::Duration::from_std(older_than).unwrap();

        self.pending_acks
            .retain(|_, ack| ack.status != AckStatus::TimedOut || ack.sent_at > cutoff);
    }
}

impl Default for AckManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_register_and_receive_ack() {
        let mut manager = AckManager::new();
        let msg_id = MessageId::new();

        // Register pending ACK
        manager.register_pending_ack(msg_id.clone(), None).unwrap();
        assert!(manager.is_waiting_for_ack(&msg_id));
        assert_eq!(manager.pending_count(), 1);

        // Receive ACK
        assert!(manager.record_ack_received(&msg_id));

        let pending = manager.get_pending_ack(&msg_id).unwrap();
        assert_eq!(pending.status, AckStatus::Received);
    }

    #[test]
    fn test_ack_timeout() {
        let config = AckConfig {
            default_timeout_ms: 50, // 50ms for fast test
            max_pending_acks: 100,
        };
        let mut manager = AckManager::with_config(config);
        let msg_id = MessageId::new();

        manager
            .register_pending_ack(msg_id.clone(), Some(50))
            .unwrap();

        // Not timed out immediately
        assert!(manager.drain_timed_out().is_empty());

        // Wait for timeout
        thread::sleep(Duration::from_millis(60));

        // Should be timed out now
        let timed_out = manager.drain_timed_out();
        assert_eq!(timed_out.len(), 1);
        assert_eq!(timed_out[0].message_id, msg_id);
    }

    #[test]
    fn test_retry_count_increment() {
        let mut manager = AckManager::new();
        let msg_id = MessageId::new();

        manager.register_pending_ack(msg_id.clone(), None).unwrap();

        let pending = manager.get_pending_ack(&msg_id).unwrap();
        assert_eq!(pending.retry_count, 0);

        manager.increment_retry_count(&msg_id);

        let pending = manager.get_pending_ack(&msg_id).unwrap();
        assert_eq!(pending.retry_count, 1);
    }

    #[test]
    fn test_remove_ack() {
        let mut manager = AckManager::new();
        let msg_id = MessageId::new();

        manager.register_pending_ack(msg_id.clone(), None).unwrap();
        assert!(manager.is_waiting_for_ack(&msg_id));

        manager.remove_ack(&msg_id);
        assert!(!manager.is_waiting_for_ack(&msg_id));
    }

    #[test]
    fn test_max_pending_limit() {
        let config = AckConfig {
            default_timeout_ms: 5000,
            max_pending_acks: 3,
        };
        let mut manager = AckManager::with_config(config);

        // Should be able to register up to limit
        manager
            .register_pending_ack(MessageId::new(), None)
            .unwrap();
        manager
            .register_pending_ack(MessageId::new(), None)
            .unwrap();
        manager
            .register_pending_ack(MessageId::new(), None)
            .unwrap();

        // Fourth should fail
        let result = manager.register_pending_ack(MessageId::new(), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_prune_old_timeouts() {
        let config = AckConfig {
            default_timeout_ms: 10,
            max_pending_acks: 100,
        };
        let mut manager = AckManager::with_config(config);

        let msg_id = MessageId::new();
        manager
            .register_pending_ack(msg_id.clone(), Some(10))
            .unwrap();

        // Wait for timeout
        thread::sleep(Duration::from_millis(20));
        manager.cleanup_timed_out();

        assert_eq!(manager.pending_count(), 1);

        // Prune old timeouts
        manager.prune_old_timeouts(Duration::from_millis(5));
        assert_eq!(manager.pending_count(), 0);
    }

    #[test]
    fn test_ack_not_found() {
        let mut manager = AckManager::new();
        let msg_id = MessageId::new();

        // Recording ACK for non-existent message should return false
        assert!(!manager.record_ack_received(&msg_id));
    }
}
