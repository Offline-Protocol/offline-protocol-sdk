//! Main OfflineProtocol SDK implementation

use crate::{
    config::OfflineProtocolConfig,
    error::{Error, Result},
    events::*,
    file_transfer::{FileTransferManager, ProgressCallback},
};
use crossbeam_channel::{bounded, Receiver, Sender};
use offline_protocol_core::{
    DeviceId, Message, MessageEnvelope, MessageId, Priority, TextMessage, UserId,
};
use offline_protocol_reliability::{
    ack_manager::{AckManager, AckManagerConfig},
    deduplicator::{Deduplicator, DeduplicatorConfig},
    retry_queue::{RetryQueue, RetryQueueConfig, RetryStrategy},
};
use offline_protocol_router::{
    dors::DorsConfig as RouterDorsConfig,
    dors::DorsEngine,
    relay::{RelayManager, RelayManagerConfig, RelayPriority},
    router::{Router, RouterConfig},
};
use offline_protocol_transport::{
    ble::{BleTransport, BleTransportConfig},
    Transport, TransportType,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Main Offline Protocol SDK struct
pub struct OfflineProtocol {
    config: OfflineProtocolConfig,
    device_id: DeviceId,
    user_id: UserId,
    running: Arc<RwLock<bool>>,
    paused: Arc<RwLock<bool>>,
    
    // Core components
    router: Arc<Router>,
    relay_manager: Arc<RelayManager>,
    _dors_engine: Arc<DorsEngine>,
    ack_manager: Arc<AckManager>,
    retry_queue: Arc<RetryQueue>,
    _deduplicator: Arc<Deduplicator>,
    file_manager: Arc<FileTransferManager>,
    
    // Event handling
    _event_tx: Sender<Event>,
    event_rx: Arc<RwLock<Option<Receiver<Event>>>>,
    
    // Background tasks
    tasks: Arc<RwLock<Vec<JoinHandle<()>>>>,
}

impl OfflineProtocol {
    /// Create a new OfflineProtocol instance
    pub fn new(config: OfflineProtocolConfig) -> Result<Self> {
        let device_id = DeviceId::new();
        let user_id = UserId::new(config.username.clone());

        // Create event channel
        let (event_tx, event_rx) = bounded(1000);

        // Create reliability components
        let ack_manager = Arc::new(AckManager::new(AckManagerConfig {
            ack_timeout: Duration::from_millis(config.reliability.ack_timeout),
        }));

        let retry_queue = Arc::new(RetryQueue::new(RetryQueueConfig {
            retry_strategy: RetryStrategy {
                max_retries: config.reliability.max_retries,
                initial_delay: Duration::from_secs(1),
                max_delay: Duration::from_secs(60),
                multiplier: 2.0,
            },
            max_queue_size: 1000,
            message_lifetime: Duration::from_millis(config.reliability.outbox_max_lifetime),
        }));

        let deduplicator = Arc::new(Deduplicator::new(DeduplicatorConfig::default()));

        // Create DORS engine
        let dors_engine = Arc::new(DorsEngine::new(RouterDorsConfig {
            auto_switch: config.dors.auto_switch,
            switch_hysteresis_secs: config.dors.switch_hysteresis,
            switch_cooldown_secs: config.dors.switch_cooldown,
            ble_to_wifi_retry_threshold: config.dors.ble_to_wifi_retry_threshold,
            rssi_switch_threshold: config.dors.rssi_switch_threshold,
            delivery_ratio_threshold: 0.5,
        }));

        // Create relay manager
        let relay_manager = Arc::new(RelayManager::new(RelayManagerConfig {
            allow_act_as_relay: config.relay.allow_act_as_relay,
            relay_priority: RelayPriority::from(config.relay.relay_priority.as_str()),
            relay_threshold: config.network.relay_threshold,
            min_battery_for_relay: config.relay.min_battery_for_relay,
        }));

        // Create router
        let router = Arc::new(Router::new(
            RouterConfig {
                initial_ttl: config.network.initial_ttl,
                enable_dors: config.network.enable_dors,
            },
            device_id,
            Some(Arc::clone(&dors_engine)),
            Arc::clone(&relay_manager),
        ));

        // Create file transfer manager
        let file_manager = Arc::new(FileTransferManager::new(512)); // 512 byte chunks for BLE

        Ok(Self {
            config,
            device_id,
            user_id,
            running: Arc::new(RwLock::new(false)),
            paused: Arc::new(RwLock::new(false)),
            router,
            relay_manager,
            _dors_engine: dors_engine,
            ack_manager,
            retry_queue,
            _deduplicator: deduplicator,
            file_manager,
            _event_tx: event_tx,
            event_rx: Arc::new(RwLock::new(Some(event_rx))),
            tasks: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Start the protocol
    pub async fn start(&mut self) -> Result<()> {
        let mut running = self.running.write();
        if *running {
            return Err(Error::AlreadyStarted);
        }

        info!("Starting Offline Protocol SDK");
        info!("Device ID: {}", self.device_id);
        info!("Username: {}", self.user_id);

        // Initialize transports
        if self.config.transports.ble.enabled {
            self.start_ble_transport().await?;
        }

        if self.config.transports.wifi_direct.enabled {
            info!("Wi-Fi Direct transport enabled but not yet implemented");
            // TODO: Start Wi-Fi Direct transport
        }

        *running = true;

        // Start background tasks
        self.start_background_tasks();

        info!("Offline Protocol SDK started successfully");
        Ok(())
    }

    /// Stop the protocol
    pub async fn stop(&mut self) -> Result<()> {
        let mut running = self.running.write();
        if !*running {
            return Ok(());
        }

        info!("Stopping Offline Protocol SDK");
        *running = false;

        // Cancel all background tasks
        let mut tasks = self.tasks.write();
        for task in tasks.drain(..) {
            task.abort();
        }

        // TODO: Stop all transports

        info!("Offline Protocol SDK stopped");
        Ok(())
    }

    /// Pause operations (background mode)
    pub async fn pause(&mut self) -> Result<()> {
        *self.paused.write() = true;
        info!("Protocol paused");
        // TODO: Pause transports
        Ok(())
    }

    /// Resume operations
    pub async fn resume(&mut self) -> Result<()> {
        *self.paused.write() = false;
        info!("Protocol resumed");
        // TODO: Resume transports
        Ok(())
    }

    /// Cleanup resources
    pub async fn cleanup(&mut self) -> Result<()> {
        self.stop().await?;
        // TODO: Cleanup persistent storage, etc.
        Ok(())
    }

    /// Send a text message
    pub async fn send_message(
        &self,
        recipient: UserId,
        text: String,
        priority: Priority,
        metadata: HashMap<String, String>,
    ) -> Result<MessageId> {
        let message = Message::Text(TextMessage { text, metadata });

        let mut envelope = MessageEnvelope::new(
            self.device_id,
            self.user_id.clone(),
            Some(recipient),
            message,
            priority,
            self.config.network.initial_ttl,
        );

        let message_id = envelope.message_id;

        // Register for ACK
        let _ack_rx = self.ack_manager.register(message_id);

        // Send via router
        self.router.route(&mut envelope).await?;

        // Add to retry queue
        self.retry_queue.enqueue(envelope)?;

        Ok(message_id)
    }

    /// Send a file
    pub async fn send_file(
        &self,
        recipient: UserId,
        name: String,
        data: Vec<u8>,
        mime_type: String,
        priority: Priority,
        progress_callback: Option<ProgressCallback>,
    ) -> Result<MessageId> {
        let file_id = MessageId::new();

        // Fragment the file
        let transfer = self.file_manager.fragment_file(
            file_id,
            name.clone(),
            data,
            mime_type,
            progress_callback,
        );

        // Send file metadata first
        let metadata_message = Message::File(transfer.metadata.clone());
        let mut metadata_envelope = MessageEnvelope::new(
            self.device_id,
            self.user_id.clone(),
            Some(recipient.clone()),
            metadata_message,
            priority,
            self.config.network.initial_ttl,
        );

        self.router.route(&mut metadata_envelope).await?;

        // Register the file transfer
        self.file_manager.register_sending(transfer);

        // TODO: Send chunks (this would be done in a background task)

        Ok(file_id)
    }

    /// Get an event receiver
    pub fn event_receiver(&self) -> Receiver<Event> {
        self.event_rx
            .write()
            .take()
            .expect("Event receiver already taken")
    }

    /// Check permissions
    pub async fn check_permissions(&self) -> HashMap<String, bool> {
        // TODO: Implement platform-specific permission checks
        let mut permissions = HashMap::new();
        permissions.insert("bluetooth".to_string(), true);
        permissions.insert("location".to_string(), true);
        permissions.insert("wifiDirect".to_string(), false);
        permissions.insert("notifications".to_string(), true);
        permissions
    }

    /// Request a specific permission
    pub async fn request_permission(&self, permission: &str) -> Result<bool> {
        // TODO: Implement platform-specific permission requests
        warn!("Permission request not implemented: {}", permission);
        Ok(false)
    }

    /// Start BLE transport
    async fn start_ble_transport(&self) -> Result<()> {
        let ble_config = BleTransportConfig {
            device_id: self.device_id,
            user_id: self.user_id.clone(),
            scan_interval_ms: self.config.transports.ble.scan_interval_ms,
            beacon_interval_ms: self.config.transports.ble.advertising_interval_ms,
            is_relay: self.relay_manager.is_relay(),
        };

        let mut ble_transport = BleTransport::new(ble_config).await?;
        ble_transport.start().await?;

        // Register with router
        self.router.register_transport(
            TransportType::BLE,
            Box::new(ble_transport),
        );

        info!("BLE transport started");
        Ok(())
    }

    /// Start background tasks
    fn start_background_tasks(&self) {
        // TODO: Start tasks for:
        // - Processing received messages
        // - Retry queue processing
        // - Metrics collection
        // - Relay status monitoring
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_protocol() {
        let config = OfflineProtocolConfig {
            app_id: "test-app".to_string(),
            username: "test-user".to_string(),
            transports: Default::default(),
            network: Default::default(),
            dors: Default::default(),
            relay: Default::default(),
            reliability: Default::default(),
        };

        let protocol = OfflineProtocol::new(config);
        assert!(protocol.is_ok());
    }
}

