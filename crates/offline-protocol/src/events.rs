//! Event types emitted by the SDK

use chrono::{DateTime, Utc};
use offline_protocol_core::{DeviceId, MessageId, UserId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Events emitted by the Offline Protocol SDK
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    MessageReceived(MessageReceivedEvent),
    MessageDelivered(MessageDeliveredEvent),
    MessageFailed(MessageFailedEvent),
    FileReceived(FileReceivedEvent),
    RelayPromoted(RelayPromotedEvent),
    RelayDemoted(RelayDemotedEvent),
    TransportSwitched(TransportSwitchedEvent),
    NeighborDiscovered(NeighborDiscoveredEvent),
    NeighborLost(NeighborLostEvent),
    NetworkMetrics(NetworkMetricsEvent),
}

/// Message received event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageReceivedEvent {
    pub message_id: MessageId,
    pub sender_username: String,
    pub text: String,
    pub metadata: HashMap<String, String>,
    pub timestamp: DateTime<Utc>,
}

/// Message delivered event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDeliveredEvent {
    pub message_id: MessageId,
    pub hop_count: u8,
    pub latency_ms: u64,
    pub transport: String,
}

/// Message failed event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageFailedEvent {
    pub message_id: MessageId,
    pub reason: String,
}

/// File received event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReceivedEvent {
    pub message_id: MessageId,
    pub sender_username: String,
    pub file: FileInfo,
    pub timestamp: DateTime<Utc>,
}

/// File information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// Relay promoted event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayPromotedEvent {
    pub connection_count: u8,
    pub timestamp: DateTime<Utc>,
}

/// Relay demoted event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayDemotedEvent {
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

/// Transport switched event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportSwitchedEvent {
    pub from: String,
    pub to: String,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

/// Neighbor discovered event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighborDiscoveredEvent {
    pub username: String,
    pub device_id: DeviceId,
    pub role: String,
    pub link_quality: f64,
    pub rssi: Option<i16>,
}

/// Neighbor lost event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighborLostEvent {
    pub username: String,
    pub device_id: DeviceId,
}

/// Network metrics event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMetricsEvent {
    pub neighbor_count: usize,
    pub relay_count: usize,
    pub delivery_ratio: f64,
    pub avg_latency_ms: u64,
}

