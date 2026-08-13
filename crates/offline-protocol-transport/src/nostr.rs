//! Nostr relay transport queue engine.
//!
//! Censorship-resistant, decentralized messaging over Nostr relays
//! (WebSockets). No relay connection is opened here: the platform side
//! manages the actual WebSocket connections and subscriptions; the Rust
//! side manages queues, metrics, event signing, and the confirmation loop.
//!
//! The bridge contract: the platform reports relay connectivity via
//! [`NostrTransport::on_status_changed`], drains signed events with
//! [`NostrTransport::get_next_signed_event`] (woken by the
//! [`NostrTransport::set_on_messages_available`] callback) and submits them
//! to the relays, correlates relay `OK` responses back via
//! [`NostrTransport::confirm_sent`] /
//! [`NostrTransport::report_send_failure`], and injects inbound event
//! payloads via [`NostrTransport::on_data_received`].
//!
//! Addressing uses public routing tags derived from this device's address
//! ([`nostr_crypto::routing_tag_for_address`]); event signing uses a
//! per-install secret key that starts out ephemeral and is upgraded to a
//! persisted identity via [`NostrTransport::install_signing_secret`].

use crate::constants::{
    NOSTR_CLOCK_SKEW_MARGIN_SECS, NOSTR_CONNECTION_TIMEOUT_SECS, NOSTR_CREATED_AT_JITTER_SECS,
    NOSTR_FIRST_RUN_BACKFILL_SECS, NOSTR_FUTURE_DATED_TOLERANCE_SECS, NOSTR_MAX_PAYLOAD_SIZE,
    NOSTR_MAX_TRACKED_PEER_KEYS, NOSTR_PENDING_CONFIRMATION_TIMEOUT_SECS,
};
use crate::nostr_crypto::{self, now_unix_secs, NostrKeypair};
use crate::{
    Error, Result, SharedCallback, Transport, TransportMetrics, TransportStatus, TransportType,
};
use base64::Engine;
use offline_protocol_core::{Address, Message, MutexExt, RwLockExt};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::common::recalculate_delivery_ratios;

/// Maximum number of signing attempts before a message is permanently failed.
const MAX_SIGN_RETRIES: u8 = 3;

/// Marks a synthetic message id belonging to a published key-package record
/// rather than a protocol message.
///
/// Publications ride the ordinary send queue — the bridge treats `event_json`
/// as opaque, so nothing platform-side needed to change to publish them — but
/// they have no outbox entry, no ACK, and no retry ladder behind them. The
/// prefix keeps that visible in logs and confirmation callbacks; the engine's
/// `on_transport_send_confirmed` already returns early for an id it holds no
/// welcome lifecycle for, so a publication's confirm is a no-op by
/// construction rather than by a check that could be forgotten.
pub const NOSTR_PUBLICATION_ID_PREFIX: &str = "__nostr_kp__:";

/// Recovers the slot id from a publication's synthetic message id, or `None`
/// for an ordinary message id.
///
/// Every message-path side effect must consult this, not just the failure
/// reporting that motivated it. In particular a publication's outcome is
/// deliberately kept out of [`TransportMetrics`]: DORS scores this transport's
/// reliability on `success_count / (success_count + failure_count)`, those
/// counters are lifetime totals with no decay, and an idle install publishes
/// far more than it sends — so counting publications would score the transport
/// on something other than its ability to carry messages. A relay that rejects
/// kind 30443 would drive the ratio toward zero and make DORS deprioritise
/// Nostr for traffic that delivers fine; publications that succeed would
/// equally mask real message failures. See
/// `test_publication_outcomes_stay_out_of_the_delivery_metrics`.
fn publication_slot_id(message_id: &str) -> Option<String> {
    message_id
        .strip_prefix(NOSTR_PUBLICATION_ID_PREFIX)
        .map(str::to_string)
}

/// Maximum peers queued for key-package resolution at once.
///
/// Resolution is triggered by a send to a peer whose per-install key we lack,
/// and the recipient of a send is wire-influenced, so this is bounded like
/// every other such queue. Overflow drops the *newest* request: the queued
/// ones are already-attempted sends waiting on an answer, and a peer whose
/// request is dropped simply takes the bootstrap leg and is retried on the
/// next send to them.
const MAX_PENDING_RESOLUTIONS: usize = 64;

/// Minimum interval between resolution attempts for the same peer.
///
/// Without this, every frame to a peer who has published nothing would mint
/// another relay round-trip that cannot succeed. The bootstrap leg carries
/// those sends meanwhile, so the only cost of waiting is that the metadata
/// upgrade lands later.
const RESOLUTION_RETRY_INTERVAL: Duration = Duration::from_secs(300);

/// Maximum events one resolution query will accept, opened or not.
///
/// The REQ asks each relay for [`NOSTR_KEY_PACKAGE_SLOTS`] records, but a relay
/// is free to ignore `limit` and stream indefinitely, and the query is
/// broadcast so every connected relay answers under the same subscription id.
/// Each record that opens costs the key-package handler two durable
/// secure-storage writes, so the ceiling has to be ours rather than the
/// relay's. Sized for a generous fan-out — well above `slots × relays` for any
/// real configuration — since exceeding it only costs the metadata upgrade,
/// which falls back to the bootstrap leg.
const MAX_QUERY_EVENTS: usize = 64;

/// A resolution query the platform is currently running.
#[derive(Debug)]
struct ActiveQuery {
    /// The peer being resolved. An inbound event is meaningless without it:
    /// this is whose derivable key opens the record.
    ///
    /// Carried as a parsed [`Address`] so the record-seal derivation this
    /// feeds cannot be handed a string that was never validated.
    user_id: Address,
    /// Event ids already taken for this query. The query is broadcast, so the
    /// same record arrives once per relay, and opening it more than once
    /// re-runs the key-package handler's durable writes for no gain. Bounded
    /// by `delivered` below, which is checked before anything is inserted.
    seen_events: HashSet<String>,
    /// Events delivered for this query so far, whether or not they opened.
    delivered: usize,
}

/// A key-package record waiting to be published to the relays.
#[derive(Debug, Clone)]
struct PendingPublication {
    /// The addressable slot (`d` tag) this record occupies. Republishing the
    /// same slot replaces the record rather than adding to it.
    slot_id: String,
    /// Serialized protocol message carrying the signed `KeyPackagePayload`.
    payload: Vec<u8>,
}

/// A relay query the platform should issue on the transport's behalf.
#[derive(Debug, Clone)]
pub struct NostrQuery {
    /// Correlation id; also the NIP-01 subscription id the platform must use,
    /// so inbound events can be routed back to the right request.
    pub query_id: String,
    /// Complete `["REQ", ...]` JSON string for the relay WebSocket.
    pub req_json: String,
}

/// Mints a subscription id for a resolution query.
///
/// Random rather than sequential: the id goes on the wire in the REQ, and a
/// counter would tell every relay how many peers this install has looked up.
/// A failed RNG degrades to a fixed id, which costs correlation between one
/// install's queries — never correctness, since the platform routes on
/// whatever id it was handed.
fn new_query_id() -> String {
    use rand_core::{OsRng, RngCore};
    let mut buf = [0u8; 16];
    if OsRng.try_fill_bytes(&mut buf).is_err() {
        tracing::warn!("OS RNG unavailable for Nostr query id; using a fixed one");
        return "offline-protocol-kp-query".to_string();
    }
    hex::encode(buf)
}

/// A signed Nostr event ready for relay submission, together with the
/// metadata the platform needs for confirmation tracking.
#[derive(Debug, Clone)]
pub struct SignedNostrEvent {
    /// Protocol message ID (for confirm/fail callbacks).
    pub message_id: String,
    /// Nostr event ID (SHA-256 hex). The platform uses this to correlate
    /// relay `["OK", event_id, ...]` responses back to the message.
    pub event_id: String,
    /// Complete `["EVENT", {...}]` JSON string for the relay WebSocket.
    pub event_json: String,
}

/// Nostr transport configuration.
#[derive(Debug, Clone)]
pub struct NostrConfig {
    /// List of relay URLs to connect to (e.g., `["wss://relay.damus.io"]`).
    /// The platform manages actual WebSocket connections.
    pub relay_urls: Vec<String>,
    /// Connection timeout for reaching Nostr relays.
    pub connection_timeout: Duration,
    /// Enable automatic reconnection to relays.
    pub auto_reconnect: bool,
    /// Reconnection delay.
    pub reconnect_delay: Duration,
    /// Maximum reconnection attempts (0 = infinite).
    pub max_reconnect_attempts: u32,
}

impl Default for NostrConfig {
    fn default() -> Self {
        Self {
            relay_urls: Vec::new(),
            connection_timeout: Duration::from_secs(NOSTR_CONNECTION_TIMEOUT_SECS),
            auto_reconnect: true,
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 0,
        }
    }
}

/// Nostr relay transport implementation.
///
/// Provides connectivity via Nostr relays for censorship-resistant,
/// decentralized messaging. The platform bridges to Nostr relay
/// WebSocket connections and handles event signing and subscriptions.
///
/// ## Lock ordering
///
/// When acquiring more than one lock in a single scope, follow this order:
///
/// 1. `status`
/// 2. `pending_confirmation`
/// 3. `send_queue`
/// 4. `metrics`
/// 5. `receive_queue`
/// 6. `reconnect_attempts` / `platform_handle`
///
/// `keypair`, `receive_watermark`, `peer_nostr_pubkeys`, `sealing_enabled` and
/// `failed_publications` are leaf locks: they are only ever held in a narrow
/// scope with no other lock acquisition inside. In particular the sealing path
/// releases each before calling into the crypto layer, so a slow ECDH never
/// blocks the send queue, and the publication-failure paths collect their slot
/// ids under `pending_confirmation` and record them only after releasing it.
pub struct NostrTransport {
    device_id: String,
    /// Per-install signing keypair. Ephemeral (random per process) until the
    /// engine installs the persisted secret via
    /// [`Self::install_signing_secret`].
    keypair: RwLock<NostrKeypair>,
    /// This device's public routing tag (derived from `device_id`); peers
    /// address us by putting it in the `#p` tag, we subscribe on it.
    routing_tag: String,
    /// Newest `created_at` (unix seconds) of any relay event this install has
    /// accepted, or `None` before the first one — the high-water mark the
    /// subscription's `since` is derived from. See
    /// [`Self::advance_receive_watermark`].
    ///
    /// Owned here because [`Self::create_subscription`] needs it, but it is the
    /// *engine* that gives it durability: `OfflineProtocol` persists it as a
    /// protocol-state record and re-installs it on launch. Without that this is
    /// a per-process value and every cold start replays a full backfill window.
    receive_watermark: Mutex<Option<i64>>,
    /// The publicly computable keypair for our own address.
    ///
    /// Held to unseal bootstrap-leg frames and to seal our published records —
    /// see [`nostr_crypto::record_seal_keypair_for_address`], which documents
    /// why this is not a secret and must never authenticate anything.
    ///
    /// Its public half used to *be* `routing_tag`. It is now a separate,
    /// domain-separated derivation, so the two are unequal and neither can be
    /// mistaken for the other.
    record_seal_keypair: NostrKeypair,
    /// Peer user ID → that peer's real per-install Nostr public key, learned
    /// from the `nostr_pubkey` field of their signed key package. Populated by
    /// the engine via [`Self::set_peer_nostr_pubkey`]; a peer absent here takes
    /// the bootstrap path.
    peer_nostr_pubkeys: RwLock<HashMap<String, String>>,
    /// Whether outgoing frames are sealed into gift wraps. Mirrors
    /// `TransportConfig::nostr_sealing_enabled`; the receive path always
    /// accepts both forms regardless.
    sealing_enabled: Mutex<bool>,
    /// Whether this install publishes key-package records and resolves peers'.
    /// Mirrors `TransportConfig::nostr_cold_contact_enabled`.
    ///
    /// Gates both halves together on purpose: resolving records nobody
    /// publishes is pure round-trips, and publishing into a fleet that never
    /// resolves is a standing beacon bought for nothing.
    cold_contact_enabled: Mutex<bool>,
    /// Key-package records waiting to go out, drained ahead of the message
    /// queue so a fresh slot replaces a consumed one before the traffic that
    /// depends on it.
    publication_queue: Mutex<VecDeque<PendingPublication>>,
    /// Slot ids whose publication left this queue but never reached a relay,
    /// waiting for the engine to drain them via
    /// [`Self::take_failed_publications`].
    ///
    /// The engine marks a slot published when it *queues* the record, because
    /// that is the only point it hears about. Without this channel every
    /// failure after that point — a build error, a relay rejection, a
    /// confirmation timeout, a disconnect — would leave the slot marked
    /// published for the life of the process, so the relays hold nothing (or
    /// last process's stale record) while the engine believes the slot is
    /// healthy. That is precisely the silently-stale record the publication
    /// design exists to prevent.
    ///
    /// Bounded by construction: the keys are our own slot ids, of which the
    /// engine keeps at most `NOSTR_KEY_PACKAGE_SLOTS`.
    failed_publications: Mutex<HashSet<String>>,
    /// Peer addresses whose published key packages we want fetched.
    ///
    /// Holds parsed [`Address`]es rather than strings: an entry becomes the
    /// `#p` tag of a relay query, so admitting one that is not an address
    /// would publish a guessable label. `enqueue_resolution` is the only
    /// writer and parses there, which is what lets `next_query` derive a tag
    /// without re-validating or re-deciding what to do when it fails.
    resolve_queue: Mutex<VecDeque<Address>>,
    /// Query id → the query's state. An inbound event is meaningless without
    /// this: the id tells us whose derivable key opens it, and carries the
    /// per-query dedup and delivery ceiling with it.
    active_queries: Mutex<HashMap<String, ActiveQuery>>,
    /// Peer user id → last resolution attempt, rate-limiting repeats for a
    /// peer who has published nothing.
    resolve_attempts: Mutex<HashMap<String, Instant>>,
    config: NostrConfig,
    status: Arc<Mutex<TransportStatus>>,
    receive_queue: Arc<Mutex<VecDeque<Message>>>,
    send_queue: Arc<Mutex<VecDeque<Message>>>,
    /// Messages dequeued by the platform but not yet confirmed as sent.
    pending_confirmation: Arc<Mutex<HashMap<String, Instant>>>,
    /// Tracks how many times signing has been attempted for a given message ID.
    /// Entries are removed on success or after reaching [`MAX_SIGN_RETRIES`].
    sign_retry_counts: Arc<Mutex<HashMap<String, u8>>>,
    metrics: Arc<Mutex<TransportMetrics>>,
    reconnect_attempts: Arc<Mutex<u32>>,
    platform_handle: Arc<Mutex<Option<usize>>>,
    on_messages_available: SharedCallback,
}

impl NostrTransport {
    /// Creates a new Nostr transport with default configuration.
    pub fn new(device_id: impl Into<String>) -> Result<Self> {
        Self::with_config(device_id, NostrConfig::default())
    }

    /// Creates a new Nostr transport with custom configuration.
    ///
    /// # `device_id` must be this device's derived address
    ///
    /// Not a profile, not an app-chosen string: the id given here is the sole
    /// preimage of the routing tag, which is the label this device subscribes
    /// on and the one peers publish to. Both ways of getting it wrong are
    /// silent, and one of them is a disclosure:
    ///
    /// - a *guessable* preimage (a username-shaped profile) puts a label on a
    ///   public relay that anyone can recompute from the name, which is the
    ///   property the derived address exists to remove;
    /// - a preimage that is merely *different* from the address peers know
    ///   addresses this device where nobody writes, so every Nostr-carried
    ///   frame is simply never seen — no error, no event, nothing arriving.
    ///
    /// Neither failure surfaces anywhere at runtime, so the id is refused
    /// here rather than hashed. See
    /// [`test_construction_refuses_an_id_that_is_not_an_address`](self#tests),
    /// and — for the caller that made this necessary —
    /// `test_nostr_is_absent_until_the_identity_rebuild_installs_it` in the
    /// bindings crate.
    ///
    /// The signing keypair starts out ephemeral (random for this process);
    /// call [`Self::install_signing_secret`] once persisted storage is
    /// available to give the install a stable Nostr identity.
    pub fn with_config(device_id: impl Into<String>, config: NostrConfig) -> Result<Self> {
        let device_id = device_id.into();
        // The value is deliberately absent from the error: it is the rejected
        // id, which in the case this check exists for is the app's profile.
        let address = device_id.parse::<Address>().map_err(|e| {
            Error::ConfigurationError(format!(
                "Nostr transport requires this device's derived address: {e}"
            ))
        })?;
        let routing_tag = nostr_crypto::routing_tag_for_address(&address)?;
        let record_seal_keypair = nostr_crypto::record_seal_keypair_for_address(&address)?;
        let keypair = RwLock::new(NostrKeypair::generate_ephemeral()?);
        Ok(Self {
            device_id,
            keypair,
            routing_tag,
            receive_watermark: Mutex::new(None),
            record_seal_keypair,
            peer_nostr_pubkeys: RwLock::new(HashMap::new()),
            sealing_enabled: Mutex::new(true),
            cold_contact_enabled: Mutex::new(true),
            publication_queue: Mutex::new(VecDeque::new()),
            failed_publications: Mutex::new(HashSet::new()),
            resolve_queue: Mutex::new(VecDeque::new()),
            active_queries: Mutex::new(HashMap::new()),
            resolve_attempts: Mutex::new(HashMap::new()),
            config,
            status: Arc::new(Mutex::new(TransportStatus::Unavailable)),
            receive_queue: Arc::new(Mutex::new(VecDeque::new())),
            send_queue: Arc::new(Mutex::new(VecDeque::new())),
            pending_confirmation: Arc::new(Mutex::new(HashMap::new())),
            sign_retry_counts: Arc::new(Mutex::new(HashMap::new())),
            metrics: Arc::new(Mutex::new(TransportMetrics::default())),
            reconnect_attempts: Arc::new(Mutex::new(0)),
            platform_handle: Arc::new(Mutex::new(None)),
            on_messages_available: Arc::new(Mutex::new(None)),
        })
    }

    /// Gets the local device ID.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Gets the configuration.
    pub fn config(&self) -> &NostrConfig {
        &self.config
    }

    /// Sets the platform-specific handle.
    pub fn set_platform_handle(&self, handle: usize) {
        crate::common::set_platform_handle(&self.platform_handle, handle);
    }

    /// Gets the platform-specific handle.
    pub fn platform_handle(&self) -> Option<usize> {
        crate::common::platform_handle(&self.platform_handle)
    }

    /// Notifies the platform that messages are ready to send.
    ///
    /// The callback Arc is cloned out of the mutex and the guard dropped
    /// before the call, so a callback that re-enters the transport (e.g.
    /// another `send()`) cannot self-deadlock on the callback mutex.
    fn notify_messages_available(&self) {
        let callback = self.on_messages_available.lock_or_recover().clone();
        if let Some(cb) = callback {
            cb();
        }
    }

    /// Called when a message is received.
    pub fn on_message_received(&self, message: Message) {
        crate::common::on_message_received(&self.receive_queue, message);
    }

    /// Like [`on_message_received`](Self::on_message_received), but attaches a
    /// transport-verified `peer_id` to the message.
    pub fn on_message_received_from(&self, message: Message, peer_id: String) {
        crate::common::on_message_received_from(&self.receive_queue, message, peer_id);
    }

    /// Serializes a message to JSON bytes.
    pub fn serialize_message(&self, message: &Message) -> Result<Vec<u8>> {
        crate::common::serialize_message_with(message)
    }

    /// Whether the transport should attempt reconnection.
    pub fn should_reconnect(&self) -> bool {
        if !self.config.auto_reconnect {
            return false;
        }
        if self.config.max_reconnect_attempts == 0 {
            return true;
        }
        *self.reconnect_attempts.lock_or_recover() < self.config.max_reconnect_attempts
    }

    /// Increments the reconnection attempt counter.
    pub fn increment_reconnect_attempts(&self) {
        let mut attempts = self.reconnect_attempts.lock_or_recover();
        *attempts = attempts.saturating_add(1);
    }

    /// Updates transport metrics, preserving confirmation-loop counts.
    pub fn update_metrics(&self, incoming: TransportMetrics) {
        let mut metrics = self.metrics.lock_or_recover();
        let prev_success = metrics.success_count;
        let prev_failure = metrics.failure_count;
        *metrics = incoming;
        metrics.success_count = prev_success;
        metrics.failure_count = prev_failure;
        recalculate_delivery_ratios(&mut metrics);
    }

    /// Checks if there are messages waiting to be sent.
    pub fn has_pending_sends(&self) -> bool {
        !self.send_queue.lock_or_recover().is_empty()
    }

    /// Returns the number of messages awaiting platform confirmation.
    pub fn pending_confirmation_count(&self) -> usize {
        self.pending_confirmation.lock_or_recover().len()
    }

    // ========================================================================
    // Nostr crypto methods
    // ========================================================================

    /// Returns this install's Nostr signing public key as a 64-char hex string.
    ///
    /// This is the key outgoing events are signed with (their `pubkey`
    /// field), used by platforms to filter self-published events. It changes
    /// when [`Self::install_signing_secret`] swaps the ephemeral key for the
    /// persisted one, so platforms should read it after protocol
    /// initialization, not cache it across that boundary.
    pub fn public_key_hex(&self) -> String {
        self.keypair.read_or_recover().public_key_hex().to_string()
    }

    /// Returns this device's public routing tag (the `#p` value peers use to
    /// address us, and the value our relay subscription filters on).
    pub fn routing_tag(&self) -> &str {
        &self.routing_tag
    }

    /// Replaces the ephemeral signing keypair with one derived from the
    /// persisted per-install secret.
    ///
    /// Idempotent for a given secret: deriving from the same secret yields
    /// the same keypair. Events signed before this call used the ephemeral
    /// key, which peers accept because inbound events are never
    /// authenticated by their Nostr pubkey (sender authenticity comes from
    /// the protocol-layer MLS signatures).
    pub fn install_signing_secret(&self, secret: &[u8]) -> Result<()> {
        let keypair = NostrKeypair::from_install_secret(secret)?;
        let pubkey = keypair.public_key_hex().to_string();
        *self.keypair.write_or_recover() = keypair;
        tracing::debug!(
            pubkey = %pubkey,
            "Installed persisted Nostr signing key"
        );
        Ok(())
    }

    /// Enables or disables sealing of outgoing frames.
    ///
    /// Only the *send* side is gated. Inbound gift wraps are unsealed
    /// unconditionally, so turning this off does not make a peer's traffic
    /// unreadable and turning it back on needs no renegotiation.
    pub fn set_sealing_enabled(&self, enabled: bool) {
        *self.sealing_enabled.lock_or_recover() = enabled;
    }

    /// Whether outgoing frames are sealed.
    pub fn sealing_enabled(&self) -> bool {
        *self.sealing_enabled.lock_or_recover()
    }

    /// Enables or disables key-package publication and peer resolution.
    pub fn set_cold_contact_enabled(&self, enabled: bool) {
        *self.cold_contact_enabled.lock_or_recover() = enabled;
    }

    /// Whether key-package publication and peer resolution are enabled.
    pub fn cold_contact_enabled(&self) -> bool {
        *self.cold_contact_enabled.lock_or_recover()
    }

    /// Queues a key-package record for publication at `slot_id`.
    ///
    /// Replacing a slot is the caller's decision, not this queue's: the record
    /// is addressable, so publishing the same `slot_id` again overwrites it at
    /// the relay. A queued slot that is queued again before it drains is
    /// collapsed to the newer payload — the older one names a key package the
    /// engine has already replaced, and publishing both would briefly stand a
    /// consumed package back up as the live record.
    pub fn publish_key_package(&self, slot_id: &str, payload: Vec<u8>) {
        if !self.cold_contact_enabled() {
            return;
        }
        {
            let mut queue = self.publication_queue.lock_or_recover();
            queue.retain(|p| p.slot_id != slot_id);
            queue.push_back(PendingPublication {
                slot_id: slot_id.to_string(),
                payload,
            });
        }
        self.notify_messages_available();
    }

    /// Whether any key-package record is waiting to be published.
    pub fn has_pending_publications(&self) -> bool {
        !self.publication_queue.lock_or_recover().is_empty()
    }

    /// Records that a slot's publication never reached a relay.
    ///
    /// A `warn!` rather than a security warning: with the engine re-publishing
    /// on its next tick this is transient and self-healing, unlike an engine
    /// side refill failure (which leaves a genuinely stale record standing and
    /// does emit `NostrKeyPackageSlotExhausted`). A relay that rejects the kind
    /// outright makes this repeat once per refresh interval per slot — loud in
    /// the log and visible in the transport's failure metrics, but never
    /// escalating and never head-of-line blocking anything.
    fn mark_publications_failed(&self, slot_ids: Vec<String>) {
        if slot_ids.is_empty() {
            return;
        }
        let mut failed = self.failed_publications.lock_or_recover();
        for slot_id in slot_ids {
            tracing::warn!(
                slot_id = %slot_id,
                "Nostr key-package publication did not reach a relay; the slot will be republished"
            );
            failed.insert(slot_id);
        }
    }

    /// Drains the slot ids whose publication never reached a relay, so the
    /// engine can clear them from its published set and republish.
    pub fn take_failed_publications(&self) -> Vec<String> {
        self.failed_publications.lock_or_recover().drain().collect()
    }

    /// Requests resolution of `user_id`'s published key packages.
    ///
    /// Returns whether a request was newly queued; a `false` means the peer was
    /// already queued, was attempted within [`RESOLUTION_RETRY_INTERVAL`], or
    /// cold contact is disabled. Callers treat it as advisory — the send that
    /// triggered it proceeds on the bootstrap leg either way, because waiting
    /// on a relay round-trip would convert a metadata upgrade into latency.
    pub fn request_peer_key_packages(&self, user_id: &str) -> bool {
        let queued = self.enqueue_resolution(user_id);
        if queued {
            self.notify_messages_available();
        }
        queued
    }

    /// Queues a resolution without waking the platform.
    ///
    /// Used from the send path, which is already running inside the platform's
    /// poll loop: waking it from there would re-enter the transport partway
    /// through building an event, to tell it something it is about to look for
    /// anyway.
    fn enqueue_resolution(&self, user_id: &str) -> bool {
        // A queued id becomes the `#p` tag of a relay query, so it is held to
        // the same address requirement as a send recipient — and held here,
        // at the queue's only gate, so `next_query` can never pop an id whose
        // tag derivation fails after the entry was already consumed. Subsumes
        // the emptiness check this replaces: an empty id is not an address.
        if !self.cold_contact_enabled() {
            return false;
        }
        let Ok(address) = user_id.parse::<Address>() else {
            return false;
        };

        // Read the rate limit without stamping it. Stamping here would burn the
        // whole interval on a request the queue then refuses, so a peer dropped
        // at capacity would wait out `RESOLUTION_RETRY_INTERVAL` despite never
        // having been looked up — the opposite of what the overflow policy
        // promises.
        {
            let attempts = self.resolve_attempts.lock_or_recover();
            if let Some(last) = attempts.get(user_id) {
                if last.elapsed() < RESOLUTION_RETRY_INTERVAL {
                    return false;
                }
            }
        }

        {
            let mut queue = self.resolve_queue.lock_or_recover();
            if queue.iter().any(|q| *q == address) {
                return false;
            }
            if queue.len() >= MAX_PENDING_RESOLUTIONS {
                return false;
            }
            queue.push_back(address);
        }

        // Only now is an attempt real. Bounded like the peer-key map, and for
        // the same reason: the key is a wire-influenced recipient id. Clearing
        // costs at most one premature retry per forgotten peer.
        let mut attempts = self.resolve_attempts.lock_or_recover();
        if attempts.len() >= NOSTR_MAX_TRACKED_PEER_KEYS {
            attempts.clear();
        }
        attempts.insert(user_id.to_string(), Instant::now());
        true
    }

    /// Pops the next queued resolution and returns the REQ for the platform to
    /// issue, registering the query so inbound events can be routed back.
    pub fn next_query(&self) -> Result<Option<NostrQuery>> {
        let user_id = {
            let mut queue = self.resolve_queue.lock_or_recover();
            match queue.pop_front() {
                Some(u) => u,
                None => return Ok(None),
            }
        };

        let routing_tag = nostr_crypto::routing_tag_for_address(&user_id)?;
        let query_id = new_query_id();
        let req_json = nostr_crypto::create_key_package_query_message(&routing_tag, &query_id)?;

        {
            let mut active = self.active_queries.lock_or_recover();
            // A query the platform never completes would otherwise leak an
            // entry per attempt. The rate limit bounds the rate, this bounds
            // the total; the victim is whichever entry the map yields first
            // (arbitrary, not oldest — the order carries no meaning here) and
            // costs one unresolvable answer.
            if active.len() >= MAX_PENDING_RESOLUTIONS {
                if let Some(stale) = active.keys().next().cloned() {
                    active.remove(&stale);
                }
            }
            active.insert(
                query_id.clone(),
                ActiveQuery {
                    user_id,
                    seen_events: HashSet::new(),
                    delivered: 0,
                },
            );
        }

        Ok(Some(NostrQuery { query_id, req_json }))
    }

    /// Opens an event delivered for `query_id`, returning the author's Nostr
    /// pubkey and the decrypted protocol message bytes.
    ///
    /// `Ok(None)` means the event was not a record this query can open — a
    /// wrong kind, a foreign publisher at the same tag, or a payload that does
    /// not decrypt. All three are ordinary: the routing tag is public, so
    /// anyone may publish to it, and a query returns whatever the relay holds
    /// there. The caller drops those and keeps going.
    ///
    /// What comes back is *not* trusted here. It is fed through the same
    /// receive path as any inbound frame, so the Ed25519 control gate and the
    /// sender-address derivation decide whose key package it is — and a record
    /// placed at this peer's tag by somebody else therefore registers under
    /// *that* signer's identity, not under the peer we asked about.
    ///
    /// # What squatting a tag does buy
    ///
    /// Two things, both bounded, neither a loss:
    ///
    /// 1. **Crowding.** Foreign records displace real ones from the query's
    ///    `limit`, costing the metadata upgrade and nothing else: the send
    ///    falls back to the bootstrap leg exactly as it did before this
    ///    existed.
    /// 2. **Replaying a consumed record.** Every published record is openable
    ///    by anyone who knows the username — that is the design — so a squatter
    ///    can unseal one of the peer's *spent* records, re-seal the untouched
    ///    (and genuinely Ed25519-signed) message inside it under their own
    ///    author key, and stand it back up with a fresh `created_at`. Nothing
    ///    here detects it: the inner signature is real, and no freshness
    ///    binding ties a record to the live slot. A resolver then imports a
    ///    genuine-but-consumed key package and builds a Welcome the peer can
    ///    never process — worse than crowding, because it commits to a dead
    ///    session rather than staying on the working bootstrap leg.
    ///
    ///    It self-heals rather than stranding the pair: importing the package
    ///    runs the ordinary key-package handler, which pushes *our* package
    ///    back under `auto_key_exchange`, and the peer then establishes from
    ///    their side against a package that is actually live. So the cost is
    ///    delivery delayed by one exchange — the same bounded class as the
    ///    already-accepted `key_package_data` substitution vector, and not
    ///    something a squatter can escalate. Closing it outright needs the
    ///    record to carry slot-bound freshness (its slot id plus a signed issue
    ///    time) so a replay is distinguishable from the current record.
    pub fn open_query_event(
        &self,
        query_id: &str,
        event_json: &str,
    ) -> Result<Option<(String, Vec<u8>)>> {
        if event_json.len() > NOSTR_MAX_PAYLOAD_SIZE {
            tracing::warn!(
                len = event_json.len(),
                "Oversized Nostr key-package record; ignoring"
            );
            return Ok(None);
        }

        let user_id = {
            let mut active = self.active_queries.lock_or_recover();
            let Some(query) = active.get_mut(query_id) else {
                tracing::debug!(query_id = %query_id, "Nostr query event for an unknown query");
                return Ok(None);
            };
            // The relay decides how many events arrive here, so the ceiling has
            // to be ours. See [`MAX_QUERY_EVENTS`].
            if query.delivered >= MAX_QUERY_EVENTS {
                tracing::warn!(
                    query_id = %query_id,
                    cap = MAX_QUERY_EVENTS,
                    "Nostr resolution query exceeded its event ceiling; ignoring the rest"
                );
                return Ok(None);
            }
            query.delivered += 1;
            query.user_id
        };

        let event: serde_json::Value = match serde_json::from_str(event_json) {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(error = %e, "Unparseable Nostr key-package record");
                return Ok(None);
            }
        };

        if event.get("kind").and_then(|k| k.as_u64())
            != Some(nostr_crypto::NOSTR_KEY_PACKAGE_KIND as u64)
        {
            return Ok(None);
        }

        let (Some(author), Some(content)) = (
            event.get("pubkey").and_then(|p| p.as_str()),
            event.get("content").and_then(|c| c.as_str()),
        ) else {
            return Ok(None);
        };

        // The query is broadcast, so every connected relay answers it and the
        // same record arrives once per relay. Take each event id once: behind
        // this call the key-package handler performs two durable
        // secure-storage writes per record, and re-importing a package we
        // already hold buys nothing. Marked before opening rather than after,
        // so a record that repeatedly fails to open is absorbed too.
        if let Some(event_id) = event.get("id").and_then(|i| i.as_str()) {
            let mut active = self.active_queries.lock_or_recover();
            let Some(query) = active.get_mut(query_id) else {
                return Ok(None);
            };
            if !query.seen_events.insert(event_id.to_string()) {
                tracing::debug!(
                    query_id = %query_id,
                    "Duplicate Nostr key-package record for this query; already taken"
                );
                return Ok(None);
            }
        }

        let sealed = match base64::engine::general_purpose::STANDARD.decode(content) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::debug!(error = %e, "Nostr key-package record content is not base64");
                return Ok(None);
            }
        };

        // The peer's derivable key is computable from their user id — that is
        // the whole reason a record sealed to it stays fetchable by anyone
        // entitled to fetch it, while remaining opaque to a relay scraping by
        // kind alone.
        let peer_key = nostr_crypto::record_seal_keypair_for_address(&user_id)?;
        match nostr_crypto::open_key_package_publication(&peer_key, author, &sealed) {
            Ok(plaintext) => Ok(Some((author.to_string(), plaintext))),
            Err(e) => {
                tracing::debug!(error = %e, "Nostr key-package record did not open");
                Ok(None)
            }
        }
    }

    /// Releases a query once the platform has seen its end-of-stored-events.
    pub fn complete_query(&self, query_id: &str) {
        self.active_queries.lock_or_recover().remove(query_id);
    }

    /// Records a peer's real per-install Nostr public key, learned from the
    /// `nostr_pubkey` field of their key package.
    ///
    /// Until this is known, frames to that peer are sealed to their publicly
    /// computable key instead (bootstrap leg). Once it is known, every
    /// subsequent frame is sealed to a key only that install holds.
    ///
    /// A wrong value here is not merely a delivery denial: the plaintext is
    /// sealed *to* this key, so an attacker who substitutes their own reads the
    /// envelope metadata off a public relay. The engine is what keeps that
    /// shut — it calls this only for a key package whose Ed25519 signature it
    /// verified, never for one that merely arrived (its security gate accepts
    /// unsigned control messages from not-yet-pinned peers). Do not add a
    /// caller that skips that check.
    ///
    /// Bounded at [`NOSTR_MAX_TRACKED_PEER_KEYS`]; at capacity the map resets
    /// rather than evicting selectively, matching the engine's other
    /// wire-keyed maps. The only cost of forgetting a peer is that the next
    /// frame to them takes the bootstrap path until their key package is seen
    /// again.
    pub fn set_peer_nostr_pubkey(&self, user_id: &str, pubkey_hex: &str) {
        if user_id.is_empty()
            || pubkey_hex.len() != 64
            || !pubkey_hex.bytes().all(|b| b.is_ascii_hexdigit())
        {
            tracing::debug!(
                user_id = %user_id,
                "Ignoring malformed peer Nostr pubkey"
            );
            return;
        }

        let mut map = self.peer_nostr_pubkeys.write_or_recover();
        if map.len() >= NOSTR_MAX_TRACKED_PEER_KEYS && !map.contains_key(user_id) {
            tracing::warn!(
                capacity = NOSTR_MAX_TRACKED_PEER_KEYS,
                "Nostr peer key map at capacity; clearing"
            );
            map.clear();
        }
        map.insert(user_id.to_string(), pubkey_hex.to_ascii_lowercase());
    }

    /// Forgets a peer's per-install Nostr public key, reverting frames to them
    /// to the bootstrap path.
    ///
    /// The engine calls this when it declares everything learned about a peer
    /// stale (the unblock clean slate). Reverting rather than refusing to send
    /// is deliberate: the bootstrap key is derived from the peer's user id, so
    /// it is readable by whatever install they are on now — where a key cached
    /// from a since-wiped install is readable by nobody.
    pub fn forget_peer_nostr_pubkey(&self, user_id: &str) {
        self.peer_nostr_pubkeys.write_or_recover().remove(user_id);
    }

    /// Returns the recipient's known per-install Nostr public key, if any.
    fn peer_nostr_pubkey(&self, user_id: &str) -> Option<String> {
        self.peer_nostr_pubkeys
            .read_or_recover()
            .get(user_id)
            .cloned()
    }

    /// Unseals an inbound event payload, or returns it unchanged if it is not
    /// sealed.
    ///
    /// `sender_pubkey_hex` is the wrapper event's `pubkey` — a single-use key
    /// with no identity attached. `data` is the base64-decoded event `content`,
    /// which is the form the platform bridges produce.
    ///
    /// Two of our own keys can unseal a frame and both are tried, cheapest and
    /// most likely first:
    ///
    /// 1. the per-install signing key, used by any peer that has seen our key
    ///    package — the steady state;
    /// 2. the publicly computable key, used on the bootstrap leg before the
    ///    sender has learned our real one.
    ///
    /// A wrong key is rejected by the NIP-44 MAC, so trying both is
    /// unambiguous rather than a guess. A frame that unseals with neither is
    /// returned to the caller **as-is**: it may still be a legacy unsealed
    /// frame, and the caller's decoder is what decides. That ordering matters —
    /// deciding "sealed but undecryptable" here and dropping would break every
    /// peer that predates sealing.
    pub fn unseal_event_payload<'a>(
        &self,
        sender_pubkey_hex: &str,
        data: &'a [u8],
    ) -> Cow<'a, [u8]> {
        if !nostr_crypto::is_sealed_payload(data) {
            return Cow::Borrowed(data);
        }
        // Anyone can publish to our routing tag, so this is attacker-sized
        // input. Unsealing costs an ECDH plus an HMAC and a ChaCha20 pass over
        // the whole buffer; do that only for something that could plausibly be
        // a message. The receive path rejects it at the same bound anyway, so
        // this only moves the rejection ahead of the crypto.
        if data.len() > crate::constants::DEFAULT_MAX_MESSAGE_SIZE {
            tracing::warn!(
                len = data.len(),
                "Oversized sealed Nostr frame; not attempting to unseal"
            );
            return Cow::Borrowed(data);
        }

        {
            let keypair = self.keypair.read_or_recover();
            if let Ok(plaintext) = nostr_crypto::unwrap_gift_wrap(&keypair, sender_pubkey_hex, data)
            {
                return Cow::Owned(plaintext);
            }
        }

        match nostr_crypto::unwrap_gift_wrap(&self.record_seal_keypair, sender_pubkey_hex, data) {
            Ok(plaintext) => {
                tracing::debug!("Unsealed a bootstrap-leg Nostr frame");
                Cow::Owned(plaintext)
            }
            Err(e) => {
                // Not addressed to us, or corrupt. Handing the original bytes
                // back lets the caller's decoder produce the single
                // "undecodable frame" outcome rather than two divergent ones.
                tracing::debug!(error = %e, "Sealed Nostr frame did not unseal with either key");
                Cow::Borrowed(data)
            }
        }
    }

    /// Builds the event content for `message`: a gift wrap when sealing is on,
    /// the legacy base64 envelope otherwise.
    ///
    /// Returns the whole signed event so the two paths differ in kind, signing
    /// key, and timestamp — not just in how `content` is encoded.
    fn build_event(&self, message: &Message) -> Result<nostr_crypto::NostrEvent> {
        let recipient_device_id = message.recipient.as_str();
        // `send` already refused a non-address recipient before queueing, so
        // this parse is a restatement rather than a new gate — but it is the
        // one that produces the typed value the derivations below require, and
        // re-deriving it here means a future queue-filling path cannot skip
        // the check by accident.
        let recipient_address = recipient_device_id.parse::<Address>().map_err(|_| {
            crate::Error::PeerNotReachable(
                "Nostr addresses peers by their derived address, and the \
                 recipient id is not one"
                    .to_string(),
            )
        })?;
        let recipient_tag = nostr_crypto::routing_tag_for_address(&recipient_address)?;
        let data = self.serialize_message(message)?;

        if !self.sealing_enabled() {
            let content_base64 = base64::engine::general_purpose::STANDARD.encode(&data);
            let keypair = self.keypair.read_or_recover();
            return nostr_crypto::NostrEvent::create_dm(&keypair, &recipient_tag, &content_base64);
        }

        // With no known per-install key for the recipient, seal to their
        // publicly computable one. That is the bootstrap leg: it defeats bulk
        // collection by a relay, and nothing more — anyone who guesses the
        // recipient's user ID holds the matching private half. It is chosen
        // over refusing to send (which would make a stranger unreachable over
        // Nostr) and over falling back to a cleartext kind-4 event (which
        // would make first contact trivially identifiable with a one-line
        // relay filter). On the wire the two cases are indistinguishable.
        let encryption_pubkey = match self.peer_nostr_pubkey(recipient_device_id) {
            Some(key) => key,
            None => {
                // Ask the relays for the recipient's published records, so the
                // *next* frame seals to a key only they hold. This frame still
                // takes the bootstrap leg: blocking a send on a relay
                // round-trip would turn a metadata upgrade into latency, and
                // the round-trip may have nothing to return.
                //
                // This is also what bounds the peer-key map's reset-at-capacity
                // downgrade for peers who publish — a forgotten key is
                // re-resolved on the next send rather than waiting for a fresh
                // key-package exchange.
                self.enqueue_resolution(recipient_device_id);
                // Their record-seal key, reconstructed from their address —
                // no longer the routing tag, which since the key split is a
                // label with no private half anyone holds.
                nostr_crypto::record_seal_keypair_for_address(&recipient_address)?
                    .public_key_hex()
                    .to_string()
            }
        };

        nostr_crypto::NostrEvent::create_gift_wrap(&recipient_tag, &encryption_pubkey, &data)
    }

    /// Builds the next queued key-package publication, if any.
    ///
    /// Publications are drained ahead of messages so a slot refreshed after
    /// consumption reaches the relay before the traffic that made it stale.
    /// A failure here drops the record rather than re-queueing it — retrying in
    /// place would head-of-line block the message queue behind a record that
    /// keeps failing — and instead reports the slot through
    /// [`Self::mark_publications_failed`], which is what makes the engine
    /// republish it on a later tick. Reporting it is not optional: the engine
    /// marked the slot published when it queued the record, so a silent drop
    /// here strands the slot until the process restarts.
    fn next_publication_event(&self) -> Result<Option<SignedNostrEvent>> {
        let pending = {
            let mut queue = self.publication_queue.lock_or_recover();
            match queue.pop_front() {
                Some(p) => p,
                None => return Ok(None),
            }
        };

        let message_id = format!("{}{}", NOSTR_PUBLICATION_ID_PREFIX, pending.slot_id);

        let result = (|| {
            let event = {
                let keypair = self.keypair.read_or_recover();
                nostr_crypto::NostrEvent::create_key_package_publication(
                    &keypair,
                    &self.routing_tag,
                    self.record_seal_keypair.public_key_hex(),
                    &pending.slot_id,
                    &pending.payload,
                )?
            };
            let event_id = event.id.clone();
            let event_json = event.to_relay_message()?;
            if event_json.len() > NOSTR_MAX_PAYLOAD_SIZE {
                return Err(crate::Error::MessageTooLarge(
                    event_json.len(),
                    NOSTR_MAX_PAYLOAD_SIZE,
                ));
            }
            Ok((event_id, event_json))
        })();

        match result {
            Ok((event_id, event_json)) => {
                self.pending_confirmation
                    .lock_or_recover()
                    .insert(message_id.clone(), Instant::now());
                Ok(Some(SignedNostrEvent {
                    message_id,
                    event_id,
                    event_json,
                }))
            }
            Err(e) => {
                tracing::error!(
                    slot_id = %pending.slot_id,
                    error = %e,
                    "Failed to build a Nostr key-package publication; slot left unpublished"
                );
                self.mark_publications_failed(vec![pending.slot_id]);
                Err(e)
            }
        }
    }

    /// Pops the next outgoing message, creates a signed Nostr event, and returns
    /// `(message_id, recipient_device_id, relay_event_json)`.
    ///
    /// The `relay_event_json` is a complete `["EVENT", {...}]` string ready to
    /// send over a WebSocket connection. The platform no longer needs to do
    /// any signing or event creation.
    ///
    /// Events larger than [`NOSTR_MAX_PAYLOAD_SIZE`] are dropped here rather
    /// than handed to the platform, since relays would reject them on arrival.
    /// That drop is permanent — unlike a signing failure, an oversized event is
    /// oversized on every attempt, so retrying it would only head-of-line-block
    /// the queue behind a message no relay will accept.
    ///
    /// **Sealing shrinks how much fits under that cap**, by more than the
    /// base64 layer alone suggests: NIP-44 pads to a power-of-two bucket, so a
    /// payload just past a boundary very nearly doubles before the MAC and
    /// base64 are applied. The check therefore runs on the final event, after
    /// sealing, and a message that fits unsealed may not fit sealed.
    pub fn get_next_signed_event(&self) -> Result<Option<SignedNostrEvent>> {
        self.drain_expired_pending();

        if let Some(publication) = self.next_publication_event()? {
            return Ok(Some(publication));
        }

        let message = {
            let mut queue = self.send_queue.lock_or_recover();
            match queue.pop_front() {
                Some(m) => m,
                None => return Ok(None),
            }
        };

        let message_id = message.id.to_string();

        let result = (|| {
            let event = self.build_event(&message)?;
            let event_id = event.id.clone();
            let event_json = event.to_relay_message()?;
            if event_json.len() > NOSTR_MAX_PAYLOAD_SIZE {
                return Err(crate::Error::MessageTooLarge(
                    event_json.len(),
                    NOSTR_MAX_PAYLOAD_SIZE,
                ));
            }
            Ok((event_id, event_json))
        })();

        match result {
            Ok((event_id, event_json)) => {
                self.sign_retry_counts.lock_or_recover().remove(&message_id);
                self.pending_confirmation
                    .lock_or_recover()
                    .insert(message_id.clone(), Instant::now());

                Ok(Some(SignedNostrEvent {
                    message_id,
                    event_id,
                    event_json,
                }))
            }
            Err(e) => {
                let retriable = match &e {
                    // No number of attempts shrinks an oversized event.
                    crate::Error::MessageTooLarge(_, _) => false,
                    _ => self.record_sign_attempt(&message_id) < MAX_SIGN_RETRIES,
                };

                if retriable {
                    // Re-enqueue for another attempt.
                    self.send_queue.lock_or_recover().push_front(message);
                } else {
                    self.fail_permanently(&message_id, &e);
                }
                Err(e)
            }
        }
    }

    /// Records a signing attempt for `message_id` and returns the running count.
    fn record_sign_attempt(&self, message_id: &str) -> u8 {
        let mut counts = self.sign_retry_counts.lock_or_recover();
        let count = counts.entry(message_id.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        *count
    }

    /// Drops a message that cannot be published and records the failure.
    ///
    /// The message has already been popped off the send queue and was never
    /// entered into `pending_confirmation` — that happens only once an event
    /// reaches the platform — so the failure is counted directly here.
    /// [`Transport::report_send_failure`] is keyed on a pending entry and would
    /// be a no-op, leaving the failure invisible to DORS.
    fn fail_permanently(&self, message_id: &str, error: &crate::Error) {
        self.sign_retry_counts.lock_or_recover().remove(message_id);

        let mut metrics = self.metrics.lock_or_recover();
        metrics.failure_count = metrics.failure_count.saturating_add(1);
        recalculate_delivery_ratios(&mut metrics);
        drop(metrics);

        tracing::error!(
            message_id = %message_id,
            error_code = error.code(),
            error = %error,
            "Nostr event dropped permanently; it will not be published"
        );
    }

    /// Returns the newest accepted event timestamp (unix seconds), or `None`
    /// if no event has been accepted since the last watermark install.
    ///
    /// Read by the engine to persist the watermark.
    pub fn receive_watermark_secs(&self) -> Option<i64> {
        *self.receive_watermark.lock_or_recover()
    }

    /// Records that an event dated `created_at_secs` (unix seconds) has been
    /// received, moving the receive watermark forward. Returns whether the
    /// watermark actually moved.
    ///
    /// Two rules, both load-bearing:
    ///
    /// - **Monotonic.** The mark only ever advances, so out-of-order delivery
    ///   (the relay streams stored events newest-first) cannot walk it back.
    /// - **Never into the future.** Anyone can address an event to our routing
    ///   tag — it is derived from a public user id — and `created_at` is
    ///   written by whoever published the event. A single event dated far ahead
    ///   would otherwise pin the mark there and make every later subscription
    ///   ask for events `since` the far future, receiving nothing forever. So
    ///   values more than [`NOSTR_FUTURE_DATED_TOLERANCE_SECS`] ahead of local
    ///   time are dropped, not clamped.
    ///
    /// The restore path goes through this same function deliberately: a
    /// watermark persisted by a build without the future check must not be able
    /// to stall a subscription either.
    ///
    /// Callers should advance the mark only for frames that actually decoded
    /// into a [`Message`]. The mark means "receive progress has reached here",
    /// and a frame we cannot parse is one we never processed — counting it
    /// would let undecodable traffic push the window past real messages still
    /// waiting on the relay. The failure mode of the stricter rule is replaying
    /// more, never less, which is the right bias for a delivery path.
    pub fn advance_receive_watermark(&self, created_at_secs: i64) -> bool {
        if created_at_secs <= 0 {
            return false;
        }
        let ceiling = now_unix_secs().saturating_add(NOSTR_FUTURE_DATED_TOLERANCE_SECS);
        if created_at_secs > ceiling {
            tracing::debug!(
                created_at = created_at_secs,
                ceiling,
                "Ignoring future-dated Nostr event for the receive watermark"
            );
            return false;
        }

        let mut mark = self.receive_watermark.lock_or_recover();
        match *mark {
            Some(current) if current >= created_at_secs => false,
            _ => {
                *mark = Some(created_at_secs);
                true
            }
        }
    }

    /// The `since` this device's subscription filter should carry (unix
    /// seconds), derived from the receive watermark.
    ///
    /// With a watermark, it reaches back past it by the gift-wrap jitter window
    /// plus a clock-skew margin — an event published now may legitimately carry
    /// a `created_at` up to a jitter window old, and a sender's clock may lag
    /// ours, so a `since` sitting exactly at the mark would skip both. The
    /// watermark is also clamped to `now` first, so a mark restored from an
    /// older build that lacked the future-dated check cannot push `since`
    /// forward.
    ///
    /// With no watermark — a fresh install, a wiped one, or any subscription
    /// built before protocol-state storage has been restored — it falls back to
    /// [`NOSTR_FIRST_RUN_BACKFILL_SECS`] ago. Never zero: an absent or zero
    /// `since` is the unbounded filter this exists to remove.
    ///
    /// **Residual:** the mark can only be as good as what we have received. A
    /// relay that truncates a reconnect's history at
    /// [`NOSTR_INITIAL_QUERY_LIMIT`](crate::constants::NOSTR_INITIAL_QUERY_LIMIT)
    /// hands back its newest events, so a device that comes back to more than
    /// that much stored history advances past what it never saw. The overlap
    /// below bounds how much: only events already older than jitter + skew at
    /// the moment of truncation are at risk.
    fn subscription_since(&self) -> i64 {
        let now = now_unix_secs();
        match self.receive_watermark_secs() {
            Some(mark) => mark
                .min(now)
                .saturating_sub(NOSTR_CREATED_AT_JITTER_SECS)
                .saturating_sub(NOSTR_CLOCK_SKEW_MARGIN_SECS)
                .max(0),
            None => now.saturating_sub(NOSTR_FIRST_RUN_BACKFILL_SECS).max(0),
        }
    }

    /// Returns a NIP-01 subscription filter JSON for this device's routing tag.
    ///
    /// The platform should send this to each relay after connecting:
    /// `["REQ", "<sub_id>", {"#p": ["<routing_tag>"], "kinds": [4, 1059], "since": T, "limit": N}]`
    ///
    /// The filter is on the routing tag — not the signing pubkey — so it is
    /// stable across signing-key changes and derivable by peers. `since` bounds
    /// how far back stored-event replay reaches — it is derived from the
    /// receive watermark by the private `subscription_since`, documented on
    /// [`Self::advance_receive_watermark`] — while `limit` caps how much of
    /// that window one (re)connect pulls down. Neither caps live delivery.
    pub fn create_subscription(&self, subscription_id: &str) -> Result<String> {
        nostr_crypto::create_subscription_message(
            &self.routing_tag,
            subscription_id,
            self.subscription_since(),
        )
    }

    /// Fails all pending confirmations and records them as failures.
    fn fail_all_pending(&self) {
        let (pending, publications) = {
            let mut map = self.pending_confirmation.lock_or_recover();
            let publications: Vec<String> = map
                .keys()
                .filter_map(|id| publication_slot_id(id))
                .collect();
            // Only the messages count as failures — the publications among
            // them are reported to the engine instead. See
            // [`publication_slot_id`].
            let count = map.len().saturating_sub(publications.len());
            map.clear();
            (count, publications)
        };
        self.mark_publications_failed(publications);
        if pending > 0 {
            let mut metrics = self.metrics.lock_or_recover();
            metrics.failure_count = metrics.failure_count.saturating_add(pending as u32);
            recalculate_delivery_ratios(&mut metrics);
        }
    }

    /// Expires pending confirmations that have exceeded the timeout.
    fn drain_expired_pending(&self) {
        let timeout = Duration::from_secs(NOSTR_PENDING_CONFIRMATION_TIMEOUT_SECS);
        let now = Instant::now();
        let mut expired_count = 0u32;
        let mut expired_publications = Vec::new();

        {
            let mut pending = self.pending_confirmation.lock_or_recover();
            pending.retain(|message_id, enqueued_at| {
                if now.duration_since(*enqueued_at) > timeout {
                    // A timed-out publication is reported to the engine, not
                    // counted as a delivery failure. See
                    // [`publication_slot_id`].
                    match publication_slot_id(message_id) {
                        Some(slot_id) => expired_publications.push(slot_id),
                        None => expired_count += 1,
                    }
                    false
                } else {
                    true
                }
            });
        }

        self.mark_publications_failed(expired_publications);

        if expired_count > 0 {
            let mut metrics = self.metrics.lock_or_recover();
            metrics.failure_count = metrics.failure_count.saturating_add(expired_count);
            recalculate_delivery_ratios(&mut metrics);
        }
    }
}

impl Transport for NostrTransport {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn transport_type(&self) -> TransportType {
        TransportType::Nostr
    }

    fn status(&self) -> TransportStatus {
        *self.status.lock_or_recover()
    }

    fn metrics(&self) -> TransportMetrics {
        self.metrics.lock_or_recover().clone()
    }

    fn send(&self, message: &Message) -> Result<()> {
        let status = *self.status.lock_or_recover();
        if status != TransportStatus::Available {
            return Err(crate::Error::TransportNotAvailable(format!(
                "Nostr transport is {:?}",
                status
            )));
        }

        // The recipient is the sole preimage of the `#p` tag this frame is
        // published under, so it is refused here rather than hashed — the same
        // rule `with_config` applies to our own id, and for the same two silent
        // failures: a username-shaped id puts a label anyone can recompute from
        // the name onto third-party relays, and any non-address id addresses
        // the frame where nobody subscribes.
        //
        // Refused at `send` rather than in `build_event` because a build
        // failure is retriable by the drain: a permanently undeliverable
        // recipient would burn the whole `MAX_SIGN_RETRIES` ladder, republishing
        // that label each time, before failing. Here it costs one error to the
        // caller, which routes to another transport or fails the message.
        //
        // The value stays out of the error for the reason `with_config` keeps
        // it out: the id this rejects is, in the case worth catching, a
        // username.
        if message.recipient.as_str().parse::<Address>().is_err() {
            return Err(crate::Error::PeerNotReachable(
                "Nostr addresses peers by their derived address, and the \
                 recipient id is not one"
                    .to_string(),
            ));
        }

        let queue_len = {
            let mut queue = self.send_queue.lock_or_recover();
            queue.push_back(message.clone());
            queue.len()
        };

        let mut metrics = self.metrics.lock_or_recover();
        metrics.queue_depth = queue_len;
        metrics.congestion = ((queue_len as f32) / 50.0).clamp(0.0, 1.0);
        drop(metrics);

        self.notify_messages_available();

        Ok(())
    }

    fn receive(&self) -> Result<Option<Message>> {
        let mut queue = self.receive_queue.lock_or_recover();
        Ok(queue.pop_front())
    }

    fn start(&self) -> Result<()> {
        // Actual connection is managed by the platform.
        // Status is updated via on_status_changed().
        Ok(())
    }

    /// Stops the transport, clearing in-flight queues.
    ///
    /// `receive_watermark` is deliberately **not** cleared: it records how far
    /// receive progress has reached, which a `stop()`/`start()` cycle does not
    /// undo. Clearing it would make every restart fall back to the first-run
    /// backfill window and replay history the engine has already processed.
    fn stop(&self) -> Result<()> {
        *self.status.lock_or_recover() = TransportStatus::Disconnected;
        self.fail_all_pending();
        self.send_queue.lock_or_recover().clear();
        self.receive_queue.lock_or_recover().clear();
        Ok(())
    }

    /// Called when connection status changes.
    ///
    /// Resets reconnect counter on successful connection.
    /// Fails all pending confirmations on disconnect.
    fn on_status_changed(&self, status: TransportStatus) {
        let previous_status = {
            let mut guard = self.status.lock_or_recover();
            let prev = *guard;
            *guard = status;
            prev
        };

        if status == TransportStatus::Available {
            let queue_len = self.send_queue.lock_or_recover().len();
            *self.reconnect_attempts.lock_or_recover() = 0;

            if queue_len > 0 {
                tracing::info!(
                    pending_messages = queue_len,
                    "Nostr transport available, {} messages pending in queue",
                    queue_len
                );
            }
        } else if previous_status == TransportStatus::Available
            && status != TransportStatus::Available
        {
            self.fail_all_pending();

            let queue_len = self.send_queue.lock_or_recover().len();
            if queue_len > 0 {
                tracing::warn!(
                    pending_messages = queue_len,
                    new_status = ?status,
                    "Nostr transport disconnected with {} messages in queue (will retry)",
                    queue_len
                );
            }
        }
    }

    /// Queues an inbound frame that has **already been unsealed**.
    ///
    /// Unsealing needs the wrapper event's `pubkey` (the sender's single-use
    /// key), which this signature has no way to carry, so the caller must run
    /// [`NostrTransport::unseal_event_payload`] first — the FFI receive entry
    /// does. A still-sealed frame arriving here cannot be recovered, so it is
    /// reported rather than being left to look like ordinary malformed data.
    fn on_data_received(&self, data: Vec<u8>) -> Result<()> {
        if nostr_crypto::is_sealed_payload(&data) {
            tracing::warn!(
                "Sealed Nostr frame reached the unsealed receive path; dropping. \
                 Callers must unseal via unseal_event_payload() first."
            );
            return Ok(());
        }
        crate::common::on_data_received(&self.receive_queue, data)
    }

    /// Like [`Transport::on_data_received`], but attaches a
    /// transport-verified `peer_id` to the deserialized message.
    fn on_data_received_from(&self, data: Vec<u8>, peer_id: String) -> Result<()> {
        crate::common::on_data_received_from(&self.receive_queue, data, peer_id)
    }

    /// **Refused.** This transport has no unsealed whole-message drain.
    ///
    /// The signature can only return a bare serialized `Message` — the entire
    /// protocol envelope, both endpoints included — with no gift wrap, no
    /// signature and no event around it, so publishing the result would put
    /// exactly the cleartext this transport exists to avoid in front of every
    /// relay. There is nowhere to put a signed, sealed event in a
    /// `(String, Vec<u8>)`, and a Nostr frame without its event envelope is
    /// not deliverable anyway.
    ///
    /// It used to return that cleartext, with this comment warning callers off
    /// it. A doc comment is not a control: this method sits on the generic
    /// [`Transport`] trait, which the engine hands out as `dyn Transport` from
    /// `TransportManager::get_transport`, so reaching the unsealed bytes took
    /// no downcast and no unsafe — and the leak ran regardless of
    /// `nostr_sealing_enabled`, since this path never consulted it. The
    /// refusal is the enforcement.
    ///
    /// Poll [`NostrTransport::get_next_signed_event`] instead: it produces a
    /// complete signed, sealed `["EVENT", …]` message ready for the wire. That
    /// is what the bundled bridges and the UniFFI `nostr_get_next_message`
    /// entry call.
    ///
    /// **Nothing is consumed here.** The refusal returns before the send queue
    /// or `pending_confirmation` are read, so a caller that reaches this by
    /// mistake cannot also steal a frame out from under the sealed drain — it
    /// stays queued, and the next `get_next_signed_event` serves it.
    fn get_next_message(&self) -> Result<Option<(String, Vec<u8>)>> {
        Err(crate::Error::ConfigurationError(
            "Nostr has no unsealed message drain: get_next_message() would \
             return the bare protocol envelope in cleartext. Poll \
             NostrTransport::get_next_signed_event() instead, which returns a \
             signed, sealed [\"EVENT\", …] relay message. The queued frame is \
             untouched."
                .to_string(),
        ))
    }

    /// Sets the callback invoked when outgoing messages are queued.
    fn set_on_messages_available(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.on_messages_available.lock_or_recover() = Some(callback);
    }

    /// Platform confirms a message was sent successfully.
    fn confirm_sent(&self, message_id: &str) {
        let removed = self
            .pending_confirmation
            .lock_or_recover()
            .remove(message_id);

        // A publication is not a message and never moves the delivery metrics
        // — see [`publication_slot_id`] for why counting it would misreport
        // this transport's reliability to DORS.
        if removed.is_some() && publication_slot_id(message_id).is_none() {
            let mut metrics = self.metrics.lock_or_recover();
            metrics.success_count = metrics.success_count.saturating_add(1);
            recalculate_delivery_ratios(&mut metrics);
        }
    }

    /// Platform reports a send failure.
    fn report_send_failure(&self, message_id: &str) {
        let removed = self
            .pending_confirmation
            .lock_or_recover()
            .remove(message_id);

        // Same rule as `confirm_sent`: a publication's outcome is reported to
        // the engine below, never to the delivery metrics DORS scores on.
        if removed.is_some() && publication_slot_id(message_id).is_none() {
            let mut metrics = self.metrics.lock_or_recover();
            metrics.failure_count = metrics.failure_count.saturating_add(1);
            recalculate_delivery_ratios(&mut metrics);
        }

        // Unconditional, unlike the metrics above: a report that races the
        // confirmation timeout finds no pending entry, and missing a real
        // failure strands the slot until restart while a redundant republish
        // costs one idempotent relay write.
        if let Some(slot_id) = publication_slot_id(message_id) {
            self.mark_publications_failed(vec![slot_id]);
        }
    }
}

/// Builder for [`NostrTransport`].
pub struct NostrTransportBuilder {
    device_id: String,
    config: NostrConfig,
}

impl NostrTransportBuilder {
    /// Creates a new builder.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            config: NostrConfig::default(),
        }
    }

    /// Sets the relay URLs.
    pub fn relay_urls(mut self, urls: Vec<String>) -> Self {
        self.config.relay_urls = urls;
        self
    }

    /// Adds a single relay URL.
    pub fn add_relay_url(mut self, url: impl Into<String>) -> Self {
        self.config.relay_urls.push(url.into());
        self
    }

    /// Sets the connection timeout.
    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.config.connection_timeout = timeout;
        self
    }

    /// Sets whether to auto-reconnect.
    pub fn auto_reconnect(mut self, auto_reconnect: bool) -> Self {
        self.config.auto_reconnect = auto_reconnect;
        self
    }

    /// Sets the reconnection delay.
    pub fn reconnect_delay(mut self, delay: Duration) -> Self {
        self.config.reconnect_delay = delay;
        self
    }

    /// Sets the maximum reconnection attempts.
    pub fn max_reconnect_attempts(mut self, max: u32) -> Self {
        self.config.max_reconnect_attempts = max;
        self
    }

    /// Builds the transport.
    pub fn build(self) -> Result<NostrTransport> {
        NostrTransport::with_config(self.device_id, self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_MAX_MESSAGE_SIZE;
    use offline_protocol_core::{AppId, UserId};

    /// The address a test label stands for.
    ///
    /// Every id this transport touches — its own, a recipient's, a peer being
    /// resolved — is a derived address now, and construction refuses anything
    /// else, so a fixture cannot simply *be* `"alice"`. It can hold the same
    /// address every time it says `"alice"`, which is what this gives.
    ///
    /// Seeded from the label rather than randomly so a failure reproduces, and
    /// built straight from the hash rather than through a real keypair because
    /// nothing here verifies the address against a key: the transport only
    /// hashes it. The core crate's `test_identity::id` is the version that does
    /// come from a key, for fixtures that also have to satisfy MLS.
    fn addr(label: &str) -> String {
        addr_typed(label).to_string()
    }

    /// `addr` as the parsed type, for the derivations that take an [`Address`].
    fn addr_typed(label: &str) -> Address {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(label.as_bytes());
        let mut hash = [0u8; Address::HASH_LEN];
        hash.copy_from_slice(&digest[..Address::HASH_LEN]);
        Address::from_hash_bytes(hash)
    }

    fn create_test_message() -> Message {
        Message::new(
            UserId::new(addr("alice")).unwrap(),
            UserId::new(addr("bob")).unwrap(),
            AppId::new("test").unwrap(),
            "Test message",
        )
    }

    /// An id that is not an address is refused, not hashed into a tag.
    ///
    /// The two failures this prevents are both silent — a guessable preimage
    /// is a disclosure nothing reports, a merely-wrong one addresses this
    /// device where nobody writes — so construction is the only place either
    /// can be made to surface.
    #[test]
    fn test_construction_refuses_an_id_that_is_not_an_address() {
        for bad in [
            "device1",
            "alice",
            "",
            // Right shape, wrong checksum: the check must be the real address
            // parse, not a prefix or length test.
            "off1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
            // Canonical address, uppercased. Accepting both spellings would
            // give one identity two routing tags.
            &addr("alice").to_uppercase(),
        ] {
            assert!(
                NostrTransport::new(bad).is_err(),
                "construction must refuse a non-address id: {bad:?}"
            );
        }

        assert!(
            NostrTransport::new(addr("alice")).is_ok(),
            "a derived address is exactly what construction accepts"
        );
    }

    /// A recipient that is not an address is refused at `send`, not hashed
    /// into a public routing tag.
    ///
    /// The constructor already refuses a non-address for *our* id; this is the
    /// same rule on the other side of the frame, and it has the same two silent
    /// failure modes. It is asserted at `send` specifically because that is the
    /// only queue writer: past it, the drain would treat the failure as
    /// retriable and republish the label `MAX_SIGN_RETRIES` times.
    #[test]
    fn test_send_refuses_a_recipient_that_is_not_an_address() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = Message::new(
            UserId::new(addr("alice")).unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "for a username",
        );

        assert!(
            transport.send(&msg).is_err(),
            "a username-shaped recipient must not reach the send queue"
        );
        assert!(
            !transport.has_pending_sends(),
            "the refused message must not be queued"
        );
        assert!(
            transport.get_next_signed_event().unwrap().is_none(),
            "nothing may be published for a refused recipient"
        );

        // The address form of the same peer is what does go out.
        let ok = Message::new(
            UserId::new(addr("alice")).unwrap(),
            UserId::new(addr("bob")).unwrap(),
            AppId::new("test").unwrap(),
            "for an address",
        );
        assert!(transport.send(&ok).is_ok());
    }

    /// The resolution queue applies the same rule at its only gate, so
    /// `next_query` cannot pop an id whose tag derivation would fail after the
    /// entry was already consumed.
    #[test]
    fn test_resolution_refuses_a_peer_id_that_is_not_an_address() {
        let transport = NostrTransport::new(addr("alice")).unwrap();

        for bad in ["bob", "", "off1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq"] {
            assert!(
                !transport.request_peer_key_packages(bad),
                "a non-address peer id must not be queued for resolution: {bad:?}"
            );
        }
        assert!(
            transport.next_query().unwrap().is_none(),
            "no query may be issued for a refused peer id"
        );

        assert!(
            transport.request_peer_key_packages(&addr("bob")),
            "an address is exactly what the resolution queue accepts"
        );
    }

    #[test]
    fn test_nostr_transport_creation() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        assert_eq!(transport.device_id(), addr("device1"));
        assert_eq!(transport.transport_type(), TransportType::Nostr);
        assert_eq!(transport.status(), TransportStatus::Unavailable);
    }

    #[test]
    fn test_builder() {
        let transport = NostrTransportBuilder::new(addr("device1"))
            .relay_urls(vec!["wss://relay.example.com".to_string()])
            .add_relay_url("wss://relay2.example.com")
            .connection_timeout(Duration::from_secs(60))
            .auto_reconnect(false)
            .reconnect_delay(Duration::from_secs(10))
            .max_reconnect_attempts(5)
            .build()
            .unwrap();
        assert_eq!(transport.config().relay_urls.len(), 2);
        assert_eq!(
            transport.config().connection_timeout,
            Duration::from_secs(60)
        );
        assert!(!transport.config().auto_reconnect);
        assert_eq!(transport.config().reconnect_delay, Duration::from_secs(10));
        assert_eq!(transport.config().max_reconnect_attempts, 5);
    }

    // `test_send_receive` lived here. Its two assertions moved to the sealed
    // drain rather than being duplicated onto it: the queue-to-pending
    // transition is `test_get_next_signed_event`, and the content round trip
    // is `test_sealed_frame_round_trips_through_the_recipients_transport`,
    // which asserts the sender as well as the id.

    #[test]
    fn test_send_when_unavailable_fails() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        let msg = create_test_message();
        assert!(transport.send(&msg).is_err());
    }

    #[test]
    fn test_receive_when_empty_returns_none() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        assert!(transport.receive().unwrap().is_none());
    }

    #[test]
    fn test_confirmation_loop() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let signed = transport.get_next_signed_event().unwrap().unwrap();
        transport.confirm_sent(&signed.message_id);

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 1);
        assert_eq!(metrics.failure_count, 0);
    }

    #[test]
    fn test_send_failure_reporting() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let signed = transport.get_next_signed_event().unwrap().unwrap();
        transport.report_send_failure(&signed.message_id);

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 0);
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_fail_all_pending_on_disconnect() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let _ = transport.get_next_signed_event().unwrap();

        transport.on_status_changed(TransportStatus::Disconnected);

        let metrics = transport.metrics();
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_stop_fails_pending() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let _ = transport.get_next_signed_event().unwrap();

        transport.stop().unwrap();

        let metrics = transport.metrics();
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_serialization() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        let msg = create_test_message();
        let data = transport.serialize_message(&msg).unwrap();
        let deserialized = transport.deserialize_message(&data).unwrap();
        assert_eq!(deserialized.id, msg.id);
    }

    #[test]
    fn test_reconnect_logic() {
        let transport = NostrTransportBuilder::new(addr("device1"))
            .max_reconnect_attempts(3)
            .build()
            .unwrap();

        assert!(transport.should_reconnect());
        transport.increment_reconnect_attempts();
        transport.increment_reconnect_attempts();
        assert!(transport.should_reconnect());
        transport.increment_reconnect_attempts();
        assert!(!transport.should_reconnect());
    }

    #[test]
    fn test_on_data_received_invalid_json_drops_ok() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        let result = transport.on_data_received(b"not json".to_vec());
        assert!(result.is_ok());
        assert!(transport.receive().unwrap().is_none());
    }

    #[test]
    fn test_on_data_received_rejects_oversized_payload() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        let oversized = vec![0u8; DEFAULT_MAX_MESSAGE_SIZE + 1];
        let result = transport.on_data_received(oversized);
        assert!(result.is_err());
    }

    #[test]
    fn test_on_messages_available_callback() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let called = Arc::new(Mutex::new(false));
        let called_clone = Arc::clone(&called);
        transport.set_on_messages_available(Arc::new(move || {
            *called_clone.lock().unwrap() = true;
        }));

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        assert!(*called.lock().unwrap());
    }

    #[test]
    fn test_messages_available_callback_reentrant_send() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let transport = Arc::new(NostrTransport::new(addr("device1")).unwrap());
        transport.on_status_changed(TransportStatus::Available);

        let reentered = Arc::new(AtomicBool::new(false));
        let reentered_clone = Arc::clone(&reentered);
        let transport_clone = Arc::clone(&transport);
        transport.set_on_messages_available(Arc::new(move || {
            // Re-enters send() from inside the callback. If send() held the
            // callback mutex across this call, the inner send would
            // self-deadlock re-locking it.
            if !reentered_clone.swap(true, Ordering::SeqCst) {
                transport_clone.send(&create_test_message()).unwrap();
            }
        }));

        transport.send(&create_test_message()).unwrap();

        assert!(reentered.load(Ordering::SeqCst));
        assert_eq!(transport.send_queue.lock().unwrap().len(), 2);
    }

    #[test]
    fn test_update_metrics_preserves_confirmation_counts() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let signed = transport.get_next_signed_event().unwrap().unwrap();
        transport.confirm_sent(&signed.message_id);

        let mut new_metrics = TransportMetrics::default();
        new_metrics.rssi = Some(-70);
        transport.update_metrics(new_metrics);

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 1);
        assert_eq!(metrics.rssi, Some(-70));
    }

    #[test]
    fn test_platform_handle() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        assert!(transport.platform_handle().is_none());
        transport.set_platform_handle(42);
        assert_eq!(transport.platform_handle(), Some(42));
    }

    #[test]
    fn test_drain_expired_pending_expires_old_entries() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        // Insert a pending entry that is already past the timeout by backdating it.
        let timeout_secs = NOSTR_PENDING_CONFIRMATION_TIMEOUT_SECS;
        let expired_at = Instant::now() - Duration::from_secs(timeout_secs + 1);
        transport
            .pending_confirmation
            .lock()
            .unwrap()
            .insert("expired-msg".to_string(), expired_at);

        // Insert a recent pending entry that should survive.
        transport
            .pending_confirmation
            .lock()
            .unwrap()
            .insert("recent-msg".to_string(), Instant::now());

        transport.drain_expired_pending();

        let pending = transport.pending_confirmation.lock().unwrap();
        assert!(
            !pending.contains_key("expired-msg"),
            "Expired entry should have been drained"
        );
        assert!(
            pending.contains_key("recent-msg"),
            "Recent entry should be retained"
        );
        drop(pending);

        let metrics = transport.metrics();
        assert_eq!(
            metrics.failure_count, 1,
            "Expired entry should be counted as a failure"
        );
    }

    #[test]
    fn test_has_pending_sends() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        assert!(!transport.has_pending_sends());

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        assert!(transport.has_pending_sends());
    }

    #[test]
    fn test_pending_confirmation_count() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        assert_eq!(transport.pending_confirmation_count(), 0);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let _ = transport.get_next_signed_event().unwrap();

        assert_eq!(transport.pending_confirmation_count(), 1);
    }

    #[test]
    fn test_default_config() {
        let config = NostrConfig::default();
        assert!(config.relay_urls.is_empty());
        assert_eq!(
            config.connection_timeout,
            Duration::from_secs(NOSTR_CONNECTION_TIMEOUT_SECS)
        );
        assert!(config.auto_reconnect);
        assert_eq!(config.reconnect_delay, Duration::from_secs(5));
        assert_eq!(config.max_reconnect_attempts, 0);
    }

    #[test]
    fn test_get_next_signed_event() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let signed = transport.get_next_signed_event().unwrap().unwrap();
        assert!(!signed.message_id.is_empty());
        assert_eq!(signed.event_id.len(), 64); // 32-byte SHA-256 hex
        assert!(signed.event_json.starts_with("[\"EVENT\",{"));
        assert!(signed.event_json.ends_with("}]"));
        assert!(signed.event_json.contains(&signed.event_id));

        // Message was dequeued and moved to pending confirmation
        assert!(!transport.has_pending_sends());
        assert_eq!(transport.pending_confirmation_count(), 1);

        // No more messages
        assert!(transport.get_next_signed_event().unwrap().is_none());
    }

    /// The generic whole-message poll must refuse rather than hand back the
    /// unsealed envelope, and must cost the sealed drain nothing.
    ///
    /// Dispatched through `&dyn Transport` on purpose: that is the shape the
    /// leak had. `TransportManager::get_transport` hands out an
    /// `Arc<dyn Transport>`, so this was reachable with no downcast and no
    /// unsafe, and a concrete-typed call here would not pin the vtable entry
    /// that actually mattered.
    #[test]
    fn test_generic_transport_poll_refuses_rather_than_leaking_cleartext() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let generic: &dyn Transport = &transport;
        let err = generic.get_next_message().unwrap_err();
        assert!(
            matches!(err, crate::Error::ConfigurationError(_)),
            "expected a refusal, got {err:?}"
        );

        // The refusal returns before the queue or the pending map are touched,
        // so a caller that lands here by mistake cannot also strand the frame
        // it failed to get: nothing is dequeued, and nothing is left awaiting a
        // confirmation that no bridge will ever send.
        assert!(
            transport.has_pending_sends(),
            "the refused frame must stay queued"
        );
        assert_eq!(
            transport.pending_confirmation_count(),
            0,
            "a refusal must not enter the confirmation loop"
        );

        // ...and the sealed drain still serves that same frame.
        let signed = transport.get_next_signed_event().unwrap().unwrap();
        assert_eq!(signed.message_id, msg.id.to_string());
        assert!(signed.event_json.starts_with("[\"EVENT\",{"));
    }

    #[test]
    fn test_get_next_signed_event_confirm_flow() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let signed = transport.get_next_signed_event().unwrap().unwrap();
        transport.confirm_sent(&signed.message_id);

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 1);
        assert_eq!(metrics.failure_count, 0);
        assert_eq!(transport.pending_confirmation_count(), 0);
    }

    #[test]
    fn test_subscription_filters_on_routing_tag_not_signing_key() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        let expected_tag = nostr_crypto::routing_tag_for_address(&addr_typed("device1")).unwrap();

        assert_eq!(transport.routing_tag(), expected_tag);
        // The signing key is random per install and must never leak into the
        // subscription filter, which peers derive from our device_id.
        assert_ne!(transport.public_key_hex(), expected_tag);

        let filter = transport.create_subscription("sub1").unwrap();
        assert!(filter.contains(&expected_tag));
        assert!(!filter.contains(&transport.public_key_hex()));
    }

    #[test]
    fn test_install_signing_secret_gives_stable_identity() {
        let transport_a = NostrTransport::new(addr("device1")).unwrap();
        let transport_b = NostrTransport::new(addr("device1")).unwrap();

        // Ephemeral keys are random: two instances differ.
        assert_ne!(transport_a.public_key_hex(), transport_b.public_key_hex());

        // Installing the same persisted secret (a simulated restart)
        // converges both on the same identity.
        let secret = [42u8; 32];
        transport_a.install_signing_secret(&secret).unwrap();
        transport_b.install_signing_secret(&secret).unwrap();
        assert_eq!(transport_a.public_key_hex(), transport_b.public_key_hex());

        // Addressing is untouched by the key swap.
        assert_eq!(
            transport_a.routing_tag(),
            nostr_crypto::routing_tag_for_address(&addr_typed("device1")).unwrap()
        );
    }

    #[test]
    fn test_oversized_event_is_dropped_permanently_and_does_not_block_queue() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let oversized = Message::new(
            UserId::new(addr("alice")).unwrap(),
            UserId::new(addr("bob")).unwrap(),
            AppId::new("test").unwrap(),
            "x".repeat(NOSTR_MAX_PAYLOAD_SIZE),
        );
        transport.send(&oversized).unwrap();
        let queued_behind = create_test_message();
        transport.send(&queued_behind).unwrap();

        let err = transport.get_next_signed_event().unwrap_err();
        assert!(
            matches!(
                err,
                crate::Error::MessageTooLarge(actual, limit)
                    if actual > NOSTR_MAX_PAYLOAD_SIZE && limit == NOSTR_MAX_PAYLOAD_SIZE
            ),
            "expected MessageTooLarge, got {err:?}"
        );

        // Dropped on the first attempt rather than re-queued at the front for
        // MAX_SIGN_RETRIES rounds, so the message behind it is served now.
        let signed = transport.get_next_signed_event().unwrap().unwrap();
        assert_eq!(signed.message_id, queued_behind.id.to_string());

        assert!(
            transport.sign_retry_counts.lock().unwrap().is_empty(),
            "an unshrinkable message must not accumulate retry state"
        );
        assert_eq!(
            transport.metrics().failure_count,
            1,
            "the drop must reach metrics, or DORS never learns Nostr failed"
        );
    }

    #[test]
    fn test_size_cap_measures_the_relay_message_not_the_inner_payload() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        // The serialized message fits under the cap; base64 inflation (4/3)
        // pushes the event a relay actually sees over it. Capping the inner
        // payload instead would let this onto the wire to be rejected there.
        let msg = Message::new(
            UserId::new(addr("alice")).unwrap(),
            UserId::new(addr("bob")).unwrap(),
            AppId::new("test").unwrap(),
            "x".repeat(50_000),
        );
        assert!(
            transport.serialize_message(&msg).unwrap().len() < NOSTR_MAX_PAYLOAD_SIZE,
            "test premise: the inner payload is under the cap"
        );

        transport.send(&msg).unwrap();
        assert!(matches!(
            transport.get_next_signed_event().unwrap_err(),
            crate::Error::MessageTooLarge(_, _)
        ));
    }

    #[test]
    fn test_default_size_media_chunk_exceeds_the_relay_cap() {
        // Ground truth for why the cap matters, and why it is not a
        // regression: DORS gives Nostr a media_bonus of 30.0, so media routes
        // here — but `Message::binary_content` has no base64 serde adapter, so
        // a chunk becomes a JSON array of decimal numbers (~3.6x) before the
        // event's own base64 (~1.33x) is applied on top. At the engine's
        // 32 KiB DEFAULT_CHUNK_SIZE that is ~156 KB on the wire, well past
        // both this cap and the 64-128 KB relays typically accept. Such events
        // were never deliverable; they now fail here instead of at the relay.
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let mut chunk_message = create_test_message();
        chunk_message.content = String::new();
        chunk_message.binary_content = Some(vec![0xABu8; 32 * 1024]);

        transport.send(&chunk_message).unwrap();
        assert!(matches!(
            transport.get_next_signed_event().unwrap_err(),
            crate::Error::MessageTooLarge(_, _)
        ));

        // A BLE-sized 4 KiB chunk still fits, so the cap does not forbid
        // media over Nostr outright — only the default chunking for it.
        let mut small_chunk = create_test_message();
        small_chunk.content = String::new();
        small_chunk.binary_content = Some(vec![0xABu8; 4 * 1024]);

        transport.send(&small_chunk).unwrap();
        assert!(transport.get_next_signed_event().unwrap().is_some());
    }

    /// Parses the REQ filter object out of a `["REQ", id, {…}]` message.
    fn subscription_filter(transport: &NostrTransport) -> serde_json::Value {
        let msg = transport.create_subscription("sub1").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        parsed[2].clone()
    }

    #[test]
    fn test_first_run_since_is_a_recent_window_never_zero() {
        // A zero (or absent) `since` is the unbounded filter the watermark
        // exists to remove, and no-watermark is the *common* case: the bridges
        // subscribe on every relay connect, which can precede the restore.
        let transport = NostrTransport::new(addr("device1")).unwrap();
        assert!(transport.receive_watermark_secs().is_none());

        let since = subscription_filter(&transport)["since"].as_i64().unwrap();
        let now = now_unix_secs();

        assert!(since > 0, "first-run since must not be zero: {since}");
        let backfill = now - since;
        assert!(
            (NOSTR_FIRST_RUN_BACKFILL_SECS..NOSTR_FIRST_RUN_BACKFILL_SECS + 60).contains(&backfill),
            "first-run since should reach back one backfill window, got {backfill}s"
        );
    }

    #[test]
    fn test_since_follows_the_watermark_with_a_jitter_and_skew_overlap() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        let mark = now_unix_secs() - 30;
        assert!(transport.advance_receive_watermark(mark));

        let since = subscription_filter(&transport)["since"].as_i64().unwrap();
        assert_eq!(
            since,
            mark - NOSTR_CREATED_AT_JITTER_SECS - NOSTR_CLOCK_SKEW_MARGIN_SECS,
            "since must sit a full jitter + skew window below the watermark"
        );
    }

    #[test]
    fn test_since_overlap_covers_the_gift_wrap_jitter_window() {
        // The sealed-envelope work jitters `created_at` backwards by up to
        // NOSTR_CREATED_AT_JITTER_SECS, so an event published *now* can carry a
        // timestamp that far in the past. If `since` did not reach back at
        // least that far past the mark, those events would be filtered out by
        // the very relay query meant to fetch them — a silent delivery loss
        // with no error anywhere. This pins the two uses of the constant
        // together.
        let transport = NostrTransport::new(addr("device1")).unwrap();
        let mark = now_unix_secs();
        transport.advance_receive_watermark(mark);

        let since = subscription_filter(&transport)["since"].as_i64().unwrap();
        assert!(
            mark - since >= NOSTR_CREATED_AT_JITTER_SECS,
            "since must reach at least a jitter window below the mark"
        );
    }

    #[test]
    fn test_watermark_only_advances() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        let base = now_unix_secs() - 3600;

        assert!(transport.advance_receive_watermark(base));
        assert_eq!(transport.receive_watermark_secs(), Some(base));

        // Relays stream stored events newest-first, so older ones arrive after
        // newer ones. They must not walk the mark back.
        assert!(!transport.advance_receive_watermark(base - 500));
        assert_eq!(transport.receive_watermark_secs(), Some(base));

        // Equal timestamps are not progress either.
        assert!(!transport.advance_receive_watermark(base));

        assert!(transport.advance_receive_watermark(base + 1));
        assert_eq!(transport.receive_watermark_secs(), Some(base + 1));
    }

    #[test]
    fn test_future_dated_event_cannot_stall_the_subscription() {
        // The routing tag is derived from a public user id, so anyone can
        // publish an event addressed to us, and `created_at` is whatever the
        // publisher wrote. Accepting a far-future value would pin the mark
        // there and every later subscription would ask for events `since` the
        // far future — receiving nothing, permanently, with no error raised.
        let transport = NostrTransport::new(addr("device1")).unwrap();
        let honest = now_unix_secs() - 10;
        transport.advance_receive_watermark(honest);

        let year_3000 = 32_503_680_000i64;
        assert!(
            !transport.advance_receive_watermark(year_3000),
            "a far-future created_at must not advance the watermark"
        );
        assert_eq!(transport.receive_watermark_secs(), Some(honest));

        let since = subscription_filter(&transport)["since"].as_i64().unwrap();
        assert!(
            since < now_unix_secs(),
            "since must stay in the past: {since}"
        );
    }

    #[test]
    fn test_non_positive_created_at_is_ignored() {
        // A missing `created_at` reaches the FFI as 0; a malformed one can be
        // negative. Neither is receive progress.
        let transport = NostrTransport::new(addr("device1")).unwrap();
        assert!(!transport.advance_receive_watermark(0));
        assert!(!transport.advance_receive_watermark(-1));
        assert!(transport.receive_watermark_secs().is_none());
    }

    #[test]
    fn test_stop_preserves_the_receive_watermark() {
        // stop() clears in-flight queues, but receive progress is not undone by
        // a restart — resetting it here would replay a full backfill window on
        // every stop/start cycle.
        let transport = NostrTransport::new(addr("device1")).unwrap();
        let mark = now_unix_secs() - 60;
        transport.advance_receive_watermark(mark);

        transport.stop().unwrap();

        assert_eq!(transport.receive_watermark_secs(), Some(mark));
    }

    #[test]
    fn test_signed_event_uses_recipient_routing_tag_and_own_signing_key() {
        let transport = NostrTransport::new(addr("device1")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);
        // Addressing is a property of the unsealed path too; assert it there so
        // this test keeps covering the legacy shape rather than duplicating the
        // sealed-path tests below.
        transport.set_sealing_enabled(false);

        // create_test_message is addressed to "bob".
        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let signed = transport.get_next_signed_event().unwrap().unwrap();
        let bob_tag = nostr_crypto::routing_tag_for_address(&addr_typed("bob")).unwrap();
        assert!(
            signed.event_json.contains(&bob_tag),
            "event must be addressed to the recipient's routing tag"
        );
        assert!(
            signed
                .event_json
                .contains(&format!("\"pubkey\":\"{}\"", transport.public_key_hex())),
            "unsealed event must be signed by this install's signing key"
        );
    }

    // ===================================================================
    // Sealed envelope
    // ===================================================================

    /// Parses the event object out of a signed `["EVENT", {…}]` message.
    fn event_object(signed: &SignedNostrEvent) -> serde_json::Value {
        let parsed: serde_json::Value = serde_json::from_str(&signed.event_json).unwrap();
        parsed[1].clone()
    }

    #[test]
    fn test_relay_visible_payload_contains_no_username() {
        // THE regression test for the finding this work closes. Before sealing,
        // the event content was base64 of the whole `Message` JSON, so both
        // usernames, the app id, the content type, the metadata map and a
        // millisecond timestamp were readable by every relay, permanently.
        //
        // Asserted over the whole event except the ciphertext: a future change
        // that moved a username into a tag, or reintroduced a stable signing
        // pubkey derived from one, must fail here too. The ciphertext itself is
        // excluded only *after* pinning its sealed shape — it is random bytes,
        // and a case-folded substring assertion over its base64 spells "bob"
        // roughly once per hundred runs, failing CI with no leak present.
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = Message::new(
            UserId::new(addr("alice")).unwrap(),
            UserId::new(addr("bob")).unwrap(),
            AppId::new("fernweh").unwrap(),
            "the quick brown fox",
        );
        transport.send(&msg).unwrap();

        let signed = transport.get_next_signed_event().unwrap().unwrap();

        // The content must be a sealed NIP-44 payload — not the pre-sealing
        // leak shape, base64 of the message JSON.
        let mut event = event_object(&signed);
        let content = base64::engine::general_purpose::STANDARD
            .decode(event["content"].as_str().unwrap())
            .unwrap();
        assert!(
            nostr_crypto::is_sealed_payload(&content),
            "event content is not a sealed payload: {}",
            signed.event_json
        );

        // With the random ciphertext scrubbed, nothing left in the event may
        // name anyone or anything. `content` is the only non-hex field, so
        // nothing else can spell these strings by chance.
        event["content"] = serde_json::Value::String(String::new());
        let wire = serde_json::to_string(&event).unwrap().to_lowercase();

        for leak in ["alice", "bob", "fernweh", "the quick brown fox"] {
            assert!(
                !wire.contains(leak),
                "relay-visible payload leaks {leak:?}: {}",
                signed.event_json
            );
        }

        // The only thing a relay legitimately learns is the recipient's routing
        // tag — an opaque label it cannot invert without already knowing the
        // user id it is looking for.
        let bob_tag = nostr_crypto::routing_tag_for_address(&addr_typed("bob")).unwrap();
        assert!(wire.contains(&bob_tag));
    }

    #[test]
    fn test_sealed_event_is_a_gift_wrap_not_signed_by_our_install_key() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);
        transport.send(&create_test_message()).unwrap();

        let signed = transport.get_next_signed_event().unwrap().unwrap();
        let event = event_object(&signed);

        assert_eq!(
            event["kind"].as_u64(),
            Some(u64::from(nostr_crypto::NOSTR_GIFT_WRAP_KIND))
        );
        assert_ne!(
            event["pubkey"].as_str().unwrap(),
            transport.public_key_hex(),
            "the wrapper must not be signed by our stable install key, or every \
             event we publish is linkable back to this device"
        );
    }

    #[test]
    fn test_sealed_frame_round_trips_through_the_recipients_transport() {
        // End-to-end over the two transports, which is what proves the send and
        // receive halves agree about which key seals what.
        let alice = NostrTransport::new(addr("alice")).unwrap();
        alice.start().unwrap();
        alice.on_status_changed(TransportStatus::Available);

        let bob = NostrTransport::new(addr("bob")).unwrap();
        bob.install_signing_secret(&[88u8; 32]).unwrap();
        // Alice has seen Bob's key package, so this is the steady state.
        alice.set_peer_nostr_pubkey(&addr("bob"), &bob.public_key_hex());

        let msg = create_test_message();
        alice.send(&msg).unwrap();
        let signed = alice.get_next_signed_event().unwrap().unwrap();

        let event = event_object(&signed);
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(event["content"].as_str().unwrap())
            .unwrap();
        let sender_pubkey = event["pubkey"].as_str().unwrap();

        let plaintext = bob.unseal_event_payload(sender_pubkey, &sealed);
        let received = bob.deserialize_message(&plaintext).unwrap();
        assert_eq!(received.id, msg.id);
        assert_eq!(received.sender.as_str(), addr("alice"));
    }

    #[test]
    fn test_bootstrap_frame_round_trips_before_any_key_exchange() {
        // Cold first contact: Alice knows only Bob's user id. The frame is
        // sealed to Bob's publicly computable key, and Bob's receive path finds
        // it on the second attempt.
        let alice = NostrTransport::new(addr("alice")).unwrap();
        alice.start().unwrap();
        alice.on_status_changed(TransportStatus::Available);

        let bob = NostrTransport::new(addr("bob")).unwrap();
        bob.install_signing_secret(&[91u8; 32]).unwrap();
        // Deliberately NOT calling set_peer_nostr_pubkey.

        let msg = create_test_message();
        alice.send(&msg).unwrap();
        let signed = alice.get_next_signed_event().unwrap().unwrap();
        let event = event_object(&signed);

        // Indistinguishable from the steady-state case on the wire: same kind,
        // same tag shape, ephemeral pubkey. A relay cannot filter for "these
        // two are just starting to talk".
        assert_eq!(
            event["kind"].as_u64(),
            Some(u64::from(nostr_crypto::NOSTR_GIFT_WRAP_KIND))
        );

        let sealed = base64::engine::general_purpose::STANDARD
            .decode(event["content"].as_str().unwrap())
            .unwrap();
        let plaintext = bob.unseal_event_payload(event["pubkey"].as_str().unwrap(), &sealed);
        assert_eq!(bob.deserialize_message(&plaintext).unwrap().id, msg.id);
    }

    #[test]
    fn test_unseal_leaves_legacy_unsealed_frames_untouched() {
        // A peer on a build from before sealing publishes the old cleartext
        // envelope. It must pass straight through, or upgrading breaks every
        // conversation with a peer that has not upgraded yet.
        let alice = NostrTransport::new(addr("alice")).unwrap();
        alice.set_sealing_enabled(false);
        alice.start().unwrap();
        alice.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        alice.send(&msg).unwrap();
        let signed = alice.get_next_signed_event().unwrap().unwrap();
        let event = event_object(&signed);
        assert_eq!(
            event["kind"].as_u64(),
            Some(u64::from(nostr_crypto::NOSTR_LEGACY_DM_KIND))
        );

        let legacy = base64::engine::general_purpose::STANDARD
            .decode(event["content"].as_str().unwrap())
            .unwrap();

        let bob = NostrTransport::new(addr("bob")).unwrap();
        let passed_through = bob.unseal_event_payload(event["pubkey"].as_str().unwrap(), &legacy);
        assert_eq!(passed_through.as_ref(), legacy.as_slice());
        assert_eq!(bob.deserialize_message(&passed_through).unwrap().id, msg.id);
    }

    #[test]
    fn test_frame_sealed_to_someone_else_is_returned_unchanged_not_forged() {
        // An event addressed to our routing tag but sealed to a third party
        // must fail closed. Returning the ciphertext unchanged lets the
        // ordinary decoder reject it as one undecodable frame — the same
        // outcome as random junk, which is what it is to us.
        let bob = NostrTransport::new(addr("bob")).unwrap();
        let carol_tag = nostr_crypto::routing_tag_for_address(&addr_typed("carol")).unwrap();

        let event =
            nostr_crypto::NostrEvent::create_gift_wrap(&carol_tag, &carol_tag, b"not for bob")
                .unwrap();
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(&event.content)
            .unwrap();

        let out = bob.unseal_event_payload(&event.pubkey, &sealed);
        assert_eq!(out.as_ref(), sealed.as_slice());
        assert!(bob.deserialize_message(&out).is_err());
    }

    #[test]
    fn test_sealed_frame_on_the_unsealed_receive_path_is_dropped_not_queued() {
        // `on_data_received` cannot unseal — it has no access to the wrapper's
        // pubkey. A bridge wired to it would otherwise enqueue ciphertext as if
        // it were a message.
        let bob = NostrTransport::new(addr("bob")).unwrap();
        let bob_tag = nostr_crypto::routing_tag_for_address(&addr_typed("bob")).unwrap();
        let event =
            nostr_crypto::NostrEvent::create_gift_wrap(&bob_tag, &bob_tag, b"sealed").unwrap();
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(&event.content)
            .unwrap();

        assert!(bob.on_data_received(sealed).is_ok());
        assert!(bob.receive().unwrap().is_none());
    }

    #[test]
    fn test_peer_key_map_is_bounded_and_rejects_malformed_keys() {
        let transport = NostrTransport::new(addr("device1")).unwrap();

        transport.set_peer_nostr_pubkey(&addr("bob"), "not-hex");
        transport.set_peer_nostr_pubkey(&addr("bob"), &"ab".repeat(31)); // 62 chars
        transport.set_peer_nostr_pubkey("", &"ab".repeat(32));
        assert!(transport.peer_nostr_pubkey(&addr("bob")).is_none());

        // Case is normalized: `#p`-style values are lowercase hex by spec, and a
        // mixed-case duplicate must not seal to a different string.
        let key = "AB".repeat(32);
        transport.set_peer_nostr_pubkey(&addr("bob"), &key);
        assert_eq!(
            transport.peer_nostr_pubkey(&addr("bob")),
            Some(key.to_lowercase())
        );

        for i in 0..NOSTR_MAX_TRACKED_PEER_KEYS + 10 {
            transport.set_peer_nostr_pubkey(&format!("peer{i}"), &"cd".repeat(32));
        }
        assert!(
            transport.peer_nostr_pubkeys.read().unwrap().len() <= NOSTR_MAX_TRACKED_PEER_KEYS,
            "wire-keyed map must stay bounded"
        );
    }

    #[test]
    fn test_sealing_can_be_disabled_without_breaking_inbound_unsealing() {
        // The kill switch gates the send side only. A peer that keeps sealing
        // must stay readable, or flipping the flag on one device would sever
        // conversations rather than merely un-protecting our own traffic.
        let alice = NostrTransport::new(addr("alice")).unwrap();
        alice.start().unwrap();
        alice.on_status_changed(TransportStatus::Available);

        let bob = NostrTransport::new(addr("bob")).unwrap();
        bob.set_sealing_enabled(false);
        assert!(!bob.sealing_enabled());

        let msg = create_test_message();
        alice.send(&msg).unwrap();
        let signed = alice.get_next_signed_event().unwrap().unwrap();
        let event = event_object(&signed);
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(event["content"].as_str().unwrap())
            .unwrap();

        let plaintext = bob.unseal_event_payload(event["pubkey"].as_str().unwrap(), &sealed);
        assert_eq!(bob.deserialize_message(&plaintext).unwrap().id, msg.id);
    }

    #[test]
    fn test_sealing_overhead_stays_within_the_relay_cap_for_ordinary_messages() {
        // NIP-44 pads to a power-of-two bucket, so sealing can nearly double a
        // payload before base64 — considerably more than the ~33% a base64
        // layer alone would suggest. Ordinary text messages must still fit
        // comfortably, and the cap must be measured on the sealed event.
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = Message::new(
            UserId::new(addr("alice")).unwrap(),
            UserId::new(addr("bob")).unwrap(),
            AppId::new("test").unwrap(),
            "x".repeat(4096),
        );
        transport.send(&msg).unwrap();

        let signed = transport.get_next_signed_event().unwrap().unwrap();
        assert!(
            signed.event_json.len() <= NOSTR_MAX_PAYLOAD_SIZE,
            "a 4 KiB message must still fit sealed: {} bytes",
            signed.event_json.len()
        );
    }

    // ========================================================================
    // Published key packages (cold contact)
    // ========================================================================

    /// The regression test for the reason published records are sealed at all.
    ///
    /// A key package carries its owner's user id in the payload *and*,
    /// unremovably, in the MLS leaf credential. Published in the clear, a
    /// filter naming only the kind — no tag, no author — would return a
    /// directory of every username on the relay, which is precisely the
    /// preimage the `SHA-256(user_id)` routing tag exists to withhold.
    #[test]
    fn test_published_key_package_record_leaks_no_username() {
        let transport = NostrTransport::new(addr("alice-the-identifiable")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        // Stand in for a key package: what matters is that the user id appears
        // in the plaintext, which it does in both of the real carriers.
        let payload = br#"{"user_id":"alice-the-identifiable","key_package_data":[1,2,3]}"#;
        transport.publish_key_package("slot-a", payload.to_vec());

        let signed = transport.get_next_signed_event().unwrap().unwrap();
        assert!(
            !signed.event_json.contains("alice-the-identifiable"),
            "a published record must not carry the username in the clear: {}",
            signed.event_json
        );
    }

    #[test]
    fn test_published_record_is_addressable_and_opens_with_the_derivable_key() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.install_signing_secret(&[7u8; 32]).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        transport.publish_key_package("slot-a", b"the key package".to_vec());
        let signed = transport.get_next_signed_event().unwrap().unwrap();
        let event = event_object(&signed);

        assert_eq!(
            event["kind"].as_u64(),
            Some(u64::from(nostr_crypto::NOSTR_KEY_PACKAGE_KIND))
        );
        // Signed by the real install key — that is how a fetcher learns which
        // key to seal to — unlike a gift wrap's throwaway key.
        assert_eq!(
            event["pubkey"].as_str().unwrap(),
            transport.public_key_hex(),
            "a fetcher reads the sealing key off this field"
        );
        let tags = event["tags"].as_array().unwrap();
        assert_eq!(tags[0][0].as_str(), Some("d"));
        assert_eq!(tags[0][1].as_str(), Some("slot-a"));
        assert_eq!(tags[1][0].as_str(), Some("p"));
        assert_eq!(tags[1][1].as_str(), Some(transport.routing_tag()));

        // A stranger who knows only the username can open it.
        let stranger_view =
            nostr_crypto::record_seal_keypair_for_address(&addr_typed("alice")).unwrap();
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(event["content"].as_str().unwrap())
            .unwrap();
        let opened = nostr_crypto::open_key_package_publication(
            &stranger_view,
            event["pubkey"].as_str().unwrap(),
            &sealed,
        )
        .unwrap();
        assert_eq!(opened, b"the key package");
    }

    /// A published record's `created_at` must not be jittered backwards the way
    /// a gift wrap's is. Relays keep the newest event per `(kind, pubkey, d)`,
    /// so a backdated republication is silently discarded — leaving a consumed
    /// key package standing as the live record.
    #[test]
    fn test_republication_is_not_backdated_below_the_record_it_replaces() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let now = nostr_crypto::now_unix_secs();
        transport.publish_key_package("slot-a", b"first".to_vec());
        let first = event_object(&transport.get_next_signed_event().unwrap().unwrap());

        let created = first["created_at"].as_i64().unwrap();
        assert!(
            created >= now,
            "publication was backdated by {}s; a relay would drop the replacement",
            now - created
        );
    }

    /// Re-queueing a slot before it drains must collapse to the newer payload:
    /// the older one names a key package the engine has already replaced, and
    /// publishing both would briefly stand a consumed package back up.
    #[test]
    fn test_requeued_slot_collapses_to_the_newer_payload() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        transport.publish_key_package("slot-a", b"stale".to_vec());
        transport.publish_key_package("slot-a", b"fresh".to_vec());

        let signed = transport.get_next_signed_event().unwrap().unwrap();
        let event = event_object(&signed);
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(event["content"].as_str().unwrap())
            .unwrap();
        let key = nostr_crypto::record_seal_keypair_for_address(&addr_typed("alice")).unwrap();
        let opened = nostr_crypto::open_key_package_publication(
            &key,
            event["pubkey"].as_str().unwrap(),
            &sealed,
        )
        .unwrap();
        assert_eq!(opened, b"fresh");

        assert!(
            transport.get_next_signed_event().unwrap().is_none(),
            "the superseded payload must not also publish"
        );
    }

    #[test]
    fn test_publications_drain_ahead_of_messages() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        transport.send(&create_test_message()).unwrap();
        transport.publish_key_package("slot-a", b"kp".to_vec());

        let first = transport.get_next_signed_event().unwrap().unwrap();
        assert!(
            first.message_id.starts_with(NOSTR_PUBLICATION_ID_PREFIX),
            "a refreshed slot must reach the relay before the traffic that \
             depends on it, got {}",
            first.message_id
        );
    }

    /// A publication that leaves the queue but never reaches a relay must be
    /// reported back, or the engine — which marked the slot published when it
    /// queued the record — leaves it looking healthy for the life of the
    /// process while the relays hold nothing.
    #[test]
    fn test_rejected_publication_is_reported_back_for_republication() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        transport.publish_key_package("slot-a", b"kp".to_vec());
        let signed = transport.get_next_signed_event().unwrap().unwrap();
        assert!(
            transport.take_failed_publications().is_empty(),
            "a publication in flight is not yet a failure"
        );

        transport.report_send_failure(&signed.message_id);

        assert_eq!(
            transport.take_failed_publications(),
            vec!["slot-a".to_string()],
            "a relay rejection must surface the slot for republication"
        );
        assert!(
            transport.take_failed_publications().is_empty(),
            "draining must consume the report"
        );
    }

    /// The same, for the disconnect path: dropping the relay fails every
    /// in-flight event, and a publication among them is no exception.
    #[test]
    fn test_disconnect_reports_in_flight_publications_for_republication() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        transport.publish_key_package("slot-a", b"kp".to_vec());
        transport.get_next_signed_event().unwrap().unwrap();

        transport.on_status_changed(TransportStatus::Disconnected);

        assert_eq!(
            transport.take_failed_publications(),
            vec!["slot-a".to_string()],
            "a publication in flight when the relay dropped must be republished"
        );
    }

    /// A message failing must not be mistaken for a publication failing — the
    /// slot set is keyed on the synthetic publication id and nothing else.
    #[test]
    fn test_message_failure_reports_no_publication_slot() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        transport.send(&create_test_message()).unwrap();
        let signed = transport.get_next_signed_event().unwrap().unwrap();
        transport.report_send_failure(&signed.message_id);

        assert!(transport.take_failed_publications().is_empty());
    }

    #[test]
    fn test_cold_contact_disabled_publishes_nothing_and_resolves_nothing() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.set_cold_contact_enabled(false);
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        transport.publish_key_package("slot-a", b"kp".to_vec());
        assert!(!transport.has_pending_publications());
        assert!(!transport.request_peer_key_packages(&addr("bob")));
        assert!(transport.next_query().unwrap().is_none());
    }

    #[test]
    fn test_send_to_an_unknown_peer_queues_a_resolution_and_still_sends() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        // No key for bob: the frame takes the bootstrap leg *and* asks.
        transport.send(&create_test_message()).unwrap();
        let signed = transport.get_next_signed_event().unwrap().unwrap();
        assert!(
            !signed.message_id.starts_with(NOSTR_PUBLICATION_ID_PREFIX),
            "the message must still go out rather than waiting on a round-trip"
        );

        let query = transport.next_query().unwrap().expect("resolution queued");
        let expected_tag = nostr_crypto::routing_tag_for_address(&addr_typed("bob")).unwrap();
        assert!(query.req_json.contains(&expected_tag));
        assert!(query
            .req_json
            .contains(&nostr_crypto::NOSTR_KEY_PACKAGE_KIND.to_string()));
    }

    #[test]
    fn test_resolution_is_rate_limited_per_peer() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        assert!(transport.request_peer_key_packages(&addr("bob")));
        assert!(
            !transport.request_peer_key_packages(&addr("bob")),
            "a peer who has published nothing must not mint a round-trip per frame"
        );
        assert!(transport.request_peer_key_packages(&addr("carol")));
    }

    #[test]
    fn test_query_event_opens_only_for_the_peer_it_was_issued_for() {
        let alice = NostrTransport::new(addr("alice")).unwrap();
        let bob = NostrTransport::new(addr("bob")).unwrap();
        bob.install_signing_secret(&[9u8; 32]).unwrap();
        bob.start().unwrap();
        bob.on_status_changed(TransportStatus::Available);

        bob.publish_key_package("slot-a", b"bob's key package".to_vec());
        let published = bob.get_next_signed_event().unwrap().unwrap();
        let event = event_object(&published);
        let event_json = serde_json::to_string(&event).unwrap();

        alice.request_peer_key_packages(&addr("bob"));
        let query = alice.next_query().unwrap().unwrap();

        let opened = alice
            .open_query_event(&query.query_id, &event_json)
            .unwrap()
            .expect("bob's record opens with bob's derivable key");
        assert_eq!(opened.1, b"bob's key package");

        // The same record delivered under a query for someone else does not
        // open: the query id is what says whose key to try.
        alice.request_peer_key_packages(&addr("carol"));
        let other = alice.next_query().unwrap().unwrap();
        assert!(alice
            .open_query_event(&other.query_id, &event_json)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_query_event_for_an_unknown_or_completed_query_is_dropped() {
        let alice = NostrTransport::new(addr("alice")).unwrap();
        alice.request_peer_key_packages(&addr("bob"));
        let query = alice.next_query().unwrap().unwrap();
        alice.complete_query(&query.query_id);

        assert!(alice
            .open_query_event(&query.query_id, "{\"kind\":30443}")
            .unwrap()
            .is_none());
        assert!(alice
            .open_query_event("never-issued", "{\"kind\":30443}")
            .unwrap()
            .is_none());
    }

    /// A public routing tag means anyone can leave anything there, so a query
    /// returns whatever the relay holds. Junk must be dropped quietly rather
    /// than failing the query.
    #[test]
    fn test_query_event_ignores_wrong_kinds_and_unopenable_content() {
        let alice = NostrTransport::new(addr("alice")).unwrap();
        alice.request_peer_key_packages(&addr("bob"));
        let query = alice.next_query().unwrap().unwrap();

        let wrong_kind = r#"{"kind":1059,"pubkey":"aa","content":"AQID"}"#;
        assert!(alice
            .open_query_event(&query.query_id, wrong_kind)
            .unwrap()
            .is_none());

        let bad_content = r#"{"kind":30443,"pubkey":"not-hex","content":"!!!"}"#;
        assert!(alice
            .open_query_event(&query.query_id, bad_content)
            .unwrap()
            .is_none());

        assert!(alice
            .open_query_event(&query.query_id, "not json at all")
            .unwrap()
            .is_none());
    }

    /// A publication is not a message, and DORS does not get to hear about it.
    ///
    /// The reliability score is `success / (success + failure)` over lifetime
    /// counters that never decay, and an idle install publishes far more than
    /// it sends — so counting publications would score this transport on
    /// something other than its ability to carry messages. A relay that
    /// rejects kind 30443 would drive the ratio toward zero and make DORS
    /// deprioritise Nostr for traffic that delivers perfectly well.
    #[test]
    fn test_publication_failures_stay_out_of_the_delivery_metrics() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        // One real message, delivered.
        transport.send(&create_test_message()).unwrap();
        let msg = transport.get_next_signed_event().unwrap().unwrap();
        transport.confirm_sent(&msg.message_id);

        // A full slot set the relay rejects outright.
        for i in 0..crate::constants::NOSTR_KEY_PACKAGE_SLOTS {
            transport.publish_key_package(&format!("slot-{}", i), b"kp".to_vec());
        }
        for _ in 0..crate::constants::NOSTR_KEY_PACKAGE_SLOTS {
            let published = transport.get_next_signed_event().unwrap().unwrap();
            assert!(published
                .message_id
                .starts_with(NOSTR_PUBLICATION_ID_PREFIX));
            transport.report_send_failure(&published.message_id);
        }

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 1);
        assert_eq!(
            metrics.failure_count, 0,
            "publication rejections were counted as message delivery failures"
        );
        assert_eq!(
            metrics.delivery_ratio,
            Some(1.0),
            "the only message sent was delivered; the ratio DORS reads must say so"
        );

        // ...and the slots are still reported back for republication.
        assert_eq!(
            transport.take_failed_publications().len(),
            crate::constants::NOSTR_KEY_PACKAGE_SLOTS,
            "keeping publications out of the metrics must not lose the reports"
        );
    }

    /// The same rule in the other direction: a successful publication must not
    /// inflate the ratio and mask real message failures.
    #[test]
    fn test_publication_successes_do_not_mask_message_failures() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        transport.send(&create_test_message()).unwrap();
        let msg = transport.get_next_signed_event().unwrap().unwrap();
        transport.report_send_failure(&msg.message_id);

        for i in 0..crate::constants::NOSTR_KEY_PACKAGE_SLOTS {
            transport.publish_key_package(&format!("slot-{}", i), b"kp".to_vec());
        }
        for _ in 0..crate::constants::NOSTR_KEY_PACKAGE_SLOTS {
            let published = transport.get_next_signed_event().unwrap().unwrap();
            transport.confirm_sent(&published.message_id);
        }

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 0);
        assert_eq!(metrics.failure_count, 1);
        assert_eq!(
            metrics.delivery_ratio,
            Some(0.0),
            "publications that reached a relay masked a message that did not"
        );
    }

    /// A resolution query is broadcast to every connected relay, so the same
    /// record comes back once per relay. Opening it more than once re-runs the
    /// key-package handler — two durable secure-storage writes a time — to
    /// import a package we already hold.
    #[test]
    fn test_duplicate_records_from_several_relays_open_once() {
        let alice = NostrTransport::new(addr("alice")).unwrap();
        let bob = NostrTransport::new(addr("bob")).unwrap();
        bob.install_signing_secret(&[9u8; 32]).unwrap();
        bob.start().unwrap();
        bob.on_status_changed(TransportStatus::Available);

        bob.publish_key_package("slot-a", b"bob's key package".to_vec());
        let published = bob.get_next_signed_event().unwrap().unwrap();
        let event_json = serde_json::to_string(&event_object(&published)).unwrap();

        alice.request_peer_key_packages(&addr("bob"));
        let query = alice.next_query().unwrap().unwrap();

        assert!(
            alice
                .open_query_event(&query.query_id, &event_json)
                .unwrap()
                .is_some(),
            "the first relay's copy must open"
        );
        for _ in 0..4 {
            assert!(
                alice
                    .open_query_event(&query.query_id, &event_json)
                    .unwrap()
                    .is_none(),
                "the same record must be taken once per query, not once per relay"
            );
        }
    }

    /// A relay is free to ignore the REQ's `limit` and stream whatever it
    /// likes, and every record that opens costs durable writes behind this
    /// call — so the ceiling has to be ours, not the relay's.
    #[test]
    fn test_a_query_stops_accepting_events_at_its_ceiling() {
        let alice = NostrTransport::new(addr("alice")).unwrap();
        let bob = NostrTransport::new(addr("bob")).unwrap();
        bob.install_signing_secret(&[9u8; 32]).unwrap();
        bob.start().unwrap();
        bob.on_status_changed(TransportStatus::Available);
        bob.publish_key_package("slot-a", b"bob's key package".to_vec());
        let published = bob.get_next_signed_event().unwrap().unwrap();
        let genuine = serde_json::to_string(&event_object(&published)).unwrap();

        alice.request_peer_key_packages(&addr("bob"));
        let query = alice.next_query().unwrap().unwrap();

        // Distinct ids, so it is the ceiling that stops this and not the dedup.
        for i in 0..MAX_QUERY_EVENTS {
            let junk = format!(
                r#"{{"id":"{:064x}","kind":30443,"pubkey":"aa","content":"AQID"}}"#,
                i
            );
            assert!(alice
                .open_query_event(&query.query_id, &junk)
                .unwrap()
                .is_none());
        }

        assert!(
            alice
                .open_query_event(&query.query_id, &genuine)
                .unwrap()
                .is_none(),
            "the query kept accepting events past its ceiling"
        );

        // The record itself is fine, and the ceiling is per query: a fresh one
        // opens it.
        let alice2 = NostrTransport::new(addr("alice")).unwrap();
        alice2.request_peer_key_packages(&addr("bob"));
        let fresh = alice2.next_query().unwrap().unwrap();
        assert!(alice2
            .open_query_event(&fresh.query_id, &genuine)
            .unwrap()
            .is_some());
    }

    /// The same malformed key, by the route that was already shipping.
    ///
    /// `unseal_event_payload` takes the event's `pubkey` field verbatim off a
    /// public relay and hands it to the NIP-44 derive, which used to reach a
    /// fixed-size decoder that aborts the process on a wrong-length key. One
    /// hostile event was enough; it must be an unopenable frame instead.
    #[test]
    fn test_malformed_event_pubkey_does_not_abort_the_receive_path() {
        let transport = NostrTransport::new(addr("alice")).unwrap();
        let sealed = vec![0x02u8; 160];

        for pubkey in ["", "aa", "abcd", &"ab".repeat(64)] {
            let out = transport.unseal_event_payload(pubkey, &sealed);
            assert_eq!(
                &*out,
                &sealed[..],
                "a malformed pubkey must leave the frame untouched, not abort"
            );
        }
    }

    /// A request the queue refuses for capacity was never looked up, so it must
    /// not burn the retry interval — otherwise the overflow policy's promise
    /// that a dropped peer "is retried on the next send to them" is false, and
    /// the peer instead waits out `RESOLUTION_RETRY_INTERVAL`.
    #[test]
    fn test_a_resolution_refused_at_capacity_does_not_burn_the_rate_limit() {
        let transport = NostrTransport::new(addr("alice")).unwrap();

        for i in 0..MAX_PENDING_RESOLUTIONS {
            assert!(transport.request_peer_key_packages(&addr(&format!("peer-{}", i))));
        }

        assert!(
            !transport.request_peer_key_packages(&addr("bob")),
            "the queue is full, so this request is refused"
        );

        // Draining one makes room; bob must be admitted straight away.
        transport.next_query().unwrap().unwrap();
        assert!(
            transport.request_peer_key_packages(&addr("bob")),
            "a request dropped at capacity burned the retry interval"
        );
    }
}
