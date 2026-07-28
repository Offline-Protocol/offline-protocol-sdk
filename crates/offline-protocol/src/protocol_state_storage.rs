//! Install-scoped storage for protocol delivery state.
//!
//! This storage domain is deliberately separate from [`crate::MlsStorage`].
//! MLS storage contains cryptographic identity and group material that may
//! outlive an app-container incarnation. Protocol state contains restartable
//! delivery machinery—outbox entries, pending messages, retry lifecycles, and
//! peer snapshots—and must be removed with the app container.
//!
//! # Confidentiality
//!
//! Lifecycle separation is not confidentiality separation: some of this state
//! (queued message plaintext, cloud-media `encryption_key`/`iv`) is as
//! sensitive as anything in the credential store. Implementations are *not*
//! trusted to protect it — the SDK seals those record values with an AEAD
//! whose key lives in [`crate::MlsStorage`] before they ever reach this trait
//! (see `protocol::state_crypto`). An implementation therefore only ever sees
//! ciphertext for sensitive categories, and a stolen app container without the
//! credential store yields nothing.
//!
//! Record *keys* (`key_type` / `key_id`) are not sealed: they are peer ids and
//! message ids, and the storage layout needs them in the clear to list and
//! address entries.

use std::fmt;

/// Failure of a protocol-state storage operation.
///
/// Deliberately defined here rather than reusing
/// `offline_protocol_mls::StorageError`: the whole point of this trait is that
/// install-scoped delivery state does not share a contract — or a crate — with
/// cryptographic storage. Implementations map their platform errors onto these
/// variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolStateError {
    /// The requested entry does not exist.
    ///
    /// Callers treat an absent entry as `Ok(None)` from
    /// [`ProtocolStateStorage::load`]; this variant exists for implementations
    /// whose platform API cannot express absence any other way.
    NotFound(String),
    /// The entry exists but could not be decoded by the backing store.
    Corrupted(String),
    /// The entry could not be written.
    StoreFailed(String),
    /// The entry could not be read.
    LoadFailed(String),
    /// The entry could not be removed.
    DeleteFailed(String),
}

impl fmt::Display for ProtocolStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(detail) => write!(f, "Protocol state entry not found: {}", detail),
            Self::Corrupted(detail) => write!(f, "Protocol state entry corrupted: {}", detail),
            Self::StoreFailed(detail) => write!(f, "Failed to store protocol state: {}", detail),
            Self::LoadFailed(detail) => write!(f, "Failed to load protocol state: {}", detail),
            Self::DeleteFailed(detail) => write!(f, "Failed to delete protocol state: {}", detail),
        }
    }
}

impl std::error::Error for ProtocolStateError {}

/// Result alias for [`ProtocolStateStorage`] operations.
pub type ProtocolStateResult<T> = Result<T, ProtocolStateError>;

/// Largest record the SDK will ever hand a [`ProtocolStateStorage`], and the
/// ceiling implementations must enforce on the way back.
///
/// Core refuses to write anything larger and refuses to deserialize anything
/// larger on restore. But `load` returns an owned `Vec`, so by the time core
/// can check a length the provider has already allocated it: a corrupt or
/// tampered multi-gigabyte file would be read whole — and, across UniFFI,
/// copied — before core ever sees it. Bounding it is therefore the provider's
/// job, and this constant is the number to bound by.
///
/// A record over this ceiling cannot have been written by this SDK, so a
/// provider should treat it as corrupt (drop it and report absence) rather than
/// return it. See [`ProtocolStateStorage::load`].
///
/// The value is a deliberate superset of core's own record cap plus its seal
/// envelope, so a provider enforcing it never rejects a record the SDK
/// legitimately wrote. `bounded_load_ceiling_is_a_superset_of_the_record_cap`
/// pins that relationship; the built-in Swift, Kotlin, and Python providers
/// mirror the constant.
pub const MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES: usize = 8 * 1024 * 1024;

/// Storage for restartable protocol and message-plane state.
///
/// Implementations must be app-container scoped and must not use a credential
/// store whose lifetime can exceed the app container. The operations retain
/// the atomicity and durability contract defined by
/// [`offline_protocol_mls::MlsStorage`], but this
/// trait deliberately does not inherit from the crypto-storage contract:
/// lifecycle separation should be structural, not a marker on the wrong
/// abstraction.
///
/// Values for sensitive categories arrive already sealed (see the module
/// docs), so an implementation must store and return the bytes it is given
/// verbatim — it must not inspect, re-encode, or truncate them.
pub trait ProtocolStateStorage: Send + Sync {
    /// Atomically stores or replaces one protocol-state entry.
    ///
    /// Implementations may refuse a value over
    /// [`MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES`]; core never sends one.
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()>;

    /// Loads one protocol-state entry, returning `None` when absent.
    ///
    /// **Implementations must bound the read.** Check the stored entry's size
    /// before materializing it and never return — or allocate — more than
    /// [`MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES`]. An entry above the ceiling
    /// is corrupt or tampered by construction; drop it and return `Ok(None)`,
    /// which is also what core does with an oversized record it manages to see.
    /// Returning it instead defeats the size policy, because core can only
    /// check a length it has already been handed.
    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>>;

    /// Deletes one protocol-state entry and succeeds when it is already absent.
    fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()>;

    /// Lists entry IDs within one protocol-state category.
    ///
    /// Like [`Self::load`], enumeration should stay bounded: core caps every
    /// category well below any sane ceiling, so a store holding vastly more
    /// has been tampered with and an implementation may stop early.
    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>>;
}

#[cfg(test)]
mod tests {
    use super::MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES;
    use crate::protocol::state_crypto::SEALED_RECORD_OVERHEAD;
    use crate::protocol::MAX_PROTOCOL_STATE_RECORD_BYTES;

    /// The provider ceiling is what the built-in Swift, Kotlin, and Python
    /// stores enforce before reading a file. If core's own cap ever grew past
    /// it, those providers would start dropping records the SDK legitimately
    /// wrote — silent data loss that only shows up on a device.
    #[test]
    fn bounded_load_ceiling_is_a_superset_of_the_record_cap() {
        assert!(
            MAX_PROTOCOL_STATE_RECORD_BYTES + SEALED_RECORD_OVERHEAD
                < MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES,
            "provider read ceiling ({}) must exceed the largest sealed record \
             core can write ({} + {})",
            MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES,
            MAX_PROTOCOL_STATE_RECORD_BYTES,
            SEALED_RECORD_OVERHEAD,
        );
    }
}
