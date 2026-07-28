//! At-rest sealing for install-scoped protocol-state records.
//!
//! [`crate::ProtocolStateStorage`] exists to give delivery state the app
//! container's *lifecycle*. It must not also give it the app container's
//! *confidentiality*: pending-session entries carry message plaintext, and
//! outbox entries can carry cloud-media `encryption_key`/`iv` material. Before
//! this module those records were written to credential-backed storage, where
//! the OS keystore protected them at rest.
//!
//! So the SDK seals sensitive record values itself, with a per-install AEAD key
//! that lives in [`crate::MlsStorage`] (the credential store). The install-scoped
//! container only ever sees ciphertext, which restores the previous at-rest
//! protection while keeping the uninstall-scoped lifecycle that motivated the
//! storage split.
//!
//! # Envelope
//!
//! ```text
//! magic (4) | nonce (12) | ChaCha20-Poly1305(ciphertext || tag)
//! ```
//!
//! The associated data binds each record to its slot (`key_type` + `key_id`),
//! so a record cannot be moved between peers, message ids, or categories by
//! anyone with write access to the container. There is deliberately no
//! plaintext-passthrough fallback on read: an unopenable record is a corrupt or
//! tampered record, never a reason to accept unauthenticated bytes.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand_core::{OsRng, RngCore};
use zeroize::Zeroizing;

/// Length of the per-install record-sealing key.
pub(crate) const STATE_RECORD_KEY_BYTES: usize = 32;

/// Envelope version marker. A future format change bumps the last byte and
/// keeps opening the older tag for one release.
const RECORD_MAGIC: [u8; 4] = *b"OPS1";

/// ChaCha20-Poly1305 nonce length.
const NONCE_BYTES: usize = 12;

/// Separator between the two associated-data components, chosen because it
/// cannot appear in a `key_type` (a fixed SDK constant) and so cannot be used
/// to make two different (`key_type`, `key_id`) pairs produce the same AAD.
const AAD_SEPARATOR: u8 = 0x1f;

/// Seals and opens protocol-state record values with the per-install key.
pub(crate) struct StateRecordCipher {
    cipher: ChaCha20Poly1305,
}

impl std::fmt::Debug for StateRecordCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StateRecordCipher(<redacted>)")
    }
}

impl StateRecordCipher {
    /// Builds a cipher from the per-install record key.
    pub(crate) fn new(key: &[u8; STATE_RECORD_KEY_BYTES]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(key)),
        }
    }

    /// Generates a fresh per-install record key from the OS CSPRNG.
    pub(crate) fn generate_key() -> Zeroizing<[u8; STATE_RECORD_KEY_BYTES]> {
        let mut key = Zeroizing::new([0u8; STATE_RECORD_KEY_BYTES]);
        OsRng.fill_bytes(&mut *key);
        key
    }

    /// Seals `plaintext` for the (`key_type`, `key_id`) slot.
    ///
    /// Returns `None` only if the AEAD itself fails, which for this
    /// construction means an allocation failure — callers treat it like any
    /// other persistence failure (log and keep the in-memory copy).
    pub(crate) fn seal(&self, key_type: &str, key_id: &str, plaintext: &[u8]) -> Option<Vec<u8>> {
        let mut nonce_bytes = [0u8; NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let aad = Self::associated_data(key_type, key_id);

        let ciphertext = self
            .cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .ok()?;

        let mut sealed = Vec::with_capacity(RECORD_MAGIC.len() + NONCE_BYTES + ciphertext.len());
        sealed.extend_from_slice(&RECORD_MAGIC);
        sealed.extend_from_slice(&nonce_bytes);
        sealed.extend_from_slice(&ciphertext);
        Some(sealed)
    }

    /// Opens a sealed record for the (`key_type`, `key_id`) slot.
    ///
    /// Returns `None` for anything that is not an authentic envelope for this
    /// exact slot: wrong magic, truncated, wrong key, tampered ciphertext, or a
    /// record lifted from another slot. Callers treat that as a corrupt record
    /// and drop it.
    pub(crate) fn open(&self, key_type: &str, key_id: &str, sealed: &[u8]) -> Option<Vec<u8>> {
        let header = RECORD_MAGIC.len() + NONCE_BYTES;
        if sealed.len() <= header || sealed[..RECORD_MAGIC.len()] != RECORD_MAGIC {
            return None;
        }
        let nonce = Nonce::from_slice(&sealed[RECORD_MAGIC.len()..header]);
        let aad = Self::associated_data(key_type, key_id);
        self.cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &sealed[header..],
                    aad: &aad,
                },
            )
            .ok()
    }

    /// Whether `data` carries the sealed-record envelope marker. Used only to
    /// tell "this record was never sealed" from "this record will not open",
    /// for diagnostics — never to accept unsealed bytes.
    pub(crate) fn looks_sealed(data: &[u8]) -> bool {
        data.len() > RECORD_MAGIC.len() && data[..RECORD_MAGIC.len()] == RECORD_MAGIC
    }

    fn associated_data(key_type: &str, key_id: &str) -> Vec<u8> {
        let mut aad = Vec::with_capacity(RECORD_MAGIC.len() + key_type.len() + 1 + key_id.len());
        aad.extend_from_slice(&RECORD_MAGIC);
        aad.extend_from_slice(key_type.as_bytes());
        aad.push(AAD_SEPARATOR);
        aad.extend_from_slice(key_id.as_bytes());
        aad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> StateRecordCipher {
        StateRecordCipher::new(&[7u8; STATE_RECORD_KEY_BYTES])
    }

    #[test]
    fn seal_then_open_roundtrips() {
        let cipher = cipher();
        let sealed = cipher.seal("pending_messages", "bob", b"secret").unwrap();
        assert_ne!(sealed.windows(6).position(|w| w == b"secret"), Some(0));
        assert!(!sealed.windows(6).any(|window| window == b"secret"));
        assert_eq!(
            cipher.open("pending_messages", "bob", &sealed).unwrap(),
            b"secret"
        );
    }

    #[test]
    fn open_rejects_a_record_lifted_from_another_slot() {
        let cipher = cipher();
        let sealed = cipher.seal("pending_messages", "bob", b"secret").unwrap();
        assert!(cipher.open("pending_messages", "carol", &sealed).is_none());
        assert!(cipher.open("outbox", "bob", &sealed).is_none());
    }

    #[test]
    fn open_rejects_a_foreign_key_tampering_and_plaintext() {
        let sealed = cipher().seal("outbox", "msg-1", b"secret").unwrap();

        let foreign = StateRecordCipher::new(&[9u8; STATE_RECORD_KEY_BYTES]);
        assert!(foreign.open("outbox", "msg-1", &sealed).is_none());

        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 0x01;
        assert!(cipher().open("outbox", "msg-1", &tampered).is_none());

        // A plaintext record is not silently accepted.
        assert!(cipher().open("outbox", "msg-1", b"{\"a\":1}").is_none());
        assert!(!StateRecordCipher::looks_sealed(b"{\"a\":1}"));
        assert!(StateRecordCipher::looks_sealed(&sealed));
    }

    #[test]
    fn nonces_do_not_repeat_across_seals() {
        let cipher = cipher();
        let first = cipher.seal("outbox", "msg-1", b"same").unwrap();
        let second = cipher.seal("outbox", "msg-1", b"same").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn generated_keys_are_not_constant() {
        assert_ne!(
            *StateRecordCipher::generate_key(),
            *StateRecordCipher::generate_key()
        );
    }
}
