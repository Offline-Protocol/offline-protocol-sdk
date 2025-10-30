//! Message deduplication using hash-based tracking.

use chrono::{DateTime, Utc};
use offline_protocol_core::MessageId;
use std::collections::HashMap;

/// Configuration for deduplication.
#[derive(Debug, Clone)]
pub struct DeduplicatorConfig {
    /// Maximum number of message IDs to track.
    pub max_tracked_messages: usize,

    /// Time to keep message IDs in memory (seconds).
    pub retention_time_secs: u64,
}

impl Default for DeduplicatorConfig {
    fn default() -> Self {
        Self {
            max_tracked_messages: 10000,
            retention_time_secs: 3600, // 1 hour
        }
    }
}

/// Entry for a seen message.
#[derive(Debug, Clone)]
struct SeenEntry {
    /// When this message was first seen.
    seen_at: DateTime<Utc>,
}

/// Deduplicator for tracking seen messages and preventing duplicates.
pub struct Deduplicator {
    config: DeduplicatorConfig,
    seen_messages: HashMap<String, SeenEntry>,
}

impl Deduplicator {
    /// Creates a new deduplicator with default configuration.
    pub fn new() -> Self {
        Self::with_config(DeduplicatorConfig::default())
    }

    /// Creates a new deduplicator with custom configuration.
    pub fn with_config(config: DeduplicatorConfig) -> Self {
        Self {
            config,
            seen_messages: HashMap::new(),
        }
    }

    /// Checks if a message has been seen before.
    ///
    /// # Arguments
    ///
    /// * `message_id` - The message ID to check
    ///
    /// # Returns
    ///
    /// Returns `true` if the message has been seen, `false` otherwise.
    pub fn is_duplicate(&self, message_id: &MessageId) -> bool {
        self.seen_messages.contains_key(&message_id.as_str())
    }

    /// Marks a message as seen.
    ///
    /// # Arguments
    ///
    /// * `message_id` - The message ID to mark as seen
    ///
    /// # Returns
    ///
    /// Returns `true` if this is a new message, `false` if it was already seen.
    pub fn mark_seen(&mut self, message_id: MessageId) -> bool {
        let msg_id_str = message_id.as_str();

        if self.seen_messages.contains_key(&msg_id_str) {
            // Already seen
            return false;
        }

        // Check if we've hit the limit
        if self.seen_messages.len() >= self.config.max_tracked_messages {
            // Clean up old entries first
            self.cleanup_expired();

            // If still at limit, remove oldest entry (FIFO)
            if self.seen_messages.len() >= self.config.max_tracked_messages {
                if let Some((oldest_id, _)) = self
                    .seen_messages
                    .iter()
                    .min_by_key(|(_, entry)| entry.seen_at)
                    .map(|(id, entry)| (id.clone(), entry.clone()))
                {
                    self.seen_messages.remove(&oldest_id);
                }
            }
        }

        // Mark as seen
        self.seen_messages.insert(
            msg_id_str,
            SeenEntry {
                seen_at: Utc::now(),
            },
        );

        true
    }

    /// Removes expired entries that exceed the retention time.
    ///
    /// # Returns
    ///
    /// Returns the number of entries removed.
    pub fn cleanup_expired(&mut self) -> usize {
        let now = Utc::now();
        let retention = chrono::Duration::seconds(self.config.retention_time_secs as i64);
        let cutoff = now - retention;

        let before_count = self.seen_messages.len();

        self.seen_messages.retain(|_, entry| entry.seen_at > cutoff);

        before_count - self.seen_messages.len()
    }

    /// Gets the number of messages currently being tracked.
    pub fn tracked_count(&self) -> usize {
        self.seen_messages.len()
    }

    /// Clears all tracked messages.
    pub fn clear(&mut self) {
        self.seen_messages.clear();
    }

    /// Checks if a message ID is being tracked (seen).
    pub fn is_tracked(&self, message_id: &MessageId) -> bool {
        self.seen_messages.contains_key(&message_id.as_str())
    }

    /// Gets statistics about the deduplicator.
    pub fn stats(&self) -> DeduplicatorStats {
        let now = Utc::now();
        let recent_cutoff = now - chrono::Duration::seconds(60); // Last minute

        let recent_count = self
            .seen_messages
            .values()
            .filter(|entry| entry.seen_at > recent_cutoff)
            .count();

        DeduplicatorStats {
            total_tracked: self.seen_messages.len(),
            recent_tracked: recent_count,
            capacity_used_percent: (self.seen_messages.len() as f32
                / self.config.max_tracked_messages as f32
                * 100.0) as u8,
        }
    }
}

impl Default for Deduplicator {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the deduplicator.
#[derive(Debug, Clone)]
pub struct DeduplicatorStats {
    /// Total number of messages being tracked.
    pub total_tracked: usize,
    /// Number of messages seen in the last minute.
    pub recent_tracked: usize,
    /// Percentage of capacity used (0-100).
    pub capacity_used_percent: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_basic_deduplication() {
        let mut dedup = Deduplicator::new();
        let msg_id = MessageId::new();

        // First time should not be duplicate
        assert!(!dedup.is_duplicate(&msg_id));
        assert!(dedup.mark_seen(msg_id.clone()));

        // Second time should be duplicate
        assert!(dedup.is_duplicate(&msg_id));
        assert!(!dedup.mark_seen(msg_id.clone()));
    }

    #[test]
    fn test_multiple_messages() {
        let mut dedup = Deduplicator::new();

        let msg1 = MessageId::new();
        let msg2 = MessageId::new();
        let msg3 = MessageId::new();

        assert!(dedup.mark_seen(msg1.clone()));
        assert!(dedup.mark_seen(msg2.clone()));
        assert!(dedup.mark_seen(msg3.clone()));

        assert_eq!(dedup.tracked_count(), 3);

        assert!(dedup.is_duplicate(&msg1));
        assert!(dedup.is_duplicate(&msg2));
        assert!(dedup.is_duplicate(&msg3));
    }

    #[test]
    fn test_capacity_limit() {
        let config = DeduplicatorConfig {
            max_tracked_messages: 3,
            retention_time_secs: 3600,
        };
        let mut dedup = Deduplicator::with_config(config);

        let msg1 = MessageId::new();
        let msg2 = MessageId::new();
        let msg3 = MessageId::new();
        let msg4 = MessageId::new();

        dedup.mark_seen(msg1.clone());
        dedup.mark_seen(msg2.clone());
        dedup.mark_seen(msg3.clone());

        assert_eq!(dedup.tracked_count(), 3);

        // Adding fourth message should evict oldest
        dedup.mark_seen(msg4.clone());
        assert_eq!(dedup.tracked_count(), 3);

        // msg1 should have been evicted
        assert!(!dedup.is_duplicate(&msg1));
        assert!(dedup.is_duplicate(&msg4));
    }

    #[test]
    fn test_cleanup_expired() {
        let config = DeduplicatorConfig {
            max_tracked_messages: 100,
            retention_time_secs: 1, // 1 second for testing
        };
        let mut dedup = Deduplicator::with_config(config);

        let msg1 = MessageId::new();
        let msg2 = MessageId::new();

        dedup.mark_seen(msg1.clone());

        // Wait a bit
        thread::sleep(Duration::from_millis(500));

        dedup.mark_seen(msg2.clone());

        // Wait for msg1 to expire
        thread::sleep(Duration::from_millis(600));

        let removed = dedup.cleanup_expired();
        assert_eq!(removed, 1); // msg1 should be removed
        assert_eq!(dedup.tracked_count(), 1);

        assert!(!dedup.is_duplicate(&msg1)); // Expired
        assert!(dedup.is_duplicate(&msg2)); // Still valid
    }

    #[test]
    fn test_clear() {
        let mut dedup = Deduplicator::new();

        dedup.mark_seen(MessageId::new());
        dedup.mark_seen(MessageId::new());
        dedup.mark_seen(MessageId::new());

        assert_eq!(dedup.tracked_count(), 3);

        dedup.clear();
        assert_eq!(dedup.tracked_count(), 0);
    }

    #[test]
    fn test_is_tracked() {
        let mut dedup = Deduplicator::new();
        let msg_id = MessageId::new();

        assert!(!dedup.is_tracked(&msg_id));

        dedup.mark_seen(msg_id.clone());
        assert!(dedup.is_tracked(&msg_id));
    }

    #[test]
    fn test_stats() {
        let mut dedup = Deduplicator::new();

        for _ in 0..5 {
            dedup.mark_seen(MessageId::new());
        }

        let stats = dedup.stats();
        assert_eq!(stats.total_tracked, 5);
        assert_eq!(stats.recent_tracked, 5); // All recent
        assert!(stats.capacity_used_percent < 1); // Very small percentage of 10000
    }

    #[test]
    fn test_capacity_percent() {
        let config = DeduplicatorConfig {
            max_tracked_messages: 10,
            retention_time_secs: 3600,
        };
        let mut dedup = Deduplicator::with_config(config);

        for _ in 0..5 {
            dedup.mark_seen(MessageId::new());
        }

        let stats = dedup.stats();
        assert_eq!(stats.capacity_used_percent, 50); // 5/10 = 50%
    }
}
