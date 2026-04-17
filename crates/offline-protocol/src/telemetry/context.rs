//! Crate-private telemetry context bundling the installed sink, its config,
//! and the derived identifier scrubber.
//!
//! `OfflineProtocol::install_telemetry_sink` constructs an
//! `Arc<TelemetryContext>` once and shares it between the protocol handle and
//! `SharedState` so that both emit paths (`SharedState::emit_event` for
//! protocol events, `OfflineProtocol::emit_mls_lifecycle_event` for MLS
//! lifecycle events) dispatch through the same configuration.

use std::sync::Arc;

use super::config::TelemetryConfig;
use super::scrubber::Scrubber;
use super::sink::TelemetrySink;

/// Installed telemetry surface. The sink, config, and scrubber reach every
/// emit site through an `Arc<TelemetryContext>`.
#[allow(dead_code)]
pub(crate) struct TelemetryContext {
    pub(crate) sink: Arc<dyn TelemetrySink>,
    pub(crate) config: TelemetryConfig,
    pub(crate) scrubber: Scrubber,
}

impl TelemetryContext {
    /// Builds a context from an installed sink and config.
    ///
    /// `fallback_secret` is the per-instance hashing key used when the
    /// supplied `config` does not carry its own `scrub_secret`. Passing the
    /// same fallback across multiple calls on a single protocol instance
    /// keeps opaque identifiers stable even when the config is re-installed
    /// without an explicit secret.
    pub(crate) fn new(
        sink: Arc<dyn TelemetrySink>,
        config: TelemetryConfig,
        fallback_secret: [u8; 16],
    ) -> Arc<Self> {
        let scrubber = Scrubber::from_config(&config, fallback_secret);
        Arc::new(Self {
            sink,
            config,
            scrubber,
        })
    }
}
