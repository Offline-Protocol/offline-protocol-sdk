//! Protocol configuration.

use offline_protocol_reliability::{AckConfig, DeduplicatorConfig, RetryConfig};
use offline_protocol_router::{DorsConfig, PathConfig, RelayConfig};
use serde::{Deserialize, Serialize};

/// Transport-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// Whether BLE transport is enabled.
    pub ble_enabled: bool,

    /// Whether Wi-Fi Direct transport is enabled.
    pub wifi_direct_enabled: bool,

    /// Whether Internet transport is enabled.
    pub internet_enabled: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            ble_enabled: true,
            wifi_direct_enabled: true,
            internet_enabled: true,
        }
    }
}

/// Reliability configuration combining ACK, retry, and deduplication settings.
#[derive(Debug, Clone, Default)]
pub struct ReliabilityConfig {
    /// ACK manager configuration.
    pub ack: AckConfig,

    /// Retry queue configuration.
    pub retry: RetryConfig,

    /// Deduplicator configuration.
    pub dedup: DeduplicatorConfig,
}

/// Main configuration for the Offline Protocol.
#[derive(Debug, Clone)]
pub struct ProtocolConfig {
    /// Application identifier (required).
    pub app_id: String,

    /// User identifier (required).
    pub user_id: String,

    /// Transport configuration.
    pub transport: TransportConfig,

    /// DORS (Dynamic Offline Relay Switch) configuration.
    pub dors: DorsConfig,

    /// Relay management configuration.
    pub relay: RelayConfig,

    /// Path selection configuration.
    pub path: PathConfig,

    /// Reliability layer configuration.
    pub reliability: ReliabilityConfig,

    /// Initial TTL (Time-To-Live) for messages.
    pub initial_ttl: u8,
}

impl ProtocolConfig {
    /// Creates a new protocol configuration.
    ///
    /// # Arguments
    ///
    /// * `app_id` - Application identifier
    /// * `user_id` - User identifier
    pub fn new(app_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            user_id: user_id.into(),
            transport: TransportConfig::default(),
            dors: DorsConfig::default(),
            relay: RelayConfig::default(),
            path: PathConfig::default(),
            reliability: ReliabilityConfig::default(),
            initial_ttl: 8, // Default from spec
        }
    }

    /// Creates a builder for more granular configuration.
    pub fn builder(app_id: impl Into<String>, user_id: impl Into<String>) -> ProtocolConfigBuilder {
        ProtocolConfigBuilder::new(app_id, user_id)
    }

    /// Validates the configuration.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if valid, `Err` with a description of the problem if invalid.
    pub fn validate(&self) -> crate::Result<()> {
        if self.app_id.is_empty() {
            return Err(crate::Error::InvalidConfiguration(
                "app_id cannot be empty".to_string(),
            ));
        }

        if self.user_id.is_empty() {
            return Err(crate::Error::InvalidConfiguration(
                "user_id cannot be empty".to_string(),
            ));
        }

        if self.initial_ttl == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "initial_ttl must be greater than 0".to_string(),
            ));
        }

        if !self.transport.ble_enabled
            && !self.transport.wifi_direct_enabled
            && !self.transport.internet_enabled
        {
            return Err(crate::Error::InvalidConfiguration(
                "At least one transport must be enabled".to_string(),
            ));
        }

        Ok(())
    }
}

/// Builder for ProtocolConfig with a fluent API.
pub struct ProtocolConfigBuilder {
    config: ProtocolConfig,
}

impl ProtocolConfigBuilder {
    /// Creates a new builder.
    pub fn new(app_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            config: ProtocolConfig::new(app_id, user_id),
        }
    }

    /// Configures transports.
    pub fn transport(mut self, config: TransportConfig) -> Self {
        self.config.transport = config;
        self
    }

    /// Enables or disables BLE transport.
    pub fn ble_enabled(mut self, enabled: bool) -> Self {
        self.config.transport.ble_enabled = enabled;
        self
    }

    /// Enables or disables Wi-Fi Direct transport.
    pub fn wifi_direct_enabled(mut self, enabled: bool) -> Self {
        self.config.transport.wifi_direct_enabled = enabled;
        self
    }

    /// Enables or disables Internet transport.
    pub fn internet_enabled(mut self, enabled: bool) -> Self {
        self.config.transport.internet_enabled = enabled;
        self
    }

    /// Configures DORS (Dynamic Offline Relay Switch).
    pub fn dors(mut self, config: DorsConfig) -> Self {
        self.config.dors = config;
        self
    }

    /// Enables online-first mode (prefer Internet when available).
    pub fn online_first(mut self, enabled: bool) -> Self {
        self.config.dors.prefer_online = enabled;
        self
    }

    /// Configures relay management.
    pub fn relay(mut self, config: RelayConfig) -> Self {
        self.config.relay = config;
        self
    }

    /// Configures path selection.
    pub fn path(mut self, config: PathConfig) -> Self {
        self.config.path = config;
        self
    }

    /// Configures reliability layer.
    pub fn reliability(mut self, config: ReliabilityConfig) -> Self {
        self.config.reliability = config;
        self
    }

    /// Sets the initial TTL for messages.
    pub fn initial_ttl(mut self, ttl: u8) -> Self {
        self.config.initial_ttl = ttl;
        self
    }

    /// Builds and validates the configuration.
    ///
    /// # Returns
    ///
    /// Returns `Ok(ProtocolConfig)` if valid, `Err` otherwise.
    pub fn build(self) -> crate::Result<ProtocolConfig> {
        self.config.validate()?;
        Ok(self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_creation() {
        let config = ProtocolConfig::new("test-app", "user123");
        assert_eq!(config.app_id, "test-app");
        assert_eq!(config.user_id, "user123");
        assert_eq!(config.initial_ttl, 8);
    }

    #[test]
    fn test_config_validation_success() {
        let config = ProtocolConfig::new("test-app", "user123");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_app_id() {
        let config = ProtocolConfig::new("", "user123");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_empty_user_id() {
        let config = ProtocolConfig::new("test-app", "");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_zero_ttl() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.initial_ttl = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_no_transports() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.transport.ble_enabled = false;
        config.transport.wifi_direct_enabled = false;
        config.transport.internet_enabled = false;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_builder() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .ble_enabled(true)
            .wifi_direct_enabled(false)
            .online_first(true)
            .initial_ttl(10)
            .build()
            .unwrap();

        assert_eq!(config.app_id, "test-app");
        assert!(config.transport.ble_enabled);
        assert!(!config.transport.wifi_direct_enabled);
        assert!(config.dors.prefer_online);
        assert_eq!(config.initial_ttl, 10);
    }

    #[test]
    fn test_transport_config_default() {
        let transport = TransportConfig::default();
        assert!(transport.ble_enabled);
        assert!(transport.wifi_direct_enabled);
        assert!(transport.internet_enabled);
    }

    #[test]
    fn test_reliability_config_default() {
        let reliability = ReliabilityConfig::default();
        assert_eq!(reliability.ack.default_timeout_ms, 5000);
        assert_eq!(reliability.retry.max_retries, 3);
        assert_eq!(reliability.dedup.max_tracked_messages, 10000);
    }
}
