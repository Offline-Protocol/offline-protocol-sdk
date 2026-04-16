//! Runtime telemetry configuration.
//!
//! [`TelemetryConfig`] carries the knobs consumed by the emit paths wired in
//! follow-up work. The struct is `#[non_exhaustive]` so new knobs can be added
//! without a breaking-change bump; construct via [`TelemetryConfig::default`]
//! and the `with_*` builder methods.

use std::fmt;
use std::time::Duration;

/// Verbosity tier for MLS lifecycle telemetry.
///
/// Replaces the compile-time `mls-observability` feature flag with a runtime
/// knob. The SDK emits MLS events at or below the configured tier; apps that
/// want zero MLS telemetry install `Off`, apps that want the existing
/// production-grade stream install `Lifecycle`, and apps that want verbose
/// diagnostic output install `Diagnostic`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MlsVerbosity {
    /// Suppress all MLS lifecycle telemetry.
    Off,
    /// Emit the standard lifecycle stream (initialization, session ready,
    /// decryption failures, etc.) — matches the legacy `mls-observability`
    /// feature-enabled behavior.
    ///
    /// No-op until emission wiring lands: in this release, MLS emit paths
    /// remain gated by the compile-time `mls-observability` feature, so
    /// setting this knob (or `Off`/`Diagnostic`) does not yet change what
    /// the SDK actually emits.
    #[default]
    Lifecycle,
    /// Emit the full lifecycle stream plus additional per-operation
    /// diagnostics intended for local debugging.
    Diagnostic,
}

/// Runtime configuration for the telemetry subsystem.
///
/// Constructed via [`TelemetryConfig::default`] (which locks in the
/// privacy-preserving defaults) and refined with the `with_*` builder
/// methods. Fields are crate-private; read them through the accessor
/// methods and mutate them through the builder methods — this keeps a
/// single idiom for configuration and prevents mid-run mutation of
/// identity-correlation parameters like the scrub secret (which would
/// invalidate every previously emitted opaque identifier).
///
/// The struct is `#[non_exhaustive]` to preserve room for future knobs.
#[non_exhaustive]
#[derive(Clone)]
pub struct TelemetryConfig {
    pub(crate) scrub_ids: bool,
    pub(crate) mls_verbosity: MlsVerbosity,
    pub(crate) metrics_cadence: Option<Duration>,
    pub(crate) scrub_secret: Option<[u8; 16]>,
}

impl fmt::Debug for TelemetryConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelemetryConfig")
            .field("scrub_ids", &self.scrub_ids)
            .field("mls_verbosity", &self.mls_verbosity)
            .field("metrics_cadence", &self.metrics_cadence)
            .field(
                "scrub_secret",
                &self.scrub_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            scrub_ids: true,
            mls_verbosity: MlsVerbosity::Lifecycle,
            metrics_cadence: Some(Duration::from_secs(5)),
            scrub_secret: None,
        }
    }
}

impl TelemetryConfig {
    /// Sets the identifier-scrubbing flag.
    ///
    /// When `true` (the default), long-lived pseudonymous identifiers
    /// (`peer_id`, `user_id`, `app_id`, `group_id`) are hashed before
    /// crossing the telemetry sink boundary — third-party sinks (analytics,
    /// crash reporters) must not receive raw identifiers by default.
    pub fn with_scrub_ids(mut self, scrub_ids: bool) -> Self {
        self.scrub_ids = scrub_ids;
        self
    }

    /// Sets the MLS lifecycle verbosity tier.
    pub fn with_mls_verbosity(mut self, mls_verbosity: MlsVerbosity) -> Self {
        self.mls_verbosity = mls_verbosity;
        self
    }

    /// Sets the cadence for periodic `MetricsSnapshot` emission. `None`
    /// disables periodic emission entirely (apps fall back to pull-based
    /// access via `TransportManager::metrics()`).
    pub fn with_metrics_cadence(mut self, cadence: Option<Duration>) -> Self {
        self.metrics_cadence = cadence;
        self
    }

    /// Sets the opaque-identifier hashing secret.
    ///
    /// Supplying `Some(secret)` enables cross-session correlation of the
    /// same peer in backend telemetry; supplying `None` (the default)
    /// leaves the secret unset in the config, and the SDK falls back to a
    /// random per-instance secret derived at protocol construction.
    /// Install-time generation of a stable, per-install secret (so
    /// correlation survives process restarts) lands with the emission-
    /// wiring follow-up.
    ///
    /// **Do not share the secret across tenants, and do not rotate it
    /// mid-run** — rotation invalidates every previously emitted opaque
    /// identifier and breaks correlation on the receiving side.
    pub fn with_scrub_secret(mut self, secret: Option<[u8; 16]>) -> Self {
        self.scrub_secret = secret;
        self
    }

    /// Returns whether identifier scrubbing is enabled.
    pub fn scrub_ids(&self) -> bool {
        self.scrub_ids
    }

    /// Returns the configured MLS lifecycle verbosity tier.
    pub fn mls_verbosity(&self) -> MlsVerbosity {
        self.mls_verbosity
    }

    /// Returns the configured periodic metrics-snapshot cadence.
    pub fn metrics_cadence(&self) -> Option<Duration> {
        self.metrics_cadence
    }

    /// Returns the configured opaque-identifier hashing secret, if any.
    ///
    /// Crate-private: the secret feeds `Scrubber` construction and is not
    /// part of the external reader surface — the Debug redaction on this
    /// struct would otherwise be defeated by a public byte-level accessor.
    ///
    /// Currently only consumed by `Scrubber::from_config` (scaffolding for
    /// the emission-wiring follow-up).
    #[allow(dead_code)]
    pub(crate) fn scrub_secret(&self) -> Option<[u8; 16]> {
        self.scrub_secret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_locks_in_privacy_preserving_knobs() {
        let cfg = TelemetryConfig::default();
        assert!(cfg.scrub_ids());
        assert_eq!(cfg.mls_verbosity(), MlsVerbosity::Lifecycle);
        assert_eq!(cfg.metrics_cadence(), Some(Duration::from_secs(5)));
        assert!(cfg.scrub_secret().is_none());
    }

    #[test]
    fn builders_override_defaults() {
        let cfg = TelemetryConfig::default()
            .with_scrub_ids(false)
            .with_mls_verbosity(MlsVerbosity::Off)
            .with_metrics_cadence(None)
            .with_scrub_secret(Some([7; 16]));
        assert!(!cfg.scrub_ids());
        assert_eq!(cfg.mls_verbosity(), MlsVerbosity::Off);
        assert!(cfg.metrics_cadence().is_none());
        assert_eq!(cfg.scrub_secret(), Some([7; 16]));
    }

    #[test]
    fn metrics_cadence_is_typed_duration() {
        let cfg = TelemetryConfig::default().with_metrics_cadence(Some(Duration::from_millis(250)));
        assert_eq!(cfg.metrics_cadence(), Some(Duration::from_millis(250)));
    }

    #[test]
    fn debug_redacts_scrub_secret() {
        let cfg = TelemetryConfig::default().with_scrub_secret(Some([0xAB; 16]));
        let rendered = format!("{cfg:?}");
        assert!(
            rendered.contains("<redacted>"),
            "expected redaction marker, got {rendered}"
        );
        assert!(
            !rendered.contains("ab, ab") && !rendered.contains("171, 171"),
            "scrub_secret bytes leaked into Debug output: {rendered}"
        );
    }

    #[test]
    fn debug_shows_none_when_no_secret() {
        let cfg = TelemetryConfig::default();
        let rendered = format!("{cfg:?}");
        assert!(rendered.contains("scrub_secret: None"), "got {rendered}");
    }
}
