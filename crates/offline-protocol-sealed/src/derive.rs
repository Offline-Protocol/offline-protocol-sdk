//! The SDK's one address derivation.
//!
//! An address is the truncated hash of an identity key, so anything carrying
//! both is self-certifying: a recipient re-derives and compares rather than
//! consulting a directory. Every trust gate in the protocol is that
//! comparison, which is why there is exactly one implementation of it and why
//! it lives where both a phone and a leaf node can reach it.

use crate::{Result, SealedError};
use alloc::format;
use offline_protocol_core::Address;

/// Length of an Ed25519 public key, in bytes.
pub const ED25519_PUBLIC_KEY_LEN: usize = 32;

/// Derives the canonical self-certifying address of an Ed25519 identity
/// key: `off1…`, the bech32m encoding of
/// `0x01 ‖ SHA-256(public_key)[..20]`.
///
/// This is the only address derivation in the SDK. The MLS crate exposes it as
/// `MlsManager::derive_address`, bridges and apps reach that through the
/// `derive_address` FFI function, and a leaf node calls this directly, so that
/// every platform agrees byte for byte.
///
/// # Arguments
///
/// * `public_key` - The Ed25519 public key bytes (exactly 32)
///
/// # Errors
///
/// Returns [`SealedError::InvalidPublicKey`] if `public_key` is not 32 bytes.
/// The length is part of the format contract: hashing a differently-sized
/// input would yield a different address for the same identity. The bytes
/// are deliberately *not* checked against the curve. An address is defined
/// over key bytes, and what proves ownership is the signature verification
/// that accompanies it (`MlsManager::verify_signature`), so binding the
/// address format to a signature library's parsing strictness would only risk
/// the derivation drifting between versions.
pub fn derive_address(public_key: &[u8]) -> Result<Address> {
    use sha2::{Digest, Sha256};

    if public_key.len() != ED25519_PUBLIC_KEY_LEN {
        return Err(SealedError::InvalidPublicKey(format!(
            "Ed25519 public key must be {} bytes, got {}",
            ED25519_PUBLIC_KEY_LEN,
            public_key.len()
        )));
    }

    let hash = Sha256::digest(public_key);
    let mut truncated = [0u8; Address::HASH_LEN];
    truncated.copy_from_slice(&hash[..Address::HASH_LEN]);
    Ok(Address::from_hash_bytes(truncated))
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use alloc::string::ToString;

    /// RFC 8032 test vector 1's Ed25519 public key.
    const RFC8032_TV1_PK: [u8; 32] = [
        0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07,
        0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07,
        0x51, 0x1a,
    ];

    /// The address that key derives to.
    ///
    /// Re-derive it from the reference implementation if it ever needs to
    /// change, never edit it to match new code output.
    const RFC8032_TV1_ADDRESS: &str = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn";

    #[test]
    fn derive_address_matches_the_pinned_vector() {
        let address = derive_address(&RFC8032_TV1_PK).expect("32-byte key derives");
        assert_eq!(address.to_string(), RFC8032_TV1_ADDRESS);
        assert_eq!(
            RFC8032_TV1_ADDRESS.parse::<Address>().expect("parses"),
            address,
            "the pinned string must round-trip back to the same address"
        );
    }

    #[test]
    fn derive_address_rejects_wrong_key_lengths() {
        for len in [0usize, 31, 33, 64] {
            let err = derive_address(&vec![0u8; len]).unwrap_err();
            assert!(
                matches!(err, SealedError::InvalidPublicKey(_)),
                "{len}-byte key must be refused, got: {err}"
            );
        }
    }
}

/// The frozen conformance vectors for address derivation and the session slot.
///
/// The chapter these pin is `docs/spec/identity.md`. They were computed by
/// `tools/spec-vectors/generate.py` from the rules in that chapter, not by
/// running [`derive_address`]: a vector produced by the function under test
/// agrees with any derivation it happens to compute, including a wrong one.
///
/// The public keys are the public halves of RFC 8032 section 7.1 test vectors 1
/// to 3, so both halves of each case can be checked against a published source
/// rather than against this repository.
#[cfg(all(test, feature = "std"))]
mod spec_vectors {
    use super::*;
    use crate::envelope::GroupId;

    const VECTORS: &str = include_str!("../tests/data/derive-address-v1.vectors.json");

    fn vectors() -> serde_json::Value {
        serde_json::from_str(VECTORS).expect("the vector file is JSON")
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex digit"))
            .collect()
    }

    /// Asserts the file still carries what it carried, before anything iterates
    /// it: a loop over an array a bad merge emptied passes by not running.
    #[test]
    fn the_vector_file_is_the_size_it_was() {
        let v = vectors();
        assert_eq!(v["derive"].as_array().expect("derive").len(), 5);
        assert_eq!(v["sessions"].as_array().expect("sessions").len(), 3);
        assert!(
            v["sessions"]
                .as_array()
                .expect("sessions")
                .iter()
                .any(|s| s["orders_disagree"] == true),
            "at least one session pair must be one where hash order and string \
             order disagree, or the file cannot catch the ordering bug it exists \
             to catch"
        );
    }

    #[test]
    fn every_public_key_derives_its_address() {
        for case in vectors()["derive"].as_array().expect("derive") {
            let name = case["name"].as_str().expect("a name");
            let pk = unhex(case["public_key_hex"].as_str().expect("a public key"));
            let got = derive_address(&pk).unwrap_or_else(|e| panic!("[{name}]: {e}"));
            assert_eq!(
                got.to_string(),
                case["address"].as_str().expect("an address"),
                "[{name}] derived a different address than the chapter specifies"
            );
        }
    }

    /// The session slot both parties compute without exchanging it.
    ///
    /// The ordering is by hash bytes, not by the rendered string. Where the two
    /// disagree an implementation that sorts the strings names a slot the peer
    /// never looks at, and neither side sees an error: each simply never hears
    /// from the other.
    #[test]
    fn every_pair_derives_its_session_slot() {
        for case in vectors()["sessions"].as_array().expect("sessions") {
            let a = case["a"].as_str().expect("a");
            let b = case["b"].as_str().expect("b");
            let want = case["session_id"].as_str().expect("a slot");

            let got = GroupId::for_session(a, b).expect("a slot");
            assert_eq!(got.as_str(), want, "session slot for {a} and {b}");

            let swapped = GroupId::for_session(b, a).expect("a slot");
            assert_eq!(
                swapped.as_str(),
                want,
                "the slot is not symmetric, so the two parties would name \
                 different sessions"
            );

            if case["orders_disagree"] == true {
                assert_ne!(
                    want,
                    case["session_id_if_ordered_by_string"]
                        .as_str()
                        .expect("the string-ordered slot"),
                    "this pair no longer distinguishes the two orderings"
                );
            }
        }
    }

    /// The chapter, or `None` where the repo tree is absent: `cargo package`
    /// carries `tests/` and the vector file but cannot carry `docs/`.
    fn chapter() -> Option<String> {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/spec/identity.md");
        std::fs::read_to_string(&path).ok().or_else(|| {
            eprintln!("spec tree not present, skipping the identity chapter drift checks");
            None
        })
    }

    #[test]
    fn the_chapter_states_the_derivation_the_code_performs() {
        let Some(text) = chapter() else { return };

        assert!(
            text.contains("SHA-256(ed25519_public_key)[0..20]"),
            "the chapter no longer states the derivation this function computes"
        );
        assert!(
            text.contains("session:\" || lower || \":\" || higher"),
            "the chapter no longer states the session slot construction"
        );
        assert!(
            text.contains(&format!("{ED25519_PUBLIC_KEY_LEN}")),
            "the chapter does not state the {ED25519_PUBLIC_KEY_LEN}-byte key length"
        );
    }
}
