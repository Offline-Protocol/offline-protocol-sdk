//! Network visualization and topology export.
//!
//! This module provides tools for visualizing the mesh network topology
//! and exporting metrics for debugging and monitoring.

use crate::constants::{HISTORY_CLEANUP_BATCH_SIZE, MAX_MESSAGE_HISTORY};
use crate::{Error, Result};
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

/// Node in the network topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkNode {
    /// Node user ID
    pub user_id: String,
    /// Node role (Normal or Relay)
    pub role: NodeRole,
    /// Connection count
    pub connection_count: usize,
    /// Battery level (0-100), if known
    pub battery_level: Option<u8>,
    /// Last seen timestamp
    pub last_seen: i64,
    /// Transport types available
    pub transports: Vec<TransportType>,
}

/// Node role in the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeRole {
    /// Normal node
    Normal,
    /// Relay node (forwarding messages)
    Relay,
}

/// Edge/link between two nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkLink {
    /// Source node
    pub from: String,
    /// Destination node
    pub to: String,
    /// Link quality (0.0 - 1.0)
    pub quality: f32,
    /// Transport type used for this link
    pub transport: TransportType,
    /// RSSI (signal strength) if available
    pub rssi: Option<i16>,
}

/// Complete network topology snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkTopology {
    /// Timestamp of this snapshot
    pub timestamp: i64,
    /// Local device user ID
    pub local_user_id: String,
    /// All nodes in the network
    pub nodes: Vec<NetworkNode>,
    /// All links between nodes
    pub links: Vec<NetworkLink>,
    /// Network-wide statistics
    pub stats: NetworkStats,
}

/// Network-wide statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStats {
    /// Total nodes in network
    pub total_nodes: usize,
    /// Total relay nodes
    pub relay_nodes: usize,
    /// Total active connections
    pub total_connections: usize,
    /// Average link quality
    pub avg_link_quality: f32,
    /// Network diameter (max hops between any two nodes)
    pub network_diameter: Option<u8>,
}

/// Message delivery statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageStats {
    /// Message ID
    pub message_id: String,
    /// Sender
    pub sender: String,
    /// Recipient
    pub recipient: String,
    /// Timestamp sent
    pub sent_at: i64,
    /// Timestamp delivered (if delivered)
    pub delivered_at: Option<i64>,
    /// Number of hops
    pub hop_count: u8,
    /// Transport used for final delivery
    pub transport: Option<TransportType>,
    /// Retry count
    pub retry_count: u32,
    /// Delivery latency in milliseconds (if delivered)
    pub latency_ms: Option<u64>,
}

/// Network metrics collector for visualization.
pub struct NetworkVisualizer {
    /// Discovered nodes
    nodes: HashMap<String, NetworkNode>,
    /// Active links
    links: Vec<NetworkLink>,
    /// Message delivery history
    message_history: Vec<MessageStats>,
    /// Local user ID
    local_user_id: String,
}

impl NetworkVisualizer {
    /// Creates a new network visualizer.
    pub fn new(local_user_id: impl Into<String>) -> Self {
        Self {
            nodes: HashMap::new(),
            links: Vec::new(),
            message_history: Vec::new(),
            local_user_id: local_user_id.into(),
        }
    }

    /// Registers or updates a node in the topology.
    pub fn update_node(&mut self, node: NetworkNode) {
        self.nodes.insert(node.user_id.clone(), node);
    }

    /// Adds a link between two nodes.
    pub fn add_link(&mut self, link: NetworkLink) {
        // Remove existing link if present
        self.links
            .retain(|l| !(l.from == link.from && l.to == link.to));
        self.links.push(link);
    }

    /// Removes a link between two nodes.
    pub fn remove_link(&mut self, from: &str, to: &str) {
        self.links.retain(|l| !(l.from == from && l.to == to));
    }

    /// Records a message delivery attempt.
    pub fn record_message(&mut self, stats: MessageStats) {
        self.message_history.push(stats);

        if self.message_history.len() > MAX_MESSAGE_HISTORY {
            self.message_history.drain(0..HISTORY_CLEANUP_BATCH_SIZE);
        }
    }

    /// Generates a network topology snapshot.
    pub fn get_topology(&self) -> NetworkTopology {
        let stats = self.calculate_stats();

        NetworkTopology {
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            local_user_id: self.local_user_id.clone(),
            nodes: self.nodes.values().cloned().collect(),
            links: self.links.clone(),
            stats,
        }
    }

    /// Calculates network-wide statistics.
    fn calculate_stats(&self) -> NetworkStats {
        let total_nodes = self.nodes.len();
        let relay_nodes = self
            .nodes
            .values()
            .filter(|n| n.role == NodeRole::Relay)
            .count();
        let total_connections = self.links.len();

        let avg_link_quality = if total_connections > 0 {
            self.links.iter().map(|l| l.quality).sum::<f32>() / total_connections as f32
        } else {
            0.0
        };

        NetworkStats {
            total_nodes,
            relay_nodes,
            total_connections,
            avg_link_quality,
            network_diameter: self.calculate_diameter(),
        }
    }

    /// Calculates network diameter (max hops between any two nodes).
    ///
    /// Uses Floyd-Warshall algorithm for simplicity.
    fn calculate_diameter(&self) -> Option<u8> {
        if self.nodes.is_empty() {
            return None;
        }

        // Build adjacency map
        let nodes: Vec<_> = self.nodes.keys().cloned().collect();
        let n = nodes.len();

        if n == 1 {
            return Some(0);
        }

        // Initialize distance matrix
        let mut dist = vec![vec![u8::MAX; n]; n];

        // Set diagonal to 0
        for (i, row) in dist.iter_mut().enumerate() {
            row[i] = 0;
        }

        // Set direct connections to 1
        for link in &self.links {
            if let (Some(from_idx), Some(to_idx)) = (
                nodes.iter().position(|id| id == &link.from),
                nodes.iter().position(|id| id == &link.to),
            ) {
                dist[from_idx][to_idx] = 1;
                dist[to_idx][from_idx] = 1; // Assume bidirectional
            }
        }

        // Floyd-Warshall
        for k in 0..n {
            for i in 0..n {
                for j in 0..n {
                    if dist[i][k] != u8::MAX && dist[k][j] != u8::MAX {
                        dist[i][j] = dist[i][j].min(dist[i][k].saturating_add(dist[k][j]));
                    }
                }
            }
        }

        // Find maximum distance
        let max_dist = dist
            .iter()
            .flat_map(|row| row.iter())
            .filter(|&&d| d != u8::MAX)
            .max()
            .copied();

        max_dist
    }

    /// Exports topology as JSON.
    pub fn export_json(&self) -> Result<String> {
        let topology = self.get_topology();
        serde_json::to_string_pretty(&topology)
            .map_err(|e| Error::Other(format!("Failed to serialize topology: {}", e)))
    }

    /// Gets message delivery statistics.
    pub fn get_message_stats(&self) -> Vec<MessageStats> {
        self.message_history.clone()
    }

    /// Calculates delivery success rate.
    pub fn delivery_success_rate(&self) -> f32 {
        if self.message_history.is_empty() {
            return 0.0;
        }

        let delivered = self
            .message_history
            .iter()
            .filter(|m| m.delivered_at.is_some())
            .count();

        delivered as f32 / self.message_history.len() as f32
    }

    /// Calculates median delivery latency.
    pub fn median_latency(&self) -> Option<u64> {
        let mut latencies: Vec<_> = self
            .message_history
            .iter()
            .filter_map(|m| m.latency_ms)
            .collect();

        if latencies.is_empty() {
            return None;
        }

        latencies.sort_unstable();
        Some(latencies[latencies.len() / 2])
    }

    /// Calculates median hop count.
    pub fn median_hops(&self) -> Option<u8> {
        let mut hops: Vec<_> = self
            .message_history
            .iter()
            .filter(|m| m.delivered_at.is_some())
            .map(|m| m.hop_count)
            .collect();

        if hops.is_empty() {
            return None;
        }

        hops.sort_unstable();
        Some(hops[hops.len() / 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visualizer_creation() {
        let viz = NetworkVisualizer::new("alice");
        let topology = viz.get_topology();
        assert_eq!(topology.local_user_id, "alice");
        assert_eq!(topology.nodes.len(), 0);
    }

    #[test]
    fn test_add_node() {
        let mut viz = NetworkVisualizer::new("alice");

        let node = NetworkNode {
            user_id: "bob".to_string(),
            role: NodeRole::Normal,
            connection_count: 1,
            battery_level: Some(80),
            last_seen: 0,
            transports: vec![TransportType::BLE],
        };

        viz.update_node(node);

        let topology = viz.get_topology();
        assert_eq!(topology.nodes.len(), 1);
        assert_eq!(topology.nodes[0].user_id, "bob");
    }

    #[test]
    fn test_add_link() {
        let mut viz = NetworkVisualizer::new("alice");

        let link = NetworkLink {
            from: "alice".to_string(),
            to: "bob".to_string(),
            quality: 0.9,
            transport: TransportType::BLE,
            rssi: Some(-50),
        };

        viz.add_link(link);

        let topology = viz.get_topology();
        assert_eq!(topology.links.len(), 1);
    }

    #[test]
    fn test_network_stats() {
        let mut viz = NetworkVisualizer::new("alice");

        // Add nodes
        for i in 0..5 {
            viz.update_node(NetworkNode {
                user_id: format!("node{}", i),
                role: if i < 2 {
                    NodeRole::Relay
                } else {
                    NodeRole::Normal
                },
                connection_count: 2,
                battery_level: Some(75),
                last_seen: 0,
                transports: vec![TransportType::BLE],
            });
        }

        // Add links
        viz.add_link(NetworkLink {
            from: "node0".to_string(),
            to: "node1".to_string(),
            quality: 0.8,
            transport: TransportType::BLE,
            rssi: Some(-60),
        });

        let topology = viz.get_topology();
        assert_eq!(topology.stats.total_nodes, 5);
        assert_eq!(topology.stats.relay_nodes, 2);
        assert_eq!(topology.stats.total_connections, 1);
    }

    #[test]
    fn test_message_stats() {
        let mut viz = NetworkVisualizer::new("alice");

        viz.record_message(MessageStats {
            message_id: "msg1".to_string(),
            sender: "alice".to_string(),
            recipient: "bob".to_string(),
            sent_at: 0,
            delivered_at: Some(100),
            hop_count: 3,
            transport: Some(TransportType::BLE),
            retry_count: 0,
            latency_ms: Some(100),
        });

        viz.record_message(MessageStats {
            message_id: "msg2".to_string(),
            sender: "alice".to_string(),
            recipient: "charlie".to_string(),
            sent_at: 100,
            delivered_at: None,
            hop_count: 0,
            transport: None,
            retry_count: 3,
            latency_ms: None,
        });

        assert_eq!(viz.delivery_success_rate(), 0.5);
        assert_eq!(viz.median_latency(), Some(100));
        assert_eq!(viz.median_hops(), Some(3));
    }

    #[test]
    fn test_export_json() {
        let mut viz = NetworkVisualizer::new("alice");

        viz.update_node(NetworkNode {
            user_id: "bob".to_string(),
            role: NodeRole::Relay,
            connection_count: 3,
            battery_level: Some(90),
            last_seen: 0,
            transports: vec![TransportType::BLE, TransportType::WiFiDirect],
        });

        let json = viz.export_json().unwrap();
        assert!(json.contains("bob"));
        assert!(json.contains("relay"));
    }
}
