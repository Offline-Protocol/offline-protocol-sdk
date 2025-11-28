//! Adaptive TTL management for large-scale mesh networks.
//!
//! This module implements adaptive TTL calculation that scales with network size
//! to ensure messages can reach distant nodes while preventing excessive flooding.

use crate::constants::*;

/// Configuration for adaptive TTL behavior.
#[derive(Debug, Clone)]
pub struct AdaptiveTtlConfig {
    /// Base TTL for small networks.
    pub base_ttl: u8,
    /// Additional TTL per 100 devices in the network.
    pub ttl_per_100_devices: u8,
    /// Maximum TTL to prevent infinite propagation.
    pub max_ttl: u8,
    /// Minimum TTL to ensure basic propagation.
    pub min_ttl: u8,
    /// TTL boost for congested/queued messages.
    pub congestion_boost: u8,
    /// Network size threshold below which base TTL is used.
    pub small_network_threshold: usize,
    /// Enable adaptive TTL (if false, always use base_ttl).
    pub enabled: bool,
}

impl Default for AdaptiveTtlConfig {
    fn default() -> Self {
        Self {
            base_ttl: ADAPTIVE_TTL_BASE,
            ttl_per_100_devices: ADAPTIVE_TTL_PER_100_DEVICES,
            max_ttl: ADAPTIVE_TTL_MAX,
            min_ttl: ADAPTIVE_TTL_MIN,
            congestion_boost: ADAPTIVE_TTL_CONGESTION_BOOST,
            small_network_threshold: ADAPTIVE_TTL_SMALL_NETWORK_THRESHOLD,
            enabled: true,
        }
    }
}

/// Network size estimate for TTL calculation.
#[derive(Debug, Clone, Default)]
pub struct NetworkSizeEstimate {
    /// Number of directly connected peers.
    pub connected_peers: usize,
    /// Number of peers seen in advertisements (1-hop neighborhood).
    pub visible_peers: usize,
    /// Estimated total network size (may be derived from gossip or heuristics).
    pub estimated_total: Option<usize>,
    /// Current congestion level (0.0-1.0).
    pub congestion_level: f32,
    /// Whether we've been experiencing delivery failures.
    pub has_delivery_failures: bool,
}

impl NetworkSizeEstimate {
    /// Creates a new network size estimate.
    pub fn new(connected_peers: usize, visible_peers: usize) -> Self {
        Self {
            connected_peers,
            visible_peers,
            estimated_total: None,
            congestion_level: 0.0,
            has_delivery_failures: false,
        }
    }

    /// Sets the estimated total network size.
    pub fn with_estimated_total(mut self, total: usize) -> Self {
        self.estimated_total = Some(total);
        self
    }

    /// Sets the congestion level.
    pub fn with_congestion(mut self, level: f32) -> Self {
        self.congestion_level = level.clamp(0.0, 1.0);
        self
    }

    /// Marks that delivery failures have occurred.
    pub fn with_delivery_failures(mut self) -> Self {
        self.has_delivery_failures = true;
        self
    }

    /// Estimates network size using heuristics if not explicitly set.
    ///
    /// Uses a simple model: if each peer connects to ~4 others on average,
    /// the total network size is approximately visible_peers * 4 / degree.
    /// This is a rough estimate and should be replaced with actual gossip-based
    /// network size estimation in production.
    fn estimated_size(&self) -> usize {
        if let Some(total) = self.estimated_total {
            return total;
        }

        // Heuristic: assume each node has ~4 connections on average
        // So visible peers represent roughly 1/4 of the network
        // But be conservative and assume we see at least 1/2
        let min_estimate = self.visible_peers.max(self.connected_peers);
        let max_estimate = self.visible_peers.saturating_mul(4);

        // Use geometric mean as a reasonable middle ground
        ((min_estimate as f64 * max_estimate as f64).sqrt() as usize).max(min_estimate)
    }
}

/// Adaptive TTL calculator.
pub struct AdaptiveTtlCalculator {
    config: AdaptiveTtlConfig,
}

impl AdaptiveTtlCalculator {
    /// Creates a new calculator with default configuration.
    pub fn new() -> Self {
        Self::with_config(AdaptiveTtlConfig::default())
    }

    /// Creates a new calculator with custom configuration.
    pub fn with_config(config: AdaptiveTtlConfig) -> Self {
        Self { config }
    }

    /// Computes the optimal TTL for a message based on network conditions.
    ///
    /// # Arguments
    ///
    /// * `network_estimate` - Current network size estimate
    /// * `is_priority_message` - Whether this is a high-priority message
    /// * `is_queued` - Whether this message has been queued (retry scenario)
    ///
    /// # Returns
    ///
    /// The recommended TTL value for the message.
    pub fn compute_ttl(
        &self,
        network_estimate: &NetworkSizeEstimate,
        is_priority_message: bool,
        is_queued: bool,
    ) -> u8 {
        if !self.config.enabled {
            return self.config.base_ttl;
        }

        let estimated_size = network_estimate.estimated_size();

        // Start with base TTL
        let mut ttl = self.config.base_ttl;

        // Add TTL based on network size
        if estimated_size > self.config.small_network_threshold {
            let extra_hundreds = (estimated_size - self.config.small_network_threshold) / 100;
            let additional_ttl =
                (extra_hundreds as u8).saturating_mul(self.config.ttl_per_100_devices);
            ttl = ttl.saturating_add(additional_ttl);
        }

        // Boost TTL for queued messages (they've already lost time)
        if is_queued {
            ttl = ttl.saturating_add(self.config.congestion_boost);
        }

        // Boost TTL if we're experiencing delivery failures
        if network_estimate.has_delivery_failures {
            ttl = ttl.saturating_add(1);
        }

        // Boost TTL for priority messages
        if is_priority_message {
            ttl = ttl.saturating_add(2);
        }

        // Reduce TTL slightly in high congestion to prevent amplifying the problem
        if network_estimate.congestion_level > 0.7 && !is_priority_message {
            ttl = ttl.saturating_sub(1);
        }

        // Clamp to valid range
        ttl.clamp(self.config.min_ttl, self.config.max_ttl)
    }

    /// Computes TTL for a reply message (e.g., ACK).
    ///
    /// Reply TTL should match the hop count of the incoming message
    /// to ensure the reply can reach the original sender.
    pub fn compute_reply_ttl(
        &self,
        incoming_hop_count: u8,
        network_estimate: &NetworkSizeEstimate,
    ) -> u8 {
        // Reply needs at least as many hops as it took to get here
        let min_reply_ttl = incoming_hop_count.saturating_add(2); // +2 for safety margin

        // Compute normal TTL
        let normal_ttl = self.compute_ttl(network_estimate, false, false);

        // Use the larger of the two
        min_reply_ttl.max(normal_ttl).min(self.config.max_ttl)
    }

    /// Checks if a message's TTL is sufficient for the current network.
    ///
    /// Returns the recommended boost if TTL seems too low, or 0 if sufficient.
    pub fn recommend_ttl_boost(
        &self,
        current_ttl: u8,
        network_estimate: &NetworkSizeEstimate,
    ) -> u8 {
        let optimal_ttl = self.compute_ttl(network_estimate, false, false);

        if current_ttl >= optimal_ttl {
            0
        } else {
            optimal_ttl.saturating_sub(current_ttl)
        }
    }

    /// Gets the current configuration.
    pub fn config(&self) -> &AdaptiveTtlConfig {
        &self.config
    }
}

impl Default for AdaptiveTtlCalculator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_ttl_for_small_network() {
        let calc = AdaptiveTtlCalculator::new();
        let estimate = NetworkSizeEstimate::new(4, 10);

        let ttl = calc.compute_ttl(&estimate, false, false);
        assert_eq!(ttl, 8); // Base TTL for small network
    }

    #[test]
    fn test_increased_ttl_for_large_network() {
        let calc = AdaptiveTtlCalculator::new();
        let estimate = NetworkSizeEstimate::new(4, 100).with_estimated_total(500);

        let ttl = calc.compute_ttl(&estimate, false, false);
        // 500 devices - 50 threshold = 450 extra, 450/100 = 4 extra TTL units
        // Base 8 + 4*2 = 16
        assert!(ttl > 8);
        assert!(ttl <= 24);
    }

    #[test]
    fn test_ttl_boost_for_queued_message() {
        let calc = AdaptiveTtlCalculator::new();
        let estimate = NetworkSizeEstimate::new(4, 20);

        let normal_ttl = calc.compute_ttl(&estimate, false, false);
        let queued_ttl = calc.compute_ttl(&estimate, false, true);

        assert!(queued_ttl > normal_ttl);
    }

    #[test]
    fn test_ttl_boost_for_priority_message() {
        let calc = AdaptiveTtlCalculator::new();
        let estimate = NetworkSizeEstimate::new(4, 20);

        let normal_ttl = calc.compute_ttl(&estimate, false, false);
        let priority_ttl = calc.compute_ttl(&estimate, true, false);

        assert!(priority_ttl > normal_ttl);
    }

    #[test]
    fn test_ttl_reduction_in_congestion() {
        let calc = AdaptiveTtlCalculator::new();
        let normal_estimate = NetworkSizeEstimate::new(4, 20);
        let congested_estimate = NetworkSizeEstimate::new(4, 20).with_congestion(0.8);

        let normal_ttl = calc.compute_ttl(&normal_estimate, false, false);
        let congested_ttl = calc.compute_ttl(&congested_estimate, false, false);

        assert!(congested_ttl <= normal_ttl);
    }

    #[test]
    fn test_ttl_clamped_to_max() {
        let calc = AdaptiveTtlCalculator::new();
        let huge_estimate = NetworkSizeEstimate::new(4, 1000).with_estimated_total(10000);

        let ttl = calc.compute_ttl(&huge_estimate, true, true);
        assert!(ttl <= 24); // Max TTL
    }

    #[test]
    fn test_ttl_clamped_to_min() {
        let config = AdaptiveTtlConfig {
            base_ttl: 2, // Very low base
            ..Default::default()
        };
        let calc = AdaptiveTtlCalculator::with_config(config);
        let estimate = NetworkSizeEstimate::new(2, 5).with_congestion(0.9);

        let ttl = calc.compute_ttl(&estimate, false, false);
        assert!(ttl >= 4); // Min TTL
    }

    #[test]
    fn test_reply_ttl_matches_hop_count() {
        let calc = AdaptiveTtlCalculator::new();
        let estimate = NetworkSizeEstimate::new(4, 20);

        // If message took 6 hops, reply needs at least 8 (6 + 2 safety margin)
        let reply_ttl = calc.compute_reply_ttl(6, &estimate);
        assert!(reply_ttl >= 8);
    }

    #[test]
    fn test_disabled_adaptive_ttl() {
        let config = AdaptiveTtlConfig {
            enabled: false,
            base_ttl: 10,
            ..Default::default()
        };
        let calc = AdaptiveTtlCalculator::with_config(config);

        let estimate = NetworkSizeEstimate::new(4, 1000).with_estimated_total(5000);
        let ttl = calc.compute_ttl(&estimate, true, true);

        assert_eq!(ttl, 10); // Should use base TTL regardless
    }

    #[test]
    fn test_network_size_estimation_heuristic() {
        let estimate = NetworkSizeEstimate::new(4, 50);
        let size = estimate.estimated_size();

        // Should estimate between visible_peers and visible_peers * 4
        assert!(size >= 50);
        assert!(size <= 200);
    }
}
