//! Error types for MLS operations.

use thiserror::Error;

/// Result type alias for MLS operations.
pub type Result<T> = std::result::Result<T, MlsError>;

/// Which validation gate refused a leaf in [`MlsError::LeafAddressMismatch`].
///
/// RFC 9420 §7.3 names two moments at which a client validates a ratchet tree
/// — joining, and processing a Commit — and the SDK adds a third at use time
/// (the sender's own leaf, checked where the message is attributed). They fail
/// for the same reason but mean different things operationally: a Welcome
/// refusal is an invite that was never joined, a commit refusal is a member of
/// a group you are already in forging identities, and a sender refusal on a
/// leaf that passed the first two means local group state was written behind
/// the SDK's back.
// Adding a variant is a breaking change without this attribute.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafSource {
    /// A leaf in the ratchet tree of a Welcome being joined.
    WelcomeTree,
    /// A leaf added by an Add proposal in a staged commit.
    CommitAdd,
    /// A leaf replaced by an Update proposal in a staged commit.
    CommitUpdate,
    /// The committer's own leaf, carried in the commit's update path.
    CommitPath,
    /// The leaf a decrypted message is attributed to.
    MessageSender,
    /// A leaf read back out of stored group state to build a roster.
    RosterEntry,
}

impl std::fmt::Display for LeafSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::WelcomeTree => "Welcome ratchet-tree",
            Self::CommitAdd => "committed Add proposal's",
            Self::CommitUpdate => "committed Update proposal's",
            Self::CommitPath => "commit update-path",
            Self::MessageSender => "message sender's",
            Self::RosterEntry => "stored roster",
        };
        f.write_str(name)
    }
}

/// Errors that can occur during MLS operations.
// Adding a variant to a public error enum is a breaking change without
// this attribute; downstream crates must carry a wildcard arm.
#[non_exhaustive]
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
    /// a ciphertext that failed authentication. This is a *recoverable* failure:
    /// tearing down and re-establishing the 1:1 session restores the channel. It
    /// is deliberately distinct from [`MlsError::Decryption`] so the protocol
    /// layer can withhold the delivery ACK and trigger a re-key instead of
    /// silently dropping the message. Message-specific ratchet failures (a
    /// discarded past generation) and AEAD/corrupt failures are NOT classified
    /// here — re-keying would not help, and routing them into a re-key would add
    /// a second, unbounded storm vector on top of the one below.
    ///
    /// **This classification does not mean the frame was authentic.** The epoch
    /// is read during framing validation, before any AEAD, sender-data or
    /// signature check, so a frame forged with no key material at all lands here
    /// (`manager.rs::test_forged_frame_reaches_session_desync_without_any_key_material`).
    /// Callers must treat it as an unauthenticated hint and keep whatever they
    /// hang off it bounded and non-destructive; see the SECURITY block on
    /// `OfflineProtocol::schedule_session_rekey` in the `offline-protocol` crate.
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

    /// The stored identity key does not derive to the address this manager
    /// was constructed for.
    ///
    /// Under self-certifying addressing the address *is* the identity key's
    /// hash, so these two can only disagree if the wrong storage namespace was
    /// opened for this profile, or a stored keypair was replaced. Continuing
    /// would build a credential claiming an address this device cannot prove
    /// it owns — every peer would reject it at the derivation check, so the
    /// failure is raised here where it is still legible.
    ///
    /// Only reachable for address-shaped ids: a manager constructed with a
    /// legacy/nickname id has no derivation to check against.
    #[error("Stored identity key derives to '{derived}', but this manager is '{expected}'")]
    IdentityAddressMismatch {
        /// The address this manager was constructed for.
        expected: String,
        /// The address the stored identity key actually derives to.
        derived: String,
    },

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

    /// A key package's leaf signature key does not derive to the address it
    /// claims to belong to.
    ///
    /// Distinct from [`MlsError::CredentialIdentityMismatch`], and it has to
    /// be: credentials here are MLS *basic* credentials, whose content is a
    /// bare self-asserted identity string. Anyone can generate a signature
    /// keypair and stamp an address on it, so the identity check catches only a
    /// careless substitution. This one recomputes the address *from the key*,
    /// and so catches a deliberate one.
    ///
    /// RFC 9420 leaves credential validation to the application's
    /// Authentication Service. For this SDK that service is a pure derivation:
    /// an address *is* `bech32m(0x01 ‖ SHA-256(signature_key)[..20])`, so the
    /// claim carries its own proof and needs no prior contact to check. This is
    /// where that verdict is enforced at key-package use time.
    #[error(
        "Key package signature key derives to '{derived}', not the claimed address '{claimed}'"
    )]
    KeyPackageAddressMismatch {
        /// The address the key package was claimed to belong to.
        claimed: String,
        /// The address its leaf signature key actually derives to.
        derived: String,
    },

    /// A leaf node in a ratchet tree carries a credential its own signature
    /// key does not derive to.
    ///
    /// The tree-side sibling of [`MlsError::KeyPackageAddressMismatch`]. That
    /// one fires on a key package *this device supplied* — imported for a
    /// contact, read back from the contact cache, or handed to
    /// `add_group_member`. This one fires on a leaf that entered the tree some
    /// other way: a Welcome's ratchet tree, or an Add/Update another member
    /// committed. The two are kept separate so the error text says which gate
    /// refused, because the recovery differs — a bad key package means
    /// re-exchange, a bad leaf means someone in the group is forging identities.
    ///
    /// RFC 9420 §5.3.1 puts this check on the application: the Authentication
    /// Service must verify "that the credential's presented identifiers are
    /// correctly associated with the `signature_key` field in the member's
    /// LeafNode", and §7.3 applies that "when a client validates a ratchet
    /// tree, e.g., when joining a group or after processing a Commit". OpenMLS
    /// does not do it for you — its own external-commit validation says so in
    /// as many words ("This MUST be checked by the application"), and
    /// `StagedCommit::credentials_to_verify` exists precisely to hand the
    /// application the credentials it must judge.
    ///
    /// Without this, SEC-M1 ([`MlsError::SenderIdentityMismatch`]) is checking
    /// a wire-claimed sender against a *self-asserted* string: a member who
    /// commits a leaf whose credential names someone else is then attributed as
    /// that someone else, which on the `__GROUP_MSG__` data-plane path needs no
    /// signature from anyone.
    #[error(
        "{context} leaf signature key derives to '{derived}', not the address its credential claims ('{claimed}')"
    )]
    LeafAddressMismatch {
        /// The identity the leaf's credential asserts.
        claimed: String,
        /// The address the leaf's own signature key derives to, or a rendering
        /// of why no address could be derived from it.
        derived: String,
        /// Which gate refused, so a report names the path rather than only the
        /// verdict (see [`LeafSource`]).
        context: LeafSource,
    },

    /// A message, or a commit, was authenticated to a sender that is not a
    /// member of the group.
    ///
    /// MLS admits three such senders: an external joiner committing itself in
    /// (`NewMemberCommit`), a would-be member proposing its own Add
    /// (`NewMemberProposal`), and a party authorized by the group's
    /// `ExternalSenders` extension. This SDK issues none of them and configures
    /// no external senders, so any of the three arriving is either a peer
    /// running something else or an attacker.
    ///
    /// They are refused rather than ignored because each one is a leaf, or a
    /// signing key, that [`MlsError::LeafAddressMismatch`] cannot judge the
    /// usual way: an external joiner's leaf has no prior entry in the tree to
    /// compare against, and an `ExternalSenders` credential is not a leaf at
    /// all. Refusing keeps the tree to identities that entered through a gate
    /// this SDK actually operates.
    #[error("Unsupported sender: {detail}")]
    UnsupportedSender {
        /// Which unsupported sender shape arrived, and where.
        detail: String,
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
