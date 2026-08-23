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

/// Domain separator for the signing payload that also binds *when*.
///
/// The v1 payload states who, to whom and what, and nothing about time, so a
/// signature over it is a bearer capability that never expires: a frame
/// captured off the air verifies as well on its tenth delivery as on its
/// first. This domain covers [`control_signing_payload_v2`], which adds the
/// frame's own timestamp as a fifth field, and a verifier that bounds it is
/// what turns the signature back into something perishable.
///
/// It is a **new domain rather than a new field under the old one** for the
/// reason the module header gives: the two payloads must not be confusable.
/// Were the timestamp simply appended under `offline-ctrl-v1`, a v1 verifier
/// handed a v2 frame would rebuild four fields, get different bytes, and
/// report a signature failure rather than a version it does not know, and a
/// v2 signature would be indistinguishable from a v1 signature over a
/// different field split had the length prefixes not been there. Separating
/// the domains states the difference instead of relying on it being detected.
///
/// The two domains are mutually non-prefixing, which
/// `signing_domains_are_mutually_non_prefixing` in the engine pins over every
/// domain in the protocol, including the two the relay and the bridges carry.
pub const CTRL_SIGN_DOMAIN_V2: &[u8] = b"offline-ctrl-v2";

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
/// hop count) is deliberately unsigned, because it is rewritten in flight by
/// relays and forwarders.
///
/// The frame's timestamp is *not* rewritten in flight, and leaving it out is
/// what issue 403 named: a signature over these four fields alone never goes
/// stale. [`control_signing_payload_v2`] is the payload that binds it, under
/// its own domain, and this one is retained to verify frames from peers that
/// have not yet moved.
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

/// Builds the canonical signing payload for a control frame, binding freshness.
///
/// The five fields, in this order: sender, message id, recipient, content, and
/// the frame's timestamp as 8 big-endian bytes of milliseconds since the Unix
/// epoch. The first four are exactly [`control_signing_payload`]'s; the fifth
/// is what a verifier bounds against its own clock, so a captured frame stops
/// verifying once it is older than the window that verifier allows.
///
/// # Why the timestamp and not a counter or a nonce
///
/// A counter cannot be reconciled with this protocol's delivery: a control
/// frame is retransmitted as frozen signed bytes for as long as the outbox
/// holds it, and a published key package is left on a relay to be *found*
/// rather than delivered, so a receiver that demands monotonicity refuses
/// frames that are late by design. A challenge-response nonce fails harder
/// still: the verifier of a frame that waited a week was not reachable when
/// it was minted and so cannot have issued anything. The timestamp is the
/// only freshness statement a sender can make with no knowledge of when, or
/// to what state, the frame will arrive.
///
/// # Why the field is fixed-width
///
/// Eight big-endian bytes rather than a decimal rendering, so there is exactly
/// one encoding of any instant. A textual field would let `"1000"` and
/// `"+1000"` and a zero-padded form all name the same time under different
/// signatures, which is the ambiguity the length prefixes exist to prevent one
/// level up.
///
/// # Errors
///
/// Returns [`SealedError::FieldTooLarge`] under the condition
/// [`canonical_payload`] documents.
pub fn control_signing_payload_v2(message: &Message) -> Result<Vec<u8>> {
    let id = message.id.as_str();
    let stamped = message.timestamp.as_millis().to_be_bytes();
    let fields: [&[u8]; 5] = [
        message.sender.as_str().as_bytes(),
        id.as_bytes(),
        message.recipient.as_str().as_bytes(),
        message.content.as_bytes(),
        &stamped,
    ];
    canonical_payload(CTRL_SIGN_DOMAIN_V2, &fields)
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, MessageId, Timestamp, UserId};

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
        assert_eq!(CTRL_SIGN_DOMAIN_V2, b"offline-ctrl-v2");
        assert_eq!(CTRL_SIG_META_KEY, "__ctrl_sig");
        assert_eq!(CTRL_PK_META_KEY, "__ctrl_pk");
    }

    /// Neither control domain may be a prefix of the other. The engine pins
    /// this across every domain in the protocol; pinning it here as well is
    /// what catches a rename inside this crate before it reaches the engine's
    /// wider test.
    #[test]
    fn the_two_control_domains_are_mutually_non_prefixing() {
        assert!(!CTRL_SIGN_DOMAIN_V2.starts_with(CTRL_SIGN_DOMAIN));
        assert!(!CTRL_SIGN_DOMAIN.starts_with(CTRL_SIGN_DOMAIN_V2));
    }

    fn control_frame(stamp_ms: i64) -> Message {
        let mut message = Message::from_parts(
            MessageId::from_bytes([7u8; 16]),
            UserId::new("off1alice").expect("sender"),
            UserId::new("off1bob").expect("recipient"),
            AppId::new("test").expect("app"),
            "__MLS_KEY_PKG__{}",
            Timestamp::from_millis(stamp_ms),
        );
        // Metadata is outside both payloads; setting some proves it.
        message
            .metadata
            .insert("unsigned".into(), "rewritten in flight".into());
        message
    }

    /// The v2 payload is not the v1 payload, even for the same frame, so a
    /// signature over one can never be presented as a signature over the
    /// other.
    #[test]
    fn the_two_payloads_differ_for_one_frame() {
        let message = control_frame(1_700_000_000_000);
        let v1 = control_signing_payload(&message).expect("v1");
        let v2 = control_signing_payload_v2(&message).expect("v2");
        assert_ne!(v1, v2);
    }

    /// The property the whole change exists for: two frames alike in every
    /// signed field except when they were minted produce different payloads,
    /// so a captured frame's signature does not carry over to a fresh stamp.
    #[test]
    fn the_v2_payload_changes_with_the_timestamp() {
        let early = control_signing_payload_v2(&control_frame(1_700_000_000_000)).expect("early");
        let late = control_signing_payload_v2(&control_frame(1_700_000_000_001)).expect("late");
        assert_ne!(early, late);
    }

    /// And the v1 payload does *not*, which is the gap being closed. This is
    /// the negative control for the test above: without it, that assertion
    /// would still pass if the timestamp had been bound in some other way.
    #[test]
    fn the_v1_payload_is_blind_to_the_timestamp() {
        let early = control_signing_payload(&control_frame(1_700_000_000_000)).expect("early");
        let late = control_signing_payload(&control_frame(1_900_000_000_000)).expect("late");
        assert_eq!(early, late);
    }

    /// The stamp is carried as eight big-endian bytes, and it is the last
    /// field. Pinned literally because both ends rebuild it: a leaf node
    /// writing this loop from the specification has to produce these exact
    /// bytes.
    #[test]
    fn the_v2_payload_ends_with_the_stamp_in_network_order() {
        let stamp: i64 = 1_700_000_000_000;
        let payload = control_signing_payload_v2(&control_frame(stamp)).expect("v2");
        let mut tail = [0u8; 12];
        tail[..4].copy_from_slice(&8u32.to_be_bytes());
        tail[4..].copy_from_slice(&stamp.to_be_bytes());
        assert!(payload.ends_with(&tail));
        assert!(payload.starts_with(CTRL_SIGN_DOMAIN_V2));
    }

    /// A negative stamp is a legal `i64` and must encode without panicking:
    /// the field is signed, and a device with no time source can report one.
    #[test]
    fn a_negative_stamp_encodes() {
        let payload = control_signing_payload_v2(&control_frame(-1)).expect("v2");
        assert!(payload.ends_with(&(-1i64).to_be_bytes()));
    }
}
