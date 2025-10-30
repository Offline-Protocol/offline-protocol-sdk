//! Routing layer for offline protocol
//!
//! This crate provides DORS (Dynamic Offline Routing Strategy), relay management,
//! and multi-hop routing logic.

pub mod dors;
pub mod error;
pub mod relay;
pub mod router;

pub use dors::{DorsConfig, DorsEngine};
pub use error::{Error, Result};
pub use relay::{RelayManager, RelayManagerConfig};
pub use router::{Router, RouterConfig};

