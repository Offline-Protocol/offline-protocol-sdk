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
