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
/// The SDK emits MLS events at or below the configured tier. Apps that want
/// zero MLS telemetry install `Off`, apps that want the standard operational
/// stream install `Lifecycle` (the default), and apps that want verbose
/// per-operation diagnostics install `Diagnostic`. `Off` suppresses both the
/// [`TelemetrySink`] fan-out and the legacy
/// [`crate::mls_observability::MlsEventEmitter`] path.
///
/// [`TelemetrySink`]: crate::telemetry::TelemetrySink
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MlsVerbosity {
    /// Suppress all MLS lifecycle telemetry.
    Off,
    /// Emit the standard lifecycle stream: initialization, session ready,
    /// decryption failures, session missing, encryption used.
    #[default]
    Lifecycle,
    /// Emit the full lifecycle stream plus additional per-operation
    /// diagnostics intended for local debugging.
    ///
    /// # Current behavior
    ///
    /// As of this release, `Diagnostic` is accepted by the configuration
    /// surface but produces the *same* event stream as [`Self::Lifecycle`]
    /// — the additional per-operation diagnostics are introduced alongside
    /// the extra emission sites they describe. Apps that select
    /// `Diagnostic` today are forward-compatible with those follow-ups;
    /// they will not need to reconfigure when the richer stream lands.
    Diagnostic,
}

/// Default background flush cadence.
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(30);
/// Default size trigger for an early flush, and the largest batch cut.
pub const DEFAULT_MAX_BATCH_BYTES: usize = 256 * 1024;
/// Largest batch the configuration accepts. The ingest's body limit sits
/// above it; a batch that cannot fit is dropped, never split.
pub const MAX_BATCH_BYTES_LIMIT: usize = 1024 * 1024;
/// Default ring-buffer capacity; drops oldest beyond it.
pub const DEFAULT_MAX_BUFFERED_RECORDS: usize = 4096;
/// Shortest flush cadence the configuration accepts.
pub const MIN_FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// Runtime configuration for the telemetry subsystem.
///
/// Constructed via [`TelemetryConfig::default`] (which locks in the
/// privacy-preserving defaults) and refined with the `with_*` builder
/// methods. `enable_telemetry` additionally needs an `api_key` and an
/// `app_id`, both issued by the developer portal; see
/// [`TelemetryConfig::validate`] for what it refuses. Fields are crate-private; read them through the accessor
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
    pub(crate) routing_diagnostic: bool,
    pub(crate) mls_sampling_bypass: bool,
    pub(crate) api_key: Option<String>,
    pub(crate) app_id: Option<String>,
    pub(crate) app_version: String,
    pub(crate) debug: bool,
    pub(crate) flush_interval: Duration,
    pub(crate) max_batch_bytes: usize,
    pub(crate) max_buffered_records: usize,
    pub(crate) include_device_id: bool,
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
            .field("routing_diagnostic", &self.routing_diagnostic)
            .field("mls_sampling_bypass", &self.mls_sampling_bypass)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("app_id", &self.app_id)
            .field("app_version", &self.app_version)
            .field("debug", &self.debug)
            .field("flush_interval", &self.flush_interval)
            .field("max_batch_bytes", &self.max_batch_bytes)
            .field("max_buffered_records", &self.max_buffered_records)
            .field("include_device_id", &self.include_device_id)
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
            routing_diagnostic: false,
            mls_sampling_bypass: false,
            api_key: None,
            app_id: None,
            app_version: "0.0.0".to_string(),
            debug: false,
            flush_interval: DEFAULT_FLUSH_INTERVAL,
            max_batch_bytes: DEFAULT_MAX_BATCH_BYTES,
            max_buffered_records: DEFAULT_MAX_BUFFERED_RECORDS,
            include_device_id: false,
        }
    }
}

impl TelemetryConfig {
    /// Sets the identifier-scrubbing flag.
    ///
    /// When `true` (the default), the engine hashes the peer and group ids it
    /// stamps on MLS lifecycle records with the scrub secret (see
    /// [`Self::with_scrub_secret`]). The telemetry pipe reads the peer id only
    /// to pair a handshake's start with its end, and no identifier has a wire
    /// field, so the flag changes what the pipe holds in memory and never what
    /// is uploaded. It does not apply to `on_event`, which delivers every
    /// event with its identifiers as they are.
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
    /// Supplying `Some(secret)` pins an explicit, caller-owned secret and
    /// always takes precedence over the SDK-managed fallback below.
    ///
    /// Supplying `None` (the default) leaves the secret unset in the config.
    /// In that case the SDK manages the fallback for you: the first time
    /// secure storage is provided via `OfflineProtocol::initialize_mls`, the
    /// SDK loads — or, on first run, generates and persists — a stable
    /// **per-install** secret. This makes
    /// opaque identifiers survive process restarts, so backend telemetry can
    /// count distinct devices across sessions. Until storage is available (or
    /// if storage is never provided), the SDK uses a random per-instance
    /// secret derived at protocol construction.
    ///
    /// Precedence: explicit config secret > SDK-managed persistent secret >
    /// random per-instance secret.
    ///
    /// **Do not share the secret across tenants, and do not rotate it
    /// mid-run** — rotation invalidates every previously emitted opaque
    /// identifier and breaks correlation on the receiving side. For the
    /// SDK-managed fallback this means: do not back two tenants with the same
    /// storage, and do not clear the `scrub_secret` storage entry within a
    /// measurement window.
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

    /// Sets whether `RoutingDecision` records should carry the full
    /// per-transport score breakdown.
    ///
    /// When `true`, every emitted
    /// [`crate::telemetry::RoutingDecision::scores`] is populated with the
    /// seven-factor [`offline_protocol_router::TransportScore`] for each
    /// ranked transport. When `false` (the default), `scores` is empty —
    /// the hot path avoids the per-emission vector allocation and
    /// `Diagnostic`-tier consumers simply see an empty vector.
    ///
    /// Leaf identifiers are not affected: this knob only gates the
    /// scoring-factor detail, not identity fields.
    pub fn with_routing_diagnostic(mut self, routing_diagnostic: bool) -> Self {
        self.routing_diagnostic = routing_diagnostic;
        self
    }

    /// Returns whether routing records should carry diagnostic score detail.
    pub fn routing_diagnostic(&self) -> bool {
        self.routing_diagnostic
    }

    /// Opts the installed sink out of MLS lifecycle event rate limiting.
    ///
    /// By default (`false`), high-volume MLS lifecycle events
    /// (`mls.decryption_failed`, `mls.session_missing`) pass through a
    /// best-effort fixed-window limiter that caps emission per peer+kind. The
    /// cap protects naïve sinks from event floods, but it also clips genuine
    /// failure spikes to the per-window ceiling — an aggregating backend
    /// cannot tell a real burst of N failures apart from the cap.
    ///
    /// Setting this to `true` disables that limiter so a **telemetry-grade**
    /// sink receives every MLS lifecycle event un-sampled, and aggregate
    /// counts reflect reality. Only enable it for sinks that apply their own
    /// backpressure (for example, pushing onto a bounded channel and draining
    /// on a background task per the [`TelemetrySink`] contract) — an
    /// unbuffered sink that does real work inline can be overwhelmed by an
    /// un-capped failure spike.
    ///
    /// This knob only affects MLS-event rate limiting; it does not change
    /// identifier scrubbing, verbosity, or any other field.
    ///
    /// [`TelemetrySink`]: crate::telemetry::TelemetrySink
    pub fn with_mls_sampling_bypass(mut self, mls_sampling_bypass: bool) -> Self {
        self.mls_sampling_bypass = mls_sampling_bypass;
        self
    }

    /// Returns whether MLS lifecycle event rate limiting is bypassed.
    pub fn mls_sampling_bypass(&self) -> bool {
        self.mls_sampling_bypass
    }

    /// Sets the API key the developer portal issued. Sent as a bearer token
    /// on every upload; redacted from `Debug` output.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Sets the app id the key was issued against. The ingest cross-checks
    /// the two and refuses a mismatch, which stops a key pasted into the
    /// wrong app from misattributing its data.
    pub fn with_app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = Some(app_id.into());
        self
    }

    /// Sets the application's own version string, stamped on every batch.
    pub fn with_app_version(mut self, app_version: impl Into<String>) -> Self {
        self.app_version = app_version.into();
        self
    }

    /// Logs every wire event at `debug` under the pipe's `tracing` target,
    /// so an adopter can see exactly what leaves the device.
    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    /// Sets the background flush cadence. Not below [`MIN_FLUSH_INTERVAL`].
    pub fn with_flush_interval(mut self, flush_interval: Duration) -> Self {
        self.flush_interval = flush_interval;
        self
    }

    /// Sets the size trigger for an early flush and the largest batch cut.
    pub fn with_max_batch_bytes(mut self, max_batch_bytes: usize) -> Self {
        self.max_batch_bytes = max_batch_bytes;
        self
    }

    /// Sets the ring-buffer capacity.
    pub fn with_max_buffered_records(mut self, max_buffered_records: usize) -> Self {
        self.max_buffered_records = max_buffered_records;
        self
    }

    /// Stamps the stable per-install id on every batch, so the ingest counts
    /// distinct devices rather than distinct sessions.
    ///
    /// Off by default because it is a disclosure decision: a stable
    /// per-install identifier is a "device or other ID" under Google Play's
    /// definition and a `NSPrivacyCollectedDataTypeDeviceID` under Apple's.
    /// See `docs/privacy.md`.
    pub fn with_include_device_id(mut self, include_device_id: bool) -> Self {
        self.include_device_id = include_device_id;
        self
    }

    /// Returns the configured API key.
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Returns the configured app id.
    pub fn app_id(&self) -> Option<&str> {
        self.app_id.as_deref()
    }

    /// Returns the application version stamped on batches.
    pub fn app_version(&self) -> &str {
        &self.app_version
    }

    /// Returns whether the debug tap is on.
    pub fn debug(&self) -> bool {
        self.debug
    }

    /// Returns the background flush cadence.
    pub fn flush_interval(&self) -> Duration {
        self.flush_interval
    }

    /// Returns the size trigger and batch ceiling.
    pub fn max_batch_bytes(&self) -> usize {
        self.max_batch_bytes
    }

    /// Returns the ring-buffer capacity.
    pub fn max_buffered_records(&self) -> usize {
        self.max_buffered_records
    }

    /// Returns whether the per-install id is stamped on batches.
    pub fn include_device_id(&self) -> bool {
        self.include_device_id
    }

    /// Checks the fields `enable_telemetry` depends on, naming the first one
    /// that fails.
    ///
    /// Refused: a missing or empty `api_key` or `app_id`, or one that is not
    /// printable ASCII (a header value the ingest could never read, which
    /// would otherwise be retried forever as a network error); an empty
    /// `app_version`; a `max_batch_bytes` of zero or above
    /// [`MAX_BATCH_BYTES_LIMIT`]; a `max_buffered_records` of zero; a
    /// `flush_interval` below [`MIN_FLUSH_INTERVAL`], which is reported under
    /// the name every binding gives it, `flush_interval_ms`.
    pub fn validate(&self) -> Result<(), &'static str> {
        fn header_safe(value: Option<&str>) -> bool {
            value.is_some_and(|v| !v.is_empty() && v.bytes().all(|b| (0x20..=0x7e).contains(&b)))
        }
        if !header_safe(self.api_key.as_deref()) {
            return Err("api_key");
        }
        if !header_safe(self.app_id.as_deref()) {
            return Err("app_id");
        }
        if self.app_version.is_empty() {
            return Err("app_version");
        }
        if self.max_batch_bytes == 0 || self.max_batch_bytes > MAX_BATCH_BYTES_LIMIT {
            return Err("max_batch_bytes");
        }
        if self.max_buffered_records == 0 {
            return Err("max_buffered_records");
        }
        if self.flush_interval < MIN_FLUSH_INTERVAL {
            // Named for the field the caller set, not for the one this struct
            // holds. The refusal is the only diagnostic an application gets,
            // and every binding spells this `flushIntervalMs`; a message
            // naming `flush_interval` sends the reader looking for a field
            // their SDK does not have.
            return Err("flush_interval_ms");
        }
        Ok(())
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
        assert!(!cfg.routing_diagnostic());
        assert!(!cfg.mls_sampling_bypass());
        assert!(cfg.api_key().is_none());
        assert!(cfg.app_id().is_none());
        assert_eq!(cfg.app_version(), "0.0.0");
        assert!(!cfg.debug());
        assert_eq!(cfg.flush_interval(), Duration::from_secs(30));
        assert_eq!(cfg.max_batch_bytes(), 256 * 1024);
        assert_eq!(cfg.max_buffered_records(), 4096);
        assert!(
            !cfg.include_device_id(),
            "a stable id is a disclosure decision"
        );
    }

    #[test]
    fn validate_names_the_first_field_it_refuses() {
        let valid = TelemetryConfig::default()
            .with_api_key("mp_k")
            .with_app_id("app_1 (prod)");
        assert_eq!(valid.validate(), Ok(()));
        assert_eq!(TelemetryConfig::default().validate(), Err("api_key"));
        assert_eq!(valid.clone().with_api_key("").validate(), Err("api_key"));
        assert_eq!(
            valid.clone().with_api_key("k\u{e9}y").validate(),
            Err("api_key")
        );
        assert_eq!(valid.clone().with_app_id("").validate(), Err("app_id"));
        assert_eq!(valid.clone().with_app_id("app\n").validate(), Err("app_id"));
        assert_eq!(
            valid.clone().with_app_version("").validate(),
            Err("app_version")
        );
        assert_eq!(
            valid.clone().with_max_batch_bytes(0).validate(),
            Err("max_batch_bytes")
        );
        assert_eq!(
            valid
                .clone()
                .with_max_batch_bytes(MAX_BATCH_BYTES_LIMIT + 1)
                .validate(),
            Err("max_batch_bytes")
        );
        assert_eq!(
            valid.clone().with_max_buffered_records(0).validate(),
            Err("max_buffered_records")
        );
        assert_eq!(
            valid
                .clone()
                .with_flush_interval(Duration::from_millis(999))
                .validate(),
            Err("flush_interval_ms")
        );
    }

    #[test]
    fn debug_redacts_the_api_key() {
        let cfg = TelemetryConfig::default().with_api_key("mp_secret_key");
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("mp_secret_key"), "{rendered}");
        assert!(
            rendered.contains("api_key: Some(\"<redacted>\")"),
            "{rendered}"
        );
    }

    #[test]
    fn builders_override_defaults() {
        let cfg = TelemetryConfig::default()
            .with_scrub_ids(false)
            .with_mls_verbosity(MlsVerbosity::Off)
            .with_metrics_cadence(None)
            .with_scrub_secret(Some([7; 16]))
            .with_routing_diagnostic(true)
            .with_mls_sampling_bypass(true);
        assert!(!cfg.scrub_ids());
        assert_eq!(cfg.mls_verbosity(), MlsVerbosity::Off);
        assert!(cfg.metrics_cadence().is_none());
        assert_eq!(cfg.scrub_secret(), Some([7; 16]));
        assert!(cfg.routing_diagnostic());
        assert!(cfg.mls_sampling_bypass());
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
