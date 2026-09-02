//! Control message signing, verification, and peer identity derivation.

use super::storage::MAX_RESTORE_KEYS_PER_CATEGORY;
use super::{
    base64_decode, base64_encode, internal_prefixes, storage_keys, ControlGateOutcome,
    ControlSigVersion, ControlVerification, EncryptionCapableEntry, InternalMessageResult,
    OfflineProtocol, CTRL_PK_META_KEY, CTRL_SIG_META_KEY, DATA_PLANE_PREFIXES, INTERNAL_PREFIXES,
    MAX_CONTROL_GATE_WARNED_PEERS, MAX_PLAINTEXT_RECEIVE_WARNED_PEERS, RELAY_ANSWER_PREFIXES,
};
use crate::events::{Event, SecurityWarningCode};
use crate::{Error, Result};
use chrono::Utc;
use offline_protocol_core::{Address, Message, UserId};
use offline_protocol_sealed::{gateway_address_proof_payload, Freshness};
use offline_protocol_transport::TransportType;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Minimum gap between [`SecurityWarningCode::PushKeyPackagePoolExhausted`]
/// emissions. Matches the suppression the Nostr slot-exhaustion and
/// unauthorized-membership reports use, for the same reason: the condition
/// persists, so an unsuppressed warning reports one cause many times.
const PUSH_KEY_PACKAGE_WARNING_SUPPRESS_INTERVAL: Duration = Duration::from_secs(300);

/// Minimum interval between two `GatewayAddressDeclarationRefused` warnings.
///
/// A refusing gateway is retried on the reconnect ladder, which tops out at
/// 30s, so without this the app is told about one misconfigured box twice a
/// minute for as long as the transport is enabled. The condition persists,
/// and the second report says nothing the first did not; the refusal itself
/// is still logged every time.
const GATEWAY_REFUSAL_WARNING_SUPPRESS_INTERVAL: Duration = Duration::from_secs(300);

/// Bytes of challenge a gateway mints per connection, from the contract.
///
/// A device refuses to sign a proof over anything else: the challenge is the
/// whole of the replay bound, and one that is shorter than this is either a
/// broken gateway or one trying to pick the bytes that go under our key.
pub const GATEWAY_CHALLENGE_LEN: usize = 32;

/// What a gateway's address echo says about this device, as a decision
/// separate from how any one carrier reports it.
///
/// Private to this module: the relay and the daemon contract ask the same
/// question and answer it with different text and different codes, and the
/// question is the part that must not drift between them.
enum AddressBinding<'a> {
    /// The echoed address is this device's.
    Ours,
    /// The echoed address is some other address, and this device holds
    /// `local`. There is no benign reading: the gateway re-derives the
    /// address from the key that signed the proof, so an echo naming another
    /// address is a binding it never verified.
    NotOurs { local: &'a str },
    /// This device has no address yet, so nothing local can be compared. That
    /// is itself the finding: the bridges do not declare before an identity
    /// exists, so an echo answers a declaration that was never sent.
    NoIdentity,
}

/// What [`OfflineProtocol::judge_control_frame_freshness`] concluded.
///
/// Private to this module: it is the gate's own vocabulary for a decision it
/// then reports outward as `freshness_bound` plus an accept-or-drop.
enum FreshnessVerdict {
    /// Signed under the freshness-bound payload, inside the window.
    Bound,
    /// Authentic, with its age not established: signed under the older
    /// payload, or under either one with the freshness check switched off.
    ///
    /// Accepted, and it carries **no timestamp onward**, which is the point.
    /// What a directive that destroys state may then do is the dispatch site's
    /// decision, not this one: a peer that has never proved the newer payload
    /// keeps its reset, and a peer that has proved it does not. Either way
    /// nothing here can move a replay mark, because there is no stamp this
    /// verdict vouches for.
    Unbound,
    /// Refused. The caller drops the frame without acknowledging it.
    Refused,
}

impl OfflineProtocol {
    /// Builds the canonical signing payload a control frame is authenticated
    /// over: `CTRL_SIGN_DOMAIN` followed by sender, message id, recipient and
    /// content, each as `<4-byte big-endian length><utf-8 bytes>`, which makes
    /// the encoding unambiguous regardless of field content (no
    /// delimiter-collision risk).
    ///
    /// The construction itself lives in
    /// [`offline_protocol_sealed::control_signing_payload`], with every other
    /// signing domain in the protocol, so a leaf node signs and verifies the
    /// same bytes. Both the signer and the verifier here call this one
    /// function: a verifier that rebuilds the payload from its own copy of the
    /// field order starts accepting forgeries the moment the two drift.
    pub(crate) fn build_canonical_payload(message: &Message) -> Result<Vec<u8>> {
        offline_protocol_sealed::control_signing_payload(message)
            .map_err(|e| Error::Other(e.to_string()))
    }

    /// Builds the freshness-bound signing payload: the four fields above plus
    /// the frame's timestamp, under `offline-ctrl-v2`.
    ///
    /// Lives in the sealed layer beside its v1 sibling for the same reason
    /// that one does, and the reason is sharper here: a leaf node has to
    /// reproduce these bytes from a different MLS implementation, and a
    /// verifier whose idea of the payload drifts from the signer's does not
    /// fail loudly, it fails as "this peer's signatures are all invalid".
    pub(crate) fn build_canonical_payload_v2(message: &Message) -> Result<Vec<u8>> {
        offline_protocol_sealed::control_signing_payload_v2(message)
            .map_err(|e| Error::Other(e.to_string()))
    }

    /// Signs a control message by adding an Ed25519 signature and the sender's
    /// public key to the message metadata.
    ///
    /// Returns `Ok(())` on success (or when MLS is not initialized — unsigned
    /// messages are acceptable for backward compatibility with pre-MLS peers).
    /// Returns `Err` when MLS is initialized but signing fails, so the caller
    /// can decide whether to abort the send.
    pub(super) fn sign_control_message(&self, message: &mut Message) -> Result<()> {
        let mls = match self.mls_manager.as_ref() {
            Some(m) => m,
            None => {
                debug!("MLS not initialized — sending unsigned control message");
                return Ok(());
            }
        };
        let manager = match mls.read() {
            Ok(guard) => guard,
            Err(e) => {
                let reason = format!("MLS lock poisoned — cannot sign control message: {}", e);
                error!(error = %e, "MLS lock poisoned — cannot sign control message");
                return Err(Error::Other(reason));
            }
        };

        let freshness_bound = self.signs_freshness_bound_control_to(message.recipient.as_str());
        Self::sign_control_message_with(message, &manager, freshness_bound)
    }

    /// Whether a control frame addressed to `recipient` should carry the
    /// freshness-bound signature.
    ///
    /// Signing v2 at a peer that cannot verify it makes every control frame we
    /// send them fail their signature check, which reads on their side as an
    /// attack rather than a version gap. So the newer payload is used only
    /// where the recipient said it verifies one, on the one channel it has for
    /// saying so.
    ///
    /// # First contact necessarily signs v1
    ///
    /// The first key package to a peer we have never met is signed under the
    /// old payload, because their capabilities arrive *in their reply*. That
    /// is inherent rather than a gap to close: nothing can know a stranger's
    /// capabilities before meeting them. It converges in one round trip, and
    /// the ratchet closes behind it — once a peer's v2 signature has verified
    /// once, their v1 frames are refused, so the first-contact frame cannot be
    /// replayed at us later.
    ///
    /// # Frames addressed to ourselves
    ///
    /// A relay hint is addressed to `local_id` and comes back to us if a
    /// prefix-unaware relay echoes it, so the only possible verifier is this
    /// node, whose capability is not in doubt.
    pub(crate) fn signs_freshness_bound_control_to(&self, recipient: &str) -> bool {
        recipient == self.local_id || self.peer_ctrl_freshness.contains(recipient)
    }

    /// Stamps `message` with an Ed25519 signature and public key from
    /// `manager`'s identity.
    ///
    /// Split out from [`Self::sign_control_message`] so a caller that already
    /// holds the signing identity — a test standing in for a peer's device —
    /// can produce the same bytes the peer's own instance would, rather than
    /// reimplementing the canonical payload and getting it subtly wrong.
    ///
    /// `freshness_bound` selects the payload: the one that covers the frame's
    /// timestamp, or the one that does not. It is a parameter rather than a
    /// lookup because this function has no `self` to look anything up on, and
    /// making it guess would put the choice in two places.
    pub(crate) fn sign_control_message_with(
        message: &mut Message,
        manager: &offline_protocol_mls::MlsManager,
        freshness_bound: bool,
    ) -> Result<()> {
        let public_key = match manager.get_identity_public_key() {
            Ok(pk) => pk,
            Err(e) => {
                let reason = format!("Failed to get identity public key: {}", e);
                error!(error = %e, "Failed to get identity public key — cannot sign control message");
                return Err(Error::Other(reason));
            }
        };
        let canonical = if freshness_bound {
            Self::build_canonical_payload_v2(message)?
        } else {
            Self::build_canonical_payload(message)?
        };
        let signature = match manager.sign_data(&canonical) {
            Ok(sig) => sig,
            Err(e) => {
                let reason = format!("Failed to sign control message: {}", e);
                error!(error = %e, "Failed to sign control message");
                return Err(Error::Other(reason));
            }
        };

        message
            .metadata
            .insert(CTRL_SIG_META_KEY.to_string(), base64_encode(&signature));
        message
            .metadata
            .insert(CTRL_PK_META_KEY.to_string(), base64_encode(&public_key));
        Ok(())
    }

    /// Verifies a control message's Ed25519 signature from metadata, and that
    /// the key which signed it is the one the claimed sender's address names.
    ///
    /// Returns:
    /// - `Ok(Verified(version))` — valid signature, and the signing key derives
    ///   to `sender`; `version` says which canonical payload it was over
    /// - `Ok(Unsigned)` — no signature metadata at all
    /// - `Err(..)` — signature invalid, sender/key derivation mismatch, or
    ///   malformed metadata (e.g. public key present without a signature,
    ///   or vice versa)
    ///
    /// # Why both payloads are tried
    ///
    /// A signature does not say which byte string it was made over, so a
    /// verifier holding one candidate payload cannot distinguish "signed under
    /// the other domain" from "forged". Trying the freshness-bound payload and
    /// then the older one is what turns a version gap into a version answer.
    /// The newer one goes first so the common case costs one verification, and
    /// so that a frame which satisfies both — impossible, the domains differ —
    /// would be read as the stronger claim rather than the weaker.
    ///
    /// The second verification is only reached by a frame that failed the
    /// first, which is either a legacy peer or a forgery. Both are rare
    /// relative to control traffic as a whole, and a control frame is not on
    /// the message path, so the cost is one extra Ed25519 verification on a
    /// path that already does a SHA-256 and an address parse.
    pub(super) fn verify_control_message(
        &mut self,
        message: &Message,
    ) -> Result<ControlVerification> {
        let sig_b64 = match message.metadata.get(CTRL_SIG_META_KEY) {
            Some(s) => s,
            None => {
                // If the public key metadata is present without a signature,
                // treat this as a malformed message rather than unsigned.
                if message.metadata.contains_key(CTRL_PK_META_KEY) {
                    return Err(Error::Other(
                        "Control message has public key but missing signature (malformed)"
                            .to_string(),
                    ));
                }
                // Truly unsigned — caller decides policy.
                return Ok(ControlVerification::Unsigned);
            }
        };
        let pk_b64 = match message.metadata.get(CTRL_PK_META_KEY) {
            Some(s) => s,
            None => {
                return Err(Error::Other(
                    "Control message has signature but missing public key".to_string(),
                ));
            }
        };

        let signature = base64_decode(sig_b64)
            .map_err(|e| Error::Other(format!("Invalid control signature encoding: {}", e)))?;
        let public_key = base64_decode(pk_b64)
            .map_err(|e| Error::Other(format!("Invalid control public key encoding: {}", e)))?;

        // Verify the Ed25519 signature over a length-prefixed canonical
        // payload. The freshness-bound one binds sender, message ID,
        // recipient, content and the frame's timestamp; the older one binds
        // every field but the last.
        let verifies = |payload: &[u8]| -> Result<bool> {
            offline_protocol_mls::MlsManager::verify_signature(&public_key, payload, &signature)
                .map_err(|e| Error::Other(format!("Signature verification error: {}", e)))
        };

        let version = if verifies(&Self::build_canonical_payload_v2(message)?)? {
            ControlSigVersion::V2
        } else if verifies(&Self::build_canonical_payload(message)?)? {
            ControlSigVersion::V1
        } else {
            return Err(Error::Other(
                "Control message signature verification failed".to_string(),
            ));
        };

        // The claim is now *proved*, not pinned: the address the frame claims
        // to come from is the hash of the key that just signed it, or it is
        // not this peer's frame.
        self.verify_sender_derivation(message.sender.as_str(), &public_key)?;

        // Reaching here means an Ed25519 signature over the canonical payload
        // verified against a key that derives to the claimed sender, and that
        // key *is* the peer's MLS identity key (`get_identity_public_key`
        // returns the credential's `signature_key`). So this is proof the peer
        // runs MLS.
        self.record_encryption_capable(message.sender.as_str());

        // And, when the signature was over the freshness-bound payload, that
        // this peer can produce one. This is the ratchet: from here on their
        // frames under the older payload are refused, which is what stops the
        // whole check being side-stepped with a recording made before they
        // upgraded.
        if version == ControlSigVersion::V2 {
            self.record_control_freshness_proved(message.sender.as_str());
        }
        Ok(ControlVerification::Verified(version))
    }

    /// Requires `public_key` to derive to the address in `sender`.
    ///
    /// This one function is what the TOFU pin store used to be. The store held
    /// a key per peer so a later frame could be compared against the first one
    /// seen; an address makes that comparison unnecessary, because the address
    /// already *is* `bech32m(0x01 ‖ SHA-256(key)[..20])`. Impersonation stops
    /// costing "be the first to claim the name" and starts costing a 160-bit
    /// second preimage (~2^160). Note that this is the *targeted* figure — the
    /// birthday bound on the same truncation is ~2^80, which buys two keys
    /// under one address rather than a specific peer's address; see
    /// `Address::HASH_LEN` for why that trade was taken.
    ///
    /// # Why an unparseable sender is rejected
    ///
    /// A sender that is not an address has no derivation to check, and the
    /// tempting answer — pass, there is nothing to compare — is the whole
    /// bypass: an attacker would claim a nickname and skip the gate. So a
    /// control frame whose sender is not an address is refused outright. This
    /// is also what makes the check unconditional in the sense that matters:
    /// there is no input for which it declines to run.
    fn verify_sender_derivation(&mut self, sender: &str, public_key: &[u8]) -> Result<()> {
        let claimed = match sender.parse::<Address>() {
            Ok(claimed) => claimed,
            Err(e) => {
                // Reported like the mismatch arm below, and for the same
                // reason: this is the other half of "the claim cannot be bound
                // to the key that signed it". The event stays identifier-free
                // — `sender` here is an unparseable, attacker-chosen string, so
                // rendering it would put arbitrary wire text on the sink. The
                // rendered error below reaches the device log through the
                // caller's `warn!`.
                self.warn_control_gate_rejection(
                    sender,
                    SecurityWarningCode::SenderAddressMismatch,
                    "Control message sender is not an address, so no signing key can be bound \
                     to the claim",
                );
                return Err(Error::Other(format!(
                    "Control message sender '{}' is not an address: {}",
                    sender, e
                )));
            }
        };

        let derived = offline_protocol_mls::MlsManager::derive_address(public_key)
            .map_err(|e| Error::Other(format!("Cannot derive an address from the key: {}", e)))?;

        if derived != claimed {
            warn!(
                sender = %sender,
                derived = %derived,
                "Control message signing key does not derive to the claimed sender"
            );
            self.warn_control_gate_rejection(
                sender,
                SecurityWarningCode::SenderAddressMismatch,
                "Signing key does not derive to the claimed sender address (impersonation attempt)",
            );
            return Err(Error::Other(format!(
                "Sender address mismatch: '{}' claimed, key derives to '{}'",
                sender, derived
            )));
        }
        Ok(())
    }

    /// Marks `peer_id` encryption-capable, in memory and durably.
    ///
    /// The durable half is not bookkeeping: `encryption_capable_peers` is what
    /// keeps the plaintext-downgrade gate shut, and a session can be torn down
    /// remotely, so without a record that outlives the session the next launch
    /// would come up knowing nothing about a peer it had verified and re-open
    /// the gate for them. The TOFU store used to be that record as a side
    /// effect of holding pins; it is now the only thing this category does.
    ///
    /// # Why the write is gated on the in-memory mark
    ///
    /// `mark_encryption_capable` refuses past
    /// [`MAX_ENCRYPTION_CAPABLE_PEERS`](super::MAX_ENCRYPTION_CAPABLE_PEERS),
    /// and persisting regardless would leave the durable category unbounded
    /// while the set it feeds is capped. That gap is reachable: minting an
    /// identity that signs honestly as itself costs one Ed25519 keygen, so an
    /// attacker can write records without limit. The damage is not the disk —
    /// it is that `restore_encryption_capable_peers` reads only the first
    /// [`MAX_RESTORE_KEYS_PER_CATEGORY`] keys the store lists, so a flooded
    /// category can push a legitimate peer's record out of the restore window
    /// and re-open the plaintext gate for them on the next launch. Writing only
    /// what the capped set accepted holds the category under that cap, which is
    /// what makes the restore walk's "durable records are first in line" claim
    /// true rather than aspirational. The deleted TOFU store had this property
    /// too — its store-full branch declined to pin *and* to persist.
    ///
    /// # Why the write is skipped once the record is known durable
    ///
    /// The record's presence *is* the fact; its `last_seen_ms` is diagnostic and
    /// nothing reads it back as a policy input (see [`EncryptionCapableEntry`]).
    /// Rewriting it on every verified frame would mean a synchronous credential-
    /// store write per control message — on iOS and Android a Keychain/Keystore
    /// round-trip — to refresh a field no decision depends on, and the gated
    /// prefixes include the chatty ones (`__TYPING__`, `__READ_RECEIPT__`,
    /// `__PRESENCE__`). The pin store this replaced did rewrite per frame, but
    /// it had to: the timestamp drove its LRU eviction. Nothing evicts here.
    ///
    /// The skip keys on [`Self::encryption_capable_persisted`] — writes that
    /// actually landed — rather than on set membership, because
    /// [`Self::persist_encryption_capable`] is best-effort. Keying on the
    /// in-memory set would let one transient storage failure strand the peer
    /// unwritten for the rest of the process, which is precisely the state that
    /// re-opens the plaintext gate for them on the next launch. Keyed this way
    /// a failed write is simply retried on their next verified frame.
    ///
    /// # The mark is never skipped, only the write
    ///
    /// Note the ordering: `mark_encryption_capable` runs *before* the cache is
    /// consulted. Returning early on a cache hit would skip the mark too, and
    /// the two can legitimately disagree — a failed `initialize_mls` rolls
    /// `encryption_capable_peers` back to its pre-restore snapshot while the
    /// cache still remembers the records restore read off disk. A peer in that
    /// window would then be waved through here without ever entering the set
    /// that holds the plaintext gate shut for them: the exact fail-open this
    /// field exists to prevent, bought for a saved write.
    pub(super) fn record_encryption_capable(&mut self, peer_id: &str) {
        if !self.mark_encryption_capable(peer_id) {
            return;
        }
        if self.encryption_capable_persisted.contains(peer_id) {
            return;
        }
        if self.persist_encryption_capable(peer_id) {
            self.encryption_capable_persisted
                .insert(peer_id.to_string());
        }
    }

    /// Security gate for control messages. Validates transport-level sender
    /// identity and cryptographic signature before allowing the message to
    /// proceed through `process_internal_message`.
    ///
    /// Returns [`ControlGateOutcome::Rejected`] to drop the message (without
    /// sending a delivery ACK), or [`ControlGateOutcome::Proceed`] to allow it
    /// through — carrying whether the frame was actually signed, so a handler
    /// that treats a payload field as authenticated can check rather than
    /// assume. See [`ControlGateOutcome::Proceed`] for what that bit does and
    /// does not mean.
    pub(super) fn security_gate_control_message(
        &mut self,
        message: &Message,
        arrival_transport: Option<TransportType>,
    ) -> ControlGateOutcome {
        let content = &message.content;
        let sender = message.sender.as_str();

        if !Self::is_security_gated_prefix(content) {
            // Not a security-gated control message — no gate needed, and
            // nothing verified, so `signed` stays false.
            return ControlGateOutcome::Proceed {
                signed: false,
                freshness_bound: false,
            };
        }

        // Transport-level identity check
        if !self.validate_transport_sender(message) {
            warn!(
                sender = %sender,
                message_id = %message.id,
                "Dropping control message: sender/transport identity mismatch or missing"
            );
            self.warn_control_gate_rejection(
                sender,
                SecurityWarningCode::TransportIdentityMismatch,
                "Control message sender does not match transport peer identity",
            );
            return ControlGateOutcome::Rejected(InternalMessageResult::SecurityRejected);
        }

        // Log telemetry when transport identity is absent (passed best-effort).
        // This is not emitted as a SecurityWarning event because relayed/forwarded
        // messages routinely lack transport_peer_id and flooding the event stream
        // would desensitize operators to real warnings.
        if message.transport_peer_id().is_none() {
            debug!(
                sender = %sender,
                message_id = %message.id,
                require_identity = %self.config.security.require_transport_identity,
                "Control message passed without transport peer identity (best-effort)"
            );
        }

        // Cryptographic signature check
        let (signed, freshness_bound) = match self.verify_control_message(message) {
            Ok(ControlVerification::Verified(version)) => {
                match self.judge_control_frame_freshness(message, version) {
                    FreshnessVerdict::Bound => (true, true),
                    FreshnessVerdict::Unbound => (true, false),
                    FreshnessVerdict::Refused => {
                        return ControlGateOutcome::Rejected(
                            InternalMessageResult::SecurityRejected,
                        )
                    }
                }
            }
            Ok(ControlVerification::Unsigned)
                if Self::is_unsignable_relay_answer(message, arrival_transport) =>
            {
                // A relay-originated answer, which no peer signed because no
                // peer sent it. See `RELAY_ANSWER_PREFIXES` for why this cannot
                // be signature-gated and what does and does not protect it.
                debug!(
                    sender = %sender,
                    message_id = %message.id,
                    "Accepting unsigned relay-originated control frame (no peer signs these)"
                );
                // Nothing signed it, so nothing bound its age either. The
                // relay-answer forgery residual (threat model R1) is unchanged
                // by any of this: a frame with no signer cannot be given one.
                (false, false)
            }
            Ok(ControlVerification::Unsigned) => {
                // Unsigned control traffic is refused, unconditionally.
                //
                // This used to be a two-part policy: reject if the sender had a
                // pin (a known-signed peer going quiet is a downgrade), else
                // reject only under `require_transport_identity`. Both halves
                // existed because an unsigned frame from a peer we knew nothing
                // about was, at the time, indistinguishable from a legacy peer
                // — there was no way to check an identity claim without prior
                // contact, so refusing would have meant refusing first contact.
                //
                // Derived addresses remove that excuse: a signature is now
                // *self-verifying* against the sender's own id, so a peer that
                // will not sign is a peer making an unprovable claim, whether
                // or not we have met them. Leaving any accepting path here
                // would also leave the forged-`hop_count` bypass open — a
                // spoofer who sets `hop_count > 0` skips the transport-identity
                // strict match and lands exactly on this branch, and would then
                // impersonate any peer without committing a signing key at all.
                warn!(
                    sender = %sender,
                    message_id = %message.id,
                    "Dropping unsigned control message: control traffic must be signed"
                );
                self.warn_control_gate_rejection(
                    sender,
                    SecurityWarningCode::UnsignedControlRejected,
                    "Unsigned control message rejected: control traffic must carry a signature \
                     from the key its sender address derives from",
                );
                return ControlGateOutcome::Rejected(InternalMessageResult::SecurityRejected);
            }
            Err(err) => {
                // Signature invalid, derivation mismatch, or malformed metadata — drop
                warn!(
                    sender = %sender,
                    message_id = %message.id,
                    error = %err,
                    "Dropping control message: signature verification failed"
                );
                // Classified, not rendered. `err` names both the claimed sender
                // and — on the derivation-mismatch arm — the address the
                // signing key really derives to, and a `SecurityWarning`'s
                // `reason` is shipped verbatim by the telemetry scrubber (only
                // `peer_id`, the *claimed* sender, is hashed). Rendering it
                // here would hand the sink the pair that de-anonymizes that
                // hash, beside the deliberately identifier-free event
                // `verify_sender_derivation` already emitted for the same
                // refusal. The full error is in the `warn!` above.
                self.warn_control_gate_rejection(
                    sender,
                    SecurityWarningCode::ControlSignatureInvalid,
                    "Control message rejected: its signature, signing metadata, or \
                     sender/key derivation failed verification",
                );
                return ControlGateOutcome::Rejected(InternalMessageResult::SecurityRejected);
            }
        };

        ControlGateOutcome::Proceed {
            signed,
            freshness_bound,
        }
    }

    /// Decides what a verified control frame's age means for it.
    ///
    /// Three outcomes, because a verified signature now carries three
    /// different amounts of weight:
    ///
    /// - [`FreshnessVerdict::Bound`]: signed under the freshness-bound payload
    ///   and inside the window, so it is provably not a recording. Only such a
    ///   frame may carry a directive that destroys state.
    /// - [`FreshnessVerdict::Unbound`]: authentic, with its age not
    ///   established. Everything a control frame did before this existed, it
    ///   still does, and no timestamp travels with it. Whether it may tear a
    ///   session down is decided downstream, by the ratchet and the switch,
    ///   because a peer that has proved nothing must keep its resets or it can
    ///   never heal a forked session.
    /// - [`FreshnessVerdict::Refused`]: the frame states an age this node will
    ///   not accept, or comes under the older payload from a peer that has
    ///   proved it can produce the newer one.
    ///
    /// # The key-package escape
    ///
    /// A peer held to the newer payload is refused *except* on
    /// `__MLS_KEY_PKG__`, which is admitted and then has its `session_reset`
    /// ignored (the `Unbound` verdict is what conveys that downstream). The
    /// escape exists because the ratchet would otherwise be a trap with no
    /// way out: a peer whose record of *us* was lost signs the older payload,
    /// because it no longer knows we accept the newer one, and is refused on
    /// the very frame that would have told it. Since a key package is also the
    /// only frame that re-teaches capabilities, refusing it makes the state
    /// permanent. See the escape itself for what actually loses that record —
    /// not a reinstall, which arrives as a different address entirely.
    ///
    /// Admitting it costs nothing an attacker wants: a key package with its
    /// reset ignored advertises capabilities, which is the thing this protocol
    /// treats as unauthenticated hint data everywhere else, and its destructive
    /// half stays shut.
    fn judge_control_frame_freshness(
        &mut self,
        message: &Message,
        version: ControlSigVersion,
    ) -> FreshnessVerdict {
        let sender = message.sender.as_str();

        if !self.config.security.control_freshness_enforced {
            // The switch returns this node to its pre-403 behaviour exactly:
            // signatures verified, no frame refused for its age, and a reset
            // honoured on any verified frame.
            //
            // `Unbound` rather than `Bound`, and the difference is not
            // cosmetic. What keeps resets working with the switch off is the
            // dispatch site's own `!control_freshness_enforced` clause, not
            // this verdict; reporting `Bound` here would additionally hand the
            // handler the frame's timestamp as though the signature covered
            // it, and on a v1 frame it does not. That is the failure this
            // change calls worse than the replay it fixes: the stamp is
            // metadata anyone can rewrite, so one captured v1 reset frame
            // parks the peer's durable high-water mark at `i64::MAX` and
            // denies that peer every future reset, across restarts and across
            // the switch being turned back on.
            //
            // A clock is the reason this switch exists, so the honest reading
            // of "the operator switched the check off" is that nobody looked,
            // which is what `Unbound` says. Pre-403 recorded no mark at all,
            // and with nothing established there is nothing to record.
            return FreshnessVerdict::Unbound;
        }

        match version {
            ControlSigVersion::V2 => {
                let verdict = offline_protocol_sealed::control_frame_freshness(
                    message.timestamp.as_millis(),
                    Utc::now().timestamp_millis(),
                    offline_protocol_sealed::CTRL_FRESHNESS_PAST_MS,
                    offline_protocol_sealed::CTRL_FRESHNESS_FUTURE_MS,
                );
                match verdict {
                    Freshness::Fresh => FreshnessVerdict::Bound,
                    Freshness::Stale { age_ms } => {
                        warn!(
                            sender = %sender,
                            message_id = %message.id,
                            age_ms,
                            "Dropping control message: stamped further in the past than the \
                             freshness window allows"
                        );
                        self.warn_control_gate_rejection(
                            sender,
                            SecurityWarningCode::StaleControlFrame,
                            "Control message refused as stale: its signed timestamp is older \
                             than this node accepts, which is what a replayed capture looks like",
                        );
                        FreshnessVerdict::Refused
                    }
                    Freshness::FromTheFuture { skew_ms } => {
                        warn!(
                            sender = %sender,
                            message_id = %message.id,
                            skew_ms,
                            "Dropping control message: stamped ahead of this device's clock by \
                             more than the freshness window allows"
                        );
                        self.warn_control_gate_rejection(
                            sender,
                            SecurityWarningCode::StaleControlFrame,
                            "Control message refused for clock skew: its signed timestamp is \
                             further ahead of this device's clock than is allowed. If this \
                             device's own clock is wrong, every peer looks like this",
                        );
                        FreshnessVerdict::Refused
                    }
                }
            }
            ControlSigVersion::V1 => {
                if !self.signs_freshness_bound_control(sender) {
                    // A peer that has never proved otherwise. Legacy, and
                    // accepted exactly as before — refusing here would refuse
                    // first contact with every install that has not upgraded.
                    return FreshnessVerdict::Unbound;
                }
                if message.content.starts_with(internal_prefixes::KEY_PACKAGE) {
                    debug!(
                        sender = %sender,
                        message_id = %message.id,
                        "Admitting a key package under the older payload from a peer held to the \
                         newer one; its session reset will be ignored"
                    );
                    // Clearing this is what makes the escape actually escape:
                    // the reciprocal send in `handle_key_package_message` is
                    // skipped for a peer we have already sent to, so without
                    // it the peer never receives the advertisement that would
                    // teach it to sign the newer payload again.
                    //
                    // The case it heals is capability-record *loss*, not a
                    // reinstall: an address is the hash of an identity key, so
                    // a peer that reinstalls arrives as a different peer
                    // entirely. What produces a peer at a known address whose
                    // record we no longer hold is eviction under
                    // `MAX_PENDING_KEY_PACKAGES` pressure, a restore that lost
                    // the capability category, or a storage failure on it.
                    //
                    // Replaying a captured v1 key package therefore costs us
                    // one key-package send. Bounded twice over: the receive
                    // deduplicator refuses an exact repeat for an hour, and
                    // `take_push_key_package` re-hands that peer's existing
                    // package rather than minting a fresh one, so nothing
                    // accumulates.
                    self.key_package_sent_to.remove(sender);
                    return FreshnessVerdict::Unbound;
                }
                warn!(
                    sender = %sender,
                    message_id = %message.id,
                    "Dropping control message: peer has proved it signs the freshness-bound \
                     payload and this frame does not carry one"
                );
                self.warn_control_gate_rejection(
                    sender,
                    SecurityWarningCode::StaleControlFrame,
                    "Control message refused as a downgrade: this peer has proved it signs the \
                     payload that states freshness, so one that does not is either a recording \
                     or a stripped signature",
                );
                FreshnessVerdict::Refused
            }
        }
    }

    /// Whether this frame is a relay answer that structurally cannot be signed.
    ///
    /// All three conditions are required, and the two beyond the prefix are what
    /// keep this from being a hole in the peer-to-peer path:
    ///
    /// - the prefix is one the relay server originates
    ///   ([`RELAY_ANSWER_PREFIXES`]);
    /// - it arrived on the Internet transport, where the relay ingest lives;
    /// - it carries no transport peer identity, which is what a locally
    ///   synthesized answer looks like — a real peer's frame on this transport
    ///   is attributed by the relay and would fail this.
    ///
    /// A peer sending one of these prefixes over the mesh, or over the relay
    /// with an attributed identity, is therefore still required to sign it.
    fn is_unsignable_relay_answer(
        message: &Message,
        arrival_transport: Option<TransportType>,
    ) -> bool {
        arrival_transport == Some(TransportType::Internet)
            && message.transport_peer_id().is_none()
            && RELAY_ANSWER_PREFIXES
                .iter()
                .any(|p| message.content.starts_with(p))
    }

    /// Validates that the claimed `message.sender` matches the transport-level
    /// peer identity, if available. Returns `true` if validated or if no
    /// transport identity is available (best-effort). Returns `false` if
    /// there is a mismatch on a frame claiming direct origin.
    ///
    /// # Hop-count rule
    ///
    /// The strict match applies only to frames with `hop_count == 0` — the
    /// frame's claim that the carrying peer *is* its origin. A frame with
    /// `hop_count > 0` was mesh-relayed: the transport identity names the
    /// nearest carrier (the relaying peer), not the origin in
    /// `message.sender`, so a mismatch is expected and carries no spoofing
    /// signal. Those frames fall back to the signature + derivation gate.
    ///
    /// A spoofer can forge `hop_count > 0` to skip this check, and that is now
    /// uninteresting: the forged-hop path lands on the same signature +
    /// derivation gate every other control frame passes, which refuses an
    /// unsigned frame outright and refuses a signed one whose key does not
    /// derive to the claimed sender. There is no configuration in which
    /// skipping this check buys an attacker anything, because this check is no
    /// longer what establishes identity — it only cross-checks an identity the
    /// signature has already proved.
    ///
    /// That is also why `require_transport_identity` keeps its `false` default
    /// rather than flipping with the rest of this work. Its remaining job is
    /// the `None` branch below, and demanding transport identity there costs
    /// real delivery for no authenticity: Nostr frames carry none by design
    /// (the Nostr pubkey is not the protocol id — see
    /// `nostr_message_received_inner`), and neither do relay frames that arrive
    /// without a named sender. Turning it on rejects every control message on
    /// those paths. Deleting the field belongs with the wider API sweep; until
    /// then it is a hardening knob for deployments that run neither transport.
    ///
    /// # Relay / mesh forwarding
    ///
    /// Forwarded messages re-created via [`send_internal_message`] →
    /// [`create_message`] have `transport_peer_id: None`, and because
    /// `transport_peer_id` is `#[serde(skip)]` it is also `None` after
    /// deserialization unless the receiving transport attaches an
    /// authenticated identity (Internet relay, Reticulum). Frames relayed
    /// verbatim by `try_relay_message` keep their original `sender` but
    /// always arrive with `hop_count >= 1` (the relayer increments before
    /// re-sending), so the hop-count rule above prevents this check from
    /// rejecting them.
    pub(super) fn validate_transport_sender(&self, message: &Message) -> bool {
        match message.transport_peer_id() {
            Some(transport_peer) => {
                if message.hop_count.value() > 0 {
                    debug!(
                        claimed_sender = %message.sender,
                        transport_peer = %transport_peer,
                        hop_count = message.hop_count.value(),
                        message_id = %message.id,
                        "Relayed control message: transport peer is the carrier, not the origin; skipping strict match"
                    );
                } else if message.sender.as_str() != transport_peer {
                    warn!(
                        claimed_sender = %message.sender,
                        transport_peer = %transport_peer,
                        message_id = %message.id,
                        "Sender identity mismatch: claimed sender does not match transport peer"
                    );
                    return false;
                }
            }
            None => {
                if self.config.security.require_transport_identity {
                    warn!(
                        claimed_sender = %message.sender,
                        message_id = %message.id,
                        "Rejecting control message without transport peer identity \
                         (require_transport_identity=true)"
                    );
                    return false;
                }
            }
        }
        true
    }

    /// Emits a `SecurityWarning` event for the given peer.
    pub(super) fn emit_security_warning(
        &self,
        peer_id: &str,
        reason_code: SecurityWarningCode,
        reason: impl Into<String>,
    ) {
        self.emit_event(Event::security_warning(
            peer_id.to_string(),
            reason_code,
            reason.into(),
        ));
    }

    /// The bit [`Self::warn_control_gate_rejection`] tracks for `code`.
    ///
    /// Zero for anything the control gate does not emit, which reads as "no bit
    /// to suppress on" and lets such a code through unthrottled — the throttle
    /// is deliberately scoped to the gate's own rejections rather than to
    /// security warnings in general.
    fn control_gate_warning_bit(code: SecurityWarningCode) -> u8 {
        match code {
            SecurityWarningCode::TransportIdentityMismatch => 1 << 0,
            SecurityWarningCode::ControlSignatureInvalid => 1 << 1,
            SecurityWarningCode::UnsignedControlRejected => 1 << 2,
            SecurityWarningCode::SenderAddressMismatch => 1 << 3,
            SecurityWarningCode::StaleControlFrame => 1 << 4,
            _ => 0,
        }
    }

    /// Emits a control-gate rejection warning for `peer_id`, at most once per
    /// peer per reason code (tracked in a bounded map that resets at
    /// [`MAX_CONTROL_GATE_WARNED_PEERS`], so a peer may re-warn after a
    /// forged-sender flood).
    ///
    /// Every one of these codes is reachable from an unauthenticated frame
    /// naming an attacker-chosen sender — a gate rejection is by definition a
    /// frame that proved nothing — so emitting per frame hands an off-path
    /// injector a way to bury the signal that matters
    /// ([`SecurityWarningCode::SenderAddressMismatch`], which per its own docs
    /// has no benign reading) under noise. The per-rejection `warn!` logs stay
    /// at the call sites; this throttles only the app-facing event.
    ///
    /// Suppression is per code, not per peer, so a peer that first trips one
    /// rejection still surfaces a later, different one. A single frame can
    /// legitimately report two codes — a derivation mismatch reports the
    /// specific `SenderAddressMismatch` and then the gate's general
    /// `ControlSignatureInvalid` — and both are bounded to one event each.
    pub(super) fn warn_control_gate_rejection(
        &mut self,
        peer_id: &str,
        reason_code: SecurityWarningCode,
        reason: impl Into<String>,
    ) {
        let bit = Self::control_gate_warning_bit(reason_code);
        if bit != 0 {
            if self
                .control_gate_warned
                .get(peer_id)
                .is_some_and(|mask| mask & bit != 0)
            {
                return;
            }
            // The keys are attacker-controlled, so the map resets at capacity
            // rather than growing without bound — same trade as
            // `warn_plaintext_receive_rejected`.
            if self.control_gate_warned.len() >= MAX_CONTROL_GATE_WARNED_PEERS
                && !self.control_gate_warned.contains_key(peer_id)
            {
                self.control_gate_warned.clear();
            }
            *self
                .control_gate_warned
                .entry(peer_id.to_string())
                .or_default() |= bit;
        }
        self.emit_security_warning(peer_id, reason_code, reason);
    }

    /// Emits a [`SecurityWarningCode::PlaintextSend`] warning for `peer_id`,
    /// at most once per peer per protocol instance. Called from the outbound
    /// plaintext fall-throughs, which are reachable only under the explicit
    /// `require_encryption = false` opt-out — the warning keeps cleartext
    /// flows visible without emitting per message.
    pub(super) fn warn_plaintext_send(&mut self, peer_id: &str) {
        if self.plaintext_send_warned.insert(peer_id.to_string()) {
            warn!(
                recipient = %peer_id,
                "Outbound message sent as plaintext (encryption disabled or MLS \
                 uninitialized, require_encryption=false)"
            );
            self.emit_security_warning(
                peer_id,
                SecurityWarningCode::PlaintextSend,
                "Outbound message sent as plaintext: encryption is disabled or MLS \
                 is not initialized, and require_encryption is false",
            );
        }
    }

    /// Emits a [`SecurityWarningCode::PushKeyPackagePoolExhausted`] warning,
    /// at most once per [`PUSH_KEY_PACKAGE_WARNING_SUPPRESS_INTERVAL`].
    ///
    /// The condition — the per-peer key-package pool at its ceiling, so this
    /// advertisement reuses a package another peer already holds — persists
    /// until packages are consumed or expire, and every push meanwhile would
    /// otherwise emit. Suppressed on time rather than per peer because the
    /// interesting fact is the pool's state, not which peer happened to ask.
    pub(super) fn warn_push_key_package_pool_exhausted(&mut self, peer_id: &str) {
        let now = Instant::now();
        if let Some(last) = self.last_push_key_package_warning {
            if now.duration_since(last) < PUSH_KEY_PACKAGE_WARNING_SUPPRESS_INTERVAL {
                return;
            }
        }
        self.last_push_key_package_warning = Some(now);
        warn!(
            peer_id = %peer_id,
            "Key-package pool at capacity; this peer shares an init key with another"
        );
        self.emit_security_warning(
            peer_id,
            SecurityWarningCode::PushKeyPackagePoolExhausted,
            "key-package pool at capacity: this peer was advertised an init key \
             another peer also holds, weakening forward secrecy at session \
             establishment until packages are consumed or expire",
        );
    }

    /// Emits a [`SecurityWarningCode::PlaintextReceiveRejected`] warning for
    /// `peer_id`, at most once per peer (tracked in a bounded set that resets
    /// at [`MAX_PLAINTEXT_RECEIVE_WARNED_PEERS`], so a peer may re-warn after
    /// a forged-sender flood). Called from the inbound plaintext policy gate
    /// ([`Self::accept_plaintext_content`]) when cleartext text or legacy
    /// media is dropped — the per-rejection `warn!` logs live at the call
    /// sites; this keeps the event stream from flooding when a legacy or
    /// malicious peer keeps sending.
    pub(super) fn warn_plaintext_receive_rejected(&mut self, peer_id: &str, detail: &str) {
        if self.plaintext_receive_warned.contains(peer_id) {
            return;
        }
        // The keys are attacker-controlled (wire-claimed sender ids), so the
        // set resets at capacity instead of growing without bound: a forged-
        // sender flood degrades the throttle to once-per-peer-per-generation
        // while memory stays capped.
        if self.plaintext_receive_warned.len() >= MAX_PLAINTEXT_RECEIVE_WARNED_PEERS {
            self.plaintext_receive_warned.clear();
        }
        self.plaintext_receive_warned.insert(peer_id.to_string());
        self.emit_security_warning(
            peer_id,
            SecurityWarningCode::PlaintextReceiveRejected,
            detail,
        );
    }

    /// Verifies the relay's `AddressDeclared` acknowledgement against the
    /// address this node actually holds.
    ///
    /// This is the lockstep assertion for the relay leg of addressing. The
    /// bridge declares [`Self::local_address`] and signs a proof over a
    /// per-connection challenge; the relay verifies that the declared address
    /// derives from the signing key and echoes back what it bound. Agreement
    /// means every frame the relay forwards from here on is attributed to the
    /// same identity the core stamps into `Message.sender`, which is what
    /// `validate_transport_sender` strict-matches on the receiving side.
    ///
    /// A disagreement is reported, never acted on: see
    /// [`SecurityWarningCode::RelayAddressBindingMismatch`] for why tearing the
    /// connection down would buy nothing. Callers pass the echoed address
    /// verbatim — an empty or malformed one simply cannot match and is reported
    /// like any other mismatch.
    ///
    /// Reached only through the FFI's dedicated entry point, not through
    /// message-plane injection, so a notification payload cannot synthesize it.
    pub fn on_relay_address_declared(&self, declared: &str) {
        match self.classify_address_binding(declared) {
            AddressBinding::Ours => {
                info!(
                    address = %declared,
                    "Relay bound this connection to our address; frames are attributed by address"
                );
            }
            AddressBinding::NotOurs { local } => {
                warn!(
                    declared = %declared,
                    local = %local,
                    "Relay acknowledged an address that is not ours"
                );
                // `declared` is this event's `peer_id`, where the scrubber
                // hashes it; interpolating it into `reason` — which is shipped
                // verbatim — would undo that hashing inside the same record.
                // The raw pair is in the `warn!` above.
                self.emit_security_warning(
                    declared,
                    SecurityWarningCode::RelayAddressBindingMismatch,
                    "relay bound this connection to an address that is not this device's: \
                     its frames are attributed to an identity we cannot prove, so receivers \
                     will reject our security-gated control traffic",
                );
            }
            AddressBinding::NoIdentity => {
                // The bridge does not declare before an identity exists, so an
                // acknowledgement here answers a declaration this node never
                // made. Same disposition, distinct text: nothing local can be
                // compared, which is itself the finding.
                warn!(
                    declared = %declared,
                    "Relay acknowledged an address declaration this node never made"
                );
                self.emit_security_warning(
                    declared,
                    SecurityWarningCode::RelayAddressBindingMismatch,
                    "relay bound this connection to an address while this device has no \
                     established identity to declare: no declaration was sent from here",
                );
            }
        }
    }

    /// Records the relay refusing this connection's address declaration.
    ///
    /// Non-fatal by contract on both sides — the connection stays authenticated
    /// and keeps working in account-name space. What degrades is *new* MLS
    /// session establishment over the relay; see
    /// [`SecurityWarningCode::RelayAddressDeclarationRefused`]. `reason` is the
    /// relay's own text: opaque, remote-chosen, and logged rather than
    /// emitted — an event field never carries text a remote party wrote.
    ///
    /// Attributed to this node rather than to a peer (the failure is ours, and
    /// no peer is involved), following the same convention as the Nostr
    /// slot-exhaustion warning.
    pub fn on_relay_address_declaration_refused(&self, reason: &str) {
        warn!(
            reason = %reason,
            "Relay refused our address declaration; staying in account-name space"
        );
        // The relay's own wording does not travel onto the event: it is
        // remote-chosen text of arbitrary length and content, and this event's
        // `reason` is shipped verbatim to telemetry sinks. The code *is* the
        // classification; the wording stays in the `warn!` above.
        self.emit_security_warning(
            &self.local_id,
            SecurityWarningCode::RelayAddressDeclarationRefused,
            "relay refused this connection's address declaration: frames stay \
             attributed by account name, so new encrypted sessions cannot be \
             established over the relay until a later connection declares successfully",
        );
    }

    /// Builds the signed proof this device presents to attach to a gateway.
    ///
    /// The bridge does not build these bytes. It holds the socket, so it holds
    /// the challenge, but the payload commits this device's own address under
    /// a domain that must not be confusable with the relay's, and the private
    /// key that signs it never leaves the core. Putting the construction here
    /// means there is one implementation, pinned by conformance vectors that
    /// CI runs, instead of one per bridge pinned by whatever that platform's
    /// test target happens to compile.
    ///
    /// Returns `(address, public_key, signature)`. The caller base64-encodes
    /// the two byte fields into `DeclareAddress`.
    ///
    /// # Errors
    ///
    /// - The challenge is not 32 bytes (`GATEWAY_CHALLENGE_LEN`). A gateway that
    ///   mints a shorter one has weakened the replay bound it exists to
    ///   provide, and signing whatever it sent would be the wrong way to find
    ///   that out. The check is here rather than in the bridge because a
    ///   signing routine that will put its key over any bytes a remote party
    ///   chose is the shape of a signing oracle.
    /// - This device has no identity yet, so there is nothing to declare
    ///   ([`Error::MlsNotInitialized`], which the FFI maps to the code the
    ///   bridges are documented to expect).
    pub fn gateway_address_declaration(
        &self,
        challenge: &[u8],
    ) -> Result<(String, Vec<u8>, Vec<u8>)> {
        if challenge.len() != GATEWAY_CHALLENGE_LEN {
            return Err(Error::InvalidArgument(format!(
                "gateway challenge must be {GATEWAY_CHALLENGE_LEN} bytes, got {}",
                challenge.len()
            )));
        }
        // One variant for both: the address exists once the MLS identity
        // does, so "no identity yet" and "no MLS" are one condition to a
        // caller, and it is the code the FFI documents for it.
        let address = self
            .local_address()
            .ok_or(Error::MlsNotInitialized)?
            .to_string();
        let mls = self.mls_manager.as_ref().ok_or(Error::MlsNotInitialized)?;
        let manager = mls
            .read()
            .map_err(|e| Error::Other(format!("MLS lock poisoned: {e}")))?;
        let public_key = manager
            .get_identity_public_key()
            .map_err(|e| Error::Other(format!("failed to get identity public key: {e}")))?;
        let payload = gateway_address_proof_payload(&address, challenge)
            .map_err(|e| Error::Other(format!("failed to build the gateway proof: {e}")))?;
        let signature = manager
            .sign_data(&payload)
            .map_err(|e| Error::Other(format!("failed to sign the gateway proof: {e}")))?;
        debug!(address = %address, "Built a gateway address declaration");
        Ok((address, public_key, signature))
    }

    /// What a gateway's `AddressDeclared` echo says about this device.
    ///
    /// Every gateway kind runs this same comparison, so it exists once: the
    /// texts a caller reports differ per carrier, but the question does not,
    /// and two copies of a security check are two chances for one of them to
    /// be relaxed on its own.
    fn classify_address_binding(&self, declared: &str) -> AddressBinding<'_> {
        match self.local_address() {
            Some(local) if local == declared => AddressBinding::Ours,
            Some(local) => AddressBinding::NotOurs { local },
            None => AddressBinding::NoIdentity,
        }
    }

    /// Checks a gateway's `AddressDeclared` answer against this device's own
    /// address.
    ///
    /// The gateway twin of [`Self::on_relay_address_declared`], and the same
    /// disposition: report, do not act. A gateway verifies that the declared
    /// address derives from the key that signed the proof before it echoes
    /// anything, so an echo naming another address means it bound what it did
    /// not verify, and a session attached under an address this device does
    /// not control draws that address's inbound traffic here and poisons the
    /// presence answers given about it. Tearing the connection down from here
    /// buys nothing: a gateway that owns the socket owns everything a local
    /// teardown would protect.
    ///
    /// What *is* acted on lives in the bridge and is narrower: it refuses to
    /// report the carrier available at all unless the session was bound, so a
    /// gateway that will not bind is a transport the selector never sees
    /// rather than one it trusts.
    ///
    /// Reached only through the FFI's dedicated entry point, not through
    /// message-plane injection, so a notification payload cannot synthesize
    /// it.
    pub fn on_gateway_address_declared(&self, declared: &str) {
        match self.classify_address_binding(declared) {
            AddressBinding::Ours => {
                info!(
                    address = %declared,
                    "Gateway bound this connection to our address"
                );
            }
            AddressBinding::NotOurs { local } => {
                warn!(
                    declared = %declared,
                    local = %local,
                    "Gateway acknowledged an address that is not ours"
                );
                // `declared` is this event's `peer_id`, where the scrubber
                // hashes it; interpolating it into `reason` — which is
                // shipped verbatim — would undo that hashing inside the same
                // record. The raw pair is in the `warn!` above.
                self.emit_security_warning(
                    declared,
                    SecurityWarningCode::GatewayAddressBindingMismatch,
                    "gateway bound this connection to an address that is not this device's: \
                     it will attribute our frames to an identity we cannot prove, and answer \
                     presence about an address we do not hold",
                );
            }
            AddressBinding::NoIdentity => {
                warn!(
                    declared = %declared,
                    "Gateway acknowledged an address declaration this node never made"
                );
                self.emit_security_warning(
                    declared,
                    SecurityWarningCode::GatewayAddressBindingMismatch,
                    "gateway bound this connection to an address while this device has no \
                     established identity to declare: no declaration was sent from here",
                );
            }
        }
    }

    /// Records a gateway refusing this connection's address declaration.
    ///
    /// Unlike the relay case, this is **not** a degraded-but-working
    /// connection. The relay keeps delivering on established sessions in
    /// account-name space; a gateway has no such space, so an unproved
    /// session may submit and be told a verdict and is never registered as a
    /// recipient. The bridge therefore does not report the carrier available,
    /// and this warning is what explains a transport that stays down while
    /// its socket connects perfectly well.
    ///
    /// `reason` is the gateway's own text: opaque, remote-chosen, and logged
    /// rather than emitted — an event field never carries text a remote party
    /// wrote. Attributed to this node rather than to a peer, since the
    /// failure is ours and no peer is involved.
    ///
    /// Emitted at most once per
    /// `GATEWAY_REFUSAL_WARNING_SUPPRESS_INTERVAL` (five minutes): the bridge closes a
    /// refused connection and reconnects on its ladder, so a gateway that
    /// keeps refusing would otherwise produce a security event per rung,
    /// forever. The `warn!` is not suppressed.
    pub fn on_gateway_address_declaration_refused(&mut self, reason: &str) {
        warn!(
            reason = %reason.chars().take(256).collect::<String>(),
            "Gateway refused our address declaration; the carrier stays unavailable"
        );
        let now = Instant::now();
        if let Some(last) = self.last_gateway_refusal_warning {
            if now.duration_since(last) < GATEWAY_REFUSAL_WARNING_SUPPRESS_INTERVAL {
                return;
            }
        }
        self.last_gateway_refusal_warning = Some(now);
        self.emit_security_warning(
            &self.local_id,
            SecurityWarningCode::GatewayAddressDeclarationRefused,
            "gateway refused this connection's address declaration: the session can be \
             told verdicts but is never a recipient, so the carrier is not offered for \
             sending until a later connection declares successfully",
        );
    }

    /// Returns `true` if the message content starts with any internal prefix.
    /// Used for injection prevention on the public send APIs.
    pub(crate) fn is_internal_prefix(content: &str) -> bool {
        INTERNAL_PREFIXES.iter().any(|p| content.starts_with(p))
    }

    /// Returns `true` if the message content starts with a control-plane
    /// prefix that requires security gate enforcement (transport identity +
    /// signature verification + sender derivation). Data-plane prefixes listed in
    /// `DATA_PLANE_PREFIXES` (e.g. `__MLS_ENC__`) are excluded because MLS
    /// provides its own authentication layer.
    pub(crate) fn is_security_gated_prefix(content: &str) -> bool {
        // A prefix is security-gated if it is an internal prefix AND not a
        // data-plane prefix. This derives the gated set from INTERNAL_PREFIXES
        // minus DATA_PLANE_PREFIXES, so any new internal prefix is automatically
        // gated unless explicitly excluded.
        Self::is_internal_prefix(content)
            && !DATA_PLANE_PREFIXES.iter().any(|p| content.starts_with(p))
    }

    /// Whether `peer_id` has ever presented a control-frame signature over the
    /// freshness-bound payload.
    ///
    /// In memory rather than a storage read, exactly like
    /// [`Self::is_encryption_capable`]: it is consulted on the control path of
    /// every frame, and the durable record exists to survive a restart rather
    /// than to be read per frame.
    pub(crate) fn signs_freshness_bound_control(&self, peer_id: &str) -> bool {
        self.control_freshness_peers.contains(peer_id)
    }

    /// Records that `peer_id` has proved it signs the freshness-bound payload.
    ///
    /// # Why this set needs no cap of its own
    ///
    /// It is a strict subset of the capped encryption-capable set, and the
    /// order of the two checks below is what makes that true rather than
    /// merely likely: a peer the capped set refused is not admitted here
    /// either. Inserting first and checking after would let a forged-sender
    /// flood grow this one without bound, and it is reachable by exactly the
    /// same flood, since minting an identity that signs honestly as itself
    /// costs one Ed25519 keygen.
    ///
    /// # Why refusal is safe here, unlike for encryption capability
    ///
    /// Failing to record leaves this peer able to send us the older payload,
    /// which is where every peer started. It loses an improvement rather than
    /// a protection, so the flood costs later peers the ratchet and never
    /// costs an earlier peer anything they had.
    pub(crate) fn record_control_freshness_proved(&mut self, peer_id: &str) {
        if !self.is_encryption_capable(peer_id) {
            return;
        }
        if !self.control_freshness_peers.insert(peer_id.to_string()) {
            return;
        }
        // Reached once per peer per process, on the transition. The durable
        // record is rewritten here rather than through
        // `record_encryption_capable`, whose elision cache deliberately skips
        // a peer whose record already exists — which is every peer that just
        // upgraded.
        self.persist_control_freshness_proved(peer_id);
    }

    /// Rewrites `peer_id`'s durable capability record with the ratchet set.
    ///
    /// Best-effort, like the record it extends: a failed write costs the
    /// ratchet across a restart, and it is re-proved by that peer's next
    /// freshness-bound frame. That is the fail-open direction, and it is the
    /// right one here — the alternative, refusing the peer's traffic because
    /// we could not write a note about them, breaks a working pair over a
    /// storage error.
    fn persist_control_freshness_proved(&self, peer_id: &str) {
        let Some(storage) = &self.secure_storage else {
            return;
        };
        let entry = EncryptionCapableEntry {
            last_seen_ms: Utc::now().timestamp_millis(),
            ctrl_freshness_proved: true,
            last_reset_ms: self
                .control_reset_watermark
                .get(peer_id)
                .copied()
                .unwrap_or(0),
        };
        match serde_json::to_vec(&entry) {
            Ok(data) => {
                if let Err(e) =
                    storage.store(storage_keys::ENCRYPTION_CAPABLE_PEERS, peer_id, &data)
                {
                    warn!(peer_id = %peer_id, error = %e, "Failed to persist control-freshness ratchet");
                }
            }
            Err(e) => {
                warn!(peer_id = %peer_id, error = %e, "Failed to serialize control-freshness ratchet");
            }
        }
    }

    /// Whether a session reset stamped `signed_ms` from `peer_id` is one this
    /// node has not already acted on.
    ///
    /// Strictly newer, not newer-or-equal: a frame is identified by its stamp
    /// here, so admitting an equal one admits the frame itself a second time.
    /// Two legitimate resets in the same millisecond would need a rekey ladder
    /// far faster than `REKEY_INTERVAL_SECS`, and if one ever happened the
    /// pair still converges — a refused reset shows up as the next decrypt
    /// failure, which drives a fresh one.
    pub(crate) fn reset_is_unspent(&self, peer_id: &str, signed_ms: i64) -> bool {
        self.control_reset_watermark
            .get(peer_id)
            .is_none_or(|seen| signed_ms > *seen)
    }

    /// Records a session reset as spent, in memory and durably.
    ///
    /// **Call this before acting on the reset, never after.** The teardown it
    /// authorizes is followed by a fresh session, so a crash between the two
    /// leaves a frame that has already destroyed one session still able to
    /// destroy its replacement. Recording first costs, at worst, a reset that
    /// was recorded and not carried out, which the pair recovers from the same
    /// way it recovers from a lost frame: the next decrypt failure drives
    /// another.
    pub(crate) fn record_reset_spent(&mut self, peer_id: &str, signed_ms: i64) {
        // The same subset rule the ratchet uses, checked before the insert:
        // this map inherits the encryption-capable cap rather than needing one
        // of its own.
        if !self.is_encryption_capable(peer_id) {
            return;
        }
        self.control_reset_watermark
            .insert(peer_id.to_string(), signed_ms);

        let Some(storage) = &self.secure_storage else {
            return;
        };
        let entry = EncryptionCapableEntry {
            last_seen_ms: Utc::now().timestamp_millis(),
            ctrl_freshness_proved: self.signs_freshness_bound_control(peer_id),
            last_reset_ms: signed_ms,
        };
        match serde_json::to_vec(&entry) {
            Ok(data) => {
                if let Err(e) =
                    storage.store(storage_keys::ENCRYPTION_CAPABLE_PEERS, peer_id, &data)
                {
                    warn!(peer_id = %peer_id, error = %e, "Failed to persist the spent-reset mark");
                }
            }
            Err(e) => {
                warn!(peer_id = %peer_id, error = %e, "Failed to serialize the spent-reset mark");
            }
        }
    }

    /// Persists the durable "this peer runs MLS" record.
    ///
    /// Best-effort and idempotent: the in-memory set is already updated by the
    /// time this runs, so a storage failure costs the knowledge only across a
    /// restart — and costs it in the safe direction, since the peer re-proves
    /// capability on their next signed control message.
    ///
    /// Returns whether the record is now durable, which is what lets
    /// [`Self::record_encryption_capable`] skip the repeat write without
    /// stranding a peer whose write failed. A node with no secure storage
    /// answers `false` forever — correctly: nothing was persisted, and there is
    /// no cost to re-answering that on each frame.
    pub(super) fn persist_encryption_capable(&self, peer_id: &str) -> bool {
        let Some(storage) = &self.secure_storage else {
            return false;
        };
        let entry = EncryptionCapableEntry {
            last_seen_ms: Utc::now().timestamp_millis(),
            // Carried forward rather than defaulted. This path writes the
            // whole record, so spelling `false` here would clear a ratchet
            // that is set — and a ratchet any ordinary frame can clear is not
            // one. In practice the elision cache means this rarely rewrites an
            // existing record, which is exactly the kind of "cannot happen"
            // that stops being true after a refactor.
            ctrl_freshness_proved: self.signs_freshness_bound_control(peer_id),
            last_reset_ms: self
                .control_reset_watermark
                .get(peer_id)
                .copied()
                .unwrap_or(0),
        };
        match serde_json::to_vec(&entry) {
            Ok(data) => {
                match storage.store(storage_keys::ENCRYPTION_CAPABLE_PEERS, peer_id, &data) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!(peer_id = %peer_id, error = %e, "Failed to persist encryption-capability record");
                        false
                    }
                }
            }
            Err(e) => {
                warn!(peer_id = %peer_id, error = %e, "Failed to serialize encryption-capability record");
                false
            }
        }
    }

    /// Restores the durable encryption-capability records from storage.
    ///
    /// Skips corrupted entries with a warning (best-effort restore), and is
    /// bounded by [`MAX_RESTORE_KEYS_PER_CATEGORY`] like every other category
    /// walk on the boot path, so a store listing more entries than any
    /// legitimate run can produce cannot turn initialization into an unbounded
    /// number of loads.
    ///
    /// Unlike the two cache restores (`restore_peer_key_packages`,
    /// `restore_peer_capabilities`), the overflow is **not** pruned from
    /// durable storage, and there is no in-memory cap applied here. A cached
    /// key package only costs a re-exchange when it is dropped; one of these
    /// records is the sole durable evidence that a peer runs MLS, and dropping
    /// it silently re-opens the plaintext gate for that peer. Stranding an
    /// over-cap entry is the strictly safer failure, so this walk stops and
    /// says so rather than shrinking the store.
    ///
    /// `mark_encryption_capable` applies its own bound
    /// ([`MAX_ENCRYPTION_CAPABLE_PEERS`](super::MAX_ENCRYPTION_CAPABLE_PEERS)),
    /// by refusal rather than eviction, and restore runs before `start()`
    /// admits traffic — so peers with durable records are first in line for
    /// the capacity a forged-sender flood would otherwise consume.
    ///
    /// That ordering is only worth anything because the category itself is
    /// bounded by the same cap: [`Self::record_encryption_capable`] writes a
    /// record only for a peer the capped set accepted. Were it not, a flood of
    /// self-consistent identities could fill the category past
    /// [`MAX_RESTORE_KEYS_PER_CATEGORY`] and push a legitimate peer's record
    /// outside the window this walk reads — losing exactly the protection the
    /// category exists to carry.
    pub(super) fn restore_encryption_capable_peers(&mut self) {
        let Some(storage) = &self.secure_storage else {
            return;
        };
        let peer_ids = match storage.list_keys(storage_keys::ENCRYPTION_CAPABLE_PEERS) {
            Ok(keys) => keys,
            Err(e) => {
                warn!(error = %e, "Failed to list encryption-capability records, starting empty");
                return;
            }
        };
        let listed = peer_ids.len();
        // Collected before marking: `mark_encryption_capable` takes `&mut self`
        // while `storage` is borrowed from `self`.
        let mut capable: Vec<(String, bool, i64)> = Vec::new();
        for peer_id in peer_ids.iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            // Storage keys bypass `UserId::new()`, so a corrupted or
            // pre-validation-era entry could contain hostile characters.
            if UserId::new(peer_id).is_err() {
                warn!(peer_id = %peer_id, "Skipping capability record with invalid peer ID");
                continue;
            }
            match storage.load(storage_keys::ENCRYPTION_CAPABLE_PEERS, peer_id) {
                Ok(Some(data)) => match serde_json::from_slice::<EncryptionCapableEntry>(&data) {
                    // The record's presence *is* the encryption-capability
                    // fact; its timestamp is diagnostic. The ratchet flag is
                    // read, because that one is a field rather than a
                    // presence. Deserializing anyway keeps a garbage record
                    // from being read as evidence.
                    Ok(entry) => capable.push((
                        peer_id.clone(),
                        entry.ctrl_freshness_proved,
                        entry.last_reset_ms,
                    )),
                    Err(e) => {
                        warn!(peer_id = %peer_id, error = %e, "Skipping corrupted capability record");
                    }
                },
                Ok(None) => {}
                Err(e) => {
                    warn!(peer_id = %peer_id, error = %e, "Failed to load capability record");
                }
            }
        }
        let restored = capable.len() as u32;
        for (peer_id, ctrl_freshness_proved, last_reset_ms) in capable {
            // Read back from the category, so the record demonstrably exists:
            // seeding the elision cache here is what keeps the first verified
            // frame from a restored peer from rewriting a record that is
            // already on disk.
            self.encryption_capable_persisted.insert(peer_id.clone());
            let marked = self.mark_encryption_capable(&peer_id);
            if marked {
                // Restored through the same subset rule the live path applies,
                // so a category that somehow holds more peers than the cap
                // cannot seed a set that the cap is supposed to bound.
                if ctrl_freshness_proved {
                    self.control_freshness_peers.insert(peer_id.clone());
                }
                if last_reset_ms != 0 {
                    // Losing this across a restart would make every reset this
                    // node already acted on spendable again, which is the
                    // replay it exists to deny.
                    self.control_reset_watermark.insert(peer_id, last_reset_ms);
                }
            }
        }
        if listed > MAX_RESTORE_KEYS_PER_CATEGORY {
            warn!(
                listed,
                cap = MAX_RESTORE_KEYS_PER_CATEGORY,
                "Capability store listed more peers than any legitimate run can produce; ignoring \
                 the tail rather than pruning it, since a dropped record re-opens the plaintext gate"
            );
        }
        if restored > 0 {
            info!(
                count = restored,
                "Restored encryption-capability records from storage"
            );
        }
    }
}
