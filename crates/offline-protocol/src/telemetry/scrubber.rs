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
use std::fmt;

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
#[cfg_attr(not(feature = "mls-observability"), allow(dead_code))]
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
#[derive(Clone)]
pub(crate) struct Scrubber {
    enabled: bool,
    #[cfg_attr(not(feature = "mls-observability"), allow(dead_code))]
    secret: [u8; 16],
}

impl fmt::Debug for Scrubber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Hand-rolled to redact `secret` — a derived Debug would dump the
        // per-instance hashing key verbatim into any log that formats a
        // Scrubber (directly or transitively via an owning struct).
        f.debug_struct("Scrubber")
            .field("enabled", &self.enabled)
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl Scrubber {
    /// Constructs a scrubber with the given enabled flag and secret.
    #[allow(dead_code)]
    pub fn new(enabled: bool, secret: [u8; 16]) -> Self {
        Self { enabled, secret }
    }

    /// Constructs a scrubber from a [`TelemetryConfig`], falling back to the
    /// supplied per-instance secret when the config does not carry one of
    /// its own.
    ///
    /// Scaffolding for the emission-wiring follow-up; the protocol engine
    /// does not yet invoke this path.
    #[allow(dead_code)]
    pub fn from_config(config: &TelemetryConfig, fallback_secret: [u8; 16]) -> Self {
        Self {
            enabled: config.scrub_ids(),
            secret: config.scrub_secret().unwrap_or(fallback_secret),
        }
    }

    /// Returns `true` when scrubbing is active.
    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Hashes `raw` into a 32-character hex opaque identifier, or borrows
    /// `raw` unchanged when scrubbing is disabled.
    ///
    /// The `Cow` return avoids allocation on the disabled-scrubbing hot path.
    ///
    /// Use this for **leaf** identifiers the caller has chosen to expose
    /// (individual `peer_id`, `group_id`, `user_id` on a record). For
    /// **derived correlation tokens** — values built by concatenating
    /// multiple raw identifiers — use [`Scrubber::hash_always`] instead, so
    /// that the composite cannot be disabled by a user-facing scrubbing
    /// preference.
    #[cfg_attr(not(feature = "mls-observability"), allow(dead_code))]
    pub fn hash_id<'a>(&self, raw: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(opaque_id(raw, &self.secret))
        } else {
            Cow::Borrowed(raw)
        }
    }

    /// Unconditionally hashes `raw` with the scrubber's secret, ignoring the
    /// `enabled` flag.
    ///
    /// Intended for derived correlation tokens (session IDs built from
    /// concatenated peer+group IDs, fingerprints over composite seeds) where
    /// emitting the raw string would leak more than the sum of its parts —
    /// these MUST be obfuscated regardless of the user's identifier-scrubbing
    /// preference, because disabling scrubbing is a choice about leaf IDs
    /// the caller already controls, not about derived values the SDK
    /// constructs internally.
    #[cfg_attr(not(feature = "mls-observability"), allow(dead_code))]
    pub fn hash_always(&self, raw: &str) -> String {
        opaque_id(raw, &self.secret)
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

    /// Locks the leaf-identifier policy the emission-wiring follow-up will
    /// inherit: when the caller opts out of scrubbing via
    /// `TelemetryConfig::with_scrub_ids(false)`, `hash_id` passes the raw
    /// identifier through unchanged, but `hash_always` still hashes derived
    /// correlation tokens. Flipping either half silently would change the
    /// privacy contract apps are told to expect.
    #[test]
    fn from_config_with_scrub_ids_disabled_passes_raw_through_hash_id() {
        let config = TelemetryConfig::default().with_scrub_ids(false);
        let scrubber = Scrubber::from_config(&config, [3; 16]);
        assert!(!scrubber.is_enabled());

        let leaf = scrubber.hash_id("peer-a");
        assert_eq!(leaf, "peer-a");
        assert!(matches!(leaf, Cow::Borrowed(_)));

        let derived = scrubber.hash_always("peer-a|group-b");
        assert_eq!(derived.len(), 32);
        assert!(derived.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(derived, "peer-a|group-b");
    }

    #[test]
    fn hash_always_ignores_enabled_flag() {
        let enabled = Scrubber::new(true, [1; 16]);
        let disabled = Scrubber::new(false, [1; 16]);
        // Identical output across both modes: `hash_always` never passes
        // through, so it is safe for derived correlation tokens.
        assert_eq!(
            enabled.hash_always("peer-a"),
            disabled.hash_always("peer-a")
        );
        // Output is always a 32-character hex opaque — never the raw input.
        let out = disabled.hash_always("peer-a");
        assert_eq!(out.len(), 32);
        assert!(out.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(out, "peer-a");
    }

    #[test]
    fn hash_always_matches_hash_id_when_enabled() {
        let scrubber = Scrubber::new(true, [9; 16]);
        assert_eq!(
            scrubber.hash_always("peer-a"),
            scrubber.hash_id("peer-a").into_owned(),
        );
    }

    #[test]
    fn debug_redacts_secret() {
        let scrubber = Scrubber::new(true, [0xAB; 16]);
        let rendered = format!("{scrubber:?}");
        assert!(
            rendered.contains("<redacted>"),
            "expected redaction marker, got {rendered}"
        );
        assert!(
            !rendered.contains("ab, ab") && !rendered.contains("171, 171"),
            "secret bytes leaked into Debug output: {rendered}"
        );
    }
}
