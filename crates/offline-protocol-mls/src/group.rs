//! MLS group management.
//!
//! This module handles creation, modification, and state management of MLS groups.

use crate::error::{LeafSource, MlsError, Result};
use crate::provider::MlsProvider;
use crate::storage::{MlsStorage, StorageError};
use crate::types::{GroupId, GroupInfo, GroupMetadata, GroupRole, StorageKeyType};

use openmls::prelude::*;
use openmls_traits::signatures::Signer;
use openmls_traits::OpenMlsProvider;
use std::sync::Arc;
use tracing::{debug, warn};

/// Default ciphersuite for MLS operations.
pub const DEFAULT_CIPHERSUITE: Ciphersuite =
    Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

/// How many generations behind the latest decrypted message a sender ratchet
/// key is kept, so late/reordered messages remain decryptable.
///
/// The OpenMLS default (5) is smaller than the windowed media transfer's
/// in-flight budget (up to 8 chunks on internet transports, interleaved with
/// text on the same 1:1 session ratchet), which would make a sufficiently
/// delayed chunk *permanently* undecryptable and stall the transfer. 32 gives
/// 4x headroom over the largest window at the cost of retaining up to 32
/// unused message keys per sender ratchet.
///
/// The protocol layer keeps the combined in-flight budget within this bound
/// by capping concurrent media transfers per peer
/// (`MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER` in `offline-protocol`); revisit
/// both together if either changes.
pub const SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE: u32 = 32;

/// How far ahead of the highest seen generation a sender ratchet may be
/// fast-forwarded when messages are lost (OpenMLS default).
pub const SENDER_RATCHET_MAXIMUM_FORWARD_DISTANCE: u32 = 1000;

/// Sender ratchet configuration applied to every created and joined group.
fn sender_ratchet_configuration() -> SenderRatchetConfiguration {
    SenderRatchetConfiguration::new(
        SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE,
        SENDER_RATCHET_MAXIMUM_FORWARD_DISTANCE,
    )
}

/// Requires a leaf's credential to be the address its own signature key
/// derives to.
///
/// This is the Authentication Service RFC 9420 §5.3.1 delegates to the
/// application, and for this SDK the whole service is one derivation: an
/// address *is* `bech32m(0x01 ‖ SHA-256(signature_key)[..20])`, so a leaf
/// carries its own proof and no prior contact is needed to check it. §7.3
/// requires it wherever a ratchet tree is validated — on joining, and after
/// processing a Commit — which is why this is called from three places rather
/// than one.
///
/// # Why a non-address credential is refused, not skipped
///
/// The same reason [`crate::MlsManager::verify_address_binding`] and the
/// control gate's `verify_sender_derivation` refuse one: a credential with no
/// derivation to check is not a leaf that needs waving through, it is the
/// bypass. Skip it and an impostor writes a nickname into their credential
/// instead of an address and lands in the tree unjudged — and on the
/// `__GROUP_MSG__` data-plane path, where the wire sender carries no signature,
/// being attributed that nickname costs nothing further.
///
/// Nothing in this SDK produces such a leaf: production always constructs the
/// manager at a derived address (`initialize_mls_inner`), and the only leaf
/// this crate ever writes comes from the local credential or from a key package
/// that already passed the same derivation.
///
/// Delegates to [`crate::MlsManager::derive_address`] rather than hashing here,
/// because that is the SDK's single address derivation and a second one is a
/// second format.
fn verify_leaf_binding(
    credential: &Credential,
    signature_key: &[u8],
    context: LeafSource,
) -> Result<()> {
    // A key that cannot derive at all (wrong length) is refused with the same
    // verdict as one that derives elsewhere: either way the leaf has not
    // proved the identity it claims.
    //
    // `claimed` is rendered only on the failure arms. It is attacker-controlled
    // and this runs per inbound group message and per member of every roster
    // read, so the success path must not pay for a string it discards.
    let derived = match crate::MlsManager::derive_address(signature_key) {
        Ok(address) => address,
        Err(e) => {
            return Err(MlsError::LeafAddressMismatch {
                claimed: rendered_credential(credential),
                derived: format!("<no address: {}>", e),
                context,
            });
        }
    };

    if derived.to_string().as_bytes() != credential.serialized_content() {
        return Err(MlsError::LeafAddressMismatch {
            claimed: rendered_credential(credential),
            derived: derived.to_string(),
            context,
        });
    }
    Ok(())
}

/// Renders a credential's content for a diagnostic, bounded.
///
/// A credential is TLS `VLBytes` — the wire permits megabytes of it, and a leaf
/// that reaches [`verify_leaf_binding`] has not proved anything, so its content
/// is attacker-chosen. That string is `error!`-logged and interpolated verbatim
/// into the user-facing `security_warning` reason, so it is bounded here rather
/// than at each of those sinks. An honest address is 62 characters.
fn rendered_credential(credential: &Credential) -> String {
    const MAX_RENDERED_CREDENTIAL_BYTES: usize = 80;

    let content = credential.serialized_content();
    let shown = content
        .get(..MAX_RENDERED_CREDENTIAL_BYTES)
        .unwrap_or(content);
    let mut rendered = String::from_utf8_lossy(shown).into_owned();
    if shown.len() < content.len() {
        rendered.push_str("… (truncated)");
    }
    rendered
}

/// Manages MLS groups for encrypted messaging.
pub struct GroupManager {
    /// Storage backend for persisting group state.
    storage: Arc<dyn MlsStorage>,

    /// OpenMLS crypto provider.
    provider: MlsProvider,

    /// Whether membership commits are checked against the local admin
    /// overlay before merging. Default `false`; see
    /// [`Self::set_enforce_admin_commits`].
    enforce_admin_commits: bool,
}

impl GroupManager {
    /// Creates a new group manager.
    pub fn new(storage: Arc<dyn MlsStorage>, provider: MlsProvider) -> Self {
        Self {
            storage,
            provider,
            enforce_admin_commits: false,
        }
    }

    /// Enables or disables receive-side authorization of membership commits.
    ///
    /// **Off by default, and changing that is a group-partitioning decision,
    /// not a hardening toggle.** Refusing a commit means not merging it: our
    /// epoch stays behind every member who did merge, which MLS cannot heal —
    /// the app has to re-invite us. Enforcement is therefore safe only if
    /// every member reaches the *same* verdict, and the admin overlay is
    /// replicated best-effort (role changes ride unreconciled mesh
    /// notifications; joiners get a point-in-time snapshot). Members whose
    /// role maps merely disagree will partition each other with no attacker
    /// involved.
    ///
    /// `authorize_membership_commit` (private) fails open on *absent* knowledge
    /// to keep the common lag case harmless, but it cannot detect *divergent*
    /// knowledge. Enable this only for a closed deployment that controls role
    /// distribution.
    pub fn set_enforce_admin_commits(&mut self, enforce: bool) {
        self.enforce_admin_commits = enforce;
    }

    /// Returns a reference to the crypto provider.
    pub fn provider(&self) -> &MlsProvider {
        &self.provider
    }

    /// Creates a new MLS group.
    pub fn create_group(
        &self,
        group_id: &GroupId,
        credential_with_key: &CredentialWithKey,
        signer: &impl Signer,
    ) -> Result<MlsGroup> {
        let group_config = MlsGroupCreateConfig::builder()
            .ciphersuite(DEFAULT_CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .sender_ratchet_configuration(sender_ratchet_configuration())
            .build();

        let mls_group_id = openmls::group::GroupId::from_slice(group_id.as_str().as_bytes());

        let group = MlsGroup::new_with_group_id(
            &self.provider,
            signer,
            &group_config,
            mls_group_id,
            credential_with_key.clone(),
        )
        .map_err(|e| MlsError::GroupCreation(e.to_string()))?;

        // Persist group state
        self.save_group(group_id, &group)?;

        debug!(group_id = %group_id, "Created new MLS group");
        Ok(group)
    }

    /// Loads a group from storage.
    pub fn load_group(&self, group_id: &GroupId) -> Result<Option<MlsGroup>> {
        let key_type = StorageKeyType::GroupState.as_str();
        let data = self.storage.load(key_type, group_id.as_str())?;

        match data {
            Some(_bytes) => {
                // MlsGroup in OpenMLS 0.7 uses the storage provider pattern
                let mls_group_id =
                    openmls::group::GroupId::from_slice(group_id.as_str().as_bytes());

                // Try to load from the provider's storage
                match MlsGroup::load(self.provider.storage(), &mls_group_id) {
                    Ok(Some(group)) => Ok(Some(group)),
                    Ok(None) => {
                        debug!(group_id = %group_id, "Group not found in crypto storage");
                        Ok(None)
                    }
                    Err(e) => Err(MlsError::Deserialization(format!(
                        "Failed to load group: {:?}",
                        e
                    ))),
                }
            }
            None => Ok(None),
        }
    }

    /// Saves a group marker to storage.
    ///
    /// OpenMLS persists group state through the provider during mutations.
    /// This marker is kept in our storage layer for listing/enumeration.
    pub fn save_group(&self, group_id: &GroupId, group: &MlsGroup) -> Result<()> {
        let key_type = StorageKeyType::GroupState.as_str();
        let marker = group.epoch().as_u64().to_le_bytes();
        self.storage.store(key_type, group_id.as_str(), &marker)?;
        Ok(())
    }

    /// Deletes a group from storage, including all OpenMLS provider state.
    pub fn delete_group(&self, group_id: &GroupId) -> Result<()> {
        let mls_group_id = openmls::group::GroupId::from_slice(group_id.as_str().as_bytes());

        match MlsGroup::load(self.provider.storage(), &mls_group_id) {
            Ok(Some(mut group)) => {
                group.delete(self.provider.storage()).map_err(|e| {
                    MlsError::Storage(StorageError::DeleteFailed(format!(
                        "Failed to delete group from provider storage: {:?}",
                        e
                    )))
                })?;
            }
            // No group at this id — nothing to clean up.
            Ok(None) => {}
            // We could not load the group to delete its provider-side state
            // (epoch keypairs, secrets); that state may now leak. Warn rather
            // than abort and still drop the marker below: callers such as
            // `join_group_replacing` deliberately rely on `delete_group` not
            // failing here once the one-time key package is already consumed.
            Err(e) => {
                warn!(
                    group_id = %group_id,
                    error = ?e,
                    "delete_group: could not load group to clean provider state; dropping marker only"
                );
            }
        }

        let key_type = StorageKeyType::GroupState.as_str();
        self.storage.delete(key_type, group_id.as_str())?;

        Ok(())
    }

    /// Lists all groups.
    ///
    /// Stored keys that fail group-id validation (possible only for state
    /// written before validation was enforced) are skipped with a warning
    /// rather than resurrected as live group ids.
    pub fn list_groups(&self) -> Result<Vec<GroupId>> {
        let key_type = StorageKeyType::GroupState.as_str();
        let keys = self.storage.list_keys(key_type)?;
        Ok(keys
            .into_iter()
            .filter_map(|key| match GroupId::new(key) {
                Ok(group_id) => Some(group_id),
                Err(e) => {
                    warn!(error = %e, "Skipping stored group with invalid id");
                    None
                }
            })
            .collect())
    }

    /// Adds a member to a group using their key package.
    pub fn add_member(
        &self,
        group: &mut MlsGroup,
        key_package: KeyPackage,
        signer: &impl Signer,
    ) -> Result<(MlsMessageOut, MlsMessageOut)> {
        let (commit, welcome, _group_info) = group
            .add_members(&self.provider, signer, &[key_package])
            .map_err(|e| MlsError::AddMember(e.to_string()))?;

        // Merge the pending commit
        group
            .merge_pending_commit(&self.provider)
            .map_err(|e| MlsError::AddMember(format!("Failed to merge commit: {}", e)))?;

        Ok((commit, welcome))
    }

    /// Removes members from a group.
    ///
    /// Takes every leaf to remove in one commit rather than one leaf per call,
    /// because removing "that member" has to mean all of them or the peer stays
    /// in the group holding live keys while every roster read shows them gone.
    ///
    /// Two leaves for one identity cannot arrive through the wire gates — MLS
    /// requires unique signature keys and the binding ties credential to key —
    /// but they can exist in a tampered provider store, where a forged leaf
    /// claims a peer's address while carrying the attacker's key. See
    /// [`crate::MlsManager::remove_group_member`] for the full argument; do not
    /// restate it here, and do not reduce this to a first match.
    pub fn remove_member(
        &self,
        group: &mut MlsGroup,
        member_indices: &[LeafNodeIndex],
        signer: &impl Signer,
    ) -> Result<MlsMessageOut> {
        let (commit, _welcome, _group_info) = group
            .remove_members(&self.provider, signer, member_indices)
            .map_err(|e| MlsError::RemoveMember(e.to_string()))?;

        group
            .merge_pending_commit(&self.provider)
            .map_err(|e| MlsError::RemoveMember(format!("Failed to merge commit: {}", e)))?;

        Ok(commit)
    }

    /// Creates an encrypted application message.
    pub fn encrypt_message(
        &self,
        group: &mut MlsGroup,
        plaintext: &[u8],
        signer: &impl Signer,
    ) -> Result<MlsMessageOut> {
        let message = group
            .create_message(&self.provider, signer, plaintext)
            .map_err(|e| MlsError::Encryption(e.to_string()))?;

        Ok(message)
    }

    /// Decrypts an incoming MLS message.
    ///
    /// Security (SEC-M1): `claimed_sender` is the identity the caller will
    /// attribute this message to (the transport-level sender). It is
    /// cross-checked against the MLS-authenticated credential of the
    /// processed message *before* any plaintext is surfaced or any commit
    /// is merged, so a group member cannot impersonate another member by
    /// lying in the wire envelope.
    pub fn decrypt_message(
        &self,
        group: &mut MlsGroup,
        group_id: &GroupId,
        message: MlsMessageIn,
        claimed_sender: &str,
    ) -> Result<Option<Vec<u8>>> {
        let protocol_message = message
            .try_into_protocol_message()
            .map_err(|e| MlsError::Decryption(format!("Invalid protocol message: {:?}", e)))?;

        let processed = group
            .process_message(&self.provider, protocol_message)
            .map_err(|e| {
                // Distinguish a *recoverable* epoch desync (the two sides
                // disagree on the MLS epoch — e.g. after a fork) from a genuine
                // decrypt failure. Only the epoch-mismatch family is signalled
                // as `SessionDesync` so the protocol layer can re-key rather
                // than silently drop. AEAD/corrupt failures, ratchet-generation
                // failures (a discarded past generation — the session is
                // healthy, so re-keying would not help), and everything else
                // stay `Decryption`; widening the recoverable arm to cover them
                // would open a second, unbounded re-key-storm vector.
                //
                // It does NOT make the recoverable arm authenticated. OpenMLS
                // validates framing — group id, then epoch — before any AEAD,
                // sender-data or signature check, so `WrongEpoch` is produced
                // with the sender still entirely unverified and a frame forged
                // from nothing reaches this arm (see, in `manager.rs`,
                // `test_forged_frame_reaches_session_desync_without_any_key_material`).
                // `SessionDesync` is an unauthenticated hint by construction;
                // the mitigation lives in what the protocol layer hangs off it.
                match &e {
                    ProcessMessageError::ValidationError(ValidationError::WrongEpoch)
                    | ProcessMessageError::ValidationError(ValidationError::NoPastEpochData) => {
                        MlsError::SessionDesync(e.to_string())
                    }
                    _ => MlsError::Decryption(e.to_string()),
                }
            })?;

        // SEC-M1 compares the wire sender against the MLS credential, so the
        // credential has to be worth comparing against. Bind the sender's leaf
        // first: a basic credential is a self-asserted string, and until its
        // own signature key is shown to hash to it, matching it proves only
        // that the forger typed the name they wanted to be called.
        //
        // The entry-point gates (`verify_staged_welcome_tree`,
        // `verify_staged_commit_leaves`) mean a leaf in the tree has already
        // passed this once. Re-checking here is the same import-time plus
        // use-time pairing `MlsManager::get_contact_key_package` documents, and
        // it exists for the same window: group state is re-read from the
        // install-scoped provider store long after the gate that admitted it,
        // and anything able to write that store could otherwise seat a leaf the
        // gates never saw.
        Self::verify_sender_leaf(group, &processed)?;

        // Credentials in this SDK are basic credentials carrying the user id
        // as raw bytes (see `MlsManager::create_identity`).
        let authenticated = processed.credential().serialized_content();
        if authenticated != claimed_sender.as_bytes() {
            return Err(MlsError::SenderIdentityMismatch {
                claimed: claimed_sender.to_string(),
                authenticated: String::from_utf8_lossy(authenticated).into_owned(),
            });
        }

        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(app_msg) => Ok(Some(app_msg.into_bytes())),
            ProcessedMessageContent::ProposalMessage(_) => {
                debug!("Received proposal message");
                Ok(None)
            }
            ProcessedMessageContent::StagedCommitMessage(staged_commit) => {
                debug!("Received commit message, merging");
                // Authorize before merging — this is the only point at which
                // a commit can still be refused. `merge_staged_commit`
                // advances our epoch irreversibly, and every group decrypt
                // path in the SDK funnels through here (commit frames,
                // commits riding the application channel, `__MLS_ENC__`
                // envelopes naming a group id), so a check anywhere further
                // up would be bypassable by reframing the same ciphertext.
                //
                // Identity first, policy second. The admin check reads the
                // credentials this one validates — its `added` list, and the
                // `GroupUnauthorizedMembershipChange` report built from it, are
                // Add-proposal credential strings — so running it against
                // unvalidated leaves would have it name identities nobody
                // proved.
                Self::verify_staged_commit_leaves(&staged_commit)?;
                self.authorize_membership_commit(group, group_id, &staged_commit, claimed_sender)?;
                group
                    .merge_staged_commit(&self.provider, *staged_commit)
                    .map_err(|e| MlsError::Decryption(format!("Failed to merge commit: {}", e)))?;
                Ok(None)
            }
            // Unreachable since the leaf binding landed: an external join
            // proposal is authenticated to `Sender::NewMemberProposal`, which
            // `verify_sender_leaf` refuses above with
            // `MlsError::UnsupportedSender` before `into_content` is reached.
            // Kept because the enum is `#[non_exhaustive]`-shaped in practice
            // and a match arm is cheaper than a panic, but note the disposition
            // is now *refuse*, not tolerate-and-ignore — do not read this arm as
            // the policy.
            ProcessedMessageContent::ExternalJoinProposalMessage(_) => {
                warn!(
                    "Received external join proposal (not supported); \
                     expected `verify_sender_leaf` to have refused it already"
                );
                Ok(None)
            }
        }
    }

    /// Requires the leaf a processed message is authenticated to to carry the
    /// address its own signature key derives to.
    ///
    /// The use-time half of the binding, and the one that actually closes the
    /// attribution hole: it holds regardless of how the leaf reached the tree,
    /// including paths no entry gate covers (a leaf seated by a direct write to
    /// the provider store, or by a build predating the gates).
    ///
    /// O(1) — the sender's leaf is resolved by index, not by walking the
    /// roster — so it costs one SHA-256 per inbound group message even in a
    /// large group.
    ///
    /// # Non-member senders
    ///
    /// `Sender::Member` is the only shape this SDK sends or expects. The other
    /// three are refused rather than skipped, because skipping would hand back
    /// exactly what the check just took away: an external joiner's leaf is not
    /// in the pre-merge tree, so there is no entry to resolve, and answering
    /// "no leaf, nothing to check" would let a `NewMemberCommit` be attributed
    /// to whatever its credential claims. The commit's own update-path leaf is
    /// still validated by [`Self::verify_staged_commit_leaves`], so refusing
    /// here costs nothing an honest peer was doing — this SDK issues no
    /// external commits, no external proposals, and configures no external
    /// senders.
    fn verify_sender_leaf(group: &MlsGroup, processed: &ProcessedMessage) -> Result<()> {
        let leaf_index = match processed.sender() {
            Sender::Member(index) => *index,
            other => {
                return Err(MlsError::UnsupportedSender {
                    detail: format!(
                        "a message was authenticated to {:?}, which is not a group member; \
                         this SDK issues and expects only member senders",
                        other
                    ),
                });
            }
        };

        // A sender index with no leaf cannot happen for a message OpenMLS just
        // authenticated against that leaf — but resolving it is fallible, and
        // the safe answer to "we cannot see the leaf" is the same as to "the
        // leaf is wrong".
        let member = group
            .member_at(leaf_index)
            .ok_or_else(|| MlsError::UnsupportedSender {
                detail: format!(
                    "tree integrity: a message was authenticated to leaf {}, which is not \
                     in the tree",
                    leaf_index.u32()
                ),
            })?;

        verify_leaf_binding(
            &member.credential,
            &member.signature_key,
            LeafSource::MessageSender,
        )
    }

    /// Requires every leaf a staged commit would install to carry the address
    /// its own signature key derives to (RFC 9420 §7.3, "after processing a
    /// Commit").
    ///
    /// # Why this is unconditional, when the admin check beside it is opt-in
    ///
    /// Both refuse a commit, and refusing a commit means not merging it, which
    /// forks us from every member who did merge. For
    /// [`Self::authorize_membership_commit`] that risk is what keeps it behind
    /// [`GroupManager::set_enforce_admin_commits`]: its verdict depends on the
    /// admin overlay, which replicates best-effort, so two honest members can
    /// legitimately disagree and partition each other with no attacker present.
    ///
    /// This verdict has no such input. It is computed from the commit's own
    /// bytes — a hash of a key in the commit, compared to a string in the
    /// commit — so every honest member reaches the same answer from the same
    /// message. A refused commit does not split the group; it forks the
    /// *attacker* off a group that stays consistent. That is why it is safe to
    /// run for everyone, and why it must: a check the fleet does not run is a
    /// check an attacker chooses to be judged by.
    ///
    /// It also runs for `session:` groups, which the admin check skips. A 1:1
    /// slot is already bound to its pair by `verify_welcome_slot` and the
    /// session-id derivation, so there is nothing extra to catch there today —
    /// but the cost is one hash and the alternative is a seam whose coverage
    /// depends on the group id's spelling.
    ///
    /// # The four sources
    ///
    /// Taken from OpenMLS's own [`StagedCommit::credentials_to_verify`], which
    /// enumerates exactly where a credential can enter through a commit. That
    /// helper hands back bare `Credential`s, which is not enough here — the
    /// whole check is credential *against its signature key* — so the leaves
    /// are walked directly, but the enumeration is copied from it deliberately:
    ///
    /// 1. the commit's update-path leaf (the committer's own, rotated in place);
    /// 2. Update proposals (a member replacing their leaf);
    /// 3. Add proposals (a new member's key package leaf);
    /// 4. `GroupContextExtensions` carrying `ExternalSenders` — not a leaf, and
    ///    refused outright rather than bound, see [`MlsError::UnsupportedSender`].
    ///
    /// Validating only (3) would leave (1) and (2) as bypasses. The OpenMLS
    /// book names only "add & update proposals"; its own helper is the more
    /// complete list, and this follows the helper.
    fn verify_staged_commit_leaves(staged_commit: &StagedCommit) -> Result<()> {
        // 1. The committer's own leaf, rotated in place by the update path.
        if let Some(leaf) = staged_commit.update_path_leaf_node() {
            verify_leaf_binding(
                leaf.credential(),
                leaf.signature_key().as_slice(),
                LeafSource::CommitPath,
            )?;
        }

        // 2. Members replacing their own leaf.
        //
        // Defensive, and unreachable through this SDK today — noted because an
        // untested loop that cannot fire looks identical to one that is merely
        // untested, and a future change re-opens it silently. An Update
        // proposal reaches a commit two ways, and both are currently closed:
        // inline, where MLS attributes the proposal to the *committer* and
        // forbids committing your own Update (OpenMLS drops it when building
        // and answers `CommitterIncludedOwnUpdate` when validating); or by
        // reference, which requires the *receiver* to hold the proposal in its
        // store, and this SDK drops every received `ProposalMessage` rather
        // than storing it (see `decrypt_message`). Give the SDK a propose-only
        // API — or start storing received proposals — and this loop becomes
        // live, which is the point of keeping it.
        for proposal in staged_commit.update_proposals() {
            let leaf = proposal.update_proposal().leaf_node();
            verify_leaf_binding(
                leaf.credential(),
                leaf.signature_key().as_slice(),
                LeafSource::CommitUpdate,
            )?;
        }

        // 3. New members. This is the source the impersonation rides: a member
        // commits an Add whose key package credential names someone else, and
        // from then on SEC-M1 attributes that leaf's messages to them.
        for proposal in staged_commit.add_proposals() {
            let leaf = proposal.add_proposal().key_package().leaf_node();
            verify_leaf_binding(
                leaf.credential(),
                leaf.signature_key().as_slice(),
                LeafSource::CommitAdd,
            )?;
        }

        // 4. External senders authorize a non-member to send into the group.
        // The SDK configures none and issues none, and their credentials are
        // not leaves, so there is no binding to check — refuse instead.
        for proposal in staged_commit.queued_proposals() {
            if let Proposal::GroupContextExtensions(gce) = proposal.proposal() {
                if gce.extensions().external_senders().is_some() {
                    return Err(MlsError::UnsupportedSender {
                        detail: "a commit proposes an ExternalSenders group-context extension, \
                                 which would authorize a non-member to send into this group"
                            .to_string(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Refuses a staged membership commit whose committer or proposal senders
    /// the local admin overlay does not authorize.
    ///
    /// Runs only when [`Self::set_enforce_admin_commits`] is on. **Every
    /// unknown is resolved in favour of merging**, because the cost of a
    /// wrong rejection (a permanent epoch fork needing an app-level
    /// re-invite) far exceeds the cost of a wrong acceptance (an insider
    /// membership change, which the protocol layer still reports):
    ///
    /// - Not a membership commit (no Add/Remove proposals) → merge. A pure
    ///   KeyUpdate changes no membership and needs no admin; the fork
    ///   resolver's deterministic leader issues these and is often not one.
    /// - A `session:` group → merge. 1:1 sessions have no admin overlay.
    /// - Metadata unreadable or absent, or no admin role stored at all →
    ///   merge. This is "we do not know who the admins are", the exact state
    ///   a lagging replica is in, and rejecting on it would partition a
    ///   healthy group with no attacker involved. Note this deliberately does
    ///   *not* consult `created_by`: the creator fallback is a single
    ///   unauthenticated claim, fine for gating our own sends and for
    ///   reporting, too thin to fork a group over.
    ///
    /// Only a *positive* contradiction rejects: we hold a non-empty admin set
    /// and a principal is not in it. The principals are the committer (already
    /// authenticated against the wire sender by SEC-M1 above) and the sender of
    /// each Add/Remove proposal, since MLS lets a member commit a proposal
    /// another member made. Update and PSK proposals are deliberately excluded
    /// — they change no membership.
    ///
    /// It cannot detect *divergent* admin views, only absent ones — which is
    /// why enforcement stays opt-in.
    fn authorize_membership_commit(
        &self,
        group: &MlsGroup,
        group_id: &GroupId,
        staged_commit: &StagedCommit,
        committer: &str,
    ) -> Result<()> {
        if !self.enforce_admin_commits || group_id.as_str().starts_with("session:") {
            return Ok(());
        }

        // Resolve the membership delta from the proposals rather than the
        // post-merge roster, which does not exist yet. Leaf indices resolve
        // against the *pre-merge* tree, which is the roster the removes name.
        let mut added: Vec<String> = staged_commit
            .add_proposals()
            .filter_map(|p| {
                String::from_utf8(
                    p.add_proposal()
                        .key_package()
                        .leaf_node()
                        .credential()
                        .serialized_content()
                        .to_vec(),
                )
                .ok()
            })
            .collect();
        let removed_indices: Vec<LeafNodeIndex> = staged_commit
            .remove_proposals()
            .map(|p| p.remove_proposal().removed())
            .collect();

        if added.is_empty() && removed_indices.is_empty() {
            return Ok(());
        }

        let roster: Vec<(LeafNodeIndex, String)> = group
            .members()
            .filter_map(|m| {
                String::from_utf8(m.credential.serialized_content().to_vec())
                    .ok()
                    .map(|id| (m.index, id))
            })
            .collect();
        let resolve = |index: LeafNodeIndex| -> Option<String> {
            roster
                .iter()
                .find(|(i, _)| *i == index)
                .map(|(_, id)| id.clone())
        };

        let mut removed: Vec<String> = removed_indices
            .iter()
            .copied()
            .filter_map(resolve)
            .collect();

        let metadata = match self.load_group_metadata(group_id) {
            Ok(Some(metadata)) => metadata,
            Ok(None) => {
                warn!(
                    group_id = %group_id,
                    "Commit enforcement: no group metadata — merging (unknown admin set fails open)"
                );
                return Ok(());
            }
            Err(e) => {
                warn!(
                    group_id = %group_id,
                    error = ?e,
                    "Commit enforcement: could not read group metadata — merging (unknown admin set fails open)"
                );
                return Ok(());
            }
        };
        if !metadata.has_any_admin() {
            warn!(
                group_id = %group_id,
                "Commit enforcement: no admin role stored — merging (unknown admin set fails open)"
            );
            return Ok(());
        }

        // Every principal behind the *membership* change must be an admin:
        // the committer, plus the sender of each Add/Remove proposal, since
        // MLS lets a member commit a proposal another member made. A
        // non-member proposal sender (external, new-member) can never be an
        // admin of this group.
        //
        // Scoped to membership proposals on purpose, defensively: a commit may
        // also carry Update or PSK proposals, and an Update is legitimate
        // self-service that needs no admin, so rejecting an admin's Add because
        // it batched a member's key update would fork the group over a proposal
        // that changes no membership. No SDK client produces that shape today —
        // there is no propose-only API, and a received `ProposalMessage` is
        // dropped rather than stored — but the check should not become the
        // thing that forks a group if that ever changes.
        let membership_proposal_senders = staged_commit
            .add_proposals()
            .map(|p| p.sender().clone())
            .chain(staged_commit.remove_proposals().map(|p| p.sender().clone()));
        let mut unauthorized = metadata.get_role(committer) != GroupRole::Admin;
        if !unauthorized {
            unauthorized = membership_proposal_senders.into_iter().any(|s| match s {
                Sender::Member(index) => resolve(index)
                    .map(|id| metadata.get_role(&id) != GroupRole::Admin)
                    .unwrap_or(true),
                _ => true,
            });
        }

        if unauthorized {
            added.sort();
            removed.sort();
            warn!(
                group_id = %group_id,
                committer = %committer,
                added = ?added,
                removed = ?removed,
                "Commit enforcement: refusing an unauthorized membership commit before merge"
            );
            return Err(MlsError::CommitNotAuthorized {
                committer: committer.to_string(),
                added,
                removed,
            });
        }

        Ok(())
    }

    /// Loads a group's metadata overlay (roles, creator) for the admin check.
    ///
    /// Deliberately duplicated from `MlsManager` rather than reached through
    /// it: the manager holds this `GroupManager`, so calling back up would
    /// invert ownership, and the protocol layer holds the manager behind an
    /// `RwLock` that is already locked while decryption runs.
    fn load_group_metadata(&self, group_id: &GroupId) -> Result<Option<GroupMetadata>> {
        let key_type = StorageKeyType::GroupMetadata.as_str();
        match self.storage.load(key_type, group_id.as_str())? {
            Some(data) => {
                let mut metadata: GroupMetadata = serde_json::from_slice(&data)
                    .map_err(|e| MlsError::Deserialization(e.to_string()))?;
                metadata.migrate_legacy_roles();
                Ok(Some(metadata))
            }
            None => Ok(None),
        }
    }

    /// Binds a staged Welcome to the `group_id` the caller authenticated.
    ///
    /// OpenMLS derives the id it persists the joined group under from the
    /// Welcome's embedded `GroupContext` — a value the inviter picks freely at
    /// group creation (`MlsGroup::new_with_group_id`). Our storage marker
    /// ([`save_group`]) and every [`load_group`]/`delete_group` lookup are
    /// instead keyed by the caller-supplied wire `group_id`, and every
    /// SEC-M5/M6 binding (`verify_welcome_slot`, the `session:`
    /// reserved-namespace check) validates only that wire field. If the two
    /// diverge, `into_group` writes the group under the *embedded* id, letting
    /// an authenticated inviter seed or overwrite an arbitrary slot the
    /// wire-id checks never inspected (e.g. embed `session:alice:bob` behind a
    /// benign wire id that passes `verify_welcome_slot`) and hijack the
    /// victim's session. Assert the two agree *before* `into_group` so no
    /// cross-slot state is ever persisted. Legitimate Welcomes always carry
    /// embedded == wire (both created via `new_with_group_id(group_id)`), so
    /// this only rejects forgeries.
    ///
    /// Staging has already consumed the one-time key package by this point;
    /// that only burns the package (inherent to any Welcome processing) and
    /// does not touch existing group state.
    fn verify_staged_group_id(staged: &StagedWelcome, group_id: &GroupId) -> Result<()> {
        let embedded = staged.group_context().group_id().as_slice();
        if embedded != group_id.as_str().as_bytes() {
            return Err(MlsError::WelcomeGroupIdMismatch {
                expected: group_id.to_string(),
                embedded: String::from_utf8_lossy(embedded).into_owned(),
            });
        }
        Ok(())
    }

    /// Requires every leaf in a staged Welcome's ratchet tree to carry the
    /// address its own signature key derives to (RFC 9420 §7.3, "when a client
    /// validates a ratchet tree, e.g., when joining a group").
    ///
    /// The inviter chooses this tree wholesale. Nothing else validates it: the
    /// wire `group_id` is bound by [`Self::verify_staged_group_id`] and the
    /// inviter is authenticated a layer up, but neither says anything about
    /// *who else* the inviter claims is in the room. Without this, an inviter
    /// hands you a group whose roster names your real contacts on leaves the
    /// inviter holds the keys to — and then speaks as them, since the only
    /// thing standing between a leaf and an attributed message is SEC-M1's
    /// comparison against that same unvalidated credential.
    ///
    /// Runs before `into_group`, alongside the group-id binding and for the
    /// same reason: `into_group` is the step that persists, so a tree judged
    /// after it would already be installed. Staging has consumed the one-time
    /// key package by then, which costs the package and nothing else — the same
    /// trade [`Self::verify_staged_group_id`] documents.
    ///
    /// All-or-nothing on purpose. Dropping the offending leaves and joining
    /// anyway would leave us in a group whose tree we know to be forged, at an
    /// epoch every other member computed over the full tree — decrypting
    /// nothing and holding a roster that agrees with nobody.
    fn verify_staged_welcome_tree(staged: &StagedWelcome) -> Result<()> {
        for member in staged.members() {
            verify_leaf_binding(
                &member.credential,
                &member.signature_key,
                LeafSource::WelcomeTree,
            )?;
        }

        // The commit gate refuses a proposal that would *add* an
        // `ExternalSenders` extension; a group created with one already in its
        // context arrives this way instead. Refused here too, so the two entry
        // gates are symmetric rather than differing by which one the inviter
        // chose. Inert today — `verify_sender_leaf` refuses `Sender::External`
        // at use time — but an entry gate that admits what the use gate always
        // refuses is a group we can never fully participate in, and it is
        // better to decline the invite than to hold it.
        if staged
            .group_context()
            .extensions()
            .external_senders()
            .is_some()
        {
            return Err(MlsError::UnsupportedSender {
                detail: "a Welcome's group context carries an ExternalSenders extension, \
                         which would authorize a non-member to send into this group"
                    .to_string(),
            });
        }

        Ok(())
    }

    /// Joins a group using a Welcome message.
    pub fn join_group(&self, welcome: Welcome, group_id: &GroupId) -> Result<MlsGroup> {
        let group_config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .sender_ratchet_configuration(sender_ratchet_configuration())
            .build();

        let staged = StagedWelcome::new_from_welcome(&self.provider, &group_config, welcome, None)
            .map_err(|e| MlsError::WelcomeProcessing(format!("Failed to stage welcome: {}", e)))?;

        // Bind the Welcome's embedded group id to the authenticated wire id
        // before persisting — otherwise `into_group` would install under the
        // attacker-chosen embedded id. See `verify_staged_group_id`.
        Self::verify_staged_group_id(&staged, group_id)?;

        // The inviter picks the whole ratchet tree; judge every identity in it
        // before adopting the roster. See `verify_staged_welcome_tree`.
        Self::verify_staged_welcome_tree(&staged)?;

        let group = staged
            .into_group(&self.provider)
            .map_err(|e| MlsError::WelcomeProcessing(format!("Failed to join group: {}", e)))?;

        self.save_group(group_id, &group)?;

        debug!(group_id = %group_id, "Joined group via Welcome");
        Ok(group)
    }

    /// Joins a group from a Welcome, replacing any existing group at the same
    /// `group_id`. **Non-destructive on the common failure (a duplicate
    /// Welcome); best-effort — NOT atomic — on the rare storage failure.**
    ///
    /// Staging the incoming Welcome (`StagedWelcome::new_from_welcome`) is the
    /// step that consumes the one-time key package, so it is the natural failure
    /// point: if the key package is unavailable — e.g. a retransmitted Welcome we
    /// already adopted, or a first-contact key-package race — staging returns
    /// `Err` **before** we touch the existing group, which is therefore left
    /// completely intact. This makes both-create adoption idempotent: a duplicate
    /// Welcome is a safe no-op rather than a re-brick, the case this method exists
    /// to handle.
    ///
    /// Caveat — once staging succeeds the swap is **not atomic**. We delete the
    /// prior group (to avoid orphaning its old-leaf epoch encryption keypairs,
    /// which are keyed by `(group_id, epoch, leaf_index)`; the adopted group
    /// installs at a different leaf and so would not overwrite them) and then call
    /// `into_group`, which performs several sequential, non-transactional storage
    /// writes. If one of those fails (e.g. a Keychain/Keystore write error), the
    /// old group is already gone and the new one is absent — a lost session. The
    /// key package is spent by then, so the same Welcome cannot simply be retried;
    /// recovery is a fresh key-package exchange. The caller distinguishes this
    /// "lost group" outcome from a benign duplicate by observing `has_session`
    /// (false here, true for a duplicate) and surfaces it as a session failure.
    pub fn join_group_replacing(&self, welcome: Welcome, group_id: &GroupId) -> Result<MlsGroup> {
        let group_config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .sender_ratchet_configuration(sender_ratchet_configuration())
            .build();

        // Stage first — non-destructive failure point (consumes the key package).
        // A failure here leaves the existing group untouched (benign duplicate).
        let staged = StagedWelcome::new_from_welcome(&self.provider, &group_config, welcome, None)
            .map_err(|e| MlsError::WelcomeProcessing(format!("Failed to stage welcome: {}", e)))?;

        // Bind the Welcome's embedded group id to the authenticated wire id
        // before the destructive replace below. A forged Welcome whose embedded
        // id differs from `group_id` would otherwise `into_group` under the
        // embedded id and hijack that slot; rejecting here also leaves the
        // existing group at `group_id` untouched (no `delete_group` runs). See
        // `verify_staged_group_id`.
        Self::verify_staged_group_id(&staged, group_id)?;

        // Same placement, same reason: refusing a forged tree here leaves the
        // existing group intact, because the destructive `delete_group` below
        // has not run yet. See `verify_staged_welcome_tree`.
        Self::verify_staged_welcome_tree(&staged)?;

        // Staging consumed the key package; from here a failure is not
        // recoverable by retrying the same Welcome. Drop the prior group so its
        // old-leaf epoch keypairs are not orphaned. `delete_group` already
        // no-ops on absence and tolerates a corrupt prior group, so call it
        // unconditionally — a redundant `load_group` here would only add another
        // fallible storage read that could abort the join with the key package
        // already spent.
        self.delete_group(group_id)?;

        let group = staged.into_group(&self.provider).map_err(|e| {
            // Distinct message from the staging failure above: the prior group is
            // already deleted and the key package is spent, so this is the
            // non-atomic "lost group" window, not a benign duplicate.
            MlsError::WelcomeProcessing(format!(
                "Failed to install adopted group (prior group removed, key package consumed): {}",
                e
            ))
        })?;

        self.save_group(group_id, &group)?;

        debug!(group_id = %group_id, "Joined/adopted group via Welcome (non-destructive stage, best-effort swap)");
        Ok(group)
    }

    /// Gets information about a group.
    ///
    /// Members whose leaf does not carry the address its own signature key
    /// derives to are skipped with a warning, the same disposition
    /// [`Self::list_groups`] gives a stored group id that fails validation.
    ///
    /// Post-gates such a leaf cannot arrive over the wire, so one appearing
    /// here means local group state was written behind the SDK's back. Keeping
    /// it out of the roster matters because the roster is not only what the app
    /// displays: it addresses the per-member fan-out, feeds the group's
    /// rich-payload capability gate, and supplies the candidates for the
    /// address-ordered tiebreakers (leave election, admin auto-promotion, fork
    /// leader). A forged entry in it is not cosmetic.
    ///
    /// Skipping rather than failing keeps a tampered store from taking the
    /// whole group down, and the message it would have carried is refused
    /// anyway — [`Self::verify_sender_leaf`] judges the same leaf at decrypt.
    ///
    /// O(N) in the roster, against [`Self::verify_sender_leaf`]'s O(1): one
    /// SHA-256 over 32 bytes plus a bech32m encode per member, per call, and
    /// `process_commit_core` calls this twice per commit to derive a membership
    /// delta. Immaterial next to the MLS crypto on the same path — microseconds
    /// at a hundred members — but this is the expensive seam of the two, so do
    /// not reach for it in a loop.
    pub fn get_group_info(&self, group: &MlsGroup, group_id: &GroupId) -> GroupInfo {
        let members: Vec<String> = group
            .members()
            .filter_map(|m| {
                if let Err(e) =
                    verify_leaf_binding(&m.credential, &m.signature_key, LeafSource::RosterEntry)
                {
                    warn!(
                        group_id = %group_id,
                        error = %e,
                        "Skipping a group member whose leaf does not prove its own identity"
                    );
                    return None;
                }
                let credential = m.credential.serialized_content();
                String::from_utf8(credential.to_vec()).ok()
            })
            .collect();

        let is_session = group_id.as_str().starts_with("session:");
        let members_count = members.len() as u32;

        GroupInfo {
            group_id: group_id.clone(),
            name: None,
            members,
            members_count,
            epoch: group.epoch().as_u64(),
            is_session,
            created_at_ms: 0,
            last_activity_ms: chrono::Utc::now().timestamp_millis() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryStorage;
    use crate::storage_adapter::MlsStorageAdapter;

    fn create_test_group_manager() -> GroupManager {
        let storage = Arc::new(InMemoryStorage::new());
        let adapter = MlsStorageAdapter::new(storage.clone());
        let provider = MlsProvider::new(adapter);
        GroupManager::new(storage, provider)
    }

    #[test]
    fn test_group_manager_creation() {
        let manager = create_test_group_manager();
        assert!(manager.list_groups().unwrap().is_empty());
    }

    /// An oversized credential is bounded before it reaches a log line or a
    /// user-facing `security_warning` reason.
    ///
    /// A credential is TLS `VLBytes` and a leaf reaching the binding check has
    /// proved nothing, so its content is attacker-chosen and unbounded on the
    /// wire. Both sinks interpolate it verbatim.
    #[test]
    fn an_oversized_credential_is_truncated_in_the_error() {
        let huge = vec![b'A'; 4096];
        let credential = Credential::new(CredentialType::Basic, huge);

        let err = verify_leaf_binding(&credential, &[0u8; 31], LeafSource::WelcomeTree)
            .expect_err("a leaf claiming 4 KiB of nonsense must not pass");
        let MlsError::LeafAddressMismatch { claimed, .. } = &err else {
            panic!("got {err:?}");
        };
        assert!(
            claimed.len() < 128,
            "an unbounded credential reached the error text: {} bytes",
            claimed.len()
        );
        assert!(claimed.ends_with("… (truncated)"), "{claimed}");

        // An honest address is well under the cap and must survive intact.
        let keys = openmls_basic_credential::SignatureKeyPair::new(
            DEFAULT_CIPHERSUITE.signature_algorithm(),
        )
        .unwrap();
        let address = crate::MlsManager::derive_address(keys.public()).unwrap();
        let honest = Credential::new(
            CredentialType::Basic,
            address.to_string().as_bytes().to_vec(),
        );
        let err = verify_leaf_binding(&honest, &[0u8; 31], LeafSource::WelcomeTree)
            .expect_err("a key that cannot derive must fail regardless");
        let MlsError::LeafAddressMismatch { claimed, .. } = &err else {
            panic!("got {err:?}");
        };
        assert_eq!(
            *claimed,
            address.to_string(),
            "a normal address must be reported verbatim"
        );
    }

    /// A leaf whose signature key cannot yield an address at all is refused,
    /// not waved through.
    ///
    /// Unreachable through the wire — OpenMLS validates a key package's
    /// signature key against the ciphersuite before any of this runs, so a
    /// wrong-length key never reaches a tree — which is exactly why it is
    /// tested directly: the arm is defensive, and an untested fail-open written
    /// there would look identical to an untested fail-closed one. "No address
    /// could be derived" is not "nothing to check", it is a leaf that proved
    /// nothing.
    #[test]
    fn undecodable_signature_key_is_refused_not_skipped() {
        let credential = Credential::new(CredentialType::Basic, b"off1whatever".to_vec());
        // 31 bytes: one short of an Ed25519 public key.
        let err = verify_leaf_binding(&credential, &[0u8; 31], LeafSource::WelcomeTree)
            .expect_err("a key that cannot derive an address must not pass");
        assert!(
            matches!(err, MlsError::LeafAddressMismatch { ref derived, .. }
                if derived.contains("no address")),
            "got {err:?}"
        );
    }

    /// The honest case, pinned next to it so the helper is not merely always
    /// failing.
    #[test]
    fn a_leaf_carrying_its_own_derived_address_passes() {
        use openmls_basic_credential::SignatureKeyPair;

        let keys = SignatureKeyPair::new(DEFAULT_CIPHERSUITE.signature_algorithm()).unwrap();
        let address = crate::MlsManager::derive_address(keys.public()).unwrap();
        let credential = Credential::new(
            CredentialType::Basic,
            address.to_string().as_bytes().to_vec(),
        );
        verify_leaf_binding(&credential, keys.public(), LeafSource::WelcomeTree)
            .expect("a leaf that derives its own credential must pass");
    }
}
