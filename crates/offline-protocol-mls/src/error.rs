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

    /// A membership commit was refused before merging because the local
    /// admin policy did not authorize its committer or proposal senders.
    ///
    /// Only produced when commit enforcement is explicitly enabled
    /// (`GroupConfig::enforce_admin_commits`, default off) — see
    /// `GroupManager::authorize_membership_commit` for the fail-open rules
    /// that keep an incomplete local admin view from rejecting a legitimate
    /// commit and forking the group. Permanent: the same commit can never
    /// become authorized, so callers must not buffer and retry it.
    #[error("Commit not authorized: {committer} is not an admin of this group")]
    CommitNotAuthorized {
        /// The MLS-authenticated committer whose commit was refused.
        committer: String,
        /// User ids the refused commit would have added, sorted.
        added: Vec<String>,
        /// User ids the refused commit would have removed, sorted.
        removed: Vec<String>,
    },

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

    /// A key package's leaf signature key is not the key already pinned for
    /// the peer it claims to belong to.
    ///
    /// Distinct from [`MlsError::CredentialIdentityMismatch`], and it has to
    /// be: credentials here are MLS *basic* credentials, whose content is a
    /// bare self-asserted identity string. Anyone can generate a signature
    /// keypair and stamp `bob` on it, so the identity check catches only a
    /// careless substitution. This one compares the *key*, against a pin
    /// established when the peer's signature was verified, and so catches a
    /// deliberate one.
    ///
    /// RFC 9420 leaves credential validation to the application's
    /// Authentication Service; for this SDK that service is TOFU, and this is
    /// where its verdict is enforced at key-package use time.
    #[error("Key package signature key does not match the pinned key for '{peer_id}'")]
    KeyPackagePinMismatch {
        /// The peer the key package was claimed to belong to.
        peer_id: String,
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

    /// A 1:1 envelope's `group_id` does not name the session slot shared with
    /// the sender the transport layer attributed it to.
    ///
    /// This is the **failure-path** counterpart to
    /// [`MlsError::SenderIdentityMismatch`], and the two are not
    /// interchangeable: SEC-M1 binds the wire sender to the MLS credential, but
    /// that credential only exists once `process_message` *succeeds*. Every
    /// pre-authentication verdict — most importantly the framing checks OpenMLS
    /// runs before any AEAD (group id, then epoch, yielding
    /// [`MlsError::SessionDesync`]) — is reached with the wire sender still
    /// entirely unverified. Since a 1:1 slot id is `session:<a>:<b>` over two
    /// public user ids, anything keyed off such a verdict would otherwise be
    /// reachable by a stranger naming any session and any sender, with no key
    /// material at all. Requiring the envelope's slot to be the one shared with
    /// the claimed sender is the only binding that covers those paths, so it
    /// runs before the group is loaded.
    #[error("Session identity mismatch: envelope names '{found}', sender's session slot is '{expected}'")]
    SessionIdentityMismatch {
        /// The session slot shared with the claimed sender.
        expected: String,
        /// The slot the envelope's `group_id` actually named.
        found: String,
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
