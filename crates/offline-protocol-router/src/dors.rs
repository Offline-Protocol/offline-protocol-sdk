//! DORS (Dynamic Offline Relay Switch) transport selection.
//!
//! This module implements the intelligent transport selection algorithm
//! that automatically chooses and switches between Internet, BLE Mesh, and Wi-Fi Direct
//! based on real-time network conditions.

use chrono::{DateTime, Duration, Utc};
use offline_protocol_core::Message;
use offline_protocol_transport::{TransportMetrics, TransportType};
use std::collections::{HashMap, VecDeque};
use tracing::debug;

/// Configuration for DORS transport selection.
#[derive(Debug, Clone)]
pub struct DorsConfig {
    /// Minimum score improvement required to switch transports (hysteresis).
    pub switch_hysteresis: f32,

    /// Cooldown period after switching (seconds).
    pub switch_cooldown_secs: u64,

    /// Number of retry failures before escalating from BLE to Wi-Fi Direct.
    pub ble_to_wifi_retry_threshold: u32,

    /// RSSI threshold for switching from BLE to Wi-Fi Direct (dBm).
    pub rssi_switch_threshold: i16,

    /// Queue depth threshold for detecting congestion.
    pub congestion_queue_threshold: usize,

    /// Duration for checking stability before switching (seconds).
    pub stability_window_secs: u64,

    /// Duration that RSSI must remain below the threshold before escalating (seconds).
    pub poor_signal_duration_secs: u64,

    /// TTL threshold that indicates messages are nearing exhaustion.
    pub ttl_escalation_threshold: u8,

    /// Whether to prefer online/Internet transport first.
    pub prefer_online: bool,

    /// Duration that queue congestion must persist before escalating (seconds).
    pub congestion_duration_secs: u64,

    /// Duration to keep TTL escalation signal active after detection (seconds).
    pub ttl_escalation_hold_secs: u64,

    /// Number of historical samples to retain per transport for smoothing metrics.
    pub history_window_size: usize,

    /// Ratio of `congestion_queue_threshold` that indicates recovery (0.0-1.0).
    pub queue_recovery_ratio: f32,

    /// Battery percentage considered low for energy-aware decisions.
    pub low_battery_threshold: u8,

    /// Minimum battery percentage required to escalate to high-power transports when not charging.
    pub relay_min_battery_level: u8,

    /// Target number of relay connections before considering the relay saturated.
    pub relay_optimal_connection_count: u8,
}

impl Default for DorsConfig {
    fn default() -> Self {
        Self {
            switch_hysteresis: 15.0,
            switch_cooldown_secs: 20,
            ble_to_wifi_retry_threshold: 2,
            rssi_switch_threshold: -85,
            congestion_queue_threshold: 50,
            stability_window_secs: 8,
            poor_signal_duration_secs: 10,
            ttl_escalation_threshold: 2,
            prefer_online: false,
            congestion_duration_secs: 10,
            ttl_escalation_hold_secs: 20,
            history_window_size: 10,
            queue_recovery_ratio: 0.5,
            low_battery_threshold: 20,
            relay_min_battery_level: 30,
            relay_optimal_connection_count: 4,
        }
    }
}

/// Score breakdown for transport selection.
#[derive(Debug, Clone)]
pub struct TransportScore {
    /// Signal strength score (0-100).
    pub signal: f32,
    /// Proximity/hop distance score (0-100).
    pub proximity: f32,
    /// Available bandwidth score (0-100).
    pub bandwidth: f32,
    /// Congestion score (0-100, higher is less congested).
    pub congestion: f32,
    /// Energy efficiency score (0-100).
    pub energy: f32,
    /// Reliability score (0-100).
    pub reliability: f32,
    /// Load score (0-100, higher = more capacity available).
    pub load: f32,
    /// Total weighted score.
    pub total: f32,
}

#[derive(Default, Debug)]
struct TransportHistory {
    queue_depths: VecDeque<usize>,
    congestion_levels: VecDeque<f32>,
    rssi_samples: VecDeque<i16>,
    success_ratios: VecDeque<f32>,
    latency_samples: VecDeque<u32>,
}

impl TransportHistory {
    fn push_queue_depth(&mut self, depth: usize, limit: usize) {
        self.queue_depths.push_back(depth);
        self.truncate(limit);
    }

    fn push_congestion(&mut self, level: f32, limit: usize) {
        self.congestion_levels.push_back(level);
        self.truncate(limit);
    }

    fn push_rssi(&mut self, rssi: i16, limit: usize) {
        self.rssi_samples.push_back(rssi);
        self.truncate(limit);
    }

    fn push_success_ratio(&mut self, ratio: f32, limit: usize) {
        self.success_ratios.push_back(ratio.clamp(0.0, 1.0));
        self.truncate(limit);
    }

    fn push_latency(&mut self, latency: u32, limit: usize) {
        self.latency_samples.push_back(latency);
        self.truncate(limit);
    }

    fn truncate(&mut self, limit: usize) {
        let limit = limit.max(1);
        while self.queue_depths.len() > limit {
            self.queue_depths.pop_front();
        }
        while self.congestion_levels.len() > limit {
            self.congestion_levels.pop_front();
        }
        while self.rssi_samples.len() > limit {
            self.rssi_samples.pop_front();
        }
        while self.success_ratios.len() > limit {
            self.success_ratios.pop_front();
        }
        while self.latency_samples.len() > limit {
            self.latency_samples.pop_front();
        }
    }

    fn average_queue_depth(&self) -> Option<f32> {
        average_usize(&self.queue_depths)
    }

    fn average_congestion(&self) -> Option<f32> {
        average_f32(&self.congestion_levels)
    }

    fn average_rssi(&self) -> Option<f32> {
        if self.rssi_samples.is_empty() {
            None
        } else {
            Some(
                self.rssi_samples.iter().map(|v| *v as f32).sum::<f32>()
                    / self.rssi_samples.len() as f32,
            )
        }
    }

    fn average_success_ratio(&self) -> Option<f32> {
        average_f32(&self.success_ratios)
    }
}

fn average_usize(samples: &VecDeque<usize>) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    Some(samples.iter().sum::<usize>() as f32 / samples.len() as f32)
}

fn average_f32(samples: &VecDeque<f32>) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    Some(samples.iter().sum::<f32>() / samples.len() as f32)
}

/// Transport selector for DORS.
///
/// This struct implements the intelligent transport selection algorithm
/// that chooses between Internet, BLE, and Wi-Fi Direct based on network conditions.
pub struct TransportSelector {
    config: DorsConfig,
    current_transport: Option<TransportType>,
    last_switch_time: Option<DateTime<Utc>>,
    transport_scores_history: HashMap<TransportType, Vec<(DateTime<Utc>, f32)>>,
    transport_history: HashMap<TransportType, TransportHistory>,
    retry_counts: HashMap<TransportType, u32>,
    last_metrics: HashMap<TransportType, (TransportMetrics, DateTime<Utc>)>,
    ble_poor_signal_since: Option<DateTime<Utc>>,
    ble_high_congestion_since: Option<DateTime<Utc>>,
    low_ttl_detected_at: Option<DateTime<Utc>>,
}

impl TransportSelector {
    /// Creates a new transport selector with default configuration.
    pub fn new() -> Self {
        Self::with_config(DorsConfig::default())
    }

    /// Creates a new transport selector with custom configuration.
    pub fn with_config(config: DorsConfig) -> Self {
        Self {
            config,
            current_transport: None,
            last_switch_time: None,
            transport_scores_history: HashMap::new(),
            transport_history: HashMap::new(),
            retry_counts: HashMap::new(),
            last_metrics: HashMap::new(),
            ble_poor_signal_since: None,
            ble_high_congestion_since: None,
            low_ttl_detected_at: None,
        }
    }

    /// Selects the best transport for sending a message.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to send
    /// * `available_transports` - Available transport types and their metrics
    ///
    /// # Returns
    ///
    /// Returns the recommended transport type.
    pub fn select_transport(
        &mut self,
        message: &Message,
        available_transports: &HashMap<TransportType, TransportMetrics>,
    ) -> Option<TransportType> {
        // If no transports available, return None
        if available_transports.is_empty() {
            return None;
        }

        // Calculate scores for all available transports
        let mut scored_transports: Vec<(TransportType, TransportScore)> = Vec::new();

        for (transport_type, metrics) in available_transports.iter() {
            self.record_metrics(*transport_type, metrics);
            self.update_history(*transport_type, metrics);

            if *transport_type == TransportType::BLE {
                self.update_ble_conditions(message, metrics);
            }

            let score = self.calculate_transport_score(message, *transport_type, metrics);
            self.record_score(*transport_type, score.total);
            scored_transports.push((*transport_type, score));
        }

        // Sort by total score (descending)
        scored_transports.sort_by(|a, b| {
            b.1.total
                .partial_cmp(&a.1.total)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Get the best transport
        let best = scored_transports.first()?;
        let best_transport = best.0;
        let best_score = best.1.total;

        // Log all scored transports for observability.
        // Guard allocation behind level check to avoid Vec<String> on every call.
        if tracing::enabled!(tracing::Level::DEBUG) {
            let scores_summary: Vec<_> = scored_transports
                .iter()
                .map(|(t, s)| format!("{:?}={:.1}", t, s.total))
                .collect();
            debug!(
                scores = %scores_summary.join(", "),
                best = ?best_transport,
                previous = ?self.current_transport,
                prefer_online = self.config.prefer_online,
                "DORS scored transports"
            );
        }

        if let Some(current) = self.current_transport {
            // If current transport is no longer available, switch immediately
            // regardless of cooldown or hysteresis (e.g. Internet disconnected).
            let current_still_available = available_transports.contains_key(&current);

            if current_still_available {
                if current == best_transport {
                    return Some(current);
                }

                if let Some(current_score) = scored_transports
                    .iter()
                    .find(|(t, _)| *t == current)
                    .map(|(_, s)| s.total)
                {
                    if !self.should_switch(current, current_score, best_transport, best_score) {
                        debug!(
                            current = ?current,
                            current_score = current_score,
                            candidate = ?best_transport,
                            candidate_score = best_score,
                            "DORS: switch blocked by hysteresis/cooldown/stability"
                        );
                        return Some(current);
                    }
                }
            } else {
                debug!(
                    lost = ?current,
                    fallback = ?best_transport,
                    "DORS: current transport unavailable, switching immediately"
                );
            }
        }

        // Track switch
        debug!(
            from = ?self.current_transport,
            to = ?best_transport,
            score = best_score,
            "DORS: transport switched"
        );
        self.last_switch_time = Some(Utc::now());
        self.current_transport = Some(best_transport);

        Some(best_transport)
    }

    /// Scores all available transports and returns them ranked by score (descending).
    ///
    /// Unlike `select_transport`, this does not apply hysteresis or switch the
    /// current transport. It is used by `TransportManager::send()` to build a
    /// fallback list when the primary transport's send fails.
    pub fn score_and_rank(
        &mut self,
        message: &Message,
        available_transports: &HashMap<TransportType, TransportMetrics>,
    ) -> Vec<(TransportType, f32)> {
        let mut scored: Vec<(TransportType, f32)> = Vec::new();

        for (transport_type, metrics) in available_transports.iter() {
            self.record_metrics(*transport_type, metrics);
            self.update_history(*transport_type, metrics);

            if *transport_type == TransportType::BLE {
                self.update_ble_conditions(message, metrics);
            }

            let score = self.calculate_transport_score(message, *transport_type, metrics);
            self.record_score(*transport_type, score.total);
            scored.push((*transport_type, score.total));
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }

    /// Calculates the transport score based on multiple factors.
    fn calculate_transport_score(
        &self,
        message: &Message,
        transport_type: TransportType,
        metrics: &TransportMetrics,
    ) -> TransportScore {
        let signal_score = self.calculate_signal_score(transport_type, metrics);
        let proximity_score = self.calculate_proximity_score(message);
        let bandwidth_score = self.calculate_bandwidth_score(transport_type, metrics);
        let congestion_score = self.calculate_congestion_score(transport_type, metrics);
        let energy_score = self.calculate_energy_score(transport_type, metrics);
        let reliability_score = self.calculate_reliability_score(transport_type, metrics);
        let load_score = self.calculate_load_score(transport_type, metrics);

        // Weighted combination based on DORS specification
        let total = match transport_type {
            TransportType::Internet => {
                // Internet prioritises bandwidth and reliability.
                // Give Internet a baseline advantage when connected to ensure it's competitive.
                // Additional boost when prefer_online is enabled.
                let baseline = if self.config.prefer_online {
                    25.0
                } else {
                    10.0
                };
                baseline
                    + (bandwidth_score * 0.35)
                    + (reliability_score * 0.3)
                    + (congestion_score * 0.15)
                    + (energy_score * 0.1)
                    + (load_score * 0.1)
            }
            TransportType::BLE => {
                // BLE: balanced for signal, energy, congestion, proximity, reliability, and available capacity
                (signal_score * 0.3)
                    + (energy_score * 0.3)
                    + (congestion_score * 0.15)
                    + (proximity_score * 0.15)
                    + (reliability_score * 0.05)
                    + (load_score * 0.05)
            }
            TransportType::WiFiDirect => {
                // Wi-Fi Direct: prefer bandwidth, proximity, congestion, reliability, and available capacity
                (bandwidth_score * 0.35)
                    + (proximity_score * 0.2)
                    + (congestion_score * 0.2)
                    + (reliability_score * 0.15)
                    + (load_score * 0.1)
            }
        };

        // Floor at 0 but do not cap at 100 — the prefer_online baseline
        // intentionally pushes Internet above the weighted-sum ceiling so
        // the gap exceeds the switch hysteresis.
        let total = total.max(0.0);

        TransportScore {
            signal: signal_score,
            proximity: proximity_score,
            bandwidth: bandwidth_score,
            congestion: congestion_score,
            energy: energy_score,
            reliability: reliability_score,
            load: load_score,
            total,
        }
    }

    /// Determines whether the selector should switch from `current_transport` to `candidate_transport`.
    fn should_switch(
        &self,
        current_transport: TransportType,
        current_score: f32,
        candidate_transport: TransportType,
        candidate_score: f32,
    ) -> bool {
        if candidate_transport == current_transport {
            return false;
        }

        // Emergency bypass: if current transport has critical degradation, switch immediately
        if self.is_emergency_switch_needed(current_transport) {
            return candidate_score > 10.0; // Any reasonably scored candidate is acceptable
        }

        if candidate_score <= current_score {
            return false;
        }

        let improvement = candidate_score - current_score;
        if improvement < self.config.switch_hysteresis {
            return false;
        }

        if !self.is_past_cooldown() {
            return false;
        }

        self.is_stable_better(
            candidate_transport,
            current_transport,
            self.config.stability_window_secs,
            self.config.switch_hysteresis / 2.0,
        )
    }

    /// Checks if the current transport has critical degradation requiring emergency switch.
    /// This bypasses normal hysteresis and cooldown for:
    /// - Transport unavailable (status != Available)
    /// - Success rate below 30%
    /// - Consecutive failures exceeding threshold
    fn is_emergency_switch_needed(&self, transport: TransportType) -> bool {
        // Check retry failure count
        let retry_failures = self.retry_counts.get(&transport).copied().unwrap_or(0);
        if retry_failures >= self.config.ble_to_wifi_retry_threshold {
            return true;
        }

        // Check historical success rate
        if let Some(history) = self.transport_history.get(&transport) {
            if let Some(success_rate) = history.average_success_ratio() {
                if success_rate < 0.30 {
                    return true;
                }
            }
        }

        // Check for prolonged poor signal (for wireless transports)
        if matches!(transport, TransportType::BLE | TransportType::WiFiDirect) {
            if let Some((metrics, _)) = self.last_metrics.get(&transport) {
                if let Some(rssi) = metrics.rssi {
                    // Very poor signal for extended period
                    if rssi < -90 {
                        let poor_duration = self
                            .ble_poor_signal_since
                            .map(|since| Utc::now().signed_duration_since(since).num_seconds());
                        if poor_duration.unwrap_or(0)
                            >= self.config.poor_signal_duration_secs as i64 * 2
                        {
                            return true;
                        }
                    }
                }
            }
        }

        false
    }

    /// Calculates signal strength score from RSSI.
    fn calculate_signal_score(
        &self,
        transport_type: TransportType,
        metrics: &TransportMetrics,
    ) -> f32 {
        if !matches!(
            transport_type,
            TransportType::BLE | TransportType::WiFiDirect
        ) {
            // Non-radio transports do not rely on RSSI
            return 60.0;
        }

        let rssi_value = metrics.rssi.map(|r| r as f32).or_else(|| {
            self.transport_history
                .get(&transport_type)
                .and_then(|history| history.average_rssi())
        });

        let rssi_value = match rssi_value {
            Some(value) => value,
            None => return 50.0,
        };

        if rssi_value >= -50.0 {
            100.0
        } else if rssi_value >= -70.0 {
            70.0 + ((rssi_value + 70.0) * 30.0 / 20.0)
        } else if rssi_value >= -85.0 {
            40.0 + ((rssi_value + 85.0) * 30.0 / 15.0)
        } else {
            ((rssi_value + 100.0).max(0.0) * 40.0 / 15.0).max(0.0)
        }
    }

    /// Calculates proximity score from hop count.
    fn calculate_proximity_score(&self, message: &Message) -> f32 {
        let hop_count = message.hop_count.value();
        let ttl = message.ttl.value();

        // Lower hop count = higher score
        // If message just started (hop_count = 0), give high score
        if hop_count == 0 {
            100.0
        } else {
            // Score decreases as hop count increases
            let ratio = (ttl as f32 - hop_count as f32) / ttl as f32;
            (ratio * 100.0).max(0.0)
        }
    }

    /// Calculates bandwidth score.
    fn calculate_bandwidth_score(
        &self,
        transport_type: TransportType,
        metrics: &TransportMetrics,
    ) -> f32 {
        if let Some(bandwidth) = metrics.bandwidth_bps {
            // Normalize bandwidth to 0-100 scale
            // BLE: ~150 KB/s = 150,000 B/s
            // Wi-Fi Direct: ~2 MB/s = 2,000,000 B/s
            match transport_type {
                TransportType::BLE => (bandwidth as f32 / 150_000.0 * 100.0).min(100.0),
                TransportType::WiFiDirect => (bandwidth as f32 / 2_000_000.0 * 100.0).min(100.0),
                TransportType::Internet => 100.0, // Assume high bandwidth for Internet
            }
        } else {
            // Default scores based on typical bandwidth
            match transport_type {
                TransportType::BLE => 40.0,
                TransportType::WiFiDirect => 90.0,
                TransportType::Internet => 100.0,
            }
        }
    }

    /// Calculates congestion score (higher = less congested).
    fn calculate_congestion_score(
        &self,
        transport_type: TransportType,
        metrics: &TransportMetrics,
    ) -> f32 {
        let avg_congestion = self
            .transport_history
            .get(&transport_type)
            .and_then(|history| history.average_congestion())
            .unwrap_or(metrics.congestion);
        let congestion_level = avg_congestion.clamp(0.0, 1.0);

        // Invert: less congestion = higher score
        let base_score = (1.0 - congestion_level) * 100.0;

        // Factor in queue depth
        let threshold = self.config.congestion_queue_threshold.max(1) as f32;
        let avg_queue = self
            .transport_history
            .get(&transport_type)
            .and_then(|history| history.average_queue_depth())
            .unwrap_or(metrics.queue_depth as f32);
        let queue_pressure = (avg_queue / threshold).clamp(0.0, 1.0);
        let queue_penalty = (queue_pressure.min(1.0) * 30.0).max(0.0);

        (base_score - queue_penalty).max(0.0)
    }

    /// Calculates energy efficiency score.
    fn calculate_energy_score(
        &self,
        transport_type: TransportType,
        metrics: &TransportMetrics,
    ) -> f32 {
        let mut base = match transport_type {
            TransportType::BLE => 90.0,        // Low power baseline
            TransportType::WiFiDirect => 40.0, // High power baseline
            TransportType::Internet => 60.0,   // Medium power baseline
        };

        if let Some(cost) = metrics.energy_cost {
            // Penalise transports that advertise higher energy cost.
            let penalty = (cost * 100.0).clamp(0.0, 40.0);
            base = (base - penalty).max(0.0);
        }

        if let Some(battery) = metrics.battery_level {
            let battery = battery.min(100);
            let battery_ratio = battery as f32 / 100.0;
            let low_threshold = self.config.low_battery_threshold.max(1);

            if !metrics.is_charging {
                if battery <= low_threshold {
                    // Strongly discourage high-power transports when battery is critically low.
                    if matches!(
                        transport_type,
                        TransportType::WiFiDirect | TransportType::Internet
                    ) {
                        let deficit = (low_threshold - battery) as f32 / low_threshold as f32;
                        base = (base * (1.0 - deficit)).max(0.0);
                    } else {
                        // BLE becomes more attractive when the battery is low.
                        base = (base + (20.0 * (1.0 - battery_ratio))).clamp(0.0, 100.0);
                    }
                } else if matches!(
                    transport_type,
                    TransportType::WiFiDirect | TransportType::Internet
                ) {
                    // Slight penalty based on remaining battery.
                    let penalty = (1.0 - battery_ratio).max(0.0) * 15.0;
                    base = (base - penalty).max(0.0);
                }
            } else {
                // Being on charge makes energy-intensive transports more acceptable.
                base = (base + 10.0).min(100.0);
            }
        } else if metrics.is_charging {
            base = (base + 5.0).min(100.0);
        }

        if metrics.is_active_relay {
            // Active relays pay an additional cost; bias towards low-energy transports.
            match transport_type {
                TransportType::BLE => {
                    base = (base + 5.0).min(100.0);
                }
                TransportType::WiFiDirect => {
                    base = (base - 10.0).max(0.0);
                }
                _ => {}
            }
        }

        base.clamp(0.0, 100.0)
    }

    /// Calculates reliability score based on historical success ratios.
    fn calculate_reliability_score(
        &self,
        transport_type: TransportType,
        metrics: &TransportMetrics,
    ) -> f32 {
        let ratio = metrics
            .effective_delivery_ratio()
            .or_else(|| {
                self.transport_history
                    .get(&transport_type)
                    .and_then(|history| history.average_success_ratio())
            })
            .unwrap_or(0.85)
            .clamp(0.0, 1.0);

        (ratio * 100.0).clamp(0.0, 100.0)
    }

    /// Calculates load score (higher means more capacity available).
    fn calculate_load_score(
        &self,
        transport_type: TransportType,
        metrics: &TransportMetrics,
    ) -> f32 {
        let threshold = self.config.congestion_queue_threshold.max(1) as f32;
        let avg_queue = self
            .transport_history
            .get(&transport_type)
            .and_then(|history| history.average_queue_depth())
            .unwrap_or(metrics.queue_depth as f32);

        let utilisation = (avg_queue / threshold).clamp(0.0, 1.5);
        let mut score = ((1.0 - utilisation).clamp(0.0, 1.0)) * 100.0;

        if let Some(drop) = metrics.effective_drop_ratio() {
            let drop_penalty = (drop * 100.0).clamp(0.0, 40.0);
            score = (score - drop_penalty).max(0.0);
        }

        if metrics.is_active_relay {
            let optimal = self.config.relay_optimal_connection_count.max(1) as f32;
            let connections = metrics.relay_connection_count as f32;
            if connections > optimal {
                let overload = ((connections - optimal) / optimal).clamp(0.0, 1.5);
                score = (score * (1.0 - (0.4 * overload))).max(0.0);
            } else {
                score = (score + 5.0).min(100.0);
            }
        }

        score.clamp(0.0, 100.0)
    }

    /// Records the latest metrics for the transport.
    fn record_metrics(&mut self, transport_type: TransportType, metrics: &TransportMetrics) {
        self.last_metrics
            .insert(transport_type, (metrics.clone(), Utc::now()));
    }

    fn history_window(&self) -> usize {
        self.config.history_window_size.max(1)
    }

    fn update_history(&mut self, transport_type: TransportType, metrics: &TransportMetrics) {
        let window = self.history_window();
        let history = self.transport_history.entry(transport_type).or_default();

        history.push_queue_depth(metrics.queue_depth, window);
        history.push_congestion(metrics.congestion.clamp(0.0, 1.0), window);

        if let Some(rssi) = metrics.rssi {
            history.push_rssi(rssi, window);
        }

        if let Some(latency) = metrics.latency_ms {
            history.push_latency(latency, window);
        }

        let total = metrics.success_count + metrics.failure_count;
        if total > 0 {
            let success_ratio = metrics.success_count as f32 / total as f32;
            history.push_success_ratio(success_ratio, window);
        }
    }

    /// Updates BLE-specific escalation signals.
    fn update_ble_conditions(&mut self, message: &Message, metrics: &TransportMetrics) {
        let now = Utc::now();

        if let Some(rssi) = metrics.rssi {
            if rssi <= self.config.rssi_switch_threshold {
                if self.ble_poor_signal_since.is_none() {
                    self.ble_poor_signal_since = Some(now);
                }
            } else {
                self.ble_poor_signal_since = None;
            }
        }

        let congestion_threshold = self.config.congestion_queue_threshold;
        let average_queue = self
            .transport_history
            .get(&TransportType::BLE)
            .and_then(|history| history.average_queue_depth());

        let queue_high = if congestion_threshold == 0 {
            metrics.queue_depth > 0
        } else {
            metrics.queue_depth >= congestion_threshold
                || average_queue
                    .map(|avg| avg >= congestion_threshold as f32)
                    .unwrap_or(false)
        };

        if queue_high {
            if self.ble_high_congestion_since.is_none() {
                self.ble_high_congestion_since = Some(now);
            }
        } else {
            let recovery_ratio = self.config.queue_recovery_ratio.clamp(0.0, 1.0);
            let recovery_threshold = if congestion_threshold == 0 {
                0
            } else {
                (congestion_threshold as f32 * recovery_ratio)
                    .ceil()
                    .max(1.0) as usize
            };

            let recovered = if congestion_threshold == 0 {
                metrics.queue_depth == 0
            } else {
                metrics.queue_depth <= recovery_threshold
                    && average_queue
                        .map(|avg| avg <= recovery_threshold as f32)
                        .unwrap_or(true)
            };

            if recovered {
                self.ble_high_congestion_since = None;
            }
        }

        if message.ttl.value() <= self.config.ttl_escalation_threshold {
            self.low_ttl_detected_at = Some(now);
        } else if message.ttl.value() > self.config.ttl_escalation_threshold.saturating_add(1) {
            // Reset once we are comfortably above the threshold.
            self.low_ttl_detected_at = None;
        }
    }

    /// Checks if the cooldown period has passed since last switch.
    fn is_past_cooldown(&self) -> bool {
        if let Some(last_switch) = self.last_switch_time {
            let elapsed = Utc::now().signed_duration_since(last_switch);
            elapsed.num_seconds() >= self.config.switch_cooldown_secs as i64
        } else {
            true // No previous switch, cooldown doesn't apply
        }
    }

    /// Checks if a transport has been consistently better for the stability window.
    fn is_stable_better(
        &self,
        new_transport: TransportType,
        current_transport: TransportType,
        window_secs: u64,
        required_improvement: f32,
    ) -> bool {
        let now = Utc::now();
        let window_start = now - chrono::Duration::seconds(window_secs as i64);

        // Get scores for both transports within the window
        let new_scores = self.get_scores_in_window(new_transport, window_start);
        let current_scores = self.get_scores_in_window(current_transport, window_start);

        // If we don't have enough history, allow the switch
        if new_scores.is_empty() || current_scores.is_empty() {
            return true;
        }

        // Calculate average scores
        let new_avg: f32 = new_scores.iter().sum::<f32>() / new_scores.len() as f32;
        let current_avg: f32 = current_scores.iter().sum::<f32>() / current_scores.len() as f32;

        // New transport must be consistently better
        (new_avg - current_avg) >= required_improvement
    }

    /// Gets transport scores within a time window.
    fn get_scores_in_window(
        &self,
        transport_type: TransportType,
        window_start: DateTime<Utc>,
    ) -> Vec<f32> {
        self.transport_scores_history
            .get(&transport_type)
            .map(|history| {
                history
                    .iter()
                    .filter(|(time, _)| *time >= window_start)
                    .map(|(_, score)| *score)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Records a transport score for history tracking.
    fn record_score(&mut self, transport_type: TransportType, score: f32) {
        let now = Utc::now();
        let limit = self.history_window().max(10);
        let history = self
            .transport_scores_history
            .entry(transport_type)
            .or_default();

        history.push((now, score));

        // Keep history bounded based on configured window
        while history.len() > limit {
            history.remove(0);
        }
    }

    /// Records a retry failure for a transport.
    /// Uses saturating addition to prevent overflow.
    pub fn record_retry_failure(&mut self, transport_type: TransportType) {
        let count = self.retry_counts.entry(transport_type).or_insert(0);
        *count = count.saturating_add(1);
    }

    /// Resets retry count for a transport (e.g., after successful delivery).
    pub fn reset_retry_count(&mut self, transport_type: TransportType) {
        self.retry_counts.insert(transport_type, 0);
    }

    /// Checks if escalation from BLE to Wi-Fi Direct is needed.
    pub fn should_escalate_to_wifi(&self) -> bool {
        let retry_failure = self
            .retry_counts
            .get(&TransportType::BLE)
            .map(|count| *count >= self.config.ble_to_wifi_retry_threshold)
            .unwrap_or(false);

        let now = Utc::now();

        let poor_signal = self
            .ble_poor_signal_since
            .map(|since| {
                now.signed_duration_since(since)
                    >= Duration::seconds(self.config.poor_signal_duration_secs as i64)
            })
            .unwrap_or(false);

        let high_congestion_active = self
            .ble_high_congestion_since
            .map(|since| {
                now.signed_duration_since(since)
                    >= Duration::seconds(self.config.congestion_duration_secs as i64)
            })
            .unwrap_or(false);

        let load_exceeded = self
            .transport_history
            .get(&TransportType::BLE)
            .and_then(|history| history.average_queue_depth())
            .map(|avg| {
                let threshold = self.config.congestion_queue_threshold.max(1) as f32;
                avg >= threshold
            })
            .unwrap_or(false);

        let high_congestion = high_congestion_active && load_exceeded;

        let low_ttl_recent = self
            .low_ttl_detected_at
            .map(|since| {
                now.signed_duration_since(since)
                    <= Duration::seconds(self.config.ttl_escalation_hold_secs as i64)
            })
            .unwrap_or(false);

        let battery_too_low = self
            .last_metrics
            .get(&TransportType::BLE)
            .and_then(|(metrics, _)| {
                metrics
                    .battery_level
                    .map(|level| (level, metrics.is_charging))
            })
            .map(|(level, charging)| level < self.config.relay_min_battery_level && !charging)
            .unwrap_or(false);

        if battery_too_low {
            return false;
        }

        retry_failure || poor_signal || high_congestion || low_ttl_recent
    }

    /// Checks if WiFi escalation is appropriate for a message with given priority.
    ///
    /// For Critical priority messages, battery constraints are bypassed since
    /// message delivery is more important than battery preservation.
    ///
    /// # Arguments
    ///
    /// * `priority` - The message priority
    ///
    /// # Returns
    ///
    /// `true` if WiFi escalation is appropriate for this priority level
    pub fn should_escalate_to_wifi_for_priority(
        &self,
        priority: offline_protocol_core::MessagePriority,
    ) -> bool {
        use offline_protocol_core::MessagePriority;

        // For critical priority, bypass battery check
        if matches!(priority, MessagePriority::Critical) {
            return self.should_escalate_to_wifi_ignoring_battery();
        }

        // For high priority, still escalate but respect battery constraints
        if matches!(priority, MessagePriority::High) {
            return self.should_escalate_to_wifi();
        }

        // For normal/low priority, only escalate under severe conditions
        let retry_failure = self
            .retry_counts
            .get(&TransportType::BLE)
            .map(|count| *count >= self.config.ble_to_wifi_retry_threshold * 2)
            .unwrap_or(false);

        retry_failure && self.should_escalate_to_wifi()
    }

    /// Internal: checks escalation conditions without battery constraint.
    fn should_escalate_to_wifi_ignoring_battery(&self) -> bool {
        let retry_failure = self
            .retry_counts
            .get(&TransportType::BLE)
            .map(|count| *count >= self.config.ble_to_wifi_retry_threshold)
            .unwrap_or(false);

        let now = Utc::now();

        let poor_signal = self
            .ble_poor_signal_since
            .map(|since| {
                now.signed_duration_since(since)
                    >= Duration::seconds(self.config.poor_signal_duration_secs as i64)
            })
            .unwrap_or(false);

        let high_congestion_active = self
            .ble_high_congestion_since
            .map(|since| {
                now.signed_duration_since(since)
                    >= Duration::seconds(self.config.congestion_duration_secs as i64)
            })
            .unwrap_or(false);

        let load_exceeded = self
            .transport_history
            .get(&TransportType::BLE)
            .and_then(|history| history.average_queue_depth())
            .map(|avg| {
                let threshold = self.config.congestion_queue_threshold.max(1) as f32;
                avg >= threshold
            })
            .unwrap_or(false);

        let high_congestion = high_congestion_active && load_exceeded;

        let low_ttl_recent = self
            .low_ttl_detected_at
            .map(|since| {
                now.signed_duration_since(since)
                    <= Duration::seconds(self.config.ttl_escalation_hold_secs as i64)
            })
            .unwrap_or(false);

        // For critical messages, we escalate if any single condition is met
        // (not requiring battery check)
        retry_failure || poor_signal || high_congestion || low_ttl_recent
    }

    /// Gets the current transport.
    pub fn current_transport(&self) -> Option<TransportType> {
        self.current_transport
    }
}

impl Default for TransportSelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, UserId};

    fn create_test_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "Test message",
        )
    }

    fn create_test_metrics(
        rssi: Option<i16>,
        congestion: f32,
        queue_depth: usize,
    ) -> TransportMetrics {
        TransportMetrics {
            rssi,
            latency_ms: Some(10),
            bandwidth_bps: Some(150_000),
            congestion,
            queue_depth,
            success_count: 10,
            failure_count: 0,
            battery_level: Some(80),
            is_charging: false,
            relay_connection_count: 3,
            is_active_relay: true,
            delivery_ratio: Some(0.9),
            drop_rate: Some(0.1),
            average_hop_count: Some(2.0),
            energy_cost: Some(0.2),
        }
    }

    #[test]
    fn test_transport_selection_prefers_best_score() {
        let mut selector = TransportSelector::new();
        let message = create_test_message();

        let mut transports = HashMap::new();
        transports.insert(TransportType::BLE, create_test_metrics(Some(-60), 0.2, 10));
        transports.insert(
            TransportType::WiFiDirect,
            create_test_metrics(Some(-55), 0.1, 5),
        );

        let selected = selector.select_transport(&message, &transports);
        assert!(selected.is_some());
    }

    #[test]
    fn test_online_first_mode() {
        let config = DorsConfig {
            prefer_online: true,
            ..Default::default()
        };
        let mut selector = TransportSelector::with_config(config);
        let message = create_test_message();

        let mut transports = HashMap::new();
        transports.insert(TransportType::Internet, create_test_metrics(None, 0.0, 0));
        transports.insert(TransportType::BLE, create_test_metrics(Some(-60), 0.2, 10));

        let selected = selector.select_transport(&message, &transports).unwrap();
        assert_eq!(selected, TransportType::Internet);
    }

    #[test]
    fn test_hysteresis_prevents_switching() {
        let mut selector = TransportSelector::new();
        let message = create_test_message();

        let mut transports = HashMap::new();
        transports.insert(TransportType::BLE, create_test_metrics(Some(-60), 0.2, 10));
        transports.insert(
            TransportType::WiFiDirect,
            create_test_metrics(Some(-55), 0.1, 5),
        );

        // First selection
        let first = selector.select_transport(&message, &transports).unwrap();

        // Slightly modify metrics (not enough to overcome hysteresis)
        transports.insert(TransportType::BLE, create_test_metrics(Some(-61), 0.2, 10));

        // Should stay with same transport due to hysteresis
        let second = selector.select_transport(&message, &transports).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn test_retry_escalation() {
        let mut selector = TransportSelector::new();

        assert!(!selector.should_escalate_to_wifi());

        selector.record_retry_failure(TransportType::BLE);
        assert!(!selector.should_escalate_to_wifi());

        selector.record_retry_failure(TransportType::BLE);
        assert!(selector.should_escalate_to_wifi());

        selector.reset_retry_count(TransportType::BLE);
        assert!(!selector.should_escalate_to_wifi());
    }

    #[test]
    fn test_poor_signal_escalation() {
        let config = DorsConfig {
            rssi_switch_threshold: -80,
            poor_signal_duration_secs: 0, // Immediate for test
            ..Default::default()
        };

        let mut selector = TransportSelector::with_config(config);
        let message = create_test_message();

        let mut transports = HashMap::new();
        transports.insert(TransportType::BLE, create_test_metrics(Some(-90), 0.1, 5));

        selector.select_transport(&message, &transports);
        assert!(selector.should_escalate_to_wifi());
    }

    #[test]
    fn test_congestion_escalation() {
        let config = DorsConfig {
            congestion_queue_threshold: 10,
            congestion_duration_secs: 0,
            ..Default::default()
        };

        let mut selector = TransportSelector::with_config(config);
        let message = create_test_message();

        let mut transports = HashMap::new();
        transports.insert(TransportType::BLE, create_test_metrics(Some(-60), 0.9, 20));

        selector.select_transport(&message, &transports);
        assert!(selector.should_escalate_to_wifi());
    }

    #[test]
    fn test_low_battery_blocks_escalation() {
        let mut selector = TransportSelector::new();
        let message = create_test_message();

        let mut ble_metrics = create_test_metrics(Some(-90), 0.9, 25);
        ble_metrics.battery_level = Some(10);
        ble_metrics.is_charging = false;

        let mut transports = HashMap::new();
        transports.insert(TransportType::BLE, ble_metrics);
        transports.insert(
            TransportType::WiFiDirect,
            create_test_metrics(Some(-60), 0.2, 5),
        );

        selector.select_transport(&message, &transports);
        selector.record_retry_failure(TransportType::BLE);
        selector.record_retry_failure(TransportType::BLE);

        assert!(!selector.should_escalate_to_wifi());
    }

    #[test]
    fn test_signal_score_calculation() {
        let selector = TransportSelector::new();

        let excellent = create_test_metrics(Some(-40), 0.0, 0);
        let score = selector.calculate_signal_score(TransportType::BLE, &excellent);
        assert_eq!(score, 100.0);

        let poor = create_test_metrics(Some(-90), 0.0, 0);
        let score = selector.calculate_signal_score(TransportType::BLE, &poor);
        assert!(score < 40.0);
    }

    #[test]
    fn test_congestion_score() {
        let selector = TransportSelector::new();

        let low_congestion = create_test_metrics(Some(-60), 0.1, 5);
        let score = selector.calculate_congestion_score(TransportType::BLE, &low_congestion);
        assert!(score > 80.0);

        let high_congestion = create_test_metrics(Some(-60), 0.9, 60);
        let score = selector.calculate_congestion_score(TransportType::BLE, &high_congestion);
        assert!(score < 20.0);
    }

    #[test]
    fn test_load_score_discourages_overloaded_transport() {
        let mut selector = TransportSelector::new();
        let message = create_test_message();

        let mut transports = HashMap::new();
        transports.insert(TransportType::BLE, create_test_metrics(Some(-55), 0.2, 5));

        let mut congested = create_test_metrics(Some(-55), 0.9, 90);
        congested.queue_depth = 90;
        congested.congestion = 0.9;
        transports.insert(TransportType::WiFiDirect, congested);

        let selected = selector.select_transport(&message, &transports).unwrap();
        assert_eq!(selected, TransportType::BLE);
    }

    #[test]
    fn test_unavailable_transport_bypasses_cooldown() {
        let config = DorsConfig {
            prefer_online: true,
            switch_cooldown_secs: 60, // Long cooldown
            ..Default::default()
        };
        let mut selector = TransportSelector::with_config(config);
        let message = create_test_message();

        // First: select Internet while it's available
        let mut transports_with_internet = HashMap::new();
        transports_with_internet
            .insert(TransportType::Internet, create_test_metrics(None, 0.0, 0));
        transports_with_internet
            .insert(TransportType::BLE, create_test_metrics(Some(-60), 0.2, 10));

        let selected = selector
            .select_transport(&message, &transports_with_internet)
            .unwrap();
        assert_eq!(selected, TransportType::Internet);

        // Now Internet disconnects (removed from available transports).
        // Despite the 60s cooldown, DORS must switch to BLE immediately.
        let mut transports_without_internet = HashMap::new();
        transports_without_internet
            .insert(TransportType::BLE, create_test_metrics(Some(-60), 0.2, 10));

        let selected = selector
            .select_transport(&message, &transports_without_internet)
            .unwrap();
        assert_eq!(
            selected,
            TransportType::BLE,
            "DORS must fall back to BLE when Internet is no longer available"
        );
    }
}
