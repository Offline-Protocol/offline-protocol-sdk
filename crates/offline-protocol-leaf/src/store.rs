//! The storage seam, and the one rule that makes it safe.
//!
//! # Persist before emit
//!
//! A leaf node MUST have its MLS state durable before it emits a frame whose
//! production advanced that state. The failure this prevents is not a delivery
//! hiccup: a device that answers and then loses power before its ratchet state
//! reaches flash comes back and **reuses an AEAD nonce**, which is a
//! confidentiality failure in a protocol whose whole claim is the AEAD
//! boundary.
//!
//! This crate does not ask firmware to remember that. Every operation that
//! advances state writes through this trait and only then returns the bytes to
//! send, so a store that returns an error produces no frame at all. The
//! ordering is not documented here and implemented elsewhere; it is the reason
//! [`LeafDevice`](crate::LeafDevice) hands back frames rather than exposing
//! the MLS group it seals with.
//!
//! # What an implementation owes
//!
//! [`LeafStore::store`] must be **durable and atomic per entry**: after it
//! returns `Ok`, a power cut must leave the new value readable, and it must
//! never leave a torn one. mls-rs asks the same of its own storage provider,
//! in the same words, for the same reason. A flash driver that buffers a write
//! and reports success satisfies the type and breaks the rule.
//!
//! # Where the key material should live
//!
//! Everything written through this trait is secret: the identity private key,
//! MLS group state, key package private keys. On a part with secure key
//! storage this trait is how that storage is reached, which is
//! [R12](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/security/threat-model.md)
//! in the threat model. A device that keeps them in general flash yields them
//! to anyone holding the device.

use alloc::{string::String, vec::Vec};
use thiserror::Error;

/// Key type for the device's own identity material.
pub const KEY_TYPE_IDENTITY: &str = "identity";
/// Key type for MLS group state.
pub const KEY_TYPE_GROUP_STATE: &str = "group_state";
/// Key type for MLS prior-epoch records.
pub const KEY_TYPE_GROUP_EPOCH: &str = "group_epoch";
/// Key type for key package private material.
pub const KEY_TYPE_KEY_PACKAGE: &str = "key_package";
/// Key type for what a peer told us it can parse.
pub const KEY_TYPE_PEER: &str = "peer";

/// What a store can fail with.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StoreError {
    /// The write did not happen, or cannot be proven to have happened.
    #[error("store failed: {0}")]
    Store(String),
    /// The read failed.
    #[error("load failed: {0}")]
    Load(String),
    /// The delete failed.
    #[error("delete failed: {0}")]
    Delete(String),
    /// The stored bytes are not what this crate wrote.
    #[error("corrupt record: {0}")]
    Corrupt(String),
}

/// Durable storage for a leaf node's secret material and MLS state.
///
/// The shape mirrors `MlsStorage` in `offline-protocol-mls`, which is the same
/// seam on the phone: a two-part `(key_type, key_id)` key, `&self` methods so
/// one store can be shared by the several places that write through it, and
/// per-entry atomicity. A device implements it over the part's secure key
/// storage, its flash filesystem, or an EEPROM.
///
/// # Errors
///
/// Returning an error is always safe. It aborts the operation before anything
/// is emitted, which is the whole point of the seam.
pub trait LeafStore: Send + Sync {
    /// Writes `data`, replacing any previous value for this key.
    ///
    /// Must be durable and atomic per entry: after `Ok`, a power cut leaves
    /// either the new value or the old one, never a torn record, and a
    /// subsequent [`LeafStore::load`] returns the new value.
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> Result<(), StoreError>;

    /// Reads a value, or `None` if this key was never written.
    fn load(&self, key_type: &str, key_id: &str) -> Result<Option<Vec<u8>>, StoreError>;

    /// Removes a value. Removing a key that is not there is not an error.
    ///
    /// Every value this crate stores is secret, so an implementation should
    /// erase rather than unlink.
    fn delete(&self, key_type: &str, key_id: &str) -> Result<(), StoreError>;
}

#[cfg(any(test, feature = "std"))]
mod memory {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::string::ToString;
    use std::sync::Mutex;

    /// A store that keeps everything in memory.
    ///
    /// For tests and for bringing a board up before its flash driver works.
    /// **It is not a leaf node's storage**: it satisfies the durability
    /// contract only in the sense that there is nothing to lose power.
    #[derive(Debug, Default)]
    pub struct MemoryStore {
        entries: Mutex<BTreeMap<(String, String), Vec<u8>>>,
    }

    impl MemoryStore {
        /// Creates an empty store.
        pub fn new() -> Self {
            Self::default()
        }

        /// Number of entries held, for tests that assert what was written.
        pub fn len(&self) -> usize {
            self.entries.lock().map(|e| e.len()).unwrap_or(0)
        }

        /// Whether the store holds nothing.
        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
    }

    impl LeafStore for MemoryStore {
        fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> Result<(), StoreError> {
            let mut entries = self
                .entries
                .lock()
                .map_err(|e| StoreError::Store(e.to_string()))?;
            entries.insert((key_type.to_string(), key_id.to_string()), data.to_vec());
            Ok(())
        }

        fn load(&self, key_type: &str, key_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
            let entries = self
                .entries
                .lock()
                .map_err(|e| StoreError::Load(e.to_string()))?;
            Ok(entries
                .get(&(key_type.to_string(), key_id.to_string()))
                .cloned())
        }

        fn delete(&self, key_type: &str, key_id: &str) -> Result<(), StoreError> {
            let mut entries = self
                .entries
                .lock()
                .map_err(|e| StoreError::Delete(e.to_string()))?;
            entries.remove(&(key_type.to_string(), key_id.to_string()));
            Ok(())
        }
    }
}

#[cfg(any(test, feature = "std"))]
pub use memory::MemoryStore;
