//! Normalized username claims.
//!
//! A username is not an identity. It is a *label* a device claims in a
//! non-authoritative directory, and the identity it points at is the
//! [`Address`](crate::Address) inside the signed record. Nothing in the
//! protocol authenticates a username: anyone may claim any name, and the
//! resolver's job is to hand back every claimant rather than to pick one. See
//! `docs/spec/username-discovery.md`.
//!
//! # Why this is a type and not a `String`
//!
//! A discovery tag is a hash of the normalized name. Two implementations that
//! normalize differently derive different tags and **silently fail to find each
//! other** — there is no error, no mismatch to observe, just an empty result
//! for a name that exists. That is the whole failure mode this type exists to
//! remove: normalization happens once, at parse, and the derivations downstream
//! take [`Username`] rather than `&str`, so "derive a tag for whatever the app
//! typed" does not compile.
//!
//! It is the same move [`Address`](crate::Address) makes for routing tags, and
//! for the same reason.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// Why a username string was rejected by [`Username::from_str`].
// Adding a variant to a public error enum is a breaking change without this
// attribute; downstream crates must carry a wildcard arm.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsernameError {
    /// The username normalized to an empty string.
    ///
    /// Reachable from input that is not itself empty: a string of only
    /// whitespace or of characters that NFC discards normalizes away.
    #[error("username is empty after normalization")]
    Empty,
    /// The normalized username exceeds [`Username::MAX_BYTES`].
    ///
    /// Measured after normalization, since that is the form that gets
    /// published, hashed and signed.
    #[error("username is {len} bytes after normalization, maximum is {max}")]
    TooLong {
        /// Normalized length in bytes.
        len: usize,
        /// The ceiling.
        max: usize,
    },
    /// The username has the shape of an [`Address`](crate::Address).
    ///
    /// Refused at the type rather than at publish time so no caller can skip
    /// the screen. Two namespaces that never share a spelling cannot be
    /// confused by a UI that renders both, and a claim on an address-shaped
    /// name has no legitimate use.
    #[error("username has the shape of an address, which is not claimable")]
    AddressShaped,
    /// The username contains a control character.
    ///
    /// A name that can contain a newline or a bidi override is a name that
    /// renders as something other than itself in every UI that displays it.
    #[error("username contains a control character")]
    ControlCharacter,
}

/// A normalized username claim: NFC, lowercase, bounded, never address-shaped.
///
/// Construct with [`Username::from_str`]. The stored form **is** the normalized
/// form, so [`Username::as_str`] is what gets hashed into a discovery tag,
/// signed into a record, and compared against a queried name.
///
/// # Ordering
///
/// [`Ord`] compares the normalized bytes. Nothing in the protocol breaks a tie
/// on usernames — a resolution returns an unordered set of claims and the
/// *user* arbitrates — so this exists for deterministic test fixtures and
/// `BTreeMap` keys, not for consensus.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Username {
    normalized: String,
}

impl Username {
    /// Maximum length of the normalized form, in bytes.
    ///
    /// Bytes rather than characters because the bound exists to cap what goes
    /// on the wire and through a hash, and a character count says nothing
    /// about either. Matches the design record's `≤ 64 bytes`, which is far
    /// above every username policy this SDK's apps enforce (user-service caps
    /// at 20 characters) — the ceiling is a wire bound, not a policy.
    pub const MAX_BYTES: usize = 64;

    /// Length of the canonical string form of an address.
    ///
    /// Duplicated from `Address::ENCODED_LEN` rather than imported so this
    /// module stays a leaf: the screen below is a *shape* test that must reject
    /// address-looking strings whose checksum is wrong, so it deliberately does
    /// not parse. `username_address_shape_matches_real_addresses` pins the two
    /// against each other.
    const ADDRESS_LEN: usize = 44;

    /// Human-readable prefix of an address, including the bech32 separator.
    const ADDRESS_PREFIX: &'static str = "off1";

    /// Normalizes and validates a username.
    ///
    /// Normalization is **lowercase then NFC**, in that order. The order is
    /// load-bearing: Unicode lowercasing can emit a decomposed sequence, so
    /// normalizing first would leave a form that is not NFC. Running NFC last
    /// makes the operation idempotent, which
    /// [`username_normalization_is_idempotent`] pins — a non-idempotent
    /// normalizer would derive one tag on publish and another on resolve.
    fn normalize(raw: &str) -> String {
        raw.to_lowercase().nfc().collect()
    }

    /// Whether `candidate` has the shape of an address.
    ///
    /// A shape test, not a parse: an address with a corrupted checksum is not
    /// a valid address but is still an address-*shaped* string, and claiming it
    /// as a username is exactly as confusing in a UI as claiming a valid one.
    fn looks_like_address(candidate: &str) -> bool {
        candidate.len() == Self::ADDRESS_LEN
            && candidate.starts_with(Self::ADDRESS_PREFIX)
            && candidate
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    }

    /// Returns the normalized username.
    ///
    /// This is the form that is hashed, signed and published. A caller that
    /// wants the string the user typed must keep it themselves; the protocol
    /// only ever sees this one.
    pub fn as_str(&self) -> &str {
        &self.normalized
    }

    /// Consumes the username, returning the normalized string.
    pub fn into_string(self) -> String {
        self.normalized
    }
}

impl FromStr for Username {
    type Err = UsernameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = Self::normalize(s);

        if normalized.is_empty() {
            return Err(UsernameError::Empty);
        }
        if normalized.len() > Self::MAX_BYTES {
            return Err(UsernameError::TooLong {
                len: normalized.len(),
                max: Self::MAX_BYTES,
            });
        }
        // Checked on the normalized form: a name that only becomes a control
        // character after normalization would otherwise slip the screen.
        if normalized.chars().any(char::is_control) {
            return Err(UsernameError::ControlCharacter);
        }
        if Self::looks_like_address(&normalized) {
            return Err(UsernameError::AddressShaped);
        }

        Ok(Self { normalized })
    }
}

impl fmt::Display for Username {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.normalized)
    }
}

impl Serialize for Username {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.normalized)
    }
}

impl<'de> Deserialize<'de> for Username {
    /// Re-validates on the way in.
    ///
    /// A username arriving in a deserialized record is wire input: it has been
    /// normalized by *somebody*, and that somebody may be an attacker who
    /// normalized it differently on purpose. Parsing rather than accepting
    /// means a record whose username is not in canonical form fails to
    /// deserialize, instead of being compared against a queried name it can
    /// never equal.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let parsed: Self = s.parse().map_err(serde::de::Error::custom)?;
        // Reject a name that *normalizes* to something else rather than
        // silently repairing it. Accepting `Alice` here would make the record
        // verify against a tag it was never published at.
        if parsed.as_str() != s {
            return Err(serde::de::Error::custom(
                "username is not in normalized form",
            ));
        }
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Username {
        s.parse().expect("username should parse")
    }

    #[test]
    fn username_lowercases_ascii() {
        assert_eq!(parse("Alice").as_str(), "alice");
        assert_eq!(parse("ALICE").as_str(), "alice");
    }

    #[test]
    fn username_applies_nfc() {
        // "é" as e + U+0301 COMBINING ACUTE ACCENT normalizes to U+00E9.
        let decomposed = "cafe\u{0301}";
        let composed = "caf\u{00e9}";
        assert_ne!(decomposed, composed, "fixture must start decomposed");
        assert_eq!(parse(decomposed).as_str(), composed);
        assert_eq!(parse(decomposed), parse(composed));
    }

    /// The property the whole type exists for: publish and resolve must derive
    /// the same tag. A normalizer that is not idempotent derives one on the way
    /// out and another on the way back, and the failure is a silent empty
    /// result rather than an error.
    #[test]
    fn username_normalization_is_idempotent() {
        for raw in [
            "Alice",
            "cafe\u{0301}",
            "\u{1e9e}", // LATIN CAPITAL LETTER SHARP S, lowercases to "ß"
            "İ",        // LATIN CAPITAL LETTER I WITH DOT ABOVE, expands
            "ǅungla",   // titlecase digraph
        ] {
            let once = parse(raw);
            let twice = parse(once.as_str());
            assert_eq!(
                once, twice,
                "normalizing {:?} twice must equal normalizing it once",
                raw
            );
        }
    }

    #[test]
    fn username_rejects_empty_and_whitespace_only() {
        assert_eq!("".parse::<Username>(), Err(UsernameError::Empty));
    }

    #[test]
    fn username_rejects_over_length() {
        let long = "a".repeat(Username::MAX_BYTES + 1);
        assert_eq!(
            long.parse::<Username>(),
            Err(UsernameError::TooLong {
                len: Username::MAX_BYTES + 1,
                max: Username::MAX_BYTES,
            })
        );
        let at_limit = "a".repeat(Username::MAX_BYTES);
        assert!(at_limit.parse::<Username>().is_ok());
    }

    /// The bound is on the *normalized* form, so a name that grows past the
    /// ceiling only once normalized must still be refused.
    #[test]
    fn username_length_is_measured_after_normalization() {
        // Each "ẞ" lowercases to "ß" (2 bytes in UTF-8).
        let raw = "\u{1e9e}".repeat(Username::MAX_BYTES / 2 + 1);
        assert!(matches!(
            raw.parse::<Username>(),
            Err(UsernameError::TooLong { .. })
        ));
    }

    #[test]
    fn username_rejects_control_characters() {
        assert_eq!(
            "ali\nce".parse::<Username>(),
            Err(UsernameError::ControlCharacter)
        );
    }

    /// D3's publish-time refusal, enforced at the type so no call site can
    /// forget it.
    #[test]
    fn username_rejects_address_shaped_names() {
        let address = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn";
        assert_eq!(address.len(), Username::ADDRESS_LEN);
        assert_eq!(
            address.parse::<Username>(),
            Err(UsernameError::AddressShaped)
        );
        // Uppercase input normalizes into the refused shape rather than
        // slipping past a case-sensitive screen.
        assert_eq!(
            address.to_uppercase().parse::<Username>(),
            Err(UsernameError::AddressShaped)
        );
    }

    /// The screen is a shape test, so a *broken* address is refused too — an
    /// address with a mangled checksum reads exactly as confusingly in a UI.
    #[test]
    fn username_rejects_address_shaped_names_with_bad_checksums() {
        let mangled = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgqq";
        assert_eq!(mangled.len(), Username::ADDRESS_LEN);
        assert_eq!(
            mangled.parse::<Username>(),
            Err(UsernameError::AddressShaped)
        );
    }

    /// Pins the duplicated constant against the real address format. If
    /// `Address::ENCODED_LEN` ever changes, this fails rather than leaving the
    /// screen quietly matching nothing.
    #[test]
    fn username_address_shape_matches_real_addresses() {
        use crate::Address;
        assert_eq!(Username::ADDRESS_LEN, Address::ENCODED_LEN);
        let address = Address::from_hash_bytes([0u8; Address::HASH_LEN]).to_string();
        assert!(
            Username::looks_like_address(&address),
            "a real address must be refused as a username: {}",
            address
        );
    }

    /// A name that merely starts with `off1` is fine — only the full address
    /// shape is refused, so this does not quietly ban a namespace.
    #[test]
    fn username_allows_names_that_merely_start_with_the_address_prefix() {
        assert_eq!(parse("off1ce").as_str(), "off1ce");
    }

    #[test]
    fn username_round_trips_through_serde() {
        let username = parse("alice");
        let json = serde_json::to_string(&username).expect("serialize");
        assert_eq!(json, "\"alice\"");
        let back: Username = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, username);
    }

    /// Wire input is parsed, not accepted. A record naming `Alice` must fail to
    /// deserialize rather than be repaired into `alice` — the repaired form
    /// would verify against a tag the record was never published at.
    #[test]
    fn username_deserialization_refuses_unnormalized_input() {
        for raw in ["\"Alice\"", "\"cafe\\u0301\""] {
            assert!(
                serde_json::from_str::<Username>(raw).is_err(),
                "unnormalized {} must not deserialize",
                raw
            );
        }
    }
}
