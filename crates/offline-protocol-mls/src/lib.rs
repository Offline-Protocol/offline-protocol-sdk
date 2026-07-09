//! MLS (Message Layer Security) integration for the Offline Protocol SDK.
//!
//! This crate provides end-to-end encryption using the MLS protocol via OpenMLS.
//! It supports both 1:1 encrypted messaging and group messaging with forward secrecy
//! and post-compromise security.
//!
//! # Architecture
//!
//! The crate is designed with a storage-agnostic approach:
//! - Apps implement the [`MlsStorage`] trait for platform-native secure storage
//! - The [`MlsManager`] handles all MLS operations (key generation, encryption, decryption)
//! - Groups are used for both 1:1 sessions (2-person groups) and multi-party chats
//!
//! # Example
//!
//! ```ignore
//! use offline_protocol_mls::{MlsManager, MlsStorage};
//!
//! // Implement MlsStorage for your platform (iOS Keychain, Android Keystore, etc.)
//! let storage = MyPlatformStorage::new();
//!
//! // Create the MLS manager
//! let manager = MlsManager::new("user123", Box::new(storage))?;
//!
//! // Generate a key package to share with others
//! let key_package = manager.generate_key_package()?;
//!
//! // Create a 1:1 session and send an encrypted message
//! let ciphertext = manager.encrypt_for_user("bob", b"Hello, Bob!")?;
//! ```

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod group;
pub mod manager;
pub mod provider;
pub mod session;
pub mod storage;
mod storage_adapter;
pub mod types;

pub use error::{MlsError, Result};
pub use manager::MlsManager;
pub use storage::{MlsStorage, StorageError};
pub use types::{
    EncryptedMessage, GroupId, GroupInfo, GroupMetadata, GroupRole, KeyPackageBundle,
    MlsMessageType, ParseGroupRoleError, WelcomeMessage,
};
