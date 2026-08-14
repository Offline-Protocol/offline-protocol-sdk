//! Relay management for promotion and demotion logic.

use serde::{Deserialize, Serialize};

/// Battery level below which a device stops doing anything for other people,
/// however willing its configuration is.
///
/// This is the hard floor beneath the soft [`RelayConfig::min_battery_for_relay`]:
/// a charging device, or one configured [`RelayPriority::Always`], is excused
/// the soft minimum but never this. It is public because the protocol crate
/// applies the same floor to message forwarding — a device must not keep
/// carrying traffic at a level that would have stripped it of the relay role,
/// and two copies of the number would eventually disagree.
pub const CRITICAL_RELAY_BATTERY_LEVEL: u8 = 15;

/// Configuration for relay behavior.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Minimum battery level to act as relay (percentage).
    pub min_battery_for_relay: u8,

    /// Whether this device allows acting as a relay.
    pub allow_relay: bool,

    /// Priority for relay selection (higher = more preferred).
    pub relay_priority: RelayPriority,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            min_battery_for_relay: 30,
            allow_relay: true,
            relay_priority: RelayPriority::Auto,
        }
    }
}

/// Relay priority levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayPriority {
    /// Never act as relay.
    Never,
    /// Automatically decide based on conditions.
    Auto,
    /// Always try to act as relay (if conditions met).
    Always,
}

/// Relay role of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayRole {
    /// Regular device (not a relay).
    Regular,
    /// Acting as a relay.
    Relay,
}

/// Information about a potential relay.
#[derive(Debug, Clone)]
pub struct RelayInfo {
    /// Number of connections this device has.
    pub connection_count: usize,

    /// Battery level (0-100).
    pub battery_level: u8,

    /// Whether the device is charging.
    pub is_charging: bool,

    /// Current relay role.
    pub role: RelayRole,

    /// Link quality to this relay (0-100).
    pub link_quality: u8,

    /// Current queue depth at this relay.
    pub queue_depth: usize,

    /// Congestion level at this relay (0.0-1.0).
    pub congestion_level: f32,
}

/// Scores relay candidates.
///
/// Holds no role of its own: whether *this* device is acting as a relay is
/// answered by the forwarding governor from what the device has actually
/// carried, not predicted here from thresholds. What remains is the
/// comparison of *other* devices as onward candidates.
pub struct RelayManager;

impl RelayManager {
    /// Creates a relay manager.
    pub fn new() -> Self {
        Self
    }

    /// Calculates a score for a potential relay.
    ///
    /// Higher score = better relay candidate.
    pub fn calculate_relay_score(&self, relay: &RelayInfo) -> f32 {
        let mut score = 0.0;

        // Connection count factor (0-30 points)
        let connection_score = (relay.connection_count as f32 / 10.0 * 30.0).min(30.0);
        score += connection_score;

        // Battery level factor (0-20 points)
        let battery_score = relay.battery_level as f32 / 100.0 * 20.0;
        score += battery_score;

        // Charging bonus (20 points)
        if relay.is_charging {
            score += 20.0;
        }

        // Link quality factor (0-20 points)
        let link_score = relay.link_quality as f32 / 100.0 * 20.0;
        score += link_score;

        // Congestion penalty (0-15 points deduction)
        let congestion_penalty = relay.congestion_level * 15.0;
        score -= congestion_penalty;

        // Queue depth penalty (0-15 points deduction)
        let queue_penalty = (relay.queue_depth as f32 / 50.0 * 15.0).min(15.0);
        score -= queue_penalty;

        score.max(0.0)
    }

    /// Selects the best relays from a list of candidates.
    ///
    /// # Arguments
    ///
    /// * `candidates` - List of potential relays
    /// * `count` - Number of relays to select
    /// * `max_congestion_level` - Congestion level (0.0-1.0) at or above
    ///   which a candidate is considered overloaded and excluded; pass the
    ///   caller's path policy (e.g. `PathConfig::max_congestion_level`)
    ///
    /// # Returns
    ///
    /// Returns up to `count` relay candidates, sorted by score (best first).
    pub fn select_best_relays(
        &self,
        mut candidates: Vec<RelayInfo>,
        count: usize,
        max_congestion_level: f32,
    ) -> Vec<RelayInfo> {
        // Filter out overloaded relays
        candidates.retain(|relay| relay.congestion_level < max_congestion_level);

        // Calculate scores and sort
        candidates.sort_by(|a, b| {
            let score_a = self.calculate_relay_score(a);
            let score_b = self.calculate_relay_score(b);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top N
        candidates.into_iter().take(count).collect()
    }
}

impl Default for RelayManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relay_scoring() {
        let manager = RelayManager::new();

        let good_relay = RelayInfo {
            connection_count: 8,
            battery_level: 80,
            is_charging: true,
            role: RelayRole::Relay,
            link_quality: 90,
            queue_depth: 5,
            congestion_level: 0.1,
        };

        let poor_relay = RelayInfo {
            connection_count: 3,
            battery_level: 25,
            is_charging: false,
            role: RelayRole::Regular,
            link_quality: 40,
            queue_depth: 60,
            congestion_level: 0.8,
        };

        let good_score = manager.calculate_relay_score(&good_relay);
        let poor_score = manager.calculate_relay_score(&poor_relay);

        assert!(good_score > poor_score);
        assert!(good_score > 70.0); // Good relay should score high
    }

    #[test]
    fn test_select_best_relays() {
        let manager = RelayManager::new();

        let relays = vec![
            RelayInfo {
                connection_count: 8,
                battery_level: 80,
                is_charging: true,
                role: RelayRole::Relay,
                link_quality: 90,
                queue_depth: 5,
                congestion_level: 0.1,
            },
            RelayInfo {
                connection_count: 5,
                battery_level: 60,
                is_charging: false,
                role: RelayRole::Relay,
                link_quality: 70,
                queue_depth: 15,
                congestion_level: 0.3,
            },
            RelayInfo {
                connection_count: 3,
                battery_level: 40,
                is_charging: false,
                role: RelayRole::Regular,
                link_quality: 50,
                queue_depth: 40,
                congestion_level: 0.6,
            },
        ];

        let best = manager.select_best_relays(relays, 2, 0.7);
        assert_eq!(best.len(), 2);

        // First relay should be the best one
        assert_eq!(best[0].connection_count, 8);
    }

    #[test]
    fn test_select_best_relays_honors_congestion_threshold() {
        let manager = RelayManager::new();
        let candidate = |congestion_level: f32| RelayInfo {
            connection_count: 5,
            battery_level: 60,
            is_charging: false,
            role: RelayRole::Relay,
            link_quality: 70,
            queue_depth: 10,
            congestion_level,
        };

        // A 0.6-congestion candidate passes the default-style 0.7 threshold
        // but must be excluded under a stricter 0.5 policy.
        let best = manager.select_best_relays(vec![candidate(0.6)], 1, 0.7);
        assert_eq!(best.len(), 1);

        let best = manager.select_best_relays(vec![candidate(0.6)], 1, 0.5);
        assert!(best.is_empty());
    }
}
