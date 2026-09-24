//! Verifying the identity assertion a peer proves its address with.
//!
//! The layout is `public_key(32) || signature(64) || signed_data`, and it has
//! one home, [`offline_protocol_sealed::identity_assertion`]. This module is
//! the one verifier of it: it parses, checks the signature under the key, and
//! derives the address from the key, in that order, and returns the derived
//! address or an error. It is what the `verify_identity_assertion` FFI
//! function calls, so a Bluetooth LE central on any platform and a peer-stream
//! transport on any host run the same four lines against the same bytes.
//!
//! # What verification proves, and what it does not
//!
//! A returned address means the peer that served these bytes holds, or once
//! held, the private key that address derives from. It does not mean the peer
//! is live or recent: the assertion is static and replayable, as
//! [BLE framing](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/spec/ble-framing.md#the-identity-assertion)
//! states, and a device in range can serve a value it copied. What it cannot
//! do is sign the frames that address must sign, which is where authenticity
//! is established. A caller MUST NOT treat a verified assertion as proof of
//! liveness or of sole ownership.
//!
//! # The fourth step is the caller's
//!
//! The chapter's verification has four steps. The first three are here. The
//! fourth, comparing the derived address to the one the peer claimed over
//! some other channel (the Device id characteristic, a DNS-SD TXT record),
//! belongs to whoever read that channel, because this function never sees it.
//! The comparison is exact, and the announced address is always the derived
//! one, never the claimed string: the two are equal whenever verification
//! succeeds, so the derived value costs nothing, and it means no code path
//! announces a name that arrived unauthenticated.

use offline_protocol_core::Address;
use offline_protocol_sealed::{encode_identity_assertion, parse_identity_assertion};

use crate::error::{MlsError, Result};
use crate::manager::MlsManager;

/// Verifies an identity assertion and returns the address it proves.
///
/// In order, cheap before expensive:
///
/// 1. the bytes are at least 96 long and split into key, signature and data;
/// 2. the key parses as an Ed25519 verifying key;
/// 3. the signature verifies over `signed_data` under it;
/// 4. the address is derived from the key by the SDK's one derivation.
///
/// `signed_data` is not interpreted. Nothing domain-separates it, so any
/// signature the key ever produced over any message verifies here; that is
/// harmless only while the bytes mean nothing, and the chapter says so.
///
/// # Errors
///
/// [`MlsError::Deserialization`] for anything under 96 bytes,
/// [`MlsError::InvalidPublicKey`] for a key that is not on the curve, and
/// [`MlsError::VerificationFailed`] for a signature that does not verify
/// under RFC 8032's strict rules, which also refuse a small-order key and a
/// non-canonical signature.
/// Every case means the peer is not surfaced at all, never surfaced with a
/// caveat: what this returns becomes `Message.recipient` on every outbound
/// frame and the transport-verified peer id the receiving core matches
/// `Message.sender` against.
pub fn verify_identity_assertion(bytes: &[u8]) -> Result<Address> {
    use ed25519_dalek::{Signature, VerifyingKey};

    let parts = parse_identity_assertion(bytes)?;
    let key = VerifyingKey::try_from(parts.public_key)
        .map_err(|e| MlsError::InvalidPublicKey(format!("Invalid Ed25519 public key: {e}")))?;
    let signature = Signature::try_from(parts.signature)
        .map_err(|e| MlsError::VerificationFailed(format!("Invalid signature format: {e}")))?;
    // Strict, not `MlsManager::verify_signature`: the permissive check accepts
    // a small-order key with a signature anyone can write down, over any
    // message. That forges nothing a peer could not get by minting a key, but
    // this is the one verifier for every link that proves an address, and an
    // assertion no private key ever produced has no business verifying.
    key.verify_strict(parts.signed_data, &signature)
        .map_err(|_| {
            MlsError::VerificationFailed(
                "Identity assertion signature does not verify under its public key".to_string(),
            )
        })?;
    MlsManager::derive_address(parts.public_key)
}

impl MlsManager {
    /// Builds this identity's assertion over `signed_data`.
    ///
    /// The result is what a Bluetooth LE peripheral serves from its Identity
    /// characteristic and what a peer-stream transport sends first. The key
    /// never leaves the manager: the caller supplies the bytes to sign and
    /// receives the whole layout, so no bridge assembles it.
    ///
    /// `signed_data` means nothing to a verifier and may be empty. The mobile
    /// peripherals put their mesh advertisement there; a peripheral with
    /// nothing to say should pass nothing.
    ///
    /// # Errors
    ///
    /// Whatever [`MlsManager::sign_data`] returns when there is no identity to
    /// sign with.
    pub fn identity_assertion(&self, signed_data: &[u8]) -> Result<Vec<u8>> {
        let public_key = self.get_identity_public_key()?;
        let signature = self.sign_data(signed_data)?;
        encode_identity_assertion(&public_key, &signature, signed_data).map_err(MlsError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryStorage;
    use ed25519_dalek::{Signer, SigningKey};
    use std::sync::Arc;

    fn assertion_from(seed: u8, signed_data: &[u8]) -> (Vec<u8>, Address) {
        let signing = SigningKey::from_bytes(&[seed; 32]);
        let public = signing.verifying_key().to_bytes();
        let signature = signing.sign(signed_data).to_bytes();
        let address = MlsManager::derive_address(&public).expect("derive");
        (
            encode_identity_assertion(&public, &signature, signed_data).expect("encode"),
            address,
        )
    }

    #[test]
    fn a_valid_assertion_yields_the_address_its_key_derives_to() {
        let (bytes, address) = assertion_from(1, b"mesh advertisement");
        assert_eq!(
            verify_identity_assertion(&bytes).expect("verifies"),
            address
        );
    }

    #[test]
    fn an_empty_signed_data_verifies_at_the_floor() {
        let (bytes, address) = assertion_from(2, b"");
        assert_eq!(bytes.len(), 96);
        assert_eq!(
            verify_identity_assertion(&bytes).expect("verifies"),
            address
        );
    }

    #[test]
    fn a_short_assertion_is_refused_before_any_cryptography() {
        for len in [0usize, 32, 95] {
            let err = verify_identity_assertion(&vec![0u8; len]).unwrap_err();
            assert!(matches!(err, MlsError::Deserialization(_)), "{len}: {err}");
        }
    }

    /// The attack the signature exists for: a public key anyone can copy,
    /// paired with a signature it did not make.
    #[test]
    fn a_signature_by_another_key_is_refused() {
        let (mut bytes, _) = assertion_from(3, b"data");
        let foreign = SigningKey::from_bytes(&[4u8; 32]);
        bytes[32..96].copy_from_slice(&foreign.sign(b"data").to_bytes());
        assert!(matches!(
            verify_identity_assertion(&bytes),
            Err(MlsError::VerificationFailed(_))
        ));
    }

    #[test]
    fn a_signature_over_other_data_is_refused() {
        let (mut bytes, _) = assertion_from(5, b"data");
        // Same key, same signature, one byte of the covered data changed.
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert!(matches!(
            verify_identity_assertion(&bytes),
            Err(MlsError::VerificationFailed(_))
        ));
    }

    #[test]
    fn every_flipped_bit_of_the_signature_is_refused() {
        let (bytes, _) = assertion_from(6, b"");
        for byte in 32..96 {
            let mut tampered = bytes.clone();
            tampered[byte] ^= 0x80;
            assert!(
                verify_identity_assertion(&tampered).is_err(),
                "flipping signature byte {byte} must not verify"
            );
        }
    }

    /// Which step refuses it is the signature library's business (one
    /// version rejects the point at parse, another at verify), so this pins
    /// the refusal and not the variant.
    #[test]
    fn a_key_that_is_not_on_the_curve_is_refused() {
        // All-0xff is not a valid compressed Edwards point.
        let mut bytes = vec![0xffu8; 96];
        bytes[32..].fill(0);
        assert!(verify_identity_assertion(&bytes).is_err());
    }

    /// The forgery the permissive check admits: the identity point as the
    /// key, the identity point as `R` and a zero scalar verify over any
    /// message, with no private key behind them. The permissive
    /// `verify_signature` is asserted to accept it, so this test fails if the
    /// verifier is ever routed back through it.
    #[test]
    fn a_small_order_key_with_a_trivial_signature_is_refused() {
        let mut identity_point = [0u8; 32];
        identity_point[0] = 1;
        let mut signature = [0u8; 64];
        signature[..32].copy_from_slice(&identity_point);
        let data = b"any message at all";

        assert!(
            MlsManager::verify_signature(&identity_point, data, &signature).expect("parses"),
            "the permissive check accepts this; if it stops, this test no longer \
             proves the verifier is strict"
        );
        let bytes = encode_identity_assertion(&identity_point, &signature, data).expect("encode");
        assert!(matches!(
            verify_identity_assertion(&bytes),
            Err(MlsError::VerificationFailed(_))
        ));
    }

    /// The instance side: what a manager builds is what the verifier
    /// accepts, and it names the manager's own address.
    #[test]
    fn a_manager_builds_an_assertion_the_verifier_accepts() {
        let storage: Arc<dyn crate::MlsStorage> = Arc::new(InMemoryStorage::new());
        let manager = MlsManager::new("device", storage).expect("manager");
        let own = manager.get_identity_public_key().expect("key");
        let own_address = MlsManager::derive_address(&own).expect("derive");

        for signed_data in [&b""[..], b"advertisement"] {
            let bytes = manager.identity_assertion(signed_data).expect("assertion");
            assert_eq!(bytes.len(), 96 + signed_data.len());
            assert_eq!(&bytes[96..], signed_data);
            assert_eq!(
                verify_identity_assertion(&bytes).expect("verifies"),
                own_address
            );
        }
    }

    /// The published vectors, end to end: the signatures in the file were
    /// pasted from RFC 8032, so a verifier that accepts them and returns the
    /// pinned address has its whole pipeline checked against a source outside
    /// this repository.
    #[test]
    fn the_conformance_assertions_verify_to_their_pinned_addresses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../offline-protocol-sealed/tests/data/identity-assertion-v1.vectors.json");
        // Absent in a published .crate archive, present in the repo.
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("sealed vector file not present, skipping");
            return;
        };
        let v: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        let unhex = |s: &str| -> Vec<u8> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
                .collect()
        };
        let cases = v["assertions"].as_array().expect("assertions");
        assert_eq!(cases.len(), 3);
        for case in cases {
            let name = case["name"].as_str().expect("name");
            let bytes = unhex(case["assertion_hex"].as_str().expect("hex"));
            let got = verify_identity_assertion(&bytes).unwrap_or_else(|e| panic!("[{name}]: {e}"));
            assert_eq!(
                got.to_string(),
                case["address"].as_str().expect("address"),
                "[{name}]"
            );
        }
        for case in v["refusals"].as_array().expect("refusals") {
            let name = case["name"].as_str().expect("name");
            let bytes = unhex(case["assertion_hex"].as_str().expect("hex"));
            assert!(
                verify_identity_assertion(&bytes).is_err(),
                "[{name}] must be refused"
            );
        }
    }
}
