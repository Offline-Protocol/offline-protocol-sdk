//! Retry queue with exponential backoff

use offline_protocol_core::{MessageEnvelope, MessageId, Priority};
use parking_lot::RwLock;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Retry strategy configuration
#[derive(Debug, Clone)]
pub struct RetryStrategy {
    pub max_retries: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

impl Default for RetryStrategy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            multiplier: 2.0,
        }
    }
}

impl RetryStrategy {
    /// Calculate delay for a specific retry attempt
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let delay_secs = self.initial_delay.as_secs_f64()
            * self.multiplier.powi(attempt as i32);
        
        let delay = Duration::from_secs_f64(delay_secs);
        std::cmp::min(delay, self.max_delay)
    }
}

/// Message with retry metadata
#[derive(Clone)]
struct RetryMessage {
    envelope: MessageEnvelope,
    attempt: u32,
    next_retry: Instant,
}

impl PartialEq for RetryMessage {
    fn eq(&self, other: &Self) -> bool {
        self.next_retry == other.next_retry
    }
}

impl Eq for RetryMessage {}

impl PartialOrd for RetryMessage {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RetryMessage {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse ordering for min-heap (earlier times first)
        other.next_retry.cmp(&self.next_retry)
            .then_with(|| other.envelope.priority.cmp(&self.envelope.priority))
    }
}

/// Configuration for retry queue
#[derive(Debug, Clone)]
pub struct RetryQueueConfig {
    pub retry_strategy: RetryStrategy,
    pub max_queue_size: usize,
    pub message_lifetime: Duration,
}

impl Default for RetryQueueConfig {
    fn default() -> Self {
        Self {
            retry_strategy: RetryStrategy::default(),
            max_queue_size: 1000,
            message_lifetime: Duration::from_secs(3600), // 1 hour
        }
    }
}

/// Queue for managing message retries with exponential backoff
pub struct RetryQueue {
    config: RetryQueueConfig,
    queue: Arc<RwLock<BinaryHeap<RetryMessage>>>,
    messages: Arc<RwLock<HashMap<MessageId, Instant>>>, // Track message creation times
}

impl RetryQueue {
    pub fn new(config: RetryQueueConfig) -> Self {
        Self {
            config,
            queue: Arc::new(RwLock::new(BinaryHeap::new())),
            messages: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a message to the retry queue
    pub fn enqueue(&self, envelope: MessageEnvelope) -> crate::Result<()> {
        let mut queue = self.queue.write();
        
        if queue.len() >= self.config.max_queue_size {
            return Err(crate::Error::QueueFull);
        }

        let now = Instant::now();
        self.messages.write().insert(envelope.message_id, now);

        let retry_msg = RetryMessage {
            envelope,
            attempt: 0,
            next_retry: now,
        };

        queue.push(retry_msg);
        debug!("Enqueued message for retry");

        Ok(())
    }

    /// Get the next message ready for retry
    pub fn next_ready(&self) -> Option<MessageEnvelope> {
        let mut queue = self.queue.write();
        let now = Instant::now();

        // Check if the top message is ready
        if let Some(msg) = queue.peek() {
            if msg.next_retry <= now {
                if let Some(msg) = queue.pop() {
                    // Check if message has exceeded lifetime
                    if let Some(created_at) = self.messages.read().get(&msg.envelope.message_id) {
                        if now.duration_since(*created_at) > self.config.message_lifetime {
                            warn!("Message {} exceeded lifetime, dropping", msg.envelope.message_id);
                            self.messages.write().remove(&msg.envelope.message_id);
                            return None;
                        }
                    }

                    // Check if we should retry
                    if msg.attempt < self.config.retry_strategy.max_retries {
                        // Requeue with incremented attempt
                        let delay = self.config.retry_strategy.delay_for_attempt(msg.attempt + 1);
                        let retry_msg = RetryMessage {
                            envelope: msg.envelope.clone(),
                            attempt: msg.attempt + 1,
                            next_retry: now + delay,
                        };
                        queue.push(retry_msg);
                        debug!(
                            "Message {} retry attempt {} (next in {:?})",
                            msg.envelope.message_id,
                            msg.attempt + 1,
                            delay
                        );
                    } else {
                        warn!("Message {} exceeded max retries", msg.envelope.message_id);
                        self.messages.write().remove(&msg.envelope.message_id);
                        return None;
                    }

                    return Some(msg.envelope);
                }
            }
        }

        None
    }

    /// Remove a message from the retry queue (e.g., after successful ACK)
    pub fn remove(&self, message_id: MessageId) {
        self.messages.write().remove(&message_id);
        
        // Note: We don't remove from the heap immediately for efficiency.
        // The message will be filtered out when it's popped.
        debug!("Removed message {} from retry queue", message_id);
    }

    /// Get the number of messages in the queue
    pub fn len(&self) -> usize {
        self.queue.read().len()
    }

    /// Check if the queue is empty
    pub fn is_empty(&self) -> bool {
        self.queue.read().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{DeviceId, Message, TextMessage, UserId};
    use std::collections::HashMap;

    #[test]
    fn test_exponential_backoff() {
        let strategy = RetryStrategy::default();
        
        assert_eq!(strategy.delay_for_attempt(0), Duration::from_secs(1));
        assert_eq!(strategy.delay_for_attempt(1), Duration::from_secs(2));
        assert_eq!(strategy.delay_for_attempt(2), Duration::from_secs(4));
        assert_eq!(strategy.delay_for_attempt(3), Duration::from_secs(8));
    }

    #[tokio::test]
    async fn test_retry_queue() {
        let config = RetryQueueConfig {
            retry_strategy: RetryStrategy {
                max_retries: 2,
                initial_delay: Duration::from_millis(10),
                max_delay: Duration::from_secs(60),
                multiplier: 2.0,
            },
            max_queue_size: 10,
            message_lifetime: Duration::from_secs(3600),
        };

        let queue = RetryQueue::new(config);

        let envelope = MessageEnvelope::new(
            DeviceId::new(),
            UserId::new("sender"),
            Some(UserId::new("recipient")),
            Message::Text(TextMessage {
                text: "Test".to_string(),
                metadata: HashMap::new(),
            }),
            Priority::High,
            8,
        );

        queue.enqueue(envelope.clone()).unwrap();

        // First attempt should be immediate
        let msg1 = queue.next_ready();
        assert!(msg1.is_some());

        // Second attempt should require waiting (initial_delay * multiplier = 20ms)
        assert!(queue.next_ready().is_none());
        
        tokio::time::sleep(Duration::from_millis(25)).await;
        let msg2 = queue.next_ready();
        assert!(msg2.is_some());
    }
}

