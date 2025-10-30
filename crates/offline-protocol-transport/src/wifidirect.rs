//! Wi-Fi Direct transport implementation (stub)
//!
//! This is platform-specific and requires native code (especially on Android).
//! The full implementation would involve JNI bridges and platform-specific APIs.

use crate::{
    LinkQuality, Neighbor, Result, Transport, TransportEvent, TransportMetrics, TransportType,
};
use async_trait::async_trait;
use offline_protocol_core::{DeviceId, MessageEnvelope};
use tokio::sync::mpsc;

/// Configuration for Wi-Fi Direct transport
#[derive(Debug, Clone)]
pub struct WiFiDirectTransportConfig {
    pub device_id: DeviceId,
    pub group_owner_intent: u8, // 0-15, higher = more likely to be group owner
}

/// Wi-Fi Direct transport (stub implementation)
///
/// Full implementation requires platform-specific code:
/// - Android: Wi-Fi P2P APIs via JNI
/// - iOS: Not available (iOS doesn't support Wi-Fi Direct)
pub struct WiFiDirectTransport {
    _config: WiFiDirectTransportConfig,
    event_rx: Option<mpsc::UnboundedReceiver<TransportEvent>>,
}

impl WiFiDirectTransport {
    pub fn new(config: WiFiDirectTransportConfig) -> Self {
        let (_event_tx, event_rx) = mpsc::unbounded_channel();
        
        Self {
            _config: config,
            event_rx: Some(event_rx),
        }
    }
}

#[async_trait]
impl Transport for WiFiDirectTransport {
    async fn start(&mut self) -> Result<()> {
        // TODO: Platform-specific implementation
        Err(crate::Error::WiFiDirect(
            "Wi-Fi Direct not implemented - requires platform-specific code".to_string(),
        ))
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    async fn pause(&mut self) -> Result<()> {
        Ok(())
    }

    async fn resume(&mut self) -> Result<()> {
        Ok(())
    }

    async fn send(&mut self, _device_id: DeviceId, _message: &MessageEnvelope) -> Result<()> {
        Err(crate::Error::WiFiDirect("Not implemented".to_string()))
    }

    async fn broadcast(&mut self, _message: &MessageEnvelope) -> Result<()> {
        Err(crate::Error::WiFiDirect("Not implemented".to_string()))
    }

    async fn get_neighbors(&self) -> Vec<Neighbor> {
        Vec::new()
    }

    async fn get_link_quality(&self, _device_id: DeviceId) -> Option<LinkQuality> {
        None
    }

    async fn get_metrics(&self) -> TransportMetrics {
        TransportMetrics::new(TransportType::WiFiDirect)
    }

    fn event_channel(&self) -> mpsc::UnboundedReceiver<TransportEvent> {
        // For WiFi Direct stub, create a new channel
        let (_tx, rx) = mpsc::unbounded_channel();
        rx
    }

    fn is_running(&self) -> bool {
        false
    }

    fn is_paused(&self) -> bool {
        false
    }
}

