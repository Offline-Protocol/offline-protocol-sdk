//! Error types for MLS operations.

use thiserror::Error;

/// Result type alias for MLS operations.
pub type Result<T> = std::result::Result<T, MlsError>;

/// Errors that can occur during MLS operations.
#[derive(Debug, Error)]
pub enum MlsError {
    /// Storage operation failed.
    #[error("Storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    /// Failed to generate cryptographic material.
    #[error("Crypto generation failed: {0}")]
    CryptoGeneration(String),

    /// Failed to create a credential.
    #[error("Credential creation failed: {0}")]
    CredentialCreation(String),

    /// Failed to create a key package.
    #[error("Key package creation failed: {0}")]
    KeyPackageCreation(String),

    /// Invalid key package received.
    #[error("Invalid key package: {0}")]
    InvalidKeyPackage(String),

    /// Failed to create a group.
    #[error("Group creation failed: {0}")]
    GroupCreation(String),

    /// Group not found.
    #[error("Group not found: {0}")]
    GroupNotFound(String),

    /// Failed to add member to group.
    #[error("Failed to add member: {0}")]
    AddMember(String),

    /// Failed to remove member from group.
    #[error("Failed to remove member: {0}")]
    RemoveMember(String),

    /// Encryption failed.
    #[error("Encryption failed: {0}")]
    Encryption(String),

    /// Decryption failed.
    #[error("Decryption failed: {0}")]
    Decryption(String),

    /// Decryption failed because the local session is out of sync with the
    /// sender's epoch (the two sides disagree on the MLS epoch), as opposed to
    /// a corrupt or forged ciphertext. This is a *recoverable* failure: tearing
    /// down and re-establishing the 1:1 session restores the channel. It is
    /// deliberately distinct from [`MlsError::Decryption`] so the protocol layer
    /// can withhold the delivery ACK and trigger a re-key instead of silently
    /// dropping the message. Message-specific ratchet failures (a discarded past
    /// generation) and AEAD/corrupt failures are NOT classified here — re-keying
    /// would not help and, for forged ciphertext, would be a re-key-storm vector.
    #[error("Session out of sync: {0}")]
    SessionDesync(String),

    /// Failed to process Welcome message.
    #[error("Welcome processing failed: {0}")]
    WelcomeProcessing(String),

    /// Failed to serialize data.
    #[error("Serialization failed: {0}")]
    Serialization(String),

    /// Failed to deserialize data.
    #[error("Deserialization failed: {0}")]
    Deserialization(String),

    /// MLS manager not initialized.
    #[error("MLS not initialized")]
    NotInitialized,

    /// No key package available for the user.
    #[error("No key package available for user: {0}")]
    NoKeyPackage(String),

    /// Session not found for 1:1 messaging.
    #[error("Session not found for user: {0}")]
    SessionNotFound(String),

    /// Invalid message format.
    #[error("Invalid message format: {0}")]
    InvalidMessage(String),

    /// User not found in group.
    #[error("User not found in group: {0}")]
    UserNotInGroup(String),

    /// OpenMLS library error.
    #[error("OpenMLS error: {0}")]
    OpenMls(String),

    /// Signing operation failed.
    #[error("Signing failed: {0}")]
    Signing(String),

    /// Signature verification failed.
    #[error("Signature verification failed: {0}")]
    VerificationFailed(String),

    /// Invalid public key format.
    #[error("Invalid public key: {0}")]
    InvalidPublicKey(String),

    /// Group id failed storage-key validation (empty, oversized, or
    /// containing path-traversal / storage-hostile characters).
    #[error("Invalid group id: {0}")]
    InvalidGroupId(String),

    /// User id failed storage-key validation (empty, or containing
    /// path-traversal / storage-hostile characters).
    #[error("Invalid user id: {0}")]
    InvalidUserId(String),

    /// A key package's embedded credential identity does not match the
    /// user id it was claimed to belong to.
    #[error(
        "Credential identity mismatch: expected '{expected}', key package credential is '{found}'"
    )]
    CredentialIdentityMismatch {
        /// The user id the key package was claimed to belong to.
        expected: String,
        /// The identity actually embedded in the key package credential.
        found: String,
    },

    /// The MLS-authenticated sender of a decrypted message does not match
    /// the sender the transport layer attributed the message to.
    #[error(
        "Sender identity mismatch: claimed '{claimed}', MLS-authenticated sender is '{authenticated}'"
    )]
    SenderIdentityMismatch {
        /// The sender the wire envelope claimed.
        claimed: String,
        /// The identity authenticated by the MLS credential.
        authenticated: String,
    },

    /// A 1:1-session Welcome's `group_id` — the session storage slot it would
    /// install into — does not match the slot for the (local user,
    /// authenticated inviter) pair. Rejecting this stops an authenticated peer
    /// from installing or overwriting a *third* party's 1:1 session slot, which
    /// would hijack the victim's session so their outbound messages encrypt to
    /// the attacker's group. This guards the session-Welcome (`join_session`)
    /// half of SEC-M6; the group-Welcome half is guarded by
    /// [`MlsError::ReservedSessionNamespace`].
    #[error(
        "Welcome session-slot mismatch: inviter maps to slot '{expected}', Welcome claims '{found}'"
    )]
    WelcomeIdentityMismatch {
        /// The only session slot the authenticated inviter may write.
        expected: String,
        /// The slot the Welcome's `group_id` actually named.
        found: String,
    },

    /// A group Welcome named a `group_id` in the reserved `session:` namespace.
    /// That namespace is owned exclusively by identity-bound 1:1 sessions
    /// (installed only via `join_session`, guarded by
    /// [`MlsError::WelcomeIdentityMismatch`]). A group Welcome carries an
    /// attacker-controllable `group_id` with no (self, inviter) binding and
    /// installs into the *same* storage/OpenMLS keyspace, so allowing one to
    /// name a session slot would let an authenticated peer seed or overwrite a
    /// third party's 1:1 session — the identical hijack SEC-M6 blocks on the
    /// session-Welcome path, reached through the group path. This is the
    /// group-Welcome half of that binding.
    #[error("Group Welcome may not target the reserved 'session:' namespace: '{group_id}'")]
    ReservedSessionNamespace {
        /// The reserved-namespace slot the group Welcome tried to install into.
        group_id: String,
    },

    /// A Welcome's embedded MLS group id (the id OpenMLS actually persists the
    /// joined group under) does not match the wire `group_id` the caller
    /// authenticated and keys storage by. The embedded id lives in the
    /// Welcome's `GroupContext` and is chosen freely by the inviter at group
    /// creation (`new_with_group_id`); every SEC-M5/M6 binding validates only
    /// the wire field. Left unchecked, `into_group` would persist the group
    /// under the attacker-chosen embedded id — seeding or overwriting an
    /// arbitrary session/group slot the wire-id checks never inspected, the
    /// same hijack SEC-M6 blocks, reached through the identifier it does not
    /// cover. Rejecting binds the two before any state is written.
    #[error(
        "Welcome group-id mismatch: wire slot '{expected}', Welcome installs under '{embedded}'"
    )]
    WelcomeGroupIdMismatch {
        /// The wire `group_id` the caller authenticated and keys storage by.
        expected: String,
        /// The group id embedded in the Welcome's GroupContext (attacker-chosen).
        embedded: String,
    },
}
