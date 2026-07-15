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
//! [`WireMessageV1`], a flat DTO with a fixed field order, and converts to/from
//! `Message` through the validating constructors (`UserId::new`,
//! `LamportClock::from_value`, …). That keeps the security checks the JSON path
//! enforces (identifier caps, Lamport clamp) intact on the binary path.
//!
//! # Evolution contract
//!
//! postcard is positional: reordering, removing, retyping, or inserting a field
//! silently corrupts decoding on peers running the previous layout. Therefore:
//!
//! * **Never** change [`WireMessageV1`]'s existing fields.
//! * Additive, backward-compatible data goes into [`WireMessageV1::ext`], a
//!   trailing `(tag, bytes)` list that old decoders read and ignore.
//! * A change that cannot be expressed as an `ext` entry requires a new magic
//!   byte (`0xF6` = v2) and out-of-band version negotiation.
//!
//! The numeric enum mappings ([`priority_to_u8`], [`content_type_to_u8`]) are
//! likewise a frozen wire contract.

use crate::message::{ContentType, MessagePriority};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LAMPORT_CLOCK_MAX;
    use crate::{
        AppId, ForwardInfo, LamportClock, MediaMetadata, Message, MessageId, Timestamp, UserId,
        MAX_ID_LEN, TTL,
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
    fn wire_v1_decode_rejects_non_v1_frames() {
        assert!(decode(&[0x7B, 0x00]).is_err()); // a JSON '{'
        assert!(decode(&[]).is_err()); // empty
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
