//! Bluetooth Low Energy (BLE) transport implementation.
//!
//! This module provides the BLE transport layer for peer-to-peer communication.
//! It handles:
//! - Device discovery (advertising and scanning)
//! - GATT server/client operations
//! - Message transmission over BLE characteristics
//! - Message fragmentation for large payloads

use crate::constants::{
    BLE_FRAGMENT_TIMEOUT_SECS, BLE_MAX_FRAGMENT_ASSEMBLIES, BLE_MAX_FRAGMENT_COUNT,
    BLE_MAX_FRAGMENT_SIZE, DEFAULT_MAX_MESSAGE_SIZE, FRAGMENT_HEADER_FIXED, FRAGMENT_MAGIC,
    FRAGMENT_VERSION, MAX_REASONABLE_BLE_PAYLOAD,
};
use crate::{Result, SharedCallback, Transport, TransportMetrics, TransportStatus, TransportType};
use offline_protocol_core::Message;
use std::collections::{HashMap, VecDeque};
use std::convert::TryInto;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, SystemTime};

/// Information about a fragment assembly that was evicted due to capacity pressure.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FragmentEvictionInfo {
    /// ID of the message whose fragment assembly was evicted.
    pub message_id: String,
    /// Completion percentage (0–100) at the time of eviction.
    pub completion_percent: u8,
}

/// Callback invoked when a fragment assembly is evicted from the reassembly cache.
pub type FragmentEvictionCallback = Arc<dyn Fn(FragmentEvictionInfo) + Send + Sync>;

/// Peer device information
#[derive(Debug, Clone)]
pub struct PeerDevice {
    /// Device ID (user_id)
    pub device_id: String,
    /// BLE address (platform-specific)
    pub address: String,
    /// Signal strength in dBm
    pub rssi: i16,
    /// Last seen timestamp
    pub last_seen: std::time::SystemTime,
    /// Connection status
    pub connected: bool,
}

#[derive(Debug, Clone)]
struct DecodedFragment {
    message_id: String,
    fragment_index: u16,
    total_fragments: u16,
    data: Vec<u8>,
}

/// Reassembly buffer for incoming fragments
#[derive(Debug)]
struct FragmentAssembly {
    /// Total expected fragments
    total_fragments: u16,
    /// Received fragments (index -> data)
    fragments: HashMap<u16, Vec<u8>>,
    /// First fragment received time. Anchors the assembly-latency metric and
    /// the eviction-priority age penalty — NOT the idle timeout.
    started_at: SystemTime,
    /// Time the most recent fragment was inserted. The idle timeout in
    /// [`Self::cleanup_fragment_buffers`] is measured from this, not
    /// [`started_at`], so a large/slow multi-fragment message that is still
    /// arriving is not evicted mid-flight just because its first fragment is
    /// old (e.g. a backgrounded receiver, a 100-200ms connection interval, or
    /// an iOS->Android sender with no per-write pacing).
    last_seen: SystemTime,
}

impl FragmentAssembly {
    /// Returns the completion ratio (0.0 to 1.0) of this assembly.
    fn completion_ratio(&self) -> f32 {
        if self.total_fragments == 0 {
            return 0.0;
        }
        self.fragments.len() as f32 / self.total_fragments as f32
    }

    /// Returns a priority score for eviction (lower = more likely to be evicted).
    /// Prioritizes keeping near-complete assemblies.
    fn eviction_priority(&self, now: SystemTime) -> f32 {
        let completion = self.completion_ratio();

        // Age factor: older assemblies are slightly less valuable
        let age_secs = now
            .duration_since(self.started_at)
            .unwrap_or(StdDuration::from_secs(0))
            .as_secs_f32();
        let age_penalty = (age_secs / 60.0).min(1.0) * 0.2; // Max 20% penalty for age

        // Priority = completion ratio (0-1) minus age penalty
        // Higher value = more valuable = less likely to evict
        completion - age_penalty
    }
}

/// BLE transport implementation.
///
/// This is a platform-agnostic abstraction. The actual BLE operations
/// are delegated to platform-specific implementations via callbacks.
pub struct BleTransport {
    /// Local device ID
    device_id: String,
    /// Transport status
    status: Arc<Mutex<TransportStatus>>,
    /// Discovered peers
    peers: Arc<Mutex<HashMap<String, PeerDevice>>>,
    /// Received message queue
    receive_queue: Arc<Mutex<VecDeque<Message>>>,
    /// Send queue
    send_queue: Arc<Mutex<VecDeque<(String, Message)>>>,
    /// Pending serialized fragments waiting to be delivered
    #[allow(clippy::type_complexity)]
    pending_fragments: Arc<Mutex<VecDeque<(String, Vec<u8>)>>>,
    /// Transport metrics
    metrics: Arc<Mutex<TransportMetrics>>,
    /// Platform-specific handle (opaque pointer)
    platform_handle: Arc<Mutex<Option<usize>>>,
    /// Fragment reassembly buffers
    fragment_buffers: Arc<Mutex<HashMap<String, FragmentAssembly>>>,
    /// Per-peer maximum fragment payload in bytes, keyed by peer device id.
    ///
    /// Populated by the platform layer after BLE MTU negotiation completes
    /// for a given link. Stored values are already header-adjusted (i.e.
    /// "max usable payload", not the raw ATT MTU): iOS's
    /// `maximumWriteValueLength(for:)` returns this directly, and the Android
    /// facade subtracts the 3-byte ATT header before calling in. Peers with
    /// no entry fall back to [`BLE_MAX_FRAGMENT_SIZE`].
    ///
    /// **Keying contract:** entries MUST be keyed by the same string the
    /// platform layer passes to [`Self::on_peer_discovered`] as
    /// `PeerDevice::device_id`, which is also the string that lands in
    /// [`Message::recipient`] for outbound sends. Today the codebase treats
    /// the BLE device id and the peer's `UserId` as the same opaque string,
    /// so `fragment_message` can look up MTUs by `message.recipient`. If a
    /// future refactor ever introduces UserId ↔ device-id translation, this
    /// map MUST be rekeyed at the same time or `fragment_message` will
    /// silently fall back to [`BLE_MAX_FRAGMENT_SIZE`] for every peer with
    /// no compile-time warning.
    peer_mtus: Arc<Mutex<HashMap<String, usize>>>,
    /// Count of undersized MTU reports received from the platform layer.
    ///
    /// Incremented every time [`Self::set_peer_mtu`] takes the reject
    /// branch (payload < [`BLE_MAX_FRAGMENT_SIZE`]). A non-zero value in
    /// production is a signal that the "modern BLE controllers never
    /// negotiate below 185" assumption is being violated on some link —
    /// any such peer is silently falling back to the 185-byte floor,
    /// which is *higher* than the real usable payload, and every
    /// fragment written to it will be dropped by the controller. Expose
    /// via [`Self::undersized_mtu_reports`] so fleet telemetry can
    /// surface it.
    undersized_mtu_reports: Arc<AtomicU64>,
    /// Count of [`Self::fragment_message`] calls that fell back to the
    /// [`BLE_MAX_FRAGMENT_SIZE`] floor **and whose recipient is still a
    /// registered direct BLE peer** at the moment of fallback.
    ///
    /// In healthy operation this should remain **zero**. Both platforms
    /// push the MTU BEFORE announcing the peer to the protocol layer
    /// (iOS: `bleSetPeerMtu` precedes `blePeerDiscovered` in the
    /// DEVICE_ID read handler; Android: the facade flushes the staged
    /// MTU via `onDeviceIdResolved` before `blePeerDiscovered` fires),
    /// so by the time any fragmenting send can reach a peer the MTU
    /// entry is already on file. The `peers.contains_key` gate on the
    /// increment deliberately excludes the benign TOCTOU where
    /// `send()` enqueues while the peer is live and a concurrent
    /// `on_peer_lost` drops both maps before `get_next_fragment` pops
    /// the queued message — that race produces fragments to a dead
    /// link, not a broken invariant, and counting it would drown out
    /// the real signal every time a peer disconnects with in-flight
    /// sends.
    ///
    /// A non-zero value therefore means one of two things: the
    /// MTU-before-discover ordering invariant regressed on one of the
    /// platforms, or the `recipient → device_id` keying contract
    /// broke (e.g. a future multi-hop refactor where
    /// `message.recipient` is the final destination rather than the
    /// direct peer). Either way: every fragmenting send to live peers
    /// is silently regressing to 185 bytes with no compile-time
    /// error. Surface via [`Self::fragment_fallback_count`] so
    /// production alerts fire the first time the invariant breaks.
    fragment_fallback_count: Arc<AtomicU64>,
    /// Count of [`Transport::send`] calls that returned
    /// [`PeerNotReachable`](crate::Error::PeerNotReachable) for a recipient that
    /// was **not** among the connected BLE peers, *while other peers WERE
    /// connected* (the peers map was non-empty).
    ///
    /// "No BLE peers at all" is the ordinary offline case and is **not** counted —
    /// DORS escalating that send to Internet is exactly right. A non-empty map
    /// missing only this recipient is the suspicious case: the device is meshing
    /// with someone, just not the addressed id. The dominant cause is a
    /// `recipient` ↔ `ble_peer_discovered` device-id **keying mismatch** (e.g. an
    /// app addressing a message by a user id that differs from the advertised mesh
    /// id), which makes a physically-in-range peer look unreachable and silently
    /// routes the message to Internet. Surfacing it turns a silent misroute into
    /// an observable signal. Exposed via [`Self::recipient_not_among_peers_count`].
    recipient_not_among_peers_count: Arc<AtomicU64>,
    /// Platform callback invoked when new fragments are available to send.
    /// Called from `send()` after enqueueing — the platform layer should
    /// respond by calling `get_next_fragment()` and performing the BLE write.
    on_fragments_available: SharedCallback,
    /// Optional callback fired when a fragment assembly is evicted from the
    /// reassembly cache due to capacity pressure. Wired by the protocol layer
    /// to emit `Event::FragmentAssemblyEvicted`.
    eviction_callback: Arc<Mutex<Option<FragmentEvictionCallback>>>,
}

impl BleTransport {
    /// Creates a new BLE transport.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            status: Arc::new(Mutex::new(TransportStatus::Unavailable)),
            peers: Arc::new(Mutex::new(HashMap::new())),
            receive_queue: Arc::new(Mutex::new(VecDeque::new())),
            send_queue: Arc::new(Mutex::new(VecDeque::new())),
            pending_fragments: Arc::new(Mutex::new(VecDeque::new())),
            metrics: Arc::new(Mutex::new(TransportMetrics::default())),
            platform_handle: Arc::new(Mutex::new(None)),
            fragment_buffers: Arc::new(Mutex::new(HashMap::new())),
            peer_mtus: Arc::new(Mutex::new(HashMap::new())),
            undersized_mtu_reports: Arc::new(AtomicU64::new(0)),
            fragment_fallback_count: Arc::new(AtomicU64::new(0)),
            recipient_not_among_peers_count: Arc::new(AtomicU64::new(0)),
            on_fragments_available: Arc::new(Mutex::new(None)),
            eviction_callback: Arc::new(Mutex::new(None)),
        }
    }

    /// Registers a callback that fires when new outgoing fragments become available.
    ///
    /// The platform layer (Swift/Kotlin) implements this to wake up and call
    /// `get_next_fragment()` instead of polling on a timer.
    pub fn set_on_fragments_available(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.on_fragments_available.lock().unwrap() = Some(callback);
    }

    /// Registers a callback that fires when a fragment assembly is evicted
    /// from the reassembly cache due to capacity pressure.
    pub fn set_fragment_eviction_callback(&self, callback: Option<FragmentEvictionCallback>) {
        *self.eviction_callback.lock().unwrap() = callback;
    }

    /// Notifies the platform that fragments are ready to send.
    fn notify_fragments_available(&self) {
        let callback = self.on_fragments_available.lock().unwrap().clone();
        if let Some(cb) = callback {
            cb();
        }
    }

    /// Records the maximum usable fragment payload for a peer after BLE MTU
    /// negotiation.
    ///
    /// The platform layer passes the already-header-adjusted value — iOS via
    /// `CBPeripheral.maximumWriteValueLength(for:)`, Android via
    /// `onMtuChanged` followed by `mtu - 3`. Accepted values are clamped to
    /// [`MAX_REASONABLE_BLE_PAYLOAD`] on the high end to protect the
    /// fragmenter from native layers reporting nonsensical sizes.
    ///
    /// **Undersized values are rejected, not clamped up.** A report below
    /// [`BLE_MAX_FRAGMENT_SIZE`] means the link's real usable payload is
    /// smaller than our fallback floor — storing a clamped-up 185 would
    /// cause the fragmenter to emit chunks the controller cannot transmit
    /// and every fragment to that peer would be silently dropped. We warn
    /// and drop any previously stored entry so [`Self::peer_mtu`] falls
    /// back to [`BLE_MAX_FRAGMENT_SIZE`]. Dropping (rather than leaving
    /// the prior entry in place) is load-bearing for mid-link
    /// renegotiation: if a peer had a stored 400 and the controller
    /// later renegotiates down to 20, silently keeping 400 would still
    /// produce writes the new link cannot honor. Self-consistent with
    /// the invariant that we do not operate below that floor on the
    /// platforms we target (both iOS and Android request the BLE-5
    /// maximum ATT MTU and every modern controller negotiates well
    /// above 188).
    pub fn set_peer_mtu(&self, peer_id: &str, max_payload: usize) {
        // Symmetric with the debug warn in `fragment_message`: neither
        // platform ever calls `set_peer_mtu` for a peer it has not
        // already announced via `blePeerDiscovered`, so a store with
        // no matching `peers` entry is either a buggy platform layer
        // or a broken keying contract. Debug-only to keep the hot
        // path free of an extra mutex acquisition in release builds.
        #[cfg(debug_assertions)]
        {
            if !self.peers.lock().unwrap().contains_key(peer_id) {
                tracing::warn!(
                    peer = %peer_id,
                    max_payload,
                    "ble: set_peer_mtu for unregistered peer — platform must call \
                     on_peer_discovered before reporting MTU, otherwise the stored \
                     entry will not be cleared on peer loss"
                );
            }
        }
        if max_payload < BLE_MAX_FRAGMENT_SIZE {
            self.undersized_mtu_reports.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                peer = %peer_id,
                max_payload,
                floor = BLE_MAX_FRAGMENT_SIZE,
                "ble: undersized MTU report; dropping stored entry and falling back"
            );
            self.peer_mtus.lock().unwrap().remove(peer_id);
            return;
        }
        let clamped = max_payload.min(MAX_REASONABLE_BLE_PAYLOAD);
        tracing::debug!(
            peer = %peer_id,
            raw = max_payload,
            clamped,
            "ble: stored per-peer MTU"
        );
        self.peer_mtus
            .lock()
            .unwrap()
            .insert(peer_id.to_string(), clamped);
    }

    /// Removes any stored MTU for a peer (called on disconnect or before
    /// renegotiation).
    pub fn clear_peer_mtu(&self, peer_id: &str) {
        tracing::debug!(peer = %peer_id, "ble: cleared per-peer MTU");
        self.peer_mtus.lock().unwrap().remove(peer_id);
    }

    /// Returns the maximum fragment payload to use for a given peer, falling
    /// back to [`BLE_MAX_FRAGMENT_SIZE`] when no negotiated value is on file.
    pub fn peer_mtu(&self, peer_id: &str) -> usize {
        self.peer_mtus
            .lock()
            .unwrap()
            .get(peer_id)
            .copied()
            .unwrap_or(BLE_MAX_FRAGMENT_SIZE)
    }

    /// Monotonic count of undersized MTU reports since transport creation.
    ///
    /// A non-zero value means at least one peer reported a max usable
    /// payload below [`BLE_MAX_FRAGMENT_SIZE`] and is now being served
    /// the fallback floor — which is *higher* than the real link
    /// capacity, so outbound writes to that peer are being dropped by
    /// the controller. Surface in dashboards to detect controllers that
    /// violate the target-platform assumption.
    pub fn undersized_mtu_reports(&self) -> u64 {
        self.undersized_mtu_reports.load(Ordering::Relaxed)
    }

    /// Monotonic count of [`Self::fragment_message`] calls that sized
    /// against [`BLE_MAX_FRAGMENT_SIZE`] because no per-peer MTU had
    /// been recorded for the recipient.
    ///
    /// A small steady-state value is normal (one-shot window between
    /// `blePeerDiscovered` and `bleSetPeerMtu` for each new peer). A
    /// sustained non-trivial rate indicates the keying contract
    /// between `message.recipient` and the per-peer MTU map has
    /// broken — at which point every fragmenting send is silently
    /// regressing to the 185-byte floor. Surface in dashboards.
    pub fn fragment_fallback_count(&self) -> u64 {
        self.fragment_fallback_count.load(Ordering::Relaxed)
    }

    /// Monotonic count of [`Transport::send`] calls whose recipient was not among
    /// the connected BLE peers **while other peers were connected**.
    ///
    /// Zero in healthy operation. A sustained non-zero value means messages to
    /// in-range peers are being silently routed off BLE — almost always a
    /// `recipient` ↔ `ble_peer_discovered` device-id keying mismatch in the host
    /// app. Surface in dashboards alongside [`Self::fragment_fallback_count`].
    pub fn recipient_not_among_peers_count(&self) -> u64 {
        self.recipient_not_among_peers_count.load(Ordering::Relaxed)
    }

    /// Sets the platform-specific handle.
    ///
    /// This is called by the platform implementation to store its context.
    pub fn set_platform_handle(&self, handle: usize) {
        crate::common::set_platform_handle(&self.platform_handle, handle);
    }

    /// Gets the platform-specific handle.
    pub fn platform_handle(&self) -> Option<usize> {
        crate::common::platform_handle(&self.platform_handle)
    }

    /// Gets the local device ID.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Called when a peer device is discovered.
    pub fn on_peer_discovered(&self, peer: PeerDevice) {
        let mut peers = self.peers.lock().unwrap();
        peers.insert(peer.device_id.clone(), peer);
    }

    /// Called when a peer device is lost.
    ///
    /// Also drops any stored per-peer MTU so a reconnect cannot observe a
    /// stale value from the previous link. Platforms may still call
    /// [`Self::clear_peer_mtu`] directly for mid-link renegotiation paths.
    pub fn on_peer_lost(&self, device_id: &str) {
        self.peers.lock().unwrap().remove(device_id);
        self.peer_mtus.lock().unwrap().remove(device_id);
    }

    /// Called when a message is received from a peer.
    pub fn on_message_received(&self, message: Message) {
        crate::common::on_message_received(&self.receive_queue, message);
    }

    /// Like [`on_message_received`](Self::on_message_received), but attaches a
    /// transport-verified `peer_id` to the message.
    ///
    /// # Parameters
    ///
    /// * `peer_id` — The **user-level identifier** (i.e., the remote peer's
    ///   `UserId` string) that the transport layer has authenticated for this
    ///   connection. This is **not** the raw transport address (MAC, BLE device
    ///   UUID, etc.). The protocol layer uses this value to verify that
    ///   `message.sender` matches the physical peer that delivered it.
    pub fn on_message_received_from(&self, message: Message, peer_id: String) {
        crate::common::on_message_received_from(&self.receive_queue, message, peer_id);
    }

    /// Called when connection status changes.
    ///
    /// When transitioning away from [`TransportStatus::Available`], all
    /// per-session state is drained so a subsequent reconnect starts clean.
    /// The drain mirrors [`Transport::stop`] and respects the same lockstep
    /// ordering (peers → peer_mtus → fragment_buffers → send_queue →
    /// pending_fragments → receive_queue). Monotonic lifetime counters
    /// (`undersized_mtu_reports`, `fragment_fallback_count`) are preserved.
    pub fn on_status_changed(&self, status: TransportStatus) {
        let previous = {
            let mut guard = self.status.lock().unwrap();
            let prev = *guard;
            *guard = status;
            prev
        };

        if previous == TransportStatus::Available && status != TransportStatus::Available {
            self.peers.lock().unwrap().clear();
            self.peer_mtus.lock().unwrap().clear();
            self.fragment_buffers.lock().unwrap().clear();
            self.send_queue.lock().unwrap().clear();
            self.pending_fragments.lock().unwrap().clear();
            self.receive_queue.lock().unwrap().clear();
            self.update_queue_metric();
        }
    }

    /// Gets all discovered peers.
    pub fn get_peers(&self) -> Vec<PeerDevice> {
        let peers = self.peers.lock().unwrap();
        peers.values().cloned().collect()
    }

    /// Gets a specific peer by device ID.
    pub fn get_peer(&self, device_id: &str) -> Option<PeerDevice> {
        let peers = self.peers.lock().unwrap();
        peers.get(device_id).cloned()
    }

    /// Updates transport metrics.
    pub fn update_metrics(&self, metrics: TransportMetrics) {
        *self.metrics.lock().unwrap() = metrics;
    }

    fn update_queue_metric(&self) {
        let send_len = self.send_queue.lock().unwrap().len();
        let fragment_len = self.pending_fragments.lock().unwrap().len();
        let mut metrics = self.metrics.lock().unwrap();
        metrics.queue_depth = send_len + fragment_len;
        let heuristic_capacity = 50_f32;
        metrics.congestion = ((metrics.queue_depth as f32) / heuristic_capacity).clamp(0.0, 1.0);
    }

    /// Records a successful send for metrics tracking.
    pub fn record_send_success(&self) {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.success_count = metrics.success_count.saturating_add(1);
        let total = metrics.success_count + metrics.failure_count;
        if total > 0 {
            let ratio = metrics.success_count as f32 / total as f32;
            metrics.delivery_ratio = Some(ratio);
            metrics.drop_rate = Some((1.0 - ratio).clamp(0.0, 1.0));
        }
    }

    /// Records a failed send for metrics tracking.
    pub fn record_send_failure(&self) {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.failure_count = metrics.failure_count.saturating_add(1);
        let total = metrics.success_count + metrics.failure_count;
        if total > 0 {
            let drop_ratio = metrics.failure_count as f32 / total as f32;
            metrics.drop_rate = Some(drop_ratio.clamp(0.0, 1.0));
            metrics.delivery_ratio = Some((1.0 - drop_ratio).clamp(0.0, 1.0));
        }
    }

    fn record_latency(&self, latency_ms: u128) {
        let value = latency_ms.min(u128::from(u32::MAX)) as u32;
        let mut metrics = self.metrics.lock().unwrap();
        metrics.latency_ms = Some(match metrics.latency_ms {
            Some(existing) => {
                let ema = (existing as f32 * 0.7) + (value as f32 * 0.3);
                ema as u32
            }
            None => value,
        });
    }

    fn cleanup_fragment_buffers(&self) {
        let mut buffers = self.fragment_buffers.lock().unwrap();
        let now = SystemTime::now();
        let mut expired = Vec::new();

        for (message_id, assembly) in buffers.iter() {
            // Idle timeout: evict only buffers with no fragment received within
            // the window (measured from last_seen, not started_at), so a slow
            // but still-arriving message is never torn mid-assembly.
            if now
                .duration_since(assembly.last_seen)
                .unwrap_or_else(|_| StdDuration::from_secs(0))
                > StdDuration::from_secs(BLE_FRAGMENT_TIMEOUT_SECS)
            {
                expired.push(message_id.clone());
            }
        }

        for message_id in expired {
            buffers.remove(&message_id);
            self.record_send_failure();
            tracing::debug!(
                message_id = %message_id,
                "Dropped expired BLE fragment assembly"
            );
        }
    }

    /// Gets current queue depth for metrics.
    pub fn get_queue_depth(&self) -> usize {
        let send_len = self.send_queue.lock().unwrap().len();
        let fragment_len = self.pending_fragments.lock().unwrap().len();
        send_len + fragment_len
    }

    /// Processes the send queue (to be called by platform implementation).
    pub fn dequeue_send(&self) -> Option<(String, Message)> {
        let result = {
            let mut queue = self.send_queue.lock().unwrap();
            queue.pop_front()
        };

        if result.is_some() {
            self.update_queue_metric();
        }

        result
    }

    /// Checks if there are messages to send.
    pub fn has_pending_sends(&self) -> bool {
        let queue = self.send_queue.lock().unwrap();
        !queue.is_empty()
    }

    /// Serializes a message to bytes (JSON).
    pub fn serialize_message(&self, message: &Message) -> Result<Vec<u8>> {
        crate::common::serialize_message(message)
    }

    /// Deserializes a message from bytes (JSON).
    pub fn deserialize_message(&self, data: &[u8]) -> Result<Message> {
        crate::common::deserialize_message(data)
    }

    /// Fragments a message into chunks suitable for BLE transmission.
    ///
    /// The fragment payload size is chosen from the recipient's negotiated
    /// MTU — derived from `message.recipient`, which matches the key the
    /// platform layer uses when reporting negotiated values via
    /// [`Self::set_peer_mtu`]. Falls back to [`BLE_MAX_FRAGMENT_SIZE`] when
    /// no MTU has been reported yet.
    ///
    /// **Race window with [`Self::set_peer_mtu`]:** the MTU is read once
    /// at the top of this function and the lock is released before the
    /// fragment loop runs. A concurrent `set_peer_mtu` landing between
    /// the read and the emit produces a single batch of fragments sized
    /// against the stale value. This is harmless on the common upward-
    /// renegotiation path (old MTU was ≤ new MTU, so the old-sized
    /// fragments still fit). The only hazard is a *downward* mid-link
    /// renegotiation that stays above the floor (e.g. 400 → 200), where
    /// the in-flight batch may exceed the new link capacity. We accept
    /// that window rather than widening the critical section because
    /// (a) BLE controllers almost never renegotiate down, (b) the
    /// undersized-reject branch already covers the common "drop to
    /// floor" case, and (c) the reliability layer retransmits any
    /// dropped fragments under the new MTU — so recovery is automatic.
    /// Verified: `RetryEntry` at
    /// `crates/offline-protocol-reliability/src/retry_queue.rs:45-60`
    /// stores the full `Message`, not a cached fragment vector, so
    /// retries re-enter `fragment_message` under the current
    /// `peer_mtus` value rather than replaying stale chunks.
    /// If production telemetry ever shows sustained fragment loss on
    /// downward renegotiations, snapshot-then-revalidate at the pre-
    /// send boundary; do not hold `peer_mtus` across the whole loop,
    /// which would serialise every fragmenting send on a single peer.
    pub fn fragment_message(&self, message: &Message) -> Result<Vec<Vec<u8>>> {
        let message_bytes = self.serialize_message(message)?;

        let recipient = message.recipient.as_str();
        // Look up the per-peer MTU. On a miss, the `peers` map tells
        // us whether this is a keying-contract break (recipient is
        // still a registered direct peer but has no MTU on file — the
        // platform ordering invariant must have regressed, or
        // `recipient` no longer matches the direct-peer device_id
        // used to key `peer_mtus`) or the benign send/on_peer_lost
        // race (recipient was live when `send()` validated and
        // enqueued, but `on_peer_lost` dropped both maps in lockstep
        // before `get_next_fragment` popped the queued message —
        // fragments in that race go to a dead link, not a broken
        // invariant). Only the first case increments
        // `fragment_fallback_count`; counting the second would drown
        // out the real signal on every disconnect with in-flight
        // sends.
        //
        // The peers-lock acquisition happens only on the miss branch,
        // so the healthy hot path (MTU on file) pays just the
        // peer_mtus lock. With the platform-side MTU-before-discover
        // ordering (see commit ac8cce8), the miss branch should
        // effectively never execute for a live peer.
        //
        // **Lock-ordering note:** the peer_mtus lookup is hoisted into
        // its own statement so the MutexGuard drops before the miss
        // branch reaches for `peers.lock()`. Under Rust 2021 temporary-
        // scope rules, embedding the lookup in the `match` scrutinee
        // would keep the peer_mtus guard alive across the arm bodies
        // and establish an implicit `peer_mtus -> peers` ordering held
        // by a single thread — safe today (no path holds `peers` while
        // taking `peer_mtus`) but one refactor away from an AB/BA
        // deadlock. Keep this as two statements.
        let cached_mtu = self.peer_mtus.lock().unwrap().get(recipient).copied();
        let mtu = match cached_mtu {
            Some(mtu) => mtu,
            None => {
                let is_direct_peer = self.peers.lock().unwrap().contains_key(recipient);
                if is_direct_peer {
                    self.fragment_fallback_count.fetch_add(1, Ordering::Relaxed);
                    // Debug-only: surface the contract break with a
                    // warn in development builds so a refactor that
                    // regresses the ordering trips immediately. Release
                    // builds rely on the counter + dashboard alert.
                    #[cfg(debug_assertions)]
                    tracing::warn!(
                        peer = %recipient,
                        "ble: fragment_message found no MTU for a registered direct peer — \
                         MTU-before-discover ordering invariant regressed, or the \
                         recipient -> device_id keying contract broke"
                    );
                } else {
                    // Benign teardown race: `send()` enqueued while
                    // the peer was live, `on_peer_lost` dropped both
                    // maps before `get_next_fragment` got here. The
                    // fragments will be produced under the floor and
                    // routed to a now-absent peer — the reliability
                    // layer will retry under whatever transport is
                    // available next. Not a contract break.
                    #[cfg(debug_assertions)]
                    tracing::debug!(
                        peer = %recipient,
                        "ble: fragment_message for recipient no longer in direct-peer map — \
                         benign send/on_peer_lost race, not counted"
                    );
                }
                BLE_MAX_FRAGMENT_SIZE
            }
        };
        tracing::trace!(
            peer = %recipient,
            mtu,
            "ble: selected fragment payload size"
        );
        let message_id = message.id.as_str();
        let message_id_bytes = message_id.as_bytes();
        if message_id_bytes.len() > u8::MAX as usize {
            return Err(crate::Error::Other("Message ID too long".to_string()));
        }

        // Ensure fragment payload fits within MTU once headers are applied
        let header_overhead = FRAGMENT_HEADER_FIXED + message_id_bytes.len();
        if header_overhead >= mtu {
            return Err(crate::Error::Other(
                "MTU too small for fragment header".to_string(),
            ));
        }

        let max_fragment_payload = mtu - header_overhead;
        let total_fragments =
            (message_bytes.len() + max_fragment_payload - 1) / max_fragment_payload;
        if total_fragments == 0 {
            return Err(crate::Error::Other("Empty message".to_string()));
        }
        if total_fragments > BLE_MAX_FRAGMENT_COUNT {
            return Err(crate::Error::Other(
                "Message would require too many BLE fragments".to_string(),
            ));
        }

        if total_fragments > u16::MAX as usize {
            return Err(crate::Error::Other(
                "Message too large to fragment".to_string(),
            ));
        }

        let mut fragments = Vec::with_capacity(total_fragments);
        for (i, chunk) in message_bytes.chunks(max_fragment_payload).enumerate() {
            let encoded =
                encode_fragment(message_id_bytes, i as u16, total_fragments as u16, chunk)?;
            fragments.push(encoded);
        }

        Ok(fragments)
    }

    /// Processes an incoming fragment and reassembles if complete.
    ///
    /// Returns Ok(Some(Message)) if message is complete, Ok(None) if more fragments needed.
    pub fn process_fragment(&self, fragment_data: &[u8]) -> Result<Option<Message>> {
        self.cleanup_fragment_buffers();

        // Decode fragment from binary format
        let fragment = decode_fragment(fragment_data)?;

        if fragment.total_fragments == 1 {
            if fragment.data.len() > DEFAULT_MAX_MESSAGE_SIZE {
                return Err(crate::Error::MessageTooLarge(
                    fragment.data.len(),
                    DEFAULT_MAX_MESSAGE_SIZE,
                ));
            }
            return Ok(Some(self.deserialize_message(&fragment.data)?));
        }

        let mut completed_payload: Option<Vec<u8>> = None;
        let mut assembly_started_at: Option<SystemTime> = None;
        let mut evicted = false;
        // Collected outside the lock to avoid holding fragment_buffers while
        // invoking the callback (which may acquire shared_state).
        let mut eviction_info: Option<FragmentEvictionInfo> = None;

        {
            // Multi-fragment message - add to reassembly buffer
            let mut buffers = self.fragment_buffers.lock().unwrap();
            let now = SystemTime::now();

            if !buffers.contains_key(&fragment.message_id)
                && buffers.len() >= BLE_MAX_FRAGMENT_ASSEMBLIES
            {
                // Priority-based eviction: prefer evicting assemblies with less progress
                // rather than just the oldest. This preserves near-complete assemblies.
                if let Some((evict_id, completion_percent)) = buffers
                    .iter()
                    .min_by(|(_, a), (_, b)| {
                        let priority_a = a.eviction_priority(now);
                        let priority_b = b.eviction_priority(now);
                        priority_a
                            .partial_cmp(&priority_b)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(id, assembly)| {
                        let pct = (assembly.completion_ratio() * 100.0)
                            .round()
                            .clamp(0.0, 100.0) as u8;
                        (id.clone(), pct)
                    })
                {
                    tracing::debug!(
                        message_id = %evict_id,
                        completion_percent,
                        "Evicting fragment assembly to make room (priority-based)"
                    );

                    eviction_info = Some(FragmentEvictionInfo {
                        message_id: evict_id.clone(),
                        completion_percent,
                    });

                    buffers.remove(&evict_id);
                    evicted = true;
                }
            }

            // Get or create assembly buffer
            let assembly = buffers
                .entry(fragment.message_id.clone())
                .or_insert_with(|| FragmentAssembly {
                    total_fragments: fragment.total_fragments,
                    fragments: HashMap::new(),
                    started_at: now,
                    last_seen: now,
                });

            // Validate fragment
            if assembly.total_fragments != fragment.total_fragments {
                return Err(crate::Error::Other(format!(
                    "Fragment count mismatch: expected {}, got {}",
                    assembly.total_fragments, fragment.total_fragments
                )));
            }

            assembly
                .fragments
                .insert(fragment.fragment_index, fragment.data);
            // Idle-timeout anchor: refresh on every fragment so an actively
            // arriving multi-fragment message keeps its buffer alive past the
            // BLE_FRAGMENT_TIMEOUT_SECS window measured from the first fragment.
            // Security note: a peer dribbling one fragment per sub-timeout interval
            // can therefore hold a partial assembly open indefinitely (vs. a hard
            // first-fragment deadline). The exposure is bounded by the
            // BLE_MAX_FRAGMENT_ASSEMBLIES cap + low-progress priority eviction, and
            // is the deliberate tradeoff for not tearing a slow-but-legitimate
            // multi-fragment message mid-flight.
            assembly.last_seen = now;

            // Check if complete
            if assembly.fragments.len() == assembly.total_fragments as usize {
                // Reassemble message
                let mut complete_data = Vec::new();
                for i in 0..assembly.total_fragments {
                    if let Some(data) = assembly.fragments.get(&i) {
                        complete_data.extend_from_slice(data);
                    } else {
                        return Err(crate::Error::Other(format!("Missing fragment {}", i)));
                    }
                }

                assembly_started_at = Some(assembly.started_at);
                buffers.remove(&fragment.message_id);
                completed_payload = Some(complete_data);
            }
        }

        // Fire eviction callback outside the fragment_buffers lock to avoid
        // lock ordering issues (callback may acquire shared_state).
        if let Some(info) = eviction_info {
            if let Some(cb) = self.eviction_callback.lock().unwrap().as_ref() {
                cb(info);
            }
        }

        if evicted {
            self.record_send_failure();
        }

        if let Some(payload) = completed_payload {
            if payload.len() > DEFAULT_MAX_MESSAGE_SIZE {
                return Err(crate::Error::MessageTooLarge(
                    payload.len(),
                    DEFAULT_MAX_MESSAGE_SIZE,
                ));
            }

            let start = assembly_started_at.unwrap_or_else(SystemTime::now);
            let latency = SystemTime::now()
                .duration_since(start)
                .unwrap_or_else(|_| StdDuration::from_millis(0))
                .as_millis();
            self.record_latency(latency);

            // Deserialize complete message
            let message = self.deserialize_message(&payload)?;
            return Ok(Some(message));
        }

        // More fragments needed
        Ok(None)
    }

    /// Called when raw fragment data is received from BLE (platform callback).
    ///
    /// This handles fragmentation reassembly and queues complete messages.
    pub fn on_fragment_received(&self, fragment_data: Vec<u8>) -> Result<()> {
        match self.process_fragment(&fragment_data) {
            Ok(Some(message)) => {
                // Message complete - queue it
                let mut queue = self.receive_queue.lock().unwrap();
                queue.push_back(message.clone());
                // Note: sender/recipient are intentionally not logged to protect user privacy
                tracing::debug!(
                    message_id = %message.id,
                    "Complete message assembled from fragments"
                );
                Ok(())
            }
            Ok(None) => {
                // More fragments needed
                tracing::debug!("Fragment received, more needed for complete message");
                Ok(())
            }
            Err(e) => {
                // Log error but don't fail - just drop bad fragment
                tracing::warn!(error = %e, "Error processing fragment, dropping bad fragment");
                Ok(())
            }
        }
    }

    /// Like [`on_fragment_received`](Self::on_fragment_received), but attaches a
    /// transport-verified `peer_id` to the reassembled message.
    ///
    /// # Parameters
    ///
    /// * `peer_id` — The **user-level identifier** (i.e., the remote peer's
    ///   `UserId` string) that the transport layer has authenticated for this
    ///   connection. This is **not** the raw transport address (MAC, BLE device
    ///   UUID, etc.). The protocol layer uses this value to verify that
    ///   `message.sender` matches the physical peer that delivered it.
    pub fn on_fragment_received_from(&self, fragment_data: Vec<u8>, peer_id: String) -> Result<()> {
        match self.process_fragment(&fragment_data) {
            Ok(Some(mut message)) => {
                message.set_transport_peer_id(peer_id)?;
                let msg_id = message.id.clone();
                let mut queue = self.receive_queue.lock().unwrap();
                queue.push_back(message);
                tracing::debug!(
                    message_id = %msg_id,
                    "Complete message assembled from fragments (with peer identity)"
                );
                Ok(())
            }
            Ok(None) => {
                tracing::debug!("Fragment received, more needed for complete message");
                Ok(())
            }
            Err(e) => {
                tracing::warn!(error = %e, "Error processing fragment, dropping bad fragment");
                Ok(())
            }
        }
    }

    /// Gets the next fragment to send (for platform implementation).
    ///
    /// Returns (recipient, fragment_data) or None if no messages to send.
    pub fn get_next_fragment(&self) -> Result<Option<(String, Vec<u8>)>> {
        // Check for pending fragments first
        if let Some(fragment) = {
            let mut pending = self.pending_fragments.lock().unwrap();
            pending.pop_front()
        } {
            self.update_queue_metric();
            return Ok(Some(fragment));
        }

        // No serialized fragments waiting – pull a fresh message from the queue
        let maybe_message = {
            let mut queue = self.send_queue.lock().unwrap();
            queue.pop_front()
        };

        let Some((recipient, message)) = maybe_message else {
            self.update_queue_metric();
            return Ok(None);
        };

        let fragments = self.fragment_message(&message)?;

        if fragments.is_empty() {
            self.update_queue_metric();
            return Ok(None);
        }

        {
            let mut pending = self.pending_fragments.lock().unwrap();
            for fragment in fragments {
                pending.push_back((recipient.clone(), fragment));
            }
        }

        self.update_queue_metric();

        let result = {
            let mut pending = self.pending_fragments.lock().unwrap();
            pending.pop_front()
        };

        self.update_queue_metric();

        Ok(result)
    }

    /// Re-queues a fragment at the front of the pending queue (used when platform send fails).
    pub fn requeue_fragment(&self, recipient: &str, fragment_data: Vec<u8>) {
        {
            let mut pending = self.pending_fragments.lock().unwrap();
            pending.push_front((recipient.to_string(), fragment_data));
        }

        self.update_queue_metric();
    }
}

fn encode_fragment(
    message_id: &[u8],
    fragment_index: u16,
    total_fragments: u16,
    data: &[u8],
) -> Result<Vec<u8>> {
    if data.len() > u16::MAX as usize {
        return Err(crate::Error::Other(
            "Fragment payload too large".to_string(),
        ));
    }

    let mut encoded = Vec::with_capacity(FRAGMENT_HEADER_FIXED + message_id.len() + data.len());
    encoded.extend_from_slice(&FRAGMENT_MAGIC);
    encoded.push(FRAGMENT_VERSION);
    encoded.push(message_id.len() as u8);
    encoded.extend_from_slice(message_id);
    encoded.extend_from_slice(&fragment_index.to_le_bytes());
    encoded.extend_from_slice(&total_fragments.to_le_bytes());
    encoded.extend_from_slice(&(data.len() as u16).to_le_bytes());
    encoded.extend_from_slice(data);
    Ok(encoded)
}

fn decode_fragment(fragment_data: &[u8]) -> Result<DecodedFragment> {
    if fragment_data.len() < FRAGMENT_HEADER_FIXED {
        return Err(crate::Error::Other("Fragment too short".to_string()));
    }

    if fragment_data[0..2] != FRAGMENT_MAGIC {
        return Err(crate::Error::Other("Invalid fragment magic".to_string()));
    }

    let version = fragment_data[2];
    if version != FRAGMENT_VERSION {
        return Err(crate::Error::Other(format!(
            "Unsupported fragment version {}",
            version
        )));
    }

    let id_len = fragment_data[3] as usize;
    let header_len = FRAGMENT_HEADER_FIXED + id_len;
    if fragment_data.len() < header_len {
        return Err(crate::Error::Other("Fragment truncated (id)".to_string()));
    }

    let mut offset = 4;
    let message_id_bytes = &fragment_data[offset..offset + id_len];
    offset += id_len;

    let message_id = String::from_utf8(message_id_bytes.to_vec())
        .map_err(|_| crate::Error::Other("Invalid UTF-8 in message ID".to_string()))?;

    if fragment_data.len() < offset + 6 {
        return Err(crate::Error::Other(
            "Fragment truncated (header)".to_string(),
        ));
    }

    let fragment_index = u16::from_le_bytes(
        fragment_data[offset..offset + 2]
            .try_into()
            .map_err(|_| crate::Error::Other("Fragment truncated (index)".to_string()))?,
    );
    offset += 2;
    let total_fragments = u16::from_le_bytes(
        fragment_data[offset..offset + 2]
            .try_into()
            .map_err(|_| crate::Error::Other("Fragment truncated (total)".to_string()))?,
    );
    offset += 2;
    let data_len = u16::from_le_bytes(
        fragment_data[offset..offset + 2]
            .try_into()
            .map_err(|_| crate::Error::Other("Fragment truncated (length)".to_string()))?,
    ) as usize;
    offset += 2;

    if fragment_data.len() < offset + data_len {
        return Err(crate::Error::Other("Fragment truncated (data)".to_string()));
    }

    let data = fragment_data[offset..offset + data_len].to_vec();

    Ok(DecodedFragment {
        message_id,
        fragment_index,
        total_fragments,
        data,
    })
}

impl Transport for BleTransport {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn transport_type(&self) -> TransportType {
        TransportType::BLE
    }

    fn status(&self) -> TransportStatus {
        *self.status.lock().unwrap()
    }

    fn metrics(&self) -> TransportMetrics {
        self.metrics.lock().unwrap().clone()
    }

    fn send(&self, message: &Message) -> Result<()> {
        let status = self.status();

        if status != TransportStatus::Available {
            return Err(crate::Error::TransportNotAvailable(format!(
                "BLE transport is not available (status: {:?})",
                status
            )));
        }

        let recipient = message.recipient.as_str().to_string();

        {
            let peers = self.peers.lock().unwrap();
            if !peers.contains_key(&recipient) {
                // Distinguish "no BLE peers at all" (ordinary offline; escalate to
                // the next transport) from "meshing with other peers but not this
                // recipient" — the latter is the keying-mismatch signal that
                // silently routes an in-range peer's message to Internet. Count and
                // warn only the latter so the metric isn't drowned out by the
                // benign no-peers case. See `recipient_not_among_peers_count`.
                if !peers.is_empty() {
                    self.recipient_not_among_peers_count
                        .fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        recipient = %recipient,
                        connected_peers = peers.len(),
                        "ble: send recipient is not among the connected BLE peers while \
                         other peers are connected — likely a recipient↔device-id keying \
                         mismatch; this send will fall back off BLE"
                    );
                }
                return Err(crate::Error::PeerNotReachable(format!(
                    "BLE: no connected peer for recipient {}",
                    recipient
                )));
            }
        }

        {
            let mut queue = self.send_queue.lock().unwrap();
            queue.push_back((recipient, message.clone()));
        }

        self.update_queue_metric();
        self.notify_fragments_available();

        Ok(())
    }

    fn receive(&self) -> Result<Option<Message>> {
        let mut queue = self.receive_queue.lock().unwrap();
        Ok(queue.pop_front())
    }

    fn start(&mut self) -> Result<()> {
        // Set status to Available when starting
        // Platform can still override this via on_status_changed() if BLE is not available
        *self.status.lock().unwrap() = TransportStatus::Available;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        // **Caller contract:** the platform callback pump must already
        // be quiesced before `stop()` returns. Every `set_peer_mtu` /
        // `on_peer_discovered` / `on_fragment_received` entry takes
        // `&self`, so a late callback landing after the drains below
        // will re-seed state into what is supposed to be an at-rest
        // transport. Android/iOS bindings enforce this by draining
        // their own GATT state before calling into the Rust stop path.
        *self.status.lock().unwrap() = TransportStatus::Disconnected;
        // Drain every per-session cache on teardown so a subsequent
        // start() observes no state from the prior session. Every
        // reconnect re-runs peer discovery, fragment reassembly, and
        // MTU negotiation from scratch. Keep the drains in lockstep —
        // a one-sided drain (e.g. clearing `peer_mtus` but leaving
        // `peers` intact) would let `send()` pass its peer guard while
        // the fragmenter silently falls back to the 185-byte floor, or
        // would let stale reassembly state outlive the peer it came
        // from. The `undersized_mtu_reports` counter is a monotonic
        // lifetime metric and is intentionally *not* reset here.
        self.peers.lock().unwrap().clear();
        self.peer_mtus.lock().unwrap().clear();
        self.fragment_buffers.lock().unwrap().clear();
        self.send_queue.lock().unwrap().clear();
        self.pending_fragments.lock().unwrap().clear();
        self.receive_queue.lock().unwrap().clear();
        self.update_queue_metric();
        Ok(())
    }
}

/// BLE transport builder for configuration.
pub struct BleTransportBuilder {
    device_id: String,
}

impl BleTransportBuilder {
    /// Creates a new builder.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
        }
    }

    /// Builds the BLE transport.
    pub fn build(self) -> BleTransport {
        BleTransport::new(self.device_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, Message, MessagePriority, UserId, TTL};

    fn peer_device(id: &str) -> PeerDevice {
        PeerDevice {
            device_id: id.to_string(),
            address: "AA:BB:CC:DD:EE:FF".to_string(),
            rssi: -60,
            last_seen: std::time::SystemTime::now(),
            connected: false,
        }
    }

    fn small_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("app").unwrap(),
            "hi",
        )
    }

    #[test]
    fn test_ble_transport_creation() {
        let transport = BleTransport::new("test-device");
        assert_eq!(transport.device_id(), "test-device");
        assert_eq!(transport.status(), TransportStatus::Unavailable);
    }

    #[test]
    fn test_ble_send_when_unavailable_fails() {
        let transport = BleTransport::new("test-device");
        let msg = small_message();
        let result = transport.send(&msg);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::Error::TransportNotAvailable(_)
        ));
    }

    #[test]
    fn test_ble_start_stop() {
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();
        assert_eq!(transport.status(), TransportStatus::Available);
        transport.stop().unwrap();
        assert_eq!(transport.status(), TransportStatus::Disconnected);
    }

    #[test]
    fn test_ble_recipient_not_among_peers_count_signals_keying_mismatch() {
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();

        // No BLE peers at all: PeerNotReachable, but this is the ordinary offline
        // case (escalating to the next transport is correct) and must NOT be counted.
        assert!(matches!(
            transport.send(&small_message()).unwrap_err(),
            crate::Error::PeerNotReachable(_)
        ));
        assert_eq!(transport.recipient_not_among_peers_count(), 0);

        // Meshing with another peer ("alice") but not the addressed recipient
        // ("bob"): the keying-mismatch signal — a physically-present mesh whose
        // ids don't match the send target. This MUST be counted.
        transport.on_peer_discovered(peer_device("alice"));
        assert!(matches!(
            transport.send(&small_message()).unwrap_err(),
            crate::Error::PeerNotReachable(_)
        ));
        assert_eq!(transport.recipient_not_among_peers_count(), 1);

        // Recipient now connected: send succeeds and the counter does not move.
        transport.on_peer_discovered(peer_device("bob"));
        transport.send(&small_message()).unwrap();
        assert_eq!(transport.recipient_not_among_peers_count(), 1);
    }

    #[test]
    fn test_ble_on_status_changed() {
        let transport = BleTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);
        assert_eq!(transport.status(), TransportStatus::Available);
        transport.on_status_changed(TransportStatus::Error);
        assert_eq!(transport.status(), TransportStatus::Error);
    }

    #[test]
    fn test_ble_on_status_changed_drains_session_state() {
        let transport = BleTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);

        transport.on_peer_discovered(peer_device("bob"));
        transport.set_peer_mtu("bob", 244);
        transport.send(&small_message()).unwrap();

        // Seed a fragment buffer via on_fragment_received with a partial fragment.
        // Simpler: insert directly.
        transport.fragment_buffers.lock().unwrap().insert(
            "msg-1".to_string(),
            FragmentAssembly {
                total_fragments: 2,
                fragments: HashMap::new(),
                started_at: SystemTime::now(),
                last_seen: SystemTime::now(),
            },
        );

        // Bump a lifetime counter so we can verify it survives.
        transport
            .undersized_mtu_reports
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Transition away from Available.
        transport.on_status_changed(TransportStatus::Unavailable);

        assert!(transport.peers.lock().unwrap().is_empty());
        assert!(transport.peer_mtus.lock().unwrap().is_empty());
        assert!(transport.fragment_buffers.lock().unwrap().is_empty());
        assert!(transport.send_queue.lock().unwrap().is_empty());
        assert!(transport.pending_fragments.lock().unwrap().is_empty());
        assert!(transport.receive_queue.lock().unwrap().is_empty());
        assert_eq!(transport.metrics.lock().unwrap().queue_depth, 0);

        // Lifetime counters must survive.
        assert_eq!(
            transport
                .undersized_mtu_reports
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn test_ble_on_status_changed_available_to_available_preserves_state() {
        let transport = BleTransport::new("test-device");
        transport.on_status_changed(TransportStatus::Available);

        transport.on_peer_discovered(peer_device("bob"));
        transport.set_peer_mtu("bob", 244);
        transport.send(&small_message()).unwrap();

        // Transition Available → Available must not clear anything.
        transport.on_status_changed(TransportStatus::Available);

        assert_eq!(transport.peers.lock().unwrap().len(), 1);
        assert_eq!(transport.peer_mtus.lock().unwrap().len(), 1);
        assert!(!transport.send_queue.lock().unwrap().is_empty());
    }

    #[test]
    fn test_ble_on_status_changed_non_available_to_non_available_no_drain() {
        let transport = BleTransport::new("test-device");

        // Start at Unavailable (default), add a peer manually.
        transport
            .peers
            .lock()
            .unwrap()
            .insert("alice".to_string(), peer_device("alice"));

        // Transition Unavailable → Error — should NOT drain because
        // previous was not Available.
        transport.on_status_changed(TransportStatus::Error);
        assert_eq!(transport.peers.lock().unwrap().len(), 1);
    }

    #[test]
    fn test_ble_peer_mtu_defaults_to_fallback() {
        let transport = BleTransport::new("test-device");
        assert_eq!(transport.peer_mtu("unknown-peer"), BLE_MAX_FRAGMENT_SIZE);
    }

    #[test]
    fn test_ble_set_peer_mtu_records_and_clamps() {
        let transport = BleTransport::new("test-device");

        // Reasonable value stored verbatim.
        transport.set_peer_mtu("alice", 244);
        assert_eq!(transport.peer_mtu("alice"), 244);

        // Undersized value is rejected outright — no entry is stored, so
        // peer_mtu falls back to BLE_MAX_FRAGMENT_SIZE rather than
        // recording a clamped-up 185 that the controller cannot honor.
        transport.set_peer_mtu("bob", 20);
        assert_eq!(transport.peer_mtu("bob"), BLE_MAX_FRAGMENT_SIZE);
        assert!(
            !transport.peer_mtus.lock().unwrap().contains_key("bob"),
            "undersized MTU report must not be stored"
        );

        // Oversized value clamped down to MAX_REASONABLE_BLE_PAYLOAD.
        transport.set_peer_mtu("carol", 9999);
        assert_eq!(transport.peer_mtu("carol"), MAX_REASONABLE_BLE_PAYLOAD);

        // Exact floor is accepted and stored.
        transport.set_peer_mtu("dave", BLE_MAX_FRAGMENT_SIZE);
        assert_eq!(transport.peer_mtu("dave"), BLE_MAX_FRAGMENT_SIZE);
        assert!(transport.peer_mtus.lock().unwrap().contains_key("dave"));

        // Per-peer isolation.
        assert_eq!(transport.peer_mtu("alice"), 244);
    }

    #[test]
    fn test_ble_undersized_renegotiation_drops_stored_entry() {
        // Regression: the reject branch must remove any prior entry, not
        // just return early. Otherwise a stored-400 → renegotiate-to-20
        // transition keeps the stale 400 and the fragmenter writes chunks
        // the new link cannot honor.
        let transport = BleTransport::new("test-device");
        transport.set_peer_mtu("alice", 400);
        assert_eq!(transport.peer_mtu("alice"), 400);

        transport.set_peer_mtu("alice", 20);
        assert_eq!(transport.peer_mtu("alice"), BLE_MAX_FRAGMENT_SIZE);
        assert!(
            !transport.peer_mtus.lock().unwrap().contains_key("alice"),
            "undersized renegotiation must drop the stale entry"
        );
    }

    #[test]
    fn test_ble_stop_drains_peer_mtus() {
        // stop() must clear per-peer MTUs so a subsequent start() cannot
        // observe stale values from the prior session.
        let mut transport = BleTransport::new("test-device");
        transport.set_peer_mtu("alice", 400);
        transport.set_peer_mtu("bob", 300);
        assert_eq!(transport.peer_mtu("alice"), 400);
        assert_eq!(transport.peer_mtu("bob"), 300);

        transport.stop().unwrap();

        assert!(transport.peer_mtus.lock().unwrap().is_empty());
        assert_eq!(transport.peer_mtu("alice"), BLE_MAX_FRAGMENT_SIZE);
        assert_eq!(transport.peer_mtu("bob"), BLE_MAX_FRAGMENT_SIZE);
    }

    #[test]
    fn test_ble_stop_drains_all_per_session_state() {
        // stop() must drain every per-session cache in lockstep. A
        // one-sided drain would let `send()` pass its peer guard while
        // the fragmenter silently falls back to the 185-byte floor, or
        // would let stale reassembly state outlive the peer it came
        // from.
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();
        transport.on_peer_discovered(peer_device("bob"));
        transport.set_peer_mtu("bob", 400);
        transport.send(&small_message()).unwrap();
        // Seed a partial reassembly buffer by feeding one of two
        // fragments from a larger message.
        let big = Message::builder(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("app").unwrap(),
        )
        .content("z".repeat(600))
        .build();
        let frags = transport.fragment_message(&big).unwrap();
        assert!(frags.len() > 1);
        transport.on_fragment_received(frags[0].clone()).unwrap();
        // Queue a received message so the receive_queue is non-empty.
        transport.on_message_received(small_message());

        assert!(!transport.peers.lock().unwrap().is_empty());
        assert!(!transport.peer_mtus.lock().unwrap().is_empty());
        assert!(!transport.fragment_buffers.lock().unwrap().is_empty());
        assert!(
            !transport.send_queue.lock().unwrap().is_empty()
                || !transport.pending_fragments.lock().unwrap().is_empty()
        );
        assert!(!transport.receive_queue.lock().unwrap().is_empty());

        transport.stop().unwrap();

        assert!(transport.peers.lock().unwrap().is_empty());
        assert!(transport.peer_mtus.lock().unwrap().is_empty());
        assert!(transport.fragment_buffers.lock().unwrap().is_empty());
        assert!(transport.send_queue.lock().unwrap().is_empty());
        assert!(transport.pending_fragments.lock().unwrap().is_empty());
        assert!(transport.receive_queue.lock().unwrap().is_empty());
    }

    #[test]
    fn test_ble_on_peer_lost_clears_stored_mtu() {
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("alice"));
        transport.set_peer_mtu("alice", 400);
        assert_eq!(transport.peer_mtu("alice"), 400);

        transport.on_peer_lost("alice");
        // Peer map and MTU map must drop in lockstep so a reconnect cannot
        // observe a stale value from the previous link.
        assert!(transport.get_peer("alice").is_none());
        assert_eq!(transport.peer_mtu("alice"), BLE_MAX_FRAGMENT_SIZE);
    }

    #[test]
    fn test_ble_clear_peer_mtu_reverts_to_fallback() {
        let transport = BleTransport::new("test-device");
        transport.set_peer_mtu("alice", 400);
        assert_eq!(transport.peer_mtu("alice"), 400);
        transport.clear_peer_mtu("alice");
        assert_eq!(transport.peer_mtu("alice"), BLE_MAX_FRAGMENT_SIZE);
    }

    #[test]
    fn test_ble_fragment_size_differs_per_peer() {
        let transport = BleTransport::new("test-device");
        let sender = UserId::new("alice").unwrap();
        let app_id = AppId::new("app").unwrap();
        let content = "y".repeat(1024);

        // Two messages with identical content but different recipients so
        // the fragmenter keys into different per-peer MTU slots.
        let msg_small = Message::builder(
            sender.clone(),
            UserId::new("small-peer").unwrap(),
            app_id.clone(),
        )
        .content(content.clone())
        .build();
        let msg_big = Message::builder(sender, UserId::new("big-peer").unwrap(), app_id)
            .content(content)
            .build();

        // `fragment_message` warns if the recipient is not a registered
        // direct peer — reflect the real send() precondition in the test
        // setup so the warn path does not trigger spurious log noise.
        transport.on_peer_discovered(peer_device("small-peer"));
        transport.on_peer_discovered(peer_device("big-peer"));
        transport.set_peer_mtu("small-peer", BLE_MAX_FRAGMENT_SIZE);
        transport.set_peer_mtu("big-peer", 500);

        let small_frags = transport.fragment_message(&msg_small).unwrap();
        let big_frags = transport.fragment_message(&msg_big).unwrap();

        // Larger MTU ⇒ fewer fragments, each individual fragment larger.
        assert!(big_frags.len() < small_frags.len());
        let max_small = small_frags.iter().map(|f| f.len()).max().unwrap();
        let max_big = big_frags.iter().map(|f| f.len()).max().unwrap();
        assert!(max_big > max_small);
        assert!(max_small <= BLE_MAX_FRAGMENT_SIZE);
        assert!(max_big <= 500);
    }

    #[test]
    fn test_ble_fragment_unknown_peer_uses_fallback() {
        let transport = BleTransport::new("test-device");
        // "bob" is a registered direct peer, but no MTU has been reported
        // for it yet — the fragmenter must fall back to
        // BLE_MAX_FRAGMENT_SIZE.
        transport.on_peer_discovered(peer_device("bob"));
        let msg = small_message();
        let fragments = transport.fragment_message(&msg).unwrap();
        for fragment in &fragments {
            assert!(fragment.len() <= BLE_MAX_FRAGMENT_SIZE);
        }
    }

    #[test]
    fn test_ble_upward_renegotiation_overwrites_stored_mtu() {
        // Mid-link upward renegotiation: a peer starts at 400, the
        // controller later renegotiates up to 500. The stored entry must
        // track the latest value so subsequent fragments size against the
        // real capacity rather than remaining pinned at the first
        // negotiated value.
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("alice"));

        transport.set_peer_mtu("alice", 400);
        assert_eq!(transport.peer_mtu("alice"), 400);

        transport.set_peer_mtu("alice", 500);
        assert_eq!(transport.peer_mtu("alice"), 500);

        // And the fragmenter actually picks up the new size — emit the
        // same payload before and after and observe that the post-
        // renegotiation batch has larger individual fragments.
        let msg = Message::builder(
            UserId::new("sender").unwrap(),
            UserId::new("alice").unwrap(),
            AppId::new("app").unwrap(),
        )
        .content("x".repeat(1024))
        .build();

        transport.set_peer_mtu("alice", 300);
        let small_frags = transport.fragment_message(&msg).unwrap();
        transport.set_peer_mtu("alice", 500);
        let big_frags = transport.fragment_message(&msg).unwrap();

        let max_small = small_frags.iter().map(|f| f.len()).max().unwrap();
        let max_big = big_frags.iter().map(|f| f.len()).max().unwrap();
        assert!(
            max_big > max_small,
            "upward reneg must produce larger fragments: {} vs {}",
            max_big,
            max_small
        );
        assert!(big_frags.len() < small_frags.len());
    }

    #[test]
    fn test_ble_stop_start_round_trip_is_fresh() {
        // stop() must leave the transport in a state where a subsequent
        // start() observes no carry-over from the prior session: no
        // stored MTUs, no peers, no queued fragments. Round-trip through
        // a full start → seed → stop → start → verify cycle.
        // `small_message()` has recipient "bob", so the seeded peer must
        // also be "bob" for the send() precondition to pass. Keying the
        // test to match the real send-path invariant (recipient ==
        // device_id) catches any regression where stop/start leaks a
        // ghost peer under a different key.
        let mut transport = BleTransport::new("test-device");

        transport.start().unwrap();
        transport.on_peer_discovered(peer_device("bob"));
        transport.set_peer_mtu("bob", 400);
        transport.send(&small_message()).unwrap();
        assert_eq!(transport.peer_mtu("bob"), 400);

        transport.stop().unwrap();
        transport.start().unwrap();

        // Peer map, MTU map, and send queue must all be empty — a fresh
        // start observes no residue from the prior session.
        assert!(transport.get_peer("bob").is_none());
        assert_eq!(transport.peer_mtu("bob"), BLE_MAX_FRAGMENT_SIZE);

        // After re-start, `send()` must fail because there are no peers
        // — which is exactly the "at-rest" precondition start() must
        // satisfy. If any per-session state had leaked, `send()` would
        // succeed against a ghost peer.
        let result = transport.send(&small_message());
        assert!(
            matches!(result, Err(crate::Error::PeerNotReachable(_))),
            "send after stop/start must report no reachable peer, got {:?}",
            result
        );

        // Re-seeding the same peer with a different MTU works as
        // expected — no stale entry shadowing the new value.
        transport.on_peer_discovered(peer_device("bob"));
        transport.set_peer_mtu("bob", 250);
        assert_eq!(transport.peer_mtu("bob"), 250);
    }

    #[test]
    fn test_ble_undersized_mtu_reports_counter() {
        // The counter must:
        //   - start at zero
        //   - increment exactly once per undersized report
        //   - NOT increment on accepted values (including exact floor)
        //   - NOT increment on clamped-down oversized values
        //   - survive the clear_peer_mtu path (monotonic lifetime counter,
        //     never reset)
        let transport = BleTransport::new("test-device");
        assert_eq!(transport.undersized_mtu_reports(), 0);

        transport.set_peer_mtu("alice", 20);
        assert_eq!(transport.undersized_mtu_reports(), 1);

        // Accepted value — no increment.
        transport.set_peer_mtu("bob", 400);
        assert_eq!(transport.undersized_mtu_reports(), 1);

        // Exact floor — accepted, no increment.
        transport.set_peer_mtu("carol", BLE_MAX_FRAGMENT_SIZE);
        assert_eq!(transport.undersized_mtu_reports(), 1);

        // Oversized — clamped down, no increment.
        transport.set_peer_mtu("dave", 9999);
        assert_eq!(transport.undersized_mtu_reports(), 1);

        // Another undersized report — increment to 2.
        transport.set_peer_mtu("eve", 10);
        assert_eq!(transport.undersized_mtu_reports(), 2);

        // Monotonic across clear_peer_mtu — this is a lifetime counter,
        // not a per-peer state.
        transport.clear_peer_mtu("bob");
        assert_eq!(transport.undersized_mtu_reports(), 2);

        // Undersized renegotiation of an existing peer still counts.
        transport.set_peer_mtu("alice", 15);
        assert_eq!(transport.undersized_mtu_reports(), 3);
    }

    #[test]
    fn test_ble_fragment_fallback_count_increments_on_registered_peer_without_mtu() {
        // The keying-contract telemetry counter must:
        //   - start at zero
        //   - increment exactly once per fragment_message call that
        //     falls back to BLE_MAX_FRAGMENT_SIZE because the recipient
        //     is a registered direct peer with no stored per-peer MTU
        //     (the contract-break case)
        //   - NOT increment once an MTU has been recorded for that peer
        //   - NOT increment when the recipient is not a registered
        //     direct peer (the benign send/on_peer_lost race — see
        //     `test_ble_fragment_fallback_count_ignores_teardown_race`)
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("bob"));
        assert_eq!(transport.fragment_fallback_count(), 0);

        // No MTU stored for "bob" but it IS a registered direct peer —
        // this is the contract-break shape and must increment.
        let msg = small_message();
        transport.fragment_message(&msg).unwrap();
        assert_eq!(transport.fragment_fallback_count(), 1);

        // Another send in the same pre-flush window — increment again.
        transport.fragment_message(&msg).unwrap();
        assert_eq!(transport.fragment_fallback_count(), 2);

        // Record an MTU; subsequent sends must NOT increment.
        transport.set_peer_mtu("bob", 400);
        transport.fragment_message(&msg).unwrap();
        transport.fragment_message(&msg).unwrap();
        assert_eq!(transport.fragment_fallback_count(), 2);

        // Clearing the MTU re-opens the fallback window for a still-
        // registered peer — increment again.
        transport.clear_peer_mtu("bob");
        transport.fragment_message(&msg).unwrap();
        assert_eq!(transport.fragment_fallback_count(), 3);
    }

    #[test]
    fn test_ble_fragment_fallback_count_ignores_teardown_race() {
        // Benign send/on_peer_lost race: `send()` validates and
        // enqueues while the peer is live, then `on_peer_lost` drops
        // both `peers` and `peer_mtus` in lockstep, then
        // `get_next_fragment` pops the queued message and calls
        // `fragment_message`. The miss branch must NOT increment
        // `fragment_fallback_count` in this shape, because the
        // invariant (MTU-before-discover) was not violated — the
        // peer simply evaporated mid-send. Counting this case would
        // drown the real signal out on every disconnect with
        // in-flight sends.
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("bob"));
        transport.set_peer_mtu("bob", 400);
        assert_eq!(transport.fragment_fallback_count(), 0);

        // Simulate the peer disappearing between `send()` enqueue and
        // `get_next_fragment` fragmentation. `fragment_message` is
        // called directly (mirroring the code path inside
        // `get_next_fragment`) for a recipient that is neither in
        // `peers` nor in `peer_mtus`.
        transport.on_peer_lost("bob");
        let msg = small_message();
        transport.fragment_message(&msg).unwrap();

        assert_eq!(
            transport.fragment_fallback_count(),
            0,
            "teardown race must not tick the contract-break counter"
        );
    }

    #[test]
    fn test_ble_golden_path_handshake_never_falls_back() {
        // Pins the MTU-before-discover ordering invariant from the
        // Rust side. Both platform layers MUST call
        // `set_peer_mtu` before `on_peer_discovered` so that by the
        // time any fragmenting send can reach a live peer the MTU
        // is already on file — see `BleManager.swift`
        // (`didUpdateValueFor` DEVICE_ID branch: `bleSetPeerMtu`
        // precedes `blePeerDiscovered`) and
        // `BleTransportFacade.kt` (`handleDeviceIdRead` flushes
        // staged MTU via `onDeviceIdResolved` before
        // `blePeerDiscovered`). A regression that swaps the order
        // on either platform would let the first send fall back,
        // and with `peers.contains_key` now gating the counter,
        // that exact shape (registered direct peer + no MTU on
        // file) is what this test pins.
        //
        // If this test starts failing, either a platform layer
        // reordered its handshake OR a refactor introduced a
        // recipient -> device_id translation that breaks the
        // keying contract. Do NOT "fix" by registering an MTU
        // inside this test — go find the regression.
        let transport = BleTransport::new("test-device");

        // Golden ordering: MTU first, then peer-discovered — same
        // as the platforms.
        transport.set_peer_mtu("bob", 400);
        transport.on_peer_discovered(peer_device("bob"));

        let msg = small_message();
        let fragments = transport.fragment_message(&msg).unwrap();
        assert!(!fragments.is_empty());

        assert_eq!(
            transport.fragment_fallback_count(),
            0,
            "golden-path handshake must not fall back — the \
             platform-side MTU-before-discover invariant regressed, \
             or the recipient -> device_id keying contract broke"
        );
    }

    #[test]
    fn test_ble_peripheral_role_disconnect_must_not_clear_central_mtu() {
        // Regression pin for the bug fixed in commit c0ff33e (see
        // BleTransportFacade.handleCentralDisconnectedOnMain).
        //
        // The Rust transport has no concept of central vs. peripheral
        // role — there is one peer, one MTU. The platform layers own
        // which role "populated" a given entry. Before the fix, the
        // Android facade's peripheral-role disconnect handler was
        // calling `bleClearPeerMtu` under the mistaken belief that the
        // peripheral-role disconnect owned MTU teardown. It did not:
        // `peer_mtus[deviceId]` is populated by the central-role link
        // via `CentralGattClient.onMtuChanged → flushPeerMtu`, and
        // clearing it on a peripheral-role disconnect demoted an alive
        // central-role link from its negotiated BLE 5 MTU back to the
        // 185-byte floor for the rest of the link's life.
        //
        // This test pins the Rust-side contract that the platform fix
        // depends on: the *only* valid signals that affect
        // `peer_mtus[X]` are `set_peer_mtu(X, ...)`,
        // `clear_peer_mtu(X)`, `on_peer_lost(X)`, and the full-state
        // drains on `stop()`. A peripheral-role disconnect that
        // (correctly) calls none of those must leave fragment sizing
        // untouched. If a future refactor causes `fragment_message`
        // to re-read state in a way that is not idempotent across
        // "no-op" calls, this test catches it.
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("bob"));
        transport.set_peer_mtu("bob", 400);

        let msg = Message::builder(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("app").unwrap(),
        )
        .content("x".repeat(1024))
        .build();

        let pre_frags = transport.fragment_message(&msg).unwrap();
        let pre_max = pre_frags.iter().map(|f| f.len()).max().unwrap();
        assert!(
            pre_max > BLE_MAX_FRAGMENT_SIZE,
            "pre-disconnect fragments must size against the negotiated 400-byte MTU"
        );

        // Simulated peripheral-role disconnect: the fix requires the
        // platform layer to call *none* of `on_peer_lost`,
        // `clear_peer_mtu`, `set_peer_mtu`. Anything else here would
        // either clobber the central-role MTU state or declare the
        // peer universally lost.

        let post_frags = transport.fragment_message(&msg).unwrap();
        let post_max = post_frags.iter().map(|f| f.len()).max().unwrap();
        assert_eq!(
            pre_max, post_max,
            "peripheral-role disconnect must not change fragment sizing for an alive central-role link"
        );
        assert_eq!(pre_frags.len(), post_frags.len());
        assert_eq!(
            transport.fragment_fallback_count(),
            0,
            "a no-op peripheral-role disconnect must not tick the keying-contract counter"
        );
        assert_eq!(
            transport.peer_mtu("bob"),
            400,
            "peer MTU must survive a peripheral-role disconnect at the Rust layer"
        );
    }

    #[test]
    fn test_ble_platform_handle() {
        let transport = BleTransport::new("test-device");
        assert_eq!(transport.platform_handle(), None);
        transport.set_platform_handle(42);
        assert_eq!(transport.platform_handle(), Some(42));
    }

    #[test]
    fn test_ble_update_metrics() {
        let transport = BleTransport::new("test-device");
        let mut m = TransportMetrics::default();
        m.rssi = Some(-70);
        transport.update_metrics(m);
        assert_eq!(transport.metrics().rssi, Some(-70));
    }

    #[test]
    fn test_ble_record_send_success_failure() {
        let transport = BleTransport::new("test-device");
        transport.record_send_success();
        transport.record_send_success();
        transport.record_send_failure();
        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 2);
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_ble_peer_discovery() {
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("peer-1"));
        let peers = transport.get_peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_id, "peer-1");
        assert!(transport.get_peer("peer-1").is_some());
        assert!(transport.get_peer("other").is_none());
    }

    #[test]
    fn test_ble_peer_lost() {
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("peer-1"));
        transport.on_peer_lost("peer-1");
        assert_eq!(transport.get_peers().len(), 0);
    }

    #[test]
    fn test_ble_has_pending_sends_dequeue_send_get_queue_depth() {
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();
        transport.on_peer_discovered(peer_device("bob"));
        let msg = small_message();
        transport.send(&msg).unwrap();
        assert!(transport.has_pending_sends());
        assert_eq!(transport.get_queue_depth(), 1);
        let dequeued = transport.dequeue_send();
        assert!(dequeued.is_some());
        assert!(!transport.has_pending_sends());
        assert_eq!(transport.get_queue_depth(), 0);
        assert!(transport.dequeue_send().is_none());
    }

    #[test]
    fn test_ble_serialize_deserialize_message() {
        let transport = BleTransport::new("test-device");
        let msg = small_message();
        let data = transport.serialize_message(&msg).unwrap();
        let back = transport.deserialize_message(&data).unwrap();
        assert_eq!(back.id, msg.id);
        assert_eq!(back.content, msg.content);
    }

    #[test]
    fn test_ble_deserialize_invalid_json() {
        let transport = BleTransport::new("test-device");
        let result = transport.deserialize_message(b"not json");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::Error::SerializationError(_)
        ));
    }

    #[test]
    fn test_ble_single_fragment_roundtrip() {
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("bob"));
        transport.set_peer_mtu("bob", 512);
        let msg = small_message();
        let fragments = transport.fragment_message(&msg).unwrap();
        assert_eq!(
            fragments.len(),
            1,
            "small message with large MTU should fit in one fragment"
        );
        let reconstructed = transport.process_fragment(&fragments[0]).unwrap();
        assert!(reconstructed.is_some());
        assert_eq!(reconstructed.unwrap().content, msg.content);
    }

    #[test]
    fn test_ble_fragment_roundtrip() {
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("bob"));
        let sender = UserId::new("alice").unwrap();
        let recipient = UserId::new("bob").unwrap();
        let app_id = AppId::new("app").unwrap();
        let content = "x".repeat(512);

        let message = Message::builder(sender, recipient, app_id)
            .content(content.clone())
            .priority(MessagePriority::High)
            .ttl(TTL::new(8).unwrap())
            .build();

        let fragments = transport.fragment_message(&message).unwrap();
        assert!(fragments.len() > 1);
        for fragment in &fragments {
            assert!(fragment.len() <= BLE_MAX_FRAGMENT_SIZE);
        }

        let mut reconstructed = None;
        for fragment in fragments {
            if let Some(msg) = transport.process_fragment(&fragment).unwrap() {
                reconstructed = Some(msg);
            }
        }

        let reconstructed = reconstructed.expect("Expected complete message");
        assert_eq!(reconstructed.content, content);
    }

    #[test]
    fn test_ble_process_fragment_invalid_magic() {
        let transport = BleTransport::new("test-device");
        let mut bad = vec![0x00, 0x00, 1, 0, 0, 0, 0, 0, 0, 0]; // wrong magic
        bad.extend_from_slice(b"{}");
        let result = transport.process_fragment(&bad);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::Error::Other(s) if s.contains("magic")));
    }

    #[test]
    fn test_ble_process_fragment_too_short() {
        let transport = BleTransport::new("test-device");
        let result = transport.process_fragment(&[0x4f, 0x50]); // "OP" only
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), crate::Error::Other(s) if s.contains("short") || s.contains("truncat"))
        );
    }

    #[test]
    fn test_ble_process_fragment_wrong_version() {
        let transport = BleTransport::new("test-device");
        // Minimal header with wrong version: magic(2) + version(1)=99 + id_len(1)=0 + index(2) + total(2) + data_len(2)
        let bad = [b'O', b'P', 99u8, 0u8, 0u8, 0u8, 1u8, 0u8, 0u8, 0u8];
        let result = transport.process_fragment(&bad);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::Error::Other(s) if s.contains("version")));
    }

    #[test]
    fn test_ble_on_fragment_received_complete_queues_message() {
        let transport = BleTransport::new("test-device");
        transport.on_peer_discovered(peer_device("bob"));
        transport.set_peer_mtu("bob", 512);
        let msg = small_message();
        let fragments = transport.fragment_message(&msg).unwrap();
        for fragment in &fragments {
            transport.on_fragment_received(fragment.clone()).unwrap();
        }
        let received = transport.receive().unwrap();
        assert!(received.is_some());
        assert_eq!(received.unwrap().content, msg.content);
    }

    #[test]
    fn test_ble_on_fragment_received_bad_data_drops_ok() {
        let transport = BleTransport::new("test-device");
        let result = transport.on_fragment_received(vec![0u8; 5]);
        assert!(result.is_ok()); // drops bad fragment, doesn't propagate error
    }

    #[test]
    fn test_ble_get_next_fragment_requeue() {
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();
        transport.on_peer_discovered(peer_device("bob"));
        let msg = small_message();
        transport.send(&msg).unwrap();
        let first = transport.get_next_fragment().unwrap();
        assert!(first.is_some());
        let (recipient, data) = first.unwrap();
        transport.requeue_fragment(&recipient, data.clone());
        let again = transport.get_next_fragment().unwrap();
        assert!(again.is_some());
        assert_eq!(again.unwrap().1, data);
    }

    #[test]
    fn test_ble_get_next_fragment_none_when_empty() {
        let transport = BleTransport::new("test-device");
        assert!(transport.get_next_fragment().unwrap().is_none());
    }

    #[test]
    fn test_ble_on_message_received() {
        let transport = BleTransport::new("test-device");
        let msg = small_message();
        transport.on_message_received(msg.clone());
        let received = transport.receive().unwrap();
        assert!(received.is_some());
        assert_eq!(received.unwrap().id, msg.id);
    }

    #[test]
    fn test_ble_send_rejects_unknown_peer() {
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();
        let msg = small_message();
        let result = transport.send(&msg);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::Error::PeerNotReachable(_)
        ));
    }

    #[test]
    fn test_ble_send_allows_known_peer() {
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();
        transport.on_peer_discovered(peer_device("bob"));
        let msg = small_message();
        assert!(transport.send(&msg).is_ok());
    }

    #[test]
    fn test_ble_send_after_peer_lost() {
        let mut transport = BleTransport::new("test-device");
        transport.start().unwrap();
        transport.on_peer_discovered(peer_device("bob"));
        assert!(transport.send(&small_message()).is_ok());
        transport.on_peer_lost("bob");
        let result = transport.send(&small_message());
        assert!(matches!(
            result.unwrap_err(),
            crate::Error::PeerNotReachable(_)
        ));
    }

    #[test]
    fn test_ble_transport_builder() {
        let transport = BleTransportBuilder::new("my-device").build();
        assert_eq!(transport.device_id(), "my-device");
        assert_eq!(transport.transport_type(), TransportType::BLE);
    }

    #[test]
    fn test_ble_reassembled_payload_rejects_oversized() {
        let transport = BleTransport::new("test-device");
        // Split an oversized payload across many fragments so each individual
        // fragment's data_len fits in a u16 but the reassembled total exceeds
        // DEFAULT_MAX_MESSAGE_SIZE.
        let chunk_size: usize = 60_000; // well under u16::MAX
        let num_fragments = (DEFAULT_MAX_MESSAGE_SIZE / chunk_size) + 2; // guarantees total > limit

        for i in 0..num_fragments {
            let frag = encode_fragment(
                b"big",
                i as u16,
                num_fragments as u16,
                &vec![0xAA; chunk_size],
            )
            .unwrap();

            let result = transport.process_fragment(&frag);
            if i < num_fragments - 1 {
                // Incomplete — should be Ok(None)
                assert!(result.unwrap().is_none());
            } else {
                // Final fragment completes assembly — must reject as too large
                assert!(result.is_err());
                assert!(matches!(
                    result.unwrap_err(),
                    crate::Error::MessageTooLarge(_, _)
                ));
            }
        }
    }

    #[test]
    fn test_fragment_eviction_fires_callback() {
        let transport = BleTransport::new("test-device");

        let evictions: Arc<Mutex<Vec<FragmentEvictionInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let evictions_clone = evictions.clone();
        transport.set_fragment_eviction_callback(Some(Arc::new(move |info| {
            evictions_clone.lock().unwrap().push(info);
        })));

        // Fill the reassembly buffer to capacity with partial assemblies.
        // Each entry: message_id "msg-N", fragment 0 of 4 (so 25% complete).
        for i in 0..BLE_MAX_FRAGMENT_ASSEMBLIES {
            let msg_id = format!("msg-{i}");
            let frag = encode_fragment(msg_id.as_bytes(), 0, 4, b"payload-data").unwrap();
            let result = transport.process_fragment(&frag).unwrap();
            assert!(result.is_none(), "partial fragment should not complete");
        }

        // No evictions yet
        assert!(evictions.lock().unwrap().is_empty());

        // Add one more — must trigger eviction of the least-valuable entry.
        let trigger_frag = encode_fragment(b"new-msg", 0, 4, b"payload-data").unwrap();
        let result = transport.process_fragment(&trigger_frag).unwrap();
        assert!(result.is_none());

        let captured = evictions.lock().unwrap();
        assert_eq!(captured.len(), 1, "exactly one eviction should fire");
        assert_eq!(captured[0].completion_percent, 25);
        assert!(
            captured[0].message_id.starts_with("msg-"),
            "evicted entry should be one of the pre-filled assemblies"
        );
    }
}
