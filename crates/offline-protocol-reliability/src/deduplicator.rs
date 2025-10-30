//! Message deduplicator to prevent forwarding loops

use lru::LruCache;
use offline_protocol_core::MessageId;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use tracing::debug;

/// Configuration for deduplicator
#[derive(Debug, Clone)]
pub struct DeduplicatorConfig {
    pub cache_size: usize,
}

impl Default for DeduplicatorConfig {
    fn default() -> Self {
        Self {
            cache_size: 1000,
        }
    }
}

/// Deduplicates messages based on message ID
pub struct Deduplicator {
    cache: Mutex<LruCache<MessageId, ()>>,
}

impl Deduplicator {
    pub fn new(config: DeduplicatorConfig) -> Self {
        let cache_size = NonZeroUsize::new(config.cache_size).unwrap_or(NonZeroUsize::new(1000).unwrap());
        
        Self {
            cache: Mutex::new(LruCache::new(cache_size)),
        }
    }

    /// Check if a message is a duplicate and mark it as seen
    ///
    /// Returns true if this is a duplicate (already seen), false if it's new
    pub fn is_duplicate(&self, message_id: MessageId) -> bool {
        let mut cache = self.cache.lock();
        
        if cache.contains(&message_id) {
            debug!("Duplicate message detected: {}", message_id);
            true
        } else {
            cache.put(message_id, ());
            false
        }
    }

    /// Mark a message as seen without checking
    pub fn mark_seen(&self, message_id: MessageId) {
        let mut cache = self.cache.lock();
        cache.put(message_id, ());
    }

    /// Check if a message has been seen before
    pub fn has_seen(&self, message_id: MessageId) -> bool {
        let cache = self.cache.lock();
        cache.contains(&message_id)
    }

    /// Clear the cache
    pub fn clear(&self) {
        let mut cache = self.cache.lock();
        cache.clear();
    }

    /// Get the number of messages in the cache
    pub fn len(&self) -> usize {
        let cache = self.cache.lock();
        cache.len()
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        let cache = self.cache.lock();
        cache.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deduplicator() {
        let dedup = Deduplicator::new(DeduplicatorConfig::default());
        let msg_id = MessageId::new();

        // First time should not be a duplicate
        assert!(!dedup.is_duplicate(msg_id));

        // Second time should be a duplicate
        assert!(dedup.is_duplicate(msg_id));

        // Third time should still be a duplicate
        assert!(dedup.is_duplicate(msg_id));
    }

    #[test]
    fn test_deduplicator_lru() {
        let config = DeduplicatorConfig { cache_size: 3 };
        let dedup = Deduplicator::new(config);

        let msg1 = MessageId::new();
        let msg2 = MessageId::new();
        let msg3 = MessageId::new();
        let msg4 = MessageId::new();

        // Add first three messages
        assert!(!dedup.is_duplicate(msg1));
        assert!(!dedup.is_duplicate(msg2));
        assert!(!dedup.is_duplicate(msg3));

        // All should be in cache
        assert!(dedup.has_seen(msg1));
        assert!(dedup.has_seen(msg2));
        assert!(dedup.has_seen(msg3));

        // Adding fourth should evict the first
        assert!(!dedup.is_duplicate(msg4));
        assert!(!dedup.has_seen(msg1)); // msg1 should be evicted
        assert!(dedup.has_seen(msg2));
        assert!(dedup.has_seen(msg3));
        assert!(dedup.has_seen(msg4));
    }
}

