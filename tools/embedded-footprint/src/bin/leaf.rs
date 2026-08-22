//! A leaf node's whole image: the protocol layer plus the MLS half.
//!
//! `protocol` measures what it costs to parse and validate a frame. This adds
//! what it costs to *open* one, which ADR 0020 left open on the grounds that
//! MLS does not fit on this class of part. That claim is true of OpenMLS and of
//! large groups, and the pairing this measures is neither: a phone and one
//! device are a two-member group, where the ratchet tree is three nodes.
//!
//! The workload is the **never-committing member** profile. The phone creates
//! the group, adds the device, and issues every commit; the device mints a key
//! package at pairing, joins from a Welcome, processes what arrives, seals an
//! answer, persists, and only then emits. No `commit_builder` appears here,
//! deliberately: leaving the commit path unlinked is part of why the number is
//! what it is.
//!
//! Two deltas come out of this. Against `baseline` it is the whole leaf image,
//! which answers "does it fit". Against `protocol` it is what MLS costs on top
//! of the protocol layer already measured.
//!
//! # What this image is not
//!
//! It is linked and measured, never executed, exactly like the other two. The
//! frame handed to the device below is an ordinary text message rather than a
//! Welcome or a sealed envelope, so at runtime the interesting arms would
//! return early. That is sound for a code-size measurement, because the
//! optimiser cannot prove which arm runs and links every path, and it is
//! worthless as a functional test. Proving that this stack interoperates with
//! the phone's OpenMLS is a separate exercise with a separate harness
//! (`tools/mls-interop`, plus the in-process tests in the leaf crate itself).
//! The guard against the measurement silently hollowing out is the symbol
//! count in `measure.sh`, not this file.
//!
//! # What it measures now
//!
//! The whole of `offline-protocol-leaf`, which is the code a device runs. An
//! earlier version of this file drove mls-rs directly and therefore linked
//! neither the envelope codec, nor the control-frame signing, nor the address
//! derivation: it priced an image nobody could ship. The figure is larger for
//! that reason and is the honest one.

#![no_std]
#![no_main]
// `#[entry] fn main() -> !` has to diverge and there is nothing to park on in a
// program that is linked and never run.
#![allow(clippy::empty_loop)]

extern crate alloc;

// Links the allocator, the panic handler, and the getrandom backend.
use embedded_footprint as _;

use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hint::black_box;
use cortex_m_rt::entry;
use offline_protocol_core::{validate_id_chars, Address, Message, UserId};
use offline_protocol_leaf::{LeafDevice, LeafStore, StoreError};

/// A real message, produced by the SDK on the host and pasted here. Both
/// codecs round-trip it, which was checked before it was embedded.
///
/// Duplicated from `protocol.rs` rather than shared, so that adding this binary
/// cannot move the number that binary already reports.
const SAMPLE_JSON: &str = r#"{"id":"ea22cfba-7e3f-4820-9ba5-fe33dd9ef33e","sender":"off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s","recipient":"off1qy3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygqe5r2d","app_id":"test-app","priority":"medium","ttl":8,"hop_count":0,"timestamp":1787314332937,"lamport_clock":0,"content_type":"text","content":"hello","metadata":{},"requires_ack":true,"reply_to_msg":null}"#;

/// The sender from `SAMPLE_JSON`, canonically spelled.
const SAMPLE_ADDRESS: &str = "off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s";

/// When the device was paired, in seconds since the epoch.
///
/// This is a parameter of the workload rather than something read from a clock
/// because a bare-metal leaf has no clock to read. Handed `None`, mls-rs stamps
/// `not_before = 0`, which puts the key package's validity window in 1970 and
/// gets it refused as expired by the phone (`tools/mls-interop` proves that
/// refusal). Supplying the timestamp is therefore the shape the firmware has to
/// have, and it is why ADR 0021 makes a time source at pairing an obligation on
/// the device: on hardware this value comes from the radio stack, the
/// commissioner, or the pairing exchange. The literal is `SAMPLE_JSON`'s
/// timestamp in seconds, so this file quotes one clock and not two.
const PAIRED_AT: u64 = 1_787_314_332;

/// The protocol layer's own workload, identical to `protocol.rs`.
fn protocol_workload() {
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

    if let Ok(address) = black_box(SAMPLE_ADDRESS).parse::<Address>() {
        black_box(address.to_string() == SAMPLE_ADDRESS);
        black_box(address.hash_bytes());
    }

    black_box(validate_id_chars(black_box("alice-123"), "User ID").is_ok());
    black_box(UserId::new(black_box(SAMPLE_ADDRESS)).is_ok());
}

/// A store that holds nothing.
///
/// Enough to link every path through the crate's storage seam, and nothing a
/// device would ship: `load` always answers "not there", so the workload
/// provisions a fresh identity on every call rather than resuming one. Real
/// firmware implements this over the part's secure key storage, and owes the
/// durability and per-entry atomicity the trait documents, because that is
/// what keeps a power cut from rolling a ratchet back onto a used nonce.
struct NullStore;

impl LeafStore for NullStore {
    fn store(&self, _key_type: &str, _key_id: &str, _data: &[u8]) -> Result<(), StoreError> {
        Ok(())
    }

    fn load(&self, _key_type: &str, _key_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(None)
    }

    fn delete(&self, _key_type: &str, _key_id: &str) -> Result<(), StoreError> {
        Ok(())
    }
}

/// Everything the device does, in the order it does it.
///
/// This goes through `offline-protocol-leaf` rather than reaching for mls-rs
/// directly, which is what makes the figure below a measurement of the code a
/// device would actually run. An earlier version of this file called mls-rs
/// itself and linked neither the envelope codec nor the control-frame signing
/// nor the address derivation, so it priced an image nobody could ship.
fn mls_workload() -> Option<()> {
    let store: Arc<dyn LeafStore> = Arc::new(NullStore);

    // Provisioning draws from the getrandom backend this harness registers,
    // which is a counter. On hardware that symbol is the part's TRNG, and the
    // device's identity is exactly as strong as what it returns.
    let mut device = LeafDevice::open(store, black_box("com.example.lock")).ok()?;
    black_box(device.address());

    // Pairing: the device mints one key package and signs the frame carrying
    // it. The timestamp is supplied rather than read, because a bare-metal
    // device has no clock and the library stamps 1970 when it tries to find
    // one. See `PAIRED_AT`.
    if let Ok(advertisement) = device.key_package_frame(black_box(SAMPLE_ADDRESS), PAIRED_AT) {
        if let Ok(json) = advertisement.to_json() {
            black_box(&json);
        }
    }

    // Steady state: a frame arrives off the radio, is parsed by the protocol
    // layer, and is handed to the device, which verifies it, opens it if it is
    // sealed, persists, and hands back whatever it owes in reply.
    if let Ok(inbound) = Message::from_json(black_box(SAMPLE_JSON)) {
        if let Ok(handled) = device.handle(&inbound, PAIRED_AT) {
            black_box(&handled.events);
            for frame in &handled.outbound {
                if let Ok(json) = frame.to_json() {
                    black_box(&json);
                }
            }
        }
    }

    // Answering. The persist is inside `seal`, before it returns anything, so
    // there is no ordering here for a reader of this file to get wrong: the
    // crate does not offer the sealed bytes until the state behind them is
    // durable.
    if let Ok(answer) = device.seal(black_box(SAMPLE_ADDRESS), black_box("unlocked"), PAIRED_AT) {
        if let Ok(json) = answer.to_json() {
            black_box(&json);
        }
    }

    Some(())
}

#[entry]
fn main() -> ! {
    protocol_workload();
    black_box(mls_workload());

    loop {}
}
