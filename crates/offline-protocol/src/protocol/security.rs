//! Control message signing, verification, and peer identity derivation.

use super::storage::MAX_RESTORE_KEYS_PER_CATEGORY;
use super::{
    base64_decode, base64_encode, storage_keys, ControlGateOutcome, EncryptionCapableEntry,
    InternalMessageResult, OfflineProtocol, CTRL_PK_META_KEY, CTRL_SIGN_DOMAIN, CTRL_SIG_META_KEY,
    DATA_PLANE_PREFIXES, INTERNAL_PREFIXES, MAX_CONTROL_GATE_WARNED_PEERS,
    MAX_PLAINTEXT_RECEIVE_WARNED_PEERS, RELAY_ANSWER_PREFIXES,
};
use crate::events::{Event, SecurityWarningCode};
use crate::{Error, Result};
use chrono::Utc;
use offline_protocol_core::{Address, Message, UserId};
use offline_protocol_transport::TransportType;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Minimum gap between [`SecurityWarningCode::PushKeyPackagePoolExhausted`]
/// emissions. Matches the suppression the Nostr slot-exhaustion and
/// unauthorized-membership reports use, for the same reason: the condition
/// persists, so an unsuppressed warning reports one cause many times.
const PUSH_KEY_PACKAGE_WARNING_SUPPRESS_INTERVAL: Duration = Duration::from_secs(300);

impl OfflineProtocol {
    /// Builds a canonical signing payload using length-prefixed encoding.
    ///
    /// Each field is encoded as `<4-byte big-endian length><utf-8 bytes>`,
    /// making the encoding unambiguous regardless of field content (no
    /// delimiter-collision risk).
    pub(crate) fn build_canonical_payload(message: &Message) -> Result<Vec<u8>> {
        let fields: [&str; 4] = [
            message.sender.as_str(),
            &message.id.as_str(),
            message.recipient.as_str(),
            &message.content,
        ];
        let mut buf = Vec::with_capacity(
            CTRL_SIGN_DOMAIN.len() + fields.iter().map(|f| 4 + f.len()).sum::<usize>(),
        );
        buf.extend_from_slice(CTRL_SIGN_DOMAIN);
        for field in &fields {
            let len: u32 = field.len().try_into().map_err(|_| {
                Error::Other(format!(
                    "Field too large for canonical payload length prefix: {} bytes",
                    field.len()
                ))
            })?;
            buf.extend_from_slice(&len.to_be_bytes());
            buf.extend_from_slice(field.as_bytes());
        }
        Ok(buf)
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

        Self::sign_control_message_with(message, &manager)
    }

    /// Stamps `message` with an Ed25519 signature and public key from
    /// `manager`'s identity.
    ///
    /// Split out from [`Self::sign_control_message`] so a caller that already
    /// holds the signing identity — a test standing in for a peer's device —
    /// can produce the same bytes the peer's own instance would, rather than
    /// reimplementing the canonical payload and getting it subtly wrong.
    pub(crate) fn sign_control_message_with(
        message: &mut Message,
        manager: &offline_protocol_mls::MlsManager,
    ) -> Result<()> {
        let public_key = match manager.get_identity_public_key() {
            Ok(pk) => pk,
            Err(e) => {
                let reason = format!("Failed to get identity public key: {}", e);
                error!(error = %e, "Failed to get identity public key — cannot sign control message");
                return Err(Error::Other(reason));
            }
        };
        let canonical = Self::build_canonical_payload(message)?;
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
    /// - `Ok(true)`  — valid signature, and the signing key derives to `sender`
    /// - `Ok(false)` — no signature metadata at all (unsigned message)
    /// - `Err(..)` — signature invalid, sender/key derivation mismatch, or
    ///   malformed metadata (e.g. public key present without a signature,
    ///   or vice versa)
    pub(super) fn verify_control_message(&mut self, message: &Message) -> Result<bool> {
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
                return Ok(false); // Truly unsigned — caller decides policy
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

        // Verify Ed25519 signature over a length-prefixed canonical payload
        // that binds sender, message ID, recipient, and content.
        let canonical = Self::build_canonical_payload(message)?;
        let valid =
            offline_protocol_mls::MlsManager::verify_signature(&public_key, &canonical, &signature)
                .map_err(|e| Error::Other(format!("Signature verification error: {}", e)))?;

        if !valid {
            return Err(Error::Other(
                "Control message signature verification failed".to_string(),
            ));
        }

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
        Ok(true)
    }

    /// Requires `public_key` to derive to the address in `sender`.
    ///
    /// This one function is what the TOFU pin store used to be. The store held
    /// a key per peer so a later frame could be compared against the first one
    /// seen; an address makes that comparison unnecessary, because the address
    /// already *is* `bech32m(0x01 ‖ SHA-256(key)[..20])`. Impersonation stops
    /// costing "be the first to claim the name" and starts costing a 160-bit
    /// second preimage.
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
        let claimed = sender.parse::<Address>().map_err(|e| {
            Error::Other(format!(
                "Control message sender '{}' is not an address: {}",
                sender, e
            ))
        })?;

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
            return ControlGateOutcome::Proceed { signed: false };
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
        let signed = match self.verify_control_message(message) {
            Ok(true) => {
                // Signed and verified — proceed
                true
            }
            Ok(false) if Self::is_unsignable_relay_answer(message, arrival_transport) => {
                // A relay-originated answer, which no peer signed because no
                // peer sent it. See `RELAY_ANSWER_PREFIXES` for why this cannot
                // be signature-gated and what does and does not protect it.
                debug!(
                    sender = %sender,
                    message_id = %message.id,
                    "Accepting unsigned relay-originated control frame (no peer signs these)"
                );
                false
            }
            Ok(false) => {
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
                self.warn_control_gate_rejection(
                    sender,
                    SecurityWarningCode::ControlSignatureInvalid,
                    format!("Control message rejected: {}", err),
                );
                return ControlGateOutcome::Rejected(InternalMessageResult::SecurityRejected);
            }
        };

        ControlGateOutcome::Proceed { signed }
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
        let mut capable: Vec<String> = Vec::new();
        for peer_id in peer_ids.iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            // Storage keys bypass `UserId::new()`, so a corrupted or
            // pre-validation-era entry could contain hostile characters.
            if UserId::new(peer_id).is_err() {
                warn!(peer_id = %peer_id, "Skipping capability record with invalid peer ID");
                continue;
            }
            match storage.load(storage_keys::ENCRYPTION_CAPABLE_PEERS, peer_id) {
                Ok(Some(data)) => match serde_json::from_slice::<EncryptionCapableEntry>(&data) {
                    // The record's presence *is* the fact; its timestamp is
                    // diagnostic. Deserializing anyway keeps a garbage record
                    // from being read as evidence.
                    Ok(_) => capable.push(peer_id.clone()),
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
        for peer_id in capable {
            // Read back from the category, so the record demonstrably exists:
            // seeding the elision cache here is what keeps the first verified
            // frame from a restored peer from rewriting a record that is
            // already on disk.
            self.encryption_capable_persisted.insert(peer_id.clone());
            self.mark_encryption_capable(&peer_id);
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
