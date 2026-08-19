#![deny(unsafe_code)]
#![warn(missing_docs)]

//! Replicated documents for the Offline Protocol SDK.
//!
//! Messaging is synced events; this crate is synced state. A document is a
//! set of collections that any member of a space can edit while offline,
//! merging deterministically when the replicas meet again.
//!
//! # What this crate deliberately does not do
//!
//! It does not persist, encrypt, or send anything. It turns edits into
//! opaque byte deltas and turns byte deltas back into state, and that is
//! all. Storage, sealing and delivery belong to the main `offline-protocol`
//! crate, which already carries ten releases of reliability work this layer
//! inherits for free.
//!
//! # The engine is an implementation detail
//!
//! A CRDT engine is embedded here and named nowhere in this crate's public
//! API. No engine type appears in a signature, in the FFI surface, or in any
//! binding. One leaked engine type and the engine could never be replaced
//! without a breaking release everywhere, which is exactly the lock-in this
//! layer exists to avoid.

/// Document, collections, deltas and encoding.
pub mod doc;
/// Error types.
pub mod error;
/// Size caps and the compaction trigger.
pub mod policy;
/// Values stored in collections.
pub mod value;

pub use doc::{DataDoc, Delta, DocStats, VersionToken};
pub use error::{DataError, DataResult};
pub use policy::{
    should_compact, size_verdict, SizeVerdict, COMPACT_DELTA_LOG_RATIO, COMPACT_MAX_COMMITS,
    COMPACT_MIN_DELTA_LOG_BYTES, DOC_SIZE_WARN_BYTES, MAX_DOC_BYTES,
};
pub use value::DataValue;

/// Longest accepted name for a space, document or collection.
pub const MAX_NAME_LEN: usize = 128;

/// Longest accepted map key.
pub const MAX_KEY_LEN: usize = 256;

/// Check a space, document, or collection name.
///
/// The charset is narrow on purpose. These names are composed into storage
/// record keys (`{space}/{doc}`), so a name containing a separator or a
/// path traversal would make a key that parses back into different parts
/// than it was built from. Rejecting the character is the cheap half of
/// that problem; the expensive half is discovering it after records exist.
pub fn validate_name(name: &str) -> DataResult<()> {
    if name.is_empty() {
        return Err(DataError::InvalidName {
            name: name.to_string(),
            reason: "must not be empty",
        });
    }
    if name.len() > MAX_NAME_LEN {
        return Err(DataError::InvalidName {
            name: name.to_string(),
            reason: "longer than MAX_NAME_LEN",
        });
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(DataError::InvalidName {
            name: name.to_string(),
            reason: "may only contain A-Z a-z 0-9 . _ -",
        });
    }
    Ok(())
}

/// Check a map key.
///
/// Keys are looser than names because they never enter a record key: they
/// live inside the document. The only requirements are non-empty and
/// bounded, so a single key cannot consume a meaningful share of the
/// document's byte budget on its own.
pub fn validate_key(key: &str) -> DataResult<()> {
    if key.is_empty() {
        return Err(DataError::InvalidName {
            name: key.to_string(),
            reason: "must not be empty",
        });
    }
    if key.len() > MAX_KEY_LEN {
        return Err(DataError::InvalidName {
            name: key.to_string(),
            reason: "longer than MAX_KEY_LEN",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
