//! Transport-related types

use offline_protocol_core::{DeviceId, UserId};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};

/// Type of transport
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    BLE,
    WiFiDirect,
    Mock,
}

impl std::fmt::Display for TransportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportType::BLE => write!(f, "ble"),
            TransportType::WiFiDirect => write!(f, "wifidirect"),
            TransportType::Mock => write!(f, "mock"),
        }
    }
}

/// Role of a neighbor in the network
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NeighborRole {
    Peer,
    Relay,
}

/// Link quality assessment
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinkQuality {
    /// RSSI (Received Signal Strength Indicator) in dBm
    pub rssi: Option<i16>,
    
    /// Packet delivery ratio (0.0 to 1.0)
    pub delivery_ratio: f64,
    
    /// Average latency in milliseconds
    pub avg_latency_ms: u64,
    
    /// Number of packets sent
    pub packets_sent: u64,
    
    /// Number of packets successfully delivered (ACKed)
    pub packets_delivered: u64,
}

impl LinkQuality {
    pub fn new() -> Self {
        Self {
            rssi: None,
            delivery_ratio: 1.0,
            avg_latency_ms: 0,
            packets_sent: 0,
            packets_delivered: 0,
        }
    }

    /// Update delivery ratio based on a new packet result
    pub fn update_delivery(&mut self, delivered: bool) {
        self.packets_sent += 1;
        if delivered {
            self.packets_delivered += 1;
        }
        self.delivery_ratio = self.packets_delivered as f64 / self.packets_sent as f64;
    }

    /// Update average latency with a new sample
    pub fn update_latency(&mut self, latency_ms: u64) {
        if self.packets_delivered == 0 {
            self.avg_latency_ms = latency_ms;
        } else {
            // Exponential moving average
            self.avg_latency_ms = (self.avg_latency_ms * 7 + latency_ms) / 8;
        }
    }

    /// Check if the link quality is good enough
    pub fn is_good(&self) -> bool {
        self.delivery_ratio >= 0.7 && self.rssi.map_or(true, |r| r >= -85)
    }
}

impl Default for LinkQuality {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about a discovered neighbor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbor {
    pub device_id: DeviceId,
    pub user_id: UserId,
    pub role: NeighborRole,
    pub link_quality: LinkQuality,
    pub last_seen: SystemTime,
    pub connection_count: u8,
}

impl Neighbor {
    pub fn new(device_id: DeviceId, user_id: UserId, role: NeighborRole) -> Self {
        Self {
            device_id,
            user_id,
            role,
            link_quality: LinkQuality::new(),
            last_seen: SystemTime::now(),
            connection_count: 0,
        }
    }

    /// Check if the neighbor has timed out
    pub fn is_timed_out(&self, timeout: Duration) -> bool {
        SystemTime::now()
            .duration_since(self.last_seen)
            .map_or(false, |elapsed| elapsed > timeout)
    }

    /// Update last seen timestamp
    pub fn update_last_seen(&mut self) {
        self.last_seen = SystemTime::now();
    }
}

/// Metrics for a transport
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportMetrics {
    /// Transport type
    pub transport_type: TransportType,
    
    /// Average RSSI in dBm
    pub avg_rssi: Option<i16>,
    
    /// Average latency in milliseconds
    pub avg_latency_ms: u64,
    
    /// Overall delivery ratio (0.0 to 1.0)
    pub delivery_ratio: f64,
    
    /// Available bandwidth estimate in bytes/second
    pub bandwidth_bps: Option<u64>,
    
    /// Number of active neighbors
    pub neighbor_count: usize,
    
    /// Total messages sent
    pub messages_sent: u64,
    
    /// Total messages received
    pub messages_received: u64,
}

impl TransportMetrics {
    pub fn new(transport_type: TransportType) -> Self {
        Self {
            transport_type,
            avg_rssi: None,
            avg_latency_ms: 0,
            delivery_ratio: 1.0,
            bandwidth_bps: None,
            neighbor_count: 0,
            messages_sent: 0,
            messages_received: 0,
        }
    }
}

