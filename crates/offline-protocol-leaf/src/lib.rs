//! A constrained device that speaks the Offline Protocol as a real peer.
//!
//! A leaf node is a door lock, a sensor, or a mains-powered relay: something
//! with a radio, a few hundred kilobytes of flash and no operating system. It
//! parses frames, validates addressing, runs RFC 9420 MLS, and holds an
//! end-to-end encrypted conversation with a phone **under the same guarantees
//! a phone gets**. Same frames, same envelope, same trust gates, no second
//! sealing path and no reduced properties.
//!
//! That is the decision in
//! [ADR 0021](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0021-a-leaf-node-speaks-mls.md),
//! and it is affordable because a phone paired with one device is a
//! **two-member group**: the ratchet tree is three nodes, per-commit cost is
//! two elliptic-curve operations, and per-message cost is symmetric only. What
//! both ends must agree on lives in
//! [`offline_protocol_sealed`], so the two MLS implementations never disagree
//! about a byte outside themselves.
//!
//! # What this crate is
//!
//! [`LeafDevice`] is a frame-level state machine, not a bag of primitives. It
//! takes an inbound [`Message`](offline_protocol_core::Message) and hands back
//! the frames to send and what happened. The choreography it implements is
//! security-critical and easy to get subtly wrong: the derive-and-compare gate
//! at every site that accepts an identity claim, the binding of a Welcome to
//! the key package this device minted for the peer that sent it (a package
//! travels unencrypted, so whoever copies one off the air satisfies every
//! other gate honestly), the confirmation that has to be a group-aware
//! decrypt, and the reset sequence that a driven rekey arrives as.
//!
//! ```
//! use offline_protocol_leaf::{LeafDevice, LeafStore, MemoryStore};
//! use std::sync::Arc;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // A real device implements `LeafStore` over its secure key storage.
//! let store: Arc<dyn LeafStore> = Arc::new(MemoryStore::new());
//! let mut device = LeafDevice::open(store, "com.example.lock")?;
//!
//! // `now` comes from the radio stack, the commissioner, or the pairing
//! // exchange. It is a parameter because a device has no clock, and an MLS
//! // implementation that reads one it does not have stamps 1970.
//! let now = 1_787_314_332;
//! let peer = device.address().to_string();
//! let advertisement = device.key_package_frame(&peer, now)?;
//! assert!(advertisement.content.starts_with("__MLS_KEY_PKG__"));
//! # Ok(())
//! # }
//! ```
//!
//! # Four obligations this crate cannot discharge for you
//!
//! **A time source at pairing.** Every entry point that needs a clock takes
//! `now_unix_secs`. A device that supplies something wrong emits a key package
//! the peer refuses as expired, and it never pairs at all. Validity is a
//! freshness bound rather than an authentication mechanism, so a wrong clock
//! costs availability, not confidentiality.
//!
//! **Real entropy.** This crate registers no `getrandom` backend, on purpose:
//! doing so would let firmware link and run with randomness this crate
//! invented, and MLS key generation is exactly as strong as what that symbol
//! returns. Wire it to the part's hardware entropy source.
//!
//! **Durable storage.** [`LeafStore`] must be durable and atomic per entry.
//! This crate orders every persist before the emit it belongs to, so a store
//! that lies is the one remaining way to reuse an AEAD nonce after a power
//! cut.
//!
//! **Authorization.** A session proves *who* a peer is and never that they may
//! do anything. Every gate in this crate answers the first question, and any
//! address in radio range can complete a pairing, because producing a key that
//! derives to its own address costs nothing. So firmware decides when the radio
//! accepts a new pairing, and firmware decides what a message from a given peer
//! may actuate, by the address on the event. A lock that opens for whatever
//! arrives on an established session opens for anyone patient enough to pair
//! with it. [`LeafDevice::peers`] is how firmware audits what a device
//! accumulated and [`LeafDevice::unpair`] is how it removes one.
//!
//! # One writer
//!
//! Every operation that advances state takes `&mut self`, so a device is one
//! value exclusively held rather than a handle to share between tasks. Two
//! seals at once would reuse an AEAD nonce without any power cut being
//! involved; see [`LeafDevice`] for that argument and for what a replayed
//! control frame can still do.
//!
//! # Bare metal
//!
//! Builds with `--no-default-features` for a target with no `std`, which is
//! the configuration the CI job gates. The `std` build is the same code with
//! its dependencies' `std` features on, and is what the unit tests run under.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

mod adapters;
mod device;
mod error;
mod frames;
mod identity;
mod keypkg;
pub mod store;

pub use device::{Handled, LeafDevice, LeafEvent};
pub use error::{LeafError, Result};
pub use identity::CIPHERSUITE;
pub use store::{LeafStore, StoreError};

#[cfg(any(test, feature = "std"))]
pub use store::MemoryStore;

#[cfg(all(test, feature = "std"))]
mod manifest_guard_tests {
    use std::string::{String, ToString};
    use std::vec::Vec;
    use std::{eprintln, format, fs, path::PathBuf};

    /// Dependencies this crate declares locally rather than inheriting.
    ///
    /// Inheriting them would silently drop `default-features = false` (the
    /// trap [ADR 0020](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0020-core-compiles-without-std.md)
    /// records), so they are spelled out here. This test is the counterweight:
    /// dropping inheritance means a workspace-wide version bump would
    /// otherwise stop applying to this crate with nothing to notice.
    const LOCAL_DEPS: &[&str] = &[
        "serde",
        "serde_json",
        "base64",
        "thiserror",
        "zeroize",
        "getrandom",
        "mls-rs",
        "mls-rs-crypto-rustcrypto",
        "mls-rs-core",
        "offline-protocol-core",
        "offline-protocol-sealed",
    ];

    fn version_req(manifest: &str, name: &str) -> Option<String> {
        for line in manifest.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix(name) else {
                continue;
            };
            let Some(rest) = rest.trim_start().strip_prefix('=') else {
                continue;
            };
            let rest = rest.trim();
            if let Some(inner) = rest.strip_prefix('{') {
                let idx = inner.find("version")?;
                let after = &inner[idx..];
                let start = after.find('"')? + 1;
                let end = after[start..].find('"')? + start;
                return Some(after[start..end].to_string());
            }
            if let Some(inner) = rest.strip_prefix('"') {
                let end = inner.find('"')?;
                return Some(inner[..end].to_string());
            }
        }
        None
    }

    #[test]
    fn local_dep_versions_match_the_workspace_table() {
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = here.join("../../Cargo.toml");
        // A packaged `.crate` archive has no workspace root beside it, and a
        // release must not fail for its absence.
        let (Ok(root), Ok(local)) = (
            fs::read_to_string(&root),
            fs::read_to_string(here.join("Cargo.toml")),
        ) else {
            eprintln!("skipping: workspace root not readable from here");
            return;
        };

        let mismatched: Vec<String> = LOCAL_DEPS
            .iter()
            .filter_map(|name| {
                let ours = version_req(&local, name)?;
                let theirs = version_req(&root, name)?;
                (ours != theirs).then(|| format!("{name}: local {ours}, workspace {theirs}"))
            })
            .collect();

        assert!(
            mismatched.is_empty(),
            "local dependency versions have drifted from the workspace table: {mismatched:?}"
        );

        for name in LOCAL_DEPS {
            assert!(
                version_req(&local, name).is_some(),
                "{name} is in LOCAL_DEPS but not declared in this crate's manifest"
            );
            assert!(
                version_req(&root, name).is_some(),
                "{name} is in LOCAL_DEPS but not in the workspace dependency table"
            );
        }
    }
}
