//! Crate-private telemetry context bundling the installed sink, its config,
//! and the derived identifier scrubber.
//!
//! `OfflineProtocol::install_telemetry_sink` constructs an
//! `Arc<TelemetryContext>` once and shares it between the protocol handle and
//! `SharedState` so that both emit paths (`SharedState::emit_event` for
//! protocol events, `OfflineProtocol::emit_mls_lifecycle_event` for MLS
//! lifecycle events) dispatch through the same configuration.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::config::TelemetryConfig;
use super::scrubber::Scrubber;
use super::sink::TelemetrySink;

/// Installed telemetry surface. The sink, config, and scrubber reach every
/// emit site through an `Arc<TelemetryContext>`.
pub(crate) struct TelemetryContext {
    pub(crate) sink: Arc<dyn TelemetrySink>,
    pub(crate) config: TelemetryConfig,
    pub(crate) scrubber: Scrubber,
    /// The gate every emit site reads first. Shared with the telemetry pipe
    /// so `set_telemetry_enabled(false)` stops the clone as well as the
    /// dispatch; a plain sink installs an always-true gate.
    enabled: Arc<AtomicBool>,
}

impl TelemetryContext {
    /// Builds a context whose emit gate is `enabled`, which a caller may
    /// flip at any time without reinstalling.
    ///
    /// `fallback_secret` is the per-instance hashing key used when the
    /// supplied `config` does not carry its own `scrub_secret`. Passing the
    /// same fallback across multiple calls on a single protocol instance
    /// keeps opaque identifiers stable even when the config is re-installed
    /// without an explicit secret.
    ///
    /// The only constructor: a gate-less one would have to invent an
    /// always-true gate, and the two callers that want one
    /// (`install_telemetry_sink` and its tests) are better off saying so.
    pub(crate) fn with_gate(
        sink: Arc<dyn TelemetrySink>,
        config: TelemetryConfig,
        fallback_secret: [u8; 16],
        enabled: Arc<AtomicBool>,
    ) -> Arc<Self> {
        let scrubber = Scrubber::from_config(&config, fallback_secret);
        Arc::new(Self {
            sink,
            config,
            scrubber,
            enabled,
        })
    }

    /// Whether emission is on. Checked before any record is built, cloned
    /// or scrubbed, so a disabled context costs one atomic load per site.
    #[inline]
    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
}
