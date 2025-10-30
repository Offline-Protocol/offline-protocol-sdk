//! Transport layer for offline protocol
//!
//! This crate provides the transport abstraction and implementations
//! for BLE mesh, Wi-Fi Direct, and mock transports.

pub mod ble;
pub mod error;
pub mod mock;
pub mod traits;
pub mod types;
pub mod wifidirect;

pub use error::{Error, Result};
pub use traits::{Transport, TransportEvent};
pub use types::{
    LinkQuality, Neighbor, NeighborRole, TransportMetrics, TransportType,
};

