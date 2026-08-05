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
//! Addressing uses public routing tags derived from device IDs
//! ([`nostr_crypto::routing_tag_for_device_id`]); event signing uses a
//! per-install secret key that starts out ephemeral and is upgraded to a
//! persisted identity via [`NostrTransport::install_signing_secret`].

use crate::constants::{
    NOSTR_CLOCK_SKEW_MARGIN_SECS, NOSTR_CONNECTION_TIMEOUT_SECS, NOSTR_CREATED_AT_JITTER_SECS,
    NOSTR_FIRST_RUN_BACKFILL_SECS, NOSTR_FUTURE_DATED_TOLERANCE_SECS, NOSTR_MAX_PAYLOAD_SIZE,
    NOSTR_MAX_TRACKED_PEER_KEYS, NOSTR_PENDING_CONFIRMATION_TIMEOUT_SECS,
};
use crate::nostr_crypto::{self, now_unix_secs, NostrKeypair};
use crate::{Result, SharedCallback, Transport, TransportMetrics, TransportStatus, TransportType};
use base64::Engine;
use offline_protocol_core::{Message, MutexExt, RwLockExt};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::common::recalculate_delivery_ratios;

/// Maximum number of signing attempts before a message is permanently failed.
const MAX_SIGN_RETRIES: u8 = 3;

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
/// `keypair`, `receive_watermark`, `peer_nostr_pubkeys` and `sealing_enabled`
/// are leaf locks: they are only ever held in a narrow scope with no other lock
/// acquisition inside. In particular the sealing path releases each before
/// calling into the crypto layer, so a slow ECDH never blocks the send queue.
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
    /// The publicly computable keypair whose public half is `routing_tag`.
    ///
    /// Held only to unseal (and seal) bootstrap-leg frames — see
    /// [`NostrKeypair::derivable_for_device_id`], which documents why this is
    /// not a secret and must never authenticate anything.
    derivable_keypair: NostrKeypair,
    /// Peer user ID → that peer's real per-install Nostr public key, learned
    /// from the `nostr_pubkey` field of their signed key package. Populated by
    /// the engine via [`Self::set_peer_nostr_pubkey`]; a peer absent here takes
    /// the bootstrap path.
    peer_nostr_pubkeys: RwLock<HashMap<String, String>>,
    /// Whether outgoing frames are sealed into gift wraps. Mirrors
    /// `TransportConfig::nostr_sealing_enabled`; the receive path always
    /// accepts both forms regardless.
    sealing_enabled: Mutex<bool>,
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
    /// The signing keypair starts out ephemeral (random for this process);
    /// call [`Self::install_signing_secret`] once persisted storage is
    /// available to give the install a stable Nostr identity.
    pub fn with_config(device_id: impl Into<String>, config: NostrConfig) -> Result<Self> {
        let device_id = device_id.into();
        let routing_tag = nostr_crypto::routing_tag_for_device_id(&device_id)?;
        let derivable_keypair = NostrKeypair::derivable_for_device_id(&device_id)?;
        let keypair = RwLock::new(NostrKeypair::generate_ephemeral()?);
        Ok(Self {
            device_id,
            keypair,
            routing_tag,
            receive_watermark: Mutex::new(None),
            derivable_keypair,
            peer_nostr_pubkeys: RwLock::new(HashMap::new()),
            sealing_enabled: Mutex::new(true),
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

    /// Records a peer's real per-install Nostr public key, learned from the
    /// `nostr_pubkey` field of their key package.
    ///
    /// Until this is known, frames to that peer are sealed to their publicly
    /// computable key instead (bootstrap leg). Once it is known, every
    /// subsequent frame is sealed to a key only that install holds.
    ///
    /// The key arrives inside the Ed25519-signed, TOFU-pinned key package
    /// payload, so it is bound to the claimed sender — unlike the plaintext
    /// capability lists that ride alongside it. A wrong value here is a
    /// delivery denial (the peer cannot unseal), never a disclosure: the
    /// plaintext is sealed *to* it, so an attacker substituting their own key
    /// would have to break the signature first, and if they could do that they
    /// would not need this field.
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

        match nostr_crypto::unwrap_gift_wrap(&self.derivable_keypair, sender_pubkey_hex, data) {
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
        let recipient_tag = nostr_crypto::routing_tag_for_device_id(recipient_device_id)?;
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
        let encryption_pubkey = self
            .peer_nostr_pubkey(recipient_device_id)
            .unwrap_or_else(|| recipient_tag.clone());

        nostr_crypto::NostrEvent::create_gift_wrap(&recipient_tag, &encryption_pubkey, &data)
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
    /// `["REQ", "<sub_id>", {"#p": ["<routing_tag>"], "kinds": [4], "since": T, "limit": N}]`
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
        let pending = {
            let mut map = self.pending_confirmation.lock_or_recover();
            let count = map.len();
            map.clear();
            count
        };
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

        {
            let mut pending = self.pending_confirmation.lock_or_recover();
            pending.retain(|_, enqueued_at| {
                if now.duration_since(*enqueued_at) > timeout {
                    expired_count += 1;
                    false
                } else {
                    true
                }
            });
        }

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

    /// Gets the next message to send (for platform implementation).
    ///
    /// Returns `(message_id, serialized_bytes)` or `None` if no messages.
    /// The message enters the pending-confirmation state until the platform
    /// calls [`Transport::confirm_sent`] or [`Transport::report_send_failure`].
    ///
    /// **This path does not seal, and no Nostr bridge should use it.** It
    /// returns the bare serialized `Message` — the entire protocol envelope,
    /// both usernames included — with no gift wrap and no event around it, so
    /// publishing the result puts exactly the cleartext this transport now
    /// avoids in front of every relay. There is nowhere to put a sealed event
    /// in this signature; it exists only to satisfy the generic [`Transport`]
    /// trait.
    ///
    /// Poll [`NostrTransport::get_next_signed_event`] instead: it produces a
    /// complete signed, sealed `["EVENT", …]` message ready for the wire. That
    /// is what the bundled bridges and the UniFFI `nostr_get_next_message`
    /// entry call.
    fn get_next_message(&self) -> Result<Option<(String, Vec<u8>)>> {
        self.drain_expired_pending();

        let message = {
            let mut queue = self.send_queue.lock_or_recover();
            match queue.pop_front() {
                Some(m) => m,
                None => return Ok(None),
            }
        };

        let message_id = message.id.to_string();
        let data = self.serialize_message(&message)?;

        self.pending_confirmation
            .lock_or_recover()
            .insert(message_id.clone(), Instant::now());

        Ok(Some((message_id, data)))
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

        if removed.is_some() {
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

        if removed.is_some() {
            let mut metrics = self.metrics.lock_or_recover();
            metrics.failure_count = metrics.failure_count.saturating_add(1);
            recalculate_delivery_ratios(&mut metrics);
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

    fn create_test_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "Test message",
        )
    }

    #[test]
    fn test_nostr_transport_creation() {
        let transport = NostrTransport::new("device1").unwrap();
        assert_eq!(transport.device_id(), "device1");
        assert_eq!(transport.transport_type(), TransportType::Nostr);
        assert_eq!(transport.status(), TransportStatus::Unavailable);
    }

    #[test]
    fn test_builder() {
        let transport = NostrTransportBuilder::new("device1")
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

    #[test]
    fn test_send_receive() {
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let (msg_id, data) = transport.get_next_message().unwrap().unwrap();
        assert!(!msg_id.is_empty());
        assert!(!data.is_empty());

        let deserialized = transport.deserialize_message(&data).unwrap();
        assert_eq!(deserialized.id, msg.id);
    }

    #[test]
    fn test_send_when_unavailable_fails() {
        let transport = NostrTransport::new("device1").unwrap();
        let msg = create_test_message();
        assert!(transport.send(&msg).is_err());
    }

    #[test]
    fn test_receive_when_empty_returns_none() {
        let transport = NostrTransport::new("device1").unwrap();
        assert!(transport.receive().unwrap().is_none());
    }

    #[test]
    fn test_confirmation_loop() {
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let (msg_id, _) = transport.get_next_message().unwrap().unwrap();
        transport.confirm_sent(&msg_id);

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 1);
        assert_eq!(metrics.failure_count, 0);
    }

    #[test]
    fn test_send_failure_reporting() {
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();

        let (msg_id, _) = transport.get_next_message().unwrap().unwrap();
        transport.report_send_failure(&msg_id);

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 0);
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_fail_all_pending_on_disconnect() {
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let _ = transport.get_next_message().unwrap();

        transport.on_status_changed(TransportStatus::Disconnected);

        let metrics = transport.metrics();
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_stop_fails_pending() {
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let _ = transport.get_next_message().unwrap();

        transport.stop().unwrap();

        let metrics = transport.metrics();
        assert_eq!(metrics.failure_count, 1);
    }

    #[test]
    fn test_serialization() {
        let transport = NostrTransport::new("device1").unwrap();
        let msg = create_test_message();
        let data = transport.serialize_message(&msg).unwrap();
        let deserialized = transport.deserialize_message(&data).unwrap();
        assert_eq!(deserialized.id, msg.id);
    }

    #[test]
    fn test_reconnect_logic() {
        let transport = NostrTransportBuilder::new("device1")
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
        let transport = NostrTransport::new("device1").unwrap();
        let result = transport.on_data_received(b"not json".to_vec());
        assert!(result.is_ok());
        assert!(transport.receive().unwrap().is_none());
    }

    #[test]
    fn test_on_data_received_rejects_oversized_payload() {
        let transport = NostrTransport::new("device1").unwrap();
        let oversized = vec![0u8; DEFAULT_MAX_MESSAGE_SIZE + 1];
        let result = transport.on_data_received(oversized);
        assert!(result.is_err());
    }

    #[test]
    fn test_on_messages_available_callback() {
        let transport = NostrTransport::new("device1").unwrap();
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

        let transport = Arc::new(NostrTransport::new("device1").unwrap());
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
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let (msg_id, _) = transport.get_next_message().unwrap().unwrap();
        transport.confirm_sent(&msg_id);

        let mut new_metrics = TransportMetrics::default();
        new_metrics.rssi = Some(-70);
        transport.update_metrics(new_metrics);

        let metrics = transport.metrics();
        assert_eq!(metrics.success_count, 1);
        assert_eq!(metrics.rssi, Some(-70));
    }

    #[test]
    fn test_platform_handle() {
        let transport = NostrTransport::new("device1").unwrap();
        assert!(transport.platform_handle().is_none());
        transport.set_platform_handle(42);
        assert_eq!(transport.platform_handle(), Some(42));
    }

    #[test]
    fn test_drain_expired_pending_expires_old_entries() {
        let transport = NostrTransport::new("device1").unwrap();
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
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        assert!(!transport.has_pending_sends());

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        assert!(transport.has_pending_sends());
    }

    #[test]
    fn test_pending_confirmation_count() {
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        assert_eq!(transport.pending_confirmation_count(), 0);

        let msg = create_test_message();
        transport.send(&msg).unwrap();
        let _ = transport.get_next_message().unwrap();

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
        let transport = NostrTransport::new("device1").unwrap();
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

    #[test]
    fn test_get_next_signed_event_confirm_flow() {
        let transport = NostrTransport::new("device1").unwrap();
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
        let transport = NostrTransport::new("device1").unwrap();
        let expected_tag = nostr_crypto::routing_tag_for_device_id("device1").unwrap();

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
        let transport_a = NostrTransport::new("device1").unwrap();
        let transport_b = NostrTransport::new("device1").unwrap();

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
            nostr_crypto::routing_tag_for_device_id("device1").unwrap()
        );
    }

    #[test]
    fn test_oversized_event_is_dropped_permanently_and_does_not_block_queue() {
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let oversized = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
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
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        // The serialized message fits under the cap; base64 inflation (4/3)
        // pushes the event a relay actually sees over it. Capping the inner
        // payload instead would let this onto the wire to be rejected there.
        let msg = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
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
        let transport = NostrTransport::new("device1").unwrap();
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
        let transport = NostrTransport::new("device1").unwrap();
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
        let transport = NostrTransport::new("device1").unwrap();
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
        let transport = NostrTransport::new("device1").unwrap();
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
        let transport = NostrTransport::new("device1").unwrap();
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
        let transport = NostrTransport::new("device1").unwrap();
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
        let transport = NostrTransport::new("device1").unwrap();
        assert!(!transport.advance_receive_watermark(0));
        assert!(!transport.advance_receive_watermark(-1));
        assert!(transport.receive_watermark_secs().is_none());
    }

    #[test]
    fn test_stop_preserves_the_receive_watermark() {
        // stop() clears in-flight queues, but receive progress is not undone by
        // a restart — resetting it here would replay a full backfill window on
        // every stop/start cycle.
        let transport = NostrTransport::new("device1").unwrap();
        let mark = now_unix_secs() - 60;
        transport.advance_receive_watermark(mark);

        transport.stop().unwrap();

        assert_eq!(transport.receive_watermark_secs(), Some(mark));
    }

    #[test]
    fn test_signed_event_uses_recipient_routing_tag_and_own_signing_key() {
        let transport = NostrTransport::new("device1").unwrap();
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
        let bob_tag = nostr_crypto::routing_tag_for_device_id("bob").unwrap();
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
        // Asserted over the entire event JSON, not just `content`: a future
        // change that moved a username into a tag, or reintroduced a stable
        // signing pubkey derived from one, must fail here too.
        let transport = NostrTransport::new("alice").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("fernweh").unwrap(),
            "the quick brown fox",
        );
        transport.send(&msg).unwrap();

        let signed = transport.get_next_signed_event().unwrap().unwrap();
        let wire = signed.event_json.to_lowercase();

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
        let bob_tag = nostr_crypto::routing_tag_for_device_id("bob").unwrap();
        assert!(wire.contains(&bob_tag));
    }

    #[test]
    fn test_sealed_event_is_a_gift_wrap_not_signed_by_our_install_key() {
        let transport = NostrTransport::new("alice").unwrap();
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
        let alice = NostrTransport::new("alice").unwrap();
        alice.start().unwrap();
        alice.on_status_changed(TransportStatus::Available);

        let bob = NostrTransport::new("bob").unwrap();
        bob.install_signing_secret(&[88u8; 32]).unwrap();
        // Alice has seen Bob's key package, so this is the steady state.
        alice.set_peer_nostr_pubkey("bob", &bob.public_key_hex());

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
        assert_eq!(received.sender.as_str(), "alice");
    }

    #[test]
    fn test_bootstrap_frame_round_trips_before_any_key_exchange() {
        // Cold first contact: Alice knows only Bob's user id. The frame is
        // sealed to Bob's publicly computable key, and Bob's receive path finds
        // it on the second attempt.
        let alice = NostrTransport::new("alice").unwrap();
        alice.start().unwrap();
        alice.on_status_changed(TransportStatus::Available);

        let bob = NostrTransport::new("bob").unwrap();
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
        let alice = NostrTransport::new("alice").unwrap();
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

        let bob = NostrTransport::new("bob").unwrap();
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
        let bob = NostrTransport::new("bob").unwrap();
        let carol_tag = nostr_crypto::routing_tag_for_device_id("carol").unwrap();

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
        let bob = NostrTransport::new("bob").unwrap();
        let bob_tag = nostr_crypto::routing_tag_for_device_id("bob").unwrap();
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
        let transport = NostrTransport::new("device1").unwrap();

        transport.set_peer_nostr_pubkey("bob", "not-hex");
        transport.set_peer_nostr_pubkey("bob", &"ab".repeat(31)); // 62 chars
        transport.set_peer_nostr_pubkey("", &"ab".repeat(32));
        assert!(transport.peer_nostr_pubkey("bob").is_none());

        // Case is normalized: `#p`-style values are lowercase hex by spec, and a
        // mixed-case duplicate must not seal to a different string.
        let key = "AB".repeat(32);
        transport.set_peer_nostr_pubkey("bob", &key);
        assert_eq!(transport.peer_nostr_pubkey("bob"), Some(key.to_lowercase()));

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
        let alice = NostrTransport::new("alice").unwrap();
        alice.start().unwrap();
        alice.on_status_changed(TransportStatus::Available);

        let bob = NostrTransport::new("bob").unwrap();
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
        let transport = NostrTransport::new("alice").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

        let msg = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
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
}
