//! Core types and data structures for the Offline Protocol SDK.
//!
//! This crate provides the fundamental types used throughout the SDK:
//! - Message types and identifiers
//! - Protocol types (UserId, AppId, TTL, etc.)
//! - Error types
//!
//! All types in this crate are 100% safe Rust with no unsafe blocks.
//!
//! # Building without `std`
//!
//! This crate compiles for bare-metal targets with `--no-default-features`,
//! which is how a constrained leaf node (a Cortex-M lock or sensor that speaks
//! the protocol but cannot host the engine) links it. `alloc` is required in
//! both configurations.
//!
//! What the `std` feature adds is everything that mints state from the
//! platform, because a bare-metal target supplies none of it: a clock
//! ([`Timestamp::now`], [`WallClockTimestamp::now`], [`LocalInstant`]), an
//! entropy source ([`MessageId::new`], and so [`Message::new`] and
//! [`Message::builder`] with it), and threads (the [`sync`] module's
//! poison-recovering lock helpers). Everything that parses, validates,
//! re-encodes or compares is present in both configurations, which is the half
//! a leaf node actually needs: it receives a frame someone else minted.
//!
//! Construct messages from wire-supplied parts on the no_std path, via
//! [`MessageId::from_bytes`], [`Timestamp::from_millis`] and the struct
//! literal, rather than the `now()`/`new()` constructors.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod address;
pub mod error;
pub mod message;
pub mod service;
#[cfg(feature = "std")]
pub mod sync;
pub mod types;
pub mod username;
pub mod wire;

pub use address::{Address, AddressError};
pub use error::{Error, Result};
pub use message::{
    ContentType, ForwardInfo, MediaMetadata, Message, MessageId, MessagePriority, ReplyContext,
    WireCodec,
};
pub use service::{ServiceDescriptor, ServiceId};
#[cfg(feature = "std")]
pub use sync::{MutexExt, RwLockExt};
#[cfg(feature = "std")]
pub use types::LocalInstant;
pub use types::{
    validate_id_chars, AppId, HopCount, IdValidationError, LamportClock, MetadataMap, Timestamp,
    UserId, WallClockTimestamp, MAX_ID_LEN, TTL,
};
pub use username::{contains_control_or_format, Username, UsernameError};
pub use wire::{WIRE_V1_MAGIC, WIRE_VERSION_V1};

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
        "unicode-normalization",
        "thiserror",
        "uuid",
        "tracing",
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
                 offline-protocol-core says {own:?}. Update both together."
            );
        }
    }
}
