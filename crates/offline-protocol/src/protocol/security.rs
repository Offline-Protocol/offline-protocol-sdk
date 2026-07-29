//! Control message signing, verification, and TOFU key management.

use super::storage::MAX_RESTORE_KEYS_PER_CATEGORY;
use super::{
    base64_decode, base64_encode, storage_keys, InternalMessageResult, OfflineProtocol, TofuEntry,
    CTRL_PK_META_KEY, CTRL_SIGN_DOMAIN, CTRL_SIG_META_KEY, DATA_PLANE_PREFIXES, INTERNAL_PREFIXES,
    MAX_PLAINTEXT_RECEIVE_WARNED_PEERS, MAX_TOFU_PEERS, TOFU_MIN_EVICTION_AGE_MS,
};
use crate::events::{Event, SecurityWarningCode};
use crate::{Error, Result};
use chrono::Utc;
use offline_protocol_core::{Message, UserId};
use tracing::{debug, error, info, warn};

impl OfflineProtocol {
    /// Builds a canonical signing payload using length-prefixed encoding.
    ///
    /// Each field is encoded as `<4-byte big-endian length><utf-8 bytes>`,
    /// making the encoding unambiguous regardless of field content (no
    /// delimiter-collision risk).
    pub(super) fn build_canonical_payload(message: &Message) -> Result<Vec<u8>> {
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

    /// Verifies a control message's Ed25519 signature from metadata.
    ///
    /// Returns:
    /// - `Ok(true)`  — valid signature, TOFU key check passed
    /// - `Ok(false)` — no signature metadata at all (legacy/unsigned message)
    /// - `Err(..)` — signature invalid, key mismatch, TOFU violation, or
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

        // TOFU: check/pin the public key for this sender
        self.tofu_check_or_pin(message.sender.as_str(), public_key)
    }

    /// Checks a verified public key against the TOFU store, or pins it on
    /// first contact. Handles bounded-capacity eviction with a minimum age
    /// threshold to resist cache-filling attacks.
    ///
    /// Returns `Ok(true)` on success, `Err(..)` on TOFU key mismatch or
    /// invalid (empty) public key.
    pub(super) fn tofu_check_or_pin(&mut self, sender: &str, public_key: Vec<u8>) -> Result<bool> {
        if public_key.is_empty() {
            return Err(Error::Other(
                "Cannot TOFU-pin an empty public key".to_string(),
            ));
        }
        let now_ms = Utc::now().timestamp_millis();

        // Deferred persistence actions collected here to avoid borrow conflicts
        // between `get_mut` on the HashMap and `&self` in persistence helpers.
        enum TofuAction {
            Persist(String, TofuEntry),
            Delete(String),
        }
        let mut actions: Vec<TofuAction> = Vec::new();

        if let Some(entry) = self.known_peer_public_keys.get_mut(sender) {
            if entry.public_key != public_key {
                warn!(
                    sender = %sender,
                    "TOFU key mismatch: peer presented a different public key"
                );
                self.emit_security_warning(
                    sender,
                    SecurityWarningCode::TofuKeyMismatch,
                    "Public key changed for known peer (possible impersonation)",
                );
                return Err(Error::Other(format!(
                    "TOFU key mismatch for peer '{}'",
                    sender
                )));
            }
            // Update last-seen timestamp for LRU tracking
            entry.last_seen_ms = now_ms;
            actions.push(TofuAction::Persist(sender.to_string(), entry.clone()));
        } else {
            // First contact — pin the key (with bounded capacity, LRU eviction)
            if self.known_peer_public_keys.len() >= MAX_TOFU_PEERS {
                // Only evict entries older than the minimum age to prevent a
                // cache-filling attack where an adversary rapidly registers
                // many fake identities to force eviction of legitimate peers.
                let eviction_cutoff = now_ms - TOFU_MIN_EVICTION_AGE_MS;
                let evict_key = self
                    .known_peer_public_keys
                    .iter()
                    .filter(|(_, entry)| entry.last_seen_ms < eviction_cutoff)
                    .min_by_key(|(_, entry)| entry.last_seen_ms)
                    .map(|(k, _)| k.clone());

                match evict_key {
                    Some(key) => {
                        debug!(evicted_peer = %key, "TOFU store full, evicting LRU entry");
                        self.known_peer_public_keys.remove(&key);
                        actions.push(TofuAction::Delete(key));
                    }
                    None => {
                        warn!(
                            sender = %sender,
                            store_size = self.known_peer_public_keys.len(),
                            "TOFU store full and no entry old enough to evict — \
                             refusing to pin new peer (possible cache-filling attack)"
                        );
                        self.emit_security_warning(
                            sender,
                            SecurityWarningCode::TofuStoreFull,
                            "TOFU store full, cannot pin new peer key",
                        );
                        // Still accept the message (signature was valid) but
                        // don't pin — the peer will be re-verified each time.
                        return Ok(true);
                    }
                }
            }
            debug!(sender = %sender, "TOFU: pinning public key for new peer");
            let entry = TofuEntry {
                public_key,
                last_seen_ms: now_ms,
            };
            actions.push(TofuAction::Persist(sender.to_string(), entry.clone()));
            self.known_peer_public_keys
                .insert(sender.to_string(), entry);
        }

        // Execute deferred persistence actions (HashMap borrow is now released)
        for action in actions {
            match action {
                TofuAction::Persist(peer_id, entry) => self.persist_tofu_entry(&peer_id, &entry),
                TofuAction::Delete(peer_id) => self.delete_tofu_entry(&peer_id),
            }
        }

        Ok(true)
    }

    /// Security gate for control messages. Validates transport-level sender
    /// identity and cryptographic signature before allowing the message to
    /// proceed through `process_internal_message`.
    ///
    /// Returns `Some(InternalMessageResult::SecurityRejected)` to drop the
    /// message (without sending a delivery ACK), or `None` to allow it
    /// through.
    pub(super) fn security_gate_control_message(
        &mut self,
        message: &Message,
    ) -> Option<InternalMessageResult> {
        let content = &message.content;
        let sender = message.sender.as_str();

        if !Self::is_security_gated_prefix(content) {
            return None; // Not a security-gated control message — no gate needed
        }

        // Transport-level identity check
        if !self.validate_transport_sender(message) {
            warn!(
                sender = %sender,
                message_id = %message.id,
                "Dropping control message: sender/transport identity mismatch or missing"
            );
            self.emit_security_warning(
                sender,
                SecurityWarningCode::TransportIdentityMismatch,
                "Control message sender does not match transport peer identity",
            );
            return Some(InternalMessageResult::SecurityRejected);
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
        match self.verify_control_message(message) {
            Ok(true) => {
                // Signed and verified — proceed
            }
            Ok(false) => {
                // Unsigned (legacy) — but if the sender already has a TOFU-pinned
                // key, reject: a known-signed peer going unsigned is a suspicious
                // downgrade that could indicate an impersonation attempt.
                if self.known_peer_public_keys.contains_key(sender) {
                    warn!(
                        sender = %sender,
                        message_id = %message.id,
                        "Dropping unsigned control message from TOFU-pinned peer (signature downgrade)"
                    );
                    self.emit_security_warning(
                        sender,
                        SecurityWarningCode::SignatureDowngrade,
                        "Unsigned control message from peer with pinned key (possible downgrade attack)",
                    );
                    return Some(InternalMessageResult::SecurityRejected);
                }
                // Strict deployments reject unsigned control traffic outright.
                // The transport-identity strict match only covers frames
                // claiming direct origin (`hop_count == 0`); a forged-hop
                // frame skips it and lands here, so accepting unsigned frames
                // would let a spoofer impersonate any not-yet-pinned peer
                // without even committing a signing key. Requiring a
                // signature forces the attacker to present a key that TOFU
                // pins — and later flags when the real peer shows up.
                if self.config.security.require_transport_identity {
                    warn!(
                        sender = %sender,
                        message_id = %message.id,
                        "Dropping unsigned control message (require_transport_identity demands signed control traffic)"
                    );
                    self.emit_security_warning(
                        sender,
                        SecurityWarningCode::UnsignedControlRejected,
                        "Unsigned control message rejected by strict transport-identity policy",
                    );
                    return Some(InternalMessageResult::SecurityRejected);
                }
                debug!(
                    sender = %sender,
                    message_id = %message.id,
                    "Received unsigned control message (legacy peer)"
                );
            }
            Err(err) => {
                // Signature invalid, TOFU violation, or malformed metadata — drop
                warn!(
                    sender = %sender,
                    message_id = %message.id,
                    error = %err,
                    "Dropping control message: signature verification failed"
                );
                self.emit_security_warning(
                    sender,
                    SecurityWarningCode::ControlSignatureInvalid,
                    format!("Control message rejected: {}", err),
                );
                return Some(InternalMessageResult::SecurityRejected);
            }
        }

        None // Passed the security gate
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
    /// signal. Those frames fall back to the signature + TOFU gate.
    ///
    /// A spoofer can forge `hop_count > 0` to skip this check. Under the
    /// default configuration that lands it exactly on the no-identity
    /// best-effort path (signature + TOFU) — never weaker. Under
    /// `require_transport_identity = true` the no-identity path *rejects*,
    /// so the forged-hop path would be strictly weaker if the gate accepted
    /// unsigned frames there; to close that, the gate also rejects unsigned
    /// control frames outright when the flag is set
    /// ([`SecurityWarningCode::UnsignedControlRejected`]). The residual
    /// trust assumption in every configuration is first-contact TOFU
    /// pinning — a forged-hop spoofer must commit a signing key that TOFU
    /// pins and later flags, exactly as on the identity-less mesh
    /// transports.
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
    /// signature verification + TOFU). Data-plane prefixes listed in
    /// `DATA_PLANE_PREFIXES` (e.g. `__MLS_ENC__`) are excluded because MLS
    /// provides its own authentication layer.
    pub(super) fn is_security_gated_prefix(content: &str) -> bool {
        // A prefix is security-gated if it is an internal prefix AND not a
        // data-plane prefix. This derives the gated set from INTERNAL_PREFIXES
        // minus DATA_PLANE_PREFIXES, so any new internal prefix is automatically
        // gated unless explicitly excluded.
        Self::is_internal_prefix(content)
            && !DATA_PLANE_PREFIXES.iter().any(|p| content.starts_with(p))
    }

    /// Persists a single TOFU entry to storage.
    pub(super) fn persist_tofu_entry(&self, peer_id: &str, entry: &TofuEntry) {
        let Some(storage) = &self.secure_storage else {
            return;
        };
        match serde_json::to_vec(entry) {
            Ok(data) => {
                if let Err(e) = storage.store(storage_keys::TOFU_KEYS, peer_id, &data) {
                    warn!(peer_id = %peer_id, error = %e, "Failed to persist TOFU entry");
                }
            }
            Err(e) => {
                warn!(peer_id = %peer_id, error = %e, "Failed to serialize TOFU entry");
            }
        }
    }

    /// Deletes a TOFU entry from storage (e.g. on LRU eviction).
    pub(super) fn delete_tofu_entry(&self, peer_id: &str) {
        let Some(storage) = &self.secure_storage else {
            return;
        };
        if let Err(e) = storage.delete(storage_keys::TOFU_KEYS, peer_id) {
            warn!(peer_id = %peer_id, error = %e, "Failed to delete TOFU entry from storage");
        }
    }

    /// Resets the TOFU-pinned public key for a specific peer, and drops any
    /// existing MLS session with them.
    ///
    /// Use this when a peer has legitimately re-initialized their MLS identity
    /// (e.g., reinstalled the app, new device) — typically in response to a
    /// [`Event::SecurityWarning`] carrying [`SecurityWarningCode::TofuKeyMismatch`].
    /// Unpinning the key lets the peer re-pin with their new public key on next
    /// contact.
    ///
    /// The stale MLS session is dropped in the same call because it is bound to
    /// the peer's now-dead credential; without this, the next
    /// `establish_secure_session` would be a no-op against the old session and
    /// the new keys would never take effect. Session deletion is best-effort,
    /// not atomic: the key un-pin is committed first (so this still returns
    /// `true`), then the session is dropped — no existing session (or MLS not
    /// initialized) is a harmless no-op, while a genuine deletion failure is
    /// logged at `warn` and leaves the stale session in place.
    ///
    /// Returns `true` if a TOFU entry was removed, `false` if none existed.
    /// The call is idempotent — resetting a peer with no pinned key is a no-op.
    ///
    /// Emits a `TofuReset` event only when an entry was actually removed.
    pub fn reset_tofu_for_peer(&mut self, peer_id: &str) -> bool {
        if self.known_peer_public_keys.remove(peer_id).is_some() {
            self.delete_tofu_entry(peer_id);
            // The peer re-identified, so any existing session is bound to their
            // now-dead credential — drop it so re-establishment isn't a no-op
            // against a stale session. The drop is best-effort, not atomic: the
            // un-pin above is already committed. `delete_session` is idempotent
            // (no session returns `Ok`) and `MlsNotInitialized` means none can
            // exist, so both are benign. Any *other* error means the drop
            // genuinely failed and the stale session may have outlived the
            // un-pin (the next `establish_secure_session` would no-op against
            // it), so surface it with a `warn!` instead of swallowing it.
            match self.manual_mls_delete_session(peer_id) {
                Ok(()) => {
                    info!(peer_id = %peer_id, "TOFU key reset for peer (pinned key + stale MLS session cleared)");
                }
                Err(Error::MlsNotInitialized) => {
                    info!(peer_id = %peer_id, "TOFU key reset for peer (pinned key cleared; MLS not initialized, no session to drop)");
                }
                Err(e) => {
                    warn!(
                        peer_id = %peer_id,
                        error = %e,
                        "TOFU key reset un-pinned the key but could NOT drop the stale MLS session; \
                         re-establishment may no-op against it until it is cleared"
                    );
                }
            }
            self.emit_event(Event::TofuReset {
                peer_id: peer_id.to_string(),
            });
            true
        } else {
            false
        }
    }

    /// Restores TOFU key entries from persistent storage.
    ///
    /// Skips corrupted entries with a warning (best-effort restore).
    /// Caps the restored set at `MAX_TOFU_PEERS` to honour the in-memory
    /// capacity limit even if storage contains more entries (e.g. after
    /// the limit was lowered in a new version).
    ///
    /// Bounded by [`MAX_RESTORE_KEYS_PER_CATEGORY`] like every other category
    /// walk on the restore path, so a store listing more entries than any
    /// legitimate run can produce cannot turn the boot path into an unbounded
    /// number of loads and allocations. The bound sits far above
    /// `MAX_TOFU_PEERS`, so no store this SDK wrote is ever affected by it.
    ///
    /// Unlike the two cache restores (`restore_peer_key_packages`,
    /// `restore_peer_capabilities`), the overflow is **not** pruned from
    /// durable storage. A cached key package only costs a re-exchange when it
    /// is dropped; a TOFU entry is a *pin*, and deleting one silently re-arms
    /// trust-on-first-use for that peer — the next key it offers is accepted
    /// without a mismatch warning. Stranding an over-cap entry is the strictly
    /// safer failure, so this walk stops and says so rather than shrinking the
    /// store.
    pub(super) fn restore_tofu_keys(&mut self) {
        let Some(storage) = &self.secure_storage else {
            return;
        };
        let peer_ids = match storage.list_keys(storage_keys::TOFU_KEYS) {
            Ok(keys) => keys,
            Err(e) => {
                warn!(error = %e, "Failed to list TOFU keys from storage, starting with empty store");
                return;
            }
        };
        let listed = peer_ids.len();
        // Load all valid entries first so we can sort by last_seen_ms and
        // keep the most recently seen peers when truncating.
        let mut valid_entries: Vec<(String, TofuEntry)> = Vec::new();
        for peer_id in peer_ids.iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            // Validate the peer_id: storage keys bypass UserId::new() so a
            // corrupted or pre-validation-era entry could contain hostile chars.
            if UserId::new(peer_id).is_err() {
                warn!(peer_id = %peer_id, "Skipping TOFU entry with invalid peer ID");
                continue;
            }
            match storage.load(storage_keys::TOFU_KEYS, peer_id) {
                Ok(Some(data)) => match serde_json::from_slice::<TofuEntry>(&data) {
                    Ok(entry) => {
                        valid_entries.push((peer_id.clone(), entry));
                    }
                    Err(e) => {
                        warn!(peer_id = %peer_id, error = %e, "Skipping corrupted TOFU entry");
                    }
                },
                Ok(None) => {}
                Err(e) => {
                    warn!(peer_id = %peer_id, error = %e, "Failed to load TOFU entry");
                }
            }
        }
        if valid_entries.len() > MAX_TOFU_PEERS {
            warn!(
                stored = valid_entries.len(),
                limit = MAX_TOFU_PEERS,
                "TOFU storage contains more entries than current limit, keeping most recent"
            );
            // Sort by last_seen_ms descending, then by peer_id ascending for
            // deterministic ordering when timestamps are equal.
            valid_entries.sort_by(|a, b| {
                b.1.last_seen_ms
                    .cmp(&a.1.last_seen_ms)
                    .then_with(|| a.0.cmp(&b.0))
            });
            valid_entries.truncate(MAX_TOFU_PEERS);
        }
        if listed > MAX_RESTORE_KEYS_PER_CATEGORY {
            warn!(
                listed,
                cap = MAX_RESTORE_KEYS_PER_CATEGORY,
                "TOFU store listed more peers than any legitimate run can produce; ignoring the \
                 tail rather than pruning it, since a dropped pin silently re-arms \
                 trust-on-first-use"
            );
        }
        let restored = valid_entries.len() as u32;
        for (peer_id, entry) in valid_entries {
            self.known_peer_public_keys.insert(peer_id, entry);
        }
        if restored > 0 {
            info!(count = restored, "Restored TOFU key entries from storage");
        }
    }
}
