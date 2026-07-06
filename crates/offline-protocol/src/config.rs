//! Protocol configuration.

use crate::constants::DEFAULT_INITIAL_TTL;
use offline_protocol_reliability::{AckConfig, DeduplicatorConfig, RetryConfig};
use offline_protocol_router::{DorsConfig, PathConfig, RelayConfig};
use serde::{Deserialize, Serialize};

/// Overflow policy for bounded pending encrypted message queues.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverflowPolicy {
    /// Evict the oldest queued message to make room for the new message.
    #[default]
    DropOldest,
    /// Drop the newly received message when capacity is reached.
    DropNewest,
}

/// Configuration for inbound encrypted messages received before session readiness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingQueueConfig {
    /// Maximum number of pending encrypted messages per peer.
    pub max_pending_per_peer: usize,
    /// Maximum number of pending encrypted messages across all peers.
    pub max_pending_global: usize,
    /// Maximum total payload bytes (content plus binary content) queued per
    /// peer.
    ///
    /// Encrypted media chunks are queued whole while the sender's session is
    /// not yet ready, so the count limits alone would admit up to
    /// `max_pending_per_peer` × chunk-size bytes of ciphertext. The default
    /// (4 MB) covers the largest legitimate pre-session backlog: 2 concurrent
    /// transfers × 8 in-flight chunks × 256 KB internet chunks.
    #[serde(default = "default_max_pending_bytes_per_peer")]
    pub max_pending_bytes_per_peer: usize,
    /// Maximum total payload bytes queued across all peers. The default
    /// (32 MB) bounds worst-case queue memory on mobile devices regardless of
    /// how many peers are mid-establishment.
    #[serde(default = "default_max_pending_bytes_global")]
    pub max_pending_bytes_global: usize,
    /// Time-to-live for pending encrypted messages in milliseconds.
    pub pending_ttl_ms: u64,
    /// Overflow policy when queue limits are reached.
    pub overflow_policy: OverflowPolicy,
}

fn default_max_pending_bytes_per_peer() -> usize {
    4 * 1024 * 1024
}

fn default_max_pending_bytes_global() -> usize {
    32 * 1024 * 1024
}

impl Default for PendingQueueConfig {
    fn default() -> Self {
        Self {
            max_pending_per_peer: 64,
            max_pending_global: 4096,
            max_pending_bytes_per_peer: default_max_pending_bytes_per_peer(),
            max_pending_bytes_global: default_max_pending_bytes_global(),
            pending_ttl_ms: 120_000,
            overflow_policy: OverflowPolicy::DropOldest,
        }
    }
}

/// Encryption configuration for automatic MLS handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptionConfig {
    /// Whether automatic encryption is enabled.
    /// When enabled, messages are automatically encrypted/decrypted using MLS.
    pub enabled: bool,

    /// Auto-exchange key packages on peer discovery.
    /// When enabled, key packages are automatically sent when neighbors are discovered.
    pub auto_key_exchange: bool,

    /// Store pending messages when no session exists.
    /// Messages will be sent automatically after the session is established.
    pub store_pending: bool,

    /// Require encryption for outbound messages.
    /// When enabled, sends fail if encryption cannot be applied.
    pub require_encryption: bool,

    /// Bounds and eviction policy for pending inbound encrypted messages.
    pub pending_queue: PendingQueueConfig,
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_key_exchange: true,
            store_pending: true,
            require_encryption: false,
            pending_queue: PendingQueueConfig::default(),
        }
    }
}

impl EncryptionConfig {
    /// Creates a new encryption config with encryption disabled.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            auto_key_exchange: false,
            store_pending: false,
            require_encryption: false,
            pending_queue: PendingQueueConfig::default(),
        }
    }
}

/// Security configuration for transport and control-message hardening.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// When `true`, control messages with no transport-level peer identity
    /// (`transport_peer_id` is `None`) are rejected. When `false` (default),
    /// missing transport identity emits a `SecurityWarning` but allows the
    /// message through (best-effort / fail-open).
    ///
    /// # Security implications of the default (`false`)
    ///
    /// With fail-open behavior, an attacker who can inject messages into the
    /// transport layer (e.g., by compromising a relay server or being in BLE
    /// range) can send spoofed control messages with a forged `sender` field.
    /// Ed25519 control-message signing (TOFU) mitigates this for peers whose
    /// keys have already been pinned, but the first contact with any peer is
    /// vulnerable to man-in-the-middle if the transport identity is absent.
    ///
    /// Set to `true` in production deployments where all transports reliably
    /// provide peer identity (authenticated WebSocket, bonded BLE devices).
    /// Leave as `false` during development or when using relay-based mesh
    /// routing where forwarded messages legitimately lack transport identity.
    pub require_transport_identity: bool,
}

/// Transport-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// Whether BLE transport is enabled.
    pub ble_enabled: bool,

    /// Whether Wi-Fi Direct transport is enabled.
    pub wifi_direct_enabled: bool,

    /// Whether Internet transport is enabled.
    pub internet_enabled: bool,

    /// Whether Reticulum mesh transport is enabled.
    /// Defaults to `false` because Reticulum requires external infrastructure
    /// (a running Reticulum daemon or RNode hardware).
    pub reticulum_enabled: bool,

    /// Whether Nostr relay transport is enabled.
    /// Defaults to `false` because Nostr requires relay configuration
    /// and a secp256k1 keypair.
    pub nostr_enabled: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            ble_enabled: true,
            wifi_direct_enabled: true,
            internet_enabled: true,
            reticulum_enabled: false,
            nostr_enabled: false,
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

/// Configuration for group messaging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfig {
    /// Maximum number of members allowed in a single group.
    ///
    /// Per-member fan-out is O(N) when using mesh transports.
    /// This cap prevents unbounded groups from overwhelming the network.
    pub max_group_members: usize,

    /// Whether to attempt relay server registration for groups.
    ///
    /// When `true` (default), groups are registered with the relay server
    /// for optimized fan-out when Internet transport is available.
    /// When `false`, groups always use per-member fan-out regardless of
    /// transport.
    pub relay_enabled: bool,
}

impl Default for GroupConfig {
    fn default() -> Self {
        Self {
            max_group_members: 256,
            relay_enabled: true,
        }
    }
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

    /// Encryption configuration for automatic MLS handling.
    pub encryption: EncryptionConfig,

    /// Initial TTL (Time-To-Live) for messages.
    pub initial_ttl: u8,

    /// Mesh group messaging configuration.
    pub group: GroupConfig,

    /// Security configuration for transport and control-message hardening.
    pub security: SecurityConfig,
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
            encryption: EncryptionConfig::default(),
            initial_ttl: DEFAULT_INITIAL_TTL,
            group: GroupConfig::default(),
            security: SecurityConfig::default(),
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
            && !self.transport.reticulum_enabled
            && !self.transport.nostr_enabled
        {
            return Err(crate::Error::InvalidConfiguration(
                "At least one transport must be enabled".to_string(),
            ));
        }

        if self.reliability.retry.initial_delay_ms == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "retry.initial_delay_ms must be greater than 0".to_string(),
            ));
        }

        if self.reliability.retry.max_delay_ms == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "retry.max_delay_ms must be greater than 0".to_string(),
            ));
        }

        if self.reliability.retry.initial_delay_ms > self.reliability.retry.max_delay_ms {
            return Err(crate::Error::InvalidConfiguration(
                "retry.initial_delay_ms must be <= retry.max_delay_ms".to_string(),
            ));
        }

        if !self.reliability.retry.backoff_multiplier.is_finite()
            || self.reliability.retry.backoff_multiplier < 1.0
        {
            return Err(crate::Error::InvalidConfiguration(
                "retry.backoff_multiplier must be finite and >= 1.0".to_string(),
            ));
        }

        if self.encryption.require_encryption && !self.encryption.enabled {
            return Err(crate::Error::InvalidConfiguration(
                "encryption.require_encryption requires encryption.enabled=true".to_string(),
            ));
        }

        if self.encryption.pending_queue.max_pending_per_peer == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "encryption.pending_queue.max_pending_per_peer must be greater than 0".to_string(),
            ));
        }

        if self.encryption.pending_queue.max_pending_global == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "encryption.pending_queue.max_pending_global must be greater than 0".to_string(),
            ));
        }

        if self.encryption.pending_queue.max_pending_global
            < self.encryption.pending_queue.max_pending_per_peer
        {
            return Err(crate::Error::InvalidConfiguration(
                "encryption.pending_queue.max_pending_global must be >= max_pending_per_peer"
                    .to_string(),
            ));
        }

        if self.encryption.pending_queue.pending_ttl_ms == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "encryption.pending_queue.pending_ttl_ms must be greater than 0".to_string(),
            ));
        }

        if self.group.max_group_members == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "group.max_group_members must be greater than 0".to_string(),
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

    /// Enables or disables Nostr relay transport.
    pub fn nostr_enabled(mut self, enabled: bool) -> Self {
        self.config.transport.nostr_enabled = enabled;
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

    /// Configures encryption settings.
    pub fn encryption(mut self, config: EncryptionConfig) -> Self {
        self.config.encryption = config;
        self
    }

    /// Enables or disables automatic encryption.
    pub fn encryption_enabled(mut self, enabled: bool) -> Self {
        self.config.encryption.enabled = enabled;
        self
    }

    /// Enables or disables automatic key exchange on peer discovery.
    pub fn auto_key_exchange(mut self, enabled: bool) -> Self {
        self.config.encryption.auto_key_exchange = enabled;
        self
    }

    /// Enables or disables storing pending messages for later encryption.
    pub fn store_pending_messages(mut self, enabled: bool) -> Self {
        self.config.encryption.store_pending = enabled;
        self
    }

    /// Enables or disables strict encryption requirement.
    pub fn require_encryption(mut self, required: bool) -> Self {
        self.config.encryption.require_encryption = required;
        self
    }

    /// Configures pending encrypted message queue bounds and overflow behavior.
    pub fn pending_queue(mut self, config: PendingQueueConfig) -> Self {
        self.config.encryption.pending_queue = config;
        self
    }

    /// Configures group messaging settings.
    pub fn group(mut self, config: GroupConfig) -> Self {
        self.config.group = config;
        self
    }

    /// Sets the maximum number of members allowed in a single group.
    pub fn max_group_members(mut self, max: usize) -> Self {
        self.config.group.max_group_members = max;
        self
    }

    /// Sets whether groups should attempt relay server registration.
    pub fn group_relay_enabled(mut self, enabled: bool) -> Self {
        self.config.group.relay_enabled = enabled;
        self
    }

    /// Configures security settings.
    pub fn security(mut self, config: SecurityConfig) -> Self {
        self.config.security = config;
        self
    }

    /// Sets whether transport identity is required for control messages.
    pub fn require_transport_identity(mut self, required: bool) -> Self {
        self.config.security.require_transport_identity = required;
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
    fn test_config_validation_retry_initial_delay_must_be_positive() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.reliability.retry.initial_delay_ms = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_retry_delay_bounds() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.reliability.retry.initial_delay_ms = 2000;
        config.reliability.retry.max_delay_ms = 1000;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_retry_backoff_multiplier_bounds() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.reliability.retry.backoff_multiplier = 0.5;
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
        assert_eq!(reliability.ack.default_timeout_ms, 10000);
        assert_eq!(reliability.retry.max_retries, 10);
        assert_eq!(reliability.dedup.max_tracked_messages, 1000);
    }

    #[test]
    fn test_encryption_config_default() {
        let encryption = EncryptionConfig::default();
        assert!(encryption.enabled);
        assert!(encryption.auto_key_exchange);
        assert!(encryption.store_pending);
        assert!(!encryption.require_encryption);
        assert_eq!(encryption.pending_queue.max_pending_per_peer, 64);
        assert_eq!(encryption.pending_queue.max_pending_global, 4096);
        assert_eq!(encryption.pending_queue.pending_ttl_ms, 120_000);
        assert_eq!(
            encryption.pending_queue.overflow_policy,
            OverflowPolicy::DropOldest
        );
    }

    #[test]
    fn test_encryption_config_disabled() {
        let encryption = EncryptionConfig::disabled();
        assert!(!encryption.enabled);
        assert!(!encryption.auto_key_exchange);
        assert!(!encryption.store_pending);
        assert!(!encryption.require_encryption);
    }

    #[test]
    fn test_config_builder_with_encryption() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .encryption_enabled(true)
            .auto_key_exchange(true)
            .store_pending_messages(false)
            .require_encryption(true)
            .pending_queue(PendingQueueConfig {
                max_pending_per_peer: 32,
                max_pending_global: 512,
                pending_ttl_ms: 30_000,
                overflow_policy: OverflowPolicy::DropNewest,
                ..Default::default()
            })
            .build()
            .unwrap();

        assert!(config.encryption.enabled);
        assert!(config.encryption.auto_key_exchange);
        assert!(!config.encryption.store_pending);
        assert!(config.encryption.require_encryption);
        assert_eq!(config.encryption.pending_queue.max_pending_per_peer, 32);
        assert_eq!(config.encryption.pending_queue.max_pending_global, 512);
        assert_eq!(config.encryption.pending_queue.pending_ttl_ms, 30_000);
        assert_eq!(
            config.encryption.pending_queue.overflow_policy,
            OverflowPolicy::DropNewest
        );
    }

    #[test]
    fn test_config_validation_require_encryption_requires_enabled() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.encryption.enabled = false;
        config.encryption.require_encryption = true;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_pending_queue_requires_positive_limits() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.encryption.pending_queue.max_pending_per_peer = 0;
        assert!(config.validate().is_err());

        let mut config = ProtocolConfig::new("test-app", "user123");
        config.encryption.pending_queue.max_pending_global = 0;
        assert!(config.validate().is_err());

        let mut config = ProtocolConfig::new("test-app", "user123");
        config.encryption.pending_queue.pending_ttl_ms = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_pending_queue_global_must_cover_per_peer() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.encryption.pending_queue.max_pending_per_peer = 100;
        config.encryption.pending_queue.max_pending_global = 99;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_group_config_default() {
        let group = GroupConfig::default();
        assert_eq!(group.max_group_members, 256);
    }

    #[test]
    fn test_config_has_default_group_config() {
        let config = ProtocolConfig::new("test-app", "user123");
        assert_eq!(config.group.max_group_members, 256);
    }

    #[test]
    fn test_config_validation_zero_max_group_members() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.group.max_group_members = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_positive_max_group_members() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.group.max_group_members = 1;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_builder_max_group_members() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .max_group_members(50)
            .build()
            .unwrap();
        assert_eq!(config.group.max_group_members, 50);
    }

    #[test]
    fn test_config_builder_group_config() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .group(GroupConfig {
                max_group_members: 128,
                ..Default::default()
            })
            .build()
            .unwrap();
        assert_eq!(config.group.max_group_members, 128);
    }

    #[test]
    fn test_config_builder_zero_max_group_members_rejected() {
        let result = ProtocolConfig::builder("test-app", "user123")
            .max_group_members(0)
            .build();
        assert!(result.is_err());
    }
}
