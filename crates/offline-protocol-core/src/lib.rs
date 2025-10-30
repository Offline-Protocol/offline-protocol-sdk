//! Core types and utilities for the Offline Protocol SDK
//!
//! This crate provides the fundamental data structures, message types,
//! and serialization logic used throughout the offline protocol.

pub mod error;
pub mod message;
pub mod types;

pub use error::{Error, Result};
pub use message::{
    ControlMessage, FileChunk, FileMessage, Message, MessageEnvelope, MessageType, TextMessage,
};
pub use types::{DeviceId, MessageId, Priority, UserId};

