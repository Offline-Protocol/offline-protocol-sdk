//! DORS routing and relay management for the Offline Protocol SDK.
//!
//! This crate implements the Dynamic Offline Relay Switch (DORS) system
//! for intelligent transport selection and relay management.
//!
//! All code in this crate is 100% safe Rust with no unsafe blocks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod constants;
pub mod dors;
pub mod error;
pub mod relay;
pub mod router;
pub mod ttl;

pub use dors::{
    display_routing_score, DorsConfig, EscalationTriggerReason, TransportScore,
    TransportScoreFactors, TransportSelector,
};
pub use error::{Error, Result};
pub use relay::{RelayConfig, RelayDemotionReason, RelayManager, RelayRole, RelayTransition};
pub use router::{
    ForwardingDecision, GossipConfig, GradientRoutingConfig, GradientRoutingTable, PathConfig,
    PathSelector, RouteEntry,
};
pub use ttl::{AdaptiveTtlCalculator, AdaptiveTtlConfig, NetworkSizeEstimate};
