//! Core types and data structures for the Offline Protocol SDK.
//!
//! This crate provides the fundamental types used throughout the SDK:
//! - Message types and identifiers
//! - Protocol types (UserId, AppId, TTL, etc.)
//! - Error types
//!
//! All types in this crate are 100% safe Rust with no unsafe blocks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod message;
pub mod types;

pub use error::{Error, Result};
pub use message::{Message, MessageId, MessagePriority};
pub use types::{
    AppId, HopCount, LamportClock, LocalInstant, Timestamp, UserId, WallClockTimestamp, TTL,
};
