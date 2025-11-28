//! ACK optimization to prevent feedback loop amplification in large networks.
//!
//! This module implements several techniques to reduce ACK-related network load:
//! - Probabilistic ACK relaying: not all nodes relay every ACK
//! - ACK aggregation: batch multiple ACKs into single messages
//! - Piggyback ACKs: attach ACKs to data messages going the same direction

use chrono::{DateTime, Utc};
use offline_protocol_core::MessageId;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Configuration for ACK optimization.
#[derive(Debug, Clone)]
pub struct AckOptimizationConfig {
    /// Enable probabilistic ACK relaying.
    pub probabilistic_relay_enabled: bool,
    /// Base probability for relaying an ACK (0.0-1.0).
    pub base_relay_probability: f32,
    /// Minimum relay probability (ensures some ACKs always propagate).
    pub min_relay_probability: f32,
    /// Enable ACK aggregation (batching).
    pub aggregation_enabled: bool,
    /// Maximum number of ACKs to aggregate before flushing.
    pub max_aggregation_batch: usize,
    /// Maximum time to wait for more ACKs before flushing (milliseconds).
    pub aggregation_timeout_ms: u64,
    /// Enable piggybacking ACKs on data messages.
    pub piggyback_enabled: bool,
    /// Maximum ACK data size for piggybacking (bytes).
    pub max_piggyback_size: usize,
}

impl Default for AckOptimizationConfig {
    fn default() -> Self {
        Self {
            probabilistic_relay_enabled: true,
            base_relay_probability: 0.5,    // Only 50% of nodes relay each ACK
            min_relay_probability: 0.2,     // At least 20% always relay
            aggregation_enabled: true,
            max_aggregation_batch: 10,
            aggregation_timeout_ms: 500,
            piggyback_enabled: true,
            max_piggyback_size: 64,         // Keep piggyback data small
        }
    }
}

/// Aggregated ACK entry ready for transmission.
#[derive(Debug, Clone)]
pub struct AggregatedAck {
    /// Original message ID this ACK is for.
    pub original_message_id: MessageId,
    /// Hop count when ACK was generated.
    pub hop_count: u8,
    /// Timestamp when ACK was received.
    pub received_at: DateTime<Utc>,
}

/// Pending piggyback ACK data.
#[derive(Debug, Clone)]
pub struct PiggybackAckData {
    /// Destination (sender of original message).
    pub destination: String,
    /// ACKs to piggyback on messages to this destination.
    pub acks: Vec<AggregatedAck>,
    /// When the first ACK was added.
    pub first_added_at: DateTime<Utc>,
}

/// ACK optimizer for reducing network load from acknowledgments.
pub struct AckOptimizer {
    config: AckOptimizationConfig,
    /// Local device ID for deterministic randomness.
    local_device_id: String,
    /// Pending ACKs waiting for aggregation, keyed by destination.
    pending_acks: HashMap<String, Vec<AggregatedAck>>,
    /// Timestamp of first pending ACK per destination.
    pending_since: HashMap<String, DateTime<Utc>>,
}

impl AckOptimizer {
    /// Creates a new ACK optimizer with default configuration.
    pub fn new(local_device_id: impl Into<String>) -> Self {
        Self::with_config(local_device_id, AckOptimizationConfig::default())
    }

    /// Creates a new ACK optimizer with custom configuration.
    pub fn with_config(local_device_id: impl Into<String>, config: AckOptimizationConfig) -> Self {
        Self {
            config,
            local_device_id: local_device_id.into(),
            pending_acks: HashMap::new(),
            pending_since: HashMap::new(),
        }
    }

    /// Determines if this node should relay a received ACK.
    ///
    /// Uses deterministic pseudo-random selection based on ACK ID and local device ID
    /// to ensure consistent relay decisions across the network.
    ///
    /// # Arguments
    ///
    /// * `ack_message_id` - ID of the ACK message being relayed
    /// * `visible_peer_count` - Number of visible peers in the network
    ///
    /// # Returns
    ///
    /// `true` if this node should relay the ACK, `false` otherwise.
    pub fn should_relay_ack(&self, ack_message_id: &str, visible_peer_count: usize) -> bool {
        if !self.config.probabilistic_relay_enabled {
            return true; // Always relay if disabled
        }

        // Higher peer count = lower relay probability to prevent storms
        let probability = self.compute_relay_probability(visible_peer_count);

        // Deterministic selection based on ACK ID and device ID
        let mut hasher = DefaultHasher::new();
        ack_message_id.hash(&mut hasher);
        self.local_device_id.hash(&mut hasher);
        let hash = hasher.finish();

        let hash_probability = (hash as f64 / u64::MAX as f64) as f32;
        hash_probability < probability
    }

    /// Computes the relay probability based on network density.
    fn compute_relay_probability(&self, visible_peer_count: usize) -> f32 {
        if visible_peer_count <= 5 {
            return 1.0; // Small network: always relay
        }

        // As network grows, reduce probability
        // At 50 peers: ~25% probability
        // At 100 peers: ~15% probability
        let density_factor = 5.0 / (visible_peer_count as f32);
        let raw_probability = self.config.base_relay_probability * density_factor;

        raw_probability.max(self.config.min_relay_probability).min(1.0)
    }

    /// Adds an ACK to the pending aggregation buffer.
    ///
    /// # Arguments
    ///
    /// * `destination` - User ID of the original message sender (ACK destination)
    /// * `original_message_id` - ID of the message being acknowledged
    /// * `hop_count` - Number of hops the original message traveled
    ///
    /// # Returns
    ///
    /// `Some(Vec<AggregatedAck>)` if batch is ready for transmission, `None` otherwise.
    pub fn add_ack_for_aggregation(
        &mut self,
        destination: &str,
        original_message_id: MessageId,
        hop_count: u8,
    ) -> Option<Vec<AggregatedAck>> {
        if !self.config.aggregation_enabled {
            // No aggregation: return immediately
            return Some(vec![AggregatedAck {
                original_message_id,
                hop_count,
                received_at: Utc::now(),
            }]);
        }

        let now = Utc::now();
        let ack = AggregatedAck {
            original_message_id,
            hop_count,
            received_at: now,
        };

        let pending = self.pending_acks.entry(destination.to_string()).or_default();
        pending.push(ack);

        // Record when we started collecting for this destination
        self.pending_since
            .entry(destination.to_string())
            .or_insert(now);

        // Check if batch is full
        if pending.len() >= self.config.max_aggregation_batch {
            return self.flush_pending_acks(destination);
        }

        None
    }

    /// Flushes pending ACKs for a destination.
    pub fn flush_pending_acks(&mut self, destination: &str) -> Option<Vec<AggregatedAck>> {
        self.pending_since.remove(destination);
        self.pending_acks.remove(destination)
    }

    /// Gets destinations with ACKs that have timed out and should be flushed.
    ///
    /// # Returns
    ///
    /// A vector of destination user IDs whose ACK buffers should be flushed.
    pub fn get_timed_out_destinations(&self) -> Vec<String> {
        let now = Utc::now();
        let timeout = chrono::Duration::milliseconds(self.config.aggregation_timeout_ms as i64);

        self.pending_since
            .iter()
            .filter_map(|(dest, since)| {
                if now.signed_duration_since(*since) > timeout {
                    Some(dest.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Drains all timed-out ACK batches.
    ///
    /// # Returns
    ///
    /// A vector of (destination, acks) tuples for destinations that timed out.
    pub fn drain_timed_out(&mut self) -> Vec<(String, Vec<AggregatedAck>)> {
        let destinations = self.get_timed_out_destinations();
        destinations
            .into_iter()
            .filter_map(|dest| {
                let acks = self.flush_pending_acks(&dest)?;
                Some((dest, acks))
            })
            .collect()
    }

    /// Checks if there's a pending ACK to piggyback on a data message.
    ///
    /// # Arguments
    ///
    /// * `message_destination` - Destination of the outgoing data message
    ///
    /// # Returns
    ///
    /// `Some(PiggybackAckData)` if there are ACKs to piggyback, `None` otherwise.
    pub fn get_piggyback_acks(&mut self, message_destination: &str) -> Option<PiggybackAckData> {
        if !self.config.piggyback_enabled {
            return None;
        }

        // Get pending ACKs for this destination
        let acks = self.flush_pending_acks(message_destination)?;
        if acks.is_empty() {
            return None;
        }

        // Limit size for piggybacking
        let limited_acks: Vec<_> = acks
            .into_iter()
            .take(self.estimate_piggyback_limit())
            .collect();

        if limited_acks.is_empty() {
            return None;
        }

        Some(PiggybackAckData {
            destination: message_destination.to_string(),
            acks: limited_acks,
            first_added_at: Utc::now(),
        })
    }

    /// Estimates how many ACKs can be piggybacked within size limit.
    fn estimate_piggyback_limit(&self) -> usize {
        // Rough estimate: each ACK takes ~40 bytes (message ID + metadata)
        let ack_size = 40;
        (self.config.max_piggyback_size / ack_size).max(1)
    }

    /// Returns the number of pending ACK destinations.
    pub fn pending_destination_count(&self) -> usize {
        self.pending_acks.len()
    }

    /// Returns the total number of pending ACKs.
    pub fn pending_ack_count(&self) -> usize {
        self.pending_acks.values().map(|v| v.len()).sum()
    }

    /// Clears all pending ACKs.
    pub fn clear(&mut self) {
        self.pending_acks.clear();
        self.pending_since.clear();
    }

    /// Gets the configuration.
    pub fn config(&self) -> &AckOptimizationConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probabilistic_relay_small_network() {
        let optimizer = AckOptimizer::new("device1");

        // In small networks, should always relay
        let should_relay = optimizer.should_relay_ack("ack123", 3);
        assert!(should_relay);
    }

    #[test]
    fn test_probabilistic_relay_deterministic() {
        let optimizer = AckOptimizer::new("device1");

        // Same inputs should always produce same result
        let result1 = optimizer.should_relay_ack("ack123", 50);
        let result2 = optimizer.should_relay_ack("ack123", 50);
        assert_eq!(result1, result2);
    }

    #[test]
    fn test_probabilistic_relay_varies_by_device() {
        let optimizer1 = AckOptimizer::new("device1");
        let optimizer2 = AckOptimizer::new("device2");

        // Different devices may have different relay decisions
        let results: Vec<bool> = (0..10)
            .map(|i| optimizer1.should_relay_ack(&format!("ack{}", i), 100))
            .collect();
        let results2: Vec<bool> = (0..10)
            .map(|i| optimizer2.should_relay_ack(&format!("ack{}", i), 100))
            .collect();

        // Results should differ for at least some ACKs (probabilistically)
        // This test might rarely fail due to randomness, but it's a good sanity check
        assert_ne!(results, results2);
    }

    #[test]
    fn test_aggregation_single_ack() {
        let mut optimizer = AckOptimizer::new("device1");

        let result = optimizer.add_ack_for_aggregation("alice", MessageId::new(), 3);

        // Single ACK shouldn't flush (waiting for more)
        assert!(result.is_none());
        assert_eq!(optimizer.pending_ack_count(), 1);
    }

    #[test]
    fn test_aggregation_batch_full() {
        let mut config = AckOptimizationConfig::default();
        config.max_aggregation_batch = 3;
        let mut optimizer = AckOptimizer::with_config("device1", config);

        // Add ACKs until batch is full
        optimizer.add_ack_for_aggregation("alice", MessageId::new(), 1);
        optimizer.add_ack_for_aggregation("alice", MessageId::new(), 2);
        let result = optimizer.add_ack_for_aggregation("alice", MessageId::new(), 3);

        // Should flush when batch is full
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 3);
        assert_eq!(optimizer.pending_ack_count(), 0);
    }

    #[test]
    fn test_aggregation_disabled() {
        let mut config = AckOptimizationConfig::default();
        config.aggregation_enabled = false;
        let mut optimizer = AckOptimizer::with_config("device1", config);

        let result = optimizer.add_ack_for_aggregation("alice", MessageId::new(), 3);

        // Should return immediately when aggregation disabled
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 1);
    }

    #[test]
    fn test_piggyback_acks() {
        let mut optimizer = AckOptimizer::new("device1");

        // Add an ACK for alice
        optimizer.add_ack_for_aggregation("alice", MessageId::new(), 3);

        // Get piggybacked ACKs when sending to alice
        let piggyback = optimizer.get_piggyback_acks("alice");
        assert!(piggyback.is_some());
        assert_eq!(piggyback.unwrap().acks.len(), 1);

        // Should be cleared now
        assert_eq!(optimizer.pending_ack_count(), 0);
    }

    #[test]
    fn test_piggyback_no_acks() {
        let mut optimizer = AckOptimizer::new("device1");

        // No pending ACKs for bob
        let piggyback = optimizer.get_piggyback_acks("bob");
        assert!(piggyback.is_none());
    }

    #[test]
    fn test_drain_timed_out() {
        let mut config = AckOptimizationConfig::default();
        config.aggregation_timeout_ms = 0; // Immediate timeout for testing
        let mut optimizer = AckOptimizer::with_config("device1", config);

        optimizer.add_ack_for_aggregation("alice", MessageId::new(), 1);
        optimizer.add_ack_for_aggregation("bob", MessageId::new(), 2);

        // All should be timed out immediately
        let timed_out = optimizer.drain_timed_out();
        assert_eq!(timed_out.len(), 2);
    }
}

