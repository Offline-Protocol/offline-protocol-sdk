//! PII-scrubbing helper for telemetry emit sites.
//!
//! [`Scrubber`] wraps the SHA-256-based opaque-identifier scheme already in
//! use by [`crate::mls_observability`] and exposes a single `hash_id` entry
//! point. When scrubbing is disabled (caller opted in via
//! `TelemetryConfig::with_scrub_ids(false)`), the helper returns the input
//! unchanged so emit sites can call it unconditionally.

use crate::mls_observability::opaque_id;
use crate::telemetry::config::TelemetryConfig;

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
            enabled: config.scrub_ids,
            secret: config.scrub_secret.unwrap_or(fallback_secret),
        }
    }

    /// Returns `true` when scrubbing is active.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Hashes `raw` into a 32-character hex opaque identifier, or returns
    /// `raw` unchanged when scrubbing is disabled.
    pub fn hash_id(&self, raw: &str) -> String {
        if self.enabled {
            opaque_id(raw, &self.secret)
        } else {
            raw.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_scrubber_returns_raw() {
        let scrubber = Scrubber::new(false, [0; 16]);
        assert_eq!(scrubber.hash_id("peer-a"), "peer-a");
        assert!(!scrubber.is_enabled());
    }

    #[test]
    fn enabled_scrubber_returns_stable_hash() {
        let scrubber = Scrubber::new(true, [1; 16]);
        let first = scrubber.hash_id("peer-a");
        let second = scrubber.hash_id("peer-a");
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
        let expected = Scrubber::new(true, [9; 16]).hash_id("peer-a");
        assert_eq!(scrubber.hash_id("peer-a"), expected);
    }

    #[test]
    fn from_config_uses_fallback_when_secret_absent() {
        let config = TelemetryConfig::default();
        let fallback = [5; 16];
        let scrubber = Scrubber::from_config(&config, fallback);
        let expected = Scrubber::new(true, fallback).hash_id("peer-a");
        assert_eq!(scrubber.hash_id("peer-a"), expected);
    }
}
