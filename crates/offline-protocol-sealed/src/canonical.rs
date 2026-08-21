//! The house construction for domain-separated signing payloads.
//!
//! Every signature in this protocol is taken over
//! `domain ‖ Σ(u32be(len) ‖ field_bytes)`. The length prefix is what makes the
//! encoding unambiguous: without it, two different field splits can serialize
//! to the same bytes, and a signature over one is a valid signature over the
//! other. The domain is what stops a signature produced in one context from
//! being replayed in another that happens to reuse the same identity key.
//!
//! The construction is duplicated in two other places on purpose, because they
//! are different codebases and a shared crate would not reach them: the relay
//! server's `address_proof_payload`, and the two bridge implementations of the
//! relay address proof. What keeps them honest is that the domains must be
//! mutually non-prefixing, which `signing_domains_are_mutually_non_prefixing`
//! in the engine pins over all of them.
//!
//! Everything inside this workspace shares this module. That was not true
//! before the sealed layer existed: the engine carried its own copy of the
//! loop for control frames, which is now [`control_signing_payload`].
//!
//! # Why mutual non-prefixing matters
//!
//! If one domain were a prefix of another, the shorter domain's payload could
//! be made to collide with the longer one's by choosing a first field that
//! supplies the remaining domain bytes. The signature would then verify in a
//! context it was never issued for. Length-prefixing the fields does not
//! prevent this on its own, because the domain itself is not length-prefixed.

use crate::{Result, SealedError};
use alloc::vec::Vec;
use offline_protocol_core::Message;

/// Metadata key for the Ed25519 signature over the control message content (base64).
pub const CTRL_SIG_META_KEY: &str = "__ctrl_sig";
/// Metadata key for the sender's Ed25519 public key (base64, 32 bytes raw).
pub const CTRL_PK_META_KEY: &str = "__ctrl_pk";

/// Domain separator prepended to the canonical signing payload.
///
/// Prevents cross-context signature reuse: a signature produced for control
/// messages cannot be replayed in a future protocol extension that reuses the
/// same MLS identity key but with a different domain separator.
pub const CTRL_SIGN_DOMAIN: &[u8] = b"offline-ctrl-v1";

/// Builds `domain ‖ Σ(u32be(len) ‖ field)`.
///
/// # Errors
///
/// Returns [`SealedError::FieldTooLarge`] if a field exceeds `u32::MAX` bytes,
/// which no caller in this workspace can reach (every field is bounded far
/// below it) but which must not silently truncate a length prefix if one ever
/// could.
pub fn canonical_payload(domain: &[u8], fields: &[&[u8]]) -> Result<Vec<u8>> {
    let mut buf =
        Vec::with_capacity(domain.len() + fields.iter().map(|f| 4 + f.len()).sum::<usize>());
    buf.extend_from_slice(domain);
    for field in fields {
        let len: u32 = field
            .len()
            .try_into()
            .map_err(|_| SealedError::FieldTooLarge(field.len()))?;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(field);
    }
    Ok(buf)
}

/// Builds the canonical signing payload for a control frame.
///
/// Binds the four fields a control frame is authenticated over, in this order:
/// sender, message id, recipient, content. Anything outside them (metadata,
/// hop count, timestamps) is deliberately unsigned, because it is rewritten in
/// flight by relays and forwarders.
///
/// Producer and verifier call this same function. That is not a convenience:
/// a verifier that rebuilds the payload from its own copy of the field order
/// accepts forgeries the moment the two spellings drift apart.
///
/// # Errors
///
/// Returns [`SealedError::FieldTooLarge`] under the condition
/// [`canonical_payload`] documents.
pub fn control_signing_payload(message: &Message) -> Result<Vec<u8>> {
    let id = message.id.as_str();
    let fields: [&[u8]; 4] = [
        message.sender.as_str().as_bytes(),
        id.as_bytes(),
        message.recipient.as_str().as_bytes(),
        message.content.as_bytes(),
    ];
    canonical_payload(CTRL_SIGN_DOMAIN, &fields)
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn canonical_payload_length_prefixes_every_field() {
        let payload = canonical_payload(b"dom", &[b"ab", b"c"]).expect("payload");
        assert_eq!(payload, b"dom\x00\x00\x00\x02ab\x00\x00\x00\x01c".to_vec());
    }

    /// The property the length prefix buys: two different field splits that
    /// concatenate to the same bytes must not produce the same payload.
    #[test]
    fn canonical_payload_distinguishes_field_splits() {
        let a = canonical_payload(b"dom", &[b"ab", b"c"]).expect("payload");
        let b = canonical_payload(b"dom", &[b"a", b"bc"]).expect("payload");
        assert_ne!(a, b);
    }

    #[test]
    fn canonical_payload_distinguishes_domains() {
        let a = canonical_payload(b"dom-a", &[b"x"]).expect("payload");
        let b = canonical_payload(b"dom-b", &[b"x"]).expect("payload");
        assert_ne!(a, b);
    }

    /// The control domain's spelling is wire-visible: every peer that verifies
    /// a control frame rebuilds this exact byte string.
    #[test]
    fn control_domain_has_its_published_spelling() {
        assert_eq!(CTRL_SIGN_DOMAIN, b"offline-ctrl-v1");
        assert_eq!(CTRL_SIG_META_KEY, "__ctrl_sig");
        assert_eq!(CTRL_PK_META_KEY, "__ctrl_pk");
    }
}
