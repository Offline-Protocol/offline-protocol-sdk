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
    NOSTR_CONNECTION_TIMEOUT_SECS, NOSTR_MAX_PAYLOAD_SIZE, NOSTR_PENDING_CONFIRMATION_TIMEOUT_SECS,
};
use crate::nostr_crypto::{self, NostrKeypair};
use crate::{Result, SharedCallback, Transport, TransportMetrics, TransportStatus, TransportType};
use base64::Engine;
use offline_protocol_core::{Message, MutexExt, RwLockExt};
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
/// `keypair` is a leaf lock: it is only ever held in a narrow scope with no
/// other lock acquisition inside.
pub struct NostrTransport {
    device_id: String,
    /// Per-install signing keypair. Ephemeral (random per process) until the
    /// engine installs the persisted secret via
    /// [`Self::install_signing_secret`].
    keypair: RwLock<NostrKeypair>,
    /// This device's public routing tag (derived from `device_id`); peers
    /// address us by putting it in the `#p` tag, we subscribe on it.
    routing_tag: String,
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
        let keypair = RwLock::new(NostrKeypair::generate_ephemeral()?);
        Ok(Self {
            device_id,
            keypair,
            routing_tag,
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
        let recipient_device_id = message.recipient.as_str().to_string();

        let result = (|| {
            let data = self.serialize_message(&message)?;
            let content_base64 = base64::engine::general_purpose::STANDARD.encode(&data);

            let recipient_tag = nostr_crypto::routing_tag_for_device_id(&recipient_device_id)?;
            let keypair = self.keypair.read_or_recover();
            let event =
                nostr_crypto::NostrEvent::create_dm(&keypair, &recipient_tag, &content_base64)?;
            drop(keypair);
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

    /// Returns a NIP-01 subscription filter JSON for this device's routing tag.
    ///
    /// The platform should send this to each relay after connecting:
    /// `["REQ", "<sub_id>", {"#p": ["<routing_tag>"], "kinds": [4], "limit": N}]`
    ///
    /// The filter is on the routing tag — not the signing pubkey — so it is
    /// stable across signing-key changes and derivable by peers. The `limit`
    /// caps the stored-event replay a (re)connect pulls down; it does not cap
    /// live delivery.
    pub fn create_subscription(&self, subscription_id: &str) -> Result<String> {
        nostr_crypto::create_subscription_message(&self.routing_tag, subscription_id)
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

    fn on_data_received(&self, data: Vec<u8>) -> Result<()> {
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
    /// Most Nostr platforms should poll
    /// [`NostrTransport::get_next_signed_event`] instead, which wraps and
    /// signs the payload as a Nostr event.
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

    #[test]
    fn test_signed_event_uses_recipient_routing_tag_and_own_signing_key() {
        let transport = NostrTransport::new("device1").unwrap();
        transport.start().unwrap();
        transport.on_status_changed(TransportStatus::Available);

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
            "event must be signed by this install's signing key"
        );
    }
}
