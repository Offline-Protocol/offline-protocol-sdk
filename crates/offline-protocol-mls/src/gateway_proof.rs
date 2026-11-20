//! Verifying the address declaration a device attaches to a gateway with.
//!
//! The device half lives in the engine: it signs
//! [`offline_protocol_sealed::gateway_address_proof_payload`] over its own
//! address and the gateway's challenge. This module is the gateway half, and
//! the one verifier of that proof. It exists so that a gateway daemon written
//! in any language checks the proof by calling the SDK rather than by building
//! the 103-byte payload itself: the layout has one home, in the sealed crate,
//! and a second builder is exactly the copy that drifts (a length written
//! little-endian, a challenge prefixed as if it were a second field) while its
//! own round-trip test keeps passing.
//!
//! # What this checks, and what it leaves to the gateway
//!
//! The gateway contract lists three checks a gateway makes before it answers
//! `AddressDeclared`. This function makes the first two:
//!
//! 1. the signature verifies over the canonical bytes under `public_key`;
//! 2. `address` is the address `public_key` derives to.
//!
//! The third, that `challenge` is the one this gateway minted for this
//! connection and has not been accepted before, is connection state, and only
//! the gateway holds it. A gateway that skips it turns one captured
//! declaration into a reusable credential, whatever this function returns.

use offline_protocol_core::Address;
use offline_protocol_sealed::gateway_address_proof_payload;

use crate::error::{MlsError, Result};
use crate::manager::MlsManager;

/// Verifies a gateway address declaration and returns the address it binds.
///
/// In order, cheap before expensive:
///
/// 1. the key parses as an Ed25519 verifying key and the signature is 64
///    bytes;
/// 2. the signature verifies, under RFC 8032's strict rules, over
///    `gateway_address_proof_payload(address, challenge)`;
/// 3. the address `public_key` derives to equals `address` exactly.
///
/// The signature is checked over the **claimed** address, because that is
/// what the device signed, and the derivation is checked after it. Both are
/// needed: without the derivation check a signature made under any key the
/// sender holds attaches the session to any address it cares to name.
///
/// The challenge is passed to the payload builder unexamined. Its length is a
/// wire rule the gateway enforces when it mints one; the verifier must build
/// the same bytes the device signed for whatever challenge was actually sent.
///
/// # Errors
///
/// [`MlsError::InvalidPublicKey`] for a key that is not 32 bytes or not on the
/// curve, and [`MlsError::VerificationFailed`] for a signature that is not 64
/// bytes, does not verify, or verifies under a key that derives to another
/// address. Every case means the gateway answers `AddressError` and binds
/// nothing.
pub fn verify_gateway_address_proof(
    address: &str,
    public_key: &[u8],
    signature: &[u8],
    challenge: &[u8],
) -> Result<Address> {
    use ed25519_dalek::{Signature, VerifyingKey};

    let key = VerifyingKey::try_from(public_key)
        .map_err(|e| MlsError::InvalidPublicKey(format!("Invalid Ed25519 public key: {e}")))?;
    let signature = Signature::try_from(signature)
        .map_err(|e| MlsError::VerificationFailed(format!("Invalid signature format: {e}")))?;
    let payload = gateway_address_proof_payload(address, challenge)?;
    // Strict, for the reason `verify_identity_assertion` gives: the permissive
    // check accepts a small-order key with a signature anyone can write down,
    // and a declaration no private key ever produced must not bind a session.
    key.verify_strict(&payload, &signature).map_err(|_| {
        MlsError::VerificationFailed(
            "Gateway address declaration does not verify under its public key".to_string(),
        )
    })?;
    let derived = MlsManager::derive_address(public_key)?;
    if derived.to_string() != address {
        return Err(MlsError::VerificationFailed(format!(
            "Gateway address declaration names {address}, but its key derives to {derived}"
        )));
    }
    Ok(derived)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// RFC 8032 section 7.1, test vector 1, secret half. Its public half
    /// derives to the address the first conformance proof names.
    const RFC8032_TV1_SK: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    /// RFC 8032 section 7.1, test vector 2, secret half. Its public half
    /// derives to the address the second and third proofs name.
    const RFC8032_TV2_SK: &str = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    fn signing_key(hex: &str) -> SigningKey {
        let bytes: [u8; 32] = unhex(hex).try_into().expect("32 bytes");
        SigningKey::from_bytes(&bytes)
    }

    fn address_of(key: &SigningKey) -> String {
        MlsManager::derive_address(&key.verifying_key().to_bytes())
            .expect("derive")
            .to_string()
    }

    /// What a device sends: its own address, signed over this challenge.
    fn declare(key: &SigningKey, challenge: &[u8]) -> (String, Vec<u8>, Vec<u8>) {
        let address = address_of(key);
        let payload = gateway_address_proof_payload(&address, challenge).expect("payload");
        let signature = key.sign(&payload).to_bytes().to_vec();
        (address, key.verifying_key().to_bytes().to_vec(), signature)
    }

    #[test]
    fn an_honest_declaration_verifies_to_its_address() {
        let key = signing_key(RFC8032_TV1_SK);
        let challenge = [7u8; 32];
        let (address, public_key, signature) = declare(&key, &challenge);
        let bound = verify_gateway_address_proof(&address, &public_key, &signature, &challenge)
            .expect("verifies");
        assert_eq!(bound.to_string(), address);
    }

    /// The check the contract calls the one that makes the binding mean
    /// anything: a valid signature under the sender's own key, over somebody
    /// else's address.
    #[test]
    fn a_signature_over_another_address_is_refused() {
        let attacker = signing_key(RFC8032_TV1_SK);
        let victim = address_of(&signing_key(RFC8032_TV2_SK));
        let challenge = [9u8; 32];
        let payload = gateway_address_proof_payload(&victim, &challenge).expect("payload");
        let signature = attacker.sign(&payload).to_bytes();
        let err = verify_gateway_address_proof(
            &victim,
            &attacker.verifying_key().to_bytes(),
            &signature,
            &challenge,
        )
        .unwrap_err();
        assert!(matches!(err, MlsError::VerificationFailed(_)), "{err}");
    }

    /// A proof made for one connection does not verify on another. The
    /// replay check itself is the gateway's; this is what makes it possible.
    #[test]
    fn a_proof_over_another_challenge_is_refused() {
        let key = signing_key(RFC8032_TV1_SK);
        let (address, public_key, signature) = declare(&key, &[1u8; 32]);
        assert!(matches!(
            verify_gateway_address_proof(&address, &public_key, &signature, &[2u8; 32]),
            Err(MlsError::VerificationFailed(_))
        ));
    }

    /// The relay's declaration has this payload's layout exactly, under
    /// another domain. A proof harvested there must not attach here.
    #[test]
    fn a_relay_declaration_is_refused() {
        let key = signing_key(RFC8032_TV1_SK);
        let address = address_of(&key);
        let challenge = [3u8; 32];
        // The relay's domain, spelled here the way the sealed crate's own
        // non-prefixing test spells it; the layout is the shared builder's.
        let mut relay_payload = offline_protocol_sealed::canonical_payload(
            b"offline-relay-addr-v1",
            &[address.as_bytes()],
        )
        .expect("relay payload");
        relay_payload.extend_from_slice(&challenge);
        let signature = key.sign(&relay_payload).to_bytes();
        assert!(matches!(
            verify_gateway_address_proof(
                &address,
                &key.verifying_key().to_bytes(),
                &signature,
                &challenge
            ),
            Err(MlsError::VerificationFailed(_))
        ));
    }

    #[test]
    fn every_flipped_bit_of_the_signature_is_refused() {
        let key = signing_key(RFC8032_TV2_SK);
        let challenge = [0u8; 32];
        let (address, public_key, signature) = declare(&key, &challenge);
        for byte in 0..signature.len() {
            let mut tampered = signature.clone();
            tampered[byte] ^= 0x80;
            assert!(
                verify_gateway_address_proof(&address, &public_key, &tampered, &challenge).is_err(),
                "flipping signature byte {byte} must not verify"
            );
        }
    }

    #[test]
    fn malformed_keys_and_signatures_are_refused_before_any_cryptography() {
        let key = signing_key(RFC8032_TV1_SK);
        let challenge = [5u8; 32];
        let (address, public_key, signature) = declare(&key, &challenge);
        assert!(matches!(
            verify_gateway_address_proof(&address, &public_key[..31], &signature, &challenge),
            Err(MlsError::InvalidPublicKey(_))
        ));
        assert!(matches!(
            verify_gateway_address_proof(&address, &public_key, &signature[..63], &challenge),
            Err(MlsError::VerificationFailed(_))
        ));
    }

    /// The conformance vectors, end to end. Their payloads were computed from
    /// the chapter's construction and their addresses derive from RFC 8032
    /// keys, so signing each pinned payload with the matching RFC 8032 secret
    /// and verifying it here checks this verifier against bytes it did not
    /// build: a verifier whose payload drifted from the vector refuses them.
    #[test]
    fn the_conformance_payloads_verify_when_signed_by_their_rfc8032_keys() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../offline-protocol-sealed/tests/data/gateway-address-proof-v1.vectors.json");
        // Absent in a published .crate archive, present in the repo.
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("sealed vector file not present, skipping");
            return;
        };
        let v: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        let keys = [signing_key(RFC8032_TV1_SK), signing_key(RFC8032_TV2_SK)];
        let proofs = v["proofs"].as_array().expect("proofs");
        assert_eq!(proofs.len(), 3);
        for proof in proofs {
            let name = proof["name"].as_str().expect("name");
            let address = proof["address"].as_str().expect("address");
            let challenge = unhex(proof["challenge_hex"].as_str().expect("challenge"));
            let payload = unhex(proof["payload_hex"].as_str().expect("payload"));
            let key = keys
                .iter()
                .find(|k| address_of(k) == address)
                .unwrap_or_else(|| panic!("[{name}]: no RFC 8032 key derives to {address}"));
            let signature = key.sign(&payload).to_bytes();
            let bound = verify_gateway_address_proof(
                address,
                &key.verifying_key().to_bytes(),
                &signature,
                &challenge,
            )
            .unwrap_or_else(|e| panic!("[{name}]: {e}"));
            assert_eq!(bound.to_string(), address, "[{name}]");
        }
    }
}
