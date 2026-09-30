//! A WASI host shim for the leaf node.
//!
//! The leaf is written for firmware: it reads no clock, draws no entropy on
//! its own account, and owns no radio. Those are the host's, and on a chip the
//! host is the firmware around the crate. This example is the same crate with
//! a WebAssembly runtime as the host. The runtime hands in the time and the
//! frames, and takes back the frames the device owes, over WASI's standard
//! streams. Nothing in it is WASI-specific, so the same source also builds and
//! runs natively, which is how its output was recorded.
//!
//! # Build and run
//!
//! ```text
//! rustup target add wasm32-wasip1
//! cargo build -p offline-protocol-leaf --example wasi_host_shim --target wasm32-wasip1 --release
//! wasmtime run target/wasm32-wasip1/release/examples/wasi_host_shim.wasm \
//!     com.example.lock off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s 1787314332 < frames.jsonl
//! ```
//!
//! Natively: `cargo run -p offline-protocol-leaf --example wasi_host_shim --
//! com.example.lock <peer address> <now_unix_secs> < frames.jsonl`.
//!
//! The arguments are the application id the device is provisioned under, the
//! address of the phone it is pairing with, and the current time in seconds
//! since the epoch. Standard input carries frames from that phone, one JSON
//! message per line, and may be empty.
//!
//! # What it does
//!
//! 1. Opens a device in a [`MemoryStore`], provisioning an identity on the
//!    first run, and prints its address to standard error.
//! 2. Mints the key package frame for the peer at the supplied time and prints
//!    it to standard output, one JSON frame per line. This is the frame a
//!    device advertises to pair.
//! 3. Hands every frame on standard input to [`LeafDevice::handle`] at the
//!    same time, prints the frames it owes in reply to standard output and the
//!    events it raised to standard error. A frame the device refuses is
//!    reported and skipped, as a device keeps running when a peer sends
//!    something it does not accept. A line that is not a frame at all ends
//!    the run: that is the host's stream being wrong, not the peer's.
//! 4. At the end of input, seals one message to the peer. When no Welcome from
//!    the phone was among the frames, the crate refuses with `NoSession`, and
//!    the shim prints that refusal. A sealed frame exists only for a session
//!    that exists, and this example does not fake a peer to produce one.
//!
//! # What this deliberately does not show
//!
//! - **A pairing completing.** That takes a phone: the phone creates the group
//!   from the key package and issues the Welcome, and this crate never commits
//!   (ADR 0021). The two ends meet in the crate's `phone_interop` test and in
//!   `tools/mls-interop`.
//! - **Durable storage.** [`MemoryStore`] forgets everything at exit. A host
//!   implements [`offline_protocol_leaf::LeafStore`] over storage that is
//!   atomic per entry, because a ratchet state rolled back by a crash reuses
//!   an AEAD nonce, and the crate persists before it emits precisely so that
//!   a durable store makes that impossible.
//! - **A clock.** WASI has one (`clock_time_get`), and the leaf never reads
//!   it. The time is an argument here because it is the host's obligation on
//!   every target: a device that lets the MLS library find a clock it does not
//!   have stamps 1970 and has its key package refused as expired.
//! - **Authorization.** A session proves which address a peer is, never that
//!   the owner meant them. Deciding when to accept a pairing and what a peer
//!   may actuate is the host's, on a runtime as on a chip.
//!
//! # Entropy
//!
//! On WASI, `getrandom` reads the runtime's `random_get`. The device's
//! identity and every MLS key are exactly as strong as what that call returns,
//! and nothing in the crate can tell a strong source from a weak one. A
//! runtime configured to seed `random_get` deterministically, as some do for
//! reproducible runs, produces a predictable identity. The provisioning
//! specification's obligation that entropy is real applies to the runtime
//! here as it applies to a measurement stub on a bench: neither may reach a
//! deployment, and the runtime's source is what has to be audited first.
//!
//! # Why WASI, and not `wasm32-unknown-unknown`
//!
//! On the browser target the pinned MLS library enables `getrandom`'s `js`
//! feature, and `getrandom` 0.2 selects `js` ahead of `custom`. A host that
//! registered its own entropy backend there would supply a symbol nothing
//! calls, and the module would import browser glue instead. On WASI the
//! backend is chosen by the target, ahead of both features, so the host's
//! entropy is what the library reads. That is the claim CI pins, and the only
//! WebAssembly claim this crate makes.
//!
//! This example needs the crate's `std` feature for its standard streams.
//! The crate's `no_std` half is built for the same target by CI, separately,
//! because a host may prefer to drive it through its own exports rather than
//! through a process.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use offline_protocol_core::Message;
use offline_protocol_leaf::{shared_store, LeafDevice, LeafError, MemoryStore};

const USAGE: &str = "usage: wasi_host_shim <app_id> <peer_address> <now_unix_secs> < frames.jsonl";

/// What the device says to the peer once a session exists.
const GREETING: &str = "hello from the leaf";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("wasi_host_shim: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [app_id, peer, now] = args.as_slice() else {
        return Err(USAGE.to_string());
    };
    let now_unix_secs: u64 = now
        .parse()
        .map_err(|_| format!("now_unix_secs must be a whole number of seconds, got {now:?}"))?;

    // `shared_store` rather than `Arc::new`: it is the one constructor that
    // builds on every target the crate supports, so it is the route to copy.
    let store = shared_store(MemoryStore::new());

    // Provisioning draws from the getrandom backend the target selects. On
    // WASI that is the runtime's `random_get`; see the header.
    let mut device =
        LeafDevice::open(store, app_id).map_err(|error| format!("opening the device: {error}"))?;
    eprintln!("address {}", device.address());

    let stdout = io::stdout();
    let mut out = stdout.lock();

    // Pairing starts with the device's key package. The time is supplied, not
    // read: with `now_unix_secs` the package's validity window is real, and
    // without it the library would stamp 1970.
    let advertisement = device
        .key_package_frame(peer, now_unix_secs)
        .map_err(|error| format!("minting the key package: {error}"))?;
    emit(&mut out, &advertisement)?;

    // Steady state: frames arrive, the device verifies each, opens it if it is
    // sealed, persists, and only then hands back what it owes in reply.
    for (index, line) in io::stdin().lock().lines().enumerate() {
        let line = line.map_err(|error| format!("reading standard input: {error}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let inbound = Message::from_json(&line)
            .map_err(|error| format!("line {}: not a frame: {error}", index + 1))?;
        match device.handle(&inbound, now_unix_secs) {
            Ok(handled) => {
                for event in &handled.events {
                    eprintln!("event {event:?}");
                }
                for frame in &handled.outbound {
                    emit(&mut out, frame)?;
                }
            }
            Err(error) => eprintln!("line {}: refused: {error}", index + 1),
        }
    }

    // Answering. The persist is inside `seal`, before it returns anything, so
    // there is no ordering here to get wrong. Without the phone's Welcome
    // there is no session, and the crate says so rather than sealing into a
    // group that does not exist.
    match device.seal(peer, GREETING, now_unix_secs) {
        Ok(frame) => emit(&mut out, &frame),
        Err(LeafError::NoSession(_)) => {
            eprintln!(
                "no session with {peer}: nothing sealed. A session exists only after the \
                 phone's Welcome, and no frame on standard input carried one."
            );
            Ok(())
        }
        Err(error) => Err(format!("sealing to {peer}: {error}")),
    }
}

/// Writes one frame as one line of JSON.
fn emit(out: &mut impl Write, frame: &Message) -> Result<(), String> {
    let json = frame
        .to_json()
        .map_err(|error| format!("encoding a frame: {error}"))?;
    writeln!(out, "{json}").map_err(|error| format!("writing standard output: {error}"))
}
