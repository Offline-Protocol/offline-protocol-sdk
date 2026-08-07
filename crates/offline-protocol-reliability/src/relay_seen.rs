//! Suppression cache for messages that have already been forwarded.
//!
//! A forwarding node sees the whole neighborhood's traffic, not just its own:
//! every frame that crosses it arrives once per link that carries it. Forwarding
//! each arrival would turn one message into as many copies as the network has
//! edges, and those copies would arrive back at nodes that already forwarded
//! them, and so on. This cache is what stops that — the record that says "this
//! id has been handled, ignore further arrivals."
//!
//! It is deliberately **separate from [`Deduplicator`](crate::Deduplicator)**,
//! which answers a different question. The deduplicator tracks messages
//! addressed to *us*, and its entries are load-bearing for delivery semantics:
//! entries are removed again (`unmark_seen`) when a message could not be
//! delivered, so the sender's retry is reprocessed rather than silently
//! re-acknowledged. Forwarding traffic has no such lifecycle — an id is either
//! handled or not — and mixing the two populations would let relay volume evict
//! the delivery-critical entries.
//!
//! # Sizing
//!
//! Capacity has to cover the whole *network's* traffic for as long as copies of
//! a message can still be in flight, not just our own. Two windows matter: a
//! flood's lifetime (bounded by hop limit times per-hop latency, so seconds),
//! and the sender's early retransmissions (tens of seconds). The defaults cover
//! both with margin, and are chosen so that at the maximum rate a node will
//! accept forwarding traffic, the cache still holds far more than
//! [`DEFAULT_RELAY_SEEN_RETENTION_SECS`] of it — meaning entries expire by age
//! rather than being pushed out by volume. That ordering is what makes the
//! suppression guarantee hold: an id evicted early would be forwarded a second
//! time, and by every other node too, which is how a flood becomes a storm.
//!
//! Eviction is O(1): entries leave in insertion order, which for a
//! fixed retention window is also expiry order.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Maximum ids tracked before the oldest is dropped.
pub const DEFAULT_RELAY_SEEN_CAPACITY: usize = 8192;

/// How long an id stays suppressed.
pub const DEFAULT_RELAY_SEEN_RETENTION_SECS: u64 = 600;

/// Configuration for [`RelaySeenCache`].
#[derive(Debug, Clone)]
pub struct RelaySeenConfig {
    /// Maximum ids tracked at once.
    pub capacity: usize,
    /// How long an id stays suppressed.
    pub retention: Duration,
}

impl Default for RelaySeenConfig {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_RELAY_SEEN_CAPACITY,
            retention: Duration::from_secs(DEFAULT_RELAY_SEEN_RETENTION_SECS),
        }
    }
}

/// What a cache lookup found for an id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeenOutcome {
    /// Not seen before; it has now been recorded.
    Fresh,
    /// Seen before, within the retention window.
    Duplicate,
}

/// Bounded, insertion-ordered record of recently handled message ids.
#[derive(Debug)]
pub struct RelaySeenCache {
    config: RelaySeenConfig,
    entries: HashMap<String, Instant>,
    order: VecDeque<String>,
    duplicates_suppressed: u64,
    capacity_evictions: u64,
}

impl RelaySeenCache {
    /// Creates a cache with the default sizing.
    pub fn new() -> Self {
        Self::with_config(RelaySeenConfig::default())
    }

    /// Creates a cache with explicit sizing.
    pub fn with_config(config: RelaySeenConfig) -> Self {
        let capacity = config.capacity.max(1);
        Self {
            config: RelaySeenConfig { capacity, ..config },
            entries: HashMap::with_capacity(capacity.min(1024)),
            order: VecDeque::with_capacity(capacity.min(1024)),
            duplicates_suppressed: 0,
            capacity_evictions: 0,
        }
    }

    /// Records `message_id`, reporting whether it had been seen already.
    ///
    /// This is the single entry point: checking and recording are one step so
    /// no caller can test an id and then forget to record it, which would
    /// forward every copy.
    pub fn observe(&mut self, message_id: &str) -> SeenOutcome {
        self.expire(Instant::now());

        if self.entries.contains_key(message_id) {
            self.duplicates_suppressed = self.duplicates_suppressed.saturating_add(1);
            return SeenOutcome::Duplicate;
        }

        while self.entries.len() >= self.config.capacity {
            if self.pop_oldest().is_some() {
                self.capacity_evictions = self.capacity_evictions.saturating_add(1);
            } else {
                break;
            }
        }

        self.entries.insert(message_id.to_string(), Instant::now());
        self.order.push_back(message_id.to_string());
        SeenOutcome::Fresh
    }

    /// Forgets `message_id`, so a later copy is treated as new again.
    ///
    /// For a frame that was recorded as handled and then dropped **without
    /// ever being transmitted** — displaced from a queue, abandoned for
    /// waiting too long, refused room on the way back. The record would
    /// otherwise refuse every later copy for the rest of the retention window,
    /// including the sender's own retransmissions, which carry the same id: a
    /// route closed for ten minutes by a few seconds of congestion, with
    /// nothing on the air to justify it.
    ///
    /// Safe only because nothing was transmitted. An id forgotten after the
    /// frame reached a neighbor could be forwarded a second time — by every
    /// node — which is how a flood becomes a storm, so callers must be certain
    /// the frame travelled nowhere.
    pub fn forget(&mut self, message_id: &str) {
        if self.entries.remove(message_id).is_some() {
            // Keep the insertion-order queue in lockstep with the map. Leaving
            // the stale entry would be tolerated by `expire`, but it would also
            // be counted as a capacity eviction by `observe`, and that counter
            // is the signal that the cache is undersized.
            self.order.retain(|id| id != message_id);
        }
    }

    /// Whether `message_id` is currently suppressed, without recording it.
    pub fn contains(&self, message_id: &str) -> bool {
        match self.entries.get(message_id) {
            Some(seen_at) => seen_at.elapsed() < self.config.retention,
            None => false,
        }
    }

    /// Drops entries older than the retention window.
    ///
    /// Called automatically by [`Self::observe`]; exposed so an idle node can
    /// release memory from its process tick.
    pub fn expire(&mut self, now: Instant) {
        while let Some(oldest_id) = self.order.front() {
            let expired = match self.entries.get(oldest_id) {
                Some(seen_at) => now.duration_since(*seen_at) >= self.config.retention,
                // Not in the map: a stale order entry, drop it.
                None => true,
            };
            if !expired {
                break;
            }
            self.pop_oldest();
        }
    }

    /// Number of ids currently tracked.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds no ids.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many arrivals were suppressed as already-handled.
    pub fn duplicates_suppressed(&self) -> u64 {
        self.duplicates_suppressed
    }

    /// How many ids were dropped for capacity rather than age.
    ///
    /// Expected to stay at zero: an id evicted while copies of it are still in
    /// flight would be forwarded again. A rising count means the cache is too
    /// small for the traffic the node is seeing.
    pub fn capacity_evictions(&self) -> u64 {
        self.capacity_evictions
    }

    /// Removes every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    fn pop_oldest(&mut self) -> Option<String> {
        let oldest = self.order.pop_front()?;
        self.entries.remove(&oldest);
        Some(oldest)
    }
}

impl Default for RelaySeenCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sighting_is_fresh_and_repeats_are_duplicates() {
        let mut cache = RelaySeenCache::new();
        assert_eq!(cache.observe("m1"), SeenOutcome::Fresh);
        assert_eq!(cache.observe("m1"), SeenOutcome::Duplicate);
        assert_eq!(cache.observe("m1"), SeenOutcome::Duplicate);
        assert_eq!(cache.duplicates_suppressed(), 2);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn distinct_ids_are_tracked_independently() {
        let mut cache = RelaySeenCache::new();
        assert_eq!(cache.observe("m1"), SeenOutcome::Fresh);
        assert_eq!(cache.observe("m2"), SeenOutcome::Fresh);
        assert!(cache.contains("m1"));
        assert!(cache.contains("m2"));
    }

    #[test]
    fn capacity_evicts_oldest_first_and_is_counted() {
        let mut cache = RelaySeenCache::with_config(RelaySeenConfig {
            capacity: 3,
            retention: Duration::from_secs(600),
        });

        cache.observe("m1");
        cache.observe("m2");
        cache.observe("m3");
        assert_eq!(cache.capacity_evictions(), 0);

        // Fourth id pushes out the first.
        cache.observe("m4");
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.capacity_evictions(), 1);
        assert!(!cache.contains("m1"));
        assert!(cache.contains("m2"));
        assert!(cache.contains("m4"));

        // And the evicted id is treated as new again — the condition the
        // capacity is sized to keep out of reach in production.
        assert_eq!(cache.observe("m1"), SeenOutcome::Fresh);
    }

    #[test]
    fn entries_expire_by_age() {
        let mut cache = RelaySeenCache::with_config(RelaySeenConfig {
            capacity: 128,
            retention: Duration::from_millis(0),
        });

        cache.observe("m1");
        // A zero retention means the entry is already expired when the next
        // observation sweeps.
        assert_eq!(cache.observe("m2"), SeenOutcome::Fresh);
        assert!(!cache.contains("m1"));
        assert_eq!(cache.capacity_evictions(), 0);
    }

    #[test]
    fn expiry_never_leaves_orphaned_order_entries() {
        let mut cache = RelaySeenCache::with_config(RelaySeenConfig {
            capacity: 4,
            retention: Duration::from_millis(0),
        });

        for i in 0..16 {
            cache.observe(&format!("m{i}"));
        }

        // The insertion-order queue must stay in lockstep with the map, or the
        // cache leaks memory that no expiry pass can reach.
        assert_eq!(cache.order.len(), cache.entries.len());
        assert!(cache.len() <= 4);
    }

    #[test]
    fn a_forgotten_id_is_new_again_and_leaves_no_orphan() {
        let mut cache = RelaySeenCache::new();
        cache.observe("m1");
        cache.observe("m2");

        cache.forget("m1");

        assert!(
            !cache.contains("m1"),
            "a forgotten id must not be suppressed"
        );
        assert!(cache.contains("m2"), "and its neighbors are untouched");
        assert_eq!(cache.observe("m1"), SeenOutcome::Fresh);
        // The order queue must not keep the forgotten id, or `observe` would
        // count popping it as a capacity eviction.
        assert_eq!(cache.order.len(), cache.entries.len());
        assert_eq!(cache.capacity_evictions(), 0);
    }

    #[test]
    fn forgetting_an_unknown_id_is_a_no_op() {
        let mut cache = RelaySeenCache::new();
        cache.observe("m1");

        cache.forget("never-seen");

        assert!(cache.contains("m1"));
        assert_eq!(cache.order.len(), 1);
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn clear_drops_everything() {
        let mut cache = RelaySeenCache::new();
        cache.observe("m1");
        cache.observe("m2");
        cache.clear();
        assert!(cache.is_empty());
        assert!(!cache.contains("m1"));
    }
}
