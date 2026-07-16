//! Type definitions, constants, and shared state for the protocol engine.

use crate::events::{Event, EventCallback, PresenceStatus};
use crate::telemetry::{dispatch_record, scrub_event, TelemetryContext, TelemetryRecord};
use crate::Error;
use chrono::{DateTime, Utc};
use offline_protocol_core::{
    ContentType, ForwardInfo, MediaMetadata, Message, MessageId, MessagePriority,
};
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Retry interval for persisting session confirmation after a transient storage error.
pub(crate) const CONFIRMATION_RETRY_INTERVAL_SECS: i64 = 5;
/// Probe interval for reconciling pending sessions after restart.
pub(crate) const CONFIRMATION_PROBE_INTERVAL_SECS: i64 = 5;
/// Number of welcome retry records processed per tick.
pub(crate) const WELCOME_RETRY_BATCH_SIZE: usize = 20;
/// Hard TTL for outbound welcome lifecycle records.
pub(crate) const WELCOME_LIFECYCLE_TTL_SECS: i64 = 300;
/// Jitter ratio applied to welcome retry backoff delays.
pub(crate) const WELCOME_RETRY_JITTER_RATIO: f64 = 0.2;
/// Timeout waiting for explicit internet send confirmation for welcome.
pub(crate) const WELCOME_INTERNET_CONFIRM_TIMEOUT_SECS: i64 = 10;
/// Timeout waiting for a mesh (BLE / WiFi-Direct) welcome to be confirmed by
/// the peer proving the session (probe / ack / welcome / decrypt). A mesh
/// `send()` returning Ok only means the local stack accepted the bytes, not
/// that the multi-fragment Welcome reassembled on the peer — so the lifecycle
/// stays non-terminal and the retry queue re-sends the whole Welcome after this
/// window, recovering from a lost fragment. Slightly longer than the internet
/// timeout to allow a slow multi-fragment Welcome to assemble plus the probe
/// round-trip before paying to re-fragment and re-send it.
pub(crate) const WELCOME_MESH_CONFIRM_TIMEOUT_SECS: i64 = 15;
/// Retry cadence for a Welcome that has no transport carrier at all (the peer is
/// simply unreachable right now). A send is guaranteed to fail with no carrier,
/// so we do NOT burn the speculative send/transition/event churn on it every
/// retry tick — we park it and re-check on this slow interval instead. The
/// primary recovery is event-driven (`on_neighbor_discovered` → re-arm fires the
/// instant a carrier surfaces the peer); this poll is only a safety net for a
/// carrier returning without a fresh discovery event, so it is deliberately far
/// slower than the data-plane retry interval to keep an offline device quiet.
pub(crate) const WELCOME_NO_CARRIER_RETRY_SECS: i64 = 15;
/// Cap for the escalating retry interval of a welcome repeatedly parked
/// `PeerUnreachable` while a mesh carrier is up (see
/// `apply_recipient_unreachable_failure`). Each consecutive unreachable park
/// doubles the interval from [`WELCOME_NO_CARRIER_RETRY_SECS`] up to this
/// cap: DORS may keep selecting the internet path for the timed retry, and
/// every such round trips another relay `DeliveryError` with the attempt
/// refunded — without escalation that is an unbounded 15s resend loop into
/// the relay for as long as the peer stays offline. At the cap the steady
/// state matches the presence-rescue cadence (one send per 10 min), which is
/// cheap and self-resolving.
pub(crate) const WELCOME_UNREACHABLE_RETRY_CAP_SECS: i64 = 600;
/// Age limit for a welcome lifecycle to keep its peer on the presence
/// watchlist (`welcome_pending_peers`). Without it the watch set only ever
/// grows: every offline presence answer re-parks the record and pushes its
/// `expires_at`, so a permanently-dead peer (abandoned install) is watched —
/// and its parked lifecycle persisted — forever, and each such peer occupies
/// rotation slots that delay presence rescue for live peers. Once unwatched,
/// offline answers stop, `expires_at` stops being pushed, and the record
/// ages out through normal expiry; recovery degrades to peer-initiated
/// contact or mesh discovery, both of which rebuild the lifecycle.
pub(crate) const WELCOME_WATCHLIST_MAX_AGE_SECS: i64 = 14 * 24 * 60 * 60;
/// Backoff base for presence-driven welcome rescue. The first rescue for a
/// peer is immediate; each subsequent rescue that still fails to prove the
/// session doubles the wait (40s, 80s, 160s, ...) up to
/// [`WELCOME_PRESENCE_RESCUE_MAX_SECS`]. Bounds the resend loop when a peer
/// is provably online but can never confirm (stale key package after a
/// reinstall, incompatible peer version) — without it the platform's 20s
/// presence watch would re-send the multi-frame MLS welcome forever.
pub(crate) const WELCOME_PRESENCE_RESCUE_BASE_SECS: i64 = 40;
/// Cap for the presence-rescue backoff (10 minutes). Deliberately not a
/// terminal state: a peer that stays online but never confirms keeps getting
/// one rescue per cap interval, forever. That steady state is cheap (one
/// multi-frame welcome per 10 min per such peer) and self-resolving — the
/// lifecycle disappears the moment the session confirms — so a give-up
/// threshold would only add a way to strand a recoverable session.
pub(crate) const WELCOME_PRESENCE_RESCUE_MAX_SECS: i64 = 600;
/// Well-known prefix for transport send-failure reasons meaning "the carrier
/// is up but this recipient is unreachable on it" (e.g. the internet relay
/// answered `DeliveryError` for an offline peer). Classified in
/// `on_transport_send_failed` as authoritative proof the frame was dropped:
/// a welcome parks pending a reachability edge (no timed retry — the carrier
/// being healthy means a timer would just re-send into another
/// `DeliveryError`) instead of burning a retry attempt.
///
/// Cross-layer contract: the React Native platform bridges
/// (`InternetManager.kt` / `InternetManager.swift`) hardcode this literal when
/// calling `internet_send_failed_with_reason` — keep them in sync.
pub(crate) const SEND_FAIL_REASON_RECIPIENT_UNREACHABLE: &str = "recipient_unreachable";
/// Minimum interval between session reconciliation scans (list_sessions I/O).
/// Keeps the expensive Keychain/Keystore I/O out of the hot path so that
/// sendMessage() is not blocked by Mutex contention on every process tick.
pub(crate) const RECONCILIATION_THROTTLE_MS: u64 = 2_000;
/// Lamport clock ticks between storage persistence writes. Avoids a
/// Keychain/Keystore write on every sent and received message. On crash
/// recovery, at most this many ticks are lost, which is safe — the clock
/// is only used for causal ordering and the gap is absorbed on the next
/// merge with any peer.
pub(crate) const LAMPORT_PERSIST_INTERVAL: u64 = 64;
pub(crate) const MEDIA_TRANSFER_STALE_TIMEOUT_SECS: u64 = 300;
/// Maximum number of tracked known peers for service discovery.
pub(crate) const MAX_KNOWN_PEERS: usize = 1000;
/// How long a known peer stays tracked without being re-seen.
///
/// Peers on carriers with no disconnect signal (Internet, Reticulum, Nostr,
/// and WiFi Direct message-path senders) are only ever *added* to
/// `known_peers`; this TTL is their eviction path, swept from the periodic
/// `cleanup_expired_entries` tick. A second layer — least-recently-seen
/// eviction when an insert hits `MAX_KNOWN_PEERS` — guarantees a newly
/// discovered peer is always tracked even between sweeps.
///
/// Deliberately generous: a connected-but-quiet BLE peer refreshes only via
/// platform advertisement sightings (BLE inbound messages do not route
/// through the discovery hook), so a short TTL would evict it while still
/// connected. Eviction is self-healing (the peer is re-tracked on its next
/// advertisement or message), so erring long only delays hygiene.
pub(crate) const KNOWN_PEER_TTL_SECS: u64 = 1800;

/// Metadata key for the Ed25519 signature over the control message content (base64).
pub(crate) const CTRL_SIG_META_KEY: &str = "__ctrl_sig";
/// Metadata key for the sender's Ed25519 public key (base64, 32 bytes raw).
pub(crate) const CTRL_PK_META_KEY: &str = "__ctrl_pk";

/// Domain separator prepended to the canonical signing payload.
///
/// Prevents cross-context signature reuse: a signature produced for control
/// messages cannot be replayed in a future protocol extension that reuses the
/// same MLS identity key but with a different domain separator.
pub(crate) const CTRL_SIGN_DOMAIN: &[u8] = b"offline-ctrl-v1";

/// Maximum number of TOFU-pinned peer public keys to retain.
///
/// Entries are persisted via `MlsStorage` (when available) so pinned keys
/// survive process restarts and prevent key-substitution during re-pinning.
///
/// When a peer legitimately re-initializes MLS (e.g. app reinstall), the
/// application should call `reset_tofu_for_peer()` to allow re-pinning
/// with the new key. A signed key-rotation protocol may be added in a
/// future version for automatic cross-device key updates.
pub(crate) const MAX_TOFU_PEERS: usize = 1000;

/// Maximum number of blocked users to retain.
pub(crate) const MAX_BLOCKED_USERS: usize = 10_000;

/// Maximum number of peers tracked for once-per-peer
/// `PlaintextReceiveRejected` warning suppression.
///
/// The keys are wire-claimed (attacker-controllable) sender ids, so the set
/// resets at capacity instead of growing without bound: a flood of forged
/// senders degrades the throttle to once-per-peer-per-generation while
/// memory stays capped.
pub(crate) const MAX_PLAINTEXT_RECEIVE_WARNED_PEERS: usize = 1000;

/// Maximum number of pending (received-but-unused) peer key packages retained
/// in memory and in durable `MlsStorage`.
///
/// Keyed by the wire-claimed `sender`, so — like [`MAX_KNOWN_PEERS`] — an
/// unpinned peer can flood distinct forged ids under the default config. Each
/// entry is also written to durable secure storage (`persist_peer_key_package`,
/// iOS Keychain / Android Keystore) and re-loaded on boot, so without this cap
/// a flood grows durable storage without bound and re-inflates memory on every
/// restart. At capacity a new peer evicts the soonest-to-expire entry and drops
/// its persisted copy; the restore-on-boot loop is capped the same way.
pub(crate) const MAX_PENDING_KEY_PACKAGES: usize = 1000;

/// Maximum number of peers remembered in `key_package_sent_to` (the "already
/// sent our key package to this peer" set).
///
/// Wire-claimed ids grow this set in lockstep with a key-package flood, so it
/// resets at capacity like [`MAX_PLAINTEXT_RECEIVE_WARNED_PEERS`]: the only cost
/// of forgetting a peer is a one-time idempotent re-send of our key package.
pub(crate) const MAX_KEY_PACKAGE_SENT_TO: usize = 1000;

/// Maximum lifetime (in milliseconds) honored for a *received* peer key
/// package's cached expiry.
///
/// `remaining_lifetime_ms` arrives on the wire unauthenticated and becomes the
/// eviction sort key for [`MAX_PENDING_KEY_PACKAGES`] (the soonest-to-expire
/// entry is evicted first). Without a ceiling, a flood of forged senders
/// claiming a maximal lifetime would pin their entries as latest-to-expire and
/// preferentially evict legitimate peers. Key packages are minted with a 30-day
/// lifetime (`DEFAULT_KEY_PACKAGE_LIFETIME_SECS` in `offline-protocol-mls`), so
/// a larger value is never legitimate. This bound is purely cache bookkeeping —
/// OpenMLS enforces real key-package validity at use time — so clamping only
/// affects when we drop the *cached* copy, never crypto correctness.
pub(crate) const MAX_KEY_PACKAGE_LIFETIME_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// Minimum age (in milliseconds) a TOFU entry must have before it can be
/// evicted by LRU. This prevents a cache-filling attack where an adversary
/// rapidly registers many fake identities to evict legitimate pinned keys.
///
/// Set to 1 hour.
pub(crate) const TOFU_MIN_EVICTION_AGE_MS: i64 = 3_600_000;

/// Entry in the TOFU key store, pairing the peer's public key with a
/// last-seen timestamp used for LRU eviction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TofuEntry {
    pub(crate) public_key: Vec<u8>,
    /// Milliseconds since epoch (UTC) when we last verified a signed message
    /// from this peer.
    pub(crate) last_seen_ms: i64,
}

/// Payload for key package exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KeyPackagePayload {
    /// User ID of the key package owner.
    pub(crate) user_id: String,
    /// Raw key package data.
    pub(crate) key_package_data: Vec<u8>,
    /// Remaining valid lifetime in milliseconds (relative, not absolute).
    /// Receiver applies this to their local clock, avoiding clock skew issues.
    #[serde(default)]
    pub(crate) remaining_lifetime_ms: u64,
    /// Legacy absolute timestamp field — ignored on receive, kept for
    /// backward compatibility with old nodes that may still send it.
    #[serde(default)]
    pub(crate) timestamp_ms: u64,
    /// When `true`, the sender has reset their MLS session state and the
    /// receiver should discard any existing session for this peer before
    /// establishing a new one.
    ///
    /// Primary use-case: post-unblock session convergence. When Alice unblocks
    /// Bob, Alice's side deletes her MLS session and sends a fresh key package
    /// with `session_reset: true`. Bob deletes his now-orphaned session and
    /// auto-establishes a new one from Alice's key package, so both sides
    /// converge on a single fresh MLS group.
    #[serde(default)]
    pub(crate) session_reset: bool,

    /// Wire-format versions the sender can decode (e.g. `[1]` for binary v1).
    /// Absent on legacy nodes (`#[serde(default)]` → empty → JSON only), so an
    /// old peer is never sent a binary frame it cannot parse.
    ///
    /// Trust boundary: this rides in the plaintext `KeyPackagePayload` envelope
    /// *alongside* the signed MLS `key_package_data`, not *inside* the
    /// signature, so it is not cryptographically bound to the sender. A MITM on
    /// the pre-session bootstrap could strip it (harmless JSON downgrade) or
    /// forge `[1]` onto a legacy peer (making us emit binary that peer drops —
    /// a targeted delivery DoS). This grants no new capability: such an attacker
    /// already controls key-package delivery and could deny service outright.
    /// The negotiation is a performance optimization, never a security control.
    #[serde(default)]
    pub(crate) wire_versions: Vec<u8>,

    /// MLS envelope formats the sender can parse (e.g. `[1]` for the compact
    /// envelope, [`MLS_ENVELOPE_COMPACT_V1`]). Absent on legacy nodes
    /// (`#[serde(default)]` → empty → legacy JSON envelope only), so an old
    /// peer is never sent an envelope it cannot parse.
    ///
    /// Distinct from `wire_versions`: that one is hop-local (which *frames*
    /// the peer decodes), this one is end-to-end (which `__MLS_ENC__` payload
    /// encodings the *recipient* parses after any number of relay hops).
    ///
    /// Trust boundary: identical to `wire_versions` above — a plaintext
    /// envelope field, not signature-bound. Stripping it downgrades to the
    /// JSON envelope (harmless); forging it onto a legacy peer makes us emit
    /// envelopes that peer rejects with a `message_decryption_failed`
    /// event (a targeted delivery DoS an attacker in that position already
    /// has). A performance optimization, never a security control.
    #[serde(default)]
    pub(crate) env_versions: Vec<u8>,
}

/// Compact MLS envelope version advertised in
/// [`KeyPackagePayload::env_versions`]: the `__MLS_ENC__` payload is base64 of
/// [`offline_protocol_mls::EncryptedMessage::to_bytes`] instead of the legacy
/// JSON form (whose `ciphertext` field renders as a ~3.6x integer array).
/// Receivers distinguish the two by the byte after the prefix — `{` opens the
/// JSON envelope and never occurs in base64.
pub(crate) const MLS_ENVELOPE_COMPACT_V1: u8 = 1;

/// An outbound connection request awaiting a transport outcome (see
/// `OfflineProtocol::pending_connection_requests`).
#[derive(Debug, Clone)]
pub(crate) struct PendingConnectionRequest {
    /// Recipient the request was addressed to.
    pub(crate) recipient: String,
    /// When the request was sent — entries older than
    /// [`PENDING_CONNECTION_REQUEST_TTL`] are pruned on insert.
    pub(crate) sent_at: std::time::Instant,
}

/// How long an outbound connection request stays correlatable to a
/// transport failure. Past this window the entry is pruned: a DeliveryError
/// that stale belongs to a request the app has long stopped showing a
/// spinner for.
///
/// Deliberately wider than the bridges' 60s `RecipientInFlightTracker` TTL:
/// that window anchors at the socket write, while this one anchors at
/// `send_connection_request` — a request can dwell in the internet outbox
/// (device offline, relay reconnecting) before its wire attempt, and this
/// window must cover that dwell plus the bridge's correlation window.
pub(crate) const PENDING_CONNECTION_REQUEST_TTL: std::time::Duration =
    std::time::Duration::from_secs(300);

/// Cap on tracked outbound connection requests (oldest evicted first).
pub(crate) const MAX_PENDING_CONNECTION_REQUESTS: usize = 64;

/// Upper bound (UTF-8 bytes) for `initial_message` on a connection request.
/// The request is a plaintext High-priority control frame; an unbounded
/// first message would fragment heavily over BLE and can exceed relay frame
/// limits after the SDK already returned a message id, so oversized input
/// fails loudly at the API instead.
pub(crate) const MAX_INITIAL_MESSAGE_BYTES: usize = 4096;

/// Payload for a connection request message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ConnectionRequestPayload {
    /// Display name of the sender.
    pub(crate) sender_name: String,
    /// Timestamp of the request (Unix ms).
    pub(crate) timestamp_ms: i64,
    /// Optional MLS key package data for encrypted session setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) key_package: Option<Vec<u8>>,
    /// Optional first message sent along with the request (`default` keeps
    /// payloads from pre-initial-message senders parseable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) initial_message: Option<String>,
}

/// Payload for a connection accepted message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ConnectionAcceptedPayload {
    /// Display name of the accepting party.
    pub(crate) accepted_by_name: String,
    /// Timestamp of the acceptance (Unix ms).
    #[serde(default)]
    pub(crate) timestamp_ms: i64,
    /// Optional MLS key package data for encrypted session setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) key_package: Option<Vec<u8>>,
}

// --- Presence, typing, and read receipt payloads ---

/// Maximum number of message IDs allowed in a single read receipt.
pub(crate) const MAX_READ_RECEIPT_IDS: usize = 256;

/// Payload for a presence update message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PresencePayload {
    /// Presence status.
    pub(crate) status: PresenceStatus,
    /// Timestamp of the update (Unix ms).
    pub(crate) timestamp_ms: i64,
}

/// Payload for a typing indicator message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TypingIndicatorPayload {
    /// Conversation identifier (recipient username for DMs, group_id for groups).
    pub(crate) conversation_id: String,
    /// Whether the user is currently typing.
    pub(crate) is_typing: bool,
    /// Timestamp of the indicator (Unix ms).
    pub(crate) timestamp_ms: i64,
}

/// Payload for a read receipt message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReadReceiptPayload {
    /// IDs of the messages that were read.
    pub(crate) message_ids: Vec<String>,
    /// Timestamp when the messages were read (Unix ms).
    pub(crate) timestamp_ms: i64,
}

// --- Group (relay) payloads ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupCreatedPayload {
    pub(crate) group_id: String,
    pub(crate) name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMessageReceivedPayload {
    pub(crate) group_id: String,
    pub(crate) sender: String,
    pub(crate) content: String,
    pub(crate) timestamp: String,
    pub(crate) message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reply_to_msg: Option<String>,
    /// Forwarding attribution (present when the group message was forwarded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) forward_info: Option<offline_protocol_core::ForwardInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMemberAddedPayload {
    pub(crate) group_id: String,
    pub(crate) user_id: String,
    pub(crate) added_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) group_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMemberRemovedPayload {
    pub(crate) group_id: String,
    pub(crate) user_id: String,
    pub(crate) removed_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupInfoMemberPayload {
    pub(crate) user_id: String,
    pub(crate) role: String,
    pub(crate) joined_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupInfoPayload {
    pub(crate) group_id: String,
    pub(crate) name: String,
    pub(crate) created_by: String,
    pub(crate) created_at: String,
    pub(crate) members: Vec<GroupInfoMemberPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UserGroupSummaryPayload {
    pub(crate) group_id: String,
    pub(crate) name: String,
    pub(crate) created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UserGroupsPayload {
    pub(crate) groups: Vec<UserGroupSummaryPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupErrorPayload {
    pub(crate) reason: String,
    /// Group the error concerns, when the relay scoped it (e.g. a
    /// registration sync denial). Used to drop the group from
    /// `relay_synced` so sends fall back to per-member delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) group_id: Option<String>,
}

/// A received key package awaiting use for session creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReceivedKeyPackage {
    /// Raw MLS key package bytes.
    pub(crate) key_package_data: Vec<u8>,
    /// Local wall-clock deadline (ms since epoch) computed from the sender's
    /// `remaining_lifetime_ms`, anchored to *our* clock at receive time.
    pub(crate) local_expires_at_ms: u64,
}

/// Result of processing an internal protocol message.
#[derive(Debug)]
pub(crate) enum InternalMessageResult {
    /// Message was consumed internally (don't surface to app).
    Consumed,
    /// Message was rejected by the security gate (spoofed sender, bad
    /// signature, TOFU violation, etc.). Like `Consumed`, the message is not
    /// surfaced to the app — but unlike `Consumed`, a delivery ACK must NOT
    /// be sent back, to avoid confirming to the attacker that the target is
    /// online and processing messages.
    SecurityRejected,
    /// Message was decrypted, here's the plaintext.
    Decrypted(String),
}

/// Pending message waiting for session establishment.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct PendingMessage {
    /// Original plaintext content.
    pub(crate) content: String,
    /// Message priority.
    pub(crate) priority: MessagePriority,
    /// Message ID (preserved from initial creation).
    pub(crate) message_id: MessageId,
    /// Reply-to message ID if applicable.
    pub(crate) reply_to_msg: Option<MessageId>,
    /// Forwarding attribution (preserved so it survives the pending queue).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) forwarded_from: Option<ForwardInfo>,
    /// Content type of the original message (preserved for forwarded non-text messages).
    #[serde(default)]
    pub(crate) content_type: ContentType,
    /// Media metadata (preserved for forwarded media messages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) media_metadata: Option<MediaMetadata>,
    /// When the message was queued (for future TTL/expiry support).
    pub(crate) queued_at: DateTime<Utc>,
}

/// Durable state for a peer MLS session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SessionState {
    Pending,
    Confirmed,
}

impl SessionState {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Confirmed => "Confirmed",
        }
    }
}

/// Durable lifecycle states for outbound Welcome delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum WelcomeDeliveryState {
    Created,
    SendAttempted,
    Sent,
    Failed,
    Expired,
}

impl WelcomeDeliveryState {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "Created",
            Self::SendAttempted => "SendAttempted",
            Self::Sent => "Sent",
            Self::Failed => "Failed",
            Self::Expired => "Expired",
        }
    }
}

/// Per-peer throttle for presence-driven welcome rescue (see
/// `OfflineProtocol::on_peer_presence`). Deliberately in-memory only: a
/// restart resets the backoff, and the one free rescue that buys is useful
/// after a restart anyway.
#[derive(Debug, Clone)]
pub(crate) struct PresenceRescueThrottle {
    pub(crate) next_allowed_at: DateTime<Utc>,
    /// Consecutive rescues without the session confirming; drives the
    /// exponential backoff exponent.
    pub(crate) rescues: u32,
}

/// Durable metadata for outbound Welcome reliability handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WelcomeLifecycleRecord {
    pub(crate) peer_id: String,
    pub(crate) group_id: String,
    pub(crate) state: WelcomeDeliveryState,
    pub(crate) attempt: u32,
    /// Consecutive `PeerUnreachable` parks (relay `DeliveryError` verdicts)
    /// since the last reachability edge; drives the escalating retry
    /// interval capped at [`WELCOME_UNREACHABLE_RETRY_CAP_SECS`]. Reset on
    /// re-arm (presence online / neighbor discovered). Defaulted for
    /// records persisted before the field existed.
    #[serde(default)]
    pub(crate) unreachable_parks: u32,
    pub(crate) welcome_message: Message,
    pub(crate) next_retry_at: Option<DateTime<Utc>>,
    pub(crate) last_reason_code: Option<crate::events::WelcomeReasonCode>,
    pub(crate) last_transport_error: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) expires_at: DateTime<Utc>,
}

/// Storage key types for message persistence.
pub(crate) mod storage_keys {
    /// Key type for pending encrypted messages.
    pub const PENDING_MESSAGES: &str = "pending_messages";
    /// Key type for persisted per-peer MLS session confirmation state.
    pub const SESSION_STATES: &str = "session_states";
    /// Key type for persisted per-peer received key packages (survives restart).
    pub const PEER_KEY_PACKAGES: &str = "peer_key_packages";
    /// Key type for persisted per-peer outbound welcome lifecycle state.
    pub const WELCOME_LIFECYCLES: &str = "welcome_lifecycles";
    /// Key type for persisted store-and-forward outbox entries, keyed by
    /// message id. Holds only main-outbox (non-media) entries so undelivered
    /// messages survive app restarts; the media (file-chunk) outbox is
    /// intentionally excluded because file transfers are not persisted and
    /// must be re-initiated by the app after a restart.
    pub const OUTBOX: &str = "outbox";
    /// Key type for the Lamport clock value.
    pub const LAMPORT_CLOCK: &str = "lamport_clock";
    /// Key ID for the single Lamport clock entry.
    pub const LAMPORT_CLOCK_ID: &str = "current";
    /// Key type for persisted TOFU (Trust-On-First-Use) peer public keys.
    pub const TOFU_KEYS: &str = "tofu_keys";
    /// Key type for persisted blocked user entries.
    pub const BLOCKED_USERS: &str = "blocked_users";
    /// Key type for the persistent per-install telemetry scrub secret.
    pub const SCRUB_SECRET: &str = "scrub_secret";
    /// Key ID for the single scrub-secret entry.
    pub const SCRUB_SECRET_ID: &str = "current";
    /// Key type for the persistent per-install Nostr transport signing secret.
    pub const NOSTR_SIGNING_SECRET: &str = "nostr_signing_secret";
    /// Key ID for the single Nostr signing-secret entry.
    pub const NOSTR_SIGNING_SECRET_ID: &str = "current";
    /// Key type for peers we are the both-create "owner" of and are awaiting a
    /// group-aware decrypt from before confirming (see
    /// [`crate::protocol::OfflineProtocol`]'s `both_create_awaiting_decrypt`).
    /// Persisted so an owner restart mid-convergence cannot let a stale plaintext
    /// probe/ack prematurely confirm and strand the peer on a divergent group.
    pub const BOTH_CREATE_AWAITING_DECRYPT: &str = "both_create_awaiting_decrypt";
}

/// Protocol state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolState {
    /// Protocol is not started.
    Stopped,
    /// Protocol is running.
    Running,
    /// Protocol is paused (background mode).
    Paused,
}

/// Shared state protected by mutex.
pub(crate) struct SharedState {
    /// Current protocol state.
    pub(crate) state: ProtocolState,

    /// Event handlers registered by the application.
    pub(crate) event_handlers: Vec<EventCallback>,

    /// Received messages queue.
    pub(crate) received_messages: VecDeque<Message>,

    /// Installed telemetry context. When present, `emit_event` additionally
    /// forwards every protocol event to `ctx.sink` as a
    /// `TelemetryRecord::Protocol`, with identifier scrubbing applied per
    /// `ctx.config`. Set via `OfflineProtocol::install_telemetry_sink`.
    pub(crate) telemetry: Option<Arc<TelemetryContext>>,
}

impl SharedState {
    pub(crate) fn new() -> Self {
        Self {
            state: ProtocolState::Stopped,
            event_handlers: Vec::new(),
            received_messages: VecDeque::new(),
            telemetry: None,
        }
    }

    pub(crate) fn emit_event(&self, event: Event) {
        // Legacy `EventCallback` handlers run first and receive the raw
        // event. This preserves the pre-telemetry contract — any app that
        // relied on `on_event` sees exactly what it used to.
        //
        // Each handler call is panic-isolated so a faulty handler cannot
        // unwind through this method while a `MutexGuard<SharedState>` is
        // live in the caller's frame — that would poison the shared-state
        // mutex and silently degrade every subsequent SDK operation.
        for handler in &self.event_handlers {
            let event_for_handler = event.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handler(event_for_handler);
            }));
            if result.is_err() {
                tracing::error!(
                    event = ?event,
                    "EventCallback panicked; continuing. Handlers must not panic — see OfflineProtocol::on_event.",
                );
            }
        }
        // Sink fan-out runs after, gated on an installed context. Identifier
        // fields are scrubbed per the installed config before crossing the
        // sink boundary so long-lived pseudonyms don't leak to third-party
        // sinks by default. When scrubbing is disabled
        // (`TelemetryConfig::with_scrub_ids(false)`), `scrub_event` returns
        // a borrowed reference and the sink sees the raw event.
        //
        // Dispatch goes through `dispatch_record` so a panicking sink is
        // caught and logged rather than unwinding through the caller's live
        // `MutexGuard<SharedState>` — see the helper's docstring.
        if let Some(ctx) = &self.telemetry {
            let scrubbed = scrub_event::scrub_event(&event, &ctx.scrubber);
            let record = TelemetryRecord::Protocol(Box::new(scrubbed.into_owned()));
            dispatch_record(&ctx.sink, &record);
        }
    }
}

/// Helper function to lock a mutex and convert poison errors to protocol errors.
pub(crate) fn lock_shared_state(
    state: &Arc<Mutex<SharedState>>,
) -> std::result::Result<std::sync::MutexGuard<'_, SharedState>, Error> {
    state
        .lock()
        .map_err(|_| Error::Other("Shared state mutex poisoned".to_string()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OutboxEntry {
    pub(crate) message: Message,
    pub(crate) attempt_count: u32,
    pub(crate) first_sent_at: DateTime<Utc>,
    pub(crate) last_sent_at: DateTime<Utc>,
    pub(crate) last_transport: Option<TransportType>,
}

#[derive(Clone)]
pub(crate) struct PendingMediaMetadataEntry {
    pub(crate) content_type: ContentType,
    pub(crate) media_metadata: Option<MediaMetadata>,
    pub(crate) last_updated_at: Instant,
    /// Sender of the file transfer (used to drain partial transfers on block).
    pub(crate) sender: String,
}

#[derive(Clone)]
pub(crate) struct OutboundMediaTransfer {
    pub(crate) content_type: ContentType,
    pub(crate) recipient: String,
    pub(crate) pinned_transport: TransportType,
    pub(crate) total_chunks: u32,
    pub(crate) delivered_chunks: HashSet<u32>,
    pub(crate) last_updated_at: Instant,
    pub(crate) media_metadata: Option<MediaMetadata>,
}

pub(crate) enum OutboundSendPreparation {
    Ready(String),
    Queued(MessageId),
}
