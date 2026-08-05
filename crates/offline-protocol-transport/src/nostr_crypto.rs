//! Nostr event creation and BIP-340 Schnorr signing.
//!
//! Implements NIP-01 event serialization, SHA-256 event ID computation,
//! and BIP-340 Schnorr signatures using the secp256k1 curve (via `k256`).
//!
//! The platform side (iOS/Android) no longer handles crypto — it receives
//! pre-signed event JSON strings ready for relay submission.
//!
//! # Key model
//!
//! Two independent values are in play, and they must not be conflated:
//!
//! - **Routing tag** ([`routing_tag_for_device_id`]): a public rendezvous
//!   label derived deterministically from a device/user ID. Senders put the
//!   recipient's tag in the event's `#p` tag; the recipient subscribes on its
//!   own tag. It carries no secret — anyone who knows a device ID can (and
//!   must be able to) compute it, exactly like an email address.
//! - **Signing key** ([`NostrKeypair`]): the secp256k1 private key that signs
//!   outgoing events. It is derived via HKDF-SHA256 from a per-install random
//!   secret ([`NostrKeypair::from_install_secret`]) and is never derivable
//!   from any public identifier.
//!
//! Historically both roles were served by `SHA-256(device_id)`, which let
//! anyone who knew a device ID reconstruct that device's private key and sign
//! events as it. Only the routing-tag role legitimately needs to be publicly
//! derivable, so only it retains the deterministic derivation (unchanged on
//! the wire for interoperability with older peers).

use crate::constants::NOSTR_INITIAL_QUERY_LIMIT;
use crate::{Error, Result};
use hkdf::Hkdf;
use k256::schnorr::SigningKey;
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Domain-separation prefix for deriving the Nostr signing key from the
/// per-install secret. A trailing counter byte is appended per attempt so an
/// (astronomically unlikely) invalid scalar can be retried deterministically.
const SIGNING_KEY_HKDF_INFO: &[u8] = b"offline-protocol/nostr/v1/signing-key/";

/// Upper bound on HKDF derivation attempts. Each attempt fails with
/// probability ~2^-128 (scalar of zero or above the curve order), so more
/// than one iteration is never expected in practice.
const MAX_DERIVE_ATTEMPTS: u8 = 8;

/// Minimum accepted install-secret length in bytes (128-bit security floor).
const MIN_INSTALL_SECRET_LEN: usize = 16;

/// A Nostr signing keypair for this install.
///
/// The private key is derived from a per-install random secret via
/// HKDF-SHA256 with domain separation (see [`Self::from_install_secret`]),
/// or freshly generated ([`Self::generate_ephemeral`]) when no persisted
/// secret is available yet. The public key is the x-only coordinate
/// (32 bytes, 64 hex chars) per BIP-340.
///
/// The keypair authenticates this install to Nostr relays (event signatures,
/// NIP-42 style auth). It is intentionally *not* derivable from the device
/// ID; message addressing uses the separate [`routing_tag_for_device_id`]
/// label instead.
pub struct NostrKeypair {
    signing_key: SigningKey,
    public_key_hex: String,
}

impl NostrKeypair {
    /// Derives the signing keypair from a per-install random secret.
    ///
    /// Derivation: `HKDF-SHA256(ikm = secret, salt = none,
    /// info = "offline-protocol/nostr/v1/signing-key/" || counter)` → 32-byte
    /// scalar → secp256k1 signing key. Deterministic: the same secret always
    /// yields the same keypair, so the install's Nostr identity is stable
    /// across restarts as long as the secret persists.
    ///
    /// Rejects secrets shorter than 16 bytes.
    pub fn from_install_secret(secret: &[u8]) -> Result<Self> {
        if secret.len() < MIN_INSTALL_SECRET_LEN {
            return Err(Error::CryptoError(format!(
                "Install secret too short: {} bytes (minimum {})",
                secret.len(),
                MIN_INSTALL_SECRET_LEN
            )));
        }

        let hkdf = Hkdf::<Sha256>::new(None, secret);
        let mut info = Vec::with_capacity(SIGNING_KEY_HKDF_INFO.len() + 1);
        for counter in 0..MAX_DERIVE_ATTEMPTS {
            info.clear();
            info.extend_from_slice(SIGNING_KEY_HKDF_INFO);
            info.push(counter);

            let mut candidate = Zeroizing::new([0u8; 32]);
            hkdf.expand(&info, &mut *candidate)
                .map_err(|e| Error::CryptoError(format!("HKDF expand failed: {}", e)))?;

            if let Ok(signing_key) = SigningKey::from_bytes(&*candidate) {
                let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
                return Ok(Self {
                    signing_key,
                    public_key_hex,
                });
            }
        }

        Err(Error::CryptoError(
            "Failed to derive a valid signing key from the install secret".to_string(),
        ))
    }

    /// Generates a keypair from a fresh random secret that is not retained.
    ///
    /// Used as the construction-time default before a persisted install
    /// secret is available: the key is unforgeable but changes on every
    /// process start. Callers that want a stable identity install a
    /// persisted secret afterwards via [`Self::from_install_secret`].
    pub fn generate_ephemeral() -> Result<Self> {
        let secret = Self::generate_install_secret()?;
        Self::from_install_secret(&*secret)
    }

    /// Generates a new 32-byte install secret from the OS CSPRNG.
    ///
    /// The caller is responsible for persisting it in platform-secure
    /// storage; this module never stores anything. The returned buffer is
    /// zeroed on drop.
    pub fn generate_install_secret() -> Result<Zeroizing<[u8; 32]>> {
        let mut secret = Zeroizing::new([0u8; 32]);
        OsRng
            .try_fill_bytes(&mut *secret)
            .map_err(|e| Error::CryptoError(format!("OS RNG failure: {}", e)))?;
        Ok(secret)
    }

    /// Returns the x-only public key as a 64-character lowercase hex string.
    pub fn public_key_hex(&self) -> &str {
        &self.public_key_hex
    }
}

/// Current wall-clock time as unix seconds — the unit every Nostr timestamp
/// (`created_at`, filter `since`/`until`) is expressed in.
///
/// Saturates to 0 rather than panicking if the system clock is set before the
/// epoch; a zero here only widens a subscription window.
pub(crate) fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Computes the public routing tag for a device/user ID.
///
/// Derivation: `SHA-256(device_id)` → scalar → x-only secp256k1 public key
/// hex — byte-identical to the legacy shared derivation, so old and new
/// versions address each other without a migration.
///
/// Senders place the recipient's tag in the `#p` tag of outgoing events and
/// recipients subscribe on their own tag. It is a rendezvous label only:
/// nothing signs with the corresponding scalar, and incoming events are never
/// authenticated against it (sender authenticity comes from the
/// protocol-layer MLS signatures).
pub fn routing_tag_for_device_id(device_id: &str) -> Result<String> {
    let tag_scalar = Sha256::digest(device_id.as_bytes());
    let tag_key = SigningKey::from_bytes(tag_scalar.as_slice())
        .map_err(|e| Error::CryptoError(format!("Invalid routing tag for device_id: {}", e)))?;
    Ok(hex::encode(tag_key.verifying_key().to_bytes()))
}

/// A fully signed NIP-01 Nostr event ready for relay submission.
#[derive(Debug, Clone)]
pub struct NostrEvent {
    /// Event ID: 32-byte hex SHA-256 of the canonical serialization.
    pub id: String,
    /// Sender's x-only public key (32-byte hex).
    pub pubkey: String,
    /// Unix timestamp in seconds.
    pub created_at: i64,
    /// Event kind (4 = NIP-04 direct message).
    pub kind: u32,
    /// Event tags (e.g., `[["p", "<recipient_pubkey>"]]`).
    pub tags: Vec<Vec<String>>,
    /// Event content (base64-encoded protocol message).
    pub content: String,
    /// BIP-340 Schnorr signature (64-byte hex).
    pub sig: String,
}

impl NostrEvent {
    /// Creates and signs a NIP-04 direct message event.
    ///
    /// - `keypair`: Sender's signing keypair (per-install secret key).
    /// - `recipient_pubkey_hex`: Recipient's routing tag (64-char hex, from
    ///   [`routing_tag_for_device_id`]).
    /// - `content_base64`: Base64-encoded protocol message bytes.
    pub fn create_dm(
        keypair: &NostrKeypair,
        recipient_pubkey_hex: &str,
        content_base64: &str,
    ) -> Result<Self> {
        let created_at = now_unix_secs();
        let kind: u32 = 4;
        let tags = vec![vec!["p".to_string(), recipient_pubkey_hex.to_string()]];
        let pubkey = keypair.public_key_hex().to_string();

        // NIP-01 canonical serialization for event ID computation:
        // [0, <pubkey>, <created_at>, <kind>, <tags_json>, <content>]
        let tags_json =
            serde_json::to_string(&tags).map_err(|e| Error::SerializationError(e.to_string()))?;
        let content_escaped = serde_json::to_string(content_base64)
            .map_err(|e| Error::SerializationError(e.to_string()))?;
        let serialized = format!(
            "[0,\"{}\",{},{},{},{}]",
            pubkey, created_at, kind, tags_json, content_escaped
        );

        // Event ID = SHA-256 of the serialized event
        let event_id_bytes = Sha256::digest(serialized.as_bytes());
        let event_id = hex::encode(event_id_bytes);

        // BIP-340 Schnorr signature of the event ID
        let sig = sign_event_id(&keypair.signing_key, &event_id_bytes)?;

        Ok(Self {
            id: event_id,
            pubkey,
            created_at,
            kind,
            tags,
            content: content_base64.to_string(),
            sig,
        })
    }

    /// Serializes to the relay `["EVENT", {...}]` message format.
    pub fn to_relay_message(&self) -> Result<String> {
        let event_json = self.to_event_json()?;
        Ok(format!("[\"EVENT\",{}]", event_json))
    }

    /// Serializes just the event object (without the `["EVENT", ...]` wrapper).
    fn to_event_json(&self) -> Result<String> {
        // Use serde_json for safe serialization (no injection via field values).
        // Field order matches NIP-01 convention; most relays accept any order.
        let obj = serde_json::json!({
            "id": self.id,
            "pubkey": self.pubkey,
            "created_at": self.created_at,
            "kind": self.kind,
            "tags": self.tags,
            "content": self.content,
            "sig": self.sig,
        });
        serde_json::to_string(&obj).map_err(|e| Error::SerializationError(e.to_string()))
    }
}

/// Creates a NIP-01 REQ subscription message for kind-4 DMs addressed to `pubkey_hex`
/// (this device's own routing tag, from [`routing_tag_for_device_id`]).
///
/// Returns
/// `["REQ", "<sub_id>", {"#p": ["<pubkey>"], "kinds": [4], "since": T, "limit": N}]`.
///
/// The two bounds do different jobs and neither replaces the other:
///
/// - `since` (unix seconds, **inclusive** per NIP-01) is the real bound on
///   replay: it says how far back history may reach at all. The caller derives
///   it from the persisted receive watermark — see
///   [`NostrTransport::create_subscription`](crate::nostr::NostrTransport::create_subscription).
/// - [`NOSTR_INITIAL_QUERY_LIMIT`] caps how much of that window one initial
///   query may return. It is advisory in both directions (`limit` is a SHOULD,
///   and NIP-11 `max_limit` lets a relay clamp it silently), so it bounds the
///   worst case without being something to reason from.
///
/// Because `since` is inclusive, the event that set the watermark is returned
/// again on the next connect. That duplicate is expected; the alternative
/// (`since + 1`) would drop any event sharing that exact second.
///
/// **How much of the replayed overlap dedup actually absorbs is bounded, and
/// the bound is smaller than the overlap.** The engine's deduplicator retains
/// ids for `DeduplicatorConfig::retention_time_secs` (1 h by default) and at
/// most `max_tracked_messages` of them, while `since` reaches
/// `NOSTR_CREATED_AT_JITTER_SECS + NOSTR_CLOCK_SKEW_MARGIN_SECS` (1 h 5 min)
/// below the mark. A reconnect after longer than the retention window — the
/// ordinary case for a mobile app reopened the next day — therefore re-processes
/// the overlap rather than deduplicating it. That is a cost, not a loss: a
/// replayed ciphertext whose ratchet generation is spent fails closed as
/// `Decryption` and is dropped, a past-epoch one triggers at most one
/// rate-limited re-key, and a group copy TTLs out of the pending buffer. The
/// engine pins this relationship in
/// `nostr_replay_overlap_exceeds_dedup_retention` so the two constants cannot
/// drift apart from this note.
pub fn create_subscription_message(
    pubkey_hex: &str,
    subscription_id: &str,
    since: i64,
) -> Result<String> {
    let filter = serde_json::json!({
        "#p": [pubkey_hex],
        "kinds": [4],
        "since": since.max(0),
        "limit": NOSTR_INITIAL_QUERY_LIMIT
    });
    let msg = serde_json::json!(["REQ", subscription_id, filter]);
    serde_json::to_string(&msg).map_err(|e| Error::SerializationError(e.to_string()))
}

/// BIP-340 Schnorr signature of a 32-byte event ID hash.
fn sign_event_id(signing_key: &SigningKey, event_id_hash: &[u8]) -> Result<String> {
    use k256::ecdsa::signature::hazmat::PrehashSigner;
    let signature: k256::schnorr::Signature = signing_key
        .sign_prehash(event_id_hash)
        .map_err(|e| Error::CryptoError(format!("Schnorr signing failed: {}", e)))?;
    Ok(hex::encode(signature.to_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_install_secret_deterministic() {
        let secret = [7u8; 32];
        let kp = NostrKeypair::from_install_secret(&secret).unwrap();
        assert_eq!(kp.public_key_hex().len(), 64);
        // Deterministic: same secret → same key across "restarts"
        let kp2 = NostrKeypair::from_install_secret(&secret).unwrap();
        assert_eq!(kp.public_key_hex(), kp2.public_key_hex());
    }

    #[test]
    fn test_different_secrets_different_keys() {
        let kp1 = NostrKeypair::from_install_secret(&[1u8; 32]).unwrap();
        let kp2 = NostrKeypair::from_install_secret(&[2u8; 32]).unwrap();
        assert_ne!(kp1.public_key_hex(), kp2.public_key_hex());
    }

    #[test]
    fn test_from_install_secret_rejects_short_secret() {
        assert!(NostrKeypair::from_install_secret(&[0u8; 15]).is_err());
        assert!(NostrKeypair::from_install_secret(b"").is_err());
    }

    #[test]
    fn test_signing_key_not_derivable_from_device_id() {
        // The signing key must NOT equal the legacy SHA-256(device_id) key:
        // the routing tag stays publicly derivable, the signing key does not.
        let secret = Sha256::digest("alice".as_bytes());
        let kp = NostrKeypair::from_install_secret(secret.as_slice()).unwrap();
        let tag = routing_tag_for_device_id("alice").unwrap();
        assert_ne!(kp.public_key_hex(), tag);
    }

    #[test]
    fn test_generate_ephemeral_is_random() {
        let kp1 = NostrKeypair::generate_ephemeral().unwrap();
        let kp2 = NostrKeypair::generate_ephemeral().unwrap();
        assert_ne!(kp1.public_key_hex(), kp2.public_key_hex());
    }

    #[test]
    fn test_generate_install_secret_is_random() {
        let s1 = NostrKeypair::generate_install_secret().unwrap();
        let s2 = NostrKeypair::generate_install_secret().unwrap();
        assert_ne!(*s1, *s2);
        assert_ne!(*s1, [0u8; 32]);
    }

    #[test]
    fn test_routing_tag_wire_compat_golden_values() {
        // The routing tag is a cross-version wire contract: peers on older
        // SDK versions derive our tag as SHA-256(device_id) → x-only pubkey.
        // These golden values were computed from the pre-split derivation and
        // must never change, or addressing breaks against deployed peers.
        assert_eq!(
            routing_tag_for_device_id("alice").unwrap(),
            "9997a497d964fc1a62885b05a51166a65a90df00492c8d7cf61d6accf54803be"
        );
        assert_eq!(
            routing_tag_for_device_id("bob").unwrap(),
            "4edfcf9dfe6c0b5c83d1ab3f78d1b39a46ebac6798e08e19761f5ed89ec83c10"
        );
        assert_eq!(
            routing_tag_for_device_id("device1").unwrap(),
            "01194098eb3146ae142447c78a1fcf8df55b72b6d54f9eaa4b5b1c2a11826295"
        );
    }

    #[test]
    fn test_create_dm_event() {
        let sender = NostrKeypair::generate_ephemeral().unwrap();
        let recipient_pubkey = routing_tag_for_device_id("bob").unwrap();

        let event = NostrEvent::create_dm(&sender, &recipient_pubkey, "dGVzdCBtZXNzYWdl").unwrap();

        assert_eq!(event.id.len(), 64); // 32 bytes hex
        assert_eq!(event.pubkey, sender.public_key_hex());
        assert_eq!(event.kind, 4);
        assert_eq!(event.tags.len(), 1);
        assert_eq!(event.tags[0][0], "p");
        assert_eq!(event.tags[0][1], recipient_pubkey);
        assert_eq!(event.content, "dGVzdCBtZXNzYWdl");
        assert_eq!(event.sig.len(), 128); // 64 bytes hex
    }

    #[test]
    fn test_event_id_is_sha256_of_serialization() {
        let sender = NostrKeypair::generate_ephemeral().unwrap();
        let recipient_pubkey = routing_tag_for_device_id("bob").unwrap();

        let event = NostrEvent::create_dm(&sender, &recipient_pubkey, "dGVzdCBtZXNzYWdl").unwrap();

        // Recompute the event ID from the canonical serialization
        let tags_json = serde_json::to_string(&event.tags).unwrap();
        let content_escaped = serde_json::to_string(&event.content).unwrap();
        let serialized = format!(
            "[0,\"{}\",{},{},{},{}]",
            event.pubkey, event.created_at, event.kind, tags_json, content_escaped
        );
        let expected_id = hex::encode(Sha256::digest(serialized.as_bytes()));
        assert_eq!(event.id, expected_id);
    }

    #[test]
    fn test_signature_verification() {
        let sender = NostrKeypair::generate_ephemeral().unwrap();
        let recipient_pubkey = routing_tag_for_device_id("bob").unwrap();

        let event = NostrEvent::create_dm(&sender, &recipient_pubkey, "dGVzdCBtZXNzYWdl").unwrap();

        // Verify the signature
        let verifying_key = sender.signing_key.verifying_key();
        let event_id_bytes = hex::decode(&event.id).unwrap();
        let sig_bytes = hex::decode(&event.sig).unwrap();
        let signature = k256::schnorr::Signature::try_from(sig_bytes.as_slice()).unwrap();

        use k256::ecdsa::signature::hazmat::PrehashVerifier;
        verifying_key
            .verify_prehash(&event_id_bytes, &signature)
            .expect("Signature should be valid");
    }

    #[test]
    fn test_to_relay_message() {
        let sender = NostrKeypair::generate_ephemeral().unwrap();
        let recipient_pubkey = routing_tag_for_device_id("bob").unwrap();

        let event = NostrEvent::create_dm(&sender, &recipient_pubkey, "dGVzdCBtZXNzYWdl").unwrap();
        let relay_msg = event.to_relay_message().unwrap();

        assert!(relay_msg.starts_with("[\"EVENT\",{"));
        assert!(relay_msg.ends_with("}]"));
        assert!(relay_msg.contains(&format!("\"id\":\"{}\"", event.id)));
        assert!(relay_msg.contains(&format!("\"sig\":\"{}\"", event.sig)));
    }

    #[test]
    fn test_create_subscription_message() {
        let pubkey = routing_tag_for_device_id("alice").unwrap();
        let msg = create_subscription_message(&pubkey, "sub123", 1_700_000_000).unwrap();

        assert!(msg.starts_with("[\"REQ\",\"sub123\",{"));
        assert!(msg.contains(&format!("\"#p\":[\"{}\"]", pubkey)));
        assert!(msg.contains("\"kinds\":[4]"));
    }

    #[test]
    fn test_subscription_filter_bounds_stored_event_replay() {
        // Without a `limit` the filter is unbounded, so every relay
        // (re)connect replays the relay's whole retention window — which
        // NIP-11 no longer advertises, so it cannot even be reasoned about.
        let pubkey = routing_tag_for_device_id("alice").unwrap();
        let msg = create_subscription_message(&pubkey, "sub123", 1_700_000_000).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let filter = &parsed[2];
        assert_eq!(
            filter["limit"].as_u64(),
            Some(NOSTR_INITIAL_QUERY_LIMIT as u64),
            "REQ filter must carry a limit: {}",
            msg
        );
    }

    #[test]
    fn test_subscription_filter_carries_since() {
        let pubkey = routing_tag_for_device_id("alice").unwrap();
        let msg = create_subscription_message(&pubkey, "sub123", 1_700_000_000).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(
            parsed[2]["since"].as_i64(),
            Some(1_700_000_000),
            "REQ filter must carry the caller's since: {}",
            msg
        );
    }

    #[test]
    fn test_subscription_since_is_never_negative() {
        // A negative `since` is not a valid NIP-01 filter value; relays may
        // reject the whole REQ, which would take the subscription down rather
        // than merely widening it.
        let pubkey = routing_tag_for_device_id("alice").unwrap();
        let msg = create_subscription_message(&pubkey, "sub123", -42).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed[2]["since"].as_i64(), Some(0));
    }
}
