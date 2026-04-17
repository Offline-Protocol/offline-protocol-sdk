//! DORS routing and relay management for the Offline Protocol SDK.
//!
//! This crate implements the Dynamic Offline Relay Switch (DORS) system
//! for intelligent transport selection and relay management.
//!
//! All code in this crate is 100% safe Rust with no unsafe blocks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod congestion;
pub mod constants;
pub mod dors;
pub mod error;
pub mod relay;
pub mod router;
pub mod ttl;

pub use congestion::{CongestionConfig, CongestionController, DeliveryOutcome, SendDecision};
pub use dors::{DorsConfig, EscalationTriggerReason, TransportScore, TransportSelector};
pub use error::{Error, Result};
pub use relay::{RelayConfig, RelayManager, RelayRole};
pub use router::{
    ForwardingDecision, GossipConfig, GradientRoutingConfig, GradientRoutingTable, PathConfig,
    PathSelector, RouteEntry,
};
pub use ttl::{AdaptiveTtlCalculator, AdaptiveTtlConfig, NetworkSizeEstimate};
