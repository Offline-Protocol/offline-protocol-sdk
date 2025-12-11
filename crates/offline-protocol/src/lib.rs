//! Main protocol engine for the Offline Protocol SDK.
//!
//! This crate ties together all the components (core, transport, router, reliability)
//! into a single easy-to-use API.
//!
//! All code in this crate is 100% safe Rust with no unsafe blocks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod config;
pub mod constants;
pub mod error;
pub mod events;
pub mod file_transfer;
pub mod mls;
pub mod protocol;
pub mod transport_manager;
pub mod visualization;

pub use config::ProtocolConfig;
pub use error::{Error, Result};
pub use events::{Event, EventCallback};
pub use protocol::OfflineProtocol;
pub use transport_manager::TransportManager;
pub use visualization::{
    MessageStats, NetworkLink, NetworkNode, NetworkTopology, NetworkVisualizer, NodeRole,
};

// Re-export reliability types for configuration
pub use offline_protocol_reliability::{
    AckConfig, DeduplicatorConfig, DeduplicatorMode, DeduplicatorStats, RetryConfig,
};

// Re-export MLS types for end-to-end encryption
pub use mls::{
    EncryptedMessage, GroupId, GroupInfo, KeyPackageBundle, MlsManager, MlsStorage, WelcomeMessage,
};
