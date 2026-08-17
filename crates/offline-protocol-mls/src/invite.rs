//! The invite payload: a self-verifying `{address, pubkey, petname?, sig?}`
//! blob for QR codes and links.
//!
//! This is the *permanent* path to first contact, and the primary one. Username
//! discovery (see [`crate::discovery`]) is additive: it is a non-authoritative
//! directory whose only trust anchor is a human confirming a name out of band,
//! which is exactly what scanning a QR code is. Deleting the invite path would
//! therefore delete the discovery layer's security model, not merely one of two
//! ways to reach someone.
//!
//! # What verification proves, and what it does not
//!
//! [`parse_invite`] enforces `derive_address(pubkey) == address` and, when a
//! signature is present, that the signature verifies under `pubkey`. That makes
//! the blob **self-certifying**: no directory, no server and no prior contact is
//! consulted, which is why the helpers are namespace-level and callable before
//! `create()`.
//!
//! It does **not** defend against substitution. An attacker who hands you their
//! own invite, correctly signed by their own key, is indistinguishable from a
//! legitimate stranger — no payload format can fix that, only the out-of-band
//! context in which the code was shown to you.
//!
//! What the optional signature defends is **relabeling**. Without it, anyone can
//! mint an invite pairing a victim's real, public `{address, pubkey}` with an
//! attacker-chosen petname, so an invite forwarded through a third party can
//! save Alice's key under the name "Bob". With it, the petname is bound to the
//! key by the key's owner.
//!
//! Include a signature when the invite may travel without its issuer. A QR code
//! shown phone-to-phone is already authenticated by the physical channel, and
//! an app that prompts the user to confirm or edit the name has made the user
//! the authority over it, which is what a petname properly is.
//!
//! # What an invite deliberately does not carry
//!
//! **No key package.** An MLS key package's init key is consumed by the first
//! peer who uses it, and a QR code is static, so pairing them guarantees a
//! collision as soon as two people scan the same code. Session establishment
//! runs over whatever transport connects, by the ordinary exchange.
//!
//! **No expiry.** A printed QR code that stops working is a bug. Apps that need
//! revocable invites have the server-mediated group-invite-link mechanism.
//!
//! # Container
//!
//! The SDK specifies the blob; apps own their URI scheme. The recommended form
//! is `<app-scheme>://connect?c=<blob>` — one opaque parameter, so it composes
//! with any existing scheme and route.

use std::str::FromStr;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use offline_protocol_core::{contains_control_or_format, Address};

use crate::canonical::canonical_payload;
use crate::error::{MlsError, Result};
use crate::manager::MlsManager;

/// Signature domain for invite payloads.
///
/// Must not be a prefix of, or prefixed by, any other live signing domain. See
/// the `canonical` module.
pub const INVITE_SIGN_DOMAIN: &[u8] = b"offline-invite-v1";

/// The only invite format version this build produces or accepts.
pub const INVITE_VERSION: u8 = 1;

/// Maximum petname length in bytes.
///
/// A petname is a display string, so the bound is on bytes for the same reason
/// [`Username::MAX_BYTES`](offline_protocol_core::Username::MAX_BYTES) is: it
/// caps what goes on the wire and through a signature.
pub const MAX_PETNAME_BYTES: usize = 64;

/// Length of an Ed25519 public key.
const PUBLIC_KEY_LEN: usize = 32;

/// Length of an Ed25519 signature.
const SIGNATURE_LEN: usize = 64;

/// Set when a petname follows the address.
const FLAG_PETNAME: u8 = 0b0000_0001;

/// Set when a signature terminates the blob.
const FLAG_SIGNATURE: u8 = 0b0000_0010;

/// Every flag bit this version defines.
///
/// An unknown bit is refused rather than ignored: the bits select which
/// trailing sections are present, so misreading one desynchronizes the parse
/// and would surface as a corrupt petname rather than as a version error.
const KNOWN_FLAGS: u8 = FLAG_PETNAME | FLAG_SIGNATURE;

/// A decoded and verified invite.
///
/// Holding one of these means the address self-certified against the public key
/// and, if `signed` is true, that the petname is bound to that key by its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// The address the invite reaches, already checked to be
    /// `derive_address(public_key)`.
    pub address: Address,
    /// The Ed25519 identity key the address derives from.
    pub public_key: Vec<u8>,
    /// The suggested display name, if the invite carried one.
    ///
    /// Suggested, not authoritative: it is a *locally assigned* name and an app
    /// is right to let the user edit it.
    pub petname: Option<String>,
    /// Whether a valid signature accompanied the invite.
    ///
    /// False means the petname is unbound, not that the invite is invalid. An
    /// unsigned invite is a legitimate and common shape.
    pub signed: bool,
}

/// Encodes an invite blob, optionally signed.
///
/// `signature` must be a signature by the key `public_key` names, over
/// [`invite_signing_payload`]. Pass `None` for the unsigned form.
///
/// # Errors
///
/// Returns [`MlsError::InvalidPublicKey`] if `public_key` is not 32 bytes,
/// [`MlsError::Serialization`] if the petname is over-long or carries a control
/// or format character, or if the signature is the wrong length.
pub fn encode_invite(
    address: &Address,
    public_key: &[u8],
    petname: Option<&str>,
    signature: Option<&[u8]>,
) -> Result<String> {
    if public_key.len() != PUBLIC_KEY_LEN {
        return Err(MlsError::InvalidPublicKey(format!(
            "Ed25519 public key must be {} bytes, got {}",
            PUBLIC_KEY_LEN,
            public_key.len()
        )));
    }
    // Checked here as well as at the type, because a caller can hand us an
    // address that is not this key's and produce a blob that fails to parse on
    // every scanner. Failing at mint is the honest place.
    let derived = MlsManager::derive_address(public_key)?;
    if derived != *address {
        return Err(MlsError::Serialization(
            "Invite address is not the address this public key derives to".to_string(),
        ));
    }

    let petname_bytes = match petname {
        // An empty petname and an absent one mean the same thing, so they get
        // the same encoding rather than two spellings of one state.
        Some("") => None,
        Some(name) => {
            if name.len() > MAX_PETNAME_BYTES {
                return Err(MlsError::Serialization(format!(
                    "Invite petname is {} bytes, maximum is {}",
                    name.len(),
                    MAX_PETNAME_BYTES
                )));
            }
            // The same screen a username gets, for a stronger reason. See
            // [`parse_invite`]: refusing at mint means an honest app cannot
            // build one of these by accident from a pasted display name.
            if contains_control_or_format(name) {
                return Err(MlsError::Serialization(
                    "Invite petname contains a control or format character".to_string(),
                ));
            }
            Some(name.as_bytes())
        }
        None => None,
    };

    if let Some(sig) = signature {
        if sig.len() != SIGNATURE_LEN {
            return Err(MlsError::Serialization(format!(
                "Invite signature must be {} bytes, got {}",
                SIGNATURE_LEN,
                sig.len()
            )));
        }
    }

    let address_string = address.to_string();
    let address_bytes = address_string.as_bytes();

    let mut flags = 0u8;
    if petname_bytes.is_some() {
        flags |= FLAG_PETNAME;
    }
    if signature.is_some() {
        flags |= FLAG_SIGNATURE;
    }

    let mut blob = Vec::with_capacity(
        2 + PUBLIC_KEY_LEN
            + 1
            + address_bytes.len()
            + petname_bytes.map_or(0, |p| 1 + p.len())
            + signature.map_or(0, |s| s.len()),
    );
    blob.push(INVITE_VERSION);
    blob.push(flags);
    blob.extend_from_slice(public_key);
    // The address is ASCII bech32m of a fixed length, so a single-byte prefix
    // cannot overflow.
    blob.push(address_bytes.len() as u8);
    blob.extend_from_slice(address_bytes);
    if let Some(petname) = petname_bytes {
        blob.push(petname.len() as u8);
        blob.extend_from_slice(petname);
    }
    if let Some(sig) = signature {
        blob.extend_from_slice(sig);
    }

    Ok(URL_SAFE_NO_PAD.encode(&blob))
}

/// Builds the payload an invite signature is taken over.
///
/// `domain ‖ u32be‖bytes` over `[v, address, public_key, petname]`, in that
/// fixed order, with an absent petname encoded as a zero-length field.
///
/// Exposed because signing happens where the private key is (the engine) while
/// verification happens here, and both must build byte-identical input.
pub fn invite_signing_payload(
    address: &Address,
    public_key: &[u8],
    petname: Option<&str>,
) -> Result<Vec<u8>> {
    let address_string = address.to_string();
    let petname_bytes = petname.unwrap_or("").as_bytes();
    canonical_payload(
        INVITE_SIGN_DOMAIN,
        &[
            &[INVITE_VERSION],
            address_string.as_bytes(),
            public_key,
            petname_bytes,
        ],
    )
}

/// Decodes and verifies an invite blob.
///
/// Verification is mandatory and total: an invite that fails any check is an
/// error, never a partially-trusted value. In order, cheap before expensive:
///
/// 1. base64url decodes, and the blob is structurally complete;
/// 2. `v == 1` and no unknown flag bits;
/// 3. the address parses in canonical form;
/// 4. the petname, when present, is displayable (no `Cc`, no `Cf`);
/// 5. `derive_address(public_key) == address`;
/// 6. the signature, when present, verifies under `public_key`.
///
/// # Errors
///
/// Returns [`MlsError::InvalidMessage`] for a malformed or truncated blob,
/// [`MlsError::VerificationFailed`] when the address does not derive from the
/// key or a present signature does not verify.
pub fn parse_invite(blob: &str) -> Result<Invite> {
    let bytes = URL_SAFE_NO_PAD
        .decode(blob.trim())
        .map_err(|e| MlsError::InvalidMessage(format!("Invite is not base64url: {}", e)))?;

    let mut cursor = Reader::new(&bytes);

    let version = cursor.take_u8("version")?;
    if version != INVITE_VERSION {
        return Err(MlsError::InvalidMessage(format!(
            "Unsupported invite version {}, expected {}",
            version, INVITE_VERSION
        )));
    }

    let flags = cursor.take_u8("flags")?;
    if flags & !KNOWN_FLAGS != 0 {
        return Err(MlsError::InvalidMessage(format!(
            "Invite carries unknown flag bits: {:#010b}",
            flags
        )));
    }

    let public_key = cursor.take("public key", PUBLIC_KEY_LEN)?.to_vec();

    let address_len = cursor.take_u8("address length")? as usize;
    let address_bytes = cursor.take("address", address_len)?;
    let address_str = std::str::from_utf8(address_bytes)
        .map_err(|_| MlsError::InvalidMessage("Invite address is not UTF-8".to_string()))?;
    let address = Address::from_str(address_str)
        .map_err(|e| MlsError::InvalidMessage(format!("Invite address is invalid: {}", e)))?;

    let petname = if flags & FLAG_PETNAME != 0 {
        let len = cursor.take_u8("petname length")? as usize;
        if len == 0 {
            // The flag says a petname follows; a zero-length one is a second
            // spelling of "absent", which `encode_invite` never emits.
            return Err(MlsError::InvalidMessage(
                "Invite sets the petname flag but carries an empty petname".to_string(),
            ));
        }
        if len > MAX_PETNAME_BYTES {
            return Err(MlsError::InvalidMessage(format!(
                "Invite petname is {} bytes, maximum is {}",
                len, MAX_PETNAME_BYTES
            )));
        }
        let bytes = cursor.take("petname", len)?;
        let name = std::str::from_utf8(bytes)
            .map_err(|_| MlsError::InvalidMessage("Invite petname is not UTF-8".to_string()))?;
        // A petname carrying a bidi override or a zero-width joiner renders as
        // something other than its own bytes, and this is the string an app
        // shows in the confirmation dialog after a scan. Worse than for a
        // username: when the invite is signed, the deceptive rendering arrives
        // bound to a *valid* signature, so an app that trusts `signed` is
        // trusting the wrong half. Refused here rather than left to every
        // caller to sanitize.
        if contains_control_or_format(name) {
            return Err(MlsError::InvalidMessage(
                "Invite petname contains a control or format character".to_string(),
            ));
        }
        Some(name.to_string())
    } else {
        None
    };

    let signature = if flags & FLAG_SIGNATURE != 0 {
        Some(cursor.take("signature", SIGNATURE_LEN)?.to_vec())
    } else {
        None
    };

    // Trailing bytes mean the blob is not what it claims to be. Ignoring them
    // would let one address travel under several distinct blobs, and would hide
    // a section this version does not know how to read.
    cursor.finish()?;

    // The check the whole format exists for. Runs before the signature so a
    // substituted key fails on the cheap comparison rather than on a verify.
    let derived = MlsManager::derive_address(&public_key)?;
    if derived != address {
        return Err(MlsError::VerificationFailed(
            "Invite address is not the address its public key derives to".to_string(),
        ));
    }

    let signed = match signature {
        Some(sig) => {
            let payload = invite_signing_payload(&address, &public_key, petname.as_deref())?;
            if !MlsManager::verify_signature(&public_key, &payload, &sig)? {
                return Err(MlsError::VerificationFailed(
                    "Invite signature does not verify under its public key".to_string(),
                ));
            }
            true
        }
        None => false,
    };

    Ok(Invite {
        address,
        public_key,
        petname,
        signed,
    })
}

/// A bounds-checked forward reader over the invite blob.
///
/// Every field is taken through this so a truncated blob produces a named
/// error rather than a panic on a slice index.
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, field: &str, len: usize) -> Result<&'a [u8]> {
        let end = self.offset.checked_add(len).ok_or_else(|| {
            MlsError::InvalidMessage(format!("Invite {} length overflows", field))
        })?;
        if end > self.bytes.len() {
            return Err(MlsError::InvalidMessage(format!(
                "Invite is truncated: {} needs {} bytes, {} remain",
                field,
                len,
                self.bytes.len().saturating_sub(self.offset)
            )));
        }
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn take_u8(&mut self, field: &str) -> Result<u8> {
        Ok(self.take(field, 1)?[0])
    }

    fn finish(&self) -> Result<()> {
        if self.offset != self.bytes.len() {
            return Err(MlsError::InvalidMessage(format!(
                "Invite has {} trailing bytes",
                self.bytes.len() - self.offset
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// A deterministic identity, so the golden vectors below are reproducible.
    fn identity(seed: u8) -> (SigningKey, Vec<u8>, Address) {
        let signing = SigningKey::from_bytes(&[seed; 32]);
        let public = signing.verifying_key().to_bytes().to_vec();
        let address = MlsManager::derive_address(&public).expect("derive");
        (signing, public, address)
    }

    fn sign(signing: &SigningKey, payload: &[u8]) -> Vec<u8> {
        signing.sign(payload).to_bytes().to_vec()
    }

    #[test]
    fn invite_round_trips_unsigned_without_a_petname() {
        let (_, public, address) = identity(1);
        let blob = encode_invite(&address, &public, None, None).expect("encode");
        let invite = parse_invite(&blob).expect("parse");
        assert_eq!(invite.address, address);
        assert_eq!(invite.public_key, public);
        assert_eq!(invite.petname, None);
        assert!(!invite.signed);
    }

    #[test]
    fn invite_round_trips_with_a_petname() {
        let (_, public, address) = identity(2);
        let blob = encode_invite(&address, &public, Some("Alice"), None).expect("encode");
        let invite = parse_invite(&blob).expect("parse");
        assert_eq!(invite.petname.as_deref(), Some("Alice"));
        assert!(!invite.signed);
    }

    #[test]
    fn invite_round_trips_signed() {
        let (signing, public, address) = identity(3);
        let payload = invite_signing_payload(&address, &public, Some("Alice")).expect("payload");
        let signature = sign(&signing, &payload);
        let blob =
            encode_invite(&address, &public, Some("Alice"), Some(&signature)).expect("encode");
        let invite = parse_invite(&blob).expect("parse");
        assert_eq!(invite.petname.as_deref(), Some("Alice"));
        assert!(invite.signed);
    }

    /// An empty petname is the same state as no petname, and must not produce a
    /// second encoding of it.
    #[test]
    fn invite_treats_an_empty_petname_as_absent() {
        let (_, public, address) = identity(4);
        let with_empty = encode_invite(&address, &public, Some(""), None).expect("encode");
        let without = encode_invite(&address, &public, None, None).expect("encode");
        assert_eq!(with_empty, without);
        assert_eq!(parse_invite(&with_empty).expect("parse").petname, None);
    }

    /// The check the format exists for: a blob pairing one key with another's
    /// address must be refused.
    #[test]
    fn invite_refuses_an_address_that_is_not_the_keys() {
        let (_, public_a, _) = identity(5);
        let (_, _, address_b) = identity(6);

        // `encode_invite` refuses to mint it...
        assert!(encode_invite(&address_b, &public_a, None, None).is_err());

        // ...and a hand-built blob is refused at parse, which is the check that
        // actually matters since an attacker does not use our encoder.
        let address_bytes = address_b.to_string();
        let mut blob = vec![INVITE_VERSION, 0u8];
        blob.extend_from_slice(&public_a);
        blob.push(address_bytes.len() as u8);
        blob.extend_from_slice(address_bytes.as_bytes());
        let encoded = URL_SAFE_NO_PAD.encode(&blob);

        assert!(matches!(
            parse_invite(&encoded),
            Err(MlsError::VerificationFailed(_))
        ));
    }

    /// The relabeling attack the optional signature exists to stop.
    #[test]
    fn invite_refuses_a_petname_swapped_under_a_signature() {
        let (signing, public, address) = identity(7);
        let payload = invite_signing_payload(&address, &public, Some("Alice")).expect("payload");
        let signature = sign(&signing, &payload);

        // The attacker keeps the victim's real, public {address, pubkey} and
        // the genuine signature, and swaps only the name.
        let relabeled =
            encode_invite(&address, &public, Some("Bob"), Some(&signature)).expect("encode");

        assert!(matches!(
            parse_invite(&relabeled),
            Err(MlsError::VerificationFailed(_))
        ));
    }

    /// An invite signed by a *different* key than the one it names must fail:
    /// the signature is verified under the invite's own public key, so this is
    /// the case where an attacker signs someone else's identity.
    #[test]
    fn invite_refuses_a_signature_by_a_foreign_key() {
        let (_, public, address) = identity(8);
        let (foreign_signing, _, _) = identity(9);
        let payload = invite_signing_payload(&address, &public, None).expect("payload");
        let signature = sign(&foreign_signing, &payload);
        let blob = encode_invite(&address, &public, None, Some(&signature)).expect("encode");
        assert!(matches!(
            parse_invite(&blob),
            Err(MlsError::VerificationFailed(_))
        ));
    }

    #[test]
    fn invite_refuses_an_unknown_version() {
        let (_, public, address) = identity(10);
        let blob = encode_invite(&address, &public, None, None).expect("encode");
        let mut bytes = URL_SAFE_NO_PAD.decode(&blob).expect("decode");
        bytes[0] = 2;
        assert!(matches!(
            parse_invite(&URL_SAFE_NO_PAD.encode(&bytes)),
            Err(MlsError::InvalidMessage(_))
        ));
    }

    /// An unknown flag bit selects a section this build cannot read, so it is
    /// refused rather than ignored — ignoring it desynchronizes the parse.
    #[test]
    fn invite_refuses_unknown_flag_bits() {
        let (_, public, address) = identity(11);
        let blob = encode_invite(&address, &public, None, None).expect("encode");
        let mut bytes = URL_SAFE_NO_PAD.decode(&blob).expect("decode");
        bytes[1] |= 0b1000_0000;
        assert!(matches!(
            parse_invite(&URL_SAFE_NO_PAD.encode(&bytes)),
            Err(MlsError::InvalidMessage(_))
        ));
    }

    #[test]
    fn invite_refuses_truncation_at_every_length() {
        let (signing, public, address) = identity(12);
        let payload = invite_signing_payload(&address, &public, Some("Alice")).expect("payload");
        let signature = sign(&signing, &payload);
        let blob =
            encode_invite(&address, &public, Some("Alice"), Some(&signature)).expect("encode");
        let bytes = URL_SAFE_NO_PAD.decode(&blob).expect("decode");

        for cut in 0..bytes.len() {
            let truncated = URL_SAFE_NO_PAD.encode(&bytes[..cut]);
            assert!(
                parse_invite(&truncated).is_err(),
                "a blob truncated to {} bytes must not parse",
                cut
            );
        }
    }

    #[test]
    fn invite_refuses_trailing_bytes() {
        let (_, public, address) = identity(13);
        let blob = encode_invite(&address, &public, None, None).expect("encode");
        let mut bytes = URL_SAFE_NO_PAD.decode(&blob).expect("decode");
        bytes.push(0);
        assert!(matches!(
            parse_invite(&URL_SAFE_NO_PAD.encode(&bytes)),
            Err(MlsError::InvalidMessage(_))
        ));
    }

    #[test]
    fn invite_refuses_an_over_long_petname() {
        let (_, public, address) = identity(14);
        let long = "a".repeat(MAX_PETNAME_BYTES + 1);
        assert!(encode_invite(&address, &public, Some(&long), None).is_err());
    }

    /// A petname that renders as something other than its own bytes is refused
    /// at both ends, and the parse side is the one that matters: an attacker
    /// does not use our encoder.
    ///
    /// The characters here are `Cf`, so [`char::is_control`] does not see any
    /// of them — a screen built on it alone would pass every one. This is the
    /// string an app shows in the dialog after a scan, so a right-to-left
    /// override here reads as a different name than the bytes that were
    /// signed.
    #[test]
    fn invite_refuses_a_petname_that_renders_as_another_name() {
        let (signing, public, address) = identity(15);

        for (label, name) in [
            ("right-to-left override", "ali\u{202E}ce"),
            ("zero-width joiner", "ali\u{200D}ce"),
            ("soft hyphen", "ali\u{00AD}ce"),
            ("byte-order mark", "ali\u{FEFF}ce"),
            ("newline", "ali\nce"),
        ] {
            assert!(
                encode_invite(&address, &public, Some(name), None).is_err(),
                "a {label} petname must not be mintable"
            );

            // Hand-built, since the encoder now refuses to produce one — and
            // signed, which is the case that matters: without this screen the
            // deceptive rendering would arrive carrying a *valid* signature,
            // so an app trusting `signed` would be trusting the wrong half.
            let payload = invite_signing_payload(&address, &public, Some(name)).expect("payload");
            let signature = sign(&signing, &payload);
            let address_bytes = address.to_string();
            let mut blob = vec![INVITE_VERSION, FLAG_PETNAME | FLAG_SIGNATURE];
            blob.extend_from_slice(&public);
            blob.push(address_bytes.len() as u8);
            blob.extend_from_slice(address_bytes.as_bytes());
            blob.push(name.len() as u8);
            blob.extend_from_slice(name.as_bytes());
            blob.extend_from_slice(&signature);

            assert!(
                matches!(
                    parse_invite(&URL_SAFE_NO_PAD.encode(&blob)),
                    Err(MlsError::InvalidMessage(_))
                ),
                "a {label} petname must be refused at parse even when signed"
            );
        }
    }

    /// The screen must not swallow ordinary names, or it is useless.
    #[test]
    fn invite_allows_ordinary_international_petnames() {
        let (_, public, address) = identity(16);
        for name in ["Alice", "José", "上田", "أحمد", "Ann Lee"] {
            let blob = encode_invite(&address, &public, Some(name), None).expect("encode");
            assert_eq!(
                parse_invite(&blob).expect("parse").petname.as_deref(),
                Some(name)
            );
        }
    }

    /// Golden vectors. These strings are the invite wire format: an
    /// implementation in another language must produce them byte for byte.
    ///
    /// **Computed independently of this code**, by a Python script that builds
    /// bech32m from the BIP-350 reference implementation, takes Ed25519 from
    /// `cryptography`, and assembles the blob from the written format rather
    /// than from this source. Agreement is therefore a two-implementation
    /// cross-check and not a restatement of whatever the encoder happened to
    /// emit. Regenerate them the same way, by changing the format on purpose —
    /// never by pasting new code output.
    ///
    /// The identity is the all-`0x01` Ed25519 seed, so a second implementation
    /// can reproduce every line without this repository.
    #[test]
    fn invite_golden_vectors() {
        let (signing, public, address) = identity(1);
        assert_eq!(
            address.to_string(),
            "off1qy682ruch4vlely5dkj94247jva7z49yk5xpqee0",
            "the golden identity's address changed"
        );

        let unsigned = encode_invite(&address, &public, None, None).expect("encode");
        assert_eq!(
            unsigned,
            "AQCKiOPddAnxlf1S2y08ul1yymcJvx2UEhvzdIgBtA9vXCxvZmYxcXk2ODJydWNoNHZsZWx5NWRrajk0MjQ3anZhN3o0OXlrNXhwcWVlMA"
        );

        let named = encode_invite(&address, &public, Some("alice"), None).expect("encode");
        assert_eq!(
            named,
            "AQGKiOPddAnxlf1S2y08ul1yymcJvx2UEhvzdIgBtA9vXCxvZmYxcXk2ODJydWNoNHZsZWx5NWRrajk0MjQ3anZhN3o0OXlrNXhwcWVlMAVhbGljZQ"
        );

        let payload = invite_signing_payload(&address, &public, Some("alice")).expect("payload");
        assert_eq!(
            base64::engine::general_purpose::STANDARD.encode(&payload),
            "b2ZmbGluZS1pbnZpdGUtdjEAAAABAQAAACxvZmYxcXk2ODJydWNoNHZsZWx5NWRrajk0MjQ3anZhN3o0OXlrNXhwcWVlMAAAACCKiOPddAnxlf1S2y08ul1yymcJvx2UEhvzdIgBtA9vXAAAAAVhbGljZQ=="
        );

        let signature = sign(&signing, &payload);
        let signed =
            encode_invite(&address, &public, Some("alice"), Some(&signature)).expect("encode");
        assert!(parse_invite(&signed).expect("parse").signed);
        assert_eq!(signed.len(), 199, "signed invite length changed");
    }
}
