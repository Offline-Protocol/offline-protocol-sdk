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

/// Reason a device was demoted from relay role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayDemotionReason {
    /// Active connection count fell below the relay threshold.
    LowConnections,
    /// Battery dropped below the level required to keep relaying.
    LowBattery {
        /// Minimum battery level required to remain a relay.
        min_required: u8,
    },
    /// The configuration forbids relaying (`allow_relay` is `false` or the
    /// priority is [`RelayPriority::Never`]), so the role is surrendered
    /// regardless of connections or battery.
    RelayDisallowed,
}

/// A relay-role transition produced by [`RelayManager::evaluate_transition`].
///
/// `None` from `evaluate_transition` means the role did not change; a `Some`
/// value is emitted exactly once, at the tick the transition happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayTransition {
    /// The device became a relay.
    Promoted {
        /// Active connection count at the time of promotion.
        connection_count: usize,
        /// Battery level at the time of promotion.
        battery_level: u8,
    },
    /// The device stopped being a relay.
    Demoted(RelayDemotionReason),
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
        // Config-level opt-outs apply regardless of priority mode — the
        // charging shortcut below must not override an explicit "don't relay".
        if self.relaying_disallowed() {
            return false;
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
    /// * `is_charging` - Whether the device is charging
    ///
    /// # Returns
    ///
    /// Returns `true` if the device should stop being a relay.
    pub fn should_demote_from_relay(
        &self,
        connection_count: usize,
        battery_level: u8,
        is_charging: bool,
    ) -> bool {
        // A device whose config forbids relaying must surrender the role,
        // however it acquired it (external `set_role`, config change).
        if self.relaying_disallowed() {
            return true;
        }

        // If priority is Always, stay as relay unless critical battery
        if matches!(self.config.relay_priority, RelayPriority::Always) {
            return battery_level < 15;
        }

        // Demote if connections drop below threshold
        if connection_count < self.config.relay_threshold {
            return true;
        }

        // Demote if battery is too low. A charging device is held to the same
        // relaxed floor as `should_promote_to_relay` (which favors a charging
        // device even below `min_battery_for_relay`): only a *critical* level
        // forces demotion. Without this symmetry a charging device sitting just
        // below the soft minimum would oscillate promote → demote every tick.
        if battery_level < self.demotion_battery_floor(is_charging) {
            return true;
        }

        false
    }

    /// Whether the configuration forbids acting as a relay at all.
    ///
    /// Mirrors the message-forwarding gate in the protocol crate: relaying is
    /// off when `allow_relay` is `false` or the priority is `Never`.
    fn relaying_disallowed(&self) -> bool {
        !self.config.allow_relay || matches!(self.config.relay_priority, RelayPriority::Never)
    }

    /// Battery level below which a relay must demote, given charging state.
    ///
    /// Mirrors the battery branches of [`should_demote_from_relay`] so the
    /// transition classifier and the demotion decision never disagree.
    fn demotion_battery_floor(&self, is_charging: bool) -> u8 {
        if matches!(self.config.relay_priority, RelayPriority::Always) || is_charging {
            15
        } else {
            self.config.min_battery_for_relay
        }
    }

    /// Evaluates whether the relay role should change and, if so, applies the
    /// change and returns the transition that occurred.
    ///
    /// This is the single entry point that mutates [`RelayRole`]: it compares
    /// the promote/demote decision against the current role and only returns
    /// `Some` when the role actually flips, so callers can emit a role-change
    /// event exactly once per transition. A stable role returns `None`.
    pub fn evaluate_transition(
        &mut self,
        connection_count: usize,
        battery_level: u8,
        is_charging: bool,
    ) -> Option<RelayTransition> {
        match self.current_role {
            RelayRole::Regular => {
                if self.should_promote_to_relay(connection_count, battery_level, is_charging) {
                    self.set_role(RelayRole::Relay);
                    Some(RelayTransition::Promoted {
                        connection_count,
                        battery_level,
                    })
                } else {
                    None
                }
            }
            RelayRole::Relay => {
                if self.should_demote_from_relay(connection_count, battery_level, is_charging) {
                    self.set_role(RelayRole::Regular);
                    // A config-level opt-out outranks resource state: without
                    // it a healthy-but-disallowed relay would be misreported
                    // as connection-starved. Below that, battery takes
                    // precedence: a relay that is both connection-starved and
                    // battery-starved is reported as a battery demotion,
                    // which is the signal the analytics panel tracks
                    // distinctly.
                    let floor = self.demotion_battery_floor(is_charging);
                    let reason = if self.relaying_disallowed() {
                        RelayDemotionReason::RelayDisallowed
                    } else if battery_level < floor {
                        RelayDemotionReason::LowBattery {
                            min_required: floor,
                        }
                    } else {
                        RelayDemotionReason::LowConnections
                    };
                    Some(RelayTransition::Demoted(reason))
                } else {
                    None
                }
            }
        }
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
        assert!(manager.should_demote_from_relay(2, 80, false));

        // Should demote if battery too low
        assert!(manager.should_demote_from_relay(5, 25, false));

        // Should stay as relay with good conditions
        assert!(!manager.should_demote_from_relay(5, 80, false));
    }

    #[test]
    fn test_should_demote_charging_exempt_from_soft_floor() {
        let manager = RelayManager::new(); // min_battery_for_relay = 30

        // Not charging, battery below the soft minimum -> demote.
        assert!(manager.should_demote_from_relay(5, 25, false));

        // Charging at the same level -> stay (mirrors promotion); only a
        // critical battery level forces demotion while charging.
        assert!(!manager.should_demote_from_relay(5, 25, true));
        assert!(manager.should_demote_from_relay(5, 10, true));
    }

    #[test]
    fn test_evaluate_transition_promote_then_idempotent() {
        let mut manager = RelayManager::new();
        assert_eq!(manager.current_role(), RelayRole::Regular);

        // Good conditions promote, returning the transition exactly once.
        assert_eq!(
            manager.evaluate_transition(5, 80, false),
            Some(RelayTransition::Promoted {
                connection_count: 5,
                battery_level: 80,
            })
        );
        assert_eq!(manager.current_role(), RelayRole::Relay);

        // Stable conditions on the next tick produce no transition.
        assert_eq!(manager.evaluate_transition(5, 80, false), None);
        assert_eq!(manager.current_role(), RelayRole::Relay);
    }

    #[test]
    fn test_evaluate_transition_no_promote_stays_regular() {
        let mut manager = RelayManager::new();
        // Too few connections: no promotion, role unchanged.
        assert_eq!(manager.evaluate_transition(1, 80, false), None);
        assert_eq!(manager.current_role(), RelayRole::Regular);
    }

    #[test]
    fn test_evaluate_transition_demote_low_connections() {
        let mut manager = RelayManager::new();
        manager.set_role(RelayRole::Relay);

        // Battery healthy, connections collapsed -> connection demotion.
        assert_eq!(
            manager.evaluate_transition(1, 80, false),
            Some(RelayTransition::Demoted(
                RelayDemotionReason::LowConnections
            ))
        );
        assert_eq!(manager.current_role(), RelayRole::Regular);
    }

    #[test]
    fn test_evaluate_transition_demote_low_battery() {
        let mut manager = RelayManager::new(); // min_battery_for_relay = 30
        manager.set_role(RelayRole::Relay);

        // Enough connections, battery below minimum -> battery demotion,
        // reporting the soft floor as the required level.
        assert_eq!(
            manager.evaluate_transition(5, 25, false),
            Some(RelayTransition::Demoted(RelayDemotionReason::LowBattery {
                min_required: 30,
            }))
        );
        assert_eq!(manager.current_role(), RelayRole::Regular);
    }

    #[test]
    fn test_evaluate_transition_battery_precedence_over_connections() {
        let mut manager = RelayManager::new();
        manager.set_role(RelayRole::Relay);

        // Both connection-starved and battery-starved: classified as battery.
        assert_eq!(
            manager.evaluate_transition(1, 10, false),
            Some(RelayTransition::Demoted(RelayDemotionReason::LowBattery {
                min_required: 30,
            }))
        );
    }

    #[test]
    fn test_evaluate_transition_always_critical_battery_floor() {
        let config = RelayConfig {
            relay_priority: RelayPriority::Always,
            ..Default::default()
        };
        let mut manager = RelayManager::with_config(config);
        manager.set_role(RelayRole::Relay);

        // Always priority demotes only at critical battery, reporting 15.
        assert_eq!(
            manager.evaluate_transition(1, 10, false),
            Some(RelayTransition::Demoted(RelayDemotionReason::LowBattery {
                min_required: 15,
            }))
        );
    }

    #[test]
    fn test_evaluate_transition_no_flap_while_charging() {
        let mut manager = RelayManager::new(); // min_battery_for_relay = 30

        // Charging device just below the soft minimum: promotes, then holds
        // (no promote → demote oscillation that would emit fake churn).
        assert!(matches!(
            manager.evaluate_transition(5, 25, true),
            Some(RelayTransition::Promoted { .. })
        ));
        assert_eq!(manager.evaluate_transition(5, 25, true), None);
        assert_eq!(manager.current_role(), RelayRole::Relay);
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

    #[test]
    fn test_relay_priority_never() {
        let config = RelayConfig {
            relay_priority: RelayPriority::Never,
            ..Default::default()
        };
        let manager = RelayManager::with_config(config);

        // Should never promote, even with perfect conditions
        assert!(!manager.should_promote_to_relay(10, 100, true));

        // And must surrender the role if it somehow holds it.
        assert!(manager.should_demote_from_relay(10, 100, true));
    }

    #[test]
    fn test_allow_relay_false_blocks_auto_promotion() {
        let config = RelayConfig {
            allow_relay: false,
            ..Default::default() // Auto priority
        };
        let manager = RelayManager::with_config(config);

        // Perfect conditions, including the charging shortcut that used to
        // bypass the opt-out entirely in Auto mode.
        assert!(!manager.should_promote_to_relay(10, 100, true));
        assert!(!manager.should_promote_to_relay(10, 100, false));
    }

    #[test]
    fn test_allow_relay_false_forces_demotion() {
        let config = RelayConfig {
            allow_relay: false,
            ..Default::default()
        };
        let manager = RelayManager::with_config(config);

        // Healthy connections and battery are irrelevant: the config says no.
        assert!(manager.should_demote_from_relay(10, 100, false));
    }

    #[test]
    fn test_allow_relay_false_blocks_always_priority() {
        let config = RelayConfig {
            allow_relay: false,
            relay_priority: RelayPriority::Always,
            ..Default::default()
        };
        let manager = RelayManager::with_config(config);

        assert!(!manager.should_promote_to_relay(10, 100, true));
        assert!(manager.should_demote_from_relay(10, 100, true));
    }

    #[test]
    fn test_evaluate_transition_demote_relay_disallowed() {
        let config = RelayConfig {
            allow_relay: false,
            ..Default::default()
        };
        let mut manager = RelayManager::with_config(config);
        manager.set_role(RelayRole::Relay);

        // Healthy stats: the demotion must be classified as disallowed, not
        // misreported as connection- or battery-starvation.
        assert_eq!(
            manager.evaluate_transition(10, 100, false),
            Some(RelayTransition::Demoted(
                RelayDemotionReason::RelayDisallowed
            ))
        );
        assert_eq!(manager.current_role(), RelayRole::Regular);
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
