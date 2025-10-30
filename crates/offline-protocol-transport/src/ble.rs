//! BLE mesh transport implementation

use crate::{
    LinkQuality, Neighbor, NeighborRole, Result, Transport, TransportEvent, TransportMetrics,
    TransportType,
};
use async_trait::async_trait;
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use offline_protocol_core::{ControlMessage, DeviceId, Message, MessageEnvelope, UserId};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Custom service UUID for offline protocol
const SERVICE_UUID: Uuid = Uuid::from_u128(0x0000FE00_0000_1000_8000_00805F9B34FB);
const MESSAGE_TX_CHAR_UUID: Uuid = Uuid::from_u128(0x0000FE01_0000_1000_8000_00805F9B34FB);
const MESSAGE_RX_CHAR_UUID: Uuid = Uuid::from_u128(0x0000FE02_0000_1000_8000_00805F9B34FB);
const BEACON_CHAR_UUID: Uuid = Uuid::from_u128(0x0000FE03_0000_1000_8000_00805F9B34FB);

const NEIGHBOR_TIMEOUT: Duration = Duration::from_secs(30);
const BEACON_INTERVAL: Duration = Duration::from_secs(5);
const SCAN_INTERVAL: Duration = Duration::from_secs(5);

/// Configuration for BLE transport
#[derive(Debug, Clone)]
pub struct BleTransportConfig {
    pub device_id: DeviceId,
    pub user_id: UserId,
    pub scan_interval_ms: u64,
    pub beacon_interval_ms: u64,
    pub is_relay: bool,
}

impl Default for BleTransportConfig {
    fn default() -> Self {
        Self {
            device_id: DeviceId::new(),
            user_id: UserId::new("unknown"),
            scan_interval_ms: 5000,
            beacon_interval_ms: 5000,
            is_relay: false,
        }
    }
}

/// BLE mesh transport implementation
pub struct BleTransport {
    config: BleTransportConfig,
    manager: Manager,
    adapter: Option<Adapter>,
    running: Arc<RwLock<bool>>,
    paused: Arc<RwLock<bool>>,
    neighbors: Arc<RwLock<HashMap<DeviceId, Neighbor>>>,
    peripherals: Arc<RwLock<HashMap<DeviceId, Peripheral>>>,
    metrics: Arc<RwLock<TransportMetrics>>,
    event_tx: mpsc::UnboundedSender<TransportEvent>,
    event_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<TransportEvent>>>>,
}

impl BleTransport {
    pub async fn new(config: BleTransportConfig) -> Result<Self> {
        let manager = Manager::new().await?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Ok(Self {
            config,
            manager,
            adapter: None,
            running: Arc::new(RwLock::new(false)),
            paused: Arc::new(RwLock::new(false)),
            neighbors: Arc::new(RwLock::new(HashMap::new())),
            peripherals: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(RwLock::new(TransportMetrics::new(TransportType::BLE))),
            event_tx,
            event_rx: Arc::new(RwLock::new(Some(event_rx))),
        })
    }

    /// Start beacon broadcasting
    async fn start_beacon_broadcast(&self) {
        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let running = Arc::clone(&self.running);
        let paused = Arc::clone(&self.paused);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(config.beacon_interval_ms));
            
            while *running.read() {
                interval.tick().await;
                
                if *paused.read() {
                    continue;
                }

                // Create beacon message
                let beacon = MessageEnvelope::new(
                    config.device_id,
                    config.user_id.clone(),
                    None, // Broadcast
                    Message::Control(ControlMessage::Beacon {
                        device_id: config.device_id,
                        username: config.user_id.clone(),
                        is_relay: config.is_relay,
                        connection_count: 0, // TODO: track actual connections
                    }),
                    offline_protocol_core::Priority::Low,
                    1, // Beacons don't propagate
                );

                debug!("Broadcasting beacon");
                // TODO: Actually broadcast via BLE advertising or GATT
            }
        });
    }

    /// Start scanning for neighbors
    async fn start_scanning(&mut self) -> Result<()> {
        let adapter = self.adapter.as_ref().ok_or(crate::Error::NotStarted)?;
        
        info!("Starting BLE scan");
        adapter.start_scan(ScanFilter::default()).await?;

        let adapter_clone = adapter.clone();
        let neighbors = Arc::clone(&self.neighbors);
        let peripherals = Arc::clone(&self.peripherals);
        let event_tx = self.event_tx.clone();
        let running = Arc::clone(&self.running);
        let paused = Arc::clone(&self.paused);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(SCAN_INTERVAL);
            
            while *running.read() {
                interval.tick().await;
                
                if *paused.read() {
                    continue;
                }

                if let Ok(discovered) = adapter_clone.peripherals().await {
                    for peripheral in discovered {
                        // Check if this is an offline protocol device
                        if let Ok(properties) = peripheral.properties().await {
                            if let Some(props) = properties {
                                if let Some(local_name) = props.local_name {
                                    if local_name.starts_with("OfflineProto-") {
                                        // Extract device ID from name
                                        // TODO: Proper parsing and validation
                                        debug!("Discovered offline protocol device: {}", local_name);
                                        
                                        // Connect and exchange info
                                        if let Err(e) = Self::handle_discovered_device(
                                            &peripheral,
                                            &neighbors,
                                            &peripherals,
                                            &event_tx,
                                        ).await {
                                            warn!("Failed to handle discovered device: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Clean up timed-out neighbors
                Self::cleanup_timed_out_neighbors(&neighbors, &event_tx);
            }
        });

        Ok(())
    }

    async fn handle_discovered_device(
        _peripheral: &Peripheral,
        _neighbors: &Arc<RwLock<HashMap<DeviceId, Neighbor>>>,
        _peripherals: &Arc<RwLock<HashMap<DeviceId, Peripheral>>>,
        _event_tx: &mpsc::UnboundedSender<TransportEvent>,
    ) -> Result<()> {
        // TODO: Connect to peripheral, read characteristics, exchange device info
        // For now, this is a placeholder
        Ok(())
    }

    fn cleanup_timed_out_neighbors(
        neighbors: &Arc<RwLock<HashMap<DeviceId, Neighbor>>>,
        event_tx: &mpsc::UnboundedSender<TransportEvent>,
    ) {
        let mut neighbors = neighbors.write();
        let mut timed_out = Vec::new();

        for (device_id, neighbor) in neighbors.iter() {
            if neighbor.is_timed_out(NEIGHBOR_TIMEOUT) {
                timed_out.push(*device_id);
            }
        }

        for device_id in timed_out {
            neighbors.remove(&device_id);
            let _ = event_tx.send(TransportEvent::NeighborLost(device_id));
            debug!("Neighbor timed out: {}", device_id);
        }
    }
}

#[async_trait]
impl Transport for BleTransport {
    async fn start(&mut self) -> Result<()> {
        {
            let mut running = self.running.write();
            if *running {
                return Err(crate::Error::AlreadyStarted);
            }
            *running = true;
        }

        // Get BLE adapter
        let adapters = self.manager.adapters().await?;
        let adapter = adapters.into_iter().next().ok_or_else(|| {
            crate::Error::Ble("No Bluetooth adapter found".to_string())
        })?;

        info!("Using Bluetooth adapter: {:?}", adapter.adapter_info().await?);
        self.adapter = Some(adapter);
        
        // Start beacon broadcasting
        self.start_beacon_broadcast().await;
        
        // Start scanning for neighbors
        self.start_scanning().await?;

        let _ = self.event_tx.send(TransportEvent::Started);
        info!("BLE transport started");
        
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        {
            let mut running = self.running.write();
            if !*running {
                return Ok(());
            }
            *running = false;
        }

        if let Some(adapter) = &self.adapter {
            adapter.stop_scan().await?;
        }

        let _ = self.event_tx.send(TransportEvent::Stopped);
        info!("BLE transport stopped");
        
        Ok(())
    }

    async fn pause(&mut self) -> Result<()> {
        let mut paused = self.paused.write();
        *paused = true;
        debug!("BLE transport paused");
        Ok(())
    }

    async fn resume(&mut self) -> Result<()> {
        let mut paused = self.paused.write();
        *paused = false;
        debug!("BLE transport resumed");
        Ok(())
    }

    async fn send(&mut self, device_id: DeviceId, message: &MessageEnvelope) -> Result<()> {
        if !*self.running.read() {
            return Err(crate::Error::NotStarted);
        }

        // Get the peripheral for this device
        let peripheral = {
            let peripherals = self.peripherals.read();
            peripherals.get(&device_id).cloned()
        };

        let peripheral = peripheral.ok_or_else(|| {
            crate::Error::NeighborNotFound(device_id.to_string())
        })?;

        // Serialize message
        let data = message.to_bytes()?;

        // TODO: Write to BLE characteristic
        // This is a placeholder - actual BLE write would happen here
        debug!("Sending {} bytes to {}", data.len(), device_id);

        let mut metrics = self.metrics.write();
        metrics.messages_sent += 1;

        Ok(())
    }

    async fn broadcast(&mut self, message: &MessageEnvelope) -> Result<()> {
        let neighbors: Vec<DeviceId> = self.neighbors.read().keys().copied().collect();
        
        for device_id in neighbors {
            // Ignore individual send errors in broadcast
            let _ = self.send(device_id, message).await;
        }
        
        Ok(())
    }

    async fn get_neighbors(&self) -> Vec<Neighbor> {
        self.neighbors.read().values().cloned().collect()
    }

    async fn get_link_quality(&self, device_id: DeviceId) -> Option<LinkQuality> {
        self.neighbors.read().get(&device_id).map(|n| n.link_quality)
    }

    async fn get_metrics(&self) -> TransportMetrics {
        let mut metrics = self.metrics.read().clone();
        metrics.neighbor_count = self.neighbors.read().len();
        metrics
    }

    fn event_channel(&self) -> mpsc::UnboundedReceiver<TransportEvent> {
        self.event_rx
            .write()
            .take()
            .expect("Event channel already taken")
    }

    fn is_running(&self) -> bool {
        *self.running.read()
    }

    fn is_paused(&self) -> bool {
        *self.paused.read()
    }
}

