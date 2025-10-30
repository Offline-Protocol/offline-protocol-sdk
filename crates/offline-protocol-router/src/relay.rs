//! Relay management for promotion and demotion logic.

use serde::{Deserialize, Serialize};

/// Configuration for relay behavior.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Minimum number of connections to become a relay.
    pub relay_threshold: usize,

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
            relay_threshold: 3,
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

/// Relay manager for promotion and demotion logic.
pub struct RelayManager {
    config: RelayConfig,
    current_role: RelayRole,
}

impl RelayManager {
    /// Creates a new relay manager with default configuration.
    pub fn new() -> Self {
        Self::with_config(RelayConfig::default())
    }

    /// Creates a new relay manager with custom configuration.
    pub fn with_config(config: RelayConfig) -> Self {
        Self {
            config,
            current_role: RelayRole::Regular,
        }
    }

    /// Checks if this device should be promoted to relay role.
    ///
    /// # Arguments
    ///
    /// * `connection_count` - Number of active connections
    /// * `battery_level` - Current battery level (0-100)
    /// * `is_charging` - Whether the device is charging
    ///
    /// # Returns
    ///
    /// Returns `true` if the device should become a relay.
    pub fn should_promote_to_relay(
        &self,
        connection_count: usize,
        battery_level: u8,
        is_charging: bool,
    ) -> bool {
        // Check relay priority
        match self.config.relay_priority {
            RelayPriority::Never => return false,
            RelayPriority::Always if !self.config.allow_relay => return false,
            _ => {}
        }

        // Must have enough connections
        if connection_count < self.config.relay_threshold {
            return false;
        }

        // Check battery constraints
        if battery_level < 15 {
            // Critical battery - never become relay
            return false;
        }

        if battery_level < self.config.min_battery_for_relay && !is_charging {
            // Low battery and not charging - only if Always priority
            return matches!(self.config.relay_priority, RelayPriority::Always);
        }

        // If charging, always prefer to be relay (helps network)
        if is_charging {
            return true;
        }

        // Auto mode: become relay if conditions are good
        matches!(
            self.config.relay_priority,
            RelayPriority::Auto | RelayPriority::Always
        )
    }

    /// Checks if this device should be demoted from relay role.
    ///
    /// # Arguments
    ///
    /// * `connection_count` - Number of active connections
    /// * `battery_level` - Current battery level (0-100)
    ///
    /// # Returns
    ///
    /// Returns `true` if the device should stop being a relay.
    pub fn should_demote_from_relay(&self, connection_count: usize, battery_level: u8) -> bool {
        // If priority is Always, stay as relay unless critical battery
        if matches!(self.config.relay_priority, RelayPriority::Always) {
            return battery_level < 15;
        }

        // Demote if connections drop below threshold
        if connection_count < self.config.relay_threshold {
            return true;
        }

        // Demote if battery is too low
        if battery_level < self.config.min_battery_for_relay {
            return true;
        }

        false
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
    ///
    /// # Returns
    ///
    /// Returns up to `count` relay candidates, sorted by score (best first).
    pub fn select_best_relays(
        &self,
        mut candidates: Vec<RelayInfo>,
        count: usize,
    ) -> Vec<RelayInfo> {
        // Filter out overloaded relays (congestion > 0.7)
        candidates.retain(|relay| relay.congestion_level < 0.7);

        // Calculate scores and sort
        candidates.sort_by(|a, b| {
            let score_a = self.calculate_relay_score(a);
            let score_b = self.calculate_relay_score(b);
            score_b.partial_cmp(&score_a).unwrap()
        });

        // Take top N
        candidates.into_iter().take(count).collect()
    }

    /// Gets the current relay role.
    pub fn current_role(&self) -> RelayRole {
        self.current_role
    }

    /// Sets the current relay role.
    pub fn set_role(&mut self, role: RelayRole) {
        self.current_role = role;
    }

    /// Gets the relay configuration.
    pub fn config(&self) -> &RelayConfig {
        &self.config
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
    fn test_relay_promotion_basic() {
        let manager = RelayManager::new();

        // Good conditions: many connections, good battery
        assert!(manager.should_promote_to_relay(5, 80, false));

        // Too few connections
        assert!(!manager.should_promote_to_relay(2, 80, false));

        // Critical battery
        assert!(!manager.should_promote_to_relay(5, 10, false));
    }

    #[test]
    fn test_relay_promotion_charging_preferred() {
        let manager = RelayManager::new();

        // Charging device is preferred even with lower battery
        assert!(manager.should_promote_to_relay(3, 25, true));
    }

    #[test]
    fn test_relay_demotion() {
        let manager = RelayManager::new();

        // Should demote if connections drop
        assert!(manager.should_demote_from_relay(2, 80));

        // Should demote if battery too low
        assert!(manager.should_demote_from_relay(5, 25));

        // Should stay as relay with good conditions
        assert!(!manager.should_demote_from_relay(5, 80));
    }

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

        let best = manager.select_best_relays(relays, 2);
        assert_eq!(best.len(), 2);

        // First relay should be the best one
        assert_eq!(best[0].connection_count, 8);
    }

    #[test]
    fn test_relay_priority_never() {
        let config = RelayConfig {
            relay_priority: RelayPriority::Never,
            ..Default::default()
        };
        let manager = RelayManager::with_config(config);

        // Should never promote, even with perfect conditions
        assert!(!manager.should_promote_to_relay(10, 100, true));
    }

    #[test]
    fn test_relay_priority_always() {
        let config = RelayConfig {
            relay_priority: RelayPriority::Always,
            ..Default::default()
        };
        let manager = RelayManager::with_config(config);

        // Should promote even with lower battery (if not critical)
        assert!(manager.should_promote_to_relay(3, 20, false));

        // But not with critical battery
        assert!(!manager.should_promote_to_relay(3, 10, false));
    }
}
