//! Transport types and data structures.

use serde::{Deserialize, Serialize};

/// Types of transports available in the Offline Protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    /// Internet transport (online connectivity).
    Internet,
    /// Bluetooth Low Energy mesh transport.
    BLE,
    /// Wi-Fi Direct transport (Android only).
    WiFiDirect,
}

/// Transport metrics for monitoring and decision making.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        }
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
    }
}
