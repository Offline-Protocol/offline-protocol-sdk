//! Compact binary wire codec for [`Message`](crate::Message) (wire format **v1**).
//!
//! This is an **additive** second encoding alongside the canonical JSON
//! representation — it does not replace it. JSON remains the permanent
//! interoperability floor: every receiver still accepts JSON, and a binary frame
//! is only ever produced for a peer known to support it. The two encodings are
//! distinguished by the first byte alone ([`WIRE_V1_MAGIC`] vs. `{` = `0x7B`), so
//! no negotiation is required to *decode*.
//!
//! # Why a frozen DTO
//!
//! [`Message`](crate::Message) carries `#[serde(default)]` and
//! `#[serde(skip_serializing_if)]` attributes plus validation-on-deserialize
//! impls that a non-self-describing binary format (postcard) cannot honor
//! field-for-field. Rather than serialize `Message` directly, this module defines
//! `WireMessageV1`, a flat DTO with a fixed field order, and converts to/from
//! `Message` through the validating constructors (`UserId::new`,
//! `LamportClock::from_value`, …). That keeps the security checks the JSON path
//! enforces (identifier caps, Lamport clamp) intact on the binary path.
//!
//! # Evolution contract
//!
//! postcard is positional: reordering, removing, retyping, or inserting a field
//! silently corrupts decoding on peers running the previous layout. Therefore:
//!
//! * **Never** change `WireMessageV1`'s existing fields.
//! * Additive, backward-compatible data goes into `WireMessageV1::ext`, a
//!   trailing `(tag, bytes)` list that old decoders read and ignore.
//! * A change that cannot be expressed as an `ext` entry requires a new magic
//!   byte (`0xF6` = v2) and out-of-band version negotiation.
//!
//! The numeric enum mappings (`priority_to_u8`, `content_type_to_u8`) are
//! likewise a frozen wire contract.
//!
//! (`WireMessageV1` and the two mapping helpers are deliberately private — the
//! DTO is an encoding detail, not API surface — so they are named here in code
//! spans rather than as intra-doc links, which rustdoc rejects for private
//! items in public documentation.)

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::message::{ContentType, MessagePriority};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};

/// Magic/version byte prefixed to every wire-format-v1 binary frame.
///
/// Drawn from `0xF5..=0xFF`: these are invalid UTF-8 leading bytes and cannot
/// begin a JSON document, so a decoder tells a binary frame from the legacy JSON
/// encoding (which always starts with `{` = `0x7B`) by inspecting one byte, with
/// no negotiation. Future breaking revisions take the next value (`0xF6` = v2, …),
/// leaving room for eleven wire versions before the range is exhausted.
pub const WIRE_V1_MAGIC: u8 = 0xF5;

/// Logical wire-format version advertised during capability negotiation,
/// corresponding to [`WIRE_V1_MAGIC`]. Peers exchange the set of versions they
/// can decode in their signed key package; a peer that lists this value can
/// decode `WIRE_V1_MAGIC` frames, so it is safe to send it binary.
pub const WIRE_VERSION_V1: u8 = 1;

/// `ext` TLV tag: the trailing base64 run of `content`, carried as raw bytes.
///
/// Emitted by `Message::to_wire_v1_bytes` when `content` ends in a long
/// canonical-base64 tail (an MLS envelope, for example): the wire `content`
/// keeps only the head and the decoded tail rides here, saving the 4/3 base64
/// inflation. `Message::from_wire_v1_bytes` re-encodes the bytes and appends
/// them to `content`, reconstructing the original string byte-for-byte — the
/// split is only taken when [`split_b64_tail`] verifies that property at
/// encode time.
///
/// # Tag registry (wire v1)
///
/// | tag | meaning                                      |
/// |-----|----------------------------------------------|
/// | 1   | base64 tail of `content`, decoded            |
/// | 2   | `reply_context` as JSON ([`EXT_TAG_REPLY_CONTEXT`]) |
///
/// Tag 1 ships in wire v1's *first* release, so advertising [`WIRE_VERSION_V1`]
/// implies understanding it. Note the constraint that imposes on future tags:
/// a decoder that ignores tag 1 would reconstruct a truncated `content`, which
/// is only safe because no v1 decoder without tag-1 support ever shipped. A
/// future tag whose absence changes meaning (rather than merely costing
/// efficiency or optional context) cannot piggyback on v1 — it needs a new
/// wire version.
pub(crate) const EXT_TAG_B64_TAIL: u16 = 1;

/// `ext` TLV tag: [`ReplyContext`](crate::ReplyContext) serialized as JSON.
///
/// Rides opaquely (like `media_metadata_json`) so the struct keeps evolving
/// through its own serde attributes. Unlike tag 1, this tag may piggyback on
/// v1 even though not every v1 decoder understands it: a decoder that skips
/// it delivers the message without its reply preview — exactly the
/// degradation a legacy JSON receiver applies by ignoring the unknown
/// `reply_context` field. Meaning is preserved; only optional display
/// context is lost.
///
/// Decoders honor only the first tag-2 entry and reject a frame whose
/// payload is not valid `ReplyContext` JSON, matching the JSON path where a
/// malformed `reply_context` rejects the whole message.
pub(crate) const EXT_TAG_REPLY_CONTEXT: u16 = 2;

/// Minimum length (in base64 characters, including padding) of a trailing
/// base64 run before [`split_b64_tail`] moves it to the ext TLV. Short tails
/// save a handful of bytes at the cost of a TLV entry and an alphabet scan on
/// every hop; 64 characters (48 raw bytes) is where the trade clearly wins.
const B64_TAIL_MIN_LEN: usize = 64;

/// Splits `content` into `(head, raw)` such that
/// `head + BASE64.encode(raw) == content`, byte-for-byte.
///
/// Returns `Some` only when `content` ends in a run of at least
/// [`B64_TAIL_MIN_LEN`] standard-alphabet base64 characters (4-aligned, with
/// canonical `=` padding only at the very end) that strict-decodes **and**
/// re-encodes to exactly the original tail. That re-encode comparison makes
/// the split correct by construction for arbitrary input: non-canonical
/// padding, foreign alphabets, or lookalike text simply fail the comparison
/// and the content rides as plain text. A run longer than needed for
/// 4-alignment sheds its leading remainder into the head, which preserves the
/// reconstruction property.
pub(crate) fn split_b64_tail(content: &str) -> Option<(&str, Vec<u8>)> {
    let bytes = content.as_bytes();
    if bytes.len() < B64_TAIL_MIN_LEN {
        return None;
    }
    // Canonical padding (at most two '='), only at the very end.
    let mut run_end = bytes.len();
    let mut pad = 0;
    while pad < 2 && run_end > 0 && bytes[run_end - 1] == b'=' {
        run_end -= 1;
        pad += 1;
    }
    let is_b64 = |b: u8| b.is_ascii_alphanumeric() || b == b'+' || b == b'/';
    let mut start = run_end;
    while start > 0 && is_b64(bytes[start - 1]) {
        start -= 1;
    }
    // Base64 decodes in 4-character quanta; align by shedding the leading
    // remainder of the run into the head. All run characters are ASCII, so
    // both `start` and the shifted index stay on char boundaries.
    start += (bytes.len() - start) % 4;
    let tail = &content[start..];
    if tail.len() < B64_TAIL_MIN_LEN {
        return None;
    }
    let raw = BASE64.decode(tail).ok()?;
    if BASE64.encode(&raw) != tail {
        return None;
    }
    Some((&content[..start], raw))
}

/// Re-encodes a tag-1 ext value for appending to `content`; the exact inverse
/// of [`split_b64_tail`]'s decode.
pub(crate) fn encode_b64_tail(raw: &[u8]) -> String {
    BASE64.encode(raw)
}

/// Flat, fixed-layout DTO mirroring [`Message`](crate::Message) for wire v1.
///
/// See the [module docs](self) for the field-stability contract. Fields are
/// `pub(crate)` so the `Message` conversion in `message.rs` can build and read
/// them; the type itself never leaves the crate.
#[derive(Serialize, Deserialize)]
pub(crate) struct WireMessageV1 {
    pub(crate) id: [u8; 16],
    pub(crate) sender: String,
    pub(crate) recipient: String,
    pub(crate) app_id: String,
    pub(crate) priority: u8,
    pub(crate) ttl: u8,
    pub(crate) hop_count: u8,
    pub(crate) timestamp: i64,
    pub(crate) lamport_clock: u64,
    pub(crate) content_type: u8,
    pub(crate) content: String,
    pub(crate) binary_content: Option<Vec<u8>>,
    /// [`MediaMetadata`](crate::MediaMetadata) serialized as JSON. Rare and
    /// structurally rich, so it rides as an opaque blob to keep the frozen
    /// surface small and let it keep evolving via its own serde attributes.
    pub(crate) media_metadata_json: Option<Vec<u8>>,
    /// Sorted for deterministic output (a `HashMap` iterates nondeterministically).
    pub(crate) metadata: Vec<(String, String)>,
    pub(crate) requires_ack: bool,
    pub(crate) reply_to_msg: Option<[u8; 16]>,
    /// [`ForwardInfo`](crate::ForwardInfo) serialized as JSON; see
    /// `media_metadata_json`.
    pub(crate) forwarded_from_json: Option<Vec<u8>>,
    /// Forward-compatible extension slots — `(tag, bytes)`. Empty in v1.
    pub(crate) ext: Vec<(u16, Vec<u8>)>,
}

/// Frozen wire mapping for [`MessagePriority`]. Must never change.
pub(crate) fn priority_to_u8(p: MessagePriority) -> u8 {
    match p {
        MessagePriority::Low => 0,
        MessagePriority::Medium => 1,
        MessagePriority::High => 2,
        MessagePriority::Critical => 3,
    }
}

/// Inverse of [`priority_to_u8`]. Unknown values fall back to the default
/// (`Medium`) so a frame from a future peer is delivered, not dropped.
pub(crate) fn priority_from_u8(v: u8) -> MessagePriority {
    match v {
        0 => MessagePriority::Low,
        1 => MessagePriority::Medium,
        2 => MessagePriority::High,
        3 => MessagePriority::Critical,
        _ => MessagePriority::Medium,
    }
}

/// Frozen wire mapping for [`ContentType`]. Must never change; append new
/// variants at the end.
pub(crate) fn content_type_to_u8(c: ContentType) -> u8 {
    match c {
        ContentType::Text => 0,
        ContentType::Image => 1,
        ContentType::Video => 2,
        ContentType::Audio => 3,
        ContentType::VoiceNote => 4,
        ContentType::VideoNote => 5,
        ContentType::File => 6,
        ContentType::FileChunk => 7,
        ContentType::Poll => 8,
    }
}

/// Inverse of [`content_type_to_u8`]. Unknown values fall back to `File`,
/// matching [`ContentType::parse`]'s treatment of unrecognized strings.
pub(crate) fn content_type_from_u8(v: u8) -> ContentType {
    match v {
        0 => ContentType::Text,
        1 => ContentType::Image,
        2 => ContentType::Video,
        3 => ContentType::Audio,
        4 => ContentType::VoiceNote,
        5 => ContentType::VideoNote,
        6 => ContentType::File,
        7 => ContentType::FileChunk,
        8 => ContentType::Poll,
        _ => ContentType::File,
    }
}

/// Encodes a [`WireMessageV1`] to a magic-prefixed binary frame.
pub(crate) fn encode(wire: &WireMessageV1) -> crate::Result<Vec<u8>> {
    let body = postcard::to_allocvec(wire)
        .map_err(|e| crate::Error::SerializationError(format!("wire v1 encode: {e}")))?;
    let mut out = Vec::with_capacity(body.len() + 1);
    out.push(WIRE_V1_MAGIC);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decodes a magic-prefixed binary frame into a [`WireMessageV1`].
///
/// Returns an error if the leading magic byte is absent or does not match
/// [`WIRE_V1_MAGIC`]; callers sniff the first byte before dispatching here.
pub(crate) fn decode(data: &[u8]) -> crate::Result<WireMessageV1> {
    match data.split_first() {
        Some((&WIRE_V1_MAGIC, body)) => postcard::from_bytes::<WireMessageV1>(body)
            .map_err(|e| crate::Error::DeserializationError(format!("wire v1 decode: {e}"))),
        Some((other, _)) => Err(crate::Error::DeserializationError(format!(
            "not a wire v1 frame: unexpected leading byte 0x{other:02X}"
        ))),
        None => Err(crate::Error::DeserializationError(
            "not a wire v1 frame: empty input".to_string(),
        )),
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::types::LAMPORT_CLOCK_MAX;
    use crate::{
        AppId, ForwardInfo, LamportClock, MediaMetadata, Message, MessageId, ReplyContext,
        Timestamp, UserId, MAX_ID_LEN, TTL,
    };

    fn sample_message() -> Message {
        let mut m = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("com.example.chat").unwrap(),
            "hello world",
        );
        m.priority = MessagePriority::High;
        m.ttl = TTL::new(5).unwrap();
        m.content_type = ContentType::Image;
        m.binary_content = Some(vec![0, 1, 2, 3, 255, 254]);
        m.lamport_clock = LamportClock::from_value(42);
        m.metadata.insert("k1".into(), "v1".into());
        m.metadata.insert("k2".into(), "v2".into());
        m.requires_ack = false;
        m.reply_to_msg = Some(MessageId::new());
        m.media_metadata = Some(MediaMetadata {
            mime_type: "image/png".into(),
            file_name: "x.png".into(),
            file_size: 1234,
            duration_ms: None,
            width: Some(640),
            height: Some(480),
            thumbnail_base64: None,
            media_id: Some("media-1".into()),
            download_url: Some("https://cdn.example/media-1".into()),
            thumbnail_url: None,
            encryption_key: Some("a2V5".into()),
            iv: None,
            ciphertext_hash: None,
            sticker_provider: None,
            sticker_remote_id: None,
            sticker_kind: None,
        });
        m.forwarded_from = Some(ForwardInfo {
            original_sender: UserId::new("carol").unwrap(),
            original_message_id: MessageId::new(),
            original_timestamp: Timestamp::from_millis(1_700_000_000_000),
            forward_count: 2,
        });
        m
    }

    /// A DTO with all-valid, minimal fields, for tampering in individual tests.
    fn base_dto() -> WireMessageV1 {
        WireMessageV1 {
            id: [7u8; 16],
            sender: "alice".into(),
            recipient: "bob".into(),
            app_id: "app".into(),
            priority: 1,
            ttl: 8,
            hop_count: 0,
            timestamp: 1,
            lamport_clock: 0,
            content_type: 0,
            content: "hi".into(),
            binary_content: None,
            media_metadata_json: None,
            metadata: Vec::new(),
            requires_ack: true,
            reply_to_msg: None,
            forwarded_from_json: None,
            ext: Vec::new(),
        }
    }

    #[test]
    fn wire_v1_round_trips_all_fields() {
        let original = sample_message();
        let bytes = original.to_wire_v1_bytes().unwrap();
        assert_eq!(bytes[0], WIRE_V1_MAGIC);
        let decoded = Message::from_wire_v1_bytes(&bytes).unwrap();

        assert_eq!(decoded.id, original.id);
        assert_eq!(decoded.sender, original.sender);
        assert_eq!(decoded.recipient, original.recipient);
        assert_eq!(decoded.app_id, original.app_id);
        assert_eq!(decoded.priority, original.priority);
        assert_eq!(decoded.ttl, original.ttl);
        assert_eq!(decoded.hop_count, original.hop_count);
        assert_eq!(decoded.timestamp, original.timestamp);
        assert_eq!(decoded.lamport_clock, original.lamport_clock);
        assert_eq!(decoded.content_type, original.content_type);
        assert_eq!(decoded.content, original.content);
        assert_eq!(decoded.binary_content, original.binary_content);
        assert_eq!(decoded.metadata, original.metadata);
        assert_eq!(decoded.requires_ack, original.requires_ack);
        assert_eq!(decoded.reply_to_msg, original.reply_to_msg);
        assert_eq!(decoded.forwarded_from, original.forwarded_from);
        // MediaMetadata has no PartialEq; compare via its JSON form.
        assert_eq!(
            serde_json::to_vec(&decoded.media_metadata).unwrap(),
            serde_json::to_vec(&original.media_metadata).unwrap(),
        );
        // transport_peer_id is never carried on the wire.
        assert!(decoded.transport_peer_id().is_none());
        // A decoded frame carries no wire-codec stamp: it defaults back to JSON
        // so a relaying node re-negotiates the codec for its *next* hop rather
        // than blindly re-emitting binary to a possibly-legacy peer.
        assert_eq!(decoded.wire_codec(), crate::WireCodec::Json);
    }

    #[test]
    fn wire_v1_round_trips_minimal_defaults() {
        let original = Message::new(
            UserId::new("a").unwrap(),
            UserId::new("b").unwrap(),
            AppId::new("c").unwrap(),
            "hi",
        );
        let decoded = Message::from_wire_v1_bytes(&original.to_wire_v1_bytes().unwrap()).unwrap();
        assert_eq!(decoded.id, original.id);
        // Default `requires_ack = true` survives a round trip.
        assert!(decoded.requires_ack);
        assert!(decoded.binary_content.is_none());
        assert!(decoded.media_metadata.is_none());
        assert!(decoded.forwarded_from.is_none());
        assert!(decoded.reply_to_msg.is_none());
        assert!(decoded.metadata.is_empty());
    }

    #[test]
    fn wire_v1_is_much_smaller_than_json() {
        let m = Message::new(
            UserId::new("a3f8c2d1-user-alice").unwrap(),
            UserId::new("b7e4d9a2-user-bob").unwrap(),
            AppId::new("com.example.chat").unwrap(),
            "hi",
        );
        let json = m.to_bytes().unwrap();
        let binary = m.to_wire_v1_bytes().unwrap();
        assert!(
            binary.len() * 2 < json.len(),
            "binary {} should be < half of json {}",
            binary.len(),
            json.len()
        );
    }

    #[test]
    fn wire_v1_preserves_identifier_length_validation() {
        // A binary frame carrying an over-long sender must be rejected exactly as
        // the JSON custom-Deserialize path rejects it.
        let mut wire = base_dto();
        wire.sender = "a".repeat(MAX_ID_LEN + 1);
        let bytes = encode(&wire).unwrap();
        assert!(Message::from_wire_v1_bytes(&bytes).is_err());
    }

    #[test]
    fn wire_v1_preserves_identifier_char_validation() {
        let mut wire = base_dto();
        wire.recipient = "evil/path".into();
        let bytes = encode(&wire).unwrap();
        assert!(Message::from_wire_v1_bytes(&bytes).is_err());
    }

    #[test]
    fn wire_v1_clamps_adversarial_lamport_clock() {
        let mut wire = base_dto();
        wire.lamport_clock = u64::MAX;
        let bytes = encode(&wire).unwrap();
        let decoded = Message::from_wire_v1_bytes(&bytes).unwrap();
        assert_eq!(decoded.lamport_clock.value(), LAMPORT_CLOCK_MAX);
    }

    #[test]
    fn wire_v1_tolerates_unknown_enum_discriminants() {
        let mut wire = base_dto();
        wire.content_type = 200;
        wire.priority = 200;
        let bytes = encode(&wire).unwrap();
        let decoded = Message::from_wire_v1_bytes(&bytes).unwrap();
        assert_eq!(decoded.content_type, ContentType::File);
        assert_eq!(decoded.priority, MessagePriority::Medium);
    }

    #[test]
    fn wire_v1_round_trips_poll_content_type() {
        let mut wire = base_dto();
        wire.content_type = content_type_to_u8(ContentType::Poll);
        let bytes = encode(&wire).unwrap();
        let decoded = Message::from_wire_v1_bytes(&bytes).unwrap();
        assert_eq!(decoded.content_type, ContentType::Poll);
    }

    #[test]
    fn wire_v1_decode_rejects_non_v1_frames() {
        assert!(decode(&[0x7B, 0x00]).is_err()); // a JSON '{'
        assert!(decode(&[]).is_err()); // empty
    }

    #[test]
    fn b64_tail_split_and_reconstruct_is_byte_identical() {
        // 50 raw bytes -> 68 base64 chars incl. one '=' pad.
        let raw: Vec<u8> = (0u8..50).collect();
        let content = format!("__MLS_ENC__{}", BASE64.encode(&raw));
        let (head, tail_raw) = split_b64_tail(&content).expect("tail must split");
        assert_eq!(head, "__MLS_ENC__");
        assert_eq!(tail_raw, raw);
        assert_eq!(format!("{head}{}", encode_b64_tail(&tail_raw)), content);
    }

    #[test]
    fn b64_tail_split_sheds_run_remainder_into_head_and_handles_headless() {
        // A 66-char run: the leading 2 chars shed into the head so the tail
        // stays 4-aligned, and reconstruction is still byte-identical.
        let content = format!("!{}", "A".repeat(66));
        let (head, raw) = split_b64_tail(&content).expect("aligned suffix splits");
        assert_eq!(head, "!AA");
        assert_eq!(format!("{head}{}", encode_b64_tail(&raw)), content);

        // No head at all: the whole content is the tail.
        let content = "A".repeat(64);
        let (head, raw) = split_b64_tail(&content).expect("headless splits");
        assert!(head.is_empty());
        assert_eq!(encode_b64_tail(&raw), content);
    }

    #[test]
    fn b64_tail_split_is_utf8_boundary_safe() {
        let content = format!("héllo → {}", "A".repeat(64));
        let (head, raw) = split_b64_tail(&content).expect("ascii tail after unicode head");
        assert_eq!(head, "héllo → ");
        assert_eq!(format!("{head}{}", encode_b64_tail(&raw)), content);
    }

    #[test]
    fn b64_tail_split_rejects_short_nonaligned_and_noncanonical() {
        // Too short overall.
        assert!(split_b64_tail("AAAA").is_none());
        // Run of 63: alignment trims to 60, under the 64-char minimum.
        assert!(split_b64_tail(&format!("!{}", "A".repeat(63))).is_none());
        // Legacy JSON int-array content: '}' and ']' break the run.
        let json_like = format!("__MLS_ENC__{{\"ciphertext\":[{}]}}", "57,".repeat(60));
        assert!(split_b64_tail(&json_like).is_none());
        // Non-canonical tail (trailing bits / bogus padding) must not split:
        // either the strict decode fails or the re-encode comparison does.
        assert!(split_b64_tail(&format!("!{}=", "B".repeat(63))).is_none());
        // Padding not at the very end breaks the run before it.
        assert!(split_b64_tail(&format!("{}={}", "A".repeat(64), "A".repeat(3))).is_none());
        // Only '=' beyond the minimum length: nothing decodable.
        assert!(split_b64_tail(&"=".repeat(64)).is_none());
    }

    #[test]
    fn wire_v1_round_trips_b64_tail_content_through_message() {
        let mut m = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("app").unwrap(),
            format!("__MLS_ENC__{}", BASE64.encode(vec![7u8; 90])),
        );
        m.requires_ack = true;
        let bytes = m.to_wire_v1_bytes().unwrap();

        // On the wire: content holds only the head, the raw tail rides in ext.
        let wire = decode(&bytes).unwrap();
        assert_eq!(wire.content, "__MLS_ENC__");
        assert_eq!(wire.ext.len(), 1);
        assert_eq!(wire.ext[0].0, EXT_TAG_B64_TAIL);
        assert_eq!(wire.ext[0].1, vec![7u8; 90]);

        // Decoded: byte-identical content, and the frame beats the base64 form.
        let decoded = Message::from_wire_v1_bytes(&bytes).unwrap();
        assert_eq!(decoded.content, m.content);
        assert!(bytes.len() < m.to_bytes().unwrap().len());
    }

    #[test]
    fn wire_v1_plain_content_emits_no_ext_entries() {
        let m = sample_message();
        let wire = decode(&m.to_wire_v1_bytes().unwrap()).unwrap();
        assert!(wire.ext.is_empty());
    }

    #[test]
    fn wire_v1_decode_ignores_unknown_ext_tags_and_extra_tag1_entries() {
        // Unknown tags are skipped; only the FIRST tag-1 entry is honored, so
        // a hostile frame cannot splice multiple tails into content.
        let mut wire = base_dto();
        wire.content = "head:".into();
        wire.ext = vec![
            (99, vec![1, 2, 3]),
            (EXT_TAG_B64_TAIL, b"AB".to_vec()),
            (EXT_TAG_B64_TAIL, b"ZZ".to_vec()),
        ];
        let bytes = encode(&wire).unwrap();
        let decoded = Message::from_wire_v1_bytes(&bytes).unwrap();
        assert_eq!(decoded.content, format!("head:{}", BASE64.encode(b"AB")));
    }

    fn sample_reply_context() -> ReplyContext {
        ReplyContext {
            sender: UserId::new("carol").unwrap(),
            text: "original text".into(),
            timestamp: Some(Timestamp::from_millis(1_700_000_000_000)),
            reply_media_label: Some("x.png".into()),
            reply_content_type: Some("image".into()),
        }
    }

    #[test]
    fn wire_v1_round_trips_reply_context() {
        // Fully-populated context on an otherwise rich message.
        let mut m = sample_message();
        m.reply_context = Some(sample_reply_context());
        let decoded = Message::from_wire_v1_bytes(&m.to_wire_v1_bytes().unwrap()).unwrap();
        assert_eq!(decoded.reply_context, m.reply_context);

        // Minimal context (all optional fields absent).
        let mut m = Message::new(
            UserId::new("a").unwrap(),
            UserId::new("b").unwrap(),
            AppId::new("c").unwrap(),
            "hi",
        );
        m.reply_context = Some(ReplyContext {
            sender: UserId::new("carol").unwrap(),
            text: "quoted".into(),
            timestamp: None,
            reply_media_label: None,
            reply_content_type: None,
        });
        let decoded = Message::from_wire_v1_bytes(&m.to_wire_v1_bytes().unwrap()).unwrap();
        assert_eq!(decoded.reply_context, m.reply_context);
    }

    #[test]
    fn wire_v1_reply_context_coexists_with_b64_tail() {
        let mut m = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("app").unwrap(),
            format!("__MLS_ENC__{}", BASE64.encode(vec![7u8; 90])),
        );
        m.reply_context = Some(sample_reply_context());
        let bytes = m.to_wire_v1_bytes().unwrap();

        let wire = decode(&bytes).unwrap();
        assert_eq!(wire.ext.len(), 2);
        assert_eq!(wire.ext[0].0, EXT_TAG_B64_TAIL);
        assert_eq!(wire.ext[1].0, EXT_TAG_REPLY_CONTEXT);

        let decoded = Message::from_wire_v1_bytes(&bytes).unwrap();
        assert_eq!(decoded.content, m.content);
        assert_eq!(decoded.reply_context, m.reply_context);
    }

    #[test]
    fn wire_v1_honors_only_first_reply_context_entry() {
        // A hostile frame cannot override an earlier reply context with a
        // spliced later entry.
        let mut wire = base_dto();
        wire.ext = vec![
            (
                EXT_TAG_REPLY_CONTEXT,
                br#"{"sender":"carol","text":"first"}"#.to_vec(),
            ),
            (
                EXT_TAG_REPLY_CONTEXT,
                br#"{"sender":"mallory","text":"second"}"#.to_vec(),
            ),
        ];
        let decoded = Message::from_wire_v1_bytes(&encode(&wire).unwrap()).unwrap();
        let rc = decoded.reply_context.expect("first entry honored");
        assert_eq!(rc.sender.as_str(), "carol");
        assert_eq!(rc.text, "first");
    }

    #[test]
    fn wire_v1_rejects_malformed_reply_context_ext() {
        // Parity with the JSON path: a malformed `reply_context` rejects the
        // whole message rather than being silently dropped.
        let mut wire = base_dto();
        wire.ext = vec![(EXT_TAG_REPLY_CONTEXT, b"not json".to_vec())];
        assert!(Message::from_wire_v1_bytes(&encode(&wire).unwrap()).is_err());

        // Valid JSON, but a sender that fails UserId validation.
        let mut wire = base_dto();
        wire.ext = vec![(
            EXT_TAG_REPLY_CONTEXT,
            br#"{"sender":"evil/path","text":"t"}"#.to_vec(),
        )];
        assert!(Message::from_wire_v1_bytes(&encode(&wire).unwrap()).is_err());
    }

    #[test]
    fn wire_v1_golden_layout_with_reply_context_ext_is_frozen() {
        // Freezes the ext-TLV encoding for tag 2 (reply context JSON). Same
        // contract as `wire_v1_golden_layout_is_frozen`: a mismatch means the
        // wire format changed — bump the magic byte, do not edit the bytes.
        let rc_json: &[u8] = br#"{"sender":"c","text":"t"}"#;
        let mut wire = base_dto();
        wire.content = String::new();
        wire.ext = vec![(EXT_TAG_REPLY_CONTEXT, rc_json.to_vec())];
        let mut expected_tail = vec![
            0x01, // ext: len 1
            0x02, // tag 2 (varint u16)
            0x19, // value: len 25
        ];
        expected_tail.extend_from_slice(rc_json); // raw JSON bytes
        let encoded = encode(&wire).unwrap();
        assert_eq!(
            &encoded[encoded.len() - expected_tail.len()..],
            expected_tail
        );
        // And the decoder surfaces it as a validated ReplyContext.
        let decoded = Message::from_wire_v1_bytes(&encoded).unwrap();
        let rc = decoded.reply_context.expect("reply context decoded");
        assert_eq!(rc.sender.as_str(), "c");
        assert_eq!(rc.text, "t");
    }

    #[test]
    fn wire_v1_golden_layout_with_b64_tail_ext_is_frozen() {
        // Freezes the ext-TLV encoding for tag 1 (base64 content tail). Same
        // contract as `wire_v1_golden_layout_is_frozen`: a mismatch means the
        // wire format changed — bump the magic byte, do not edit the bytes.
        let mut wire = base_dto();
        wire.content = String::new();
        wire.ext = vec![(EXT_TAG_B64_TAIL, vec![0xDE, 0xAD])];
        let expected_tail: &[u8] = &[
            0x01, // ext: len 1
            0x01, // tag 1 (varint u16)
            0x02, 0xDE, 0xAD, // value: len 2 + raw bytes
        ];
        let encoded = encode(&wire).unwrap();
        assert_eq!(
            &encoded[encoded.len() - expected_tail.len()..],
            expected_tail
        );
        // And the decoder reattaches the tail as canonical base64.
        let decoded = Message::from_wire_v1_bytes(&encoded).unwrap();
        assert_eq!(decoded.content, BASE64.encode([0xDE, 0xAD]));
    }

    #[test]
    fn wire_v1_golden_layout_is_frozen() {
        // Freezes the exact postcard layout. If this assertion fails the wire
        // format has changed: bump the magic byte and negotiate a new version —
        // do NOT simply edit the expected bytes.
        let wire = WireMessageV1 {
            id: [0u8; 16],
            sender: "a".into(),
            recipient: "b".into(),
            app_id: "c".into(),
            priority: 1,
            ttl: 8,
            hop_count: 0,
            timestamp: 0,
            lamport_clock: 0,
            content_type: 0,
            content: String::new(),
            binary_content: None,
            media_metadata_json: None,
            metadata: Vec::new(),
            requires_ack: true,
            reply_to_msg: None,
            forwarded_from_json: None,
            ext: Vec::new(),
        };
        let expected: &[u8] = &[
            0xF5, // magic / version
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // id (16 raw bytes)
            0x01, b'a', // sender: len 1 + "a"
            0x01, b'b', // recipient: len 1 + "b"
            0x01, b'c', // app_id: len 1 + "c"
            0x01, // priority
            0x08, // ttl
            0x00, // hop_count
            0x00, // timestamp (zigzag varint 0)
            0x00, // lamport_clock (varint 0)
            0x00, // content_type
            0x00, // content: len 0
            0x00, // binary_content: None
            0x00, // media_metadata_json: None
            0x00, // metadata: len 0
            0x01, // requires_ack: true
            0x00, // reply_to_msg: None
            0x00, // forwarded_from_json: None
            0x00, // ext: len 0
        ];
        assert_eq!(encode(&wire).unwrap(), expected);
    }
}

/// The frozen conformance vectors for the binary wire v1 frame.
///
/// The chapter these pin is `docs/spec/wire-format.md`. The vectors were
/// computed from the primitive encoding table in that chapter by
/// `tools/spec-vectors/generate.py`, a second implementation that never reads
/// this crate, rather than by running the codec below: a vector generated from
/// `encode` would agree with any format this crate happened to emit, including
/// a wrong one.
///
/// When one of these fails the wire format has changed. That needs a new magic
/// byte and a negotiated version, not an edited expectation: editing the
/// expected value to make a test pass converts a caught break into a shipped
/// one.
#[cfg(all(test, feature = "std"))]
mod spec_vectors {
    use super::*;
    use crate::types::LAMPORT_CLOCK_MAX;
    use crate::{AppId, ContentType, MessageId, MessagePriority, Timestamp, UserId, MAX_ID_LEN};
    use crate::{HopCount, LamportClock, Message, ReplyContext, TTL};

    const VECTORS: &str = include_str!("../tests/data/wire-v1.vectors.json");

    fn vectors() -> serde_json::Value {
        serde_json::from_str(VECTORS).expect("the vector file is JSON")
    }

    fn unhex(s: &str) -> Vec<u8> {
        assert!(s.len() % 2 == 0, "hex has an even length");
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex digit"))
            .collect()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Builds the message a `frames` case describes.
    ///
    /// Every field is taken from the vector rather than defaulted, so a field
    /// the generator stops emitting fails here instead of silently encoding
    /// this crate's own default on both sides of the comparison.
    fn message_from(spec: &serde_json::Value) -> Message {
        let s = |k: &str| spec[k].as_str().expect("a string field").to_string();
        let mut m = Message::from_parts(
            MessageId::from_str(&s("id")).expect("a uuid"),
            UserId::new(s("sender")).expect("a sender"),
            UserId::new(s("recipient")).expect("a recipient"),
            AppId::new(s("app_id")).expect("an app id"),
            s("content"),
            Timestamp::from_millis(spec["timestamp_ms"].as_i64().expect("an instant")),
        );

        m.priority = match spec["priority"].as_str().expect("a priority") {
            "low" => MessagePriority::Low,
            "medium" => MessagePriority::Medium,
            "high" => MessagePriority::High,
            "critical" => MessagePriority::Critical,
            other => panic!("the vector names a priority that does not exist: {other}"),
        };
        m.ttl = TTL::from_value(spec["ttl"].as_u64().expect("a ttl") as u8);
        m.hop_count = HopCount::from_value(spec["hop_count"].as_u64().expect("a hop count") as u8);
        m.lamport_clock =
            LamportClock::from_value(spec["lamport_clock"].as_u64().expect("a lamport clock"));
        m.content_type = ContentType::parse(spec["content_type"].as_str().expect("a content type"));
        m.requires_ack = spec["requires_ack"].as_bool().expect("requires_ack");

        if let Some(h) = spec["binary_content_hex"].as_str() {
            m.binary_content = Some(unhex(h));
        }
        if let Some(j) = spec["media_metadata_json"].as_str() {
            m.media_metadata = Some(serde_json::from_str(j).expect("media metadata"));
        }
        if let Some(j) = spec["forwarded_from_json"].as_str() {
            m.forwarded_from = Some(serde_json::from_str(j).expect("forward attribution"));
        }
        if let Some(j) = spec["reply_context_json"].as_str() {
            m.reply_context = Some(serde_json::from_str::<ReplyContext>(j).expect("reply context"));
        }
        if let Some(id) = spec["reply_to_msg"].as_str() {
            m.reply_to_msg = Some(MessageId::from_str(id).expect("a reply id"));
        }
        for (k, v) in spec["metadata"].as_object().expect("a metadata map") {
            m.metadata
                .insert(k.clone(), v.as_str().expect("a metadata value").to_string());
        }
        m
    }

    /// Asserts the file still carries what it carried, before anything iterates
    /// it.
    ///
    /// Every test below loops over an array, and a loop over an array that a
    /// bad merge emptied passes by not running. This is the only assertion here
    /// that fails when the file is truncated rather than wrong.
    #[test]
    fn the_vector_file_is_the_size_it_was() {
        let v = vectors();
        assert_eq!(v["magic"], "f5");
        assert_eq!(v["frames"].as_array().expect("frames").len(), 18);
        assert_eq!(v["decode_only"].as_array().expect("decode_only").len(), 6);
        assert_eq!(v["rejects"].as_array().expect("rejects").len(), 5);
    }

    #[test]
    fn every_frame_encodes_to_its_vector() {
        for case in vectors()["frames"].as_array().expect("frames") {
            let name = case["name"].as_str().expect("a name");
            let encoded = message_from(&case["message"])
                .to_wire_v1_bytes()
                .unwrap_or_else(|e| panic!("[{name}] did not encode: {e}"));
            assert_eq!(
                hex(&encoded),
                case["hex"].as_str().expect("expected hex"),
                "[{name}] encoded to different bytes than the chapter specifies"
            );
        }
    }

    /// Every frame vector decodes back into the message it was built from.
    ///
    /// Encoding alone would pass for a codec whose decoder disagreed with it,
    /// which is the shape a positional format fails in: a reordered field
    /// round-trips against itself perfectly.
    #[test]
    fn every_frame_decodes_back_into_its_message() {
        for case in vectors()["frames"].as_array().expect("frames") {
            let name = case["name"].as_str().expect("a name");
            let want = message_from(&case["message"]);
            let got = Message::from_wire_v1_bytes(&unhex(case["hex"].as_str().expect("hex")))
                .unwrap_or_else(|e| panic!("[{name}] did not decode: {e}"));

            assert_eq!(got.id, want.id, "[{name}] id");
            assert_eq!(got.sender, want.sender, "[{name}] sender");
            assert_eq!(got.recipient, want.recipient, "[{name}] recipient");
            assert_eq!(got.app_id, want.app_id, "[{name}] app_id");
            assert_eq!(got.priority, want.priority, "[{name}] priority");
            assert_eq!(got.ttl.value(), want.ttl.value(), "[{name}] ttl");
            assert_eq!(
                got.hop_count.value(),
                want.hop_count.value(),
                "[{name}] hop_count"
            );
            assert_eq!(
                got.timestamp.as_millis(),
                want.timestamp.as_millis(),
                "[{name}] timestamp"
            );
            assert_eq!(
                got.lamport_clock.value(),
                want.lamport_clock.value(),
                "[{name}] lamport_clock"
            );
            assert_eq!(got.content_type, want.content_type, "[{name}] content_type");
            assert_eq!(got.content, want.content, "[{name}] content");
            assert_eq!(
                got.binary_content, want.binary_content,
                "[{name}] binary_content"
            );
            assert_eq!(got.metadata, want.metadata, "[{name}] metadata");
            assert_eq!(got.requires_ack, want.requires_ack, "[{name}] requires_ack");
            assert_eq!(got.reply_to_msg, want.reply_to_msg, "[{name}] reply_to_msg");
            // The three that ride as embedded JSON. Their *bytes* are not
            // pinned by this suite, because they evolve under their own
            // additive rules rather than the frozen wire contract, but a
            // decoder that dropped one on the way back would otherwise pass
            // every assertion here while losing the field.
            //
            // `MediaMetadata` carries no `PartialEq`, so it is compared as
            // serialized bytes, which is what the round-trip tests above this
            // module do for the same reason.
            assert_eq!(
                serde_json::to_vec(&got.media_metadata).expect("it serializes"),
                serde_json::to_vec(&want.media_metadata).expect("it serializes"),
                "[{name}] media_metadata"
            );
            assert_eq!(
                got.forwarded_from, want.forwarded_from,
                "[{name}] forwarded_from"
            );
            assert_eq!(
                got.reply_context, want.reply_context,
                "[{name}] reply_context"
            );
        }
    }

    /// The cases no conforming encoder of this version can produce.
    ///
    /// An unknown discriminant is what a *later* sender emits, so these can
    /// only be pinned from the wire inward.
    #[test]
    fn decode_only_vectors_decode_as_specified() {
        for case in vectors()["decode_only"].as_array().expect("decode_only") {
            let name = case["name"].as_str().expect("a name");
            let got = Message::from_wire_v1_bytes(&unhex(case["hex"].as_str().expect("hex")))
                .unwrap_or_else(|e| panic!("[{name}] did not decode: {e}"));
            let expect = &case["expect"];

            if let Some(want) = expect["content_type"].as_str() {
                assert_eq!(
                    got.content_type.as_wire_str(),
                    want,
                    "[{name}] content_type"
                );
            }
            if let Some(want) = expect["priority"].as_str() {
                let got = match got.priority {
                    MessagePriority::Low => "low",
                    MessagePriority::Medium => "medium",
                    MessagePriority::High => "high",
                    MessagePriority::Critical => "critical",
                };
                assert_eq!(got, want, "[{name}] priority");
            }
            if let Some(want) = expect["lamport_clock"].as_u64() {
                assert_eq!(got.lamport_clock.value(), want, "[{name}] lamport_clock");
            }
            if let Some(want) = expect["content"].as_str() {
                assert_eq!(got.content, want, "[{name}] content");
            }
        }
    }

    #[test]
    fn reject_vectors_are_refused() {
        for case in vectors()["rejects"].as_array().expect("rejects") {
            let name = case["name"].as_str().expect("a name");
            let reason = case["reason"].as_str().expect("a reason");
            let bytes = unhex(case["hex"].as_str().expect("hex"));
            assert!(
                Message::from_wire_v1_bytes(&bytes).is_err(),
                "[{name}] was accepted, but the chapter refuses it: {reason}"
            );
        }
    }

    /// The chapter, or `None` where the repo tree is absent.
    ///
    /// Read at runtime rather than with `include_str!` because the chapter
    /// lives outside the package root: `cargo package` carries `tests/` and the
    /// vector file but cannot carry `docs/`, so compiling the path in would
    /// leave the published crate's tests unable to build at all.
    fn chapter() -> Option<String> {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/spec/wire-format.md");
        std::fs::read_to_string(&path).ok().or_else(|| {
            eprintln!("spec tree not present, skipping the wire-format chapter drift checks");
            None
        })
    }

    /// The chapter states the constants this code uses.
    ///
    /// This is the drift that would otherwise leave a correct implementation
    /// reading a stale document, which is worse than no document: the reader
    /// has no reason to doubt it.
    #[test]
    fn the_chapter_states_the_constants_the_code_uses() {
        let Some(text) = chapter() else { return };

        assert!(
            text.contains(&format!("`0x{:02X}` | Binary wire v1", WIRE_V1_MAGIC)),
            "the chapter's encoding-detection table does not state the magic byte as 0x{WIRE_V1_MAGIC:02X}"
        );
        assert!(
            text.contains(&format!(
                "{B64_TAIL_MIN_LEN} base64 characters"
            )),
            "the chapter does not state the {B64_TAIL_MIN_LEN}-character minimum for the tag 1 split"
        );
        assert!(
            text.contains(&format!("`{MAX_ID_LEN}`"))
                || text.contains(&format!("{MAX_ID_LEN}-byte")),
            "the chapter does not state the {MAX_ID_LEN}-byte identifier cap"
        );
    }

    /// The chapter states the primitive encoding at all.
    ///
    /// This is the omission the vectors exist because of: the field-order block
    /// alone reads as fixed-width to anyone who has not seen the codec, and an
    /// implementation built from it is misaligned from the first varint. A
    /// future edit that trims the table back to field order would restore that
    /// gap silently, so the presence of each rule is asserted rather than
    /// assumed.
    #[test]
    fn the_chapter_states_how_a_primitive_becomes_bytes() {
        let Some(text) = chapter() else { return };

        for required in [
            "Zigzag",
            "varint",
            "little-endian base 128",
            "no length prefix",
            // The metadata order. Byte order and code-point order are the same
            // order, so the only way to state this rule usefully is to name the
            // order that differs, and a future trim that drops the warning
            // leaves the surviving text true and useless.
            "UTF-16 code-unit order",
        ] {
            assert!(
                text.contains(required),
                "the binary chapter no longer states {required:?}, without which \
                 the field-order block cannot be implemented"
            );
        }
    }

    /// The clamp the decode vectors rely on is the one the type enforces.
    #[test]
    fn the_lamport_clamp_the_vectors_pin_is_the_types_own() {
        let v = vectors();
        let case = v["decode_only"]
            .as_array()
            .expect("decode_only")
            .iter()
            .find(|c| c["expect"]["lamport_clock"].is_u64())
            .expect("a clamp vector");
        assert_eq!(
            case["expect"]["lamport_clock"].as_u64().expect("a clamp"),
            LAMPORT_CLOCK_MAX,
            "the vector pins a ceiling the type does not enforce"
        );
    }
}
