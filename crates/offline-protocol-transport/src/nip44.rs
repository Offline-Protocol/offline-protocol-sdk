//! NIP-44 v2 encryption — the inner encryption layer of a Nostr gift wrap.
//!
//! Verified against the NIPs repository copy of `44.md` and the official
//! `nip44.vectors.json` (sha256 `269ed0f6…c25040`, the checksum the spec
//! itself publishes) on 2026-08-05. Every vector in that file plus the
//! extended-length-prefix table from the spec body is asserted in this
//! module's tests; see [`tests`].
//!
//! # Scheme
//!
//! ```text
//! conversation_key = HKDF-Extract(salt = "nip44-v2", IKM = x(a·B))
//! chacha_key ‖ chacha_nonce ‖ hmac_key = HKDF-Expand(conversation_key, info = nonce, L = 76)
//! ciphertext = ChaCha20(chacha_key, chacha_nonce, pad(plaintext))
//! mac        = HMAC-SHA256(hmac_key, nonce ‖ ciphertext)
//! payload    = base64(0x02 ‖ nonce ‖ ciphertext ‖ mac)
//! ```
//!
//! Three properties are easy to get wrong and are called out because getting
//! any of them wrong is silently interoperable-looking but broken:
//!
//! - **The ECDH output is not hashed.** NIP-44 uses the raw 32-byte x
//!   coordinate of `a·B` as HKDF input keying material. Most ECDH APIs hash
//!   it; [`k256`]'s `SharedSecret::raw_secret_bytes` does not, which is why
//!   it is used directly.
//! - **This is encrypt-then-MAC, not an AEAD**, and the MAC covers
//!   `nonce ‖ ciphertext` — the nonce is additional authenticated data, not
//!   just an IV. A MAC over the ciphertext alone verifies against these
//!   vectors for a *fixed* nonce and fails in the field.
//! - **The 32-byte value in the payload is not the ChaCha20 nonce.** It is
//!   HKDF `info`; the 12-byte cipher nonce comes out of the expansion.
//!
//! # Relationship to the transport's size cap
//!
//! Plaintexts of 65536 bytes and up carry a 6-byte extended length prefix
//! instead of the 2-byte one (added to NIP-44 on 2026-06-28, under the *same*
//! version byte `0x02` — there is no negotiation and no way to detect support).
//! [`NOSTR_MAX_PAYLOAD_SIZE`](crate::constants::NOSTR_MAX_PAYLOAD_SIZE) sits at
//! exactly that boundary, so both forms are implemented on both paths rather
//! than assuming our own cap keeps us on the short one — a peer is not bound by
//! our cap, and a decoder that only knows the u16 prefix would mis-parse the
//! first oversized frame it ever received.

use crate::{Error, Result};
use base64::Engine;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use hkdf::Hkdf;
use hmac::{Mac, SimpleHmac};
use k256::schnorr::{SigningKey, VerifyingKey};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

/// HKDF-Extract salt for the conversation key. Fixed by the spec.
const CONVERSATION_KEY_SALT: &[u8] = b"nip44-v2";

/// The only payload version this module produces or accepts.
const VERSION: u8 = 2;

/// Length of the HKDF-Expand output: 32-byte ChaCha20 key, 12-byte ChaCha20
/// nonce, 32-byte HMAC key.
const MESSAGE_KEYS_LEN: usize = 76;

/// Bytes of framing every payload carries: version (1) + nonce (32) + MAC (32).
const OVERHEAD_LEN: usize = 65;

/// Smallest decoded payload the spec permits: [`OVERHEAD_LEN`] plus the
/// smallest possible padded plaintext (2-byte prefix + 32-byte minimum pad).
const MIN_DECODED_LEN: usize = 99;

/// Smallest base64 payload the spec permits. Checked *before* base64 decoding
/// so a short-payload rejection never allocates.
///
/// Test-only along with [`decrypt`]: see the note there on why the string form
/// never appears on the production receive path.
#[cfg(test)]
const MIN_BASE64_LEN: usize = 132;

/// Plaintext length at or above which the 6-byte extended prefix is used.
const EXTENDED_PREFIX_THRESHOLD: u64 = 65536;

/// Largest plaintext this implementation will encrypt or return from a
/// decrypt, in bytes.
///
/// The spec's own maximum is `2^32 - 1` (~4 GB) but explicitly delegates a
/// tighter bound to implementations, precisely so a decoder cannot be made to
/// allocate gigabytes from a 6-byte length prefix. 1 MiB is far above anything
/// this transport can carry —
/// [`NOSTR_MAX_PAYLOAD_SIZE`](crate::constants::NOSTR_MAX_PAYLOAD_SIZE) caps a
/// whole relay message at 64 KiB — and is checked against the *declared*
/// length before any buffer is sized from it.
const MAX_PLAINTEXT_LEN: u64 = 1024 * 1024;

/// A NIP-44 conversation key: the long-term symmetric secret shared by two
/// keypairs, from which every per-message key is expanded.
///
/// Zeroed on drop. Deliberately opaque — it must never be logged, serialized,
/// or compared non-constant-time.
pub(crate) struct ConversationKey([u8; 32]);

impl Drop for ConversationKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl ConversationKey {
    /// Derives the conversation key from our secret key and a peer's x-only
    /// (BIP-340, 32-byte) public key.
    ///
    /// Symmetric by construction: `derive(a, B) == derive(b, A)`. The x
    /// coordinate of `a·B` is invariant under negation of either operand, so
    /// BIP-340's even-y convention — which can flip the sign of the scalar a
    /// signer stores relative to the point a verifier reconstructs — cannot
    /// make the two sides disagree.
    pub(crate) fn derive(secret: &SigningKey, peer_xonly: &[u8]) -> Result<Self> {
        // Rejects off-curve x values; `lift_x` picks the even-y point, per BIP-340.
        let peer = VerifyingKey::from_bytes(peer_xonly).map_err(|_| {
            Error::CryptoError("NIP-44: peer public key is not a valid curve point".to_string())
        })?;

        // NIP-44 uses the *unhashed* x coordinate as HKDF input keying
        // material. `raw_secret_bytes` is that coordinate verbatim; the
        // hashed `extract` helpers on this type would silently produce a
        // different key that no other implementation agrees with.
        let shared = k256::ecdh::diffie_hellman(secret.as_nonzero_scalar(), peer.as_affine());
        let (prk, _) = Hkdf::<Sha256>::extract(
            Some(CONVERSATION_KEY_SALT),
            shared.raw_secret_bytes().as_slice(),
        );

        let mut key = [0u8; 32];
        key.copy_from_slice(prk.as_slice());
        Ok(Self(key))
    }

    /// Builds a conversation key from raw bytes. Test-only: the real path
    /// always derives one, and accepting raw key material anywhere else would
    /// invite bypassing that.
    #[cfg(test)]
    fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Expands the per-message ChaCha20 key, ChaCha20 nonce, and HMAC key.
    ///
    /// The 32-byte `nonce` is HKDF `info` — it is *not* handed to the cipher.
    fn message_keys(&self, nonce: &[u8; 32]) -> Result<MessageKeys> {
        let hkdf = Hkdf::<Sha256>::from_prk(&self.0)
            .map_err(|e| Error::CryptoError(format!("NIP-44: invalid conversation key: {}", e)))?;
        let mut okm = Zeroizing::new([0u8; MESSAGE_KEYS_LEN]);
        hkdf.expand(nonce, &mut *okm)
            .map_err(|e| Error::CryptoError(format!("NIP-44: HKDF expand failed: {}", e)))?;

        let mut keys = MessageKeys {
            chacha_key: [0u8; 32],
            chacha_nonce: [0u8; 12],
            hmac_key: [0u8; 32],
        };
        keys.chacha_key.copy_from_slice(&okm[0..32]);
        keys.chacha_nonce.copy_from_slice(&okm[32..44]);
        keys.hmac_key.copy_from_slice(&okm[44..76]);
        Ok(keys)
    }
}

/// Per-message keys expanded from a [`ConversationKey`] and a 32-byte nonce.
struct MessageKeys {
    chacha_key: [u8; 32],
    chacha_nonce: [u8; 12],
    hmac_key: [u8; 32],
}

impl Drop for MessageKeys {
    fn drop(&mut self) {
        self.chacha_key.zeroize();
        self.chacha_nonce.zeroize();
        self.hmac_key.zeroize();
    }
}

/// Length of the padded plaintext (excluding the length prefix) for an
/// unpadded plaintext of `unpadded_len` bytes.
///
/// Powers-of-two bucketing with a 32-byte floor, so the ciphertext length
/// leaks only a coarse bucket rather than the exact message size. Computed in
/// `u64` because the result reaches `2^32` for the largest permitted
/// plaintexts, which does not fit a `u32`.
fn calc_padded_len(unpadded_len: u64) -> u64 {
    if unpadded_len <= 32 {
        return 32;
    }
    // `next_power` is 2^(floor(log2(unpadded_len - 1)) + 1). `unpadded_len` is
    // at least 33 here, so the shift argument is in 6..=32 and cannot overflow.
    let next_power = 1u64 << (64 - (unpadded_len - 1).leading_zeros());
    let chunk = if next_power <= 256 {
        32
    } else {
        next_power / 8
    };
    chunk * ((unpadded_len - 1) / chunk + 1)
}

/// Prefixes `plaintext` with its big-endian length and zero-pads it to the
/// next bucket.
///
/// Lengths below [`EXTENDED_PREFIX_THRESHOLD`] use a 2-byte `u16` prefix;
/// larger ones use `0x00 0x00` followed by a 4-byte `u32`. A leading `u16` of
/// zero is what signals the extended form, and is otherwise impossible because
/// the minimum plaintext is one byte.
fn pad(plaintext: &[u8]) -> Result<Vec<u8>> {
    let unpadded_len = plaintext.len() as u64;
    if unpadded_len == 0 {
        return Err(Error::CryptoError(
            "NIP-44: plaintext must be at least 1 byte".to_string(),
        ));
    }
    if unpadded_len > MAX_PLAINTEXT_LEN {
        return Err(Error::MessageTooLarge(
            plaintext.len(),
            MAX_PLAINTEXT_LEN as usize,
        ));
    }

    let padded_len = calc_padded_len(unpadded_len);
    let mut out = Vec::with_capacity(6 + padded_len as usize);
    if unpadded_len >= EXTENDED_PREFIX_THRESHOLD {
        out.extend_from_slice(&[0u8, 0u8]);
        out.extend_from_slice(&(unpadded_len as u32).to_be_bytes());
    } else {
        out.extend_from_slice(&(unpadded_len as u16).to_be_bytes());
    }
    out.extend_from_slice(plaintext);
    out.resize(out.len() + (padded_len - unpadded_len) as usize, 0);
    Ok(out)
}

/// Inverse of [`pad`].
///
/// Validates the declared length against both the slice actually present and
/// the padding the encoder must have produced, so a truncated or re-padded
/// frame is rejected rather than silently yielding a shortened plaintext.
fn unpad(padded: &[u8]) -> Result<Vec<u8>> {
    let invalid = || Error::CryptoError("NIP-44: invalid padding".to_string());

    if padded.len() < 2 {
        return Err(invalid());
    }
    let first_two = u16::from_be_bytes([padded[0], padded[1]]);
    let (unpadded_len, prefix_len) = if first_two == 0 {
        if padded.len() < 6 {
            return Err(invalid());
        }
        let len = u32::from_be_bytes([padded[2], padded[3], padded[4], padded[5]]) as u64;
        // A short length in the extended form is not merely redundant: it is a
        // second encoding of the same plaintext, which would break the
        // one-payload-per-plaintext property the MAC is computed over.
        if len < EXTENDED_PREFIX_THRESHOLD {
            return Err(invalid());
        }
        (len, 6usize)
    } else {
        (u64::from(first_two), 2usize)
    };

    if unpadded_len > MAX_PLAINTEXT_LEN {
        return Err(Error::MessageTooLarge(
            unpadded_len as usize,
            MAX_PLAINTEXT_LEN as usize,
        ));
    }
    if padded.len() as u64 != prefix_len as u64 + calc_padded_len(unpadded_len) {
        return Err(invalid());
    }
    let end = prefix_len + unpadded_len as usize;
    padded
        .get(prefix_len..end)
        .map(<[u8]>::to_vec)
        .ok_or_else(invalid)
}

/// HMAC-SHA256 over `aad ‖ message`, where `aad` is the 32-byte nonce.
fn hmac_aad(key: &[u8; 32], message: &[u8], aad: &[u8; 32]) -> Result<[u8; 32]> {
    let mut mac = <SimpleHmac<Sha256> as Mac>::new_from_slice(key)
        .map_err(|e| Error::CryptoError(format!("NIP-44: invalid HMAC key: {}", e)))?;
    mac.update(aad);
    mac.update(message);
    Ok(mac.finalize().into_bytes().into())
}

/// Applies the ChaCha20 keystream in place (RFC 8439, counter starting at 0).
fn chacha20_apply(keys: &MessageKeys, buf: &mut [u8]) {
    let mut cipher = chacha20::ChaCha20::new(&keys.chacha_key.into(), &keys.chacha_nonce.into());
    cipher.apply_keystream(buf);
}

/// Encrypts `plaintext` under `conversation_key` with an explicit 32-byte
/// nonce, returning the base64 payload.
///
/// Exposed separately from [`encrypt`] only so the official test vectors —
/// which fix the nonce — can be asserted. Production callers must use
/// [`encrypt`]: reusing a nonce under one conversation key makes both messages
/// decryptable.
fn encrypt_with_nonce(
    plaintext: &[u8],
    conversation_key: &ConversationKey,
    nonce: &[u8; 32],
) -> Result<String> {
    let keys = conversation_key.message_keys(nonce)?;
    let mut buf = pad(plaintext)?;
    chacha20_apply(&keys, &mut buf);
    let mac = hmac_aad(&keys.hmac_key, &buf, nonce)?;

    let mut payload = Vec::with_capacity(OVERHEAD_LEN + buf.len());
    payload.push(VERSION);
    payload.extend_from_slice(nonce);
    payload.extend_from_slice(&buf);
    payload.extend_from_slice(&mac);
    Ok(base64::engine::general_purpose::STANDARD.encode(&payload))
}

/// Encrypts `plaintext` under `conversation_key` with a fresh random nonce.
pub(crate) fn encrypt(plaintext: &[u8], conversation_key: &ConversationKey) -> Result<String> {
    use rand_core::{OsRng, RngCore};
    let mut nonce = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut nonce)
        .map_err(|e| Error::CryptoError(format!("NIP-44: OS RNG failure: {}", e)))?;
    encrypt_with_nonce(plaintext, conversation_key, &nonce)
}

/// Decrypts a base64 NIP-44 v2 payload string, returning the plaintext bytes.
///
/// Fails closed on an unknown version, a malformed payload, a MAC mismatch, or
/// invalid padding. The MAC is checked in constant time and *before* the
/// plaintext is derived, so a forged payload never produces bytes a caller
/// could act on.
///
/// **Test-only.** This is the API shape the spec and its vectors are written
/// against, so it exists to assert conformance against them verbatim — but the
/// platform bridges base64-decode the event `content` before it ever reaches
/// Rust, so production always takes [`decrypt_bytes`]. Keeping this
/// `#[cfg(test)]` rather than `pub(crate)` is what guarantees the two forms
/// cannot drift into two differently-validated receive paths.
#[cfg(test)]
fn decrypt(payload: &str, conversation_key: &ConversationKey) -> Result<Vec<u8>> {
    // `#` is the spec's reserved marker for a future non-base64 encoding.
    // Reported as an unsupported version rather than a base64 error so the
    // distinction survives to the logs.
    if payload.starts_with('#') {
        return Err(Error::CryptoError(
            "NIP-44: unsupported (non-base64) payload encoding".to_string(),
        ));
    }
    if payload.len() < MIN_BASE64_LEN {
        return Err(Error::CryptoError(format!(
            "NIP-44: payload too short: {} chars",
            payload.len()
        )));
    }

    let data = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| Error::CryptoError(format!("NIP-44: base64 decode failed: {}", e)))?;
    decrypt_bytes(&data, conversation_key)
}

/// Decrypts an already-base64-decoded NIP-44 v2 payload.
///
/// The bridges base64-decode the Nostr event `content` before handing it to
/// the transport, so the raw-bytes form is the one the receive path actually
/// takes; [`decrypt`] is the string form used by the spec's test vectors.
pub(crate) fn decrypt_bytes(data: &[u8], conversation_key: &ConversationKey) -> Result<Vec<u8>> {
    if data.len() < MIN_DECODED_LEN {
        return Err(Error::CryptoError(format!(
            "NIP-44: payload too short: {} bytes",
            data.len()
        )));
    }
    if data[0] != VERSION {
        return Err(Error::CryptoError(format!(
            "NIP-44: unsupported version {}",
            data[0]
        )));
    }

    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&data[1..33]);
    let mac_start = data.len() - 32;
    let ciphertext = &data[33..mac_start];
    let mac = &data[mac_start..];

    let keys = conversation_key.message_keys(&nonce)?;
    let expected = hmac_aad(&keys.hmac_key, ciphertext, &nonce)?;
    if expected.ct_eq(mac).unwrap_u8() != 1 {
        return Err(Error::CryptoError("NIP-44: MAC mismatch".to_string()));
    }

    let mut buf = ciphertext.to_vec();
    chacha20_apply(&keys, &mut buf);
    unpad(&buf)
}

/// Whether `data` looks like a base64-decoded NIP-44 v2 payload.
///
/// A cheap shape check for the receive path's codec sniffing, not a
/// validation: it says "try to unseal this", and [`decrypt_bytes`] is what
/// decides. Version byte `0x02` cannot collide with the other two encodings
/// this transport accepts — JSON starts with `{` (`0x7B`) and the binary wire
/// codec with its `0xF5` magic.
pub(crate) fn looks_like_payload(data: &[u8]) -> bool {
    data.len() >= MIN_DECODED_LEN && data[0] == VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    /// The official vector file, vendored verbatim.
    ///
    /// `scripts/check-nip44-vectors.sh` re-asserts its sha256 against the
    /// checksum published in the spec, so a silent upstream edit cannot slip
    /// past review as "the tests still pass".
    const VECTORS: &str = include_str!("../tests/data/nip44.vectors.json");

    fn vectors() -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(VECTORS).expect("vectors parse")["v2"].clone()
    }

    fn hex32(s: &str) -> [u8; 32] {
        let bytes = hex::decode(s).expect("hex");
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        out
    }

    fn ck(hex_key: &str) -> ConversationKey {
        ConversationKey::from_bytes(hex32(hex_key))
    }

    /// x-only public key for a hex-encoded secret key.
    fn pubkey_of(sec_hex: &str) -> [u8; 32] {
        let sk = SigningKey::from_bytes(&hex::decode(sec_hex).expect("hex")).expect("valid sec");
        let mut out = [0u8; 32];
        out.copy_from_slice(&sk.verifying_key().to_bytes());
        out
    }

    fn sha256_hex(data: &[u8]) -> String {
        hex::encode(sha2::Sha256::digest(data))
    }

    // ---------------------------------------------------------------
    // Official vectors: valid
    // ---------------------------------------------------------------

    #[test]
    fn official_vectors_get_conversation_key() {
        let v = vectors();
        let cases = v["valid"]["get_conversation_key"].as_array().unwrap();
        assert_eq!(cases.len(), 35, "vector count changed; re-review the file");

        for (i, case) in cases.iter().enumerate() {
            let sec1 = case["sec1"].as_str().unwrap();
            let pub2 = case["pub2"].as_str().unwrap();
            let expected = case["conversation_key"].as_str().unwrap();

            let sk = SigningKey::from_bytes(&hex::decode(sec1).unwrap()).unwrap();
            let key = ConversationKey::derive(&sk, &hex::decode(pub2).unwrap()).unwrap();
            assert_eq!(hex::encode(key.0), expected, "conversation_key vector {i}");
        }
    }

    #[test]
    fn official_vectors_get_message_keys() {
        let v = vectors();
        let block = &v["valid"]["get_message_keys"];
        let conversation_key = ck(block["conversation_key"].as_str().unwrap());

        for (i, case) in block["keys"].as_array().unwrap().iter().enumerate() {
            let nonce = hex32(case["nonce"].as_str().unwrap());
            let keys = conversation_key.message_keys(&nonce).unwrap();

            assert_eq!(
                hex::encode(keys.chacha_key),
                case["chacha_key"].as_str().unwrap(),
                "chacha_key vector {i}"
            );
            assert_eq!(
                hex::encode(keys.chacha_nonce),
                case["chacha_nonce"].as_str().unwrap(),
                "chacha_nonce vector {i}"
            );
            assert_eq!(
                hex::encode(keys.hmac_key),
                case["hmac_key"].as_str().unwrap(),
                "hmac_key vector {i}"
            );
        }
    }

    #[test]
    fn official_vectors_calc_padded_len() {
        let v = vectors();
        for case in v["valid"]["calc_padded_len"].as_array().unwrap() {
            let unpadded = case[0].as_u64().unwrap();
            let expected = case[1].as_u64().unwrap();
            assert_eq!(calc_padded_len(unpadded), expected, "padded len {unpadded}");
        }
    }

    #[test]
    fn official_vectors_encrypt_decrypt() {
        let v = vectors();
        let cases = v["valid"]["encrypt_decrypt"].as_array().unwrap();
        assert_eq!(cases.len(), 10, "vector count changed; re-review the file");

        for (i, case) in cases.iter().enumerate() {
            let sec1 = case["sec1"].as_str().unwrap();
            let sec2 = case["sec2"].as_str().unwrap();
            let expected_ck = case["conversation_key"].as_str().unwrap();
            let nonce = hex32(case["nonce"].as_str().unwrap());
            let plaintext = case["plaintext"].as_str().unwrap();
            let payload = case["payload"].as_str().unwrap();

            // Sender: conv(sec1, pub2), then encrypt to the exact payload.
            let sk1 = SigningKey::from_bytes(&hex::decode(sec1).unwrap()).unwrap();
            let key1 = ConversationKey::derive(&sk1, &pubkey_of(sec2)).unwrap();
            assert_eq!(hex::encode(key1.0), expected_ck, "sender key, vector {i}");
            assert_eq!(
                encrypt_with_nonce(plaintext.as_bytes(), &key1, &nonce).unwrap(),
                payload,
                "payload, vector {i}"
            );

            // Receiver: the mirrored derivation must land on the same key and
            // recover the plaintext. This is what proves `conv(a,B) == conv(b,A)`
            // survives BIP-340's even-y normalization of the scalar.
            let sk2 = SigningKey::from_bytes(&hex::decode(sec2).unwrap()).unwrap();
            let key2 = ConversationKey::derive(&sk2, &pubkey_of(sec1)).unwrap();
            assert_eq!(hex::encode(key2.0), expected_ck, "receiver key, vector {i}");
            assert_eq!(
                decrypt(payload, &key2).unwrap(),
                plaintext.as_bytes(),
                "plaintext, vector {i}"
            );
        }
    }

    #[test]
    fn official_vectors_encrypt_decrypt_long_msg() {
        let v = vectors();
        for (i, case) in v["valid"]["encrypt_decrypt_long_msg"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let conversation_key = ck(case["conversation_key"].as_str().unwrap());
            let nonce = hex32(case["nonce"].as_str().unwrap());
            let pattern = case["pattern"].as_str().unwrap();
            let repeat = case["repeat"].as_u64().unwrap() as usize;

            let plaintext = pattern.repeat(repeat);
            assert_eq!(
                sha256_hex(plaintext.as_bytes()),
                case["plaintext_sha256"].as_str().unwrap(),
                "plaintext checksum, long vector {i}"
            );

            let payload = encrypt_with_nonce(plaintext.as_bytes(), &conversation_key, &nonce)
                .expect("encrypt long");
            assert_eq!(
                sha256_hex(payload.as_bytes()),
                case["payload_sha256"].as_str().unwrap(),
                "payload checksum, long vector {i}"
            );
            assert_eq!(
                decrypt(&payload, &conversation_key).unwrap(),
                plaintext.as_bytes(),
                "round trip, long vector {i}"
            );
        }
    }

    // ---------------------------------------------------------------
    // Official vectors: invalid
    // ---------------------------------------------------------------

    #[test]
    fn official_vectors_invalid_get_conversation_key() {
        let v = vectors();
        for case in v["invalid"]["get_conversation_key"].as_array().unwrap() {
            let sec1 = case["sec1"].as_str().unwrap();
            let pub2 = case["pub2"].as_str().unwrap();
            let note = case["note"].as_str().unwrap_or("");

            let derived = hex::decode(sec1)
                .ok()
                .and_then(|b| SigningKey::from_bytes(&b).ok())
                .and_then(|sk| {
                    hex::decode(pub2)
                        .ok()
                        .and_then(|p| ConversationKey::derive(&sk, &p).ok())
                });
            assert!(derived.is_none(), "must reject: {note}");
        }
    }

    #[test]
    fn official_vectors_invalid_decrypt() {
        let v = vectors();
        let cases = v["invalid"]["decrypt"].as_array().unwrap();
        assert_eq!(cases.len(), 12, "vector count changed; re-review the file");

        for case in cases {
            let conversation_key = ck(case["conversation_key"].as_str().unwrap());
            let payload = case["payload"].as_str().unwrap();
            let note = case["note"].as_str().unwrap_or("");
            assert!(
                decrypt(payload, &conversation_key).is_err(),
                "must reject: {note}"
            );
        }
    }

    #[test]
    fn empty_plaintext_is_rejected() {
        // `invalid.encrypt_msg_lengths` in the vector file is `[0, 65536,
        // 100000, 10000000]`, but that file predates the 2026-06-28 extended
        // length prefix and was never regenerated — the same spec document
        // that publishes its checksum now *requires* 65536 and above to
        // encrypt via the 6-byte prefix, and supplies vectors for exactly
        // those lengths (asserted in `spec_extended_length_prefix_vectors`).
        // Only the zero-length case still holds, so only it is asserted here.
        // Do not "restore" the other three: doing so would delete extended
        // prefix support and silently break interop with any peer that sends
        // a large payload.
        let key = ck("c41c775356fd92eadc63ff5a0dc1da211b268cbea22316767095b2871ea1412d");
        assert!(encrypt(b"", &key).is_err());
    }

    // ---------------------------------------------------------------
    // Extended length prefix (spec body, table under "Tests and code")
    // ---------------------------------------------------------------

    #[test]
    fn spec_extended_length_prefix_vectors() {
        // From `44.md` § "Extended length prefix test vectors". These are the
        // boundary this transport's own 64 KiB cap sits exactly on, which is
        // why both prefix forms are implemented rather than assumed away.
        let key = ck("c41c775356fd92eadc63ff5a0dc1da211b268cbea22316767095b2871ea1412d");
        let nonce = hex32("0000000000000000000000000000000000000000000000000000000000000001");

        let cases: [(usize, &str, &str); 3] = [
            (
                65535,
                "6e1bebca6a8229364a162a72ef064826c4cd7457bf54f190ef782bd9deff3e42",
                "6d8c2810d1e870fbaa1f0a0937126cca837a15f9260e27060c331d70a3c0bc84",
            ),
            (
                65536,
                "bf718b6f653bebc184e1479f1935b8da974d701b893afcf49e701f3e2f9f9c5a",
                "b7b4edb36ba92e267d322d56d9aebc22e7fa96ff52e3c12adc07f07a43cbc616",
            ),
            (
                65537,
                "008ffc88d3c96a9f307524eb361e47c5222a887fc45fa0c1fb8d429c5c23b430",
                "eeb7c7c5373894ea2c1547cfd3ccb15d5a0b2d619da852e5c79df792dcc9e435",
            ),
        ];

        for (len, plaintext_sha, payload_sha) in cases {
            let plaintext = vec![b'a'; len];
            assert_eq!(sha256_hex(&plaintext), plaintext_sha, "plaintext len {len}");

            let payload = encrypt_with_nonce(&plaintext, &key, &nonce).unwrap();
            assert_eq!(
                sha256_hex(payload.as_bytes()),
                payload_sha,
                "payload for len {len}"
            );
            assert_eq!(
                decrypt(&payload, &key).unwrap(),
                plaintext,
                "round trip at len {len}"
            );
        }
    }

    #[test]
    fn extended_prefix_switches_exactly_at_the_threshold() {
        // The prefix width is not negotiated and rides under the same version
        // byte, so an off-by-one here is undetectable on the wire and corrupts
        // every large message.
        let below = pad(&vec![b'x'; (EXTENDED_PREFIX_THRESHOLD - 1) as usize]).unwrap();
        assert_ne!(&below[0..2], &[0, 0], "65535 must use the u16 prefix");

        let at = pad(&vec![b'x'; EXTENDED_PREFIX_THRESHOLD as usize]).unwrap();
        assert_eq!(&at[0..2], &[0, 0], "65536 must use the extended prefix");
        assert_eq!(
            u32::from_be_bytes([at[2], at[3], at[4], at[5]]) as u64,
            EXTENDED_PREFIX_THRESHOLD
        );
    }

    #[test]
    fn extended_prefix_declaring_a_short_length_is_rejected() {
        // Two encodings of one plaintext would break the one-payload-per-
        // plaintext property, so the redundant form must not decode.
        let mut padded = vec![0u8, 0u8];
        padded.extend_from_slice(&100u32.to_be_bytes());
        padded.extend_from_slice(&[b'x'; 100]);
        padded.resize(6 + calc_padded_len(100) as usize, 0);
        assert!(unpad(&padded).is_err());
    }

    // ---------------------------------------------------------------
    // Local behaviour
    // ---------------------------------------------------------------

    #[test]
    fn round_trip_over_non_utf8_bytes() {
        // The transport seals a serialized `Message`, which may be the binary
        // wire codec (leading 0xF5) rather than JSON. NIP-44 is specified over
        // UTF-8 strings, but every step below the API is byte-oriented, so
        // this must work — and it is the common case once binary wire is
        // negotiated.
        let alice = SigningKey::from_bytes(&[3u8; 32]).unwrap();
        let bob = SigningKey::from_bytes(&[5u8; 32]).unwrap();
        let key_a = ConversationKey::derive(&alice, &bob.verifying_key().to_bytes()).unwrap();
        let key_b = ConversationKey::derive(&bob, &alice.verifying_key().to_bytes()).unwrap();

        let plaintext: Vec<u8> = (0u8..=255).cycle().take(1000).collect();
        assert!(std::str::from_utf8(&plaintext).is_err(), "test premise");

        let payload = encrypt(&plaintext, &key_a).unwrap();
        assert_eq!(decrypt(&payload, &key_b).unwrap(), plaintext);
    }

    #[test]
    fn nonce_is_fresh_per_message() {
        // Nonce reuse under one conversation key makes both messages
        // decryptable, so identical plaintexts must not produce identical
        // payloads.
        let sk = SigningKey::from_bytes(&[9u8; 32]).unwrap();
        let peer = SigningKey::from_bytes(&[11u8; 32]).unwrap();
        let key = ConversationKey::derive(&sk, &peer.verifying_key().to_bytes()).unwrap();

        let a = encrypt(b"same plaintext", &key).unwrap();
        let b = encrypt(b"same plaintext", &key).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_key_fails_on_the_mac_not_the_padding() {
        // Fail-closed ordering: a forged or misaddressed payload must be
        // rejected by the MAC, before any plaintext bytes exist to act on.
        let sk = SigningKey::from_bytes(&[13u8; 32]).unwrap();
        let peer = SigningKey::from_bytes(&[17u8; 32]).unwrap();
        let stranger = SigningKey::from_bytes(&[19u8; 32]).unwrap();

        let key = ConversationKey::derive(&sk, &peer.verifying_key().to_bytes()).unwrap();
        let wrong = ConversationKey::derive(&sk, &stranger.verifying_key().to_bytes()).unwrap();

        let payload = encrypt(b"secret", &key).unwrap();
        let err = decrypt(&payload, &wrong).unwrap_err();
        assert!(
            err.to_string().contains("MAC mismatch"),
            "expected MAC rejection, got {err}"
        );
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let sk = SigningKey::from_bytes(&[23u8; 32]).unwrap();
        let peer = SigningKey::from_bytes(&[29u8; 32]).unwrap();
        let key = ConversationKey::derive(&sk, &peer.verifying_key().to_bytes()).unwrap();

        let payload = encrypt(b"authentic message", &key).unwrap();
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(&payload)
            .unwrap();
        let mid = raw.len() / 2;
        raw[mid] ^= 0x01;
        assert!(decrypt_bytes(&raw, &key).is_err());
    }

    #[test]
    fn oversized_declared_length_is_rejected_without_allocating() {
        // The 6-byte prefix lets a peer *declare* a 4 GB plaintext. The
        // declared length must be checked against the cap before any buffer is
        // sized from it, or a 100-byte frame becomes an OOM.
        let mut padded = vec![0u8, 0u8];
        padded.extend_from_slice(&u32::MAX.to_be_bytes());
        padded.extend_from_slice(&[b'x'; 64]);
        assert!(matches!(unpad(&padded), Err(Error::MessageTooLarge(_, _))));
    }

    #[test]
    fn version_byte_cannot_collide_with_the_other_wire_encodings() {
        // The receive path sniffs the first byte to pick a decoder. NIP-44's
        // 0x02, the binary wire codec's 0xF5, and JSON's '{' must stay
        // mutually exclusive or a frame is handed to the wrong parser.
        assert_ne!(VERSION, offline_protocol_core::WIRE_V1_MAGIC);
        assert_ne!(VERSION, b'{');
        assert!(looks_like_payload(&[VERSION; MIN_DECODED_LEN]));
        assert!(!looks_like_payload(&[VERSION; MIN_DECODED_LEN - 1]));
        assert!(!looks_like_payload(&[b'{'; MIN_DECODED_LEN]));
    }
}
