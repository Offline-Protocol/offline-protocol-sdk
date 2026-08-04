//! Protocol configuration.

use crate::constants::DEFAULT_INITIAL_TTL;
use offline_protocol_reliability::{AckConfig, DeduplicatorConfig, RetryConfig};
use offline_protocol_router::{DorsConfig, PathConfig, RelayConfig};
use serde::{Deserialize, Serialize};

/// Overflow policy for bounded pending encrypted message queues.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverflowPolicy {
    /// Evict the oldest queued message to make room for the new message.
    #[default]
    DropOldest,
    /// Drop the newly received message when capacity is reached.
    DropNewest,
}

/// Configuration for inbound encrypted messages received before session readiness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingQueueConfig {
    /// Maximum number of pending encrypted messages per peer.
    pub max_pending_per_peer: usize,
    /// Maximum number of pending encrypted messages across all peers.
    pub max_pending_global: usize,
    /// Maximum total payload bytes (content plus binary content) queued per
    /// peer.
    ///
    /// Encrypted media chunks are queued whole while the sender's session is
    /// not yet ready, so the count limits alone would admit up to
    /// `max_pending_per_peer` × chunk-size bytes of ciphertext. The default
    /// (4 MB) covers the largest legitimate pre-session backlog: 2 concurrent
    /// transfers × 8 in-flight chunks × 256 KB internet chunks.
    #[serde(default = "default_max_pending_bytes_per_peer")]
    pub max_pending_bytes_per_peer: usize,
    /// Maximum total payload bytes queued across all peers. The default
    /// (32 MB) bounds worst-case queue memory on mobile devices regardless of
    /// how many peers are mid-establishment.
    #[serde(default = "default_max_pending_bytes_global")]
    pub max_pending_bytes_global: usize,
    /// Time-to-live for pending encrypted messages in milliseconds.
    pub pending_ttl_ms: u64,
    /// Overflow policy when queue limits are reached.
    pub overflow_policy: OverflowPolicy,
}

/// Default TTL for the pending-decryption queue (30 minutes, in ms).
///
/// Under the deferred-ACK model an undecryptable message is no longer ACKed on
/// receipt, so this queue is the primary recovery window before the session
/// confirms — a 2-minute window was too short for a peer whose Welcome is slow
/// to arrive/adopt. Memory stays bounded by the per-peer/global byte caps plus
/// the `DropOldest` overflow policy; a longer TTL only lets entries linger
/// within those caps, it does not raise the ceiling.
///
/// The UniFFI `PendingQueueConfig` default mirrors this value — reference this
/// constant there rather than re-hardcoding it.
pub const DEFAULT_PENDING_TTL_MS: u64 = 1_800_000;

fn default_max_pending_bytes_per_peer() -> usize {
    4 * 1024 * 1024
}

fn default_max_pending_bytes_global() -> usize {
    32 * 1024 * 1024
}

impl Default for PendingQueueConfig {
    fn default() -> Self {
        Self {
            max_pending_per_peer: 64,
            max_pending_global: 4096,
            max_pending_bytes_per_peer: default_max_pending_bytes_per_peer(),
            max_pending_bytes_global: default_max_pending_bytes_global(),
            // 30 minutes; see DEFAULT_PENDING_TTL_MS for the deferred-ACK
            // rationale and the FFI-mirror contract.
            pending_ttl_ms: DEFAULT_PENDING_TTL_MS,
            overflow_policy: OverflowPolicy::DropOldest,
        }
    }
}

/// Encryption configuration for automatic MLS handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptionConfig {
    /// Whether automatic encryption is enabled.
    /// When enabled, messages are automatically encrypted/decrypted using MLS.
    pub enabled: bool,

    /// Auto-exchange key packages on peer discovery.
    /// When enabled, key packages are automatically sent when neighbors are discovered.
    pub auto_key_exchange: bool,

    /// Store pending messages when no session exists.
    /// Messages will be sent automatically after the session is established.
    pub store_pending: bool,

    /// Require encryption for messages (default: `true`).
    ///
    /// When enabled, sends fail closed if encryption cannot be applied:
    /// with MLS uninitialized every send returns [`crate::Error::EncryptFailed`],
    /// and with no confirmed session messages are queued (when
    /// `store_pending` is set) or fail with
    /// [`crate::Error::SessionNotReady`] instead of ever leaving as
    /// plaintext. Inbound plaintext content — text messages and legacy
    /// media alike — is rejected without being surfaced to the app
    /// (plaintext carries no sender authentication, so anyone could
    /// inject it under a contact's name); each rejection emits a
    /// [`crate::events::SecurityWarningCode::PlaintextReceiveRejected`]
    /// warning, once per peer. Even under the opt-out, inbound plaintext
    /// from a peer with a confirmed MLS session is rejected as a
    /// downgrade/forgery attempt.
    ///
    /// Disable only for deployments that deliberately send cleartext
    /// (e.g. open broadcast/mesh bootstrap without provisioned MLS
    /// storage) — set this to `false` explicitly, or use
    /// [`EncryptionConfig::disabled`] to turn encryption off entirely.
    /// Every plaintext send then emits a
    /// [`crate::events::SecurityWarningCode::PlaintextSend`] warning
    /// (once per peer). Internal control messages (key exchange, ACKs,
    /// service discovery) are exempt and unaffected.
    pub require_encryption: bool,

    /// Bounds and eviction policy for pending inbound encrypted messages.
    pub pending_queue: PendingQueueConfig,

    /// Whether to negotiate and emit the compact MLS envelope for encrypted
    /// messages (base64 of the binary `EncryptedMessage` form instead of JSON
    /// with an integer-array ciphertext — roughly 2.7x smaller).
    ///
    /// Defaults to `true`. It only takes effect toward recipients that
    /// advertise support in their key package (`env_versions`), so a mixed
    /// fleet automatically stays on the JSON envelope. Parsing of inbound
    /// compact envelopes is always on, independent of this flag; this gates
    /// only advertising and emitting. The end-to-end sibling of
    /// [`TransportConfig::binary_wire_enabled`], with an independent kill
    /// switch because the two degrade separately.
    pub compact_envelope_enabled: bool,

    /// Whether to negotiate and seal the rich `__RICH_V1__` payload for
    /// encrypted messages: quoted-reply context, rich media metadata, and
    /// forward attribution carried *inside* the MLS ciphertext instead of on
    /// the relay-visible outer message.
    ///
    /// Defaults to `true`. It only takes effect toward recipients that
    /// advertise support in their key package (`rich_versions`); toward
    /// everyone else the rich extras are silently dropped — never sent
    /// cleartext. Parsing of inbound sealed payloads is always on,
    /// independent of this flag; this gates only advertising and sealing.
    /// Independent kill switch from
    /// [`EncryptionConfig::compact_envelope_enabled`] — the two degrade
    /// separately.
    pub rich_payload_enabled: bool,

    /// Whether to recover a 1:1 MLS session that has fallen out of epoch sync
    /// with the peer (the two sides disagree on the MLS epoch — e.g. after a
    /// fork), rather than silently dropping the undecryptable message.
    ///
    /// Defaults to `true`. When enabled, an inbound encrypted DM (or media
    /// chunk) that fails to decrypt *specifically* because of an epoch
    /// mismatch is not delivery-ACKed — so the sender keeps retrying instead of
    /// marking it delivered — and a rate-limited session re-key (the
    /// `session_reset` key-package exchange) is triggered to rebuild the
    /// channel. Genuine decrypt failures (corrupt/forged ciphertext, discarded
    /// ratchet generations) are unaffected and still fail closed as before, so
    /// this never re-keys on injected garbage. When disabled, an epoch-mismatch
    /// failure falls back to the legacy drop-and-ACK behavior.
    pub crypto_recovery_enabled: bool,
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_key_exchange: true,
            store_pending: true,
            // Fail closed by default (SEC-M3): a stock-config node must never
            // silently degrade to plaintext because MLS was left uninitialized.
            require_encryption: true,
            pending_queue: PendingQueueConfig::default(),
            compact_envelope_enabled: true,
            rich_payload_enabled: true,
            crypto_recovery_enabled: true,
        }
    }
}

impl EncryptionConfig {
    /// Creates a new encryption config with encryption disabled.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            auto_key_exchange: false,
            store_pending: false,
            require_encryption: false,
            pending_queue: PendingQueueConfig::default(),
            compact_envelope_enabled: false,
            rich_payload_enabled: false,
            crypto_recovery_enabled: false,
        }
    }
}

/// Security configuration for transport and control-message hardening.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// When `true`, control messages with no transport-level peer identity
    /// (`transport_peer_id` is `None`) are rejected. When `false` (default),
    /// missing transport identity emits a `SecurityWarning` but allows the
    /// message through (best-effort / fail-open).
    ///
    /// # Security implications of the default (`false`)
    ///
    /// With fail-open behavior, an attacker who can inject messages into the
    /// transport layer (e.g., by compromising a relay server or being in BLE
    /// range) can send spoofed control messages with a forged `sender` field.
    /// Ed25519 control-message signing (TOFU) mitigates this for peers whose
    /// keys have already been pinned, but the first contact with any peer is
    /// vulnerable to man-in-the-middle if the transport identity is absent.
    ///
    /// When a transport attaches an authenticated identity (Internet relay,
    /// Reticulum), frames claiming direct origin (`hop_count == 0`) are
    /// strict-matched against `message.sender` regardless of this flag —
    /// spoofed hop-0 control frames on those transports are rejected even
    /// with the default `false`. Frames claiming mesh relay
    /// (`hop_count > 0`) skip the strict match (the identity names the
    /// carrier, not the origin) and rest on the signature + TOFU gate.
    ///
    /// Enabling this flag therefore tightens two things: control frames
    /// *without* any transport identity are rejected, and *unsigned*
    /// security-gated control frames are rejected outright — otherwise a
    /// spoofer could forge `hop_count > 0` to skip the strict match and do
    /// better than a peer with no identity at all. The remaining trust
    /// assumption is first-contact TOFU pinning.
    ///
    /// Set to `true` only when every enabled transport reliably attaches
    /// peer identity AND all peers sign control traffic (MLS initialized —
    /// legacy pre-MLS peers cannot interoperate with a strict deployment).
    /// Today that means Internet + Reticulum deployments; BLE, WiFi Direct,
    /// and Nostr inbound frames carry no transport identity, so `true`
    /// would reject all their control messages, and mesh-forwarded frames
    /// re-created by intermediate nodes legitimately lack identity as well.
    pub require_transport_identity: bool,
}

/// Transport-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// Whether BLE transport is enabled.
    pub ble_enabled: bool,

    /// Whether Wi-Fi Direct transport is enabled.
    pub wifi_direct_enabled: bool,

    /// Whether Internet transport is enabled.
    pub internet_enabled: bool,

    /// Whether Reticulum mesh transport is enabled.
    /// Defaults to `false` because Reticulum requires external infrastructure
    /// (a running Reticulum daemon or RNode hardware).
    pub reticulum_enabled: bool,

    /// Whether Nostr relay transport is enabled.
    /// Defaults to `false` because Nostr requires relay configuration
    /// and a secp256k1 keypair.
    pub nostr_enabled: bool,

    /// Whether to negotiate and emit the compact binary wire codec.
    ///
    /// Defaults to `true`. It only takes effect between two peers that both
    /// advertise support (via their signed key package), so a mixed fleet
    /// automatically stays on JSON. Decoding of binary frames is always on,
    /// independent of this flag; this gates only advertising and emitting.
    pub binary_wire_enabled: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            ble_enabled: true,
            wifi_direct_enabled: true,
            internet_enabled: true,
            reticulum_enabled: false,
            nostr_enabled: false,
            binary_wire_enabled: true,
        }
    }
}

/// Reliability configuration combining ACK, retry, and deduplication settings.
#[derive(Debug, Clone, Default)]
pub struct ReliabilityConfig {
    /// ACK manager configuration.
    pub ack: AckConfig,

    /// Retry queue configuration.
    pub retry: RetryConfig,

    /// Deduplicator configuration.
    pub dedup: DeduplicatorConfig,
}

/// Configuration for group messaging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfig {
    /// Maximum number of members allowed in a single group.
    ///
    /// Per-member fan-out is O(N) when using mesh transports.
    /// This cap prevents unbounded groups from overwhelming the network.
    pub max_group_members: usize,

    /// Whether to attempt relay server registration for groups.
    ///
    /// When `true` (default), groups are registered with the relay server
    /// for optimized fan-out when Internet transport is available.
    /// When `false`, groups always use per-member fan-out regardless of
    /// transport.
    ///
    /// This gates *registration* only — the relay group registry is what
    /// invite links resolve against, so it stays on independently of
    /// [`Self::relay_broadcast_enabled`].
    pub relay_enabled: bool,

    /// Whether a relay-synced group may send via a single O(1) relay
    /// broadcast instead of per-member fan-out.
    ///
    /// **Default `true`, and additionally gated on the relay's advertised
    /// capabilities.** The flag alone does not select the broadcast: the
    /// path is taken only when the connected relay also advertised the
    /// `group_delivery_v2` capability in its `Authenticated` answer. Against
    /// such a relay the broadcast keeps a delivery contract — the relay
    /// answers every broadcast with a *settled* per-recipient delivery
    /// report (`GroupMessageSent` v2) naming who took the message over a
    /// socket and who took a device push, and the SDK re-sends a per-member
    /// copy through the ordinary outbox/ACK/park ladder to every MLS roster
    /// member the report does not account for. A lost report re-sends the
    /// broadcast under the same logical message id (receiver and push dedup
    /// absorb the duplicate fan-out) a bounded number of times, then
    /// downgrades the message to full per-member fan-out. Against an older
    /// relay the capability gate fails and every send takes per-member
    /// fan-out — the fire-and-forget broadcast of the v1 relay (no presence
    /// check, no push fallback, no persistence, "sent" answered before
    /// delivery was known) is never taken, regardless of this flag.
    ///
    /// The one gap on the broadcast path: the tracker awaiting the report
    /// lives in memory only, so a process death inside the report window
    /// (up to `RELAY_BROADCAST_REPORT_TIMEOUT_SECS` per attempt) loses the
    /// backstop with it — members the relay could not reach get no
    /// per-member re-send, and nothing retries on restart. It needs both a
    /// missed member and a kill in that window, and is strictly smaller than
    /// the pre-report broadcast's exposure, but it is a real difference from
    /// per-member fan-out, whose outbox entries persist for 7 days.
    ///
    /// Set `false` to force per-member fan-out even against a v2 relay —
    /// the right choice for an app that must not lose a group message to a
    /// mid-flight process kill.
    /// Per-member fan-out (`__GRP_MLS_MSG__` as ordinary `SendMessage`
    /// frames) inherits the full DM ladder end to end — outbox + ACK +
    /// retry, relay write-ack + successor retry + offline push carrying the
    /// ciphertext, park-on-unreachable with presence-driven flush, and
    /// receiver-side deferred-ACK-after-decrypt — at the cost of O(N)
    /// uplink frames per send.
    ///
    /// O(N) fan-out does **not** risk tripping the relay's rate limiter at
    /// any group size. The platform bridge meters every relay-bound frame
    /// through a client-side token bucket (`RelayRateLimiter`: 28 capacity,
    /// 9/s refill) deliberately tighter than the relay's (30 burst, 10/s),
    /// and a frame that cannot take a token is *deferred to a later poll
    /// tick*, never dropped — so client spend stays at `28 + 9t` against a
    /// server budget of `30 + 10t` and the fan-out self-paces below it
    /// regardless of member count. Core enqueues all N frames at once; the
    /// bridge drains that queue.
    ///
    /// What large groups actually cost on the per-member path is drain
    /// latency. Frame N reaches the wire at roughly `(N - 28) / 9` seconds.
    /// Since the ACK timer starts at local *enqueue* (`handle_send_success`)
    /// and not at wire-confirm — `on_transport_send_confirmed` advances only
    /// the welcome lifecycle — past roughly 118 members the tail of a single
    /// fan-out exceeds the 10s ACK timeout and is retransmitted by the
    /// ladder before it was ever written. That is wasted duplicate frames,
    /// absorbed by receiver and push dedup via the stable outbox id, not
    /// lost messages. Presence checks, typing, and receipts draw on the same
    /// bucket and lower the threshold. This is the strongest reason to leave
    /// the broadcast enabled for large groups.
    pub relay_broadcast_enabled: bool,

    /// Whether an incoming MLS membership commit is refused when the local
    /// admin overlay does not authorize its committer.
    ///
    /// **Off by default, and turning it on is a decision about partition
    /// risk, not a hardening toggle.** Refusing a commit means declining the
    /// MLS merge: our epoch stays behind every member who did merge, and MLS
    /// has no way back — the application has to re-invite us. Enforcement is
    /// therefore fork-free only if every member reaches the *same* verdict,
    /// and the admin overlay is replicated best-effort (role changes ride
    /// unreconciled mesh notifications; a joiner gets a point-in-time
    /// snapshot). Two members whose role maps merely disagree will partition
    /// each other with no attacker involved.
    ///
    /// The check fails open on every *absent* input — no metadata, no admin
    /// role stored, an unreadable roster — so the common lag case stays
    /// harmless. It cannot detect *divergent* views, which is the residual
    /// risk that keeps this opt-in. Enable it only for a closed deployment
    /// that controls role distribution.
    ///
    /// With it off (the default), unauthorized membership changes are still
    /// applied and reported: `group_unauthorized_membership_change` fires and
    /// the roster events carry `authorized: false`. That reporting is
    /// unaffected by this flag; the flag only decides whether the commit is
    /// *also* refused.
    ///
    /// Rejection is receiver-local. The sender's message is still ACKed, so
    /// a committer gets no signal that anyone refused, and members with the
    /// flag off will apply a commit that members with it on rejected — do not
    /// enable it on part of a fleet.
    #[serde(default)]
    pub enforce_admin_commits: bool,
}

impl Default for GroupConfig {
    fn default() -> Self {
        Self {
            max_group_members: 256,
            relay_enabled: true,
            relay_broadcast_enabled: true,
            enforce_admin_commits: false,
        }
    }
}

/// Main configuration for the Offline Protocol.
#[derive(Debug, Clone)]
pub struct ProtocolConfig {
    /// Application identifier (required).
    pub app_id: String,

    /// User identifier (required).
    ///
    /// This exact string is the device's canonical identity on every
    /// surface: it is stamped as `Message.sender` on outbound frames, it is
    /// what peers see as `NeighborDiscovered.peer_id` when they discover
    /// this device (on any transport), and it is the `recipient` string
    /// others use to reach it. Discovery, blocking, and outbox flushing all
    /// key on this one namespace.
    pub user_id: String,

    /// Transport configuration.
    pub transport: TransportConfig,

    /// DORS (Dynamic Offline Relay Switch) configuration.
    pub dors: DorsConfig,

    /// Relay management configuration.
    pub relay: RelayConfig,

    /// Path selection configuration.
    pub path: PathConfig,

    /// Reliability layer configuration.
    pub reliability: ReliabilityConfig,

    /// Encryption configuration for automatic MLS handling.
    pub encryption: EncryptionConfig,

    /// Initial TTL (Time-To-Live) for messages.
    pub initial_ttl: u8,

    /// Mesh group messaging configuration.
    pub group: GroupConfig,

    /// Security configuration for transport and control-message hardening.
    pub security: SecurityConfig,
}

impl ProtocolConfig {
    /// Creates a new protocol configuration.
    ///
    /// # Arguments
    ///
    /// * `app_id` - Application identifier
    /// * `user_id` - User identifier
    pub fn new(app_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            user_id: user_id.into(),
            transport: TransportConfig::default(),
            dors: DorsConfig::default(),
            relay: RelayConfig::default(),
            path: PathConfig::default(),
            reliability: ReliabilityConfig::default(),
            encryption: EncryptionConfig::default(),
            initial_ttl: DEFAULT_INITIAL_TTL,
            group: GroupConfig::default(),
            security: SecurityConfig::default(),
        }
    }

    /// Creates a builder for more granular configuration.
    pub fn builder(app_id: impl Into<String>, user_id: impl Into<String>) -> ProtocolConfigBuilder {
        ProtocolConfigBuilder::new(app_id, user_id)
    }

    /// Validates the configuration.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if valid, `Err` with a description of the problem if invalid.
    pub fn validate(&self) -> crate::Result<()> {
        if self.app_id.is_empty() {
            return Err(crate::Error::InvalidConfiguration(
                "app_id cannot be empty".to_string(),
            ));
        }

        if self.user_id.is_empty() {
            return Err(crate::Error::InvalidConfiguration(
                "user_id cannot be empty".to_string(),
            ));
        }

        if self.initial_ttl == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "initial_ttl must be greater than 0".to_string(),
            ));
        }

        if !self.transport.ble_enabled
            && !self.transport.wifi_direct_enabled
            && !self.transport.internet_enabled
            && !self.transport.reticulum_enabled
            && !self.transport.nostr_enabled
        {
            return Err(crate::Error::InvalidConfiguration(
                "At least one transport must be enabled".to_string(),
            ));
        }

        if self.reliability.ack.default_timeout_ms == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "ack.default_timeout_ms must be greater than 0".to_string(),
            ));
        }

        if self.reliability.ack.max_pending_acks == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "ack.max_pending_acks must be greater than 0".to_string(),
            ));
        }

        if self.reliability.retry.initial_delay_ms == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "retry.initial_delay_ms must be greater than 0".to_string(),
            ));
        }

        if self.reliability.retry.max_delay_ms == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "retry.max_delay_ms must be greater than 0".to_string(),
            ));
        }

        if self.reliability.retry.initial_delay_ms > self.reliability.retry.max_delay_ms {
            return Err(crate::Error::InvalidConfiguration(
                "retry.initial_delay_ms must be <= retry.max_delay_ms".to_string(),
            ));
        }

        if self.reliability.retry.outbox_max_lifetime_ms == 0
            || self.reliability.retry.outbox_max_lifetime_ms > i64::MAX as u64
        {
            return Err(crate::Error::InvalidConfiguration(
                "retry.outbox_max_lifetime_ms must be in 1..=i64::MAX".to_string(),
            ));
        }

        if self.reliability.retry.pending_message_max_lifetime_ms == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "retry.pending_message_max_lifetime_ms must be greater than 0".to_string(),
            ));
        }

        if self.reliability.retry.pending_message_max_lifetime_ms > i64::MAX as u64 {
            return Err(crate::Error::InvalidConfiguration(
                "retry.pending_message_max_lifetime_ms must be <= i64::MAX".to_string(),
            ));
        }

        if !self.reliability.retry.backoff_multiplier.is_finite()
            || self.reliability.retry.backoff_multiplier < 1.0
        {
            return Err(crate::Error::InvalidConfiguration(
                "retry.backoff_multiplier must be finite and >= 1.0".to_string(),
            ));
        }

        // Checked outside the Bloom guard below, and that placement is the
        // whole point: these two are the *only* dedup fields the UniFFI
        // `DedupConfig` carries, so every constraint hidden behind
        // `use_bloom_filter` is unreachable from a binding caller. A zero here
        // is not a cosmetic problem — `Deduplicator` fails safe on the Bloom
        // parameters, but nothing fails safe on these. At
        // `max_tracked_messages == 0` the HashMap branch of `mark_seen` evicts
        // on every insert, so the map holds one id and duplicate suppression is
        // effectively off; `retention_time_secs == 0` expires every entry
        // immediately for the same net result.
        //
        // What this rejects is the *degenerate* value, and that is the whole of
        // the claim: a floor of 1 is not a floor on how well duplicates are
        // suppressed. `max_tracked_messages == 1` behaves indistinguishably
        // from `0`, and a two-second retention on a link whose retry backoff is
        // longer than that suppresses nothing either. Choosing a window wide
        // enough for the deployment is the application's call — the SDK refuses
        // only the values that cannot be a choice, because nothing downstream
        // spells them as a configuration error and the failure they produce
        // (silently re-delivered messages) does not look like a config bug.
        // Raising either floor to a policy minimum is a wider behavioural break
        // and would need its own decision.
        if self.reliability.dedup.max_tracked_messages == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "reliability.dedup.max_tracked_messages must be greater than 0".to_string(),
            ));
        }

        if self.reliability.dedup.retention_time_secs == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "reliability.dedup.retention_time_secs must be greater than 0".to_string(),
            ));
        }

        if self.reliability.dedup.use_bloom_filter {
            if self.reliability.dedup.bloom_filter_bits == 0 {
                return Err(crate::Error::InvalidConfiguration(
                    "reliability.dedup.bloom_filter_bits must be greater than 0".to_string(),
                ));
            }

            if self.reliability.dedup.bloom_hash_count == 0 {
                return Err(crate::Error::InvalidConfiguration(
                    "reliability.dedup.bloom_hash_count must be greater than 0".to_string(),
                ));
            }

            if self.reliability.dedup.bloom_filter_count == 0 {
                return Err(crate::Error::InvalidConfiguration(
                    "reliability.dedup.bloom_filter_count must be greater than 0".to_string(),
                ));
            }

            if self.reliability.dedup.bloom_rotation_secs == 0 {
                return Err(crate::Error::InvalidConfiguration(
                    "reliability.dedup.bloom_rotation_secs must be greater than 0".to_string(),
                ));
            }
        }

        if self.encryption.require_encryption && !self.encryption.enabled {
            return Err(crate::Error::InvalidConfiguration(
                "encryption.require_encryption requires encryption.enabled=true".to_string(),
            ));
        }

        if self.encryption.pending_queue.max_pending_per_peer == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "encryption.pending_queue.max_pending_per_peer must be greater than 0".to_string(),
            ));
        }

        if self.encryption.pending_queue.max_pending_global == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "encryption.pending_queue.max_pending_global must be greater than 0".to_string(),
            ));
        }

        if self.encryption.pending_queue.max_pending_global
            < self.encryption.pending_queue.max_pending_per_peer
        {
            return Err(crate::Error::InvalidConfiguration(
                "encryption.pending_queue.max_pending_global must be >= max_pending_per_peer"
                    .to_string(),
            ));
        }

        if self.encryption.pending_queue.pending_ttl_ms == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "encryption.pending_queue.pending_ttl_ms must be greater than 0".to_string(),
            ));
        }

        if self.group.max_group_members == 0 {
            return Err(crate::Error::InvalidConfiguration(
                "group.max_group_members must be greater than 0".to_string(),
            ));
        }

        Ok(())
    }
}

/// Builder for ProtocolConfig with a fluent API.
pub struct ProtocolConfigBuilder {
    config: ProtocolConfig,
}

impl ProtocolConfigBuilder {
    /// Creates a new builder.
    pub fn new(app_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            config: ProtocolConfig::new(app_id, user_id),
        }
    }

    /// Configures transports.
    pub fn transport(mut self, config: TransportConfig) -> Self {
        self.config.transport = config;
        self
    }

    /// Enables or disables BLE transport.
    pub fn ble_enabled(mut self, enabled: bool) -> Self {
        self.config.transport.ble_enabled = enabled;
        self
    }

    /// Enables or disables Wi-Fi Direct transport.
    pub fn wifi_direct_enabled(mut self, enabled: bool) -> Self {
        self.config.transport.wifi_direct_enabled = enabled;
        self
    }

    /// Enables or disables Internet transport.
    pub fn internet_enabled(mut self, enabled: bool) -> Self {
        self.config.transport.internet_enabled = enabled;
        self
    }

    /// Enables or disables Nostr relay transport.
    pub fn nostr_enabled(mut self, enabled: bool) -> Self {
        self.config.transport.nostr_enabled = enabled;
        self
    }

    /// Configures DORS (Dynamic Offline Relay Switch).
    pub fn dors(mut self, config: DorsConfig) -> Self {
        self.config.dors = config;
        self
    }

    /// Enables online-first mode (prefer Internet when available).
    pub fn online_first(mut self, enabled: bool) -> Self {
        self.config.dors.prefer_online = enabled;
        self
    }

    /// Configures relay management.
    pub fn relay(mut self, config: RelayConfig) -> Self {
        self.config.relay = config;
        self
    }

    /// Configures path selection.
    pub fn path(mut self, config: PathConfig) -> Self {
        self.config.path = config;
        self
    }

    /// Configures reliability layer.
    pub fn reliability(mut self, config: ReliabilityConfig) -> Self {
        self.config.reliability = config;
        self
    }

    /// Sets the initial TTL for messages.
    pub fn initial_ttl(mut self, ttl: u8) -> Self {
        self.config.initial_ttl = ttl;
        self
    }

    /// Configures encryption settings.
    pub fn encryption(mut self, config: EncryptionConfig) -> Self {
        self.config.encryption = config;
        self
    }

    /// Enables or disables automatic encryption.
    pub fn encryption_enabled(mut self, enabled: bool) -> Self {
        self.config.encryption.enabled = enabled;
        self
    }

    /// Enables or disables automatic key exchange on peer discovery.
    pub fn auto_key_exchange(mut self, enabled: bool) -> Self {
        self.config.encryption.auto_key_exchange = enabled;
        self
    }

    /// Enables or disables storing pending messages for later encryption.
    pub fn store_pending_messages(mut self, enabled: bool) -> Self {
        self.config.encryption.store_pending = enabled;
        self
    }

    /// Enables or disables strict encryption requirement.
    pub fn require_encryption(mut self, required: bool) -> Self {
        self.config.encryption.require_encryption = required;
        self
    }

    /// Configures pending encrypted message queue bounds and overflow behavior.
    pub fn pending_queue(mut self, config: PendingQueueConfig) -> Self {
        self.config.encryption.pending_queue = config;
        self
    }

    /// Configures group messaging settings.
    pub fn group(mut self, config: GroupConfig) -> Self {
        self.config.group = config;
        self
    }

    /// Sets the maximum number of members allowed in a single group.
    pub fn max_group_members(mut self, max: usize) -> Self {
        self.config.group.max_group_members = max;
        self
    }

    /// Sets whether groups should attempt relay server registration.
    pub fn group_relay_enabled(mut self, enabled: bool) -> Self {
        self.config.group.relay_enabled = enabled;
        self
    }

    /// Sets whether relay-synced groups may send via a single relay broadcast
    /// instead of per-member fan-out. See
    /// [`GroupConfig::relay_broadcast_enabled`] — on by default, but only
    /// ever taken against a relay that advertised the `group_delivery_v2`
    /// capability, whose settled delivery report gives the broadcast a
    /// per-recipient delivery contract.
    pub fn group_relay_broadcast_enabled(mut self, enabled: bool) -> Self {
        self.config.group.relay_broadcast_enabled = enabled;
        self
    }

    /// Sets whether unauthorized MLS membership commits are refused rather
    /// than applied-and-reported. See
    /// [`GroupConfig::enforce_admin_commits`] — off by default, and enabling
    /// it risks permanently forking members whose admin overlay disagrees.
    pub fn group_enforce_admin_commits(mut self, enabled: bool) -> Self {
        self.config.group.enforce_admin_commits = enabled;
        self
    }

    /// Configures security settings.
    pub fn security(mut self, config: SecurityConfig) -> Self {
        self.config.security = config;
        self
    }

    /// Sets whether transport identity is required for control messages.
    pub fn require_transport_identity(mut self, required: bool) -> Self {
        self.config.security.require_transport_identity = required;
        self
    }

    /// Builds and validates the configuration.
    ///
    /// # Returns
    ///
    /// Returns `Ok(ProtocolConfig)` if valid, `Err` otherwise.
    pub fn build(self) -> crate::Result<ProtocolConfig> {
        self.config.validate()?;
        Ok(self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_creation() {
        let config = ProtocolConfig::new("test-app", "user123");
        assert_eq!(config.app_id, "test-app");
        assert_eq!(config.user_id, "user123");
        assert_eq!(config.initial_ttl, 8);
    }

    #[test]
    fn test_config_validation_success() {
        let config = ProtocolConfig::new("test-app", "user123");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_app_id() {
        let config = ProtocolConfig::new("", "user123");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_empty_user_id() {
        let config = ProtocolConfig::new("test-app", "");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_zero_ttl() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.initial_ttl = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_no_transports() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.transport.ble_enabled = false;
        config.transport.wifi_direct_enabled = false;
        config.transport.internet_enabled = false;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_retry_initial_delay_must_be_positive() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.reliability.retry.initial_delay_ms = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_retry_delay_bounds() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.reliability.retry.initial_delay_ms = 2000;
        config.reliability.retry.max_delay_ms = 1000;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_retry_backoff_multiplier_bounds() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.reliability.retry.backoff_multiplier = 0.5;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_rejects_zero_bloom_parameters() {
        let mutations: [fn(&mut DeduplicatorConfig); 4] = [
            |dedup: &mut DeduplicatorConfig| dedup.bloom_filter_bits = 0,
            |dedup: &mut DeduplicatorConfig| dedup.bloom_hash_count = 0,
            |dedup: &mut DeduplicatorConfig| dedup.bloom_filter_count = 0,
            |dedup: &mut DeduplicatorConfig| dedup.bloom_rotation_secs = 0,
        ];

        for mutate in mutations {
            let mut config = ProtocolConfig::new("test-app", "user123");
            config.reliability.dedup.use_bloom_filter = true;
            mutate(&mut config.reliability.dedup);
            assert!(config.validate().is_err());
        }
    }

    #[test]
    fn test_config_builder() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .ble_enabled(true)
            .wifi_direct_enabled(false)
            .online_first(true)
            .initial_ttl(10)
            .build()
            .unwrap();

        assert_eq!(config.app_id, "test-app");
        assert!(config.transport.ble_enabled);
        assert!(!config.transport.wifi_direct_enabled);
        assert!(config.dors.prefer_online);
        assert_eq!(config.initial_ttl, 10);
    }

    #[test]
    fn test_transport_config_default() {
        let transport = TransportConfig::default();
        assert!(transport.ble_enabled);
        assert!(transport.wifi_direct_enabled);
        assert!(transport.internet_enabled);
    }

    #[test]
    fn test_reliability_config_default() {
        let reliability = ReliabilityConfig::default();
        assert_eq!(reliability.ack.default_timeout_ms, 10000);
        assert_eq!(reliability.retry.max_retries, 10);
        // 7 days — must match the app-layer presence-flush window and the
        // fallback values hardcoded in the RN native bridges
        // (OfflineProtocolModule.kt / OfflineProtocolModule.swift).
        assert_eq!(reliability.retry.outbox_max_lifetime_ms, 604_800_000);
        // 5 min ceiling — also mirrored by the RN bridge fallbacks.
        assert_eq!(reliability.retry.max_delay_ms, 300_000);
        assert_eq!(reliability.dedup.max_tracked_messages, 1000);
    }

    #[test]
    fn test_encryption_config_default() {
        let encryption = EncryptionConfig::default();
        assert!(encryption.enabled);
        assert!(encryption.auto_key_exchange);
        assert!(encryption.store_pending);
        // SEC-M3: encryption is required by default; plaintext is an
        // explicit opt-out, never a silent fallback.
        assert!(encryption.require_encryption);
        assert_eq!(encryption.pending_queue.max_pending_per_peer, 64);
        assert_eq!(encryption.pending_queue.max_pending_global, 4096);
        assert_eq!(
            encryption.pending_queue.pending_ttl_ms,
            DEFAULT_PENDING_TTL_MS
        );
        assert_eq!(
            encryption.pending_queue.overflow_policy,
            OverflowPolicy::DropOldest
        );
    }

    #[test]
    fn test_encryption_config_disabled() {
        let encryption = EncryptionConfig::disabled();
        assert!(!encryption.enabled);
        assert!(!encryption.auto_key_exchange);
        assert!(!encryption.store_pending);
        assert!(!encryption.require_encryption);
    }

    #[test]
    fn test_config_builder_with_encryption() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .encryption_enabled(true)
            .auto_key_exchange(true)
            .store_pending_messages(false)
            .require_encryption(true)
            .pending_queue(PendingQueueConfig {
                max_pending_per_peer: 32,
                max_pending_global: 512,
                pending_ttl_ms: 30_000,
                overflow_policy: OverflowPolicy::DropNewest,
                ..Default::default()
            })
            .build()
            .unwrap();

        assert!(config.encryption.enabled);
        assert!(config.encryption.auto_key_exchange);
        assert!(!config.encryption.store_pending);
        assert!(config.encryption.require_encryption);
        assert_eq!(config.encryption.pending_queue.max_pending_per_peer, 32);
        assert_eq!(config.encryption.pending_queue.max_pending_global, 512);
        assert_eq!(config.encryption.pending_queue.pending_ttl_ms, 30_000);
        assert_eq!(
            config.encryption.pending_queue.overflow_policy,
            OverflowPolicy::DropNewest
        );
    }

    #[test]
    fn test_config_validation_require_encryption_requires_enabled() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.encryption.enabled = false;
        config.encryption.require_encryption = true;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_pending_queue_requires_positive_limits() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.encryption.pending_queue.max_pending_per_peer = 0;
        assert!(config.validate().is_err());

        let mut config = ProtocolConfig::new("test-app", "user123");
        config.encryption.pending_queue.max_pending_global = 0;
        assert!(config.validate().is_err());

        let mut config = ProtocolConfig::new("test-app", "user123");
        config.encryption.pending_queue.pending_ttl_ms = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_pending_queue_global_must_cover_per_peer() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.encryption.pending_queue.max_pending_per_peer = 100;
        config.encryption.pending_queue.max_pending_global = 99;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_group_config_default() {
        let group = GroupConfig::default();
        assert_eq!(group.max_group_members, 256);
        // Registration and broadcast both default on. Registration must stay
        // on because invite links resolve against the relay group registry.
        // The broadcast default is safe because the flag alone never selects
        // the path: it is additionally gated on the relay advertising the
        // `group_delivery_v2` capability, whose settled delivery report is
        // what gives the broadcast a per-recipient delivery contract.
        assert!(group.relay_enabled);
        assert!(group.relay_broadcast_enabled);
        // Commit enforcement must stay OFF by default. On, a member whose
        // admin overlay merely lagged would refuse a legitimate commit and
        // fork itself out of a healthy group with no attacker involved —
        // unrecoverable without an app-level re-invite. Flipping this default
        // is a fleet-wide partition decision, not a hardening tweak.
        assert!(!group.enforce_admin_commits);
    }

    #[test]
    fn test_group_enforce_admin_commits_builder_opt_in() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .group_enforce_admin_commits(true)
            .build()
            .unwrap();
        assert!(config.group.enforce_admin_commits);
        assert!(
            config.group.relay_enabled && config.group.relay_broadcast_enabled,
            "opting into enforcement must not disturb the relay flags"
        );
    }

    #[test]
    fn test_group_relay_broadcast_builder_opt_out() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .group_relay_broadcast_enabled(false)
            .build()
            .unwrap();
        assert!(!config.group.relay_broadcast_enabled);
        assert!(
            config.group.relay_enabled,
            "opting out of the broadcast must not disturb registration"
        );
    }

    #[test]
    fn test_config_has_default_group_config() {
        let config = ProtocolConfig::new("test-app", "user123");
        assert_eq!(config.group.max_group_members, 256);
    }

    #[test]
    fn test_config_validation_zero_max_group_members() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.group.max_group_members = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_positive_max_group_members() {
        let mut config = ProtocolConfig::new("test-app", "user123");
        config.group.max_group_members = 1;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_builder_max_group_members() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .max_group_members(50)
            .build()
            .unwrap();
        assert_eq!(config.group.max_group_members, 50);
    }

    #[test]
    fn test_config_builder_group_config() {
        let config = ProtocolConfig::builder("test-app", "user123")
            .group(GroupConfig {
                max_group_members: 128,
                ..Default::default()
            })
            .build()
            .unwrap();
        assert_eq!(config.group.max_group_members, 128);
    }

    #[test]
    fn test_config_builder_zero_max_group_members_rejected() {
        let result = ProtocolConfig::builder("test-app", "user123")
            .max_group_members(0)
            .build();
        assert!(result.is_err());
    }

    /// The RN bridges never reach [`DEFAULT_PENDING_TTL_MS`]: each layer
    /// substitutes its own fallback when the app omits `pendingTtlMs`, and the
    /// JS layer always materializes the field before it crosses the bridge. So
    /// the Rust default is only the RN default for as long as these three
    /// literals agree with it — they silently drifted to the pre-deferred-ACK
    /// 2-minute window once already, which is the window this constant was
    /// raised to replace.
    ///
    /// The sibling guard for the ACK/retry fallbacks lives in
    /// `offline-protocol-reliability`; this one has to live here because that
    /// crate cannot depend on this one.
    #[test]
    fn rn_bridge_pending_ttl_fallbacks_match_rust_default() {
        let rn_root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bindings/react-native");

        // Each entry is (path, the fallback that file must contain). The
        // pattern names the field it guards, not just the literal, so deleting
        // the fallback cannot be masked by the same number appearing on some
        // other line. Matched against the source with whitespace squeezed, so a
        // line wrap between the field and its `?:`/`??` doesn't hide it.
        let expected = [
            (
                rn_root.join("src/index.ts"),
                format!("pendingTtlMs ?? {DEFAULT_PENDING_TTL_MS}"),
            ),
            (
                rn_root.join("android/src/main/java/com/offlineprotocol/ProtocolConfigParser.kt"),
                format!(
                    "\"pending_ttl_ms\") ?: {}L",
                    format_underscored(DEFAULT_PENDING_TTL_MS)
                ),
            ),
            (
                rn_root.join("ios/EncryptionConfigReader.swift"),
                format!(
                    "\"pending_ttl_ms\") ?? {}",
                    format_underscored(DEFAULT_PENDING_TTL_MS)
                ),
            ),
        ];

        // The guard only applies in the repo checkout; skip when the bindings
        // tree isn't present (e.g. a vendored crate). Read every file up front
        // so a moved or renamed bridge skips the whole guard rather than
        // silently dropping the checks that come after it.
        let Some(sources) = expected
            .iter()
            .map(|(path, _)| std::fs::read_to_string(path).ok())
            .collect::<Option<Vec<_>>>()
        else {
            eprintln!("bindings tree not present, skipping RN pending-TTL drift check");
            return;
        };

        for ((path, fallback), source) in expected.iter().zip(&sources) {
            let squeezed = source.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                squeezed.contains(fallback.as_str()),
                "RN pending-queue TTL fallback drifted from DEFAULT_PENDING_TTL_MS: \
                 expected `{fallback}` in {}",
                path.display()
            );
        }
    }

    /// Renders `1800000` as `1_800_000`, the form Kotlin and Swift use.
    fn format_underscored(value: u64) -> String {
        let digits = value.to_string();
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push('_');
            }
            out.push(c);
        }
        out
    }
}
