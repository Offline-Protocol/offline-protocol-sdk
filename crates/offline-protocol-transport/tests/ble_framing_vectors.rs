//! Conformance vectors for the Bluetooth LE fragment framing.
//!
//! The chapter these pin is `docs/spec/ble-framing.md`. The vectors are
//! computed from the format definition in that chapter rather than by running
//! the code below, because two copies of one mistake agree perfectly: a vector
//! generated from `encode_fragment` would pass against any format this crate
//! happened to emit, including a wrong one.
//!
//! A failure here means the wire format moved. That needs a new version byte
//! and a new vector file, not an edited expectation: editing the expected value
//! to make a test pass converts a caught break into a shipped one.
//!
//! The second test in this file pins the constants the spec chapter states
//! against the constants the code uses, which is the drift that would otherwise
//! leave a correct implementation reading a stale document.

use offline_protocol_core::{AppId, Message, MessagePriority, UserId, TTL};
use offline_protocol_transport::ble::BleTransport;
use offline_protocol_transport::ble::PeerDevice;
use offline_protocol_transport::constants::{
    BLE_FRAGMENT_TIMEOUT_SECS, BLE_MAX_FRAGMENT_ASSEMBLIES, BLE_MAX_FRAGMENT_COUNT,
    BLE_MAX_FRAGMENT_SIZE, DEFAULT_MAX_MESSAGE_SIZE, FRAGMENT_HEADER_FIXED, FRAGMENT_VERSION,
    MAX_REASONABLE_BLE_PAYLOAD,
};
use offline_protocol_transport::Transport;
use serde_json::Value;

const VECTORS: &str = include_str!("data/ble-framing-v1.vectors.json");

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

fn frames(case: &Value) -> Vec<Vec<u8>> {
    case["frames"]
        .as_array()
        .expect("a case carries a frames array")
        .iter()
        .map(|f| unhex(f.as_str().expect("a frame is a hex string")))
        .collect()
}

fn name(case: &Value) -> &str {
    case["name"].as_str().unwrap_or("<unnamed>")
}

fn transport() -> BleTransport {
    BleTransport::new("vectors-device")
}

/// The spec chapter, or `None` where the repo tree is absent.
///
/// Read at runtime rather than with `include_str!` because the chapter lives
/// outside the package root: `cargo package` carries `tests/` and the vector
/// file but cannot carry `docs/`, so compiling the path in would leave the
/// published crate's tests unable to build at all.
fn chapter() -> Option<String> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/spec/ble-framing.md");
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(_) => {
            eprintln!("spec tree not present, skipping the BLE framing chapter drift checks");
            None
        }
    }
}

/// Every reassembly vector produces exactly one message, on its last frame.
#[test]
fn reassembly_vectors_reassemble() {
    let v = vectors();
    let cases = v["reassembly"].as_array().expect("reassembly cases");
    assert!(
        !cases.is_empty(),
        "the vector file carries reassembly cases"
    );

    for case in cases {
        // A fresh transport per case: assemblies are keyed by the header's
        // message id, which the vectors deliberately reuse across cases.
        let t = transport();
        let all = frames(case);
        let (last, leading) = all.split_last().expect("a case has at least one frame");

        for (i, frame) in leading.iter().enumerate() {
            let outcome = t
                .process_fragment(frame)
                .unwrap_or_else(|e| panic!("[{}] frame {i} was refused: {e}", name(case)));
            assert!(
                outcome.is_none(),
                "[{}] frame {i} completed the message early",
                name(case)
            );
        }

        let message = t
            .process_fragment(last)
            .unwrap_or_else(|e| panic!("[{}] the final frame was refused: {e}", name(case)))
            .unwrap_or_else(|| {
                panic!(
                    "[{}] the final frame did not complete a message",
                    name(case)
                )
            });

        assert_eq!(
            message.content,
            case["message_content"].as_str().expect("message_content"),
            "[{}] reassembled content",
            name(case)
        );
    }
}

/// Every refusal vector is refused, and refused on the frame the vector names
/// rather than merely somewhere in the sequence.
#[test]
fn refusal_vectors_are_refused() {
    let v = vectors();
    let cases = v["refusals"].as_array().expect("refusal cases");
    assert!(!cases.is_empty(), "the vector file carries refusal cases");

    for case in cases {
        let t = transport();
        let all = frames(case);
        let (last, leading) = all.split_last().expect("a case has at least one frame");

        // Everything before the offending frame is well formed and must be
        // accepted, otherwise the case is proving the wrong refusal.
        for (i, frame) in leading.iter().enumerate() {
            assert!(
                t.process_fragment(frame).is_ok(),
                "[{}] setup frame {i} was refused; the case does not isolate its refusal",
                name(case)
            );
        }

        let err = match t.process_fragment(last) {
            Err(e) => e.to_string().to_lowercase(),
            Ok(_) => panic!(
                "[{}] the frame was accepted; it must be refused",
                name(case)
            ),
        };

        let reason = case["reason"]
            .as_str()
            .expect("a refusal case names a reason")
            .to_lowercase();
        assert!(
            err.contains(&reason),
            "[{}] refused for {err:?}, which does not name {reason:?}",
            name(case)
        );
    }
}

/// The sizing arithmetic the chapter states, against a real message and a real
/// per-peer MTU.
///
/// The vectors carry the formula and one worked example; this recomputes both
/// rather than asserting a stored fragment count, so a message whose serialized
/// length shifts does not turn into an edited expectation.
#[test]
fn sizing_follows_the_stated_arithmetic() {
    let v = vectors();
    let sizing = &v["sizing"];
    let mtu = sizing["worked_example"]["mtu"].as_u64().unwrap() as usize;
    let uuid_id_len = sizing["uuid_id_len"].as_u64().unwrap() as usize;
    let expected_max_payload = sizing["worked_example"]["max_fragment_payload"]
        .as_u64()
        .unwrap() as usize;

    assert_eq!(
        mtu - FRAGMENT_HEADER_FIXED - uuid_id_len,
        expected_max_payload,
        "the worked example disagrees with max_fragment_payload = mtu - header - id_len"
    );

    let t = transport();
    let peer = "peer-sizing";
    t.on_peer_discovered(PeerDevice {
        device_id: peer.to_string(),
        address: "AA:BB:CC:DD:EE:FF".to_string(),
        rssi: -60,
        last_seen: std::time::SystemTime::now(),
        connected: false,
    });
    t.set_peer_mtu(peer, mtu);

    let message = Message::builder(
        UserId::new("alice").unwrap(),
        UserId::new(peer).unwrap(),
        AppId::new("app").unwrap(),
    )
    .content("x".repeat(600))
    .priority(MessagePriority::Medium)
    .ttl(TTL::new(8).unwrap())
    .build();

    // A message id is a hyphenated UUID on this encoding, which is what makes
    // the per-fragment overhead 10 + 36 rather than 10.
    assert_eq!(
        message.id.as_str().len(),
        uuid_id_len,
        "a message id is not the length the sizing example assumes"
    );

    let serialized = t.serialize_message(&message).expect("serializes");
    let fragments = t
        .fragment_message_for(peer, &message)
        .expect("fragments under the peer MTU");

    let max_payload = mtu - FRAGMENT_HEADER_FIXED - uuid_id_len;
    assert_eq!(
        fragments.len(),
        serialized.len().div_ceil(max_payload),
        "fragment count is not ceil(payload / max_fragment_payload)"
    );

    // Every fragment fits the MTU, and every one but the last is full: a
    // short fragment in the middle would mean the receiver's offsets are
    // implicit in arrival rather than in the index.
    for (i, fragment) in fragments.iter().enumerate() {
        assert!(
            fragment.len() <= mtu,
            "fragment {i} is {} bytes, above the {mtu}-byte MTU",
            fragment.len()
        );
        if i + 1 < fragments.len() {
            assert_eq!(
                fragment.len(),
                mtu,
                "fragment {i} is short but is not the last"
            );
        }
    }

    // And the whole thing round-trips back through the receiver.
    let mut reassembled = None;
    for fragment in &fragments {
        if let Some(m) = t.process_fragment(fragment).expect("a fragment we emitted") {
            reassembled = Some(m);
        }
    }
    assert_eq!(
        reassembled
            .expect("the fragments complete a message")
            .content,
        message.content
    );
}

/// The chapter's constants table must state what the code uses.
///
/// A spec that drifts from the code is worse than no spec: an implementer who
/// follows it produces a device that fails in the field rather than at the
/// document. The table is parsed out of the chapter so a value edited in one
/// place and not the other fails here.
///
#[test]
fn the_chapter_states_the_constants_the_code_uses() {
    let Some(chapter) = chapter() else {
        return;
    };

    let expected: &[(&str, String)] = &[
        ("FRAGMENT_VERSION", FRAGMENT_VERSION.to_string()),
        ("FRAGMENT_HEADER_FIXED", FRAGMENT_HEADER_FIXED.to_string()),
        ("BLE_MAX_FRAGMENT_SIZE", BLE_MAX_FRAGMENT_SIZE.to_string()),
        (
            "MAX_REASONABLE_BLE_PAYLOAD",
            MAX_REASONABLE_BLE_PAYLOAD.to_string(),
        ),
        ("BLE_MAX_FRAGMENT_COUNT", BLE_MAX_FRAGMENT_COUNT.to_string()),
        (
            "BLE_MAX_FRAGMENT_ASSEMBLIES",
            BLE_MAX_FRAGMENT_ASSEMBLIES.to_string(),
        ),
        (
            "BLE_FRAGMENT_TIMEOUT_SECS",
            BLE_FRAGMENT_TIMEOUT_SECS.to_string(),
        ),
        (
            "DEFAULT_MAX_MESSAGE_SIZE",
            DEFAULT_MAX_MESSAGE_SIZE.to_string(),
        ),
    ];

    for (constant, value) in expected {
        let row = chapter
            .lines()
            .find(|line| line.contains(constant) && line.starts_with('|'))
            .unwrap_or_else(|| {
                panic!("the chapter has no constants-table row naming `{constant}`")
            });
        assert!(
            row.contains(&format!("`{value}`")),
            "the chapter's `{constant}` row does not state `{value}`: {row}"
        );
    }
}

/// The chapter states the largest message this radio can actually carry, and
/// that number is computed rather than measured, so nothing but a test keeps it
/// true.
///
/// `BLE_MAX_FRAGMENT_COUNT` and `DEFAULT_MAX_MESSAGE_SIZE` are both ceilings on
/// one message, and which of them binds is the whole point of the paragraph:
/// a payload the message layer accepts can still be unsendable here. Lower the
/// fragment cap or raise the floor and the stated bytes go stale silently,
/// which is the same drift the constants table guard exists to catch.
#[test]
fn the_chapter_states_the_ceiling_this_radio_has() {
    let Some(chapter) = chapter() else {
        return;
    };

    let uuid_id_len = vectors()["sizing"]["uuid_id_len"].as_u64().unwrap_or(0) as usize;

    for (label, mtu) in [
        ("floor", BLE_MAX_FRAGMENT_SIZE),
        ("clamp", MAX_REASONABLE_BLE_PAYLOAD),
    ] {
        let ceiling = BLE_MAX_FRAGMENT_COUNT * (mtu - FRAGMENT_HEADER_FIXED - uuid_id_len);
        assert!(
            chapter.contains(&ceiling.to_string()),
            "the chapter does not state the {label} ceiling of {ceiling} bytes"
        );
        // The claim the paragraph rests on: on this carrier the fragment count
        // runs out first, so quoting the 1 MiB message ceiling would mislead.
        assert!(
            ceiling < DEFAULT_MAX_MESSAGE_SIZE,
            "the {label} ceiling {ceiling} no longer binds before DEFAULT_MAX_MESSAGE_SIZE; \
             the chapter's claim that the fragment count binds first is now false"
        );
    }
}
