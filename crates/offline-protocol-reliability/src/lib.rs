//! Reliability layer for the Offline Protocol SDK.
//!
//! This crate provides ACK management, retry queuing with exponential backoff,
//! message deduplication, and ACK optimization for large networks.
//!
//! All code in this crate is 100% safe Rust with no unsafe blocks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod ack_manager;
pub mod ack_optimization;
pub mod constants;
pub mod deduplicator;
pub mod error;
pub mod retry_queue;

pub use ack_manager::{AckConfig, AckEvictionInfo, AckManager};
pub use ack_optimization::{AckOptimizationConfig, AckOptimizer, AggregatedAck, PiggybackAckData};
pub use deduplicator::{Deduplicator, DeduplicatorConfig, DeduplicatorMode, DeduplicatorStats};
pub use error::{Error, Result};
pub use retry_queue::{RetryConfig, RetryQueue};
