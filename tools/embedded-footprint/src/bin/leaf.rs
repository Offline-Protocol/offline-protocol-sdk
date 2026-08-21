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
//! package at pairing, joins from a Welcome, processes what arrives, answers,
//! and persists. No `commit_builder` appears here, deliberately: leaving the
//! commit path unlinked is part of why the number is what it is.
//!
//! Two deltas come out of this. Against `baseline` it is the whole leaf image,
//! which answers "does it fit". Against `protocol` it is what MLS costs on top
//! of the protocol layer already measured.
//!
//! # What this image is not
//!
//! It is linked and measured, never executed, exactly like the other two. The
//! MLS calls below are fed `black_box`ed bytes that are not a real Welcome and
//! not a real ciphertext, so at runtime each would return `Err`. That is sound
//! for a code-size measurement, because the optimiser cannot prove the failure
//! and links every path, and it is worthless as a functional test. Proving that
//! this stack interoperates with the phone's OpenMLS is a separate exercise
//! with a separate harness. The guard against the measurement silently hollowing
//! out is the symbol count in `measure.sh`, not this file.

#![no_std]
#![no_main]

extern crate alloc;

// Links the allocator, the panic handler, and the getrandom backend.
use embedded_footprint as _;

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::hint::black_box;
use cortex_m_rt::entry;
use offline_protocol_core::{validate_id_chars, Address, Message, UserId};

use mls_rs::identity::basic::{BasicCredential, BasicIdentityProvider};
use mls_rs::identity::SigningIdentity;
use mls_rs::{CipherSuite, CipherSuiteProvider, Client, CryptoProvider, MlsMessage};
use mls_rs_crypto_rustcrypto::RustCryptoProvider;

/// The suite the SDK pins in exactly one place
/// (`offline-protocol-mls/src/group.rs`). A leaf that negotiated anything else
/// could not talk to a phone, so the provider below is built with this one
/// enabled and the other three left out of the image.
const CIPHERSUITE: CipherSuite = CipherSuite::CURVE25519_AES128;

/// A real message, produced by the SDK on the host and pasted here. Both
/// codecs round-trip it, which was checked before it was embedded.
///
/// Duplicated from `protocol.rs` rather than shared, so that adding this binary
/// cannot move the number that binary already reports.
const SAMPLE_JSON: &str = r#"{"id":"ea22cfba-7e3f-4820-9ba5-fe33dd9ef33e","sender":"off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s","recipient":"off1qy3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygqe5r2d","app_id":"test-app","priority":"medium","ttl":8,"hop_count":0,"timestamp":1787314332937,"lamport_clock":0,"content_type":"text","content":"hello","metadata":{},"requires_ack":true,"reply_to_msg":null}"#;

/// The sender from `SAMPLE_JSON`, canonically spelled.
const SAMPLE_ADDRESS: &str = "off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s";

/// Stands in for a frame arriving off the radio. Not a valid MLS message; see
/// the module note on why that does not affect what gets linked.
const INBOUND: &[u8] = &[0u8; 128];

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

/// Everything the device does with MLS, in the order it does it.
fn mls_workload() -> Option<()> {
    // Only the pinned suite is enabled, so the other three curves never reach
    // the image.
    let crypto = RustCryptoProvider::with_enabled_cipher_suites(vec![CIPHERSUITE]);
    let suite = crypto.cipher_suite_provider(CIPHERSUITE)?;

    // The device's long-term signature key. On real hardware this is generated
    // once at provisioning and lives in whatever key storage the part offers,
    // not regenerated per boot as it is here.
    let (secret, public) = suite.signature_key_generate().ok()?;

    // The credential content is the device's own `off1` address, which is the
    // shape the SDK already requires of every leaf credential.
    let credential = BasicCredential::new(SAMPLE_ADDRESS.as_bytes().to_vec());
    let signing_identity = SigningIdentity::new(credential.into_credential(), public);

    let client: Client<_> = Client::builder()
        .identity_provider(BasicIdentityProvider)
        .crypto_provider(crypto)
        .signing_identity(signing_identity, secret, CIPHERSUITE)
        .build();

    // Pairing: the device mints one key package for the phone to consume. This
    // is the artifact a QR code must never carry, because an MLS init key is
    // single-use and a sticker is not.
    if let Ok(key_package) =
        client.generate_key_package_message(Default::default(), Default::default(), None)
    {
        if let Ok(bytes) = key_package.to_bytes() {
            black_box(&bytes);
        }
    }

    // Joining: the phone commits the Add and hands back a Welcome.
    let welcome = MlsMessage::from_bytes(black_box(INBOUND)).ok()?;
    let (mut group, info) = client.join_group(None, &welcome, None).ok()?;
    black_box(&info);

    // Steady state: open what arrives (application messages, and the commits
    // the phone issues), answer, and persist before the answer is emitted.
    if let Ok(inbound) = MlsMessage::from_bytes(black_box(INBOUND)) {
        if let Ok(received) = group.process_incoming_message(inbound) {
            black_box(&received);
        }
    }

    if let Ok(answer) = group.encrypt_application_message(black_box(b"unlocked"), Vec::new()) {
        if let Ok(bytes) = answer.to_bytes() {
            black_box(&bytes);
        }
    }

    // A leaf that emits before its ratchet state is durable will reuse an AEAD
    // nonce after a power cut. Ordering this call before the emit above is the
    // whole point of writing it down.
    group.write_to_storage().ok()?;

    Some(())
}

#[entry]
fn main() -> ! {
    protocol_workload();
    black_box(mls_workload());

    loop {}
}
