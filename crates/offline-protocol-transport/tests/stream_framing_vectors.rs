//! Conformance vectors for the peer-stream framing.
//!
//! The chapter these pin is `docs/spec/stream-framing.md`. The vectors are
//! computed from the layout stated there rather than by running the reader
//! below, because two copies of one mistake agree perfectly: a vector
//! generated from `frame()` would pass against any prefix this crate happened
//! to write, including a little-endian one, which is exactly the fault the
//! chapter names as invisible to a round-trip test.
//!
//! The preamble is verified with the one real verifier, not a stand-in. The
//! vector whose message frame arrives before any preamble is refused only
//! because a binary v1 frame does not verify as an identity assertion; a fake
//! verifier that said "not an assertion" to everything would pass that case
//! for the wrong reason and prove nothing about position.
//!
//! A failure here means the framing moved. That needs a versioned preamble
//! and a new vector file, not an edited expectation: editing the expected
//! value to make a test pass converts a caught break into a shipped one.

use offline_protocol_transport::constants::{
    DEFAULT_MAX_MESSAGE_SIZE, PEER_STREAM_MAX_FRAME_BYTES, PEER_STREAM_PREAMBLE_FLOOR,
};
use offline_protocol_transport::{frame, CloseReason, PeerStreamReader, StreamEvent};
use serde_json::Value;

const VECTORS: &str = include_str!("data/stream-framing-v1.vectors.json");

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("the vector file is valid JSON")
}

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len() % 2 == 0, "hex string has an odd length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex digit"))
        .collect()
}

fn hex_field<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("the vectors carry `{key}` as a hex string"))
}

fn name(case: &Value) -> &str {
    case["name"].as_str().unwrap_or("<unnamed>")
}

/// A reader over the one real verifier, the way a host builds one.
fn reader() -> PeerStreamReader {
    PeerStreamReader::new(Box::new(|bytes| {
        offline_protocol_mls::verify_identity_assertion(bytes)
            .ok()
            .map(|address| address.to_string())
    }))
}

/// The spec chapter, or `None` where the repo tree is absent.
///
/// Read at runtime rather than with `include_str!` because the chapter lives
/// outside the package root: `cargo package` carries `tests/` and the vector
/// file but cannot carry `docs/`.
fn chapter() -> Option<String> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/spec/stream-framing.md");
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(_) => {
            eprintln!("spec tree not present, skipping the stream framing chapter drift checks");
            None
        }
    }
}

/// The preamble frame announces exactly the address the vectors name, and the
/// sender side writes exactly the bytes the vectors carry.
#[test]
fn the_preamble_announces_the_address_it_proves() {
    let v = vectors();
    let preamble = &v["preamble"];
    let body = unhex(hex_field(preamble, "body_hex"));
    let wire = unhex(hex_field(preamble, "frame_hex"));
    let address = preamble["address"].as_str().expect("address");

    assert_eq!(frame(&body).expect("frames"), wire, "the sender's prefix");

    let mut r = reader();
    assert_eq!(
        r.feed(&wire),
        vec![StreamEvent::Announced(address.to_string())]
    );
    assert_eq!(r.peer(), Some(address));
    assert!(!r.is_closed());
}

/// The exchange delivers the message body to the core attributed to the
/// address the preamble proved, and to nothing else.
#[test]
fn the_exchange_delivers_the_message_attributed_to_the_announced_peer() {
    let v = vectors();
    let exchange = &v["exchange"];
    let frames: Vec<Vec<u8>> = exchange["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .map(|f| unhex(f.as_str().expect("hex")))
        .collect();
    let announces = exchange["announces"].as_str().expect("announces");
    let delivers = exchange["delivers"].as_array().expect("delivers");
    assert_eq!(delivers.len(), 1, "the exchange carries one message");
    let expected_body = unhex(hex_field(&delivers[0], "body_hex"));
    assert_eq!(delivers[0]["from"].as_str().unwrap(), announces);

    // The message frame is the framed message body.
    assert_eq!(
        frame(&unhex(hex_field(&v["message"], "body_hex"))).unwrap(),
        unhex(hex_field(&v["message"], "frame_hex"))
    );

    let expected = vec![
        StreamEvent::Announced(announces.to_string()),
        StreamEvent::Message {
            from: announces.to_string(),
            body: expected_body,
        },
    ];

    // In one read.
    let whole: Vec<u8> = frames.concat();
    assert_eq!(reader().feed(&whole), expected);

    // One frame per read.
    let mut r = reader();
    let mut per_frame = Vec::new();
    for f in &frames {
        per_frame.extend(r.feed(f));
    }
    assert_eq!(per_frame, expected);

    // One byte per read: a stream promises nothing about where reads end.
    let mut r = reader();
    let mut per_byte = Vec::new();
    for b in &whole {
        per_byte.extend(r.feed(std::slice::from_ref(b)));
    }
    assert_eq!(per_byte, expected);
    assert!(!r.is_closed());
}

/// The prefix at exactly the ceiling is accepted, and the body it announces
/// is delivered whole: the ceiling is inclusive, and a reader that refuses
/// `>=` instead of `>` fails here.
#[test]
fn a_prefix_of_exactly_the_ceiling_is_accepted_and_its_body_delivered() {
    let v = vectors();
    let cases = v["accepted_prefixes"]
        .as_array()
        .expect("accepted_prefixes");
    assert!(!cases.is_empty());

    for case in cases {
        let prefix = unhex(hex_field(case, "prefix_hex"));
        let length = case["length"].as_u64().expect("length") as usize;
        assert_eq!(
            length,
            PEER_STREAM_MAX_FRAME_BYTES,
            "[{}] the accepted prefix is the ceiling itself",
            name(case)
        );

        let mut r = reader();
        r.feed(&unhex(hex_field(&v["preamble"], "frame_hex")));
        assert!(
            r.feed(&prefix).is_empty() && !r.is_closed(),
            "[{}] the prefix alone must be accepted and the body awaited",
            name(case)
        );

        let body = vec![0xA5u8; length];
        let events = r.feed(&body);
        assert!(
            matches!(&events[..], [StreamEvent::Message { body: b, .. }] if b.len() == length),
            "[{}] a body of exactly the ceiling must be delivered whole; got {} event(s)",
            name(case),
            events.len()
        );
        assert!(!r.is_closed());
    }
}

/// Every refusal vector closes the stream with nothing announced and nothing
/// delivered, and the close is terminal.
#[test]
fn refusal_vectors_close_the_stream_with_nothing_delivered() {
    let v = vectors();
    let cases = v["refusals"].as_array().expect("refusals");
    assert!(!cases.is_empty());

    for case in cases {
        let mut r = reader();
        let mut events = Vec::new();
        for f in case["frames"].as_array().expect("frames") {
            events.extend(r.feed(&unhex(f.as_str().expect("hex"))));
        }
        assert!(
            matches!(events.as_slice(), [StreamEvent::Closed(_)]),
            "[{}] must yield exactly one event, the close; got {events:?}",
            name(case)
        );
        assert!(r.is_closed(), "[{}] the reader is closed", name(case));
        assert_eq!(r.peer(), None, "[{}] nothing was announced", name(case));

        // Terminal: a valid preamble after the close changes nothing.
        let valid = unhex(hex_field(&v["preamble"], "frame_hex"));
        assert!(
            r.feed(&valid).is_empty(),
            "[{}] a closed stream delivers nothing more",
            name(case)
        );

        // The vectors that carry no body are refused on the prefix, which
        // the reasons make visible: nothing about a body is named.
        let StreamEvent::Closed(reason) = &events[0] else {
            unreachable!()
        };
        let carries_body = case["carries_body"].as_bool().expect("carries_body");
        if !carries_body {
            assert!(
                matches!(
                    reason,
                    CloseReason::ZeroLength | CloseReason::OverCeiling(_)
                ),
                "[{}] a prefix-only vector is refused on the prefix; got {reason:?}",
                name(case)
            );
        }
    }
}

/// The two refusals a receiver applies to a preamble specifically: under the
/// floor on the prefix, and a message frame in the preamble's position.
#[test]
fn the_preamble_refusals_are_the_ones_the_chapter_names() {
    let v = vectors();
    let by_name = |wanted: &str| -> Vec<u8> {
        let case = v["refusals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| name(c) == wanted)
            .unwrap_or_else(|| panic!("the vectors carry a refusal named {wanted:?}"));
        unhex(case["frames"][0].as_str().unwrap())
    };

    let under_floor = by_name("preamble one under the floor");
    let mut r = reader();
    // The prefix alone decides it; feed only that and the body never matters.
    assert_eq!(
        r.feed(&under_floor[..4]),
        vec![StreamEvent::Closed(CloseReason::PreambleUnderFloor(
            PEER_STREAM_PREAMBLE_FLOOR - 1
        ))]
    );

    let message_first = by_name("a message before the preamble");
    let mut r = reader();
    assert_eq!(
        r.feed(&message_first),
        vec![StreamEvent::Closed(CloseReason::PreambleRejected)],
        "a well-formed message is not a preamble: position decides"
    );
}

/// The sender refuses what a receiver would refuse on the prefix, so no
/// conforming sender ever closes a conforming receiver's stream.
#[test]
fn the_sender_refuses_what_the_receiver_refuses() {
    assert!(frame(&[]).is_err(), "zero length");
    assert!(
        frame(&vec![0u8; PEER_STREAM_MAX_FRAME_BYTES + 1]).is_err(),
        "one over the ceiling"
    );
    let at_ceiling =
        frame(&vec![0u8; PEER_STREAM_MAX_FRAME_BYTES]).expect("the ceiling is inclusive");
    assert_eq!(&at_ceiling[..4], &unhex("00100000")[..]);
}

/// The vectors and the chapter state the bounds the code uses.
///
/// The vector file carries the ceiling, the floor and the byte order as
/// values; the chapter states them in prose. Either drifting from the
/// constants leaves an implementer following a stale document, which is the
/// failure a spec exists to prevent.
#[test]
fn the_vectors_and_the_chapter_state_the_bounds_the_code_uses() {
    let v = vectors();
    assert_eq!(
        v["ceiling"].as_u64().unwrap() as usize,
        PEER_STREAM_MAX_FRAME_BYTES
    );
    assert_eq!(
        PEER_STREAM_MAX_FRAME_BYTES, DEFAULT_MAX_MESSAGE_SIZE,
        "the framing ceiling is the transport message ceiling, not a second limit"
    );
    assert_eq!(
        v["preamble_floor"].as_u64().unwrap() as usize,
        PEER_STREAM_PREAMBLE_FLOOR
    );
    assert_eq!(v["byte_order"].as_str().unwrap(), "big-endian");
    assert_eq!(v["layout"].as_str().unwrap(), "u32be(len(body)) || body");

    let Some(chapter) = chapter() else {
        return;
    };
    for (what, needle) in [
        ("the ceiling constant", "`DEFAULT_MAX_MESSAGE_SIZE`"),
        ("the ceiling in bytes", "1,048,576"),
        ("the preamble floor", "96 bytes"),
        ("the byte order", "big-endian"),
    ] {
        assert!(
            chapter.contains(needle),
            "the chapter no longer states {what} as {needle:?}"
        );
    }
}
