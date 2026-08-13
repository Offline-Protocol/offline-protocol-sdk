//! Self-certifying protocol addresses.
//!
//! An address is a bech32m encoding of `0x01 ‖ SHA-256(ed25519_pub)[..20]`,
//! rendered as `off1…` (44 characters). Because the payload is a hash of the
//! identity key, an address authenticates itself: a peer claiming an address
//! can be checked by re-deriving it from the public key it presents, with no
//! trust-on-first-use pin store in between.
//!
//! This module never touches key material — hashing happens upstream (see
//! `MlsManager::derive_address`); [`Address`] is constructed from the already
//! truncated hash.
//!
//! # Exactly one string form
//!
//! Every payload has exactly one accepted spelling. [`Address::from_str`]
//! rejects uppercase input, the non-`m` bech32 checksum, wrong HRP, wrong
//! version byte, wrong payload length, and — critically — encodings that
//! decode to the right payload but are not the canonical spelling of it.
//! The last class is not hypothetical: the `bech32` crate's decoder discards
//! the trailing sub-byte bits without checking they are zero, and its
//! fes-to-bytes length rule (`n * 5 / 8`) maps both 34- and 35-character data
//! parts onto a 21-byte payload. Both shapes are checksum-valid and both are
//! caught here by re-encoding and requiring byte-for-byte equality with the
//! input. Briar's "spare bit" address bug is the reference failure for what
//! happens when this is skipped.

use std::fmt;
use std::str::FromStr;

use bech32::primitives::decode::CheckedHrpstring;
use bech32::{Bech32m, Hrp};
use serde::{Deserialize, Serialize};

/// Why an address string was rejected by [`Address::from_str`].
// Adding a variant to a public error enum is a breaking change without
// this attribute; downstream crates must carry a wildcard arm.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    /// The string contains ASCII uppercase.
    ///
    /// BIP-173 permits an all-uppercase spelling and the `bech32` crate
    /// accepts it; the protocol admits the lowercase form only.
    #[error("address is not lowercase (the canonical form is lowercase-only)")]
    NotLowercase,
    /// The string is not structurally valid bech32m: bad charset, missing or
    /// misplaced separator, mixed case, or a checksum that is absent, corrupt,
    /// or computed with the original bech32 constant instead of bech32m.
    #[error("address is not valid bech32m: {reason}")]
    InvalidBech32m {
        /// Rendering of the underlying `bech32` decode error.
        reason: String,
    },
    /// Valid bech32m, but the human-readable part is not [`Address::HRP_STR`].
    #[error("address has human-readable part '{found}', expected '{expected}'")]
    HrpMismatch {
        /// Human-readable part found in the input.
        found: String,
        /// The required human-readable part.
        expected: &'static str,
    },
    /// The decoded payload is not [`Address::PAYLOAD_LEN`] bytes.
    #[error("address payload is {len} bytes, expected {expected}")]
    PayloadLength {
        /// Decoded payload length.
        len: usize,
        /// Required payload length.
        expected: usize,
    },
    /// The payload does not start with a supported version byte.
    #[error("address version byte is {found:#04x}, expected {expected:#04x}")]
    VersionMismatch {
        /// Version byte found in the payload.
        found: u8,
        /// The only version byte this build understands.
        expected: u8,
    },
    /// The string decodes to a well-formed payload but is not that payload's
    /// canonical spelling — non-zero padding bits, or a data part padded out
    /// with extra characters that truncate to the same bytes.
    ///
    /// Accepting these would give one identity several valid addresses, so
    /// they are rejected rather than normalized.
    #[error("address is a non-canonical encoding of its payload")]
    NonCanonical,
}

/// A self-certifying protocol address: `off1…`.
///
/// # Ordering
///
/// [`Ord`] compares the underlying hash bytes, **not** the rendered string.
/// The two orders differ: the bech32 charset (`qpzry9x8gf2tvdw0s3jn54khce6mua7l`)
/// is not monotonic in ASCII — value 4 renders as `y` (0x79) and value 5 as
/// `9` (0x39) — and a string comparison would also weigh the checksum
/// characters. Protocol tiebreakers (both-create session ownership, leave
/// election, admin auto-promotion, fork leader) must therefore compare
/// `Address` values, never their `Display` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address {
    hash: [u8; Address::HASH_LEN],
}

impl Address {
    /// Human-readable part carried by every address.
    pub const HRP_STR: &'static str = "off";

    /// The only address version this build understands.
    pub const VERSION: u8 = 0x01;

    /// Length of the truncated identity-key hash, in bytes.
    ///
    /// 160 bits, which is a **second-preimage** margin of ~2^160 and a
    /// **collision** margin of ~2^80 — the birthday bound on the same
    /// truncation. Both are stated here because they buy different things and
    /// only the first is what the impersonation claim rests on:
    ///
    /// - ~2^160 is the cost of aiming at an address that already exists:
    ///   producing a key that derives to *someone else's* address. That is the
    ///   attack the sender-derivation gate replaced the trust-pin store to
    ///   stop, and it is the figure the guides and changelog quote.
    /// - ~2^80 is the cost of producing *two* identity keys that share one
    ///   address, neither fixed in advance. It impersonates nobody, but it does
    ///   let a single entity hold two signing keys that are indistinguishable
    ///   at the address layer — enough to equivocate, and enough to defeat the
    ///   "one identity cannot hold two leaves" property that the MLS leaf
    ///   binding otherwise inherits from MLS signature-key uniqueness.
    ///
    /// ~2^80 is below the ~2^128 a greenfield design would target, and is a
    /// deliberate trade rather than an oversight: every mesh frame carries a
    /// sender and a recipient address, and BLE budget is the binding
    /// constraint. Widening the hash changes the wire, so it is a [`VERSION`]
    /// bump and a migration, not a patch.
    ///
    /// [`VERSION`]: Address::VERSION
    pub const HASH_LEN: usize = 20;

    /// Length of the encoded payload: version byte followed by the hash.
    pub const PAYLOAD_LEN: usize = 1 + Self::HASH_LEN;

    /// Length of the canonical string form.
    ///
    /// `off` (3) + separator (1) + data (34) + checksum (6). The data part is
    /// 34 characters because 21 bytes is 168 bits, which needs 34 five-bit
    /// groups and leaves 2 padding bits.
    pub const ENCODED_LEN: usize = 44;

    /// `Hrp` form of [`Self::HRP_STR`].
    ///
    /// `parse_unchecked` is the only `const` constructor and performs no
    /// validation; `hrp_const_matches_parsed` pins it against the checked
    /// parser.
    const HRP: Hrp = Hrp::parse_unchecked(Self::HRP_STR);

    /// Builds an address from a truncated identity-key hash.
    ///
    /// Infallible: every 20-byte value is a valid address payload.
    pub fn from_hash_bytes(hash: [u8; Self::HASH_LEN]) -> Self {
        Self { hash }
    }

    /// Builds an address from a `version ‖ hash` payload.
    pub fn from_payload(payload: &[u8]) -> Result<Self, AddressError> {
        if payload.len() != Self::PAYLOAD_LEN {
            return Err(AddressError::PayloadLength {
                len: payload.len(),
                expected: Self::PAYLOAD_LEN,
            });
        }
        if payload[0] != Self::VERSION {
            return Err(AddressError::VersionMismatch {
                found: payload[0],
                expected: Self::VERSION,
            });
        }
        let mut hash = [0u8; Self::HASH_LEN];
        hash.copy_from_slice(&payload[1..]);
        Ok(Self { hash })
    }

    /// Returns the truncated identity-key hash.
    pub fn hash_bytes(&self) -> &[u8; Self::HASH_LEN] {
        &self.hash
    }

    /// Returns the `version ‖ hash` payload.
    pub fn payload(&self) -> [u8; Self::PAYLOAD_LEN] {
        let mut payload = [0u8; Self::PAYLOAD_LEN];
        payload[0] = Self::VERSION;
        payload[1..].copy_from_slice(&self.hash);
        payload
    }

    /// Returns the address version byte.
    pub fn version(&self) -> u8 {
        Self::VERSION
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `encode_lower_to_fmt` writes straight to the formatter and always
        // emits lowercase. Its error covers only a formatter failure and an
        // over-long code, the latter unreachable for a fixed 21-byte payload.
        bech32::encode_lower_to_fmt::<Bech32m, _>(f, Self::HRP, &self.payload())
            .map_err(|_| fmt::Error)
    }
}

impl FromStr for Address {
    type Err = AddressError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The crate rejects mixed case but accepts an all-uppercase string;
        // the protocol accepts one spelling, so screen case out first.
        if s.bytes().any(|b| b.is_ascii_uppercase()) {
            return Err(AddressError::NotLowercase);
        }

        // `CheckedHrpstring::new::<Bech32m>` pins the checksum algorithm.
        // `bech32::decode` must never be used here: it validates bech32m and
        // then silently falls back to the original bech32 constant.
        let checked =
            CheckedHrpstring::new::<Bech32m>(s).map_err(|e| AddressError::InvalidBech32m {
                reason: e.to_string(),
            })?;

        if checked.hrp() != Self::HRP {
            return Err(AddressError::HrpMismatch {
                found: checked.hrp().to_string(),
                expected: Self::HRP_STR,
            });
        }

        let payload: Vec<u8> = checked.byte_iter().collect();
        let address = Self::from_payload(&payload)?;

        // Every remaining degree of freedom — the two padding bits, and a
        // 35-character data part that truncates to the same 21 bytes — leaves
        // a checksum-valid string that decodes to this exact payload, so no
        // earlier check can see it. Re-encoding and requiring byte equality
        // collapses them all: one payload, one string.
        if address.to_string() != s {
            return Err(AddressError::NonCanonical);
        }

        Ok(address)
    }
}

impl Serialize for Address {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden vectors. Generated with the BIP-350 reference implementation
    // (github.com/sipa/bech32, ref/python/segwit_addr.py) — an implementation
    // independent of the `bech32` crate, so agreement between these literals
    // and the code below is a genuine two-implementation cross-check.
    //
    // These strings are the protocol's addressing format. Changing one means
    // every peer's identity changed: re-derive them from the reference
    // implementation, never edit them to match new code output.

    /// SHA-256 of the RFC 8032 §7.1 TEST 1 Ed25519 public key, truncated.
    const RFC8032_TV1_HASH: [u8; 20] = [
        0x21, 0xfe, 0x31, 0xdf, 0xa1, 0x54, 0xa2, 0x61, 0x62, 0x6b, 0xf8, 0x54, 0x04, 0x6f, 0xd2,
        0x27, 0x1b, 0x7b, 0xed, 0x4b,
    ];
    const RFC8032_TV1_ADDR: &str = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn";

    const ZEROS_HASH: [u8; 20] = [0u8; 20];
    const ZEROS_ADDR: &str = "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqn8antf";

    const FF_HASH: [u8; 20] = [0xffu8; 20];
    const FF_ADDR: &str = "off1q8llllllllllllllllllllllllllllllluvweh4p";

    const RAMP_HASH: [u8; 20] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
    ];
    const RAMP_ADDR: &str = "off1qyqqzqsrqszsvpcgpy9qkrqdpc83qygjzvpgkpr5";

    fn vectors() -> [([u8; 20], &'static str); 4] {
        [
            (RFC8032_TV1_HASH, RFC8032_TV1_ADDR),
            (ZEROS_HASH, ZEROS_ADDR),
            (FF_HASH, FF_ADDR),
            (RAMP_HASH, RAMP_ADDR),
        ]
    }

    #[test]
    fn encodes_reference_vectors() {
        for (hash, expected) in vectors() {
            let rendered = Address::from_hash_bytes(hash).to_string();
            assert_eq!(rendered, expected);
            assert_eq!(rendered.len(), Address::ENCODED_LEN);
        }
    }

    #[test]
    fn decodes_reference_vectors() {
        for (hash, encoded) in vectors() {
            let address: Address = encoded.parse().expect("reference vector must parse");
            assert_eq!(address.hash_bytes(), &hash);
            assert_eq!(address.version(), Address::VERSION);
        }
    }

    #[test]
    fn reencoding_a_parsed_vector_is_byte_identical() {
        for (_, encoded) in vectors() {
            let address: Address = encoded.parse().expect("reference vector must parse");
            assert_eq!(address.to_string(), encoded);
        }
    }

    #[test]
    fn roundtrips_arbitrary_hashes() {
        // Deterministic spread without pulling in a rng dev-dependency.
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..256 {
            let mut hash = [0u8; Address::HASH_LEN];
            for byte in hash.iter_mut() {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *byte = (state >> 33) as u8;
            }
            let address = Address::from_hash_bytes(hash);
            let encoded = address.to_string();
            assert_eq!(encoded.len(), Address::ENCODED_LEN);
            assert_eq!(encoded.parse::<Address>().expect("roundtrip"), address);
        }
    }

    #[test]
    fn payload_roundtrips() {
        let address = Address::from_hash_bytes(RAMP_HASH);
        let payload = address.payload();
        assert_eq!(payload[0], Address::VERSION);
        assert_eq!(&payload[1..], &RAMP_HASH);
        assert_eq!(Address::from_payload(&payload), Ok(address));
    }

    #[test]
    fn hrp_const_matches_parsed() {
        // `Hrp::parse_unchecked` performs no validation, so pin the const
        // against the checked parser.
        let parsed = Hrp::parse(Address::HRP_STR).expect("hrp must be valid");
        assert_eq!(parsed, Address::HRP);
    }

    #[test]
    fn serde_uses_the_canonical_string() {
        let address = Address::from_hash_bytes(RFC8032_TV1_HASH);
        let json = serde_json::to_string(&address).expect("serialize");
        assert_eq!(json, format!("\"{RFC8032_TV1_ADDR}\""));
        let back: Address = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, address);
    }

    #[test]
    fn serde_rejects_non_canonical_forms() {
        for bad in [
            ZEROS_ADDR.to_uppercase(),
            // Non-zero padding bits, valid checksum.
            "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqra5g99k".to_string(),
        ] {
            let json = format!("\"{bad}\"");
            assert!(serde_json::from_str::<Address>(&json).is_err(), "{bad}");
        }
    }

    #[test]
    fn ord_follows_hash_bytes_not_the_rendered_string() {
        let low = Address::from_hash_bytes(ZEROS_HASH);
        let high = Address::from_hash_bytes(FF_HASH);
        assert!(low < high);
        assert_eq!(low.hash_bytes() < high.hash_bytes(), low < high);

        // The two orders genuinely disagree, which is why tiebreakers must not
        // compare rendered strings: value 4 encodes as 'y' (0x79) and value 5
        // as '9' (0x39), inverting the comparison.
        let mut a = ZEROS_HASH;
        a[0] = 0x20; // leading fe = 4 -> 'y'
        let mut b = ZEROS_HASH;
        b[0] = 0x28; // leading fe = 5 -> '9'
        let (a, b) = (Address::from_hash_bytes(a), Address::from_hash_bytes(b));
        assert!(a < b, "byte order");
        assert!(a.to_string() > b.to_string(), "string order inverts");
    }

    #[test]
    fn rejects_uppercase_and_mixed_case() {
        // All-uppercase is a valid bech32m string per BIP-173 and the crate
        // accepts it; the protocol must not.
        assert_eq!(
            ZEROS_ADDR.to_uppercase().parse::<Address>(),
            Err(AddressError::NotLowercase)
        );
        let mixed = format!("{}N8antf", &ZEROS_ADDR[..ZEROS_ADDR.len() - 6]);
        assert_eq!(mixed.parse::<Address>(), Err(AddressError::NotLowercase));
    }

    #[test]
    fn rejects_wrong_hrp() {
        // Same payload, HRP "ofx", freshly checksummed.
        let other = "ofx1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq2e32t6";
        assert_eq!(
            other.parse::<Address>(),
            Err(AddressError::HrpMismatch {
                found: "ofx".to_string(),
                expected: Address::HRP_STR,
            })
        );
    }

    #[test]
    fn rejects_the_original_bech32_checksum() {
        // Same payload and HRP, checksummed with bech32 instead of bech32m.
        // `bech32::decode` would accept this; `CheckedHrpstring::<Bech32m>`
        // must not.
        let bech32_variant = "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqxmdlwt";
        assert!(matches!(
            bech32_variant.parse::<Address>(),
            Err(AddressError::InvalidBech32m { .. })
        ));
    }

    #[test]
    fn rejects_corrupted_characters() {
        let mut corrupted = ZEROS_ADDR.to_string();
        corrupted.pop();
        corrupted.push('p'); // valid charset member, wrong checksum
        assert!(matches!(
            corrupted.parse::<Address>(),
            Err(AddressError::InvalidBech32m { .. })
        ));

        // 'b', 'i', 'o' and '1' are excluded from the bech32 charset.
        for bad_char in ['b', 'i', 'o'] {
            let mut off_charset = ZEROS_ADDR.to_string();
            off_charset.pop();
            off_charset.push(bad_char);
            assert!(
                matches!(
                    off_charset.parse::<Address>(),
                    Err(AddressError::InvalidBech32m { .. })
                ),
                "{bad_char}"
            );
        }
    }

    #[test]
    fn rejects_unsupported_version_bytes() {
        for (encoded, expected_version) in [
            ("off1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqkynkkv", 0x00u8),
            ("off1qgqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqquz0u9x", 0x02),
            ("off1luqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqn347dq", 0xff),
        ] {
            assert_eq!(
                encoded.parse::<Address>(),
                Err(AddressError::VersionMismatch {
                    found: expected_version,
                    expected: Address::VERSION,
                })
            );
        }
    }

    #[test]
    fn rejects_wrong_payload_lengths() {
        // 20-byte payload (19-byte hash) and 22-byte payload (21-byte hash),
        // each correctly checksummed.
        for (encoded, len) in [
            ("off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqxfx3pw", 20usize),
            ("off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqkev2nx", 22),
        ] {
            assert_eq!(
                encoded.parse::<Address>(),
                Err(AddressError::PayloadLength {
                    len,
                    expected: Address::PAYLOAD_LEN,
                })
            );
        }
    }

    #[test]
    fn rejects_non_zero_padding_bits() {
        // 21 bytes is 168 bits carried in 34 five-bit groups, leaving 2
        // padding bits. Each mutant sets those bits and carries a freshly
        // computed, valid bech32m checksum, so it decodes to exactly the
        // all-zeros payload — the `bech32` crate discards the dirty bits
        // without complaint. This is the Briar "spare bit" bug; these three
        // assertions are the regression guard against reintroducing it.
        for mutant in [
            "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqpw3fxkm",
            "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqzqzuscy",
            "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqra5g99k",
        ] {
            assert_eq!(
                mutant.parse::<Address>(),
                Err(AddressError::NonCanonical),
                "{mutant}"
            );
            // The canonical form of that payload parses, proving the mutant is
            // a second spelling rather than a different address.
            assert!(ZEROS_ADDR.parse::<Address>().is_ok());
        }
    }

    #[test]
    fn rejects_over_long_data_parts() {
        // The crate's fes-to-bytes rule is `n * 5 / 8`, so a 35-character data
        // part also yields 21 bytes: a checksum-valid second spelling that the
        // length and version checks cannot see.
        for mutant in [
            "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqj7v8n5",
            "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqp0gcjwx",
            "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqlcjhrel",
        ] {
            assert_eq!(
                mutant.parse::<Address>(),
                Err(AddressError::NonCanonical),
                "{mutant}"
            );
        }
    }

    #[test]
    fn rejects_structural_garbage() {
        for bad in [
            "",
            "off",
            "off1",
            "off1qqq",
            "1qyqqqq",
            ZEROS_ADDR.trim_end_matches('f'),
        ] {
            assert!(
                matches!(
                    bad.parse::<Address>(),
                    Err(AddressError::InvalidBech32m { .. })
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn from_payload_rejects_bad_input() {
        assert_eq!(
            Address::from_payload(&[]),
            Err(AddressError::PayloadLength {
                len: 0,
                expected: Address::PAYLOAD_LEN,
            })
        );
        let mut payload = [0u8; Address::PAYLOAD_LEN];
        payload[0] = 0x02;
        assert_eq!(
            Address::from_payload(&payload),
            Err(AddressError::VersionMismatch {
                found: 0x02,
                expected: Address::VERSION,
            })
        );
    }
}
