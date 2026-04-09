//! Nostr event creation and BIP-340 Schnorr signing.
//!
//! Implements NIP-01 event serialization, SHA-256 event ID computation,
//! and BIP-340 Schnorr signatures using the secp256k1 curve (via `k256`).
//!
//! The platform side (iOS/Android) no longer handles crypto — it receives
//! pre-signed event JSON strings ready for relay submission.

use crate::{Error, Result};
use k256::schnorr::SigningKey;
use sha2::{Digest, Sha256};

/// A Nostr keypair derived deterministically from a device/user ID.
///
/// Key derivation: `SHA-256(device_id)` → 32-byte scalar → secp256k1 signing key.
/// The public key is the x-only coordinate (32 bytes, 64 hex chars) per BIP-340.
///
/// **Security note:** Anyone who knows the device_id can derive the keypair.
/// This is acceptable because Nostr is used only as a transport layer; real
/// identity and integrity come from the protocol-layer MLS encryption.
pub struct NostrKeypair {
    signing_key: SigningKey,
    public_key_hex: String,
}

impl NostrKeypair {
    /// Derives a keypair deterministically from a device/user ID string.
    pub fn from_device_id(device_id: &str) -> Result<Self> {
        let secret_bytes = Sha256::digest(device_id.as_bytes());
        let signing_key = SigningKey::from_bytes(secret_bytes.as_slice()).map_err(|e| {
            Error::CryptoError(format!("Invalid private key from device_id: {}", e))
        })?;
        let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
        Ok(Self {
            signing_key,
            public_key_hex,
        })
    }

    /// Returns the x-only public key as a 64-character lowercase hex string.
    pub fn public_key_hex(&self) -> &str {
        &self.public_key_hex
    }

    /// Computes the Nostr public key hex for an arbitrary device_id.
    ///
    /// Both sender and recipient use this to derive each other's pubkeys
    /// without exchanging keys out-of-band.
    pub fn pubkey_hex_for_device_id(device_id: &str) -> Result<String> {
        let kp = Self::from_device_id(device_id)?;
        Ok(kp.public_key_hex)
    }
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
    /// - `keypair`: Sender's keypair (derived from their device_id).
    /// - `recipient_pubkey_hex`: Recipient's x-only pubkey (64-char hex).
    /// - `content_base64`: Base64-encoded protocol message bytes.
    pub fn create_dm(
        keypair: &NostrKeypair,
        recipient_pubkey_hex: &str,
        content_base64: &str,
    ) -> Result<Self> {
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
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

/// Creates a NIP-01 REQ subscription message for kind-4 DMs addressed to `pubkey_hex`.
///
/// Returns `["REQ", "<sub_id>", {"#p": ["<pubkey>"], "kinds": [4]}]`.
pub fn create_subscription_message(pubkey_hex: &str, subscription_id: &str) -> Result<String> {
    let filter = serde_json::json!({
        "#p": [pubkey_hex],
        "kinds": [4]
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
    fn test_keypair_from_device_id() {
        let kp = NostrKeypair::from_device_id("alice").unwrap();
        assert_eq!(kp.public_key_hex().len(), 64);
        // Deterministic: same input → same output
        let kp2 = NostrKeypair::from_device_id("alice").unwrap();
        assert_eq!(kp.public_key_hex(), kp2.public_key_hex());
    }

    #[test]
    fn test_different_device_ids_different_keys() {
        let kp1 = NostrKeypair::from_device_id("alice").unwrap();
        let kp2 = NostrKeypair::from_device_id("bob").unwrap();
        assert_ne!(kp1.public_key_hex(), kp2.public_key_hex());
    }

    #[test]
    fn test_pubkey_hex_for_device_id() {
        let kp = NostrKeypair::from_device_id("alice").unwrap();
        let pubkey = NostrKeypair::pubkey_hex_for_device_id("alice").unwrap();
        assert_eq!(kp.public_key_hex(), pubkey);
    }

    #[test]
    fn test_create_dm_event() {
        let sender = NostrKeypair::from_device_id("alice").unwrap();
        let recipient_pubkey = NostrKeypair::pubkey_hex_for_device_id("bob").unwrap();

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
        let sender = NostrKeypair::from_device_id("alice").unwrap();
        let recipient_pubkey = NostrKeypair::pubkey_hex_for_device_id("bob").unwrap();

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
        let sender = NostrKeypair::from_device_id("alice").unwrap();
        let recipient_pubkey = NostrKeypair::pubkey_hex_for_device_id("bob").unwrap();

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
        let sender = NostrKeypair::from_device_id("alice").unwrap();
        let recipient_pubkey = NostrKeypair::pubkey_hex_for_device_id("bob").unwrap();

        let event = NostrEvent::create_dm(&sender, &recipient_pubkey, "dGVzdCBtZXNzYWdl").unwrap();
        let relay_msg = event.to_relay_message().unwrap();

        assert!(relay_msg.starts_with("[\"EVENT\",{"));
        assert!(relay_msg.ends_with("}]"));
        assert!(relay_msg.contains(&format!("\"id\":\"{}\"", event.id)));
        assert!(relay_msg.contains(&format!("\"sig\":\"{}\"", event.sig)));
    }

    #[test]
    fn test_create_subscription_message() {
        let pubkey = NostrKeypair::pubkey_hex_for_device_id("alice").unwrap();
        let msg = create_subscription_message(&pubkey, "sub123").unwrap();

        assert!(msg.starts_with("[\"REQ\",\"sub123\",{"));
        assert!(msg.contains(&format!("\"#p\":[\"{}\"]", pubkey)));
        assert!(msg.contains("\"kinds\":[4]"));
    }
}
