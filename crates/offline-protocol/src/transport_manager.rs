//! Transport management for multi-transport protocol.
//!
//! This module provides the TransportManager which manages multiple transports
//! and uses DORS (Dynamic Offline Relay Switch) to select the optimal transport
//! for each message.

use crate::constants::{HOP_COUNT_EMA_ALPHA, LATENCY_EMA_ALPHA, OBSERVED_STATS_COMPACT_THRESHOLD};
use crate::events::{DorsEscalationPhase, DorsEscalationReasonCode, DorsReasonCode, Event};
use crate::telemetry::routing::{RoutingDecision, RoutingPhase, RoutingReasonCode};
use crate::{Error, Result};
use chrono::Utc;
use offline_protocol_core::{Message, WireCodec};
use offline_protocol_router::{
    display_routing_score, DorsConfig, EscalationTriggerReason, TransportScore, TransportSelector,
};
use offline_protocol_transport::{
    Error as TransportError, Transport, TransportMetrics, TransportStatus, TransportType,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, warn};

/// Manages multiple transports and handles transport selection.
pub struct TransportManager {
    /// Available transports mapped by type.
    ///
    /// Transports are fully interior-mutable (every `Transport` method takes
    /// `&self`), so they are shared as plain `Arc`s — no outer mutex
    /// serializing sends against status/metrics reads.
    transports: HashMap<TransportType, Arc<dyn Transport>>,

    /// Current active transport.
    current_transport: Option<TransportType>,

    /// Transport selector (DORS).
    selector: TransportSelector,

    /// Locally observed delivery outcomes used to enrich transport metrics.
    observations: HashMap<TransportType, ObservedStats>,

    /// Optional callback for DORS lifecycle/decision events (OFF-258).
    dors_event_callback: Option<Arc<dyn Fn(Event) + Send + Sync>>,

    /// Optional callback for structured routing decisions. Fires alongside
    /// every legacy `dors_event_callback` emission so the telemetry sink
    /// receives the richer `RoutingDecision` shape while `EventCallback`
    /// consumers continue to see the flattened `Event::Dors*` variants.
    routing_decision_callback: Option<Arc<dyn Fn(RoutingDecision) + Send + Sync>>,

    /// When `true`, every emitted [`RoutingDecision`] carries the full
    /// per-transport [`TransportScore`] breakdown in its `scores` field.
    /// Mirrors `TelemetryConfig::routing_diagnostic` and is toggled by
    /// `OfflineProtocol::install_telemetry_sink`.
    routing_diagnostic: bool,

    /// Last escalation trigger event emitted (reason, time) for dedupe window.
    last_escalation_trigger_emitted: Option<(DorsEscalationReasonCode, std::time::Instant)>,

    /// Peers known to decode the binary wire codec (from their signed key
    /// package). Messages addressed to a listed peer are stamped
    /// [`WireCodec::BinaryV1`] at send; all others stay JSON.
    peer_binary_wire: HashSet<String>,
}

/// Carriers whose links are peer-to-peer, in the order a forwarding caller
/// should prefer them.
///
/// These are the only transports that can carry a *mesh hop*: a frame handed
/// straight to a neighbor's radio. Wi-Fi Direct leads because a link that
/// exists at all is the higher-bandwidth one; BLE is the carrier that is
/// almost always present. Everything else reaches peers through
/// infrastructure, which is not a hop — it is an exit from the mesh.
const MESH_TRANSPORTS: &[TransportType] = &[TransportType::WiFiDirect, TransportType::BLE];

/// A peer reachable over a direct mesh link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshNeighbor {
    /// User-level id of the neighbor.
    pub peer_id: String,
    /// Signal strength for the link, when the carrier measures one.
    pub rssi: Option<i16>,
    /// The carrier holding the link.
    pub transport: TransportType,
}

impl MeshNeighbor {
    /// How good this link is, on a 0–100 scale.
    ///
    /// Derived from signal strength where the carrier reports it, and a
    /// mid-scale value where it does not — an unmeasured link should not
    /// outrank a measured good one, nor be discarded like a measured bad one.
    pub fn link_quality(&self) -> u8 {
        match self.rssi {
            Some(rssi) => offline_protocol_transport::LinkQuality::from_rssi(rssi).value(),
            None => offline_protocol_transport::DEFAULT_LINK_QUALITY_WITHOUT_RSSI,
        }
    }
}

/// Dedupe window: don't re-emit same escalation trigger reason within this duration.
const ESCALATION_TRIGGER_DEDUPE_SECS: u64 = 30;

/// Cap on the per-peer binary-wire capability set. Keyed by the wire-claimed
/// peer id, so — like `key_package_sent_to` — it is cleared at capacity to bound
/// a forged-sender flood; the only cost of forgetting a peer is re-learning its
/// capability from the next key package (one JSON send in the meantime).
const MAX_PEER_BINARY_WIRE_ENTRIES: usize = 4096;

#[derive(Debug, Default, Clone)]
struct ObservedStats {
    success_count: u32,
    failure_count: u32,
    latency_ema: Option<f32>,
    hop_ema: Option<f32>,
    last_success_at: Option<Instant>,
}

impl ObservedStats {
    fn record_success(&mut self, latency_ms: u32, hop_count: u8) {
        self.success_count = self.success_count.saturating_add(1);
        self.latency_ema = Some(update_ema(
            self.latency_ema,
            latency_ms as f32,
            LATENCY_EMA_ALPHA,
        ));
        self.hop_ema = Some(update_ema(
            self.hop_ema,
            hop_count as f32,
            HOP_COUNT_EMA_ALPHA,
        ));
        self.last_success_at = Some(Instant::now());
        self.compact();
    }

    fn record_failure(&mut self) {
        self.failure_count = self.failure_count.saturating_add(1);
        self.compact();
    }

    fn compact(&mut self) {
        let total = self.success_count.saturating_add(self.failure_count);
        if total > OBSERVED_STATS_COMPACT_THRESHOLD {
            self.success_count /= 2;
            self.failure_count /= 2;
        }
    }

    fn delivery_ratio(&self) -> Option<f32> {
        let total = self.success_count + self.failure_count;
        if total == 0 {
            None
        } else {
            Some(self.success_count as f32 / total as f32)
        }
    }

    fn drop_ratio(&self) -> Option<f32> {
        self.delivery_ratio()
            .map(|ratio| (1.0 - ratio).clamp(0.0, 1.0))
    }

    fn apply_to_metrics(&self, metrics: &mut TransportMetrics) {
        if self.success_count + self.failure_count > 0 {
            // Only override delivery counts when the transport itself has not
            // reported any.  Internet transport tracks wire-level outcomes via
            // the confirmation loop; overwriting those would discard real data.
            //
            // NOTE: This check is value-based, not transport-type-based.  If
            // BLE or WiFi Direct ever start reporting their own success/failure
            // counts from the native layer (the way Internet does via the
            // confirmation loop), the condition below will silently skip the
            // local observations for those transports too.  At that point,
            // consider switching to an explicit per-transport-type check.
            if metrics.success_count + metrics.failure_count == 0 {
                metrics.success_count = self.success_count;
                metrics.failure_count = self.failure_count;
                metrics.delivery_ratio = self.delivery_ratio();
                metrics.drop_rate = self.drop_ratio();
            }
        }

        if let Some(latency) = self.latency_ema {
            metrics.latency_ms = Some(latency.round().clamp(0.0, u32::MAX as f32) as u32);
        }

        if let Some(hop) = self.hop_ema {
            metrics.average_hop_count = Some(hop);
        }
    }
}

fn update_ema(current: Option<f32>, new_value: f32, alpha: f32) -> f32 {
    let alpha = alpha.clamp(0.0, 1.0);
    match current {
        Some(existing) => existing * (1.0 - alpha) + new_value * alpha,
        None => new_value,
    }
}

impl TransportManager {
    /// Creates a new transport manager with default configuration.
    pub fn new(selector: TransportSelector) -> Self {
        Self {
            transports: HashMap::new(),
            current_transport: None,
            selector,
            observations: HashMap::new(),
            dors_event_callback: None,
            routing_decision_callback: None,
            routing_diagnostic: false,
            last_escalation_trigger_emitted: None,
            peer_binary_wire: HashSet::new(),
        }
    }

    /// Returns whether a peer is recorded as able to decode the binary wire
    /// codec (and would therefore be sent binary frames).
    pub fn peer_supports_binary_wire(&self, peer_id: &str) -> bool {
        self.peer_binary_wire.contains(peer_id)
    }

    /// Records whether a peer can decode our binary wire frames, learned from
    /// its signed key package. Messages addressed to a capable peer are stamped
    /// [`WireCodec::BinaryV1`] at send; all others stay JSON.
    pub fn mark_peer_binary_wire(&mut self, peer_id: &str, supported: bool) {
        if supported {
            // Bound the wire-keyed set: clear at capacity, then insert.
            if !self.peer_binary_wire.contains(peer_id)
                && self.peer_binary_wire.len() >= MAX_PEER_BINARY_WIRE_ENTRIES
            {
                self.peer_binary_wire.clear();
            }
            self.peer_binary_wire.insert(peer_id.to_string());
        } else {
            self.peer_binary_wire.remove(peer_id);
        }
    }

    /// Forwards a peer's advertised Nostr public key to the Nostr transport, so
    /// gift wraps addressed to them are sealed to a key only their install
    /// holds instead of to the publicly computable bootstrap key.
    ///
    /// `None` means "this peer advertises no Nostr key", which **forgets** any
    /// key already held rather than leaving it in place. That is what keeps the
    /// in-memory copy consistent with the durable record, whose downgrade
    /// semantics delete on an empty advertisement: retaining it here instead
    /// would keep sealing to a key the peer has stopped claiming, and survive
    /// only until the next restart — a divergence with no symptom.
    ///
    /// A no-op when the Nostr transport is not installed (the common case —
    /// `nostr_enabled` defaults off). The bound on the underlying map lives in
    /// the transport.
    pub fn mark_peer_nostr_pubkey(&mut self, peer_id: &str, pubkey_hex: Option<&str>) {
        let Some(transport) = self.transports.get(&TransportType::Nostr) else {
            return;
        };
        if let Some(nostr) = transport
            .as_any()
            .downcast_ref::<offline_protocol_transport::nostr::NostrTransport>()
        {
            match pubkey_hex {
                Some(pubkey_hex) => nostr.set_peer_nostr_pubkey(peer_id, pubkey_hex),
                None => nostr.forget_peer_nostr_pubkey(peer_id),
            }
        }
    }

    /// Forgets a peer's Nostr public key, so frames to them revert to the
    /// publicly computable bootstrap key.
    ///
    /// Used by the unblock clean slate, which declares everything learned about
    /// a peer stale. Reverting is the right fallback rather than refusing to
    /// send: the bootstrap key is derived from the peer's user id, so it works
    /// whatever install they are on now.
    pub fn clear_peer_nostr_pubkey(&mut self, peer_id: &str) {
        if let Some(nostr) = self.transports.get(&TransportType::Nostr).and_then(|t| {
            t.as_any()
                .downcast_ref::<offline_protocol_transport::nostr::NostrTransport>()
        }) {
            nostr.forget_peer_nostr_pubkey(peer_id);
        }
    }

    /// This install's Nostr public key, for advertising in outgoing key
    /// packages. `None` when the Nostr transport is not installed.
    pub fn nostr_public_key(&self) -> Option<String> {
        self.transports
            .get(&TransportType::Nostr)?
            .as_any()
            .downcast_ref::<offline_protocol_transport::nostr::NostrTransport>()
            .map(|nostr| nostr.public_key_hex())
    }

    /// Applies the sealed-envelope kill switch to the Nostr transport.
    pub fn set_nostr_sealing_enabled(&mut self, enabled: bool) {
        if let Some(nostr) = self.transports.get(&TransportType::Nostr).and_then(|t| {
            t.as_any()
                .downcast_ref::<offline_protocol_transport::nostr::NostrTransport>()
        }) {
            nostr.set_sealing_enabled(enabled);
        }
    }

    /// Applies the cold-contact kill switch to the Nostr transport.
    pub fn set_nostr_cold_contact_enabled(&mut self, enabled: bool) {
        if let Some(nostr) = self.nostr_transport() {
            nostr.set_cold_contact_enabled(enabled);
        }
    }

    /// Queues a key-package record for publication at `slot_id`.
    ///
    /// A no-op when the Nostr transport is not installed, which is the common
    /// case: publication is driven from the engine's tick, and that tick runs
    /// whether or not Nostr is enabled.
    ///
    /// Serializes through the transport's own chokepoint, which yields JSON for
    /// an unstamped message — and a published record must never be anything
    /// else. The binary codec is negotiated per peer from that peer's key
    /// package, and the reader of this record is by definition a stranger whose
    /// key package we have not seen. Emitting binary here would make the record
    /// unreadable by exactly the audience it exists for.
    pub fn publish_nostr_key_package(&mut self, slot_id: &str, message: &Message) -> Result<()> {
        let Some(nostr) = self.nostr_transport() else {
            return Ok(());
        };
        let payload = nostr.serialize_message(message)?;
        nostr.publish_key_package(slot_id, payload);
        Ok(())
    }

    /// Drains the slot ids whose publication never reached a relay.
    ///
    /// Empty when Nostr is not installed, which keeps the caller's tick free of
    /// a transport check it would otherwise need.
    pub fn take_failed_nostr_publications(&self) -> Vec<String> {
        self.nostr_transport()
            .map(|nostr| nostr.take_failed_publications())
            .unwrap_or_default()
    }

    /// Whether the Nostr transport is installed and publishing records.
    pub fn nostr_cold_contact_active(&self) -> bool {
        self.nostr_transport()
            .map(|nostr| nostr.cold_contact_enabled())
            .unwrap_or(false)
    }

    fn nostr_transport(&self) -> Option<&offline_protocol_transport::nostr::NostrTransport> {
        self.transports.get(&TransportType::Nostr).and_then(|t| {
            t.as_any()
                .downcast_ref::<offline_protocol_transport::nostr::NostrTransport>()
        })
    }

    /// Sets the callback for DORS decision events (dors_score_updated, dors_transport_selected,
    pub fn set_dors_event_callback(&mut self, callback: Option<Arc<dyn Fn(Event) + Send + Sync>>) {
        self.dors_event_callback = callback;
    }

    /// Sets the callback that receives structured
    /// [`crate::telemetry::RoutingDecision`] records.
    ///
    /// Fires at the same sites as `dors_event_callback`, carrying a richer
    /// shape suitable for the unified telemetry sink.
    ///
    /// Crate-private: apps must drive routing telemetry through
    /// [`crate::OfflineProtocol::install_telemetry_sink`] rather than wiring
    /// the callback directly. This keeps [`crate::telemetry::TelemetryConfig`]
    /// the single source of truth for sink configuration.
    pub(crate) fn set_routing_decision_callback(
        &mut self,
        callback: Option<Arc<dyn Fn(RoutingDecision) + Send + Sync>>,
    ) {
        self.routing_decision_callback = callback;
    }

    /// Enables or disables the routing-diagnostic detail level.
    ///
    /// When `true`, [`RoutingDecision::scores`] is populated with the full
    /// seven-factor [`TransportScore`] for every ranked transport on every
    /// emission. When `false` (default), the vector is empty and the hot
    /// path avoids the per-emission allocation. Mirrors
    /// `TelemetryConfig::routing_diagnostic`.
    ///
    /// Crate-private: see [`Self::set_routing_decision_callback`].
    pub(crate) fn set_routing_diagnostic(&mut self, enabled: bool) {
        self.routing_diagnostic = enabled;
    }

    fn emit_dors_event(&self, event: Event) {
        if let Some(ref cb) = self.dors_event_callback {
            cb(event);
        }
    }

    /// Lazy-emit a [`RoutingDecision`]. The `build` closure only runs when a
    /// callback is actually installed, so the no-sink path pays only an
    /// `Option::is_some` check — not a `Utc::now()` call and struct build.
    fn emit_routing_decision<F: FnOnce() -> RoutingDecision>(&self, build: F) {
        if let Some(ref cb) = self.routing_decision_callback {
            cb(build());
        }
    }

    fn escalation_trigger_reason_to_code(r: EscalationTriggerReason) -> DorsEscalationReasonCode {
        match r {
            EscalationTriggerReason::RetryThreshold => DorsEscalationReasonCode::RetryThreshold,
            EscalationTriggerReason::PoorSignal => DorsEscalationReasonCode::PoorSignal,
            EscalationTriggerReason::Congestion => DorsEscalationReasonCode::Congestion,
            EscalationTriggerReason::LowTtl => DorsEscalationReasonCode::LowTtl,
            EscalationTriggerReason::LowSuccessRate => DorsEscalationReasonCode::LowSuccessRate,
        }
    }

    fn dors_reason_code_to_routing(code: DorsReasonCode) -> RoutingReasonCode {
        match code {
            DorsReasonCode::InitialSelection => RoutingReasonCode::InitialSelection,
            DorsReasonCode::PrimarySelected => RoutingReasonCode::PrimarySelected,
            DorsReasonCode::PrimarySuccess => RoutingReasonCode::PrimarySuccess,
            DorsReasonCode::FallbackSuccess => RoutingReasonCode::FallbackSuccess,
            DorsReasonCode::EscalationApplied => RoutingReasonCode::EscalationApplied,
            DorsReasonCode::CurrentUnavailable => RoutingReasonCode::CurrentUnavailable,
        }
    }

    fn escalation_reason_code_to_routing(code: DorsEscalationReasonCode) -> RoutingReasonCode {
        match code {
            DorsEscalationReasonCode::FallbackSuccess => RoutingReasonCode::FallbackSuccess,
            DorsEscalationReasonCode::RetryThreshold => RoutingReasonCode::RetryThreshold,
            DorsEscalationReasonCode::PoorSignal => RoutingReasonCode::PoorSignal,
            DorsEscalationReasonCode::Congestion => RoutingReasonCode::Congestion,
            DorsEscalationReasonCode::LowTtl => RoutingReasonCode::LowTtl,
            DorsEscalationReasonCode::LowSuccessRate => RoutingReasonCode::LowSuccessRate,
        }
    }

    /// Builds a [`RoutingDecision`]. The `detailed_scores` slice is cloned
    /// into `scores` when `routing_diagnostic` is enabled on the manager;
    /// otherwise the vector stays empty (allocation-free).
    ///
    /// Takes `now_ms` by argument rather than calling `Utc::now()` per
    /// build so every decision emitted from a single `send()` cycle shares
    /// one timestamp — easier to correlate downstream and saves three to
    /// four syscalls on the send hot path.
    #[allow(clippy::too_many_arguments)]
    fn build_routing_decision(
        &self,
        now_ms: i64,
        phase: RoutingPhase,
        from: Option<TransportType>,
        to: Option<TransportType>,
        winning_score: Option<f32>,
        reason_code: Option<RoutingReasonCode>,
        detailed_scores: Option<&[(TransportType, TransportScore)]>,
    ) -> RoutingDecision {
        // Sanitize the internal fallback-demotion sentinel out of every score
        // that leaves the engine. DORS keeps Internet's `total` negative to rank
        // it last; telemetry/UniFFI consumers must see the real 0–125 quality
        // score instead. This is the single chokepoint for all RoutingDecision
        // emitters, so `winning_score` is also sanitized here defensively
        // (idempotent for an already-positive value).
        let scores = match (self.routing_diagnostic, detailed_scores) {
            (true, Some(detailed)) => detailed
                .iter()
                .map(|(t, s)| {
                    let mut s = s.clone();
                    s.total = display_routing_score(s.total);
                    (*t, s)
                })
                .collect(),
            _ => Vec::new(),
        };
        RoutingDecision {
            timestamp_ms: now_ms,
            phase,
            from,
            to,
            winning_score: winning_score.map(display_routing_score),
            reason_code,
            scores,
        }
    }

    /// Emit dors_escalation_triggered at trigger boundary (typed reason), deduped by reason + time window.
    fn emit_escalation_trigger_if_deduped(
        &mut self,
        now_ms: i64,
        reason_code: DorsEscalationReasonCode,
        from: TransportType,
        to: TransportType,
        reason_detail: Option<String>,
        detailed_scores: Option<&[(TransportType, TransportScore)]>,
    ) {
        let now = std::time::Instant::now();
        let emit = match &self.last_escalation_trigger_emitted {
            Some((last_code, last_at)) => {
                *last_code != reason_code
                    || last_at.elapsed().as_secs() >= ESCALATION_TRIGGER_DEDUPE_SECS
            }
            None => true,
        };
        if emit {
            self.last_escalation_trigger_emitted = Some((reason_code, now));
            self.emit_dors_event(Event::dors_escalation_triggered(
                DorsEscalationPhase::Triggered,
                from.to_string(),
                to.to_string(),
                reason_code,
                reason_detail,
            ));
            self.emit_routing_decision(|| {
                self.build_routing_decision(
                    now_ms,
                    RoutingPhase::Escalated,
                    Some(from),
                    Some(to),
                    None,
                    Some(Self::escalation_reason_code_to_routing(reason_code)),
                    detailed_scores,
                )
            });
        }
    }

    /// Adds a transport to the manager.
    ///
    /// # Arguments
    ///
    /// * `transport_type` - Type of transport to add
    /// * `transport` - The transport implementation
    pub fn add_transport(&mut self, transport_type: TransportType, transport: Box<dyn Transport>) {
        self.transports.insert(transport_type, Arc::from(transport));
    }

    /// Sends a message through the best available transport, with fallback.
    ///
    /// DORS selects a primary transport (applying hysteresis, cooldown, and
    /// stability checks). If the primary's `send()` fails synchronously, the
    /// remaining transports are tried in descending score order before
    /// returning an error. Each synchronous failure is recorded via
    /// `record_retry_failure` so DORS can adjust future scoring.
    ///
    /// # Internet transport — fallback does NOT apply
    ///
    /// Internet `send()` enqueues the message and returns `Ok(())`
    /// immediately. The actual wire-level outcome is reported asynchronously
    /// via the `confirm_sent` / `report_send_failure` confirmation loop on
    /// `InternetTransport`. Because `send()` never fails synchronously, the
    /// fallback loop below will **not** trigger for Internet. If the
    /// WebSocket is silently broken (status still `Available` but sends
    /// fail), messages will be enqueued, detected as failures by the
    /// confirmation-loop timeout, and counted in DORS metrics — but they
    /// are **not** retried on an alternative transport by this method.
    /// Retry/re-routing for those messages is handled by the higher-level
    /// outbox retry mechanism.
    pub fn send(&mut self, message: &Message) -> Result<()> {
        // Stamp the negotiated wire codec for this recipient. The clone happens
        // only on the binary path (peers known to support it); JSON and unknown
        // peers reuse the borrowed message. The internet transport ignores the
        // stamp (its relay bridge is JSON-only), and mesh transports only ever
        // emit to a recipient that is a directly connected peer, so `recipient`
        // is the physical peer whose capability was recorded — keying by it
        // matches the same peer-string the MTU path already keys by.
        //
        // The codec travels on the message, and both `Transport::send` and this
        // method take `&Message` as public API, so carrying the stamp across
        // that boundary needs an owned message — hence this clone. Note it is a
        // full-message clone: it copies `binary_content` (up to the max message
        // size), not just the envelope, so for media-sized payloads sent to a
        // binary-capable peer it is a real payload memcpy on the hot path. It is
        // a deliberate tradeoff: threading the codec as a separate argument would
        // shave the clone but is a breaking change to the public `Transport`
        // trait every downstream implementor relies on. The transport's own
        // queue-clone is unaffected either way. If profiling ever flags this,
        // stamp via a lightweight wrapper rather than cloning the payload.
        let stamped;
        let message = if message.wire_codec() != WireCodec::BinaryV1
            && self.peer_binary_wire.contains(message.recipient.as_str())
        {
            let mut m = message.clone();
            m.set_wire_codec(WireCodec::BinaryV1);
            stamped = m;
            &stamped
        } else {
            message
        };

        let available = self.get_available_transports();
        if available.is_empty() {
            return Err(Error::Transport(TransportError::TransportNotAvailable(
                "No available transport".to_string(),
            )));
        }

        // Single timestamp shared by every RoutingDecision emitted from this
        // send cycle — cheaper than per-emit `Utc::now()` and lets downstream
        // consumers group decisions by timestamp.
        let now_ms = Utc::now().timestamp_millis();

        let previous = self.current_transport;

        // DORS selects the primary transport (applies hysteresis/cooldown/stability).
        let primary = self
            .selector
            .select_transport(message, &available)
            .ok_or_else(|| {
                Error::Transport(TransportError::TransportNotAvailable(
                    "No suitable transport selected from available transports".to_string(),
                ))
            })?;

        // Emit DORS observability: scores and selection (before send attempt).
        // At the default (non-diagnostic) tier we project the detailed
        // breakdown down to `(transport, total)` — a single scoring pass
        // feeds both the legacy event and the routing record.
        let detailed_scores: Option<Vec<(TransportType, TransportScore)>> =
            if self.routing_diagnostic && self.routing_decision_callback.is_some() {
                Some(self.selector.score_and_rank_detailed(message, &available))
            } else {
                None
            };
        let detailed_slice = detailed_scores.as_deref();
        let scored: Vec<(TransportType, f32)> = match detailed_slice {
            Some(d) => d.iter().map(|(t, s)| (*t, s.total)).collect(),
            None => self.selector.score_and_rank(message, &available),
        };
        // Sanitize the fallback-demotion sentinel out of the emitted scores (the
        // `scored` vec itself keeps the raw totals — the fallback loop below
        // relies on Internet ranking last).
        let scores: Vec<(String, f32)> = scored
            .iter()
            .map(|(t, s)| (t.to_string(), display_routing_score(*s)))
            .collect();
        self.emit_dors_event(Event::dors_score_updated(scores));

        self.emit_routing_decision(|| {
            self.build_routing_decision(
                now_ms,
                RoutingPhase::ScoreUpdated,
                previous,
                None,
                None,
                None,
                detailed_slice,
            )
        });
        // Telemetry-only; sanitize the fallback-demotion sentinel so the emitted
        // selection score is the real 0–125 quality value, not the negative rank.
        let primary_score = scored
            .iter()
            .find(|(t, _)| *t == primary)
            .map(|(_, s)| display_routing_score(*s))
            .unwrap_or(0.0);
        let selection_reason = if previous.is_none() {
            DorsReasonCode::InitialSelection
        } else {
            DorsReasonCode::PrimarySelected
        };
        self.emit_dors_event(Event::dors_transport_selected(
            previous.map(|t| t.to_string()),
            primary.to_string(),
            selection_reason,
            primary_score,
        ));
        self.emit_routing_decision(|| {
            self.build_routing_decision(
                now_ms,
                RoutingPhase::Selected,
                previous,
                Some(primary),
                Some(primary_score),
                Some(Self::dors_reason_code_to_routing(selection_reason)),
                detailed_slice,
            )
        });

        // Try the primary transport first.
        let primary_result = {
            let transport = self
                .transports
                .get(&primary)
                .ok_or_else(|| Error::Other(format!("Transport {:?} not found", primary)))?;
            transport.send(message)
        };

        match primary_result {
            Ok(()) => {
                self.current_transport = Some(primary);
                if previous != Some(primary) {
                    let reason_code = if previous.is_some_and(|p| !available.contains_key(&p)) {
                        DorsReasonCode::CurrentUnavailable
                    } else {
                        DorsReasonCode::PrimarySuccess
                    };
                    self.emit_dors_event(Event::dors_transport_switched(
                        previous.map(|t| t.to_string()),
                        primary.to_string(),
                        reason_code,
                        None,
                    ));
                    self.emit_routing_decision(|| {
                        self.build_routing_decision(
                            now_ms,
                            RoutingPhase::Switched,
                            previous,
                            Some(primary),
                            Some(primary_score),
                            Some(Self::dors_reason_code_to_routing(reason_code)),
                            detailed_slice,
                        )
                    });
                }
                return Ok(());
            }
            Err(e) => {
                let is_peer_routing = matches!(e, TransportError::PeerNotReachable(_));
                if is_peer_routing {
                    debug!(
                        transport = ?primary,
                        error = %e,
                        "Peer not reachable via primary transport, trying fallback"
                    );
                } else {
                    warn!(
                        transport = ?primary,
                        error = %e,
                        "Primary transport send failed, trying fallback"
                    );
                    self.selector.record_retry_failure(primary);
                }
            }
        }

        // Primary failed — try remaining transports in score order.
        // Reuse the scored ranking computed above; record_retry_failure only
        // mutates retry_counts which score_and_rank does not read.
        let mut last_error = None;
        let mut attempted: Vec<TransportType> = vec![primary];

        for (transport_type, _score) in &scored {
            if *transport_type == primary {
                continue;
            }

            // Emit escalation trigger at DORS boundary (typed reason), deduped.
            if primary == TransportType::BLE && *transport_type == TransportType::WiFiDirect {
                if let Some(trigger_reason) = self.selector.escalation_trigger_reason() {
                    let reason_code = Self::escalation_trigger_reason_to_code(trigger_reason);
                    self.emit_escalation_trigger_if_deduped(
                        now_ms,
                        reason_code,
                        primary,
                        *transport_type,
                        None,
                        detailed_slice,
                    );
                }
            }

            let transport = match self.transports.get(transport_type) {
                Some(t) => t,
                None => continue,
            };

            attempted.push(*transport_type);

            match transport.send(message) {
                Ok(()) => {
                    self.current_transport = Some(*transport_type);
                    self.selector.set_current_transport(*transport_type);
                    // Emit switch only when active transport actually changed (fallback success).
                    if previous != Some(*transport_type) {
                        let reason_code = if primary == TransportType::BLE
                            && *transport_type == TransportType::WiFiDirect
                        {
                            DorsReasonCode::EscalationApplied
                        } else {
                            DorsReasonCode::FallbackSuccess
                        };
                        self.emit_dors_event(Event::dors_transport_switched(
                            previous.map(|t| t.to_string()),
                            transport_type.to_string(),
                            reason_code,
                            Some("primary send failed, fallback succeeded".to_string()),
                        ));
                        let fallback = *transport_type;
                        self.emit_routing_decision(|| {
                            self.build_routing_decision(
                                now_ms,
                                RoutingPhase::Switched,
                                previous,
                                Some(fallback),
                                None,
                                Some(Self::dors_reason_code_to_routing(reason_code)),
                                detailed_slice,
                            )
                        });
                    }
                    // Escalation applied only when BLE→WiFi fallback actually succeeded.
                    if primary == TransportType::BLE && *transport_type == TransportType::WiFiDirect
                    {
                        self.emit_dors_event(Event::dors_escalation_triggered(
                            DorsEscalationPhase::Applied,
                            primary.to_string(),
                            transport_type.to_string(),
                            DorsEscalationReasonCode::FallbackSuccess,
                            Some("primary BLE send failed, fallback to WiFi succeeded".to_string()),
                        ));
                        let escalated = *transport_type;
                        self.emit_routing_decision(|| {
                            self.build_routing_decision(
                                now_ms,
                                RoutingPhase::Escalated,
                                Some(primary),
                                Some(escalated),
                                None,
                                Some(Self::escalation_reason_code_to_routing(
                                    DorsEscalationReasonCode::FallbackSuccess,
                                )),
                                detailed_slice,
                            )
                        });
                    }
                    return Ok(());
                }
                Err(e) => {
                    if !matches!(e, TransportError::PeerNotReachable(_)) {
                        warn!(
                            transport = ?transport_type,
                            error = %e,
                            "Fallback transport send failed, trying next"
                        );
                        self.selector.record_retry_failure(*transport_type);
                    }
                    last_error = Some(e);
                }
            }
        }

        let terminal_error = last_error.unwrap_or_else(|| {
            TransportError::SendFailed(format!("All transports failed (tried {:?})", attempted))
        });
        Err(Error::Transport(terminal_error))
    }

    /// Sends a message via a specific transport, bypassing DORS selection.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to send
    /// * `transport_type` - The transport to use
    ///
    /// # Returns
    ///
    /// Returns Ok(()) if sent successfully, Err otherwise.
    pub fn send_via_transport(
        &mut self,
        message: &Message,
        transport_type: TransportType,
    ) -> Result<()> {
        let transport = self
            .transports
            .get(&transport_type)
            .ok_or_else(|| Error::Other(format!("Transport {:?} not found", transport_type)))?;

        if transport.status() != TransportStatus::Available {
            return Err(Error::Other(format!(
                "Transport {:?} is not available",
                transport_type
            )));
        }

        transport
            .send(message)
            .map_err(|e| Error::Other(format!("Transport send failed: {}", e)))?;

        // Only update current transport after successful send
        self.current_transport = Some(transport_type);

        Ok(())
    }

    /// Lists every peer reachable over a direct mesh link, with the transport
    /// that link runs on.
    ///
    /// Only the peer-to-peer carriers are consulted: a mesh neighbor is one we
    /// can hand a frame to over our own radio, which is what makes it a
    /// candidate next hop. Infrastructure carriers (Internet, Nostr, Reticulum)
    /// reach peers through something else, so they report no links and appear
    /// here as nothing — a message routed through them is not taking a mesh
    /// hop, it is leaving the mesh.
    ///
    /// A peer visible on more than one carrier is reported once, on the first
    /// carrier that claims it; [`Self::send_to_neighbor`] resolves the same way,
    /// so the pair stay consistent.
    pub fn mesh_neighbors(&self) -> Vec<MeshNeighbor> {
        let mut neighbors: Vec<MeshNeighbor> = Vec::new();

        for transport_type in MESH_TRANSPORTS {
            let Some(transport) = self.transports.get(transport_type) else {
                continue;
            };
            for link in transport.connected_peers() {
                if neighbors.iter().any(|n| n.peer_id == link.peer_id) {
                    continue;
                }
                neighbors.push(MeshNeighbor {
                    peer_id: link.peer_id,
                    rssi: link.rssi,
                    transport: *transport_type,
                });
            }
        }

        neighbors
    }

    /// Whether a frame for `peer_id` can plausibly arrive without being carried
    /// by other devices.
    ///
    /// This exists because **a send returning `Ok` is not evidence of
    /// reachability**. Only BLE refuses a recipient it holds no link to; Wi-Fi
    /// Direct and Reticulum enqueue for any recipient and report success, so a
    /// frame handed to them for someone out of range is queued for a link that
    /// will never drain — reported as sent, silently swallowed. Anything that
    /// decides "the mesh has to carry this" from a send failure therefore never
    /// fires on a device where one of those carriers is up. Asking this instead
    /// keeps that decision on facts we can check.
    ///
    /// Two ways the answer is yes:
    ///
    /// - An **infrastructure** carrier is available. Internet, Nostr and
    ///   Reticulum do their own routing and reach peers we hold no radio link
    ///   to, so a direct send is a real delivery attempt. (Reticulum counts
    ///   here because it is a routing network in its own right — its `Ok` means
    ///   "accepted for routing", which is as much as any relay-style carrier
    ///   promises.)
    /// - The peer is one of our own **mesh neighbors**, so a mesh carrier can
    ///   hand the frame straight over.
    ///
    /// Checked in that order so the common online case costs a status scan and
    /// never enumerates links.
    pub fn can_reach_without_carrying(&self, peer_id: &str) -> bool {
        let has_infrastructure = self.transports.iter().any(|(transport_type, transport)| {
            !MESH_TRANSPORTS.contains(transport_type)
                && transport.status() == TransportStatus::Available
        });
        if has_infrastructure {
            return true;
        }

        MESH_TRANSPORTS.iter().any(|transport_type| {
            self.transports
                .get(transport_type)
                .filter(|transport| transport.status() == TransportStatus::Available)
                .is_some_and(|transport| {
                    transport
                        .connected_peers()
                        .iter()
                        .any(|link| link.peer_id == peer_id)
                })
        })
    }

    /// Whether `transport_type` can actually put a frame in front of `peer_id`.
    ///
    /// The narrower question [`Self::can_reach_without_carrying`] answers across
    /// every carrier, asked of one. It exists for the same reason: a transport
    /// returning `Ok` is not evidence the frame can arrive. A **mesh** carrier
    /// can only address a peer it holds a live link to, and Wi-Fi Direct
    /// enqueues for any recipient regardless — so a frame handed to it for
    /// someone several hops away is queued for a link that never drains.
    /// **Infrastructure** carriers do their own routing, so for them the answer
    /// is yes whenever they are available.
    ///
    /// Callers that would otherwise trust a preferred transport — replying on
    /// the link a message arrived over, above all — must ask this first, or the
    /// reply dies on exactly the multi-hop path that made forwarding necessary.
    pub fn can_address_via(&self, transport_type: TransportType, peer_id: &str) -> bool {
        let Some(transport) = self.transports.get(&transport_type) else {
            return false;
        };
        if transport.status() != TransportStatus::Available {
            return false;
        }
        if !MESH_TRANSPORTS.contains(&transport_type) {
            return true;
        }
        transport
            .connected_peers()
            .iter()
            .any(|link| link.peer_id == peer_id)
    }

    /// Hands `message` to a specific mesh neighbor, whatever the message's own
    /// recipient is.
    ///
    /// This is the forwarding primitive: the frame crosses one link, to the
    /// peer named by `peer_id`. Because that peer is the physical target, the
    /// binary wire codec is negotiated against **its** capabilities rather than
    /// the recipient's — stamping a forwarded frame binary because the distant
    /// recipient supports it could put bytes a neighbor cannot decode onto that
    /// neighbor's link.
    ///
    /// Returns the transport the frame went out on.
    pub fn send_to_neighbor(&self, peer_id: &str, message: &Message) -> Result<TransportType> {
        let stamped;
        let message = if message.wire_codec() != WireCodec::BinaryV1
            && self.peer_binary_wire.contains(peer_id)
        {
            let mut m = message.clone();
            m.set_wire_codec(WireCodec::BinaryV1);
            stamped = m;
            &stamped
        } else if message.wire_codec() == WireCodec::BinaryV1
            && !self.peer_binary_wire.contains(peer_id)
        {
            // The frame arrived stamped binary (or was stamped for a different
            // peer) but this hop cannot decode it. Fall back to the JSON floor
            // rather than writing bytes the neighbor will drop.
            let mut m = message.clone();
            m.set_wire_codec(WireCodec::Json);
            stamped = m;
            &stamped
        } else {
            message
        };

        let mut last_error: Option<String> = None;

        for transport_type in MESH_TRANSPORTS {
            let Some(transport) = self.transports.get(transport_type) else {
                continue;
            };
            if transport.status() != TransportStatus::Available {
                continue;
            }
            match transport.send_to_peer(peer_id, message) {
                Ok(()) => return Ok(*transport_type),
                Err(e) => last_error = Some(e.to_string()),
            }
        }

        Err(Error::Transport(TransportError::PeerNotReachable(format!(
            "no mesh transport holds a link to {}{}",
            peer_id,
            last_error.map(|e| format!(" ({})", e)).unwrap_or_default()
        ))))
    }

    /// Attempts to receive a message from any transport.
    ///
    /// # Returns
    ///
    /// Returns Ok(Some((TransportType, Message))) if a message was received, Ok(None) if no message available.
    pub fn receive(&self) -> Result<Option<(TransportType, Message)>> {
        // Check all transports for messages
        for (transport_type, transport) in &self.transports {
            match transport.receive() {
                Ok(Some(message)) => {
                    debug!(
                        transport = ?transport_type,
                        sender = %message.sender,
                        recipient = %message.recipient,
                        "TransportManager found message"
                    );
                    return Ok(Some((*transport_type, message)));
                }
                Ok(None) => {
                    // No message from this transport, continue checking others
                }
                Err(e) => {
                    warn!(
                        transport = ?transport_type,
                        error = %e,
                        "Transport receive error"
                    );
                }
            }
        }
        Ok(None)
    }

    /// Gets metrics for all available transports.
    ///
    /// # Returns
    ///
    /// Returns a map of transport type to metrics.
    pub fn get_available_transports(&self) -> HashMap<TransportType, TransportMetrics> {
        let mut available = HashMap::new();

        for (transport_type, transport) in &self.transports {
            let maybe_metrics = {
                let status = transport.status();
                if status == TransportStatus::Available {
                    Some(transport.metrics())
                } else {
                    debug!(
                        transport = ?transport_type,
                        status = ?status,
                        "Transport excluded from available set"
                    );
                    None
                }
            };

            if let Some(mut metrics) = maybe_metrics {
                if let Some(stats) = self.observations.get(transport_type) {
                    stats.apply_to_metrics(&mut metrics);
                }
                available.insert(*transport_type, metrics);
            }
        }

        available
    }

    /// Gets the current active transport type.
    pub fn current_transport(&self) -> Option<TransportType> {
        self.current_transport
    }

    /// Returns the current status of every registered transport, including
    /// those not currently [`TransportStatus::Available`].
    ///
    /// Used by the telemetry aggregator to detect state transitions; unlike
    /// [`TransportManager::get_available_transports`], this only reads each
    /// transport's status with no metrics work.
    pub fn get_all_transport_statuses(&self) -> HashMap<TransportType, TransportStatus> {
        let mut out = HashMap::with_capacity(self.transports.len());
        for (transport_type, transport) in &self.transports {
            out.insert(*transport_type, transport.status());
        }
        out
    }

    /// Returns the per-transport status map AND the available-only metrics
    /// map in a single pass.
    ///
    /// Equivalent to calling [`Self::get_all_transport_statuses`] and
    /// [`Self::get_available_transports`] back-to-back, but reads each
    /// transport's status once instead of twice. Used by the per-tick
    /// telemetry aggregator.
    pub fn snapshot_status_and_available(
        &self,
    ) -> (
        HashMap<TransportType, TransportStatus>,
        HashMap<TransportType, TransportMetrics>,
    ) {
        let mut statuses = HashMap::with_capacity(self.transports.len());
        let mut available = HashMap::new();

        for (transport_type, transport) in &self.transports {
            let status = transport.status();
            let metrics = if status == TransportStatus::Available {
                Some(transport.metrics())
            } else {
                None
            };
            statuses.insert(*transport_type, status);
            if let Some(mut metrics) = metrics {
                if let Some(stats) = self.observations.get(transport_type) {
                    stats.apply_to_metrics(&mut metrics);
                }
                available.insert(*transport_type, metrics);
            }
        }

        (statuses, available)
    }

    /// Starts all transports.
    pub fn start(&mut self) -> Result<()> {
        for (transport_type, transport) in &self.transports {
            transport.start().map_err(|e| {
                Error::Other(format!(
                    "Failed to start transport {:?}: {}",
                    transport_type, e
                ))
            })?;
        }
        Ok(())
    }

    /// Stops all transports.
    pub fn stop(&mut self) -> Result<()> {
        for (transport_type, transport) in &self.transports {
            transport.stop().map_err(|e| {
                Error::Other(format!(
                    "Failed to stop transport {:?}: {}",
                    transport_type, e
                ))
            })?;
        }
        Ok(())
    }

    /// Gets a shared handle to a specific transport.
    pub fn get_transport(&self, transport_type: TransportType) -> Option<Arc<dyn Transport>> {
        self.transports.get(&transport_type).cloned()
    }

    /// Removes a transport from the manager.
    ///
    /// # Arguments
    ///
    /// * `transport_type` - Type of transport to remove
    pub fn remove_transport(&mut self, transport_type: TransportType) {
        self.transports.remove(&transport_type);

        // Clear current transport if it was the one removed
        if self.current_transport == Some(transport_type) {
            self.current_transport = None;
        }

        self.observations.remove(&transport_type);
    }

    /// Gets a list of all active transport types.
    ///
    /// # Returns
    ///
    /// Returns a vector of transport types that are currently added to the manager.
    pub fn get_active_transports(&self) -> Vec<TransportType> {
        self.transports.keys().copied().collect()
    }

    /// Checks if DORS suggests escalating from BLE to WiFi Direct.
    pub fn should_escalate_to_wifi(&self) -> bool {
        self.selector.should_escalate_to_wifi()
    }

    /// Records a retry failure for the given transport.
    pub fn record_retry_failure(&mut self, transport_type: TransportType) {
        self.selector.record_retry_failure(transport_type);
    }

    /// Resets retry count for the given transport after successful delivery.
    pub fn reset_retry_count(&mut self, transport_type: TransportType) {
        self.selector.reset_retry_count(transport_type);
    }

    /// Records a successful end-to-end delivery for the given transport.
    pub fn record_delivery_success(
        &mut self,
        transport_type: TransportType,
        latency_ms: u32,
        hop_count: u8,
    ) {
        let stats = self.observations.entry(transport_type).or_default();
        stats.record_success(latency_ms, hop_count);
    }

    /// Records a delivery failure (after exhausting retries) for the given transport.
    pub fn record_delivery_failure(&mut self, transport_type: TransportType) {
        let stats = self.observations.entry(transport_type).or_default();
        stats.record_failure();
    }

    /// Updates the DORS selector configuration at runtime, preserving
    /// accumulated state (transport history, retry counts, signal tracking).
    pub fn update_selector_config(&mut self, config: DorsConfig) {
        self.selector.update_config(config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, UserId};
    use offline_protocol_router::DorsConfig;
    use offline_protocol_transport::{mock::MockTransport, TransportType};
    use std::sync::Mutex;

    fn create_test_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "Test message",
        )
    }

    #[test]
    fn test_transport_manager_creation() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let manager = TransportManager::new(selector);
        assert_eq!(manager.transports.len(), 0);
        assert!(manager.current_transport().is_none());
    }

    #[test]
    fn test_add_transport() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let transport = Box::new(MockTransport::new(TransportType::BLE));
        manager.add_transport(TransportType::BLE, transport);

        assert_eq!(manager.transports.len(), 1);
    }

    #[test]
    fn test_start_stop_through_shared_transport_handle() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);
        manager.add_transport(
            TransportType::BLE,
            Box::new(MockTransport::new(TransportType::BLE)),
        );

        // The handle shares the same transport the manager drives: start()
        // and stop() go through &self, no per-transport mutex involved.
        let handle = manager
            .get_transport(TransportType::BLE)
            .expect("transport registered");
        assert_eq!(handle.status(), TransportStatus::Unavailable);

        manager.start().unwrap();
        assert_eq!(handle.status(), TransportStatus::Available);

        manager.stop().unwrap();
        assert_eq!(handle.status(), TransportStatus::Disconnected);
    }

    #[test]
    fn test_send_message() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        manager.add_transport(TransportType::BLE, Box::new(transport));

        let message = create_test_message();
        let result = manager.send(&message);
        assert!(result.is_ok());
    }

    #[test]
    fn send_stamps_binary_only_for_capable_peer() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);
        let transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        manager.add_transport(TransportType::BLE, Box::new(transport));

        // Unknown peer -> JSON (the default).
        manager.send(&create_test_message()).unwrap();
        // Peer recorded as binary-capable -> the send is stamped binary.
        manager.mark_peer_binary_wire("bob", true);
        manager.send(&create_test_message()).unwrap();

        let mock = manager
            .transports
            .get(&TransportType::BLE)
            .unwrap()
            .as_any()
            .downcast_ref::<MockTransport>()
            .unwrap();
        let sent = mock.sent_messages();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].wire_codec(), WireCodec::Json);
        assert_eq!(sent[1].wire_codec(), WireCodec::BinaryV1);
    }

    #[test]
    fn send_reverts_to_json_after_peer_downgrades() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);
        let transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        manager.add_transport(TransportType::BLE, Box::new(transport));

        // Learn the peer is binary-capable -> the next send is stamped binary.
        manager.mark_peer_binary_wire("bob", true);
        assert!(manager.peer_supports_binary_wire("bob"));
        manager.send(&create_test_message()).unwrap();

        // A later key package that no longer advertises support downgrades the
        // peer (e.g. after a session reset, or the codec being turned off on
        // their side); subsequent sends must fall back to JSON, not keep
        // emitting binary a legacy build could not decode.
        manager.mark_peer_binary_wire("bob", false);
        assert!(!manager.peer_supports_binary_wire("bob"));
        manager.send(&create_test_message()).unwrap();

        let mock = manager
            .transports
            .get(&TransportType::BLE)
            .unwrap()
            .as_any()
            .downcast_ref::<MockTransport>()
            .unwrap();
        let sent = mock.sent_messages();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].wire_codec(), WireCodec::BinaryV1);
        assert_eq!(sent[1].wire_codec(), WireCodec::Json);
    }

    #[test]
    fn test_dors_events_emitted_when_callback_set() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        manager.add_transport(TransportType::BLE, Box::new(transport));

        let events: std::sync::Arc<Mutex<Vec<Event>>> = std::sync::Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        manager.set_dors_event_callback(Some(Arc::new(move |e| {
            events_clone.lock().unwrap().push(e);
        })));

        let message = create_test_message();
        let _ = manager.send(&message);

        let captured = events.lock().unwrap();
        assert!(
            captured
                .iter()
                .any(|e| matches!(e, crate::events::Event::DorsScoreUpdated { .. })),
            "expected dors_score_updated"
        );
        assert!(
            captured
                .iter()
                .any(|e| { matches!(e, crate::events::Event::DorsTransportSelected { .. }) }),
            "expected dors_transport_selected"
        );
    }

    #[test]
    fn test_receive_message() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        let message = create_test_message();
        transport.queue_message(message.clone());

        manager.add_transport(TransportType::BLE, Box::new(transport));

        let received = manager.receive().unwrap();
        assert!(matches!(received, Some((TransportType::BLE, _))));
    }

    #[test]
    fn test_record_delivery_success_enriches_metrics() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        manager.add_transport(TransportType::BLE, Box::new(transport));

        manager.record_delivery_success(TransportType::BLE, 150, 2);

        let metrics = manager.get_available_transports();
        let ble_metrics = metrics.get(&TransportType::BLE).expect("metrics");

        assert_eq!(ble_metrics.success_count, 1);
        assert_eq!(ble_metrics.failure_count, 0);
        assert!(ble_metrics.delivery_ratio.expect("delivery ratio") > 0.99);
        assert_eq!(ble_metrics.average_hop_count, Some(2.0));
        assert_eq!(ble_metrics.latency_ms, Some(150));
    }

    #[test]
    fn test_record_delivery_failure_enriches_metrics() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        manager.add_transport(TransportType::BLE, Box::new(transport));

        manager.record_delivery_failure(TransportType::BLE);
        let metrics = manager.get_available_transports();
        let ble_metrics = metrics.get(&TransportType::BLE).expect("metrics");

        assert_eq!(ble_metrics.success_count, 0);
        assert_eq!(ble_metrics.failure_count, 1);
        assert!(ble_metrics.drop_rate.expect("drop ratio") > 0.99);
    }
}
