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

pub mod address;
pub mod error;
pub mod message;
pub mod service;
pub mod sync;
pub mod types;
pub mod username;
pub mod wire;

pub use address::{Address, AddressError};
pub use error::{Error, Result};
pub use message::{
    ContentType, ForwardInfo, MediaMetadata, Message, MessageId, MessagePriority, ReplyContext,
    WireCodec,
};
pub use service::{ServiceDescriptor, ServiceId};
pub use sync::{MutexExt, RwLockExt};
pub use types::{
    validate_id_chars, AppId, HopCount, IdValidationError, LamportClock, LocalInstant, Timestamp,
    UserId, WallClockTimestamp, MAX_ID_LEN, TTL,
};
pub use username::{contains_control_or_format, Username, UsernameError};
pub use wire::{WIRE_V1_MAGIC, WIRE_VERSION_V1};
