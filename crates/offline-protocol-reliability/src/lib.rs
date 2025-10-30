//! Reliability layer for offline protocol
//!
//! This crate provides ACK management, retry logic, and message deduplication.

pub mod ack_manager;
pub mod deduplicator;
pub mod error;
pub mod retry_queue;

pub use ack_manager::AckManager;
pub use deduplicator::Deduplicator;
pub use error::{Error, Result};
pub use retry_queue::{RetryQueue, RetryStrategy};

