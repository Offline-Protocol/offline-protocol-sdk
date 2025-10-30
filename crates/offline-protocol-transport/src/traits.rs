//! Transport trait and related types

use crate::{LinkQuality, Neighbor, Result, TransportMetrics};
use async_trait::async_trait;
use offline_protocol_core::{DeviceId, MessageEnvelope};
use tokio::sync::mpsc;

/// Events emitted by transports
#[derive(Debug, Clone)]
pub enum TransportEvent {
    /// A neighbor was discovered
    NeighborDiscovered(Neighbor),
    
    /// A neighbor was lost (timed out)
    NeighborLost(DeviceId),
    
    /// A message was received
    MessageReceived(MessageEnvelope),
    
    /// Transport started successfully
    Started,
    
    /// Transport stopped
    Stopped,
    
    /// Transport error occurred
    Error(String),
}

/// Transport trait defining the interface for all transport implementations
#[async_trait]
pub trait Transport: Send + Sync {
    /// Start the transport
    async fn start(&mut self) -> Result<()>;

    /// Stop the transport
    async fn stop(&mut self) -> Result<()>;

    /// Pause the transport (reduced operations for battery saving)
    async fn pause(&mut self) -> Result<()>;

    /// Resume the transport from paused state
    async fn resume(&mut self) -> Result<()>;

    /// Send a message to a specific neighbor
    async fn send(&mut self, device_id: DeviceId, message: &MessageEnvelope) -> Result<()>;

    /// Broadcast a message to all neighbors
    async fn broadcast(&mut self, message: &MessageEnvelope) -> Result<()>;

    /// Get the list of current neighbors
    async fn get_neighbors(&self) -> Vec<Neighbor>;

    /// Get link quality for a specific neighbor
    async fn get_link_quality(&self, device_id: DeviceId) -> Option<LinkQuality>;

    /// Get transport metrics
    async fn get_metrics(&self) -> TransportMetrics;

    /// Get a channel to receive transport events
    fn event_channel(&self) -> mpsc::UnboundedReceiver<TransportEvent>;

    /// Check if the transport is currently running
    fn is_running(&self) -> bool;

    /// Check if the transport is paused
    fn is_paused(&self) -> bool;
}

