//! The pieces both ends of a sealed conversation must agree on.
//!
//! A phone and a leaf node run different MLS implementations (ADR 0021: the
//! phone runs OpenMLS, a Cortex-M leaf runs mls-rs), but they are not allowed
//! to disagree about anything outside them. The envelope a ciphertext travels
//! in, the derivation that turns an identity key into an address, the byte
//! string a signature is taken over, and the ratchet numbers a session is
//! configured with are all *shared* rather than *implemented twice*. This
//! crate is where they live, and it exists because those pieces previously sat
//! in crates a leaf node cannot link.
//!
//! Each piece is here for the same reason, stated as the failure a second copy
//! causes:
//!
//! - [`envelope`]: two copies of the codec are two wire formats the moment one
//!   is edited alone, and the symptom is a peer that cannot read a message.
//! - [`mod@derive`]: two derivations are two identities for one key, and every
//!   trust gate in the protocol is a derive-and-compare, so a drifted copy
//!   rejects legitimate peers or, worse, accepts a mismatched one.
//! - [`canonical`]: a verifier that rebuilds a signing payload differently
//!   from the signer either rejects every signature or, if the difference is
//!   an ambiguity rather than a mismatch, accepts a forged one.
//! - [`constants`]: a number configured differently on the two ends produces
//!   a session that silently stops decrypting under load.
//! - [`freshness`]: two ends that disagree about how old a signed control
//!   frame may be produce a pair that refuses its own traffic in one direction
//!   and accepts a replayed frame in the other.
//!
//! # Why this is not part of `offline-protocol-core`
//!
//! Core links no cryptography at all, deliberately, and address derivation is
//! a SHA-256. Core also knows nothing about MLS, and these types name it.
//! Putting them in core would push a hash implementation into every consumer
//! of the protocol's base types, including a relay-only leaf image that never
//! derives an address. See ADR 0022.
//!
//! # Building without `std`
//!
//! Like [`offline_protocol_core`], this crate compiles for bare-metal targets
//! with `--no-default-features`, and `alloc` is required in both
//! configurations. Nothing here reads a clock, draws entropy or takes a lock,
//! so unlike core the no_std configuration loses no functionality: the `std`
//! feature only forwards to dependencies.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod canonical;
pub mod constants;
pub mod derive;
pub mod envelope;
pub mod error;
pub mod freshness;
pub mod keypackage;
pub mod prefixes;

pub use canonical::{
    canonical_payload, control_signing_payload, control_signing_payload_v2, CTRL_PK_META_KEY,
    CTRL_SIGN_DOMAIN, CTRL_SIGN_DOMAIN_V2, CTRL_SIG_META_KEY,
};
pub use constants::{
    LEAF_KEY_PACKAGE_LIFETIME, LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS,
    MAX_ACCEPTED_KEY_PACKAGE_LIFETIME, SENDER_RATCHET_MAXIMUM_FORWARD_DISTANCE,
    SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE,
};
pub use derive::{derive_address, ED25519_PUBLIC_KEY_LEN};
pub use envelope::{EncryptedMessage, GroupId, MlsMessageType, WelcomeMessage};
pub use error::{Result, SealedError};
pub use freshness::{
    control_frame_freshness, Freshness, CTRL_FRESHNESS_FUTURE_MS, CTRL_FRESHNESS_PAST_MS,
    LEAF_CTRL_FRESHNESS_PAST_MS,
};
pub use keypackage::{KeyPackagePayload, CTRL_SIGN_V2, MLS_ENVELOPE_COMPACT_V1};

#[cfg(all(test, feature = "std"))]
mod manifest_guard_tests {
    /// Dependencies this crate declares locally instead of inheriting from the
    /// workspace table, and which must therefore be kept in lockstep by hand.
    /// See the comment above `[dependencies]` in this crate's `Cargo.toml` for
    /// why inheritance is not an option here.
    const LOCAL_DEPS: &[&str] = &[
        "serde",
        "serde_json",
        "base64",
        "thiserror",
        "sha2",
        "offline-protocol-core",
    ];

    /// Extracts the version requirement a manifest states for `name`.
    ///
    /// Handles both spellings cargo accepts: `name = "1.0"` and
    /// `name = { version = "1.0", ... }`.
    fn version_req(manifest: &str, name: &str) -> Option<String> {
        let line = manifest
            .lines()
            .find(|l| l.starts_with(&format!("{name} = ")))?;
        let rest = match line.find("version = \"") {
            Some(i) => &line[i + "version = \"".len()..],
            None => {
                let i = line.find('"')?;
                &line[i + 1..]
            }
        };
        rest.find('"').map(|end| rest[..end].to_string())
    }

    /// The local dependency entries must not drift from the workspace table.
    ///
    /// Dropping inheritance is what makes `default-features = false` take
    /// effect (cargo ignores it on an inherited dependency), but it also means
    /// a workspace-wide version bump silently stops applying to this crate.
    /// This test is what turns that silence into a failure.
    #[test]
    fn local_dep_versions_match_the_workspace_table() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = dir.join("../../Cargo.toml");
        // A published .crate archive contains no workspace root. Skip rather
        // than panic: this guard protects the repo, and a packaged build that
        // panics here fails the release for a condition it cannot observe.
        let Ok(root_manifest) = std::fs::read_to_string(&root) else {
            return;
        };
        let own_manifest =
            std::fs::read_to_string(dir.join("Cargo.toml")).expect("own manifest is readable");

        for dep in LOCAL_DEPS {
            let workspace = version_req(&root_manifest, dep)
                .unwrap_or_else(|| panic!("`{dep}` not found in the workspace dependency table"));
            let own = version_req(&own_manifest, dep)
                .unwrap_or_else(|| panic!("`{dep}` not found in this crate's manifest"));
            assert_eq!(
                workspace, own,
                "`{dep}` version drifted: workspace says {workspace:?}, \
                 offline-protocol-sealed says {own:?}. Update both together."
            );
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod interop_harness_guard_tests {
    /// `tools/mls-interop` is its own cargo workspace, so nothing in
    /// `cargo test --workspace` compiles it and no type check connects it to
    /// this crate. It is also the one harness whose whole purpose is to run
    /// the SDK's configuration against a second MLS implementation, which it
    /// stops doing the moment it declares its own copy of that configuration.
    ///
    /// It used to: a local `derive_address` and local ratchet constants, with
    /// a manifest comment saying nothing pinned them. This test is what
    /// replaced that comment, and it is the only thing in the workspace test
    /// run that can notice them coming back.
    #[test]
    fn the_interop_harness_uses_this_crate_rather_than_its_own_copies() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let harness = dir.join("../../tools/mls-interop");
        // Absent in a published .crate archive; see the manifest guard above.
        let (Ok(main_rs), Ok(manifest)) = (
            std::fs::read_to_string(harness.join("src/main.rs")),
            std::fs::read_to_string(harness.join("Cargo.toml")),
        ) else {
            eprintln!("tools/mls-interop not present, skipping harness copy check");
            return;
        };

        assert!(
            manifest.contains("offline-protocol-sealed"),
            "the harness must depend on offline-protocol-sealed, not restate what it declares"
        );

        for symbol in [
            "derive_address",
            "SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE",
            "SENDER_RATCHET_MAXIMUM_FORWARD_DISTANCE",
            "LEAF_KEY_PACKAGE_LIFETIME",
            "LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS",
            "MAX_ACCEPTED_KEY_PACKAGE_LIFETIME",
        ] {
            assert!(
                main_rs.contains(symbol),
                "the harness no longer references `{symbol}`; if it stopped needing it, \
                 drop it from this list, but if it declared its own copy instead, do not"
            );
        }

        // The copies themselves. A `fn derive_address` or a bare numeric
        // constant here is the harness testing itself instead of the SDK.
        for copy in [
            "fn derive_address",
            "const OUT_OF_ORDER_TOLERANCE",
            "const MAXIMUM_FORWARD_DISTANCE",
            "const KEY_PACKAGE_LIFETIME",
            "const NOT_BEFORE_BACKDATE",
            "const MAX_ACCEPTED",
        ] {
            assert!(
                !main_rs.contains(copy),
                "`{copy}` is back in tools/mls-interop/src/main.rs: the harness has to use \
                 offline-protocol-sealed's, or it stops testing the SDK's configuration"
            );
        }
    }
}
