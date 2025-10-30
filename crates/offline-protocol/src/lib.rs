//! Offline Protocol SDK
//!
//! A Rust-based offline mesh networking SDK with BLE and Wi-Fi Direct transports.

pub mod config;
pub mod error;
pub mod events;
pub mod file_transfer;
pub mod protocol;

pub use config::{
    BleConfig, DorsConfig, NetworkConfig, OfflineProtocolConfig, RelayConfig, ReliabilityConfig,
    TransportsConfig, WiFiDirectConfig,
};
pub use error::{Error, Result};
pub use events::{
    Event, FileReceivedEvent, MessageDeliveredEvent, MessageFailedEvent, MessageReceivedEvent,
    NeighborDiscoveredEvent, NeighborLostEvent, NetworkMetricsEvent, RelayDemotedEvent,
    RelayPromotedEvent, TransportSwitchedEvent,
};
pub use protocol::OfflineProtocol;

// Re-export core types
pub use offline_protocol_core::{DeviceId, MessageId, Priority, UserId};

