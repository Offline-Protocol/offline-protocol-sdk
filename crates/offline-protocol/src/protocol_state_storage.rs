//! Install-scoped storage for protocol delivery state.
//!
//! This storage domain is deliberately separate from [`crate::MlsStorage`].
//! MLS storage contains cryptographic identity and group material that may
//! outlive an app-container incarnation. Protocol state contains restartable
//! delivery machinery—outbox entries, pending messages, retry lifecycles, and
//! peer snapshots—and must be removed with the app container.

use offline_protocol_mls::storage::StorageResult;

/// Storage for non-cryptographic protocol and message-plane state.
///
/// Implementations must be app-container scoped and must not use a credential
/// store whose lifetime can exceed the app container. The operations retain
/// the atomicity and durability contract defined by
/// [`offline_protocol_mls::MlsStorage`], but this
/// trait deliberately does not inherit from the crypto-storage contract:
/// lifecycle separation should be structural, not a marker on the wrong
/// abstraction.
pub trait ProtocolStateStorage: Send + Sync {
    /// Atomically stores or replaces one protocol-state entry.
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> StorageResult<()>;

    /// Loads one protocol-state entry, returning `None` when absent.
    fn load(&self, key_type: &str, key_id: &str) -> StorageResult<Option<Vec<u8>>>;

    /// Deletes one protocol-state entry and succeeds when it is already absent.
    fn delete(&self, key_type: &str, key_id: &str) -> StorageResult<()>;

    /// Lists entry IDs within one protocol-state category.
    fn list_keys(&self, key_type: &str) -> StorageResult<Vec<String>>;
}
