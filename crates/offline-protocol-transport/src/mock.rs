//! Mock transport for testing

use crate::{
    LinkQuality, Neighbor, NeighborRole, Result, Transport, TransportEvent, TransportMetrics,
    TransportType,
};
use async_trait::async_trait;
use offline_protocol_core::{DeviceId, MessageEnvelope, UserId};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Configuration for mock transport
#[derive(Debug, Clone)]
pub struct MockTransportConfig {
    pub device_id: DeviceId,
    pub user_id: UserId,
    pub latency_ms: u64,
    pub packet_loss_rate: f64,
}

/// Mock transport implementation for testing
pub struct MockTransport {
    config: MockTransportConfig,
    running: Arc<RwLock<bool>>,
    paused: Arc<RwLock<bool>>,
    neighbors: Arc<RwLock<HashMap<DeviceId, Neighbor>>>,
    metrics: Arc<RwLock<TransportMetrics>>,
    event_tx: mpsc::UnboundedSender<TransportEvent>,
    event_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<TransportEvent>>>>,
}

impl MockTransport {
    pub fn new(config: MockTransportConfig) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        
        Self {
            config,
            running: Arc::new(RwLock::new(false)),
            paused: Arc::new(RwLock::new(false)),
            neighbors: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(RwLock::new(TransportMetrics::new(TransportType::Mock))),
            event_tx,
            event_rx: Arc::new(RwLock::new(Some(event_rx))),
        }
    }

    /// Add a mock neighbor for testing
    pub fn add_neighbor(&mut self, device_id: DeviceId, user_id: UserId, role: NeighborRole) {
        let mut neighbors = self.neighbors.write();
        let neighbor = Neighbor::new(device_id, user_id, role);
        neighbors.insert(device_id, neighbor.clone());
        let _ = self.event_tx.send(TransportEvent::NeighborDiscovered(neighbor));
    }

    /// Remove a mock neighbor
    pub fn remove_neighbor(&mut self, device_id: DeviceId) {
        let mut neighbors = self.neighbors.write();
        if neighbors.remove(&device_id).is_some() {
            let _ = self.event_tx.send(TransportEvent::NeighborLost(device_id));
        }
    }

    /// Simulate receiving a message
    pub fn simulate_receive(&self, envelope: MessageEnvelope) {
        let mut metrics = self.metrics.write();
        metrics.messages_received += 1;
        let _ = self.event_tx.send(TransportEvent::MessageReceived(envelope));
    }
}

#[async_trait]
impl Transport for MockTransport {
    async fn start(&mut self) -> Result<()> {
        let mut running = self.running.write();
        if *running {
            return Err(crate::Error::AlreadyStarted);
        }
        *running = true;
        let _ = self.event_tx.send(TransportEvent::Started);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        let mut running = self.running.write();
        *running = false;
        let _ = self.event_tx.send(TransportEvent::Stopped);
        Ok(())
    }

    async fn pause(&mut self) -> Result<()> {
        let mut paused = self.paused.write();
        *paused = true;
        Ok(())
    }

    async fn resume(&mut self) -> Result<()> {
        let mut paused = self.paused.write();
        *paused = false;
        Ok(())
    }

    async fn send(&mut self, device_id: DeviceId, _message: &MessageEnvelope) -> Result<()> {
        if !*self.running.read() {
            return Err(crate::Error::NotStarted);
        }

        // Check if neighbor exists
        {
            let neighbors = self.neighbors.read();
            if !neighbors.contains_key(&device_id) {
                return Err(crate::Error::NeighborNotFound(device_id.to_string()));
            }
        }

        // Simulate latency
        if self.config.latency_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.config.latency_ms)).await;
        }

        // Simulate packet loss
        if rand::random::<f64>() < self.config.packet_loss_rate {
            return Err(crate::Error::SendFailed("Simulated packet loss".to_string()));
        }

        // Update metrics
        let mut metrics = self.metrics.write();
        metrics.messages_sent += 1;

        Ok(())
    }

    async fn broadcast(&mut self, message: &MessageEnvelope) -> Result<()> {
        let neighbors: Vec<DeviceId> = self.neighbors.read().keys().copied().collect();
        
        for device_id in neighbors {
            // Ignore send errors in broadcast
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

// Add rand dependency for packet loss simulation
use rand::Rng;

