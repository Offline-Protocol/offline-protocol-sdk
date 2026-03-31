//! Transport types and data structures.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, Mutex};

/// Thread-safe, optional callback shared between transport implementations (BLE, Wi-Fi Direct).
pub type SharedCallback = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

/// Types of transports available in the Offline Protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    /// Internet transport (online connectivity).
    Internet,
    /// Bluetooth Low Energy mesh transport.
    BLE,
    /// Wi-Fi Direct transport (Android only).
    #[serde(rename = "wifiDirect")]
    WiFiDirect,
    /// Reticulum mesh transport (LoRa, TCP, UDP, serial, I2P).
    #[serde(rename = "reticulum")]
    Reticulum,
}

impl TransportType {
    /// Canonical lowercase label for this transport.
    pub fn label(self) -> &'static str {
        match self {
            TransportType::Internet => "internet",
            TransportType::BLE => "ble",
            TransportType::WiFiDirect => "wifiDirect",
            TransportType::Reticulum => "reticulum",
        }
    }

    /// Creates a transport type from a string label.
    pub fn from_label(label: &str) -> Self {
        let normalized = label.to_ascii_lowercase();
        match normalized.as_str() {
            "internet" => TransportType::Internet,
            "wifidirect" | "wifi_direct" | "wifi-direct" => TransportType::WiFiDirect,
            "reticulum" => TransportType::Reticulum,
            _ => TransportType::BLE,
        }
    }
}

impl fmt::Display for TransportType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}
/// Transport metrics for monitoring and decision making.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TransportMetrics {
    /// Received Signal Strength Indicator in dBm.
    pub rssi: Option<i16>,
    /// Average latency in milliseconds.
    pub latency_ms: Option<u32>,
    /// Estimated bandwidth in bytes per second.
    pub bandwidth_bps: Option<u64>,
    /// Congestion level (0.0 to 1.0, where 1.0 is fully congested).
    pub congestion: f32,
    /// Number of messages in send queue.
    pub queue_depth: usize,
    /// Number of successful sends in last minute.
    pub success_count: u32,
    /// Number of failed sends in last minute.
    pub failure_count: u32,
    /// Battery level of the hosting device (0-100).
    pub battery_level: Option<u8>,
    /// Indicates if the device is currently charging.
    pub is_charging: bool,
    /// Number of concurrent connections participating in relay duties.
    pub relay_connection_count: u8,
    /// Whether this node is actively acting as a relay on this transport.
    pub is_active_relay: bool,
    /// Recently observed end-to-end delivery ratio (0.0-1.0).
    pub delivery_ratio: Option<f32>,
    /// Recently observed drop rate (0.0-1.0).
    pub drop_rate: Option<f32>,
    /// Average hop count recorded for messages on this transport.
    pub average_hop_count: Option<f32>,
    /// Estimated per-byte energy cost (abstract units, higher is more expensive).
    pub energy_cost: Option<f32>,
}

impl Default for TransportMetrics {
    fn default() -> Self {
        Self {
            rssi: None,
            latency_ms: None,
            bandwidth_bps: None,
            congestion: 0.0,
            queue_depth: 0,
            success_count: 0,
            failure_count: 0,
            battery_level: None,
            is_charging: false,
            relay_connection_count: 0,
            is_active_relay: false,
            delivery_ratio: None,
            drop_rate: None,
            average_hop_count: None,
            energy_cost: None,
        }
    }
}

impl TransportMetrics {
    /// Calculates the effective delivery ratio, falling back to success/failure counters.
    pub fn effective_delivery_ratio(&self) -> Option<f32> {
        if let Some(ratio) = self.delivery_ratio {
            return Some(ratio.clamp(0.0, 1.0));
        }

        let total = self.success_count + self.failure_count;
        if total == 0 {
            None
        } else {
            Some((self.success_count as f32 / total as f32).clamp(0.0, 1.0))
        }
    }

    /// Calculates the effective drop ratio, falling back to success/failure counters.
    pub fn effective_drop_ratio(&self) -> Option<f32> {
        if let Some(rate) = self.drop_rate {
            return Some(rate.clamp(0.0, 1.0));
        }

        let total = self.success_count + self.failure_count;
        if total == 0 {
            None
        } else {
            Some((self.failure_count as f32 / total as f32).clamp(0.0, 1.0))
        }
    }

    /// Returns the total number of success/failure samples tracked.
    pub fn sample_count(&self) -> u32 {
        self.success_count + self.failure_count
    }
}

/// Link quality score (0-100, where 100 is perfect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LinkQuality(u8);

impl LinkQuality {
    /// Maximum link quality value.
    pub const MAX: u8 = 100;
    /// Minimum link quality value.
    pub const MIN: u8 = 0;

    /// Creates a new LinkQuality, clamping to valid range [0, 100].
    pub fn new(value: u8) -> Self {
        Self(value.min(Self::MAX))
    }

    /// Returns the quality value.
    pub fn value(&self) -> u8 {
        self.0
    }

    /// Calculates link quality from RSSI.
    ///
    /// RSSI scale (typical for BLE/WiFi):
    /// - Above -50 dBm: Excellent (90-100)
    /// - -50 to -70 dBm: Good (70-90)
    /// - -70 to -85 dBm: Fair (40-70)
    /// - Below -85 dBm: Poor (0-40)
    pub fn from_rssi(rssi: i16) -> Self {
        let quality = if rssi >= -50 {
            100
        } else if rssi >= -70 {
            // Linear interpolation between 70 and 100
            70 + ((rssi + 70) * 30 / 20) as u8
        } else if rssi >= -85 {
            // Linear interpolation between 40 and 70
            40 + ((rssi + 85) * 30 / 15) as u8
        } else {
            // Below -85 dBm
            ((rssi + 100).max(0) * 40 / 15) as u8
        };
        Self::new(quality)
    }

    /// Checks if the link quality is good (>= 70).
    pub fn is_good(&self) -> bool {
        self.0 >= 70
    }

    /// Checks if the link quality is poor (<= 40).
    pub fn is_poor(&self) -> bool {
        self.0 <= 40
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- TransportType ---

    #[test]
    fn test_transport_type_label() {
        assert_eq!(TransportType::Internet.label(), "internet");
        assert_eq!(TransportType::BLE.label(), "ble");
        assert_eq!(TransportType::WiFiDirect.label(), "wifiDirect");
        assert_eq!(TransportType::Reticulum.label(), "reticulum");
    }

    #[test]
    fn test_transport_type_from_label() {
        assert_eq!(
            TransportType::from_label("internet"),
            TransportType::Internet
        );
        assert_eq!(
            TransportType::from_label("INTERNET"),
            TransportType::Internet
        );
        assert_eq!(TransportType::from_label("ble"), TransportType::BLE);
        assert_eq!(TransportType::from_label("BLE"), TransportType::BLE);
        assert_eq!(
            TransportType::from_label("wifiDirect"),
            TransportType::WiFiDirect
        );
        assert_eq!(
            TransportType::from_label("wifi_direct"),
            TransportType::WiFiDirect
        );
        assert_eq!(
            TransportType::from_label("wifi-direct"),
            TransportType::WiFiDirect
        );
        assert_eq!(
            TransportType::from_label("wifidirect"),
            TransportType::WiFiDirect
        );
        assert_eq!(
            TransportType::from_label("reticulum"),
            TransportType::Reticulum
        );
        assert_eq!(
            TransportType::from_label("RETICULUM"),
            TransportType::Reticulum
        );
        assert_eq!(TransportType::from_label("unknown"), TransportType::BLE);
        assert_eq!(TransportType::from_label(""), TransportType::BLE);
    }

    #[test]
    fn test_transport_type_display() {
        assert_eq!(TransportType::Internet.to_string(), "internet");
        assert_eq!(TransportType::BLE.to_string(), "ble");
        assert_eq!(TransportType::WiFiDirect.to_string(), "wifiDirect");
        assert_eq!(TransportType::Reticulum.to_string(), "reticulum");
    }

    // --- TransportMetrics ---

    #[test]
    fn test_transport_metrics_default() {
        let m = TransportMetrics::default();
        assert_eq!(m.rssi, None);
        assert_eq!(m.congestion, 0.0);
        assert_eq!(m.queue_depth, 0);
        assert_eq!(m.success_count, 0);
        assert_eq!(m.failure_count, 0);
        assert_eq!(m.sample_count(), 0);
    }

    #[test]
    fn test_transport_metrics_effective_delivery_ratio_explicit() {
        let mut m = TransportMetrics::default();
        m.delivery_ratio = Some(0.9);
        assert_eq!(m.effective_delivery_ratio(), Some(0.9));
    }

    #[test]
    fn test_transport_metrics_effective_delivery_ratio_from_counts() {
        let mut m = TransportMetrics::default();
        m.success_count = 8;
        m.failure_count = 2;
        assert_eq!(m.effective_delivery_ratio(), Some(0.8));
        assert_eq!(m.sample_count(), 10);
    }

    #[test]
    fn test_transport_metrics_effective_delivery_ratio_none_when_no_samples() {
        let m = TransportMetrics::default();
        assert_eq!(m.effective_delivery_ratio(), None);
        assert_eq!(m.effective_drop_ratio(), None);
    }

    #[test]
    fn test_transport_metrics_effective_drop_ratio() {
        let mut m = TransportMetrics::default();
        m.drop_rate = Some(0.2);
        assert_eq!(m.effective_drop_ratio(), Some(0.2));
    }

    #[test]
    fn test_transport_metrics_effective_drop_ratio_from_counts() {
        let mut m = TransportMetrics::default();
        m.success_count = 7;
        m.failure_count = 3;
        assert_eq!(m.effective_drop_ratio(), Some(0.3));
    }

    #[test]
    fn test_transport_metrics_delivery_ratio_clamped() {
        let mut m = TransportMetrics::default();
        m.delivery_ratio = Some(1.5);
        assert_eq!(m.effective_delivery_ratio(), Some(1.0));
        m.delivery_ratio = Some(-0.1);
        assert_eq!(m.effective_delivery_ratio(), Some(0.0));
    }

    // --- LinkQuality ---

    #[test]
    fn test_link_quality_from_rssi() {
        let excellent = LinkQuality::from_rssi(-40);
        assert_eq!(excellent.value(), 100);
        assert!(excellent.is_good());

        let good = LinkQuality::from_rssi(-60);
        assert!(good.value() >= 70 && good.value() <= 90);
        assert!(good.is_good());

        let fair = LinkQuality::from_rssi(-75);
        assert!(fair.value() >= 40 && fair.value() <= 70);

        let poor = LinkQuality::from_rssi(-90);
        assert!(poor.value() <= 40);
        assert!(poor.is_poor());
    }

    #[test]
    fn test_link_quality_clamping() {
        let quality = LinkQuality::new(150);
        assert_eq!(quality.value(), 100);
        assert_eq!(LinkQuality::new(0).value(), 0);
        assert_eq!(LinkQuality::new(100).value(), 100);
    }

    #[test]
    fn test_link_quality_new_and_value() {
        let q = LinkQuality::new(50);
        assert_eq!(q.value(), 50);
    }

    #[test]
    fn test_link_quality_is_good_is_poor() {
        assert!(LinkQuality::new(70).is_good());
        assert!(!LinkQuality::new(69).is_good());
        assert!(LinkQuality::new(40).is_poor());
        assert!(LinkQuality::new(39).is_poor());
        assert!(!LinkQuality::new(41).is_poor());
    }

    #[test]
    fn test_link_quality_from_rssi_boundaries() {
        assert_eq!(LinkQuality::from_rssi(-50).value(), 100);
        assert!(LinkQuality::from_rssi(-70).value() >= 70);
        assert!(
            LinkQuality::from_rssi(-85).value() >= 40 && LinkQuality::from_rssi(-85).value() <= 70
        );
        assert!(LinkQuality::from_rssi(-100).value() <= 40);
    }

    #[test]
    fn test_link_quality_constants() {
        assert_eq!(LinkQuality::MAX, 100);
        assert_eq!(LinkQuality::MIN, 0);
    }
}
