//! Install-scoped storage for protocol delivery state.
//!
//! This storage domain is deliberately separate from [`crate::MlsStorage`].
//! MLS storage contains cryptographic identity and group material that may
//! outlive an app-container incarnation. Protocol state contains restartable
//! delivery machinery—outbox entries, pending messages, retry lifecycles, and
//! peer snapshots—and must be removed with the app container.

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

/// Storage for restartable protocol and message-plane state.
///
/// Implementations must be app-container scoped and must not use a credential
/// store whose lifetime can exceed the app container. The operations retain
/// the atomicity and durability contract defined by
/// [`offline_protocol_mls::MlsStorage`], but this
/// trait deliberately does not inherit from the crypto-storage contract:
/// lifecycle separation should be structural, not a marker on the wrong
/// abstraction — and that includes the error type, which is this crate's own
/// rather than the MLS crate's.
pub trait ProtocolStateStorage: Send + Sync {
    /// Atomically stores or replaces one protocol-state entry.
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()>;

    /// Loads one protocol-state entry, returning `None` when absent.
    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>>;

    /// Deletes one protocol-state entry and succeeds when it is already absent.
    fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()>;

    /// Lists entry IDs within one protocol-state category.
    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>>;
}
