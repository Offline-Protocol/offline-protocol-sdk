//! Reliability layer for the Offline Protocol SDK.
//!
//! This crate provides ACK management, retry queuing with exponential backoff,
//! and message deduplication.
//!
//! All code in this crate is 100% safe Rust with no unsafe blocks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod ack_manager;
pub mod deduplicator;
pub mod error;
pub mod retry_queue;

pub use ack_manager::AckManager;
pub use deduplicator::Deduplicator;
pub use error::{Error, Result};
pub use retry_queue::RetryQueue;
