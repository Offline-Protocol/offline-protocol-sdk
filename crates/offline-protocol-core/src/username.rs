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
    /// The username is empty, or is nothing but whitespace.
    ///
    /// Whitespace-only is refused here rather than treated as a name: a claim
    /// nobody can type back, and that renders as nothing at all in every UI,
    /// is not a name. Note that normalization does not remove it — NFC
    /// discards nothing — so this is a screen, not a consequence of one.
    #[error("username is empty or whitespace after normalization")]
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
    /// The username contains a control or format character.
    ///
    /// A name that can contain a newline or a bidi override is a name that
    /// renders as something other than itself in every UI that displays it.
    ///
    /// Both Unicode `Cc` (control) and `Cf` (format) are refused, and the
    /// second is the one that matters: `char::is_control` covers only `Cc`, so
    /// a screen built on it alone lets through U+202E RIGHT-TO-LEFT OVERRIDE
    /// and the zero-width joiners — exactly the characters that make a name
    /// display as something other than its own bytes. Confusables between
    /// *scripts* remain out of scope (see the module docs); characters whose
    /// entire function is to alter rendering are not.
    #[error("username contains a control or format character")]
    ControlCharacter,
}

/// Whether `c` is a Unicode format (`Cf`) character.
///
/// [`char::is_control`] tests `Cc` only, which misses every character whose
/// entire function is to change how the text around it renders: the bidi
/// overrides, the zero-width joiners, the word joiner, the byte-order mark. A
/// name containing one displays as something other than its own bytes, which is
/// the failure [`UsernameError::ControlCharacter`] exists to prevent, so `Cf`
/// has to be screened alongside `Cc`.
///
/// Hand-rolled rather than taken from a general-category crate on purpose. The
/// alternative is a second Unicode table in the dependency graph, and this
/// crate is already carrying `unicode-normalization` into a workspace with a
/// binary-size profile (`minisize`) that cares. `Cf` is 21 ranges and it grows
/// by a handful per Unicode release.
///
/// Snapshot of `Cf` as of **Unicode 16.0**, generated from the character
/// database rather than transcribed. Regenerate it the same way; a range
/// missing here is a name that renders as something else, not a crash.
/// `username_rejects_every_format_character_range` pins one member of every
/// range so a bad edit fails rather than silently narrowing the screen.
fn is_format_character(c: char) -> bool {
    matches!(c,
        '\u{00AD}'                      // SOFT HYPHEN
        | '\u{0600}'..='\u{0605}'
        | '\u{061C}'                    // ARABIC LETTER MARK
        | '\u{06DD}'                    // ARABIC END OF AYAH
        | '\u{070F}'                    // SYRIAC ABBREVIATION MARK
        | '\u{0890}'..='\u{0891}'
        | '\u{08E2}'                    // ARABIC DISPUTED END OF AYAH
        | '\u{180E}'                    // MONGOLIAN VOWEL SEPARATOR
        | '\u{200B}'..='\u{200F}'       // ZWSP, ZWNJ, ZWJ, LRM, RLM
        | '\u{202A}'..='\u{202E}'       // bidi embedding and overrides
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}'       // bidi isolates and deprecated formats
        | '\u{FEFF}'                    // ZERO WIDTH NO-BREAK SPACE
        | '\u{FFF9}'..='\u{FFFB}'
        | '\u{110BD}'
        | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}'
        | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0001}'                   // LANGUAGE TAG
        | '\u{E0020}'..='\u{E007F}'     // tag characters
    )
}

/// Whether `s` contains a character that renders as something other than
/// itself: Unicode `Cc` (control) or `Cf` (format).
///
/// Public because a username is not the only display string this protocol
/// signs. An invite's petname is the other one, and it is the *more* exposed of
/// the two: it is what an app renders in the confirmation dialog after a scan,
/// and when the invite is signed the deceptive rendering arrives carrying a
/// valid signature. One screen serves both, so the two cannot drift into
/// disagreeing about what a displayable name is.
///
/// `Cf` is the half that matters and the half [`char::is_control`] misses; see
/// the `is_format_character` table below it.
pub fn contains_control_or_format(s: &str) -> bool {
    s.chars().any(|c| c.is_control() || is_format_character(c))
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
    ///
    /// The lowercase step is [`str::to_lowercase`], which is the **full**,
    /// language-insensitive Unicode mapping. That choice is part of the wire
    /// format, not an implementation detail: the full and simple mappings
    /// disagree wherever one character lowercases to several (`İ` becomes
    /// `i` + U+0307 under full, a bare `i` under simple), so a second
    /// implementation that picks the other one derives a different tag and the
    /// two silently never find each other. Language-insensitive matters for
    /// the same reason — the Turkish tailoring maps `I` to `ı`, which would
    /// make a name's tag depend on its publisher's locale.
    /// [`username_lowercases_with_the_full_language_insensitive_mapping`] pins
    /// both halves.
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

        // Whitespace-only is refused with the empty case rather than accepted:
        // NFC discards nothing, so a name of three spaces survives
        // normalization intact and would otherwise become a claim that renders
        // as nothing and cannot be typed back.
        if normalized.is_empty() || normalized.chars().all(char::is_whitespace) {
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
        if contains_control_or_format(&normalized) {
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

    /// The case mapping is part of the wire format, so it is pinned here.
    ///
    /// `İ` is the character the full and simple mappings disagree on: full
    /// gives `i` + U+0307, simple gives a bare `i`. An implementation that
    /// chose simple would derive a different discovery tag for the same name
    /// and silently never find the other's records — no error, just an empty
    /// result. The ASCII golden vectors elsewhere cannot catch that, because
    /// the two mappings agree on ASCII.
    #[test]
    fn username_lowercases_with_the_full_language_insensitive_mapping() {
        let parsed = parse("\u{0130}");
        assert_eq!(
            parsed.as_str(),
            "i\u{0307}",
            "the full mapping expands İ to i + COMBINING DOT ABOVE; a bare 'i' \
             means the simple mapping was used and every tag for such a name \
             will disagree with a conforming implementation"
        );
        assert_eq!(parsed.as_str().len(), 3, "i + U+0307 is 3 UTF-8 bytes");

        // Language-insensitive: the Turkish tailoring would lowercase 'I' to
        // 'ı' (U+0131), which would make a tag depend on the publisher's
        // locale.
        assert_eq!(parse("I").as_str(), "i");
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
        // Not a consequence of normalization: NFC discards none of these, so
        // without the explicit screen each one is a claimable name that renders
        // as nothing.
        for blank in [" ", "   ", "\u{00A0}", "\u{3000}", " \u{2009}"] {
            assert_eq!(
                blank.parse::<Username>(),
                Err(UsernameError::Empty),
                "whitespace-only {:?} must not be a claimable name",
                blank
            );
        }
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

    /// The characters that make a name render as something other than itself.
    ///
    /// These are `Cf`, not `Cc`, so [`char::is_control`] does not see any of
    /// them: a screen built on it alone accepts a claim carrying a
    /// right-to-left override, which a UI renders as a different name than the
    /// bytes that were signed. That is the exact failure
    /// [`UsernameError::ControlCharacter`] documents, and it went unscreened
    /// until this test existed.
    #[test]
    fn username_rejects_format_characters_that_control_check_alone_misses() {
        for (label, raw) in [
            ("right-to-left override", "ali\u{202E}ce"),
            ("zero-width joiner", "ali\u{200D}ce"),
            ("zero-width non-joiner", "ali\u{200C}ce"),
            ("zero-width space", "ali\u{200B}ce"),
            ("left-to-right mark", "ali\u{200E}ce"),
            ("soft hyphen", "ali\u{00AD}ce"),
            ("word joiner", "ali\u{2060}ce"),
            ("byte-order mark", "ali\u{FEFF}ce"),
            ("tag character", "alice\u{E0041}"),
        ] {
            let parsed = raw.parse::<Username>();
            assert!(
                !raw.chars().any(char::is_control),
                "{label} must not be Cc, or this test proves nothing new"
            );
            assert_eq!(
                parsed,
                Err(UsernameError::ControlCharacter),
                "a username carrying a {label} must be refused"
            );
        }
    }

    /// Every range of the hand-rolled `Cf` table is live.
    ///
    /// The table is a snapshot maintained by hand, so the failure to guard
    /// against is an edit that narrows a range and silently reopens the hole.
    /// One member per range is enough to catch that; completeness against the
    /// character database is a regeneration concern, not a runtime one.
    #[test]
    fn username_rejects_every_format_character_range() {
        for c in [
            '\u{00AD}',
            '\u{0600}',
            '\u{061C}',
            '\u{06DD}',
            '\u{070F}',
            '\u{0890}',
            '\u{08E2}',
            '\u{180E}',
            '\u{200B}',
            '\u{202A}',
            '\u{2060}',
            '\u{2066}',
            '\u{FEFF}',
            '\u{FFF9}',
            '\u{110BD}',
            '\u{110CD}',
            '\u{13430}',
            '\u{1BCA0}',
            '\u{1D173}',
            '\u{E0001}',
            '\u{E0020}',
        ] {
            assert!(
                is_format_character(c),
                "U+{:04X} must be screened as a format character",
                c as u32
            );
            assert_eq!(
                format!("ali{c}ce").parse::<Username>(),
                Err(UsernameError::ControlCharacter),
                "U+{:04X} must be refused in a username",
                c as u32
            );
        }
    }

    /// The screen must not swallow ordinary international names. A rule that
    /// rejects everything passes every negative test above and is useless.
    #[test]
    fn username_allows_ordinary_international_names() {
        for raw in [
            "alice",
            "josé",
            "мария",
            "上田",
            "أحمد",
            "ali ce",
            "a_b-c.d",
        ] {
            assert!(
                raw.parse::<Username>().is_ok(),
                "{raw:?} is a legitimate name and must parse"
            );
        }
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
