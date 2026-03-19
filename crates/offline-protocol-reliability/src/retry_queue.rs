//! Retry queue with exponential backoff for message retries.

use crate::constants::{
    DEFAULT_BACKOFF_MULTIPLIER, DEFAULT_INITIAL_DELAY_MS, DEFAULT_MAX_DELAY_MS,
    DEFAULT_MAX_RETRIES, DEFAULT_OUTBOX_LIFETIME_MS,
};
use chrono::{DateTime, Utc};
use offline_protocol_core::{Message, MessagePriority};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries per message.
    pub max_retries: u32,

    /// Initial retry delay in milliseconds.
    pub initial_delay_ms: u64,

    /// Maximum retry delay in milliseconds.
    pub max_delay_ms: u64,

    /// Backoff multiplier for exponential backoff.
    pub backoff_multiplier: f32,

    /// Maximum lifetime for messages in the outbox (milliseconds).
    pub outbox_max_lifetime_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            initial_delay_ms: DEFAULT_INITIAL_DELAY_MS,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
            backoff_multiplier: DEFAULT_BACKOFF_MULTIPLIER,
            outbox_max_lifetime_ms: DEFAULT_OUTBOX_LIFETIME_MS,
        }
    }
}

/// A message scheduled for retry.
#[derive(Debug, Clone)]
pub struct RetryEntry {
    /// The message to retry.
    pub message: Message,

    /// Number of times this message has been retried.
    pub retry_count: u32,

    /// When this message was first added to the queue.
    pub added_at: DateTime<Utc>,

    /// When this message should be retried next.
    pub retry_at: DateTime<Utc>,

    /// Current backoff delay in milliseconds.
    pub current_delay_ms: u64,
}

impl RetryEntry {
    /// Checks if this entry is ready for retry.
    pub fn is_ready(&self) -> bool {
        Utc::now() >= self.retry_at
    }

    /// Checks if this entry has expired (exceeded max lifetime).
    pub fn is_expired(&self, max_lifetime_ms: u64) -> bool {
        let elapsed = Utc::now().signed_duration_since(self.added_at);
        elapsed.num_milliseconds() >= max_lifetime_ms as i64
    }

    /// Calculates the next retry time with exponential backoff.
    pub fn calculate_next_retry_time(
        current_delay_ms: u64,
        backoff_multiplier: f32,
        max_delay_ms: u64,
    ) -> (DateTime<Utc>, u64) {
        let next_delay = ((current_delay_ms as f32 * backoff_multiplier) as u64).min(max_delay_ms);

        let next_retry = Utc::now() + chrono::Duration::milliseconds(next_delay as i64);
        (next_retry, next_delay)
    }
}

// Implement Ord for priority queue (earlier retry_at = higher priority)
impl Ord for RetryEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse comparison for min-heap (earliest retry_at first)
        other.retry_at.cmp(&self.retry_at).then_with(|| {
            // Tie-breaker: higher priority messages first (non-reversed,
            // so High > Low makes High pop first from the max-heap)
            self.message.priority.cmp(&other.message.priority)
        })
    }
}

impl PartialOrd for RetryEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for RetryEntry {}

impl PartialEq for RetryEntry {
    fn eq(&self, other: &Self) -> bool {
        self.retry_at == other.retry_at && self.message.priority == other.message.priority
    }
}

/// Retry queue for managing message retries with exponential backoff.
pub struct RetryQueue {
    config: RetryConfig,
    queue: BinaryHeap<RetryEntry>,
    index: HashMap<String, ()>, // For fast lookup
}

impl RetryQueue {
    /// Creates a new retry queue with default configuration.
    pub fn new() -> Self {
        Self::with_config(RetryConfig::default())
    }

    /// Creates a new retry queue with custom configuration.
    pub fn with_config(config: RetryConfig) -> Self {
        Self {
            config,
            queue: BinaryHeap::new(),
            index: HashMap::new(),
        }
    }

    /// Adds a message to the retry queue.
    ///
    /// The retry queue is a pure scheduling mechanism — it does not enforce a
    /// retry limit.  ACK-level retry limits are enforced by the ACK manager in
    /// the protocol layer.
    ///
    /// Duplicate messages (same ID already in queue) are silently ignored.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to queue for retry
    /// * `retry_count` - Current retry count (used for backoff calculation)
    pub fn enqueue(&mut self, message: Message, retry_count: u32) {
        // Prevent duplicate entries for the same message
        if self.index.contains_key(&message.id.as_str()) {
            return;
        }

        // Calculate retry delay with exponential backoff
        // Cap exponent at 20 to prevent overflow with large retry counts
        let delay_ms = if retry_count == 0 {
            self.config.initial_delay_ms
        } else {
            let base_delay = self.config.initial_delay_ms
                * (self
                    .config
                    .backoff_multiplier
                    .powi(retry_count.min(20) as i32) as u64);
            base_delay.min(self.config.max_delay_ms)
        };

        let retry_at = Utc::now() + chrono::Duration::milliseconds(delay_ms as i64);

        let entry = RetryEntry {
            message: message.clone(),
            retry_count,
            added_at: Utc::now(),
            retry_at,
            current_delay_ms: delay_ms,
        };

        self.index.insert(message.id.as_str(), ());
        self.queue.push(entry);
    }

    /// Dequeues the next message that is ready for retry.
    ///
    /// # Returns
    ///
    /// Returns `Some(RetryEntry)` if a message is ready, `None` otherwise.
    pub fn dequeue_ready(&mut self) -> Option<RetryEntry> {
        // Peek at the top entry
        let is_ready = self.queue.peek().is_some_and(|entry| entry.is_ready());
        if is_ready {
            if let Some(entry) = self.queue.pop() {
                self.index.remove(&entry.message.id.as_str());
                return Some(entry);
            }
        }
        None
    }

    /// Gets all messages ready for retry without removing them.
    pub fn peek_ready(&self) -> Vec<&RetryEntry> {
        self.queue.iter().filter(|entry| entry.is_ready()).collect()
    }

    /// Removes a message from the queue (e.g., after successful delivery).
    pub fn remove(&mut self, message_id: &str) -> bool {
        if self.index.remove(message_id).is_some() {
            // Remove from heap (expensive, but necessary)
            self.queue
                .retain(|entry| entry.message.id.as_str() != message_id);
            true
        } else {
            false
        }
    }

    /// Cleans up expired messages that exceeded their max lifetime.
    ///
    /// # Returns
    ///
    /// Returns the number of expired messages removed.
    pub fn cleanup_expired(&mut self) -> usize {
        let max_lifetime = self.config.outbox_max_lifetime_ms;

        let before_count = self.queue.len();

        // Filter out expired entries
        let new_queue: BinaryHeap<RetryEntry> = self
            .queue
            .iter()
            .filter(|entry| !entry.is_expired(max_lifetime))
            .cloned()
            .collect();

        // Update index
        self.index.clear();
        for entry in &new_queue {
            self.index.insert(entry.message.id.as_str(), ());
        }

        self.queue = new_queue;

        before_count - self.queue.len()
    }

    /// Gets the number of messages in the queue.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Checks if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Checks if a message is in the queue.
    pub fn contains(&self, message_id: &str) -> bool {
        self.index.contains_key(message_id)
    }

    /// Drains all entries from the queue, ignoring `retry_at` timing.
    ///
    /// Returns entries in priority order (earliest `retry_at` first, then
    /// highest message priority). Used for immediate flush when transport
    /// becomes available.
    pub fn drain_all(&mut self) -> Vec<RetryEntry> {
        self.index.clear();
        let mut entries = Vec::with_capacity(self.queue.len());
        while let Some(entry) = self.queue.pop() {
            entries.push(entry);
        }
        entries
    }

    /// Gets the time until the next retry is ready.
    ///
    /// # Returns
    ///
    /// Returns `Some(Duration)` if there are pending retries, `None` if empty.
    pub fn time_until_next_retry(&self) -> Option<chrono::Duration> {
        self.queue.peek().map(|entry| {
            let now = Utc::now();
            if entry.retry_at > now {
                entry.retry_at - now
            } else {
                chrono::Duration::zero()
            }
        })
    }

    /// Gets statistics about the retry queue.
    pub fn stats(&self) -> RetryQueueStats {
        let ready_count = self.peek_ready().len();
        let priority_counts = self.count_by_priority();

        RetryQueueStats {
            total_count: self.len(),
            ready_count,
            critical_priority_count: priority_counts[&MessagePriority::Critical],
            high_priority_count: priority_counts[&MessagePriority::High],
            medium_priority_count: priority_counts[&MessagePriority::Medium],
            low_priority_count: priority_counts[&MessagePriority::Low],
        }
    }

    fn count_by_priority(&self) -> HashMap<MessagePriority, usize> {
        let mut counts = HashMap::new();
        counts.insert(MessagePriority::Low, 0);
        counts.insert(MessagePriority::Medium, 0);
        counts.insert(MessagePriority::High, 0);
        counts.insert(MessagePriority::Critical, 0);

        for entry in &self.queue {
            *counts.entry(entry.message.priority).or_insert(0) += 1;
        }

        counts
    }
}

impl Default for RetryQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the retry queue.
#[derive(Debug, Clone)]
pub struct RetryQueueStats {
    /// Total number of messages in queue.
    pub total_count: usize,
    /// Number of messages ready for retry.
    pub ready_count: usize,
    /// Number of critical priority messages.
    pub critical_priority_count: usize,
    /// Number of high priority messages.
    pub high_priority_count: usize,
    /// Number of medium priority messages.
    pub medium_priority_count: usize,
    /// Number of low priority messages.
    pub low_priority_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, UserId};
    use std::thread;
    use std::time::Duration;

    fn create_test_message(priority: MessagePriority) -> Message {
        Message::builder(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
        )
        .content("Test message")
        .priority(priority)
        .build()
    }

    #[test]
    fn test_enqueue_and_dequeue() {
        let config = RetryConfig {
            initial_delay_ms: 50, // Fast for testing
            ..Default::default()
        };
        let mut queue = RetryQueue::with_config(config);

        let msg = create_test_message(MessagePriority::Medium);
        queue.enqueue(msg.clone(), 0);

        assert_eq!(queue.len(), 1);
        assert!(queue.contains(&msg.id.as_str()));

        // Not ready immediately
        assert!(queue.dequeue_ready().is_none());

        // Wait for retry time
        thread::sleep(Duration::from_millis(60));

        // Should be ready now
        let dequeued = queue.dequeue_ready().unwrap();
        assert_eq!(dequeued.message.id, msg.id);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_exponential_backoff() {
        let _config = RetryConfig {
            initial_delay_ms: 100,
            backoff_multiplier: 2.0,
            max_delay_ms: 1000,
            ..Default::default()
        };

        // First retry: 100ms
        let (_, delay1) = RetryEntry::calculate_next_retry_time(100, 2.0, 1000);
        assert_eq!(delay1, 200);

        // Second retry: 200ms * 2 = 400ms
        let (_, delay2) = RetryEntry::calculate_next_retry_time(200, 2.0, 1000);
        assert_eq!(delay2, 400);

        // Third retry: 400ms * 2 = 800ms
        let (_, delay3) = RetryEntry::calculate_next_retry_time(400, 2.0, 1000);
        assert_eq!(delay3, 800);

        // Fourth retry: 800ms * 2 = 1600ms, capped at 1000ms
        let (_, delay4) = RetryEntry::calculate_next_retry_time(800, 2.0, 1000);
        assert_eq!(delay4, 1000);
    }

    #[test]
    fn test_enqueue_accepts_high_retry_count() {
        let config = RetryConfig {
            max_retries: 2,
            ..Default::default()
        };
        let mut queue = RetryQueue::with_config(config);

        let msg = create_test_message(MessagePriority::Medium);

        // enqueue no longer rejects based on max_retries — it's a pure
        // scheduling mechanism. ACK timeouts govern permanent failure.
        queue.enqueue(msg.clone(), 0);
        queue.dequeue_ready(); // Clear queue

        queue.enqueue(msg.clone(), 1);
        queue.dequeue_ready();

        // Previously this would fail; now it should succeed
        queue.enqueue(msg.clone(), 2);
        queue.dequeue_ready();

        queue.enqueue(msg.clone(), 100);
    }

    #[test]
    fn test_drain_all() {
        let config = RetryConfig {
            initial_delay_ms: 60_000, // Long delay so nothing is "ready"
            ..Default::default()
        };
        let mut queue = RetryQueue::with_config(config);

        let msg1 = create_test_message(MessagePriority::High);
        let msg2 = create_test_message(MessagePriority::Low);
        let msg3 = create_test_message(MessagePriority::Medium);

        queue.enqueue(msg1.clone(), 0);
        queue.enqueue(msg2.clone(), 0);
        queue.enqueue(msg3.clone(), 0);

        // Nothing should be ready (long delay)
        assert!(queue.dequeue_ready().is_none());
        assert_eq!(queue.len(), 3);

        // drain_all returns everything regardless of timing, in heap order
        // (earliest retry_at first, priority as tiebreaker)
        let entries = queue.drain_all();
        assert_eq!(entries.len(), 3);
        assert!(queue.is_empty());
        assert!(!queue.contains(&msg1.id.as_str()));
    }

    #[test]
    fn test_priority_ordering() {
        let config = RetryConfig {
            initial_delay_ms: 50,
            ..Default::default()
        };
        let mut queue = RetryQueue::with_config(config);

        // Add messages with different priorities (same retry time)
        let low = create_test_message(MessagePriority::Low);
        let high = create_test_message(MessagePriority::High);
        let medium = create_test_message(MessagePriority::Medium);

        queue.enqueue(low.clone(), 0);
        queue.enqueue(high.clone(), 0);
        queue.enqueue(medium.clone(), 0);

        thread::sleep(Duration::from_millis(60));

        // All should be ready
        let ready = queue.peek_ready();
        assert_eq!(ready.len(), 3);

        // Check that high priority is among the ready messages
        let has_high = ready
            .iter()
            .any(|e| e.message.priority == MessagePriority::High);
        assert!(has_high);
    }

    #[test]
    fn test_priority_tiebreaker_ordering() {
        // Verify that when retry_at is identical, higher priority pops first.
        // We construct entries directly to ensure identical timestamps.
        let now = Utc::now();
        let make_entry = |priority: MessagePriority| {
            let mut msg = create_test_message(priority);
            // Force same priority into message
            msg.priority = priority;
            RetryEntry {
                message: msg,
                retry_count: 0,
                added_at: now,
                retry_at: now,
                current_delay_ms: 1000,
            }
        };

        let mut heap = BinaryHeap::new();
        heap.push(make_entry(MessagePriority::Low));
        heap.push(make_entry(MessagePriority::High));
        heap.push(make_entry(MessagePriority::Medium));

        assert_eq!(heap.pop().unwrap().message.priority, MessagePriority::High);
        assert_eq!(
            heap.pop().unwrap().message.priority,
            MessagePriority::Medium
        );
        assert_eq!(heap.pop().unwrap().message.priority, MessagePriority::Low);
    }

    #[test]
    fn test_remove_message() {
        let mut queue = RetryQueue::new();
        let msg = create_test_message(MessagePriority::Medium);

        queue.enqueue(msg.clone(), 0);
        assert!(queue.contains(&msg.id.as_str()));

        assert!(queue.remove(&msg.id.as_str()));
        assert!(!queue.contains(&msg.id.as_str()));
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_cleanup_expired() {
        let config = RetryConfig {
            outbox_max_lifetime_ms: 100, // 100ms for testing
            ..Default::default()
        };
        let mut queue = RetryQueue::with_config(config);

        let msg = create_test_message(MessagePriority::Medium);
        queue.enqueue(msg, 0);

        // Wait for expiration
        thread::sleep(Duration::from_millis(150));

        let removed = queue.cleanup_expired();
        assert_eq!(removed, 1);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_queue_stats() {
        let config = RetryConfig {
            initial_delay_ms: 50,
            ..Default::default()
        };
        let mut queue = RetryQueue::with_config(config);

        queue.enqueue(create_test_message(MessagePriority::High), 0);
        queue.enqueue(create_test_message(MessagePriority::High), 0);
        queue.enqueue(create_test_message(MessagePriority::Medium), 0);
        queue.enqueue(create_test_message(MessagePriority::Low), 0);

        let stats = queue.stats();
        assert_eq!(stats.total_count, 4);
        assert_eq!(stats.high_priority_count, 2);
        assert_eq!(stats.medium_priority_count, 1);
        assert_eq!(stats.low_priority_count, 1);
    }
}
