//! Path selection and routing for optimal message delivery.

use crate::relay::{RelayInfo, RelayManager};
use offline_protocol_core::Message;

/// Information about a neighboring device.
#[derive(Debug, Clone)]
pub struct NeighborInfo {
    /// Unique identifier for this neighbor.
    pub peer_id: String,

    /// RSSI signal strength (dBm).
    pub rssi: i16,

    /// Number of hops to destination through this neighbor (if known).
    pub hops_to_destination: Option<u8>,

    /// Link quality to this neighbor (0-100).
    pub link_quality: u8,

    /// Relay information if this neighbor is a relay.
    pub relay_info: Option<RelayInfo>,
}

/// Path selection configuration.
#[derive(Debug, Clone)]
pub struct PathConfig {
    /// Number of top relays to forward to (for redundancy).
    pub forward_to_top_k: usize,

    /// Maximum acceptable congestion level (0.0-1.0).
    pub max_congestion_level: f32,
}

impl Default for PathConfig {
    fn default() -> Self {
        Self {
            forward_to_top_k: 3,
            max_congestion_level: 0.7,
        }
    }
}

/// Path score breakdown for debugging/monitoring.
#[derive(Debug, Clone)]
pub struct PathScore {
    /// Signal strength component (0-100).
    pub signal: f32,

    /// Proximity/hops component (0-100).
    pub proximity: f32,

    /// Capacity component (0-100).
    pub capacity: f32,

    /// Energy component (0-100).
    pub energy: f32,

    /// Total weighted score.
    pub total: f32,
}

/// Path selector for optimal relay selection.
pub struct PathSelector {
    config: PathConfig,
    relay_manager: RelayManager,
}

impl PathSelector {
    /// Creates a new path selector with default configuration.
    pub fn new() -> Self {
        Self::with_config(PathConfig::default(), RelayManager::new())
    }

    /// Creates a new path selector with custom configuration.
    pub fn with_config(config: PathConfig, relay_manager: RelayManager) -> Self {
        Self {
            config,
            relay_manager,
        }
    }

    /// Selects the best path(s) for message delivery.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to route
    /// * `neighbors` - Available neighboring devices
    ///
    /// # Returns
    ///
    /// Returns a list of neighbors to forward the message to, ordered by preference.
    pub fn select_paths(&self, message: &Message, neighbors: &[NeighborInfo]) -> Vec<String> {
        if neighbors.is_empty() {
            return Vec::new();
        }

        // Calculate scores for each neighbor
        let mut scored_neighbors: Vec<(String, PathScore)> = neighbors
            .iter()
            .map(|neighbor| {
                let score = self.calculate_path_score(message, neighbor);
                (neighbor.peer_id.clone(), score)
            })
            .collect();

        // Filter out neighbors with relay congestion > max threshold
        scored_neighbors.retain(|(peer_id, _)| {
            if let Some(neighbor) = neighbors.iter().find(|n| n.peer_id == *peer_id) {
                if let Some(relay_info) = &neighbor.relay_info {
                    return relay_info.congestion_level <= self.config.max_congestion_level;
                }
            }
            true
        });

        // Sort by total score (descending)
        scored_neighbors.sort_by(|a, b| b.1.total.partial_cmp(&a.1.total).unwrap());

        // Select top K neighbors for redundancy
        scored_neighbors
            .into_iter()
            .take(self.config.forward_to_top_k)
            .map(|(peer_id, _)| peer_id)
            .collect()
    }

    /// Calculates the path score for a neighbor.
    fn calculate_path_score(&self, message: &Message, neighbor: &NeighborInfo) -> PathScore {
        let signal_score = self.calculate_signal_score(neighbor);
        let proximity_score = self.calculate_proximity_score(message, neighbor);
        let capacity_score = self.calculate_capacity_score(neighbor);
        let energy_score = self.calculate_energy_score(neighbor);

        // Weighted combination (from DORS spec)
        let total = (signal_score * 0.3)
            + (proximity_score * 0.2)
            + (capacity_score * 0.3)
            + (energy_score * 0.2);

        PathScore {
            signal: signal_score,
            proximity: proximity_score,
            capacity: capacity_score,
            energy: energy_score,
            total,
        }
    }

    /// Calculates signal strength score from RSSI.
    fn calculate_signal_score(&self, neighbor: &NeighborInfo) -> f32 {
        let rssi = neighbor.rssi;

        // Convert RSSI to 0-100 score
        if rssi >= -50 {
            100.0
        } else if rssi >= -70 {
            70.0 + ((rssi + 70) as f32 * 30.0 / 20.0)
        } else if rssi >= -85 {
            40.0 + ((rssi + 85) as f32 * 30.0 / 15.0)
        } else {
            ((rssi + 100).max(0) as f32 * 40.0 / 15.0).max(0.0)
        }
    }

    /// Calculates proximity score based on hop distance.
    fn calculate_proximity_score(&self, message: &Message, neighbor: &NeighborInfo) -> f32 {
        if let Some(hops) = neighbor.hops_to_destination {
            // We know the distance to destination through this neighbor
            let remaining_ttl = message.ttl.value();

            if hops == 0 {
                // Direct connection to destination
                100.0
            } else {
                // Score based on whether we have enough TTL
                let score = if hops < remaining_ttl {
                    100.0 - (hops as f32 / remaining_ttl as f32 * 50.0)
                } else {
                    // Not enough TTL, but still possible
                    20.0
                };
                score.max(0.0)
            }
        } else {
            // Unknown distance, use link quality as proxy
            neighbor.link_quality as f32
        }
    }

    /// Calculates capacity score based on relay status and congestion.
    fn calculate_capacity_score(&self, neighbor: &NeighborInfo) -> f32 {
        if let Some(relay_info) = &neighbor.relay_info {
            let relay_score = self.relay_manager.calculate_relay_score(relay_info);

            // Normalize relay score (typically 0-100) to 0-100 scale
            relay_score.min(100.0)
        } else {
            // Not a relay, assume basic capacity
            50.0
        }
    }

    /// Calculates energy score based on battery level.
    fn calculate_energy_score(&self, neighbor: &NeighborInfo) -> f32 {
        if let Some(relay_info) = &neighbor.relay_info {
            let mut score = relay_info.battery_level as f32;

            // Bonus for charging devices
            if relay_info.is_charging {
                score = 100.0;
            }

            score
        } else {
            // Assume good battery for non-relay devices
            70.0
        }
    }

    /// Finds the best single path (for unicast).
    pub fn select_best_path(
        &self,
        message: &Message,
        neighbors: &[NeighborInfo],
    ) -> Option<String> {
        self.select_paths(message, neighbors).into_iter().next()
    }

    /// Checks if routing around congestion is needed.
    pub fn should_route_around_congestion(&self, neighbor: &NeighborInfo) -> bool {
        if let Some(relay_info) = &neighbor.relay_info {
            relay_info.congestion_level > self.config.max_congestion_level
        } else {
            false
        }
    }

    /// Gets the path configuration.
    pub fn config(&self) -> &PathConfig {
        &self.config
    }
}

impl Default for PathSelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::{RelayInfo, RelayRole};
    use offline_protocol_core::{AppId, UserId};

    fn create_test_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "Test message",
        )
    }

    fn create_neighbor(id: &str, rssi: i16, hops: Option<u8>, congestion: f32) -> NeighborInfo {
        NeighborInfo {
            peer_id: id.to_string(),
            rssi,
            hops_to_destination: hops,
            link_quality: 80,
            relay_info: Some(RelayInfo {
                connection_count: 5,
                battery_level: 70,
                is_charging: false,
                role: RelayRole::Relay,
                link_quality: 80,
                queue_depth: 10,
                congestion_level: congestion,
            }),
        }
    }

    #[test]
    fn test_path_selection_basic() {
        let selector = PathSelector::new();
        let message = create_test_message();

        let neighbors = vec![
            create_neighbor("peer1", -60, Some(2), 0.2),
            create_neighbor("peer2", -70, Some(3), 0.3),
            create_neighbor("peer3", -55, Some(1), 0.1),
        ];

        let paths = selector.select_paths(&message, &neighbors);

        // Should select top K paths
        assert!(!paths.is_empty());
        assert!(paths.len() <= 3);

        // Best path should be peer3 (best RSSI, lowest hops, lowest congestion)
        assert_eq!(paths[0], "peer3");
    }

    #[test]
    fn test_congestion_filtering() {
        let selector = PathSelector::new();
        let message = create_test_message();

        let neighbors = vec![
            create_neighbor("peer1", -60, Some(2), 0.9), // High congestion
            create_neighbor("peer2", -65, Some(3), 0.3),
        ];

        let paths = selector.select_paths(&message, &neighbors);

        // Should filter out peer1 due to high congestion
        assert!(!paths.contains(&"peer1".to_string()));
    }

    #[test]
    fn test_direct_destination_preferred() {
        let selector = PathSelector::new();
        let message = create_test_message();

        let neighbors = vec![
            create_neighbor("peer1", -70, Some(5), 0.3),
            create_neighbor("peer2", -70, Some(0), 0.1), // Direct to destination
        ];

        let paths = selector.select_paths(&message, &neighbors);

        // Should prefer peer2 (direct destination) with same signal
        assert_eq!(paths[0], "peer2");
    }

    #[test]
    fn test_signal_score_calculation() {
        let selector = PathSelector::new();

        let excellent = create_neighbor("peer1", -40, None, 0.1);
        let score = selector.calculate_signal_score(&excellent);
        assert_eq!(score, 100.0);

        let poor = create_neighbor("peer2", -90, None, 0.1);
        let score = selector.calculate_signal_score(&poor);
        assert!(score < 40.0);
    }

    #[test]
    fn test_empty_neighbors() {
        let selector = PathSelector::new();
        let message = create_test_message();

        let paths = selector.select_paths(&message, &[]);
        assert!(paths.is_empty());
    }

    #[test]
    fn test_select_best_path() {
        let selector = PathSelector::new();
        let message = create_test_message();

        let neighbors = vec![
            create_neighbor("peer1", -60, Some(2), 0.2),
            create_neighbor("peer2", -55, Some(1), 0.1),
        ];

        let best = selector.select_best_path(&message, &neighbors);
        assert_eq!(best, Some("peer2".to_string()));
    }

    #[test]
    fn test_route_around_congestion() {
        let selector = PathSelector::new();

        let congested = create_neighbor("peer1", -60, Some(2), 0.9);
        assert!(selector.should_route_around_congestion(&congested));

        let normal = create_neighbor("peer2", -60, Some(2), 0.3);
        assert!(!selector.should_route_around_congestion(&normal));
    }
}
