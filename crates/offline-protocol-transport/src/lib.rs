//! Transport abstraction layer for the Offline Protocol SDK.
//!
//! This crate defines the transport trait and types for different
//! transport mechanisms (BLE, Wi-Fi Direct, Internet).
//!
//! All code in this crate is 100% safe Rust with no unsafe blocks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod ble;
pub mod error;
pub mod mock;
pub mod traits;
pub mod types;

pub use ble::{BleTransport, BleTransportBuilder, PeerDevice};
pub use error::{Error, Result};
pub use mock::MockTransport;
pub use traits::{Transport, TransportStatus};
pub use types::{LinkQuality, TransportMetrics, TransportType};
