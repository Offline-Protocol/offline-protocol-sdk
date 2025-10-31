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
pub mod router;

pub use dors::{DorsConfig, TransportSelector};
pub use error::{Error, Result};
pub use relay::{RelayConfig, RelayManager};
pub use router::{PathConfig, PathSelector};
