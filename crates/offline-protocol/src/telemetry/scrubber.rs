//! PII-scrubbing helper for telemetry emit sites.
//!
//! [`Scrubber`] is the single entry point for hashing long-lived pseudonymous
//! identifiers before they cross the telemetry sink boundary. Callers
//! construct one (via [`Scrubber::new`] or [`Scrubber::from_config`]) and call
//! `hash_id` — when scrubbing is disabled (caller opted in via
//! `TelemetryConfig::with_scrub_ids(false)`), the helper returns the input
//! unchanged so emit sites can call it unconditionally.
//!
//! Construction note: the hash is `SHA-256(secret || raw)` — a prefix-MAC,
//! adequate for deterministic pseudonymization but **not** length-extension
//! resistant. Do not repurpose for integrity checks; use HMAC if that ever
//! becomes a requirement.

use std::borrow::Cow;

use sha2::{Digest, Sha256};

use crate::telemetry::config::TelemetryConfig;

/// Generates a stable opaque identifier for telemetry.
///
/// Produces a 32-character lowercase hex string derived from the first 16
/// bytes of `SHA-256(secret || raw)`. The result is deterministic for a
/// given `(secret, raw)` pair but reveals no information about `raw` to a
/// party that does not hold the secret.
///
/// Crate-private: external callers use [`Scrubber::hash_id`] instead so
/// there is a single public entry point to the hashing operation.
pub(crate) fn opaque_id(raw: &str, secret: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret);
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(32);
    for byte in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}

/// Helper for hashing long-lived pseudonymous identifiers before they cross
/// the telemetry sink boundary.
///
/// Hashing is deterministic for a given `(secret, raw)` pair so that the
/// same peer appears under the same opaque identifier in every record
/// produced by the same SDK instance, but the raw identifier cannot be
/// recovered without the secret.
#[derive(Debug, Clone)]
pub struct Scrubber {
    enabled: bool,
    secret: [u8; 16],
}

impl Scrubber {
    /// Constructs a scrubber with the given enabled flag and secret.
    pub fn new(enabled: bool, secret: [u8; 16]) -> Self {
        Self { enabled, secret }
    }

    /// Constructs a scrubber from a [`TelemetryConfig`], falling back to the
    /// supplied per-instance secret when the config does not carry one of
    /// its own.
    pub fn from_config(config: &TelemetryConfig, fallback_secret: [u8; 16]) -> Self {
        Self {
            enabled: config.scrub_ids(),
            secret: config.scrub_secret().unwrap_or(fallback_secret),
        }
    }

    /// Returns `true` when scrubbing is active.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Hashes `raw` into a 32-character hex opaque identifier, or borrows
    /// `raw` unchanged when scrubbing is disabled.
    ///
    /// The `Cow` return avoids allocation on the disabled-scrubbing hot path.
    pub fn hash_id<'a>(&self, raw: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(opaque_id(raw, &self.secret))
        } else {
            Cow::Borrowed(raw)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_id_is_stable_for_same_input() {
        let secret = b"secret";
        let first = opaque_id("peer:alice", secret);
        let second = opaque_id("peer:alice", secret);
        assert_eq!(first, second);
        assert_eq!(first.len(), 32);
    }

    #[test]
    fn opaque_id_changes_for_different_secret() {
        let first = opaque_id("peer:alice", b"secret-a");
        let second = opaque_id("peer:alice", b"secret-b");
        assert_ne!(first, second);
    }

    #[test]
    fn opaque_id_handles_empty_input() {
        let secret = b"secret";
        let out = opaque_id("", secret);
        assert_eq!(out.len(), 32);
        assert!(out.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn disabled_scrubber_borrows_raw() {
        let scrubber = Scrubber::new(false, [0; 16]);
        let out = scrubber.hash_id("peer-a");
        assert_eq!(out, "peer-a");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert!(!scrubber.is_enabled());
    }

    #[test]
    fn disabled_scrubber_borrows_empty_input() {
        let scrubber = Scrubber::new(false, [0; 16]);
        let out = scrubber.hash_id("");
        assert_eq!(out, "");
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn enabled_scrubber_handles_empty_input() {
        let scrubber = Scrubber::new(true, [1; 16]);
        let out = scrubber.hash_id("");
        assert_eq!(out.len(), 32);
        assert!(matches!(out, Cow::Owned(_)));
    }

    #[test]
    fn enabled_scrubber_returns_stable_hash() {
        let scrubber = Scrubber::new(true, [1; 16]);
        let first = scrubber.hash_id("peer-a").into_owned();
        let second = scrubber.hash_id("peer-a").into_owned();
        assert_eq!(first, second);
        assert_eq!(first.len(), 32);
        assert_ne!(first, "peer-a");
    }

    #[test]
    fn different_secrets_produce_different_hashes() {
        let a = Scrubber::new(true, [1; 16]);
        let b = Scrubber::new(true, [2; 16]);
        assert_ne!(a.hash_id("peer-a"), b.hash_id("peer-a"));
    }

    #[test]
    fn from_config_prefers_config_secret() {
        let config = TelemetryConfig::default().with_scrub_secret(Some([9; 16]));
        let fallback = [0; 16];
        let scrubber = Scrubber::from_config(&config, fallback);
        assert!(scrubber.is_enabled());
        let expected = Scrubber::new(true, [9; 16]).hash_id("peer-a").into_owned();
        assert_eq!(scrubber.hash_id("peer-a"), expected);
    }

    #[test]
    fn from_config_uses_fallback_when_secret_absent() {
        let config = TelemetryConfig::default();
        let fallback = [5; 16];
        let scrubber = Scrubber::from_config(&config, fallback);
        let expected = Scrubber::new(true, fallback).hash_id("peer-a").into_owned();
        assert_eq!(scrubber.hash_id("peer-a"), expected);
    }
}
