//! Install-scoped storage for protocol delivery state.
//!
//! This storage domain is deliberately separate from [`crate::MlsStorage`].
//! MLS storage contains cryptographic identity and group material that may
//! outlive an app-container incarnation. Protocol state contains restartable
//! delivery machinery—outbox entries, pending messages, retry lifecycles, and
//! peer snapshots—and must be removed with the app container.

use offline_protocol_mls::MlsStorage;

/// Storage for non-cryptographic protocol and message-plane state.
///
/// Implementations must be app-container scoped and must not use a credential
/// store whose lifetime can exceed the app container. The inherited operations
/// retain the atomicity and durability contract defined by [`MlsStorage`], but
/// this distinct trait prevents secure and operational storage from being
/// accidentally wired through one SDK initialization argument.
pub trait ProtocolStateStorage: MlsStorage {}

// The MLS crate's in-memory provider is safe for both domains in tests. Tests
// must still pass distinct instances to model the production lifecycle split.
impl ProtocolStateStorage for offline_protocol_mls::storage::InMemoryStorage {}
