//! Message deduplication using rotating bloom filters for O(1) memory.
//!
//! This module implements a scalable deduplication system using bloom filters
//! that can handle millions of messages with constant memory usage.
//!
//! The rotating bloom filter approach uses multiple time-windowed filters:
//! - Current filter: accepts new message IDs
//! - Previous filters: used for lookups, rotated out after retention period

use chrono::{DateTime, Utc};
use offline_protocol_core::MessageId;
use std::collections::HashMap;

/// Configuration for deduplication.
#[derive(Debug, Clone)]
pub struct DeduplicatorConfig {
    /// Maximum number of message IDs to track (used for HashMap fallback).
    pub max_tracked_messages: usize,

    /// Time to keep message IDs in memory (seconds).
    pub retention_time_secs: u64,

    /// Enable bloom filter mode for scalable deduplication.
    pub use_bloom_filter: bool,

    /// Number of bits in each bloom filter (default: 2^20 = ~1MB per filter).
    pub bloom_filter_bits: usize,

    /// Number of hash functions for bloom filter (default: 7 for ~1% false positive).
    pub bloom_hash_count: usize,

    /// Number of rotating bloom filters (default: 4 for hourly windows).
    pub bloom_filter_count: usize,

    /// Rotation interval in seconds (default: 900 = 15 minutes).
    pub bloom_rotation_secs: u64,
}

impl Default for DeduplicatorConfig {
    fn default() -> Self {
        Self {
            // Exact-match mode by default: no false positives (Bloom can drop ~1% of legitimate messages).
            max_tracked_messages: 1000,
            retention_time_secs: 3600, // 1 hour
            use_bloom_filter: false,
            bloom_filter_bits: 1 << 20, // ~1MB per filter (1,048,576 bits)
            bloom_hash_count: 7,        // ~1% false positive rate when Bloom is enabled
            bloom_filter_count: 4,      // 4 windows for 1-hour retention
            bloom_rotation_secs: 900,   // 15 minutes per window
        }
    }
}

/// A simple bloom filter implementation.
#[derive(Clone)]
struct BloomFilter {
    bits: Vec<u64>,
    bit_count: usize,
    hash_count: usize,
    items_added: usize,
}

impl BloomFilter {
    fn new(bit_count: usize, hash_count: usize) -> Self {
        let word_count = (bit_count + 63) / 64;
        Self {
            bits: vec![0u64; word_count],
            bit_count,
            hash_count,
            items_added: 0,
        }
    }

    /// Computes multiple hash values for an item using double hashing.
    fn hashes(&self, item: &str) -> Vec<usize> {
        let mut hashes = Vec::with_capacity(self.hash_count);

        // First hash using FNV-1a
        let h1 = fnv1a_hash(item.as_bytes());
        // Second hash using a different seed
        let h2 = fnv1a_hash_seeded(item.as_bytes(), 0x5f356495);

        for i in 0..self.hash_count {
            // Kirsch-Mitzenmacher optimization: h(i) = h1 + i * h2
            let combined = h1.wrapping_add((i as u64).wrapping_mul(h2));
            hashes.push((combined as usize) % self.bit_count);
        }

        hashes
    }

    fn insert(&mut self, item: &str) {
        for bit_index in self.hashes(item) {
            let word_index = bit_index / 64;
            let bit_offset = bit_index % 64;
            self.bits[word_index] |= 1u64 << bit_offset;
        }
        self.items_added += 1;
    }

    fn contains(&self, item: &str) -> bool {
        for bit_index in self.hashes(item) {
            let word_index = bit_index / 64;
            let bit_offset = bit_index % 64;
            if (self.bits[word_index] & (1u64 << bit_offset)) == 0 {
                return false;
            }
        }
        true
    }

    fn clear(&mut self) {
        self.bits.fill(0);
        self.items_added = 0;
    }

    fn items_count(&self) -> usize {
        self.items_added
    }

    /// Estimates the current false positive probability.
    fn estimated_false_positive_rate(&self) -> f64 {
        if self.items_added == 0 {
            return 0.0;
        }
        let m = self.bit_count as f64;
        let n = self.items_added as f64;
        let k = self.hash_count as f64;
        (1.0 - (-k * n / m).exp()).powf(k)
    }
}

/// FNV-1a hash function.
fn fnv1a_hash(data: &[u8]) -> u64 {
    fnv1a_hash_seeded(data, 0xcbf29ce484222325)
}

/// FNV-1a hash with custom seed.
fn fnv1a_hash_seeded(data: &[u8], seed: u64) -> u64 {
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = seed;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// A rotating bloom filter with time windows.
struct RotatingBloomFilter {
    filters: Vec<BloomFilter>,
    current_index: usize,
    last_rotation: DateTime<Utc>,
    rotation_interval_secs: u64,
}

impl RotatingBloomFilter {
    fn new(filter_count: usize, bit_count: usize, hash_count: usize, rotation_secs: u64) -> Self {
        let filters = (0..filter_count)
            .map(|_| BloomFilter::new(bit_count, hash_count))
            .collect();
        Self {
            filters,
            current_index: 0,
            last_rotation: Utc::now(),
            rotation_interval_secs: rotation_secs,
        }
    }

    fn maybe_rotate(&mut self, now: DateTime<Utc>) {
        let elapsed = now.signed_duration_since(self.last_rotation);
        if elapsed.num_seconds() >= self.rotation_interval_secs as i64 {
            // Move to next filter and clear it
            self.current_index = (self.current_index + 1) % self.filters.len();
            self.filters[self.current_index].clear();
            self.last_rotation = now;
        }
    }

    fn insert(&mut self, item: &str) {
        self.maybe_rotate(Utc::now());
        self.filters[self.current_index].insert(item);
    }

    fn contains(&mut self, item: &str) -> bool {
        self.maybe_rotate(Utc::now());
        // Check all filters (current and previous windows)
        self.filters.iter().any(|f| f.contains(item))
    }

    fn total_items(&self) -> usize {
        self.filters.iter().map(|f| f.items_count()).sum()
    }

    fn clear(&mut self) {
        for filter in &mut self.filters {
            filter.clear();
        }
        self.current_index = 0;
        self.last_rotation = Utc::now();
    }

    fn average_false_positive_rate(&self) -> f64 {
        let rates: Vec<f64> = self
            .filters
            .iter()
            .map(|f| f.estimated_false_positive_rate())
            .collect();
        if rates.is_empty() {
            return 0.0;
        }
        rates.iter().sum::<f64>() / rates.len() as f64
    }
}

/// Entry for a seen message (used in HashMap mode).
#[derive(Debug, Clone)]
struct SeenEntry {
    /// When this message was first seen.
    seen_at: DateTime<Utc>,
    /// When this entry was last accessed (for LRU eviction).
    last_accessed: DateTime<Utc>,
}

/// Deduplicator for tracking seen messages and preventing duplicates.
///
/// Supports two modes:
/// - HashMap mode: exact tracking with FIFO eviction (original behavior)
/// - Bloom filter mode: O(1) memory with probabilistic duplicate detection
pub struct Deduplicator {
    config: DeduplicatorConfig,
    /// HashMap for exact tracking (fallback mode).
    seen_messages: HashMap<String, SeenEntry>,
    /// Rotating bloom filter for scalable tracking.
    bloom_filter: Option<RotatingBloomFilter>,
}

impl Deduplicator {
    /// Creates a new deduplicator with default configuration.
    pub fn new() -> Self {
        Self::with_config(DeduplicatorConfig::default())
    }

    /// Creates a new deduplicator with custom configuration.
    pub fn with_config(config: DeduplicatorConfig) -> Self {
        let bloom_filter = if config.use_bloom_filter {
            Some(RotatingBloomFilter::new(
                config.bloom_filter_count,
                config.bloom_filter_bits,
                config.bloom_hash_count,
                config.bloom_rotation_secs,
            ))
        } else {
            None
        };

        Self {
            config,
            seen_messages: HashMap::new(),
            bloom_filter,
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
    ///
    /// Note: In bloom filter mode, this may return false positives (claiming a
    /// message is a duplicate when it isn't). The false positive rate is typically
    /// below 1% with default settings.
    pub fn is_duplicate(&self, message_id: &MessageId) -> bool {
        let msg_id_str = message_id.as_str();

        if let Some(ref bloom) = self.bloom_filter {
            // Note: We need mutable access for rotation check, but this is safe
            // because we're only reading. We'll handle rotation in mark_seen.
            // For now, just check all filters.
            bloom.filters.iter().any(|f| f.contains(&msg_id_str))
        } else {
            self.seen_messages.contains_key(&msg_id_str)
        }
    }

    /// Checks if a message has been seen before (mutable version for bloom rotation
    /// and LRU access tracking).
    pub fn is_duplicate_mut(&mut self, message_id: &MessageId) -> bool {
        let msg_id_str = message_id.as_str();

        if let Some(ref mut bloom) = self.bloom_filter {
            bloom.contains(&msg_id_str)
        } else if let Some(entry) = self.seen_messages.get_mut(&msg_id_str) {
            entry.last_accessed = Utc::now();
            true
        } else {
            false
        }
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

        if let Some(ref mut bloom) = self.bloom_filter {
            // Check if already seen
            if bloom.contains(&msg_id_str) {
                return false;
            }
            // Add to bloom filter
            bloom.insert(&msg_id_str);
            true
        } else {
            // HashMap mode (original implementation)
            if self.seen_messages.contains_key(&msg_id_str) {
                return false;
            }

            // Check if we've hit the limit
            if self.seen_messages.len() >= self.config.max_tracked_messages {
                self.cleanup_expired();

                if self.seen_messages.len() >= self.config.max_tracked_messages {
                    // LRU eviction: evict the least recently accessed entry
                    if let Some((lru_id, _)) = self
                        .seen_messages
                        .iter()
                        .min_by_key(|(_, entry)| entry.last_accessed)
                        .map(|(id, entry)| (id.clone(), entry.clone()))
                    {
                        self.seen_messages.remove(&lru_id);
                    }
                }
            }

            let now = Utc::now();
            self.seen_messages.insert(
                msg_id_str,
                SeenEntry {
                    seen_at: now,
                    last_accessed: now,
                },
            );

            true
        }
    }

    /// Removes expired entries that exceed the retention time.
    ///
    /// # Returns
    ///
    /// Returns the number of entries removed.
    ///
    /// Note: In bloom filter mode, expiration is handled automatically through
    /// filter rotation, so this returns 0.
    pub fn cleanup_expired(&mut self) -> usize {
        if self.bloom_filter.is_some() {
            // Bloom filter handles expiration through rotation
            return 0;
        }

        let now = Utc::now();
        let retention = chrono::Duration::seconds(self.config.retention_time_secs as i64);
        let cutoff = now - retention;

        let before_count = self.seen_messages.len();
        self.seen_messages.retain(|_, entry| entry.seen_at > cutoff);
        before_count - self.seen_messages.len()
    }

    /// Gets the number of messages currently being tracked.
    ///
    /// Note: In bloom filter mode, this returns an estimate based on insertion count.
    pub fn tracked_count(&self) -> usize {
        if let Some(ref bloom) = self.bloom_filter {
            bloom.total_items()
        } else {
            self.seen_messages.len()
        }
    }

    /// Clears all tracked messages.
    pub fn clear(&mut self) {
        if let Some(ref mut bloom) = self.bloom_filter {
            bloom.clear();
        }
        self.seen_messages.clear();
    }

    /// Checks if a message ID is being tracked (seen).
    pub fn is_tracked(&self, message_id: &MessageId) -> bool {
        self.is_duplicate(message_id)
    }

    /// Returns whether bloom filter mode is active.
    pub fn is_bloom_filter_mode(&self) -> bool {
        self.bloom_filter.is_some()
    }

    /// Gets statistics about the deduplicator.
    pub fn stats(&self) -> DeduplicatorStats {
        if let Some(ref bloom) = self.bloom_filter {
            let total = bloom.total_items();
            // Estimate "capacity" based on expected items before FP rate gets too high
            let estimated_capacity = self.config.bloom_filter_bits / self.config.bloom_hash_count;
            let capacity_percent =
                ((total as f32 / estimated_capacity as f32) * 100.0).min(100.0) as u8;

            DeduplicatorStats {
                total_tracked: total,
                recent_tracked: total, // In bloom mode, we can't distinguish recent
                capacity_used_percent: capacity_percent,
                false_positive_rate: Some(bloom.average_false_positive_rate()),
                mode: DeduplicatorMode::BloomFilter,
            }
        } else {
            let now = Utc::now();
            let recent_cutoff = now - chrono::Duration::seconds(60);

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
                false_positive_rate: None,
                mode: DeduplicatorMode::HashMap,
            }
        }
    }

    /// Gets the estimated memory usage in bytes.
    pub fn estimated_memory_bytes(&self) -> usize {
        if self.bloom_filter.is_some() {
            // Each filter uses bit_count/8 bytes, times number of filters
            let filter_bytes = (self.config.bloom_filter_bits + 7) / 8;
            filter_bytes * self.config.bloom_filter_count
                + std::mem::size_of::<RotatingBloomFilter>()
        } else {
            // Rough estimate: each entry is ~50 bytes (String + DateTime)
            self.seen_messages.len() * 50 + std::mem::size_of::<HashMap<String, SeenEntry>>()
        }
    }
}

impl Default for Deduplicator {
    fn default() -> Self {
        Self::new()
    }
}

/// The mode of operation for the deduplicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeduplicatorMode {
    /// Using HashMap for exact tracking (original behavior).
    HashMap,
    /// Using bloom filters for O(1) memory.
    BloomFilter,
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
    /// Estimated false positive rate (only in bloom filter mode).
    pub false_positive_rate: Option<f64>,
    /// Current operating mode.
    pub mode: DeduplicatorMode,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    fn hashmap_config() -> DeduplicatorConfig {
        DeduplicatorConfig {
            use_bloom_filter: false,
            ..Default::default()
        }
    }

    #[test]
    fn test_default_config_is_hashmap_and_capacity_1000() {
        let config = DeduplicatorConfig::default();
        assert!(
            !config.use_bloom_filter,
            "default should be exact-match HashMap to avoid false positives"
        );
        assert_eq!(config.max_tracked_messages, 1000);

        let dedup = Deduplicator::new();
        assert!(!dedup.is_bloom_filter_mode());
        let stats = dedup.stats();
        assert_eq!(stats.mode, DeduplicatorMode::HashMap);
    }

    #[test]
    fn test_default_hashmap_no_false_positives() {
        let mut dedup = Deduplicator::new();
        let seen = MessageId::new();
        dedup.mark_seen(seen.clone());

        // Brand-new ID must never be reported as duplicate (HashMap is exact match)
        for _ in 0..20 {
            let fresh = MessageId::new();
            assert!(
                !dedup.is_duplicate(&fresh),
                "fresh message ID must not be false positive"
            );
        }
        assert!(dedup.is_duplicate(&seen));
    }

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
    fn test_basic_deduplication_hashmap_mode() {
        let mut dedup = Deduplicator::with_config(hashmap_config());
        let msg_id = MessageId::new();

        assert!(!dedup.is_duplicate(&msg_id));
        assert!(dedup.mark_seen(msg_id.clone()));

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
        // HashMap mode required for FIFO eviction
        let config = DeduplicatorConfig {
            max_tracked_messages: 3,
            retention_time_secs: 3600,
            use_bloom_filter: false,
            ..Default::default()
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
        // HashMap mode required for explicit cleanup
        let config = DeduplicatorConfig {
            max_tracked_messages: 100,
            retention_time_secs: 1,
            use_bloom_filter: false,
            ..Default::default()
        };
        let mut dedup = Deduplicator::with_config(config);

        let msg1 = MessageId::new();
        let msg2 = MessageId::new();

        dedup.mark_seen(msg1.clone());

        thread::sleep(Duration::from_millis(500));

        dedup.mark_seen(msg2.clone());

        thread::sleep(Duration::from_millis(600));

        let removed = dedup.cleanup_expired();
        assert_eq!(removed, 1);
        assert_eq!(dedup.tracked_count(), 1);

        assert!(!dedup.is_duplicate(&msg1));
        assert!(dedup.is_duplicate(&msg2));
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
    }

    #[test]
    fn test_stats_hashmap_mode() {
        let mut dedup = Deduplicator::with_config(hashmap_config());

        for _ in 0..5 {
            dedup.mark_seen(MessageId::new());
        }

        let stats = dedup.stats();
        assert_eq!(stats.total_tracked, 5);
        assert_eq!(stats.recent_tracked, 5);
        assert!(stats.capacity_used_percent < 1);
        assert_eq!(stats.mode, DeduplicatorMode::HashMap);
    }

    #[test]
    fn test_capacity_percent() {
        let config = DeduplicatorConfig {
            max_tracked_messages: 10,
            retention_time_secs: 3600,
            use_bloom_filter: false,
            ..Default::default()
        };
        let mut dedup = Deduplicator::with_config(config);

        for _ in 0..5 {
            dedup.mark_seen(MessageId::new());
        }

        let stats = dedup.stats();
        assert_eq!(stats.capacity_used_percent, 50);
    }

    #[test]
    fn test_bloom_filter_mode() {
        let config = DeduplicatorConfig {
            use_bloom_filter: true,
            bloom_filter_bits: 1 << 16, // Smaller for testing
            bloom_hash_count: 5,
            bloom_filter_count: 2,
            bloom_rotation_secs: 60,
            ..Default::default()
        };
        let mut dedup = Deduplicator::with_config(config);

        assert!(dedup.is_bloom_filter_mode());

        let msg1 = MessageId::new();
        let msg2 = MessageId::new();

        assert!(!dedup.is_duplicate(&msg1));
        dedup.mark_seen(msg1.clone());
        assert!(dedup.is_duplicate(&msg1));

        assert!(!dedup.is_duplicate(&msg2));
        dedup.mark_seen(msg2.clone());
        assert!(dedup.is_duplicate(&msg2));

        let stats = dedup.stats();
        assert_eq!(stats.mode, DeduplicatorMode::BloomFilter);
        assert!(stats.false_positive_rate.is_some());
    }

    #[test]
    fn test_bloom_filter_constant_memory() {
        let config = DeduplicatorConfig {
            use_bloom_filter: true,
            bloom_filter_bits: 1 << 16,
            bloom_hash_count: 5,
            bloom_filter_count: 2,
            bloom_rotation_secs: 3600,
            ..Default::default()
        };
        let dedup = Deduplicator::with_config(config);

        let initial_memory = dedup.estimated_memory_bytes();

        // Memory should be roughly constant regardless of items added
        // (Unlike HashMap which grows with items)
        let expected_filter_bytes = (1 << 16) / 8 * 2; // 2 filters, 64KB each
        assert!(initial_memory >= expected_filter_bytes);
        assert!(initial_memory < expected_filter_bytes * 2); // Some overhead
    }

    #[test]
    fn test_bloom_filter_high_volume() {
        let config = DeduplicatorConfig {
            use_bloom_filter: true,
            bloom_filter_bits: 1 << 20, // 1MB
            bloom_hash_count: 7,
            bloom_filter_count: 4,
            bloom_rotation_secs: 3600,
            ..Default::default()
        };
        let mut dedup = Deduplicator::with_config(config);

        // Insert 10,000 messages - should work with O(1) memory
        for _ in 0..10_000 {
            let msg = MessageId::new();
            assert!(dedup.mark_seen(msg));
        }

        assert_eq!(dedup.tracked_count(), 10_000);

        let stats = dedup.stats();
        // False positive rate should still be low
        assert!(stats.false_positive_rate.unwrap() < 0.02);
    }

    #[test]
    fn test_lru_eviction_preserves_recently_accessed() {
        let config = DeduplicatorConfig {
            max_tracked_messages: 3,
            retention_time_secs: 3600,
            use_bloom_filter: false,
            ..Default::default()
        };
        let mut dedup = Deduplicator::with_config(config);

        let msg_a = MessageId::new();
        let msg_b = MessageId::new();
        let msg_c = MessageId::new();

        dedup.mark_seen(msg_a.clone());
        thread::sleep(Duration::from_millis(10));
        dedup.mark_seen(msg_b.clone());
        thread::sleep(Duration::from_millis(10));
        dedup.mark_seen(msg_c.clone());

        // Re-access A so it becomes the most recently accessed
        assert!(dedup.is_duplicate_mut(&msg_a));

        // Insert D — should evict B (least recently accessed), not A
        let msg_d = MessageId::new();
        dedup.mark_seen(msg_d.clone());

        assert_eq!(dedup.tracked_count(), 3);
        assert!(
            dedup.is_duplicate(&msg_a),
            "A was recently accessed, should survive"
        );
        assert!(
            !dedup.is_duplicate(&msg_b),
            "B should have been evicted (LRU)"
        );
        assert!(dedup.is_duplicate(&msg_c), "C should survive");
        assert!(dedup.is_duplicate(&msg_d), "D was just inserted");
    }
}
