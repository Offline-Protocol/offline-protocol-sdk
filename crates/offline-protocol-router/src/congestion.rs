//! Congestion control for mesh network scalability.
//!
//! This module implements congestion control mechanisms to prevent network collapse:
//! - Send rate limiting based on delivery success
//! - Queue depth backpressure
//! - Fair queuing across senders

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Configuration for congestion control.
#[derive(Debug, Clone)]
pub struct CongestionConfig {
    /// Enable congestion control.
    pub enabled: bool,
    /// Maximum messages per second (global send rate limit).
    pub max_messages_per_second: f32,
    /// Minimum messages per second (floor for rate limiting).
    pub min_messages_per_second: f32,
    /// Maximum queue depth before rejecting new messages.
    pub max_queue_depth: usize,
    /// Queue depth at which to start applying backpressure.
    pub backpressure_threshold: usize,
    /// Window size for tracking delivery success (seconds).
    pub success_window_secs: u64,
    /// Minimum delivery success ratio to maintain current rate.
    pub min_success_ratio: f32,
    /// Rate increase factor when delivery is successful.
    pub rate_increase_factor: f32,
    /// Rate decrease factor when delivery fails.
    pub rate_decrease_factor: f32,
    /// Enable fair queuing across senders.
    pub fair_queuing_enabled: bool,
    /// Maximum messages per sender in fair queue.
    pub max_per_sender: usize,
}

impl Default for CongestionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_messages_per_second: 50.0,
            min_messages_per_second: 1.0,
            max_queue_depth: 100,
            backpressure_threshold: 50,
            success_window_secs: 30,
            min_success_ratio: 0.7,
            rate_increase_factor: 1.1,
            rate_decrease_factor: 0.5,
            fair_queuing_enabled: true,
            max_per_sender: 10,
        }
    }
}

/// Result of a send attempt decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendDecision {
    /// Message can be sent immediately.
    Allow,
    /// Message should be delayed (returns delay duration in ms).
    Delay(u64),
    /// Message should be rejected (queue full or rate exceeded).
    Reject,
    /// Backpressure active - message accepted but sender should slow down.
    Backpressure,
}

/// Delivery outcome for tracking success rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// Message was successfully delivered (ACK received).
    Success,
    /// Message delivery failed (timeout or explicit failure).
    Failure,
    /// Message was dropped due to congestion.
    Dropped,
}

/// Tracked delivery event for success rate calculation.
#[derive(Debug, Clone)]
struct DeliveryEvent {
    outcome: DeliveryOutcome,
    timestamp: Instant,
}

/// Per-sender tracking for fair queuing.
#[derive(Debug, Default)]
struct SenderState {
    /// Number of messages currently queued from this sender.
    queued_count: usize,
    /// Last send timestamp for rate limiting.
    last_send: Option<Instant>,
}

/// Congestion controller implementing AIMD-like rate control.
pub struct CongestionController {
    config: CongestionConfig,
    /// Current send rate limit (messages per second).
    current_rate: f32,
    /// Current queue depth.
    queue_depth: usize,
    /// Last time a message was sent.
    last_send_time: Option<Instant>,
    /// Tokens available for sending (token bucket).
    tokens: f32,
    /// Last token update time.
    last_token_update: Instant,
    /// Recent delivery events for success rate tracking.
    delivery_events: Vec<DeliveryEvent>,
    /// Per-sender state for fair queuing.
    sender_states: HashMap<String, SenderState>,
}

impl CongestionController {
    /// Creates a new congestion controller with default configuration.
    pub fn new() -> Self {
        Self::with_config(CongestionConfig::default())
    }

    /// Creates a new congestion controller with custom configuration.
    pub fn with_config(config: CongestionConfig) -> Self {
        let current_rate = config.max_messages_per_second;
        Self {
            config,
            current_rate,
            queue_depth: 0,
            last_send_time: None,
            tokens: 1.0, // Start with one token
            last_token_update: Instant::now(),
            delivery_events: Vec::new(),
            sender_states: HashMap::new(),
        }
    }

    /// Checks if a message can be sent and returns the decision.
    ///
    /// # Arguments
    ///
    /// * `sender_id` - ID of the message sender (for fair queuing)
    ///
    /// # Returns
    ///
    /// A `SendDecision` indicating whether to allow, delay, or reject the send.
    pub fn can_send(&mut self, sender_id: Option<&str>) -> SendDecision {
        if !self.config.enabled {
            return SendDecision::Allow;
        }

        // Update token bucket
        self.update_tokens();

        // Check queue depth
        if self.queue_depth >= self.config.max_queue_depth {
            return SendDecision::Reject;
        }

        // Check fair queuing limits
        if self.config.fair_queuing_enabled {
            if let Some(sender) = sender_id {
                let sender_state = self.sender_states.entry(sender.to_string()).or_default();
                if sender_state.queued_count >= self.config.max_per_sender {
                    return SendDecision::Reject;
                }
            }
        }

        // Check token bucket for rate limiting
        if self.tokens < 1.0 {
            let wait_time = ((1.0 - self.tokens) / self.current_rate * 1000.0) as u64;
            return SendDecision::Delay(wait_time.max(1));
        }

        // Check for backpressure condition
        if self.queue_depth >= self.config.backpressure_threshold {
            return SendDecision::Backpressure;
        }

        SendDecision::Allow
    }

    /// Records that a message was sent (consumes a token).
    pub fn record_send(&mut self, sender_id: Option<&str>) {
        self.tokens -= 1.0;
        self.last_send_time = Some(Instant::now());
        self.queue_depth = self.queue_depth.saturating_add(1);

        if self.config.fair_queuing_enabled {
            if let Some(sender) = sender_id {
                let sender_state = self.sender_states.entry(sender.to_string()).or_default();
                sender_state.queued_count = sender_state.queued_count.saturating_add(1);
                sender_state.last_send = Some(Instant::now());
            }
        }
    }

    /// Records the outcome of a delivery attempt.
    pub fn record_delivery(&mut self, sender_id: Option<&str>, outcome: DeliveryOutcome) {
        self.queue_depth = self.queue_depth.saturating_sub(1);

        // Update fair queue state
        if self.config.fair_queuing_enabled {
            if let Some(sender) = sender_id {
                if let Some(state) = self.sender_states.get_mut(sender) {
                    state.queued_count = state.queued_count.saturating_sub(1);
                }
            }
        }

        // Record delivery event
        self.delivery_events.push(DeliveryEvent {
            outcome,
            timestamp: Instant::now(),
        });

        // Prune old events
        self.prune_old_events();

        // Adjust rate based on success ratio
        self.adjust_rate();
    }

    /// Updates the token bucket based on elapsed time.
    fn update_tokens(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_token_update).as_secs_f32();
        self.last_token_update = now;

        // Add tokens based on elapsed time and current rate
        self.tokens += elapsed * self.current_rate;

        // Cap tokens at a small burst allowance (e.g., 3 messages)
        self.tokens = self.tokens.min(3.0);
    }

    /// Removes delivery events older than the success window.
    fn prune_old_events(&mut self) {
        let cutoff = Instant::now() - Duration::from_secs(self.config.success_window_secs);
        self.delivery_events.retain(|e| e.timestamp >= cutoff);
    }

    /// Adjusts send rate based on delivery success ratio (AIMD-like).
    fn adjust_rate(&mut self) {
        if self.delivery_events.is_empty() {
            return;
        }

        // Calculate success ratio
        let total = self.delivery_events.len() as f32;
        let successes = self.delivery_events
            .iter()
            .filter(|e| e.outcome == DeliveryOutcome::Success)
            .count() as f32;
        let success_ratio = successes / total;

        // AIMD: Additive Increase, Multiplicative Decrease
        if success_ratio >= self.config.min_success_ratio {
            // Increase rate on success
            self.current_rate = (self.current_rate * self.config.rate_increase_factor)
                .min(self.config.max_messages_per_second);
        } else {
            // Decrease rate on failure
            self.current_rate = (self.current_rate * self.config.rate_decrease_factor)
                .max(self.config.min_messages_per_second);
        }
    }

    /// Gets the current send rate limit.
    pub fn current_rate(&self) -> f32 {
        self.current_rate
    }

    /// Gets the current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.queue_depth
    }

    /// Gets the current congestion level (0.0-1.0).
    pub fn congestion_level(&self) -> f32 {
        if self.config.max_queue_depth == 0 {
            return 0.0;
        }
        (self.queue_depth as f32 / self.config.max_queue_depth as f32).min(1.0)
    }

    /// Gets the current delivery success ratio.
    pub fn success_ratio(&self) -> f32 {
        if self.delivery_events.is_empty() {
            return 1.0;
        }

        let total = self.delivery_events.len() as f32;
        let successes = self.delivery_events
            .iter()
            .filter(|e| e.outcome == DeliveryOutcome::Success)
            .count() as f32;
        successes / total
    }

    /// Gets the number of pending messages for a sender.
    pub fn sender_queue_depth(&self, sender_id: &str) -> usize {
        self.sender_states
            .get(sender_id)
            .map(|s| s.queued_count)
            .unwrap_or(0)
    }

    /// Checks if the network is considered congested.
    pub fn is_congested(&self) -> bool {
        self.congestion_level() > 0.5 || self.success_ratio() < self.config.min_success_ratio
    }

    /// Manually sets the queue depth (for synchronization with actual queue).
    pub fn set_queue_depth(&mut self, depth: usize) {
        self.queue_depth = depth;
    }

    /// Clears all state.
    pub fn reset(&mut self) {
        self.current_rate = self.config.max_messages_per_second;
        self.queue_depth = 0;
        self.tokens = 1.0;
        self.last_send_time = None;
        self.last_token_update = Instant::now();
        self.delivery_events.clear();
        self.sender_states.clear();
    }

    /// Gets the configuration.
    pub fn config(&self) -> &CongestionConfig {
        &self.config
    }
}

impl Default for CongestionController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_allow_send_when_not_congested() {
        let mut controller = CongestionController::new();
        let decision = controller.can_send(None);
        assert_eq!(decision, SendDecision::Allow);
    }

    #[test]
    fn test_reject_when_queue_full() {
        let mut config = CongestionConfig::default();
        config.max_queue_depth = 5;
        let mut controller = CongestionController::with_config(config);

        // Fill the queue
        for _ in 0..5 {
            controller.record_send(None);
        }

        let decision = controller.can_send(None);
        assert_eq!(decision, SendDecision::Reject);
    }

    #[test]
    fn test_backpressure_at_threshold() {
        let mut config = CongestionConfig::default();
        config.backpressure_threshold = 3;
        config.max_queue_depth = 10;
        config.max_messages_per_second = 1000.0; // High rate to avoid token depletion
        let mut controller = CongestionController::with_config(config);

        // Ensure we have tokens
        controller.tokens = 10.0;

        // Fill to threshold
        for _ in 0..3 {
            controller.queue_depth += 1;
        }

        let decision = controller.can_send(None);
        assert_eq!(decision, SendDecision::Backpressure);
    }

    #[test]
    fn test_rate_limiting_delay() {
        let mut config = CongestionConfig::default();
        config.max_messages_per_second = 2.0;
        let mut controller = CongestionController::with_config(config);

        // Consume tokens quickly
        controller.record_send(None);
        controller.record_send(None);
        controller.record_send(None);

        let decision = controller.can_send(None);
        match decision {
            SendDecision::Delay(ms) => assert!(ms > 0),
            _ => panic!("Expected delay decision"),
        }
    }

    #[test]
    fn test_fair_queuing_per_sender() {
        let mut config = CongestionConfig::default();
        config.max_per_sender = 2;
        config.max_queue_depth = 100;
        config.max_messages_per_second = 1000.0; // High rate to avoid token depletion
        let mut controller = CongestionController::with_config(config);

        // Ensure we have tokens
        controller.tokens = 10.0;

        // Fill sender's quota (use queue_depth directly to avoid token consumption)
        let alice_state = controller.sender_states.entry("alice".to_string()).or_default();
        alice_state.queued_count = 2;
        controller.queue_depth = 2;

        // Alice should be rejected
        let decision = controller.can_send(Some("alice"));
        assert_eq!(decision, SendDecision::Reject);

        // Bob should still be allowed
        let decision = controller.can_send(Some("bob"));
        assert_eq!(decision, SendDecision::Allow);
    }

    #[test]
    fn test_rate_decrease_on_failure() {
        let mut controller = CongestionController::new();
        let initial_rate = controller.current_rate();

        // Record failures
        for _ in 0..10 {
            controller.record_send(None);
            controller.record_delivery(None, DeliveryOutcome::Failure);
        }

        assert!(controller.current_rate() < initial_rate);
    }

    #[test]
    fn test_rate_increase_on_success() {
        let mut config = CongestionConfig::default();
        config.max_messages_per_second = 100.0;
        let mut controller = CongestionController::with_config(config);

        // Start at lower rate
        controller.current_rate = 10.0;

        // Record successes
        for _ in 0..10 {
            controller.record_send(None);
            controller.record_delivery(None, DeliveryOutcome::Success);
        }

        assert!(controller.current_rate() > 10.0);
    }

    #[test]
    fn test_congestion_level() {
        let mut config = CongestionConfig::default();
        config.max_queue_depth = 100;
        let mut controller = CongestionController::with_config(config);

        assert_eq!(controller.congestion_level(), 0.0);

        for _ in 0..50 {
            controller.record_send(None);
        }

        assert!((controller.congestion_level() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_disabled_congestion_control() {
        let mut config = CongestionConfig::default();
        config.enabled = false;
        config.max_queue_depth = 1;
        let mut controller = CongestionController::with_config(config);

        // Fill the queue beyond limit
        for _ in 0..10 {
            controller.record_send(None);
        }

        // Should still allow when disabled
        let decision = controller.can_send(None);
        assert_eq!(decision, SendDecision::Allow);
    }

    #[test]
    fn test_token_bucket_refill() {
        let mut config = CongestionConfig::default();
        config.max_messages_per_second = 10.0;
        let mut controller = CongestionController::with_config(config);

        // Consume tokens
        controller.record_send(None);
        controller.record_send(None);
        controller.record_send(None);

        // Wait for refill
        thread::sleep(Duration::from_millis(500));

        // Should have tokens now
        let decision = controller.can_send(None);
        assert_eq!(decision, SendDecision::Allow);
    }
}

