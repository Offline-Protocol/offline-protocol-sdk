//! Main router implementation with multi-hop forwarding

use crate::{DorsEngine, RelayManager, Result};
use offline_protocol_core::{ControlMessage, DeviceId, Message, MessageEnvelope, MessageId};
use offline_protocol_transport::{Transport, TransportMetrics, TransportType};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Configuration for the router
#[derive(Debug, Clone)]
pub struct RouterConfig {
    /// Initial TTL for new messages
    pub initial_ttl: u8,
    
    /// Enable DORS
    pub enable_dors: bool,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            initial_ttl: 8,
            enable_dors: true,
        }
    }
}

/// Main router for handling message routing and forwarding
pub struct Router {
    config: RouterConfig,
    device_id: DeviceId,
    dors_engine: Option<Arc<DorsEngine>>,
    relay_manager: Arc<RelayManager>,
    transports: Arc<RwLock<HashMap<TransportType, Box<dyn Transport>>>>,
}

impl Router {
    pub fn new(
        config: RouterConfig,
        device_id: DeviceId,
        dors_engine: Option<Arc<DorsEngine>>,
        relay_manager: Arc<RelayManager>,
    ) -> Self {
        Self {
            config,
            device_id,
            dors_engine,
            relay_manager,
            transports: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a transport with the router
    pub fn register_transport(&self, transport_type: TransportType, transport: Box<dyn Transport>) {
        self.transports.write().insert(transport_type, transport);
        info!("Registered transport: {:?}", transport_type);
    }

    /// Route a message (either send new or forward existing)
    pub async fn route(&self, envelope: &mut MessageEnvelope) -> Result<()> {
        // Check TTL
        if envelope.is_expired() {
            warn!("Message {} TTL expired, dropping", envelope.message_id);
            return Err(crate::Error::MessageDropped("TTL expired".to_string()));
        }

        // Select transport
        let transport_type = self.select_transport(envelope);
        
        debug!(
            "Routing message {} via {:?} (TTL: {}, hops: {})",
            envelope.message_id, transport_type, envelope.ttl, envelope.hop_count
        );

        // Send via selected transport
        self.send_via_transport(transport_type, envelope).await
    }

    /// Forward a received message (multi-hop routing)
    pub async fn forward(&self, envelope: &mut MessageEnvelope) -> Result<()> {
        // Check if we should forward
        if !self.should_forward(envelope) {
            debug!("Not forwarding message {}", envelope.message_id);
            return Ok(());
        }

        // Decrement TTL and increment hop count
        envelope.forward()?;

        // Forward the message
        self.route(envelope).await
    }

    /// Send an ACK for a received message
    pub async fn send_ack(&self, message_id: MessageId, hop_count: u8, recipient: DeviceId) -> Result<()> {
        let ack_message = Message::Control(ControlMessage::Ack {
            message_id,
            hop_count,
        });

        let mut envelope = MessageEnvelope::new(
            self.device_id,
            offline_protocol_core::UserId::new("system"), // System user for control messages
            Some(offline_protocol_core::UserId::new(recipient.to_string())),
            ack_message,
            offline_protocol_core::Priority::High,
            self.config.initial_ttl,
        );

        self.route(&mut envelope).await
    }

    /// Select the best transport for a message
    fn select_transport(&self, envelope: &MessageEnvelope) -> TransportType {
        if let Some(dors) = &self.dors_engine {
            if self.config.enable_dors {
                let transports = self.transports.read();
                let available: Vec<TransportType> = transports.keys().copied().collect();
                
                // Collect metrics
                let mut metrics = HashMap::new();
                for (transport_type, _) in transports.iter() {
                    // TODO: Gather actual metrics from transports
                    metrics.insert(
                        *transport_type,
                        TransportMetrics::new(*transport_type),
                    );
                }

                return dors.select_transport(envelope.message_id, &available, &metrics);
            }
        }

        // Fallback: prefer BLE
        if self.transports.read().contains_key(&TransportType::BLE) {
            TransportType::BLE
        } else {
            TransportType::WiFiDirect
        }
    }

    /// Send a message via a specific transport
    async fn send_via_transport(
        &self,
        transport_type: TransportType,
        envelope: &MessageEnvelope,
    ) -> Result<()> {
        let mut transports = self.transports.write();
        
        let transport = transports
            .get_mut(&transport_type)
            .ok_or(crate::Error::NoRouteAvailable)?;

        // Broadcast if no specific recipient
        if envelope.recipient_user_id.is_none() {
            transport.broadcast(envelope).await?;
        } else {
            // TODO: Resolve user ID to device ID from neighbor table
            // For now, broadcast to all neighbors
            transport.broadcast(envelope).await?;
        }

        Ok(())
    }

    /// Determine if we should forward a message
    fn should_forward(&self, envelope: &MessageEnvelope) -> bool {
        // Don't forward if TTL expired
        if envelope.is_expired() {
            return false;
        }

        // Don't forward our own messages
        if envelope.sender_device_id == self.device_id {
            return false;
        }

        // Check if we're a relay
        if !self.relay_manager.is_relay() {
            debug!("Not a relay, not forwarding");
            return false;
        }

        // Don't forward control messages (except beacons)
        if let Message::Control(ControlMessage::Ack { .. }) = &envelope.message {
            return false;
        }

        true
    }

    /// Get current transport metrics
    pub async fn get_metrics(&self) -> HashMap<TransportType, TransportMetrics> {
        let transports = self.transports.read();
        let mut metrics = HashMap::new();

        for (transport_type, transport) in transports.iter() {
            metrics.insert(*transport_type, transport.get_metrics().await);
        }

        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{TextMessage, UserId};
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_router_ttl_expiration() {
        let config = RouterConfig::default();
        let device_id = DeviceId::new();
        let relay_manager = Arc::new(RelayManager::new(Default::default()));
        
        let router = Router::new(config, device_id, None, relay_manager);

        let mut envelope = MessageEnvelope::new(
            DeviceId::new(),
            UserId::new("sender"),
            Some(UserId::new("recipient")),
            Message::Text(TextMessage {
                text: "Test".to_string(),
                metadata: HashMap::new(),
            }),
            offline_protocol_core::Priority::Medium,
            0, // TTL = 0
        );

        let result = router.route(&mut envelope).await;
        assert!(result.is_err());
    }
}

