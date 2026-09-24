//! The identity assertion: the one byte layout by which a peer proves, over a
//! link the platform established, that it holds the key its address derives
//! from.
//!
//! A Bluetooth LE peripheral serves it from the Identity characteristic
//! ([BLE framing](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/spec/ble-framing.md#the-identity-assertion)),
//! and a stream transport sends it as the first frame in each direction. The
//! layout is fixed:
//!
//! | Field | Size | Value |
//! |-------|------|-------|
//! | `public_key` | 32 | The peer's Ed25519 identity public key |
//! | `signature` | 64 | Ed25519 signature by that key over `signed_data` |
//! | `signed_data` | remainder | The bytes that signature covers, possibly empty |
//!
//! A reader refuses anything shorter than 96 bytes before any cryptography
//! runs. `signed_data` carries no meaning at this layer: it is a message to
//! sign, and nothing domain-separates it, so giving it meaning is a wire
//! break. The chapter states both rules and the reasons.
//!
//! # Why the codec is here and the verifier is not
//!
//! This module splits and joins bytes. It does not check a signature or
//! derive an address, because the sealed layer links no signature library
//! (ADR 0022: SHA-256 is the only cryptography here). Verification, which is
//! parse, verify the signature under the key, then derive the address from
//! the key, lives where Ed25519 does, in `offline-protocol-mls`, and reaches
//! bridges and applications as the `verify_identity_assertion` FFI function.
//!
//! What the codec being here buys is one layout for every implementation.
//! Before it existed the split was written three times, in Swift, in Kotlin
//! and in Python, and the Python copy was not a split at all but a JSON
//! document, which no phone could verify. A leaf node that answers a
//! peer-stream preamble needs the same layout, and it cannot link the engine.

use crate::{Result, SealedError};
use alloc::format;
use alloc::vec::Vec;

use crate::derive::ED25519_PUBLIC_KEY_LEN;

/// Length of an Ed25519 signature, in bytes.
pub const ED25519_SIGNATURE_LEN: usize = 64;

/// The shortest assertion a reader accepts: a key and a signature with an
/// empty `signed_data`.
pub const IDENTITY_ASSERTION_MIN_LEN: usize = ED25519_PUBLIC_KEY_LEN + ED25519_SIGNATURE_LEN;

/// The three fields of a parsed assertion, borrowed from the bytes they were
/// parsed out of.
///
/// Holding one of these proves nothing: the fields have the right sizes and
/// nothing more. A verifier owes the signature check and the derivation before
/// it announces the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityAssertion<'a> {
    /// The Ed25519 identity public key, exactly 32 bytes.
    pub public_key: &'a [u8],
    /// The Ed25519 signature over `signed_data`, exactly 64 bytes.
    pub signature: &'a [u8],
    /// The bytes the signature covers. May be empty, and means nothing here.
    pub signed_data: &'a [u8],
}

/// Joins the three fields into the wire layout.
///
/// # Errors
///
/// Returns [`SealedError::InvalidPublicKey`] if `public_key` is not 32 bytes
/// and [`SealedError::Serialization`] if `signature` is not 64. Both lengths
/// are the format: a reader locates the fields by offset, so a short key would
/// be parsed as a key whose first bytes of signature are its tail.
pub fn encode_identity_assertion(
    public_key: &[u8],
    signature: &[u8],
    signed_data: &[u8],
) -> Result<Vec<u8>> {
    if public_key.len() != ED25519_PUBLIC_KEY_LEN {
        return Err(SealedError::InvalidPublicKey(format!(
            "Ed25519 public key must be {} bytes, got {}",
            ED25519_PUBLIC_KEY_LEN,
            public_key.len()
        )));
    }
    if signature.len() != ED25519_SIGNATURE_LEN {
        return Err(SealedError::Serialization(format!(
            "Identity assertion signature must be {} bytes, got {}",
            ED25519_SIGNATURE_LEN,
            signature.len()
        )));
    }

    let mut out = Vec::with_capacity(IDENTITY_ASSERTION_MIN_LEN + signed_data.len());
    out.extend_from_slice(public_key);
    out.extend_from_slice(signature);
    out.extend_from_slice(signed_data);
    Ok(out)
}

/// Splits the wire layout into its three fields.
///
/// # Errors
///
/// Returns [`SealedError::Deserialization`] for anything shorter than
/// [`IDENTITY_ASSERTION_MIN_LEN`]. That is the only refusal this layer makes:
/// a 96-byte value whose key is not on the curve or whose signature does not
/// verify parses fine here and fails at the verifier, which is where the
/// chapter puts those checks.
pub fn parse_identity_assertion(bytes: &[u8]) -> Result<IdentityAssertion<'_>> {
    if bytes.len() < IDENTITY_ASSERTION_MIN_LEN {
        return Err(SealedError::Deserialization(format!(
            "Identity assertion is {} bytes, minimum is {}",
            bytes.len(),
            IDENTITY_ASSERTION_MIN_LEN
        )));
    }
    let (public_key, rest) = bytes.split_at(ED25519_PUBLIC_KEY_LEN);
    let (signature, signed_data) = rest.split_at(ED25519_SIGNATURE_LEN);
    Ok(IdentityAssertion {
        public_key,
        signature,
        signed_data,
    })
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn encode_then_parse_returns_the_fields() {
        let key = [1u8; 32];
        let sig = [2u8; 64];
        let data = b"whatever follows";
        let bytes = encode_identity_assertion(&key, &sig, data).expect("encodes");
        assert_eq!(bytes.len(), 96 + data.len());
        let parsed = parse_identity_assertion(&bytes).expect("parses");
        assert_eq!(parsed.public_key, &key);
        assert_eq!(parsed.signature, &sig);
        assert_eq!(parsed.signed_data, data);
    }

    #[test]
    fn an_empty_signed_data_is_the_floor_and_is_accepted() {
        let bytes = encode_identity_assertion(&[0u8; 32], &[0u8; 64], &[]).expect("encodes");
        assert_eq!(bytes.len(), IDENTITY_ASSERTION_MIN_LEN);
        let parsed = parse_identity_assertion(&bytes).expect("96 bytes is a whole assertion");
        assert!(parsed.signed_data.is_empty());
    }

    #[test]
    fn encode_refuses_wrong_field_sizes() {
        for key_len in [0usize, 31, 33] {
            let err = encode_identity_assertion(&vec![0u8; key_len], &[0u8; 64], &[]).unwrap_err();
            assert!(
                matches!(err, SealedError::InvalidPublicKey(_)),
                "{key_len}: {err}"
            );
        }
        for sig_len in [0usize, 63, 65] {
            let err = encode_identity_assertion(&[0u8; 32], &vec![0u8; sig_len], &[]).unwrap_err();
            assert!(
                matches!(err, SealedError::Serialization(_)),
                "{sig_len}: {err}"
            );
        }
    }

    #[test]
    fn parse_refuses_everything_under_the_floor() {
        for len in [0usize, 1, 32, 64, 95] {
            let err = parse_identity_assertion(&vec![0u8; len]).unwrap_err();
            assert!(
                matches!(err, SealedError::Deserialization(_)),
                "{len}: {err}"
            );
        }
    }
}

/// The frozen conformance vectors for the assertion layout.
///
/// The chapter these pin is `docs/spec/ble-framing.md`. Each assertion's key,
/// signature and message are the published values of RFC 8032 section 7.1 test
/// vectors 1 to 3, joined by `tools/spec-vectors/generate.py` from the layout
/// the chapter states rather than by running this codec, so agreement is a
/// cross-check and not a restatement.
#[cfg(all(test, feature = "std"))]
mod spec_vectors {
    use super::*;

    const VECTORS: &str = include_str!("../tests/data/identity-assertion-v1.vectors.json");

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
        assert_eq!(v["assertions"].as_array().expect("assertions").len(), 3);
        assert_eq!(v["refusals"].as_array().expect("refusals").len(), 3);
        assert_eq!(v["minimum_length"], IDENTITY_ASSERTION_MIN_LEN);
        assert!(
            v["assertions"]
                .as_array()
                .expect("assertions")
                .iter()
                .any(|a| a["signed_data_hex"] == ""),
            "one assertion must sit exactly on the floor, or the file cannot catch \
             a reader that refuses 96 bytes"
        );
    }

    #[test]
    fn every_assertion_parses_into_the_fields_it_was_built_from() {
        for case in vectors()["assertions"].as_array().expect("assertions") {
            let name = case["name"].as_str().expect("a name");
            let bytes = unhex(case["assertion_hex"].as_str().expect("hex"));
            let parsed =
                parse_identity_assertion(&bytes).unwrap_or_else(|e| panic!("[{name}]: {e}"));
            assert_eq!(
                parsed.public_key,
                unhex(case["public_key_hex"].as_str().expect("key")),
                "[{name}] public key"
            );
            assert_eq!(
                parsed.signature,
                unhex(case["signature_hex"].as_str().expect("signature")),
                "[{name}] signature"
            );
            assert_eq!(
                parsed.signed_data,
                unhex(case["signed_data_hex"].as_str().expect("signed data")),
                "[{name}] signed data"
            );
            // The address the key derives to is in the file so a verifier in
            // any language can pin its whole pipeline on published values.
            assert_eq!(
                crate::derive_address(parsed.public_key)
                    .expect("32-byte key")
                    .to_string(),
                case["address"].as_str().expect("address"),
                "[{name}] the address the chapter's derivation gives this key"
            );
        }
    }

    #[test]
    fn every_assertion_re_encodes_to_the_same_bytes() {
        for case in vectors()["assertions"].as_array().expect("assertions") {
            let name = case["name"].as_str().expect("a name");
            let want = unhex(case["assertion_hex"].as_str().expect("hex"));
            let got = encode_identity_assertion(
                &unhex(case["public_key_hex"].as_str().expect("key")),
                &unhex(case["signature_hex"].as_str().expect("signature")),
                &unhex(case["signed_data_hex"].as_str().expect("signed data")),
            )
            .unwrap_or_else(|e| panic!("[{name}]: {e}"));
            assert_eq!(got, want, "[{name}] encoded bytes");
        }
    }

    #[test]
    fn every_refusal_is_refused() {
        for case in vectors()["refusals"].as_array().expect("refusals") {
            let name = case["name"].as_str().expect("a name");
            let bytes = unhex(case["assertion_hex"].as_str().expect("hex"));
            assert!(
                parse_identity_assertion(&bytes).is_err(),
                "[{name}] {} bytes must be refused",
                bytes.len()
            );
        }
    }

    /// The chapter, or `None` where the repo tree is absent: `cargo package`
    /// carries `tests/` and the vector file but cannot carry `docs/`.
    fn chapter() -> Option<String> {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/spec/ble-framing.md");
        std::fs::read_to_string(&path).ok().or_else(|| {
            eprintln!("spec tree not present, skipping the BLE framing chapter drift checks");
            None
        })
    }

    #[test]
    fn the_chapter_states_the_layout_the_code_performs() {
        let Some(text) = chapter() else { return };

        assert!(
            text.contains("| `public_key` | 32 |") && text.contains("| `signature` | 64 |"),
            "the chapter no longer states the two fixed field sizes"
        );
        assert!(
            text.contains(&format!("shorter than {IDENTITY_ASSERTION_MIN_LEN} bytes")),
            "the chapter no longer states the {IDENTITY_ASSERTION_MIN_LEN}-byte floor"
        );
        assert!(
            text.contains("identity-assertion-v1.vectors.json"),
            "the chapter no longer names the vector file that pins it"
        );
    }
}
