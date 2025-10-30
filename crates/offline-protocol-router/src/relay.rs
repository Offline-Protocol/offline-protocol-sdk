//! Relay node management

use offline_protocol_core::DeviceId;
use parking_lot::RwLock;
use std::sync::Arc;
use tracing::{debug, info};

/// Configuration for relay manager
#[derive(Debug, Clone)]
pub struct RelayManagerConfig {
    /// Allow this device to act as a relay
    pub allow_act_as_relay: bool,
    
    /// Relay priority: 'auto', 'always', or 'never'
    pub relay_priority: RelayPriority,
    
    /// Minimum connection count to become a relay
    pub relay_threshold: u8,
    
    /// Minimum battery percentage to act as relay
    pub min_battery_for_relay: u8,
}

impl Default for RelayManagerConfig {
    fn default() -> Self {
        Self {
            allow_act_as_relay: true,
            relay_priority: RelayPriority::Auto,
            relay_threshold: 3,
            min_battery_for_relay: 30,
        }
    }
}

/// Relay priority setting
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayPriority {
    Auto,
    Always,
    Never,
}

impl From<&str> for RelayPriority {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "always" => RelayPriority::Always,
            "never" => RelayPriority::Never,
            _ => RelayPriority::Auto,
        }
    }
}

/// Relay manager for promoting/demoting relay nodes
pub struct RelayManager {
    config: RelayManagerConfig,
    is_relay: Arc<RwLock<bool>>,
    connection_count: Arc<RwLock<u8>>,
    battery_level: Arc<RwLock<u8>>, // Percentage
}

impl RelayManager {
    pub fn new(config: RelayManagerConfig) -> Self {
        Self {
            config,
            is_relay: Arc::new(RwLock::new(false)),
            connection_count: Arc::new(RwLock::new(0)),
            battery_level: Arc::new(RwLock::new(100)),
        }
    }

    /// Check if this device is currently acting as a relay
    pub fn is_relay(&self) -> bool {
        *self.is_relay.read()
    }

    /// Get current connection count
    pub fn connection_count(&self) -> u8 {
        *self.connection_count.read()
    }

    /// Update connection count
    pub fn set_connection_count(&self, count: u8) {
        *self.connection_count.write() = count;
        self.reevaluate_relay_status();
    }

    /// Update battery level (0-100)
    pub fn set_battery_level(&self, level: u8) {
        let level = level.min(100);
        *self.battery_level.write() = level;
        self.reevaluate_relay_status();
    }

    /// Get current battery level
    pub fn battery_level(&self) -> u8 {
        *self.battery_level.read()
    }

    /// Manually set relay status
    pub fn set_relay_status(&self, is_relay: bool) {
        let old_status = *self.is_relay.read();
        *self.is_relay.write() = is_relay;
        
        if old_status != is_relay {
            if is_relay {
                info!("Device promoted to relay");
            } else {
                info!("Device demoted from relay");
            }
        }
    }

    /// Check if device should act as relay
    fn should_be_relay(&self) -> bool {
        // Check policy
        if !self.config.allow_act_as_relay {
            return false;
        }

        match self.config.relay_priority {
            RelayPriority::Never => return false,
            RelayPriority::Always => return true,
            RelayPriority::Auto => {
                // Continue with automatic determination
            }
        }

        // Check battery level
        let battery = *self.battery_level.read();
        if battery < self.config.min_battery_for_relay {
            debug!("Battery too low for relay: {}%", battery);
            return false;
        }

        // Check connection count
        let connections = *self.connection_count.read();
        connections >= self.config.relay_threshold
    }

    /// Reevaluate and update relay status
    fn reevaluate_relay_status(&self) {
        let should_be_relay = self.should_be_relay();
        let is_relay = *self.is_relay.read();

        if should_be_relay && !is_relay {
            // Promote to relay
            self.set_relay_status(true);
        } else if !should_be_relay && is_relay {
            // Demote from relay
            self.set_relay_status(false);
        }
    }

    /// Check if a message should be forwarded (relay behavior)
    pub fn should_forward(&self, _recipient: &DeviceId, _sender: &DeviceId) -> bool {
        // Only forward if we're acting as a relay
        if !self.is_relay() {
            return false;
        }

        // TODO: More sophisticated forwarding logic
        // - Don't forward back to sender
        // - Consider message TTL
        // - Rate limiting

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relay_promotion() {
        let config = RelayManagerConfig {
            relay_threshold: 3,
            min_battery_for_relay: 30,
            ..Default::default()
        };
        let manager = RelayManager::new(config);

        // Initially not a relay
        assert!(!manager.is_relay());

        // Increase connections below threshold
        manager.set_connection_count(2);
        assert!(!manager.is_relay());

        // Reach threshold
        manager.set_connection_count(3);
        assert!(manager.is_relay());

        // Drop below threshold
        manager.set_connection_count(2);
        assert!(!manager.is_relay());
    }

    #[test]
    fn test_battery_threshold() {
        let config = RelayManagerConfig {
            relay_threshold: 2,
            min_battery_for_relay: 30,
            ..Default::default()
        };
        let manager = RelayManager::new(config);

        // Set enough connections
        manager.set_connection_count(3);
        assert!(manager.is_relay());

        // Lower battery below threshold
        manager.set_battery_level(25);
        assert!(!manager.is_relay());

        // Restore battery
        manager.set_battery_level(50);
        assert!(manager.is_relay());
    }

    #[test]
    fn test_relay_priority_always() {
        let config = RelayManagerConfig {
            relay_priority: RelayPriority::Always,
            ..Default::default()
        };
        let manager = RelayManager::new(config);

        // Should be relay even with no connections
        manager.set_connection_count(0);
        manager.reevaluate_relay_status();
        // Note: reevaluate_relay_status is private, so we test through set_connection_count
    }
}

