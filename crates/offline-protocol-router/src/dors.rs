//! DORS (Dynamic Offline Relay Switch) transport selection.
//!
//! This module implements the intelligent transport selection algorithm
//! that automatically chooses and switches between Internet, BLE Mesh, and Wi-Fi Direct
//! based on real-time network conditions.

use chrono::{DateTime, Duration, Utc};
use offline_protocol_core::Message;
use offline_protocol_transport::{TransportMetrics, TransportType};
use std::collections::HashMap;

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
    /// Total weighted score.
    pub total: f32,
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

        // If online-first mode and Internet is available, prefer it
        if self.config.prefer_online && available_transports.contains_key(&TransportType::Internet)
        {
            self.current_transport = Some(TransportType::Internet);
            return Some(TransportType::Internet);
        }

        // Calculate scores for all available transports
        let mut scored_transports: Vec<(TransportType, TransportScore)> = Vec::new();

        for (transport_type, metrics) in available_transports.iter() {
            self.record_metrics(*transport_type, metrics);

            if *transport_type == TransportType::BLE {
                self.update_ble_conditions(message, metrics);
            }

            let score = self.calculate_transport_score(message, *transport_type, metrics);
            scored_transports.push((*transport_type, score));
        }

        // Sort by total score (descending)
        scored_transports.sort_by(|a, b| b.1.total.partial_cmp(&a.1.total).unwrap());

        // Get the best transport
        let best = scored_transports.first()?;
        let best_transport = best.0;
        let best_score = best.1.total;

        // Check if we should switch
        if let Some(current) = self.current_transport {
            // If current transport is still available
            if let Some(current_score) = scored_transports
                .iter()
                .find(|(t, _)| *t == current)
                .map(|(_, s)| s.total)
            {
                // Check hysteresis: only switch if new transport is significantly better
                let improvement = best_score - current_score;
                if improvement < self.config.switch_hysteresis {
                    // Not enough improvement, stay with current
                    return Some(current);
                }

                // Check cooldown period
                if !self.is_past_cooldown() {
                    return Some(current);
                }

                // Check stability: has the new transport been consistently better?
                if !self.is_stable_better(
                    best_transport,
                    current,
                    self.config.stability_window_secs,
                ) {
                    return Some(current);
                }
            }
        }

        // Record the score for history tracking
        self.record_score(best_transport, best_score);

        // Update current transport and switch time
        if self.current_transport != Some(best_transport) {
            self.last_switch_time = Some(Utc::now());
            self.current_transport = Some(best_transport);
        }

        Some(best_transport)
    }

    /// Calculates the transport score based on multiple factors.
    fn calculate_transport_score(
        &self,
        message: &Message,
        transport_type: TransportType,
        metrics: &TransportMetrics,
    ) -> TransportScore {
        let signal_score = self.calculate_signal_score(metrics);
        let proximity_score = self.calculate_proximity_score(message);
        let bandwidth_score = self.calculate_bandwidth_score(transport_type, metrics);
        let congestion_score = self.calculate_congestion_score(metrics);
        let energy_score = self.calculate_energy_score(transport_type);

        // Weighted combination based on DORS specification
        let total = match transport_type {
            TransportType::Internet => {
                // Internet: prefer if available in online-first mode
                if self.config.prefer_online {
                    100.0
                } else {
                    0.0
                }
            }
            TransportType::BLE => {
                // BLE: balanced for signal, energy, congestion, proximity
                (signal_score * 0.3)
                    + (energy_score * 0.3)
                    + (congestion_score * 0.2)
                    + (proximity_score * 0.2)
            }
            TransportType::WiFiDirect => {
                // Wi-Fi Direct: prefer bandwidth, proximity, congestion
                (bandwidth_score * 0.4) + (proximity_score * 0.3) + (congestion_score * 0.3)
            }
        };

        TransportScore {
            signal: signal_score,
            proximity: proximity_score,
            bandwidth: bandwidth_score,
            congestion: congestion_score,
            energy: energy_score,
            total,
        }
    }

    /// Calculates signal strength score from RSSI.
    fn calculate_signal_score(&self, metrics: &TransportMetrics) -> f32 {
        if let Some(rssi) = metrics.rssi {
            // Convert RSSI to score (0-100)
            // Excellent: >= -50 dBm (100)
            // Good: -50 to -70 dBm (70-100)
            // Fair: -70 to -85 dBm (40-70)
            // Poor: < -85 dBm (0-40)
            if rssi >= -50 {
                100.0
            } else if rssi >= -70 {
                70.0 + ((rssi + 70) as f32 * 30.0 / 20.0)
            } else if rssi >= -85 {
                40.0 + ((rssi + 85) as f32 * 30.0 / 15.0)
            } else {
                ((rssi + 100).max(0) as f32 * 40.0 / 15.0).max(0.0)
            }
        } else {
            50.0 // Default middle score if RSSI unavailable
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
    fn calculate_congestion_score(&self, metrics: &TransportMetrics) -> f32 {
        let congestion_level = metrics.congestion.clamp(0.0, 1.0);

        // Invert: less congestion = higher score
        let base_score = (1.0 - congestion_level) * 100.0;

        // Factor in queue depth
        let queue_pressure =
            metrics.queue_depth as f32 / self.config.congestion_queue_threshold as f32;
        let queue_penalty = (queue_pressure.min(1.0) * 30.0).max(0.0);

        (base_score - queue_penalty).max(0.0)
    }

    /// Calculates energy efficiency score.
    fn calculate_energy_score(&self, transport_type: TransportType) -> f32 {
        match transport_type {
            TransportType::BLE => 90.0,        // Low power
            TransportType::WiFiDirect => 40.0, // High power
            TransportType::Internet => 60.0,   // Medium power
        }
    }

    /// Records the latest metrics for the transport.
    fn record_metrics(&mut self, transport_type: TransportType, metrics: &TransportMetrics) {
        self.last_metrics
            .insert(transport_type, (metrics.clone(), Utc::now()));
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

        if metrics.queue_depth >= self.config.congestion_queue_threshold {
            if self.ble_high_congestion_since.is_none() {
                self.ble_high_congestion_since = Some(now);
            }
        } else {
            let recovery_threshold = if self.config.congestion_queue_threshold > 0 {
                self.config.congestion_queue_threshold / 2
            } else {
                0
            };

            if metrics.queue_depth <= recovery_threshold {
                // Clear the signal once congestion has meaningfully recovered.
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
        new_avg > current_avg + self.config.switch_hysteresis
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
        let history = self
            .transport_scores_history
            .entry(transport_type)
            .or_default();

        history.push((now, score));

        // Keep only last 100 scores per transport to avoid unbounded growth
        if history.len() > 100 {
            history.remove(0);
        }
    }

    /// Records a retry failure for a transport.
    pub fn record_retry_failure(&mut self, transport_type: TransportType) {
        *self.retry_counts.entry(transport_type).or_insert(0) += 1;
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
                    <= Duration::seconds(self.config.switch_cooldown_secs as i64)
            })
            .unwrap_or(false);

        let low_ttl_recent = self
            .low_ttl_detected_at
            .map(|since| {
                now.signed_duration_since(since)
                    <= Duration::seconds(self.config.switch_cooldown_secs as i64)
            })
            .unwrap_or(false);

        retry_failure || poor_signal || high_congestion_active || low_ttl_recent
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
    fn test_signal_score_calculation() {
        let selector = TransportSelector::new();

        let excellent = create_test_metrics(Some(-40), 0.0, 0);
        let score = selector.calculate_signal_score(&excellent);
        assert_eq!(score, 100.0);

        let poor = create_test_metrics(Some(-90), 0.0, 0);
        let score = selector.calculate_signal_score(&poor);
        assert!(score < 40.0);
    }

    #[test]
    fn test_congestion_score() {
        let selector = TransportSelector::new();

        let low_congestion = create_test_metrics(Some(-60), 0.1, 5);
        let score = selector.calculate_congestion_score(&low_congestion);
        assert!(score > 80.0);

        let high_congestion = create_test_metrics(Some(-60), 0.9, 60);
        let score = selector.calculate_congestion_score(&high_congestion);
        assert!(score < 20.0);
    }
}
