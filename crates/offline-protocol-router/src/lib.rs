//! DORS routing and relay management for the Offline Protocol SDK.
//!
//! This crate implements the Dynamic Offline Relay Switch (DORS) system
//! for intelligent transport selection and relay management.
//!
//! All code in this crate is 100% safe Rust with no unsafe blocks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod dors;
pub mod error;
pub mod relay;

pub use dors::{
    display_routing_score, DorsConfig, EscalationTriggerReason, TransportScore,
    TransportScoreFactors, TransportSelector,
};
pub use error::{Error, Result};
pub use relay::{RelayConfig, RelayPriority, RelayRole, CRITICAL_RELAY_BATTERY_LEVEL};
