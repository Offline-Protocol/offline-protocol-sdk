//! DORS (Dynamic Offline Routing Strategy) implementation
//!
//! DORS manages transport selection, automatically escalating from BLE to Wi-Fi Direct
//! when BLE performance degrades.

use offline_protocol_core::MessageId;
use offline_protocol_transport::{TransportMetrics, TransportType};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Configuration for DORS engine
#[derive(Debug, Clone)]
pub struct DorsConfig {
    /// Enable automatic transport switching
    pub auto_switch: bool,
    
    /// Seconds to wait before switching back (prevent flapping)
    pub switch_hysteresis_secs: u64,
    
    /// Cooldown period after a switch
    pub switch_cooldown_secs: u64,
    
    /// Number of BLE retries before escalating to Wi-Fi Direct
    pub ble_to_wifi_retry_threshold: u32,
    
    /// RSSI threshold for considering BLE quality poor (dBm)
    pub rssi_switch_threshold: i16,
    
    /// Delivery ratio threshold for switching (0.0 to 1.0)
    pub delivery_ratio_threshold: f64,
}

impl Default for DorsConfig {
    fn default() -> Self {
        Self {
            auto_switch: true,
            switch_hysteresis_secs: 15,
            switch_cooldown_secs: 20,
            ble_to_wifi_retry_threshold: 2,
            rssi_switch_threshold: -85,
            delivery_ratio_threshold: 0.5,
        }
    }
}

/// Per-message retry tracking
struct MessageRetryInfo {
    message_id: MessageId,
    transport_type: TransportType,
    retry_count: u32,
    last_attempt: Instant,
}

/// DORS engine for dynamic transport selection
pub struct DorsEngine {
    config: DorsConfig,
    current_transport: Arc<RwLock<TransportType>>,
    last_switch: Arc<RwLock<Option<Instant>>>,
    message_retries: Arc<RwLock<HashMap<MessageId, MessageRetryInfo>>>,
}

impl DorsEngine {
    pub fn new(config: DorsConfig) -> Self {
        Self {
            config,
            current_transport: Arc::new(RwLock::new(TransportType::BLE)), // Always start with BLE
            last_switch: Arc::new(RwLock::new(None)),
            message_retries: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the currently selected transport
    pub fn current_transport(&self) -> TransportType {
        *self.current_transport.read()
    }

    /// Select the best transport for a message
    pub fn select_transport(
        &self,
        message_id: MessageId,
        available_transports: &[TransportType],
        metrics: &HashMap<TransportType, TransportMetrics>,
    ) -> TransportType {
        if !self.config.auto_switch {
            return *self.current_transport.read();
        }

        // Check if we're in cooldown period
        if let Some(last_switch) = *self.last_switch.read() {
            let cooldown = Duration::from_secs(self.config.switch_cooldown_secs);
            if last_switch.elapsed() < cooldown {
                return *self.current_transport.read();
            }
        }

        // Get retry info for this message
        let retry_count = self
            .message_retries
            .read()
            .get(&message_id)
            .map(|info| info.retry_count)
            .unwrap_or(0);

        // Check if we should escalate from BLE to Wi-Fi Direct
        if *self.current_transport.read() == TransportType::BLE {
            if self.should_escalate_to_wifi(retry_count, metrics, available_transports) {
                info!("Escalating from BLE to Wi-Fi Direct due to poor performance");
                self.switch_to(TransportType::WiFiDirect);
                return TransportType::WiFiDirect;
            }
        }

        // Check if we should switch back to BLE
        if *self.current_transport.read() == TransportType::WiFiDirect {
            if self.should_switch_back_to_ble(metrics) {
                info!("Switching back to BLE");
                self.switch_to(TransportType::BLE);
                return TransportType::BLE;
            }
        }

        *self.current_transport.read()
    }

    /// Record a message retry
    pub fn record_retry(&self, message_id: MessageId, transport_type: TransportType) {
        let mut retries = self.message_retries.write();
        
        let retry_info = retries.entry(message_id).or_insert(MessageRetryInfo {
            message_id,
            transport_type,
            retry_count: 0,
            last_attempt: Instant::now(),
        });

        retry_info.retry_count += 1;
        retry_info.last_attempt = Instant::now();
        retry_info.transport_type = transport_type;

        debug!(
            "Message {} retry count: {} on {:?}",
            message_id, retry_info.retry_count, transport_type
        );
    }

    /// Record successful message delivery
    pub fn record_success(&self, message_id: MessageId) {
        self.message_retries.write().remove(&message_id);
    }

    /// Check if we should escalate from BLE to Wi-Fi Direct
    fn should_escalate_to_wifi(
        &self,
        retry_count: u32,
        metrics: &HashMap<TransportType, TransportMetrics>,
        available_transports: &[TransportType],
    ) -> bool {
        // Check if Wi-Fi Direct is available
        if !available_transports.contains(&TransportType::WiFiDirect) {
            return false;
        }

        // Check retry threshold
        if retry_count >= self.config.ble_to_wifi_retry_threshold {
            return true;
        }

        // Check BLE metrics
        if let Some(ble_metrics) = metrics.get(&TransportType::BLE) {
            // Check RSSI
            if let Some(rssi) = ble_metrics.avg_rssi {
                if rssi < self.config.rssi_switch_threshold {
                    debug!("BLE RSSI too low: {} dBm", rssi);
                    return true;
                }
            }

            // Check delivery ratio
            if ble_metrics.delivery_ratio < self.config.delivery_ratio_threshold {
                debug!("BLE delivery ratio too low: {:.2}", ble_metrics.delivery_ratio);
                return true;
            }
        }

        false
    }

    /// Check if we should switch back to BLE from Wi-Fi Direct
    fn should_switch_back_to_ble(&self, metrics: &HashMap<TransportType, TransportMetrics>) -> bool {
        // Apply hysteresis - wait before switching back
        if let Some(last_switch) = *self.last_switch.read() {
            let hysteresis = Duration::from_secs(self.config.switch_hysteresis_secs);
            if last_switch.elapsed() < hysteresis {
                return false;
            }
        }

        // Check if BLE metrics are good again
        if let Some(ble_metrics) = metrics.get(&TransportType::BLE) {
            let rssi_good = ble_metrics
                .avg_rssi
                .map(|rssi| rssi >= self.config.rssi_switch_threshold + 5) // Add 5 dBm hysteresis
                .unwrap_or(false);

            let delivery_good = ble_metrics.delivery_ratio >= self.config.delivery_ratio_threshold + 0.1;

            return rssi_good && delivery_good;
        }

        false
    }

    /// Switch to a different transport
    fn switch_to(&self, transport_type: TransportType) {
        *self.current_transport.write() = transport_type;
        *self.last_switch.write() = Some(Instant::now());
    }

    /// Clean up old retry info
    pub fn cleanup_old_retries(&self, max_age: Duration) {
        let mut retries = self.message_retries.write();
        let now = Instant::now();

        retries.retain(|_, info| now.duration_since(info.last_attempt) < max_age);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dors_escalation() {
        let config = DorsConfig {
            ble_to_wifi_retry_threshold: 2,
            ..Default::default()
        };
        let dors = DorsEngine::new(config);
        let message_id = MessageId::new();

        // Initially should use BLE
        assert_eq!(dors.current_transport(), TransportType::BLE);

        // Record retries
        dors.record_retry(message_id, TransportType::BLE);
        dors.record_retry(message_id, TransportType::BLE);

        // Should escalate after threshold
        let available = vec![TransportType::BLE, TransportType::WiFiDirect];
        let metrics = HashMap::new();
        
        let selected = dors.select_transport(message_id, &available, &metrics);
        assert_eq!(selected, TransportType::WiFiDirect);
    }
}

