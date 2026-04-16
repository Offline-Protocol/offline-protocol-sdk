//! Runtime telemetry configuration.
//!
//! [`TelemetryConfig`] carries the knobs consumed by the emit paths wired in
//! follow-up work. The struct is `#[non_exhaustive]` so new knobs can be added
//! without a breaking-change bump; construct via [`TelemetryConfig::default`]
//! and the `with_*` builder methods.

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
/// methods. The struct is `#[non_exhaustive]` to preserve room for future
/// knobs; downstream crates must not construct it with struct-literal syntax.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// When `true`, long-lived pseudonymous identifiers (`peer_id`, `user_id`,
    /// `app_id`, `group_id`) are hashed before crossing the telemetry sink
    /// boundary. Defaults to `true` — third-party sinks (analytics, crash
    /// reporters) must not receive raw identifiers by default.
    pub scrub_ids: bool,
    /// Verbosity tier for MLS lifecycle telemetry.
    pub mls_verbosity: MlsVerbosity,
    /// Cadence in milliseconds for periodic `MetricsSnapshot` emission. `None`
    /// disables periodic emission entirely (apps fall back to pull-based
    /// access via `TransportManager::metrics()`).
    pub metrics_cadence_ms: Option<u64>,
    /// Optional caller-supplied secret for opaque identifier hashing. When
    /// `None`, the SDK generates a random per-instance secret at install time.
    /// Supplying a stable secret enables cross-session correlation of the
    /// same peer in backend telemetry — do not share the secret across
    /// tenants.
    pub scrub_secret: Option<[u8; 16]>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            scrub_ids: true,
            mls_verbosity: MlsVerbosity::Lifecycle,
            metrics_cadence_ms: Some(5_000),
            scrub_secret: None,
        }
    }
}

impl TelemetryConfig {
    /// Sets the identifier-scrubbing flag.
    pub fn with_scrub_ids(mut self, scrub_ids: bool) -> Self {
        self.scrub_ids = scrub_ids;
        self
    }

    /// Sets the MLS lifecycle verbosity tier.
    pub fn with_mls_verbosity(mut self, mls_verbosity: MlsVerbosity) -> Self {
        self.mls_verbosity = mls_verbosity;
        self
    }

    /// Sets the periodic metrics-snapshot cadence.
    pub fn with_metrics_cadence_ms(mut self, cadence_ms: Option<u64>) -> Self {
        self.metrics_cadence_ms = cadence_ms;
        self
    }

    /// Sets the opaque-identifier hashing secret.
    pub fn with_scrub_secret(mut self, secret: Option<[u8; 16]>) -> Self {
        self.scrub_secret = secret;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_locks_in_privacy_preserving_knobs() {
        let cfg = TelemetryConfig::default();
        assert!(cfg.scrub_ids);
        assert_eq!(cfg.mls_verbosity, MlsVerbosity::Lifecycle);
        assert_eq!(cfg.metrics_cadence_ms, Some(5_000));
        assert!(cfg.scrub_secret.is_none());
    }

    #[test]
    fn builders_override_defaults() {
        let cfg = TelemetryConfig::default()
            .with_scrub_ids(false)
            .with_mls_verbosity(MlsVerbosity::Off)
            .with_metrics_cadence_ms(None)
            .with_scrub_secret(Some([7; 16]));
        assert!(!cfg.scrub_ids);
        assert_eq!(cfg.mls_verbosity, MlsVerbosity::Off);
        assert!(cfg.metrics_cadence_ms.is_none());
        assert_eq!(cfg.scrub_secret, Some([7; 16]));
    }
}
