//! The leaf node's half of the protocol, linked so it can be measured.
//!
//! The workload is deliberately the *receiving* half. A constrained node does
//! not mint messages: it takes in a frame someone else minted, validates it,
//! and answers. So this exercises decode, validate, re-encode and compare, and
//! touches none of the `std`-gated constructors.
//!
//! Every result is passed through `black_box`, without which link-time
//! optimisation deletes the entire workload and the binary measures the same
//! as `baseline`.

#![no_std]
#![no_main]
// `#[entry] fn main() -> !` has to diverge and there is nothing to park on in a
// program that is linked and never run.
#![allow(clippy::empty_loop)]

extern crate alloc;

// Links the allocator and the panic handler.
use embedded_footprint as _;

use alloc::string::ToString;
use core::hint::black_box;
use cortex_m_rt::entry;
use offline_protocol_core::{validate_id_chars, Address, Message, UserId};

/// A real message, produced by the SDK on the host and pasted here. Both
/// codecs round-trip it, which was checked before it was embedded.
const SAMPLE_JSON: &str = r#"{"id":"ea22cfba-7e3f-4820-9ba5-fe33dd9ef33e","sender":"off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s","recipient":"off1qy3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygqe5r2d","app_id":"test-app","priority":"medium","ttl":8,"hop_count":0,"timestamp":1787314332937,"lamport_clock":0,"content_type":"text","content":"hello","metadata":{},"requires_ack":true,"reply_to_msg":null}"#;

/// The sender from `SAMPLE_JSON`, canonically spelled.
const SAMPLE_ADDRESS: &str = "off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s";

#[entry]
fn main() -> ! {
    // JSON floor in, binary v1 out, binary v1 in, JSON floor out. This is the
    // whole codec surface a leaf node needs.
    if let Ok(message) = Message::from_json(black_box(SAMPLE_JSON)) {
        if let Ok(wire) = message.to_wire_v1_bytes() {
            if let Ok(decoded) = Message::from_wire_v1_bytes(black_box(&wire)) {
                if let Ok(json) = decoded.to_json() {
                    black_box(&json);
                }
                black_box(&decoded.content);
            }
            black_box(&wire);
        }
    }

    // Address canonicality: parse, re-encode, and compare byte for byte. This
    // is the check that makes an address self-certifying rather than merely
    // checksum-valid.
    if let Ok(address) = black_box(SAMPLE_ADDRESS).parse::<Address>() {
        black_box(address.to_string() == SAMPLE_ADDRESS);
        black_box(address.hash_bytes());
    }

    // Identifier policy, which every wire-supplied id passes through before it
    // is allowed to become a storage key.
    black_box(validate_id_chars(black_box("alice-123"), "User ID").is_ok());
    black_box(UserId::new(black_box(SAMPLE_ADDRESS)).is_ok());

    loop {}
}
