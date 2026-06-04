//! Path selection and routing for optimal message delivery.
//!
//! This module implements gossip-based probabilistic forwarding to prevent
//! broadcast storms in large networks. The forwarding probability adapts
//! based on the number of visible peers to maintain constant message overhead.

use crate::constants::*;
use crate::relay::{RelayInfo, RelayManager, RelayRole, RelayTransition};
use offline_protocol_core::Message;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// Information about a neighboring device.
#[derive(Debug, Clone)]
pub struct NeighborInfo {
    /// Unique identifier for this neighbor.
    pub peer_id: String,

    /// RSSI signal strength (dBm).
    pub rssi: i16,

    /// Number of hops to destination through this neighbor (if known).
    pub hops_to_destination: Option<u8>,

    /// Link quality to this neighbor (0-100).
    pub link_quality: u8,

    /// Relay information if this neighbor is a relay.
    pub relay_info: Option<RelayInfo>,
}

/// Gossip forwarding configuration for probabilistic message propagation.
#[derive(Debug, Clone)]
pub struct GossipConfig {
    /// Enable probabilistic (gossip) forwarding to prevent broadcast storms.
    pub enabled: bool,

    /// Target number of peers to forward to in large networks.
    /// The actual forwarding probability is computed as: min(1.0, target_fanout / visible_peers)
    pub target_fanout: usize,

    /// Minimum forwarding probability (0.0-1.0). Ensures messages still propagate
    /// even in very dense networks.
    pub min_probability: f32,

    /// Peer count threshold below which we always forward (no probabilistic dropping).
    /// Below this threshold, the network is small enough to handle full flooding.
    pub small_network_threshold: usize,

    /// High-priority messages bypass probabilistic dropping.
    pub priority_bypass: bool,
}

impl Default for GossipConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            target_fanout: 4,
            min_probability: 0.15,
            small_network_threshold: 10,
            priority_bypass: true,
        }
    }
}

/// Configuration for gradient routing.
#[derive(Debug, Clone)]
pub struct GradientRoutingConfig {
    /// Enable gradient routing for directed message delivery.
    pub enabled: bool,
    /// Maximum entries in the routing table per destination.
    pub max_routes_per_destination: usize,
    /// Time to live for routing entries (seconds).
    pub route_ttl_secs: u64,
    /// Maximum total routing table entries.
    pub max_routing_table_size: usize,
}

impl Default for GradientRoutingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_routes_per_destination: 3,
            route_ttl_secs: 300, // 5 minutes
            max_routing_table_size: 1000,
        }
    }
}

/// A routing entry tracking how to reach a destination.
/// Uses a destination sequence number (DSDV-style) to avoid loops: newer updates
/// from the destination are preferred; stale routes are ignored.
#[derive(Debug, Clone)]
pub struct RouteEntry {
    /// The next-hop neighbor to reach this destination.
    pub next_hop: String,
    /// Hop count to destination through this route.
    pub hop_count: u8,
    /// When this route was last confirmed.
    pub last_seen: Instant,
    /// Quality score of this route (higher is better).
    pub quality: f32,
    /// Destination sequence number from the destination node (DSDV).
    /// Higher values are fresher; prefer route with higher sequence to avoid count-to-infinity.
    pub sequence_number: u32,
}

/// Gradient routing table for directed message delivery.
/// Learns routes from incoming messages and uses them for replies.
#[derive(Debug)]
pub struct GradientRoutingTable {
    config: GradientRoutingConfig,
    /// Routes indexed by destination user ID.
    routes: HashMap<String, Vec<RouteEntry>>,
    /// Reverse mapping: which destinations can be reached through which neighbor.
    neighbor_destinations: HashMap<String, Vec<String>>,
}

impl GradientRoutingTable {
    /// Creates a new gradient routing table.
    pub fn new() -> Self {
        Self::with_config(GradientRoutingConfig::default())
    }

    /// Creates a new gradient routing table with custom configuration.
    pub fn with_config(config: GradientRoutingConfig) -> Self {
        Self {
            config,
            routes: HashMap::new(),
            neighbor_destinations: HashMap::new(),
        }
    }

    /// Records a route learned from an incoming message (DSDV-style).
    /// Prefers higher `sequence_number` (fresher from destination) to avoid loops.
    /// Pass 0 for sequence when the message does not carry a destination sequence.
    pub fn learn_route(
        &mut self,
        destination: &str,
        next_hop: &str,
        hop_count: u8,
        quality: f32,
        sequence_number: u32,
    ) {
        if !self.config.enabled {
            return;
        }

        let now = Instant::now();

        // Get or create route list for this destination
        let routes = self.routes.entry(destination.to_string()).or_default();

        // Check if we already have a route through this neighbor
        if let Some(existing) = routes.iter_mut().find(|r| r.next_hop == next_hop) {
            // Accept update only if fresher (higher seq, with wrapping) or same seq with better hop count
            let accept = seq_is_newer(sequence_number, existing.sequence_number)
                || (sequence_number == existing.sequence_number && hop_count <= existing.hop_count);
            if accept {
                existing.hop_count = hop_count;
                existing.last_seen = now;
                existing.quality = quality;
                existing.sequence_number = sequence_number;
            }
            return;
        }

        // New route through this neighbor
        if routes.len() >= self.config.max_routes_per_destination {
            // Sort best-first so worst is last; pop() removes worst (lowest seq, lowest quality, highest hop_count)
            routes.sort_by(|a, b| {
                seq_cmp(b.sequence_number, a.sequence_number)
                    .then_with(|| {
                        b.quality
                            .partial_cmp(&a.quality)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| a.hop_count.cmp(&b.hop_count))
            });
            if let Some(evicted) = routes.pop() {
                // Remove evicted route from reverse mapping
                if let Some(dests) = self.neighbor_destinations.get_mut(&evicted.next_hop) {
                    dests.retain(|d| d != destination);
                    if dests.is_empty() {
                        self.neighbor_destinations.remove(&evicted.next_hop);
                    }
                }
            }
        }

        routes.push(RouteEntry {
            next_hop: next_hop.to_string(),
            hop_count,
            last_seen: now,
            quality,
            sequence_number,
        });

        // Update reverse mapping (only add if not already present)
        let dests = self
            .neighbor_destinations
            .entry(next_hop.to_string())
            .or_default();
        if !dests.contains(&destination.to_string()) {
            dests.push(destination.to_string());
        }

        // Enforce table size limit
        self.enforce_size_limit();
    }

    /// Gets the best route to a destination, if known.
    pub fn get_route(&self, destination: &str) -> Option<&RouteEntry> {
        if !self.config.enabled {
            return None;
        }

        let routes = self.routes.get(destination)?;
        let ttl = Duration::from_secs(self.config.route_ttl_secs);
        let now = Instant::now();

        // Find best non-expired route: prefer higher sequence (fresher, wrapping-aware),
        // then quality, then lower hop_count
        routes
            .iter()
            .filter(|r| now.duration_since(r.last_seen) < ttl)
            .max_by(|a, b| {
                seq_cmp(a.sequence_number, b.sequence_number)
                    .then_with(|| {
                        a.quality
                            .partial_cmp(&b.quality)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| b.hop_count.cmp(&a.hop_count))
            })
    }

    /// Gets all valid routes to a destination.
    pub fn get_routes(&self, destination: &str) -> Vec<&RouteEntry> {
        if !self.config.enabled {
            return Vec::new();
        }

        let Some(routes) = self.routes.get(destination) else {
            return Vec::new();
        };

        let ttl = Duration::from_secs(self.config.route_ttl_secs);
        let now = Instant::now();

        routes
            .iter()
            .filter(|r| now.duration_since(r.last_seen) < ttl)
            .collect()
    }

    /// Checks if we have a known route to a destination.
    pub fn has_route(&self, destination: &str) -> bool {
        self.get_route(destination).is_some()
    }

    /// Removes a neighbor from the routing table (e.g., on disconnect).
    pub fn remove_neighbor(&mut self, neighbor: &str) {
        // Remove all routes through this neighbor
        for routes in self.routes.values_mut() {
            routes.retain(|r| r.next_hop != neighbor);
        }

        // Remove empty entries
        self.routes.retain(|_, v| !v.is_empty());

        // Update reverse mapping
        self.neighbor_destinations.remove(neighbor);
    }

    /// Cleans up expired routes.
    pub fn cleanup_expired(&mut self) {
        let ttl = Duration::from_secs(self.config.route_ttl_secs);
        let now = Instant::now();

        for routes in self.routes.values_mut() {
            routes.retain(|r| now.duration_since(r.last_seen) < ttl);
        }

        self.routes.retain(|_, v| !v.is_empty());

        // Rebuild reverse mapping
        self.neighbor_destinations.clear();
        for (dest, routes) in &self.routes {
            for route in routes {
                self.neighbor_destinations
                    .entry(route.next_hop.clone())
                    .or_default()
                    .push(dest.clone());
            }
        }
    }

    /// Returns the number of known destinations.
    pub fn destination_count(&self) -> usize {
        self.routes.len()
    }

    /// Returns the total number of routes.
    pub fn route_count(&self) -> usize {
        self.routes.values().map(|v| v.len()).sum()
    }

    /// Enforces the maximum routing table size.
    ///
    /// Evicts routes with the lowest composite score (quality * recency_factor)
    /// so that high-quality, recently-seen routes survive over stale/low-quality ones.
    fn enforce_size_limit(&mut self) {
        let route_ttl_secs = self.config.route_ttl_secs as f64;
        while self.route_count() > self.config.max_routing_table_size {
            // Find route with the lowest composite score
            let worst = self
                .routes
                .iter()
                .flat_map(|(dest, routes)| routes.iter().map(move |r| (dest.clone(), r)))
                .min_by(|(_, a), (_, b)| {
                    let score_a = composite_eviction_score(a, route_ttl_secs);
                    let score_b = composite_eviction_score(b, route_ttl_secs);
                    score_a
                        .partial_cmp(&score_b)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

            if let Some((dest, route)) = worst {
                let next_hop = route.next_hop.clone();
                if let Some(routes) = self.routes.get_mut(&dest) {
                    routes.retain(|r| r.next_hop != next_hop);
                    if routes.is_empty() {
                        self.routes.remove(&dest);
                    }
                }
                // Update reverse mapping
                if let Some(dests) = self.neighbor_destinations.get_mut(&next_hop) {
                    dests.retain(|d| d != &dest);
                    if dests.is_empty() {
                        self.neighbor_destinations.remove(&next_hop);
                    }
                }
            } else {
                break;
            }
        }
    }
}

/// RFC 1982 serial number arithmetic for 32-bit sequence numbers.
/// Returns `true` if `new` is strictly newer than `old`, handling wrapping.
fn seq_is_newer(new: u32, old: u32) -> bool {
    // When new == old, not newer. Otherwise, check sign of difference.
    new != old && new.wrapping_sub(old) < (1u32 << 31)
}

/// Wrapping-aware comparison of two sequence numbers.
/// Returns `Ordering::Greater` if `a` is newer, `Less` if `b` is newer,
/// `Equal` if they are the same.
///
/// **Note on total order:** When `a` and `b` are exactly `1 << 31` apart,
/// RFC 1982 declares the relationship *undefined*. This function maps the
/// ambiguous case to `Less`, which means `seq_cmp(a, b)` and `seq_cmp(b, a)`
/// can both return `Less`. In practice the half-space boundary is unreachable
/// (it requires two live sequence numbers that differ by 2 billion), and the
/// `then_with` tie-breakers on quality/hop_count ensure stable sort results
/// even if it were hit.
fn seq_cmp(a: u32, b: u32) -> std::cmp::Ordering {
    if a == b {
        std::cmp::Ordering::Equal
    } else if seq_is_newer(a, b) {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Less
    }
}

/// Computes a composite score for eviction: quality * recency_factor.
/// Lower score = more likely to be evicted.
fn composite_eviction_score(route: &RouteEntry, route_ttl_secs: f64) -> f64 {
    let age_secs = route.last_seen.elapsed().as_secs_f64();
    let recency_factor = 1.0 - (age_secs / route_ttl_secs).min(1.0);
    route.quality as f64 * recency_factor
}

impl Default for GradientRoutingTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Path selection configuration.
#[derive(Debug, Clone)]
pub struct PathConfig {
    /// Number of top relays to forward to (for redundancy).
    pub forward_to_top_k: usize,

    /// Maximum acceptable congestion level (0.0-1.0).
    pub max_congestion_level: f32,

    /// Gossip forwarding configuration for scalable message propagation.
    pub gossip: GossipConfig,

    /// Gradient routing configuration for directed message delivery.
    pub gradient_routing: GradientRoutingConfig,
}

impl Default for PathConfig {
    fn default() -> Self {
        Self {
            forward_to_top_k: DEFAULT_FORWARD_TO_TOP_K,
            max_congestion_level: DEFAULT_MAX_CONGESTION_LEVEL,
            gossip: GossipConfig::default(),
            gradient_routing: GradientRoutingConfig::default(),
        }
    }
}

/// Path score breakdown for debugging/monitoring.
#[derive(Debug, Clone)]
pub struct PathScore {
    /// Signal strength component (0-100).
    pub signal: f32,

    /// Proximity/hops component (0-100).
    pub proximity: f32,

    /// Capacity component (0-100).
    pub capacity: f32,

    /// Energy component (0-100).
    pub energy: f32,

    /// Total weighted score.
    pub total: f32,
}

/// Forwarding decision result with reasoning.
#[derive(Debug, Clone)]
pub struct ForwardingDecision {
    /// Peers to forward the message to.
    pub peers: Vec<String>,
    /// Whether gossip probabilistic forwarding was applied.
    pub gossip_applied: bool,
    /// The computed forwarding probability (1.0 if gossip not applied).
    pub probability: f32,
    /// Number of peers that were probabilistically dropped.
    pub dropped_count: usize,
}

/// Path selector for optimal relay selection.
pub struct PathSelector {
    config: PathConfig,
    relay_manager: RelayManager,
    /// Local device ID for deterministic randomness.
    local_device_id: String,
    /// Gradient routing table for directed message delivery.
    routing_table: GradientRoutingTable,
}

impl PathSelector {
    /// Creates a new path selector with default configuration.
    pub fn new() -> Self {
        Self::with_config(PathConfig::default(), RelayManager::new())
    }

    /// Creates a new path selector with custom configuration.
    pub fn with_config(config: PathConfig, relay_manager: RelayManager) -> Self {
        let routing_table = GradientRoutingTable::with_config(config.gradient_routing.clone());
        Self {
            config,
            relay_manager,
            local_device_id: String::new(),
            routing_table,
        }
    }

    /// Sets the local device ID for deterministic gossip decisions.
    pub fn set_local_device_id(&mut self, device_id: impl Into<String>) {
        self.local_device_id = device_id.into();
    }

    /// Returns the current [`RelayRole`] from the internal [`RelayManager`].
    ///
    /// Exposed for read-only callers (e.g., the telemetry aggregator that
    /// needs to know the current relay role without widening the surface
    /// to the full [`RelayManager`] API).
    pub fn current_relay_role(&self) -> RelayRole {
        self.relay_manager.current_role()
    }

    /// Re-evaluates the local relay role against current connectivity and
    /// battery, applying any change to the internal [`RelayManager`].
    ///
    /// Returns `Some(transition)` only when the role actually changes, so the
    /// caller can emit a role-transition event exactly once per transition.
    pub fn evaluate_relay_transition(
        &mut self,
        connection_count: usize,
        battery_level: u8,
        is_charging: bool,
    ) -> Option<RelayTransition> {
        self.relay_manager
            .evaluate_transition(connection_count, battery_level, is_charging)
    }

    /// Computes the forwarding probability based on visible peer count.
    ///
    /// Uses the formula: min(1.0, max(min_probability, target_fanout / peer_count))
    /// This ensures constant expected message overhead regardless of network size.
    pub fn compute_forwarding_probability(&self, visible_peer_count: usize) -> f32 {
        let gossip = &self.config.gossip;

        if !gossip.enabled || visible_peer_count <= gossip.small_network_threshold {
            return 1.0;
        }

        let raw_probability = gossip.target_fanout as f32 / visible_peer_count as f32;
        raw_probability.max(gossip.min_probability).min(1.0)
    }

    /// Determines if a message should be forwarded to a specific peer using
    /// deterministic pseudo-random selection based on message ID and peer ID.
    ///
    /// This ensures that the same message-peer pair always produces the same
    /// decision across all nodes, preventing duplicate forwarding.
    fn should_forward_to_peer(&self, message_id: &str, peer_id: &str, probability: f32) -> bool {
        if probability >= 1.0 {
            return true;
        }
        if probability <= 0.0 {
            return false;
        }

        // Create deterministic hash from message ID, peer ID, and local device ID
        let mut hasher = DefaultHasher::new();
        message_id.hash(&mut hasher);
        peer_id.hash(&mut hasher);
        self.local_device_id.hash(&mut hasher);
        let hash = hasher.finish();

        // Convert hash to probability (0.0 to 1.0)
        let hash_probability = (hash as f64 / u64::MAX as f64) as f32;

        hash_probability < probability
    }

    /// Selects the best path(s) for message delivery with gossip-based filtering.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to route
    /// * `neighbors` - Available neighboring devices
    /// * `total_visible_peers` - Total number of peers visible in the network
    ///   (may be larger than neighbors if some peers are not connected)
    ///
    /// # Returns
    ///
    /// Returns a forwarding decision with selected peers and metadata.
    pub fn select_paths_with_gossip(
        &self,
        message: &Message,
        neighbors: &[NeighborInfo],
        total_visible_peers: usize,
    ) -> ForwardingDecision {
        if neighbors.is_empty() {
            return ForwardingDecision {
                peers: Vec::new(),
                gossip_applied: false,
                probability: 1.0,
                dropped_count: 0,
            };
        }

        // Check if high-priority message should bypass gossip
        let bypass_gossip = self.config.gossip.priority_bypass
            && matches!(
                message.priority,
                offline_protocol_core::MessagePriority::High
                    | offline_protocol_core::MessagePriority::Critical
            );

        // Compute forwarding probability
        let probability = if bypass_gossip {
            1.0
        } else {
            self.compute_forwarding_probability(total_visible_peers.max(neighbors.len()))
        };

        let gossip_applied = probability < 1.0;

        // Calculate scores for each neighbor
        let mut scored_neighbors: Vec<(String, PathScore)> = neighbors
            .iter()
            .map(|neighbor| {
                let score = self.calculate_path_score(message, neighbor);
                (neighbor.peer_id.clone(), score)
            })
            .collect();

        // Filter out neighbors with relay congestion > max threshold
        scored_neighbors.retain(|(peer_id, _)| {
            if let Some(neighbor) = neighbors.iter().find(|n| n.peer_id == *peer_id) {
                if let Some(relay_info) = &neighbor.relay_info {
                    return relay_info.congestion_level <= self.config.max_congestion_level;
                }
            }
            true
        });

        // Sort by total score (descending)
        scored_neighbors.sort_by(|a, b| {
            b.1.total
                .partial_cmp(&a.1.total)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top K candidates
        let candidates: Vec<String> = scored_neighbors
            .into_iter()
            .take(self.config.forward_to_top_k)
            .map(|(peer_id, _)| peer_id)
            .collect();

        // Apply probabilistic filtering if gossip is active
        let initial_count = candidates.len();
        let message_id = message.id.as_str();
        let peers: Vec<String> = if gossip_applied {
            candidates
                .into_iter()
                .filter(|peer_id| {
                    self.should_forward_to_peer(&message_id, peer_id.as_str(), probability)
                })
                .collect()
        } else {
            candidates
        };

        let dropped_count = initial_count.saturating_sub(peers.len());

        ForwardingDecision {
            peers,
            gossip_applied,
            probability,
            dropped_count,
        }
    }

    /// Selects the best path(s) for message delivery.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to route
    /// * `neighbors` - Available neighboring devices
    ///
    /// # Returns
    ///
    /// Returns a list of neighbors to forward the message to, ordered by preference.
    ///
    /// Note: This method does not apply gossip filtering. Use `select_paths_with_gossip`
    /// for scalable routing in large networks.
    pub fn select_paths(&self, message: &Message, neighbors: &[NeighborInfo]) -> Vec<String> {
        if neighbors.is_empty() {
            return Vec::new();
        }

        // Calculate scores for each neighbor
        let mut scored_neighbors: Vec<(String, PathScore)> = neighbors
            .iter()
            .map(|neighbor| {
                let score = self.calculate_path_score(message, neighbor);
                (neighbor.peer_id.clone(), score)
            })
            .collect();

        // Filter out neighbors with relay congestion > max threshold
        scored_neighbors.retain(|(peer_id, _)| {
            if let Some(neighbor) = neighbors.iter().find(|n| n.peer_id == *peer_id) {
                if let Some(relay_info) = &neighbor.relay_info {
                    return relay_info.congestion_level <= self.config.max_congestion_level;
                }
            }
            true
        });

        // Sort by total score (descending)
        scored_neighbors.sort_by(|a, b| {
            b.1.total
                .partial_cmp(&a.1.total)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Select top K neighbors for redundancy
        scored_neighbors
            .into_iter()
            .take(self.config.forward_to_top_k)
            .map(|(peer_id, _)| peer_id)
            .collect()
    }

    /// Calculates the path score for a neighbor.
    fn calculate_path_score(&self, message: &Message, neighbor: &NeighborInfo) -> PathScore {
        let signal_score = self.calculate_signal_score(neighbor);
        let proximity_score = self.calculate_proximity_score(message, neighbor);
        let capacity_score = self.calculate_capacity_score(neighbor);
        let energy_score = self.calculate_energy_score(neighbor);

        let total = (signal_score * SIGNAL_WEIGHT)
            + (proximity_score * PROXIMITY_WEIGHT)
            + (capacity_score * CAPACITY_WEIGHT)
            + (energy_score * ENERGY_WEIGHT);

        PathScore {
            signal: signal_score,
            proximity: proximity_score,
            capacity: capacity_score,
            energy: energy_score,
            total,
        }
    }

    /// Calculates signal strength score from RSSI.
    fn calculate_signal_score(&self, neighbor: &NeighborInfo) -> f32 {
        let rssi = neighbor.rssi;

        if rssi >= EXCELLENT_RSSI_THRESHOLD {
            EXCELLENT_SIGNAL_SCORE
        } else if rssi >= GOOD_RSSI_THRESHOLD {
            GOOD_SIGNAL_BASE
                + ((rssi - GOOD_RSSI_THRESHOLD) as f32 * 30.0
                    / (EXCELLENT_RSSI_THRESHOLD - GOOD_RSSI_THRESHOLD) as f32)
        } else if rssi >= FAIR_RSSI_THRESHOLD {
            FAIR_SIGNAL_BASE
                + ((rssi - FAIR_RSSI_THRESHOLD) as f32 * 30.0
                    / (GOOD_RSSI_THRESHOLD - FAIR_RSSI_THRESHOLD) as f32)
        } else {
            ((rssi - POOR_RSSI_MAX).max(0) as f32 * FAIR_SIGNAL_BASE
                / (FAIR_RSSI_THRESHOLD - POOR_RSSI_MAX) as f32)
                .max(0.0)
        }
    }

    /// Calculates proximity score based on hop distance.
    fn calculate_proximity_score(&self, message: &Message, neighbor: &NeighborInfo) -> f32 {
        if let Some(hops) = neighbor.hops_to_destination {
            // We know the distance to destination through this neighbor
            let remaining_ttl = message.ttl.value();

            if hops == 0 {
                // Direct connection to destination
                100.0
            } else {
                // Score based on whether we have enough TTL
                let score = if hops < remaining_ttl {
                    100.0 - (hops as f32 / remaining_ttl as f32 * 50.0)
                } else {
                    // Not enough TTL, but still possible
                    20.0
                };
                score.max(0.0)
            }
        } else {
            // Unknown distance, use link quality as proxy
            neighbor.link_quality as f32
        }
    }

    /// Calculates capacity score based on relay status and congestion.
    fn calculate_capacity_score(&self, neighbor: &NeighborInfo) -> f32 {
        if let Some(relay_info) = &neighbor.relay_info {
            let relay_score = self.relay_manager.calculate_relay_score(relay_info);
            relay_score.min(EXCELLENT_SIGNAL_SCORE)
        } else {
            NON_RELAY_BASIC_CAPACITY
        }
    }

    /// Calculates energy score based on battery level.
    fn calculate_energy_score(&self, neighbor: &NeighborInfo) -> f32 {
        if let Some(relay_info) = &neighbor.relay_info {
            if relay_info.is_charging {
                CHARGING_BATTERY_SCORE
            } else {
                relay_info.battery_level as f32
            }
        } else {
            NON_RELAY_ASSUMED_BATTERY
        }
    }

    /// Finds the best single path (for unicast).
    pub fn select_best_path(
        &self,
        message: &Message,
        neighbors: &[NeighborInfo],
    ) -> Option<String> {
        self.select_paths(message, neighbors).into_iter().next()
    }

    /// Checks if routing around congestion is needed.
    pub fn should_route_around_congestion(&self, neighbor: &NeighborInfo) -> bool {
        if let Some(relay_info) = &neighbor.relay_info {
            relay_info.congestion_level > self.config.max_congestion_level
        } else {
            false
        }
    }

    /// Gets the path configuration.
    pub fn config(&self) -> &PathConfig {
        &self.config
    }

    // MARK: - Gradient Routing Methods

    /// Learns a route from an incoming message.
    /// Call this when receiving a message from a neighbor to learn
    /// that the neighbor can reach the message's sender.
    pub fn learn_route_from_message(
        &mut self,
        message: &Message,
        from_neighbor: &str,
        quality: f32,
    ) {
        let sender = message.sender.as_str();
        let hop_count = message.hop_count.value();
        self.routing_table.learn_route(
            sender,
            from_neighbor,
            hop_count,
            quality,
            0, // No destination sequence in message yet; use 0 for backward compatibility
        );
    }

    /// Gets the best route to a destination, if known.
    pub fn get_route_to(&self, destination: &str) -> Option<&RouteEntry> {
        self.routing_table.get_route(destination)
    }

    /// Checks if we have a known route to a destination.
    pub fn has_route_to(&self, destination: &str) -> bool {
        self.routing_table.has_route(destination)
    }

    /// Selects the best path for a message using gradient routing when available.
    /// Falls back to flooding if no route is known.
    pub fn select_directed_path(
        &self,
        message: &Message,
        neighbors: &[NeighborInfo],
    ) -> Option<String> {
        let recipient = message.recipient.as_str();

        // Check if any neighbor IS the recipient
        for neighbor in neighbors {
            if neighbor.peer_id == recipient {
                return Some(neighbor.peer_id.clone());
            }
        }

        // Check if we have a learned route
        if let Some(route) = self.routing_table.get_route(recipient) {
            // Verify the next hop is in our current neighbors
            if neighbors.iter().any(|n| n.peer_id == route.next_hop) {
                return Some(route.next_hop.clone());
            }
        }

        // No direct route known, fall back to best path selection
        self.select_best_path(message, neighbors)
    }

    /// Removes a neighbor from the routing table (e.g., on disconnect).
    pub fn remove_neighbor_routes(&mut self, neighbor: &str) {
        self.routing_table.remove_neighbor(neighbor);
    }

    /// Cleans up expired routes.
    pub fn cleanup_routes(&mut self) {
        self.routing_table.cleanup_expired();
    }

    /// Returns routing table statistics.
    pub fn routing_stats(&self) -> (usize, usize) {
        (
            self.routing_table.destination_count(),
            self.routing_table.route_count(),
        )
    }

    /// Gets a mutable reference to the routing table for advanced operations.
    pub fn routing_table_mut(&mut self) -> &mut GradientRoutingTable {
        &mut self.routing_table
    }
}

impl Default for PathSelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::{RelayInfo, RelayRole};
    use offline_protocol_core::{AppId, UserId};

    fn create_test_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "Test message",
        )
    }

    fn create_neighbor(id: &str, rssi: i16, hops: Option<u8>, congestion: f32) -> NeighborInfo {
        NeighborInfo {
            peer_id: id.to_string(),
            rssi,
            hops_to_destination: hops,
            link_quality: 80,
            relay_info: Some(RelayInfo {
                connection_count: 5,
                battery_level: 70,
                is_charging: false,
                role: RelayRole::Relay,
                link_quality: 80,
                queue_depth: 10,
                congestion_level: congestion,
            }),
        }
    }

    #[test]
    fn test_path_selection_basic() {
        let selector = PathSelector::new();
        let message = create_test_message();

        let neighbors = vec![
            create_neighbor("peer1", -60, Some(2), 0.2),
            create_neighbor("peer2", -70, Some(3), 0.3),
            create_neighbor("peer3", -55, Some(1), 0.1),
        ];

        let paths = selector.select_paths(&message, &neighbors);

        // Should select top K paths
        assert!(!paths.is_empty());
        assert!(paths.len() <= 3);

        // Best path should be peer3 (best RSSI, lowest hops, lowest congestion)
        assert_eq!(paths[0], "peer3");
    }

    #[test]
    fn test_congestion_filtering() {
        let selector = PathSelector::new();
        let message = create_test_message();

        let neighbors = vec![
            create_neighbor("peer1", -60, Some(2), 0.9), // High congestion
            create_neighbor("peer2", -65, Some(3), 0.3),
        ];

        let paths = selector.select_paths(&message, &neighbors);

        // Should filter out peer1 due to high congestion
        assert!(!paths.contains(&"peer1".to_string()));
    }

    #[test]
    fn test_direct_destination_preferred() {
        let selector = PathSelector::new();
        let message = create_test_message();

        let neighbors = vec![
            create_neighbor("peer1", -70, Some(5), 0.3),
            create_neighbor("peer2", -70, Some(0), 0.1), // Direct to destination
        ];

        let paths = selector.select_paths(&message, &neighbors);

        // Should prefer peer2 (direct destination) with same signal
        assert_eq!(paths[0], "peer2");
    }

    #[test]
    fn test_signal_score_calculation() {
        let selector = PathSelector::new();

        let excellent = create_neighbor("peer1", -40, None, 0.1);
        let score = selector.calculate_signal_score(&excellent);
        assert_eq!(score, 100.0);

        let poor = create_neighbor("peer2", -90, None, 0.1);
        let score = selector.calculate_signal_score(&poor);
        assert!(score < 40.0);
    }

    #[test]
    fn test_empty_neighbors() {
        let selector = PathSelector::new();
        let message = create_test_message();

        let paths = selector.select_paths(&message, &[]);
        assert!(paths.is_empty());
    }

    #[test]
    fn test_select_best_path() {
        let selector = PathSelector::new();
        let message = create_test_message();

        let neighbors = vec![
            create_neighbor("peer1", -60, Some(2), 0.2),
            create_neighbor("peer2", -55, Some(1), 0.1),
        ];

        let best = selector.select_best_path(&message, &neighbors);
        assert_eq!(best, Some("peer2".to_string()));
    }

    #[test]
    fn test_route_around_congestion() {
        let selector = PathSelector::new();

        let congested = create_neighbor("peer1", -60, Some(2), 0.9);
        assert!(selector.should_route_around_congestion(&congested));

        let normal = create_neighbor("peer2", -60, Some(2), 0.3);
        assert!(!selector.should_route_around_congestion(&normal));
    }

    #[test]
    fn test_gossip_probability_small_network() {
        let selector = PathSelector::new();

        // Below threshold (10), probability should be 1.0
        assert_eq!(selector.compute_forwarding_probability(5), 1.0);
        assert_eq!(selector.compute_forwarding_probability(10), 1.0);
    }

    #[test]
    fn test_gossip_probability_large_network() {
        let selector = PathSelector::new();

        // Above threshold, probability should scale down
        // target_fanout = 4, so at 40 peers: probability = 4/40 = 0.1
        // But min_probability = 0.15, so it should be 0.15
        let prob = selector.compute_forwarding_probability(40);
        assert!((prob - 0.15).abs() < 0.01);

        // At 20 peers: probability = 4/20 = 0.2
        let prob = selector.compute_forwarding_probability(20);
        assert!((prob - 0.2).abs() < 0.01);

        // At 100 peers: probability = 4/100 = 0.04, but min is 0.15
        let prob = selector.compute_forwarding_probability(100);
        assert!((prob - 0.15).abs() < 0.01);
    }

    #[test]
    fn test_gossip_disabled() {
        let config = PathConfig {
            gossip: GossipConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let selector = PathSelector::with_config(config, RelayManager::new());

        // Even with 1000 peers, probability should be 1.0 when disabled
        assert_eq!(selector.compute_forwarding_probability(1000), 1.0);
    }

    #[test]
    fn test_gossip_forwarding_deterministic() {
        let mut selector = PathSelector::new();
        selector.set_local_device_id("device1");

        let message = create_test_message();
        let neighbors = vec![
            create_neighbor("peer1", -60, Some(2), 0.2),
            create_neighbor("peer2", -60, Some(2), 0.2),
            create_neighbor("peer3", -60, Some(2), 0.2),
        ];

        // Same input should produce same output
        let decision1 = selector.select_paths_with_gossip(&message, &neighbors, 50);
        let decision2 = selector.select_paths_with_gossip(&message, &neighbors, 50);

        assert_eq!(decision1.peers, decision2.peers);
        assert!(decision1.gossip_applied);
    }

    #[test]
    fn test_gossip_priority_bypass() {
        use offline_protocol_core::MessagePriority;

        let mut selector = PathSelector::new();
        selector.set_local_device_id("device1");

        let high_priority_message = Message::builder(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
        )
        .content("Test")
        .priority(MessagePriority::High)
        .build();

        let neighbors = vec![
            create_neighbor("peer1", -60, Some(2), 0.2),
            create_neighbor("peer2", -60, Some(2), 0.2),
        ];

        // High priority should bypass gossip
        let decision = selector.select_paths_with_gossip(&high_priority_message, &neighbors, 100);
        assert!(!decision.gossip_applied);
        assert_eq!(decision.probability, 1.0);
    }

    #[test]
    fn test_gossip_reduces_forwarding_count() {
        let mut selector = PathSelector::new();
        selector.set_local_device_id("test-device");

        let neighbors: Vec<NeighborInfo> = (0..20)
            .map(|i| create_neighbor(&format!("peer{}", i), -60, Some(2), 0.2))
            .collect();

        let message = create_test_message();

        // With 100 visible peers and target_fanout=4, probability ≈ 0.15
        // So we should forward to fewer peers on average
        let decision = selector.select_paths_with_gossip(&message, &neighbors, 100);

        // Gossip should be applied
        assert!(decision.gossip_applied);
        // Probability should be at minimum (0.15)
        assert!((decision.probability - 0.15).abs() < 0.01);
    }

    // ---------- DSDV sequence number tests (loop-free routing) ----------

    #[test]
    fn test_dsdv_prefer_higher_sequence_number() {
        let mut table = GradientRoutingTable::new();
        let dest = "alice";

        table.learn_route(dest, "peer_a", 2, 0.9, 1);
        table.learn_route(dest, "peer_b", 3, 0.8, 2);

        let route = table.get_route(dest).expect("should have route");
        assert_eq!(route.next_hop, "peer_b");
        assert_eq!(route.sequence_number, 2);
    }

    #[test]
    fn test_dsdv_reject_stale_update_same_next_hop() {
        let mut table = GradientRoutingTable::new();
        let dest = "bob";

        table.learn_route(dest, "peer_a", 2, 0.9, 2);
        table.learn_route(dest, "peer_a", 5, 0.5, 1); // stale: lower sequence

        let route = table.get_route(dest).expect("should have route");
        assert_eq!(route.next_hop, "peer_a");
        assert_eq!(route.hop_count, 2);
        assert_eq!(route.sequence_number, 2);
    }

    #[test]
    fn test_dsdv_accept_same_sequence_better_hop_count() {
        let mut table = GradientRoutingTable::new();
        let dest = "carol";

        table.learn_route(dest, "peer_a", 4, 0.7, 2);
        table.learn_route(dest, "peer_a", 2, 0.8, 2); // same seq, better hop count

        let route = table.get_route(dest).expect("should have route");
        assert_eq!(route.hop_count, 2);
        assert_eq!(route.sequence_number, 2);
    }

    #[test]
    fn test_global_eviction_prefers_low_quality_routes() {
        let config = GradientRoutingConfig {
            max_routing_table_size: 3,
            max_routes_per_destination: 3,
            route_ttl_secs: 300,
            ..Default::default()
        };
        let mut table = GradientRoutingTable::with_config(config);

        // Insert a high-quality recent route
        table.learn_route("alice", "hop_good", 1, 0.9, 0);
        // Insert two low-quality routes
        table.learn_route("bob", "hop_low1", 3, 0.1, 0);
        table.learn_route("carol", "hop_low2", 4, 0.1, 0);

        assert_eq!(table.route_count(), 3);

        // Adding a 4th route should evict the lowest composite score route
        table.learn_route("dave", "hop_new", 2, 0.5, 0);
        assert_eq!(table.route_count(), 3);

        // The high-quality route to alice should survive
        assert!(
            table.has_route("alice"),
            "High-quality route to alice should survive eviction"
        );
        // dave was just inserted, should exist
        assert!(table.has_route("dave"), "Newly inserted route should exist");
    }

    #[test]
    fn test_dsdv_eviction_removes_worst_route() {
        let config = GradientRoutingConfig {
            max_routes_per_destination: 2,
            ..Default::default()
        };
        let mut table = GradientRoutingTable::with_config(config);
        let dest = "dest";

        table.learn_route(dest, "hop1", 1, 0.9, 10);
        table.learn_route(dest, "hop2", 2, 0.8, 10);
        // At capacity: evict worst of current two (hop2), then add hop3
        table.learn_route(dest, "hop3", 3, 0.7, 5);

        let route = table.get_route(dest).expect("should have route");
        assert_eq!(route.next_hop, "hop1");
        let routes = table.get_routes(dest);
        assert_eq!(routes.len(), 2);
        assert!(routes.iter().any(|r| r.next_hop == "hop1"));
        assert!(routes.iter().any(|r| r.next_hop == "hop3"));
        assert!(
            !routes.iter().any(|r| r.next_hop == "hop2"),
            "hop2 (worst of original two) should be evicted"
        );
    }

    // ---------- RFC 1982 sequence number wrapping tests ----------

    #[test]
    fn test_seq_is_newer_basic() {
        assert!(seq_is_newer(2, 1));
        assert!(seq_is_newer(100, 50));
        assert!(!seq_is_newer(1, 2));
        assert!(!seq_is_newer(5, 5)); // equal is not newer
    }

    #[test]
    fn test_seq_is_newer_wrapping() {
        // Near u32::MAX wrapping to 0
        assert!(seq_is_newer(0, u32::MAX));
        assert!(seq_is_newer(1, u32::MAX));
        assert!(seq_is_newer(5, u32::MAX - 2));
        // Going backwards across the wrap should not be considered newer
        assert!(!seq_is_newer(u32::MAX, 0));
        assert!(!seq_is_newer(u32::MAX - 2, 5));
    }

    #[test]
    fn test_dsdv_wrapping_sequence_update() {
        let mut table = GradientRoutingTable::new();
        let dest = "alice";

        // Learn a route with seq near MAX
        table.learn_route(dest, "peer_a", 2, 0.9, u32::MAX - 1);

        // Update with wrapped sequence (0 is newer than MAX-1)
        table.learn_route(dest, "peer_a", 2, 0.9, 0);

        let route = table.get_route(dest).expect("should have route");
        assert_eq!(
            route.sequence_number, 0,
            "Wrapped sequence should be accepted"
        );
    }

    #[test]
    fn test_seq_cmp_basic_and_wrapping() {
        use std::cmp::Ordering;
        assert_eq!(seq_cmp(5, 5), Ordering::Equal);
        assert_eq!(seq_cmp(10, 5), Ordering::Greater);
        assert_eq!(seq_cmp(5, 10), Ordering::Less);
        // Wrapping: 0 is newer than MAX
        assert_eq!(seq_cmp(0, u32::MAX), Ordering::Greater);
        assert_eq!(seq_cmp(u32::MAX, 0), Ordering::Less);
    }

    #[test]
    fn test_get_route_prefers_wrapped_sequence() {
        let mut table = GradientRoutingTable::new();
        let dest = "alice";

        // Route A has seq near MAX, route B wrapped to 1
        table.learn_route(dest, "peer_old", 2, 0.9, u32::MAX - 1);
        table.learn_route(dest, "peer_new", 3, 0.7, 1);

        let route = table.get_route(dest).expect("should have route");
        assert_eq!(
            route.next_hop, "peer_new",
            "get_route must prefer wrapped (newer) sequence number"
        );
        assert_eq!(route.sequence_number, 1);
    }

    #[test]
    fn test_per_dest_eviction_respects_wrapping() {
        let config = GradientRoutingConfig {
            max_routes_per_destination: 2,
            ..Default::default()
        };
        let mut table = GradientRoutingTable::with_config(config);
        let dest = "bob";

        // Two routes with pre-wrap sequences
        table.learn_route(dest, "hop_old", 2, 0.8, u32::MAX - 5);
        table.learn_route(dest, "hop_mid", 2, 0.8, u32::MAX);

        // Third route with wrapped sequence — should evict the oldest (MAX-5)
        table.learn_route(dest, "hop_new", 2, 0.8, 2);

        let routes = table.get_routes(dest);
        assert_eq!(routes.len(), 2);
        assert!(
            routes.iter().any(|r| r.next_hop == "hop_new"),
            "Wrapped-sequence route must survive"
        );
        assert!(
            routes.iter().any(|r| r.next_hop == "hop_mid"),
            "Second-best sequence route must survive"
        );
        assert!(
            !routes.iter().any(|r| r.next_hop == "hop_old"),
            "Oldest sequence route should be evicted"
        );
    }

    #[test]
    fn test_seq_is_newer_half_space_boundary() {
        // At exactly half the sequence space, RFC 1982 says the result is
        // undefined. Our implementation treats it as "not newer".
        let half = 1u32 << 31;
        assert!(
            !seq_is_newer(half, 0),
            "Half-space distance is ambiguous, should not be newer"
        );
        assert!(!seq_is_newer(0, half), "Reverse half-space also not newer");
    }
}
