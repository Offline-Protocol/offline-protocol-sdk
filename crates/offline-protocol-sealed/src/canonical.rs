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

/// Domain separator for the proof a device signs to attach to a gateway.
///
/// Specified by the gateway contract (`docs/spec/gateway-contract.md`) and
/// registered in the signing-domain table of
/// `docs/spec/username-discovery.md`.
///
/// It **must not** equal, prefix, or be prefixed by
/// `offline-relay-addr-v1`, for a reason beyond the general non-prefixing
/// rule: the two payloads have an identical layout, so under a shared domain
/// a proof harvested by a hostile gateway would replay against the relay, and
/// the reverse. The engine's `signing_domains_are_mutually_non_prefixing`
/// pins the property across every domain in the protocol.
pub const GATEWAY_ADDR_SIGN_DOMAIN: &[u8] = b"offline-gateway-addr-v1";

/// Builds the bytes a device signs to bind `address` on the connection that
/// minted `challenge`.
///
/// The layout is `domain ‖ u32be(len(address)) ‖ address ‖ challenge`: the
/// address is the one length-prefixed field and **the challenge is appended
/// raw**. Prefixing the challenge as well would be the natural reading of
/// "two fields" and produces a payload four bytes longer that no conforming
/// verifier accepts, while a round-trip test written against the wrong
/// builder agrees with itself and never notices. A conformance vector pins
/// the bytes for that reason, as it does for a length written
/// little-endian or a dropped domain.
///
/// One prefix is enough for an unambiguous parse: it already fixes where the
/// address ends, and the challenge is the whole remainder. This is also the
/// relay declaration's layout exactly, under a different domain, which is
/// what lets one reviewer check both.
///
/// **The address is inside the signed bytes deliberately.** A signature over
/// a bare gateway-chosen challenge is a signing oracle: the gateway picks
/// challenge bytes that are also a valid control-frame payload and replays
/// the result as a frame from that peer. Committing the domain and the
/// address makes these bytes meaningful only as "this address, on this
/// gateway, holds this key".
///
/// The challenge is passed through unexamined. Its length is a wire rule the
/// caller enforces, not a property of the encoding: this function must build
/// the same bytes a verifier does for whatever challenge was actually minted,
/// including one a conformance vector chooses.
///
/// # Errors
///
/// Returns [`SealedError::FieldTooLarge`] under the same condition as
/// [`canonical_payload`], which no caller here can reach.
pub fn gateway_address_proof_payload(address: &str, challenge: &[u8]) -> Result<Vec<u8>> {
    let mut payload = canonical_payload(GATEWAY_ADDR_SIGN_DOMAIN, &[address.as_bytes()])?;
    payload.extend_from_slice(challenge);
    Ok(payload)
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

    /// The gateway domain's spelling is wire-visible on every attach.
    #[test]
    fn the_gateway_domain_has_its_published_spelling() {
        assert_eq!(GATEWAY_ADDR_SIGN_DOMAIN, b"offline-gateway-addr-v1");
    }

    /// The challenge is appended raw. Prefixing it too is the plausible wrong
    /// reading, and it is four bytes longer, so the length alone catches it.
    #[test]
    fn the_challenge_is_not_length_prefixed() {
        let challenge = [9u8; 32];
        let payload = gateway_address_proof_payload("off1abc", &challenge).expect("payload");
        assert_eq!(
            payload.len(),
            GATEWAY_ADDR_SIGN_DOMAIN.len() + 4 + "off1abc".len() + challenge.len()
        );
        assert!(payload.ends_with(&challenge));
        let doubly_prefixed =
            canonical_payload(GATEWAY_ADDR_SIGN_DOMAIN, &[b"off1abc", &challenge]).expect("alt");
        assert_ne!(payload, doubly_prefixed);
    }

    /// The address prefix is what separates two pairs whose concatenation is
    /// identical. Without it, a signature over one verifies over the other and
    /// a device is bindable to an address it never named.
    #[test]
    fn the_address_prefix_makes_the_payload_unambiguous() {
        let a = gateway_address_proof_payload("ab", b"cX").expect("a");
        let b = gateway_address_proof_payload("abc", b"X").expect("b");
        assert_ne!(a, b);
    }

    /// The same fields under the relay's domain are different bytes. This is
    /// the whole of the cross-replay defence: the layouts are identical.
    #[test]
    fn the_gateway_and_relay_domains_do_not_collide() {
        const RELAY_ADDR_SIGN_DOMAIN: &[u8] = b"offline-relay-addr-v1";
        let challenge = [3u8; 32];
        let gateway = gateway_address_proof_payload("off1abc", &challenge).expect("gateway");
        let mut relay =
            canonical_payload(RELAY_ADDR_SIGN_DOMAIN, &[b"off1abc"]).expect("relay payload");
        relay.extend_from_slice(&challenge);
        assert_ne!(gateway, relay);
        assert!(!GATEWAY_ADDR_SIGN_DOMAIN.starts_with(RELAY_ADDR_SIGN_DOMAIN));
        assert!(!RELAY_ADDR_SIGN_DOMAIN.starts_with(GATEWAY_ADDR_SIGN_DOMAIN));
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

/// The frozen conformance vectors for the control-plane canonical payloads.
///
/// The chapter these pin is `docs/spec/control-messages.md`. They were computed
/// by `tools/spec-vectors/generate.py` from the construction stated there, not
/// by running the builders below.
///
/// These pin the bytes that go under the signing key, not a signature. Ed25519
/// is specified by RFC 8032 and carries its own vectors; what is specific to
/// this protocol is *which* bytes are signed, and that is the half a second
/// implementation gets wrong.
#[cfg(all(test, feature = "std"))]
mod spec_vectors {
    use super::*;
    use offline_protocol_core::{AppId, Message, MessageId, Timestamp, UserId};

    const VECTORS: &str = include_str!("../tests/data/control-signing-v1.vectors.json");

    fn vectors() -> serde_json::Value {
        serde_json::from_str(VECTORS).expect("the vector file is JSON")
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Asserts the file still carries what it carried, before anything iterates
    /// it: a loop over an array a bad merge emptied passes by not running.
    #[test]
    fn the_vector_file_is_the_size_it_was() {
        let v = vectors();
        assert_eq!(v["payloads"].as_array().expect("payloads").len(), 6);
        assert_eq!(v["domains"]["live"].as_array().expect("live").len(), 6);
        assert_eq!(
            v["domains"]["reserved"].as_array().expect("reserved").len(),
            0,
            "every registered domain is live; a new reserved one needs a row here"
        );
    }

    /// Every case, pinned through the primitive both builders are made of.
    ///
    /// This runs for all six including the ones no valid `Message` can carry:
    /// an empty recipient is exactly the case the length prefixes exist for,
    /// and it would go untested if the only route in were the message builders.
    #[test]
    fn every_payload_matches_its_vector_through_the_primitive() {
        for case in vectors()["payloads"].as_array().expect("payloads") {
            let name = case["name"].as_str().expect("a name");
            let sender = case["sender"].as_str().expect("sender");
            let id = case["id"].as_str().expect("id");
            let recipient = case["recipient"].as_str().expect("recipient");
            let content = case["content"].as_str().expect("content");
            let stamp = case["timestamp_ms"].as_i64().expect("timestamp");

            let v1 = canonical_payload(
                CTRL_SIGN_DOMAIN,
                &[
                    sender.as_bytes(),
                    id.as_bytes(),
                    recipient.as_bytes(),
                    content.as_bytes(),
                ],
            )
            .expect("v1 builds");
            assert_eq!(
                hex(&v1),
                case["v1_hex"].as_str().expect("v1"),
                "[{name}] v1 payload"
            );

            let stamped = stamp.to_be_bytes();
            let v2 = canonical_payload(
                CTRL_SIGN_DOMAIN_V2,
                &[
                    sender.as_bytes(),
                    id.as_bytes(),
                    recipient.as_bytes(),
                    content.as_bytes(),
                    &stamped,
                ],
            )
            .expect("v2 builds");
            assert_eq!(
                hex(&v2),
                case["v2_hex"].as_str().expect("v2"),
                "[{name}] v2 payload"
            );
        }
    }

    /// The message builders produce the same bytes for every case a valid
    /// message can express.
    ///
    /// Pinning only the primitive would leave the field order the builders
    /// choose unpinned, which is the thing that actually differs between two
    /// implementations.
    #[test]
    fn the_builders_agree_with_the_vectors() {
        let mut exercised = 0;
        for case in vectors()["payloads"].as_array().expect("payloads") {
            let name = case["name"].as_str().expect("a name");
            let recipient = case["recipient"].as_str().expect("recipient");
            if recipient.is_empty() {
                // No valid `Message` carries an empty recipient; the primitive
                // test above is what covers that case.
                continue;
            }
            exercised += 1;

            let m = Message::from_parts(
                MessageId::from_str(case["id"].as_str().expect("id")).expect("a uuid"),
                UserId::new(case["sender"].as_str().expect("sender")).expect("a sender"),
                UserId::new(recipient).expect("a recipient"),
                AppId::new("com.example.chat").expect("an app id"),
                case["content"].as_str().expect("content"),
                Timestamp::from_millis(case["timestamp_ms"].as_i64().expect("timestamp")),
            );

            assert_eq!(
                hex(&control_signing_payload(&m).expect("v1")),
                case["v1_hex"].as_str().expect("v1"),
                "[{name}] the v1 builder disagrees with the chapter"
            );
            assert_eq!(
                hex(&control_signing_payload_v2(&m).expect("v2")),
                case["v2_hex"].as_str().expect("v2"),
                "[{name}] the v2 builder disagrees with the chapter"
            );
        }
        assert!(exercised >= 5, "the builders were barely exercised");
    }

    /// v1 with the stamp appended is not v2.
    ///
    /// They are separate domains rather than one payload with an optional
    /// field. An implementation that appends instead of re-domaining produces
    /// signatures that every verifier reports as forgeries, which is
    /// indistinguishable from an attack and sends the reader hunting the wrong
    /// bug.
    #[test]
    fn appending_the_stamp_does_not_produce_the_v2_payload() {
        let v = vectors();
        let case = &v["v1_is_not_v2_with_a_stamp"];
        assert_ne!(
            case["v1_with_stamp_appended_hex"]
                .as_str()
                .expect("appended"),
            case["v2_hex"].as_str().expect("v2"),
            "the two domains have collapsed into one"
        );
    }

    /// The domains this file names are the domains the code uses, and no domain
    /// prefixes another.
    ///
    /// The domain is not length-prefixed, so a domain that prefixed another
    /// would let a signature made under the shorter verify under the longer.
    #[test]
    fn the_domain_registry_matches_the_code() {
        let v = vectors();
        assert_eq!(
            v["domains"]["v1"].as_str().expect("v1"),
            core::str::from_utf8(CTRL_SIGN_DOMAIN).expect("utf8")
        );
        assert_eq!(
            v["domains"]["v2"].as_str().expect("v2"),
            core::str::from_utf8(CTRL_SIGN_DOMAIN_V2).expect("utf8")
        );

        let mut all: Vec<String> = Vec::new();
        for key in ["live", "reserved"] {
            for d in v["domains"][key].as_array().expect("a domain list") {
                all.push(d.as_str().expect("a domain").to_string());
            }
        }
        for a in &all {
            for b in &all {
                if a != b {
                    assert!(
                        !a.starts_with(b.as_str()),
                        "signing domain {b} is a prefix of {a}, which lets a \
                         signature made under one verify under the other"
                    );
                }
            }
        }
    }

    /// The chapter, or `None` where the repo tree is absent: `cargo package`
    /// carries `tests/` and the vector file but cannot carry `docs/`.
    fn chapter(name: &str) -> Option<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/spec")
            .join(name);
        std::fs::read_to_string(&path).ok().or_else(|| {
            eprintln!("spec tree not present, skipping the {name} drift checks");
            None
        })
    }

    #[test]
    fn the_chapter_states_the_payloads_the_code_builds() {
        let Some(text) = chapter("control-messages.md") else {
            return;
        };
        for domain in [CTRL_SIGN_DOMAIN, CTRL_SIGN_DOMAIN_V2] {
            let domain = core::str::from_utf8(domain).expect("utf8");
            assert!(
                text.contains(domain),
                "the chapter no longer names the {domain} payload"
            );
        }
        assert!(
            text.contains("u32be(len(sender))"),
            "the chapter no longer states the length-prefixed construction"
        );
    }

    /// The protocol-wide signing-domain registry lists every domain this file
    /// pins.
    ///
    /// The registry lives in the username-discovery chapter for historical
    /// reasons and is the one table that is supposed to see every domain at
    /// once. A domain added here and not there is a domain chosen without being
    /// checked against the whole set, which is how a prefixing pair ships.
    #[test]
    fn the_published_registry_lists_every_domain_this_file_pins() {
        let Some(text) = chapter("username-discovery.md") else {
            return;
        };
        let v = vectors();
        for key in ["live", "reserved"] {
            for d in v["domains"][key].as_array().expect("a domain list") {
                let domain = d.as_str().expect("a domain");
                assert!(
                    text.contains(domain),
                    "the signing-domain registry does not list {domain}"
                );
            }
        }
    }
}

/// The frozen conformance vectors for the gateway attach proof.
///
/// The chapter these pin is `docs/spec/gateway-contract.md`. They were computed
/// by `tools/spec-vectors/generate.py` from the construction stated there, not
/// by running the builder below.
///
/// A second implementation of this protocol has one chance to get the attach
/// proof right and no feedback when it does not: a wrong payload is a
/// signature that verifies against nothing, which looks like a key problem
/// rather than an encoding problem. These are the bytes that settle it.
#[cfg(all(test, feature = "std"))]
mod gateway_proof_vectors {
    use super::*;

    const VECTORS: &str = include_str!("../tests/data/gateway-address-proof-v1.vectors.json");

    fn vectors() -> serde_json::Value {
        serde_json::from_str(VECTORS).expect("the vector file is JSON")
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(text: &str) -> Vec<u8> {
        assert!(text.len().is_multiple_of(2), "hex has an odd length");
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex digit"))
            .collect()
    }

    /// Asserts the file still carries what it carried, before anything
    /// iterates it: a loop over an array a bad merge emptied passes by not
    /// running.
    #[test]
    fn the_vector_file_is_the_size_it_was() {
        let v = vectors();
        assert_eq!(v["proofs"].as_array().expect("proofs").len(), 3);
        assert_eq!(
            v["domain"].as_str().expect("domain"),
            "offline-gateway-addr-v1"
        );
        assert_eq!(
            v["layout"].as_str().expect("layout"),
            "domain || u32be(len(address_utf8)) || address_utf8 || challenge"
        );
    }

    /// Every case, through the builder both ends of an attach call.
    #[test]
    fn every_proof_matches_its_vector() {
        for case in vectors()["proofs"].as_array().expect("proofs") {
            let name = case["name"].as_str().expect("a name");
            let address = case["address"].as_str().expect("address");
            let challenge = unhex(case["challenge_hex"].as_str().expect("challenge"));

            let payload = gateway_address_proof_payload(address, &challenge).expect("payload");
            assert_eq!(
                hex(&payload),
                case["payload_hex"].as_str().expect("payload"),
                "[{name}] gateway attach proof"
            );
        }
    }

    /// The length prefix, pinned as a pair rather than as a property: two
    /// pairs whose concatenation is identical must not produce one payload.
    #[test]
    fn the_length_prefix_is_pinned_by_a_colliding_pair() {
        let v = vectors();
        let block = &v["the_length_prefix_makes_the_payload_unambiguous"];
        let mut seen = Vec::new();
        for key in ["a", "b"] {
            let address = block[key]["address"].as_str().expect("address");
            let challenge = unhex(block[key]["challenge_hex"].as_str().expect("challenge"));
            let payload = gateway_address_proof_payload(address, &challenge).expect("payload");
            assert_eq!(
                hex(&payload),
                block[key]["payload_hex"].as_str().expect("payload"),
                "[{key}] colliding-pair payload"
            );
            seen.push(payload);
        }
        assert_ne!(seen[0], seen[1], "the prefix is what separates them");
    }

    /// The cross-replay defence, pinned as bytes: the relay's layout under
    /// the relay's domain is not this payload, for the same fields.
    #[test]
    fn the_domain_separates_the_two_attaches() {
        let v = vectors();
        let block = &v["the_domain_is_what_stops_a_cross_replay"];
        let address = block["address"].as_str().expect("address");
        let challenge = unhex(block["challenge_hex"].as_str().expect("challenge"));

        let gateway = gateway_address_proof_payload(address, &challenge).expect("gateway");
        assert_eq!(
            hex(&gateway),
            block["gateway_payload_hex"].as_str().expect("gateway hex")
        );
        assert_ne!(
            hex(&gateway),
            block["same_fields_under_the_relay_domain_hex"]
                .as_str()
                .expect("relay hex"),
            "a shared domain would make a harvested proof replayable"
        );
    }
}
