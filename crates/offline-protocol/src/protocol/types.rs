//! Type definitions, constants, and shared state for the protocol engine.

use crate::events::{Event, EventCallback, PresenceStatus};
use crate::telemetry::{dispatch_record, scrub_event, TelemetryContext, TelemetryRecord};
use crate::Error;
use chrono::{DateTime, Utc};
use offline_protocol_core::{
    ContentType, ForwardInfo, MediaMetadata, Message, MessageId, MessagePriority, ReplyContext,
};
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::warn;

/// Retry interval for persisting session confirmation after a transient storage error.
pub(crate) const CONFIRMATION_RETRY_INTERVAL_SECS: i64 = 5;
/// Probe interval for reconciling pending sessions after restart.
pub(crate) const CONFIRMATION_PROBE_INTERVAL_SECS: i64 = 5;
/// Minimum interval between 1:1 session re-keys for the same peer, triggered by
/// an epoch-desync decrypt failure. A re-key is a full teardown + key-package
/// re-exchange, so it is rate-limited well above the confirmation-probe cadence:
/// one legit desync heals in a single round-trip, and this bounds a peer
/// replaying stale-epoch ciphertext (or an injected wrong-epoch frame) to at
/// most one re-key per this window rather than a storm. The floor is enforced
/// unconditionally — a successful decrypt on the healed session does NOT reset
/// it — so an attacker cannot defeat it by interleaving replays with legit
/// traffic (see `schedule_session_rekey`).
pub(crate) const REKEY_INTERVAL_SECS: i64 = 30;
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
/// Also the base interval for the plain-DM unreachable reachability probe
/// (`handle_recipient_unreachable_for_message`), which shares this
/// escalation.
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
/// cheap and self-resolving. Shared by the plain-DM unreachable probe
/// (`handle_recipient_unreachable_for_message`, per-peer
/// `dm_unreachable_parks` counter) for the same reason.
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

    /// Sealed rich-payload versions the sender can parse (e.g. `[1]` for
    /// [`RICH_PAYLOAD_V1`]). Absent on legacy nodes (`#[serde(default)]` →
    /// empty → plain text only), so an old peer is never sent a
    /// `__RICH_V1__` body it would surface as raw JSON text.
    ///
    /// End-to-end like `env_versions` (what the *recipient* parses inside
    /// the decrypted MLS plaintext), not hop-local like `wire_versions`.
    ///
    /// Trust boundary: identical to the two fields above — a plaintext
    /// envelope field, not signature-bound. Stripping it downgrades to plain
    /// text with the rich extras dropped (harmless); forging it onto a
    /// legacy peer makes us seal bodies that peer renders as JSON text (a
    /// nuisance an attacker in that position could match by corrupting
    /// delivery outright). A feature negotiation, never a security control.
    #[serde(default)]
    pub(crate) rich_versions: Vec<u8>,
}

/// Compact MLS envelope version advertised in
/// [`KeyPackagePayload::env_versions`]: the `__MLS_ENC__` payload is base64 of
/// [`offline_protocol_mls::EncryptedMessage::to_bytes`] instead of the legacy
/// JSON form (whose `ciphertext` field renders as a ~3.6x integer array).
/// Receivers distinguish the two by the byte after the prefix — `{` opens the
/// JSON envelope and never occurs in base64.
pub(crate) const MLS_ENVELOPE_COMPACT_V1: u8 = 1;

/// Sealed rich-payload version advertised in
/// [`KeyPackagePayload::rich_versions`]: the decrypted MLS plaintext may be
/// `__RICH_V1__` + JSON of [`RichPayloadV1`], carrying reply context, rich
/// media metadata, and forward attribution inside the AEAD boundary instead
/// of on the relay-visible outer message.
pub(crate) const RICH_PAYLOAD_V1: u8 = 1;

/// Upper bound on the serialized size of the rich extras accepted by the
/// rich send surface. Enforced at the API boundary (`send_message_with`),
/// deliberately NOT at seal time: a message queued behind session
/// establishment re-makes the seal decision at flush, and a seal-time
/// failure there would re-queue the message forever. Bounding at the
/// boundary means every queued extras blob is already known to seal.
pub(crate) const MAX_RICH_EXTRAS_BYTES: usize = 32 * 1024;

/// Rich fields accepted by the `send_message_with` surface. Only ever
/// delivered inside the sealed [`RichPayloadV1`] body — toward a recipient
/// that did not advertise [`RICH_PAYLOAD_V1`] they are silently dropped,
/// never sent cleartext.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RichSendExtras {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reply_context: Option<ReplyContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) media_metadata: Option<MediaMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) forward_info: Option<ForwardInfo>,
}

impl RichSendExtras {
    /// Whether any rich field is present (empty extras never seal).
    pub(crate) fn is_any(&self) -> bool {
        self.reply_context.is_some() || self.media_metadata.is_some() || self.forward_info.is_some()
    }

    /// Enforces [`MAX_RICH_EXTRAS_BYTES`] on the serialized extras. Shared
    /// by every rich send boundary (`send_message_with`, the forward paths,
    /// and the group surface) — enforced there, never at seal time, so a
    /// queued or re-sent message is always known to seal (see the
    /// constant's doc). Empty extras always pass.
    pub(crate) fn check_size(&self) -> crate::Result<()> {
        if !self.is_any() {
            return Ok(());
        }
        let extras_len = serde_json::to_vec(self)
            .map_err(|e| Error::Serialization(e.to_string()))?
            .len();
        if extras_len > MAX_RICH_EXTRAS_BYTES {
            return Err(Error::InvalidArgument(format!(
                "Rich extras too large: {} bytes serialized (max {})",
                extras_len, MAX_RICH_EXTRAS_BYTES
            )));
        }
        Ok(())
    }
}

/// Options for `OfflineProtocol::send_message_with`: priority and reply
/// threading (as on `send_message`), plus the rich fields introduced with
/// the sealed rich payload.
///
/// The rich fields (`reply_context`, `media_metadata`, `forward_info`) only
/// ever travel inside the MLS-sealed `__RICH_V1__` body, and only toward
/// recipients whose key package advertised `rich_versions` support. Toward
/// anyone else they are silently dropped — never sent cleartext — so the
/// message degrades to plain text with `reply_to_msg` threading intact.
#[derive(Debug, Clone, Default)]
pub struct SendMessageOptions {
    /// Message priority (defaults to Medium).
    pub priority: Option<MessagePriority>,
    /// ID of the message this is replying to.
    pub reply_to_msg: Option<String>,
    /// Content type stamped on the outer message (defaults to Text). A
    /// coarse rendering hint — the actual content stays MLS-sealed. Toward
    /// a recipient that advertised the sealed rich payload, a copy travels
    /// inside the sealed body — whenever extras seal, or the hint itself is
    /// non-Text — and the receiver treats that copy as authoritative, so a
    /// relay cannot rewrite the hint. Must not be
    /// [`ContentType::FileChunk`] (an internal transport content type; the
    /// receiver would route the message into its file-transfer manager and
    /// drop it) — rejected as `InvalidArgument`.
    pub content_type: Option<ContentType>,
    /// Quoted-reply context, delivered sealed-only.
    pub reply_context: Option<ReplyContext>,
    /// Rich media metadata (cloud attachments, stickers — including any
    /// `encryption_key`/`iv` secrets), delivered sealed-only.
    pub media_metadata: Option<MediaMetadata>,
    /// Forward attribution, delivered sealed-only.
    pub forward_info: Option<ForwardInfo>,
    /// Send via this specific transport (bypassing DORS selection), like
    /// `send_message_via_transport`.
    pub via_transport: Option<TransportType>,
}

/// Options for `OfflineProtocol::send_media_with`: the chunk-0 media
/// metadata (as on `send_media`), plus the rich fields introduced with the
/// sealed rich payload and an optional caller-supplied `file_id`.
///
/// The rich fields (`caption`, `reply_to_msg`, `reply_context`,
/// `forward_info`) only ever travel inside the MLS-sealed chunk-0 plaintext
/// (v2 media envelope), and only toward recipients whose key package
/// advertised `rich_versions` support. Toward anyone else — including every
/// plaintext (encryption opt-out) transfer — they are silently dropped,
/// never sent cleartext, so the transfer degrades to what plain
/// `send_media` sends.
#[derive(Debug, Clone, Default)]
pub struct MediaSendOptions {
    /// Media metadata delivered with chunk 0 (as on `send_media`).
    pub media_metadata: Option<MediaMetadata>,
    /// Caption text, delivered sealed-only.
    pub caption: Option<String>,
    /// ID of the message this media replies to, delivered sealed-only.
    pub reply_to_msg: Option<String>,
    /// Quoted-reply context, delivered sealed-only.
    pub reply_context: Option<ReplyContext>,
    /// Forward attribution, delivered sealed-only.
    pub forward_info: Option<ForwardInfo>,
    /// Caller-supplied file id for the transfer (minted when absent). Must
    /// not collide with an active outbound transfer; bounded to the wire
    /// `file_id` field limit.
    pub file_id: Option<String>,
}

/// The sealed rich body: what `__RICH_V1__` + JSON carries inside the MLS
/// plaintext. `text` is the user-visible content; the optional fields are
/// restored onto the inbound message by `apply_decrypted_content` *after*
/// its outer-field strip, making the sealed body the sole trusted carrier
/// for rich data on encrypted messages.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RichPayloadV1 {
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reply_context: Option<ReplyContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) media_metadata: Option<MediaMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) forward_info: Option<ForwardInfo>,
    /// Sealed copy of the outer `content_type` rendering hint — the last
    /// rich-adjacent field a relay could rewrite in transit. Additive after
    /// the body first shipped: absent from bodies sealed by older senders,
    /// in which case the outer value stands. When present it is
    /// authoritative on receive (except `FileChunk`, refused there like at
    /// the send boundary), so a relay can no longer restamp the rendering
    /// hint — or worse, restamp it `FileChunk` and get the decrypted
    /// message routed into the file-transfer manager and dropped. Fresh
    /// sends with a non-Text hint seal a hint-only body even without
    /// extras. Forwards seal their attribution and media metadata as
    /// extras toward capable recipients; only legacy queued forwards
    /// (persisted by older builds, outer-only) skip the hint-only seal,
    /// since a sealed body would wipe their outer copies on restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) content_type: Option<ContentType>,
}

impl RichPayloadV1 {
    /// Parses a sealed `__RICH_V1__` body out of decrypted MLS plaintext.
    ///
    /// Shared by the DM restore (`apply_decrypted_content`) and the group
    /// message inbound paths. Never capability-gated (mirroring envelope
    /// parsing): whatever a peer chose to seal, we try to read. Returns
    /// `None` when the prefix is absent or the body fails to parse (logged;
    /// callers surface the raw text rather than dropping an authenticated
    /// message). A sealed `FileChunk` content-type claim is refused here —
    /// mirroring the send boundary — so a hostile sender can never steer a
    /// decrypted message into the file-transfer manager, which would drop
    /// it.
    pub(crate) fn parse_sealed(plaintext: &str, sender: &str) -> Option<Self> {
        let sealed = plaintext.strip_prefix(super::internal_prefixes::RICH_V1)?;
        match serde_json::from_str::<Self>(sealed) {
            Ok(mut rich) => {
                if rich.content_type == Some(ContentType::FileChunk) {
                    warn!(
                        sender = %sender,
                        "Sealed rich payload claims the internal FileChunk content type, ignoring the hint"
                    );
                    rich.content_type = None;
                }
                Some(rich)
            }
            Err(e) => {
                warn!(
                    sender = %sender,
                    error = %e,
                    "Failed to parse sealed rich payload, surfacing raw text"
                );
                None
            }
        }
    }
}

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
///
/// Also deliberately wider than the worst-case default ACK retry schedule
/// (10 retries, 1s initial delay, x2 backoff capped at 5 min, plus a 10s
/// ACK timeout per attempt — up to ~910s end to end): retry exhaustion is
/// the last settlement point that can still emit a typed undeliverable
/// event, so the window must outlive it with headroom.
pub(crate) const PENDING_CONNECTION_REQUEST_TTL: std::time::Duration =
    std::time::Duration::from_secs(1800);

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

/// Durable record of the end-to-end capability versions a peer last
/// advertised in its key package (`env_versions` / `rich_versions`).
///
/// Persisted separately from [`ReceivedKeyPackage`] because the cached key
/// package is deleted once a session is established, while the capabilities
/// must survive restarts for exactly those peers: mobile apps restart
/// constantly and MLS sessions persist, so without this record a rich send
/// right after relaunch silently degrades to bare text (and the compact
/// envelope to JSON) until the next live key-package exchange.
///
/// Stores the raw advertised versions, not the config-gated subset: the kill
/// switches (`compact_envelope_enabled` / `rich_payload_enabled`) gate use —
/// live recording, restore, and send — not knowledge, so toggling one across
/// restarts behaves the same as toggling it live.
///
/// `wire_versions` is deliberately absent: it is hop-local (which frames a
/// directly-connected peer decodes), and connection setup re-exchanges key
/// packages on discovery anyway, so persisting it would buy nothing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PeerCapabilities {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) env_versions: Vec<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) rich_versions: Vec<u8>,
    /// Rich-payload versions a *group inviter* attested for this peer
    /// (carried on the group Add commit / Welcome), as opposed to the
    /// direct self-advertised `rich_versions` above. Kept separate because
    /// the trust differs: attestation is third-party and may be stale, so a
    /// direct key-package exchange overwrites the whole record (clearing
    /// this field) — direct knowledge is always authoritative. Consulted
    /// only by the group seal gate, never by DM sealing or envelope
    /// selection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) attested_rich_versions: Vec<u8>,
}

/// Cap on how many advertised version entries persist per capability list
/// in a [`PeerCapabilities`] record. The lists are unauthenticated wire
/// input stored raw (raw so unknown future versions survive a local
/// upgrade), and a hostile advertiser could otherwise bloat each durable
/// record up to the transport message size. Truncation only hurts the
/// advertiser — real senders list a handful of versions.
pub(crate) const MAX_PERSISTED_CAPABILITY_VERSIONS: usize = 8;

impl PeerCapabilities {
    /// Builds a record from the wire-advertised version lists, truncating
    /// each to [`MAX_PERSISTED_CAPABILITY_VERSIONS`].
    pub(crate) fn from_advertised(env_versions: &[u8], rich_versions: &[u8]) -> Self {
        Self {
            env_versions: env_versions
                .iter()
                .copied()
                .take(MAX_PERSISTED_CAPABILITY_VERSIONS)
                .collect(),
            rich_versions: rich_versions
                .iter()
                .copied()
                .take(MAX_PERSISTED_CAPABILITY_VERSIONS)
                .collect(),
            attested_rich_versions: Vec::new(),
        }
    }

    /// Whether any capability is advertised. Empty records are deleted
    /// rather than stored — the durable side of the downgrade semantics.
    pub(crate) fn is_any(&self) -> bool {
        !self.env_versions.is_empty()
            || !self.rich_versions.is_empty()
            || !self.attested_rich_versions.is_empty()
    }
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
    /// Message could not be decrypted *yet* because the MLS session/group is
    /// not established, so it was queued for delayed decryption
    /// (`enqueue_pending_decryption`). Unlike `Consumed`, a delivery ACK must
    /// NOT be sent and the id must NOT stay dedup-marked: the message is
    /// provably not delivered, so the receiver must leave the sender's retry
    /// lever intact. The receive loop responds by unmarking the id (so a
    /// resend re-enters processing instead of hitting the duplicate re-ACK
    /// path) and skipping the ACK. The queued copy is surfaced — and the id
    /// re-marked — once the session confirms and the queue drains
    /// (`process_pending_decryption`), which also sends the deferred delivery
    /// ACK directly on the recorded arrival transport (so a sender that gave up
    /// before the session confirmed still learns of delivery). See the
    /// deferred-ACK design in CLAUDE.md's MLS envelope notes.
    Deferred,
    /// Message was decrypted, here's the plaintext.
    Decrypted(String),
}

/// Outcome of routing an inbound file-chunk message through
/// [`OfflineProtocol::handle_incoming_file_chunk`]. Distinguishes chunks that
/// were dealt with terminally (decrypted/assembled, or dropped for a permanent
/// reason) from chunks queued for delayed decryption — so the receive loop can
/// defer the ACK for the latter, exactly like the text `Deferred` path.
///
/// [`OfflineProtocol::handle_incoming_file_chunk`]: crate::OfflineProtocol
///
/// `#[must_use]`: the ACK/defer decision hinges on this outcome. Dropping it on
/// the floor silently reverts to the pre-deferred-ACK behavior (always ACK,
/// leave dedup-marked), reintroducing the queue-path silent-loss bug — so every
/// caller must branch on it.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkOutcome {
    /// The chunk was decrypted/assembled or dropped for a terminal reason
    /// (parse failure, resource limit, crypto failure). The sender should stop
    /// retrying either way, so the caller ACKs as before.
    Handled,
    /// The chunk could not be decrypted yet (session not ready) and was queued
    /// for delayed decryption. The caller must NOT ACK and must unmark the id,
    /// so the sender keeps retrying and the resend re-enters processing.
    Deferred,
    /// The chunk was unencrypted and rejected by the encryption policy. Like
    /// [`InternalMessageResult::SecurityRejected`] for text, the caller must NOT
    /// ACK (don't confirm to an injector that the target processes their
    /// messages) and must unmark the id (so a replay re-enters this gate instead
    /// of the duplicate re-ACK path), matching the plaintext-text rejection in
    /// the receive loop.
    Rejected,
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
    /// Option-borne rich extras from the rich send surface. Kept separate
    /// from the legacy `forwarded_from`/`media_metadata` fields above: those
    /// flush as outer cleartext (shipped forward behavior), while these must
    /// flush inside the sealed rich body or be dropped — never cleartext.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rich: Option<RichSendExtras>,
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
    /// Key type for persisted per-peer advertised capability versions
    /// (env/rich), which must outlive the key package entry above — that one
    /// is deleted once a session is established (see
    /// [`super::PeerCapabilities`]).
    pub const PEER_CAPABILITIES: &str = "peer_capabilities";
    /// Key type for persisted per-peer outbound welcome lifecycle state.
    pub const WELCOME_LIFECYCLES: &str = "welcome_lifecycles";
    /// Key type for persisted store-and-forward outbox entries, keyed by
    /// message id. Holds only main-outbox (non-media) entries so undelivered
    /// messages survive app restarts; the media (file-chunk) outbox is
    /// intentionally excluded because file transfers are not persisted and
    /// must be re-initiated by the app after a restart.
    pub const OUTBOX: &str = "outbox";
    /// Key type for persisted outbound media transfer descriptors, keyed by
    /// file id. Descriptor-only (never chunk bytes): a descriptor surviving
    /// into a restore marks a transfer the app must re-initiate, surfaced
    /// via `MediaResendRequired`.
    pub const MEDIA_DESCRIPTORS: &str = "media_descriptors";
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

/// Provenance kept on an outbox entry so an encrypted DM can be *re-sealed*
/// against the peer's current MLS session on each resend, instead of replaying
/// the ciphertext bytes sealed at first send (which become permanently
/// undecryptable once the peer re-keys to a new epoch). Mirrors the fields the
/// pre-send [`PendingMessage`] carries, so both re-seal through the same
/// `prepare_outbound_content` chokepoint. Only populated for main-outbox
/// (non-media) encrypted DMs; absent (`None`) means verbatim replay — the
/// fallback for plaintext sends and media chunks.
///
/// **Memory-only by design.** This holds the message *plaintext*, so it is never
/// persisted (see the `#[serde(skip)]` on [`OutboxEntry::reseal`]): persisting it
/// would broaden plaintext-at-rest to every sent-but-unACKed encrypted DM for the
/// full outbox lifetime, weakening forward secrecy in exchange for only a narrow
/// cross-restart reseal benefit. After a restart the restored entry replays
/// verbatim; if that resend hits a desync, Tier 1 (un-ACK + re-key) still keeps
/// delivery honest rather than silently losing the message.
#[derive(Debug, Clone)]
pub(crate) struct OutboxReseal {
    /// Original plaintext content.
    pub(crate) content: String,
    /// Message priority.
    pub(crate) priority: MessagePriority,
    /// Reply-to message ID if applicable.
    pub(crate) reply_to_msg: Option<MessageId>,
    /// Forwarding attribution.
    pub(crate) forwarded_from: Option<ForwardInfo>,
    /// Content type of the original message.
    pub(crate) content_type: ContentType,
    /// Media metadata (cleartext-outer fallback provenance).
    pub(crate) media_metadata: Option<MediaMetadata>,
    /// Sealed-only rich extras (reply context, rich media metadata, forward
    /// info) — re-evaluated against current capability at re-seal time.
    pub(crate) rich: Option<RichSendExtras>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OutboxEntry {
    pub(crate) message: Message,
    pub(crate) attempt_count: u32,
    pub(crate) first_sent_at: DateTime<Utc>,
    pub(crate) last_sent_at: DateTime<Utc>,
    pub(crate) last_transport: Option<TransportType>,
    /// Re-seal provenance; `None` for verbatim-replay entries (plaintext or
    /// media). **Memory-only** (`#[serde(skip)]`): it carries the message
    /// plaintext, which must never be persisted (see [`OutboxReseal`]). A
    /// restored entry deserializes with `reseal: None` and therefore replays
    /// verbatim.
    #[serde(skip)]
    pub(crate) reseal: Option<OutboxReseal>,
}

#[derive(Clone)]
pub(crate) struct PendingMediaMetadataEntry {
    pub(crate) content_type: ContentType,
    pub(crate) media_metadata: Option<MediaMetadata>,
    pub(crate) last_updated_at: Instant,
    /// Sender of the file transfer (used to drain partial transfers on block).
    pub(crate) sender: String,
    /// Rich extras from the sealed chunk-0 plaintext. Never populated from
    /// wire (legacy plaintext) chunks — the sealed body is the only trusted
    /// carrier.
    pub(crate) rich_extras: Option<crate::media_envelope::MediaRichExtras>,
    /// The chunk-0 outer `Message` timestamp (wall-clock ms) — the sender's
    /// send time, surfaced on `FileReceived` for display ordering.
    pub(crate) timestamp_ms: i64,
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
    /// Rich extras sealed into chunk 0 (already capability-gated at the
    /// `send_media_with` boundary). Kept on the transfer because chunk
    /// batches are (re-)encoded via `pump_media_transfers` too.
    pub(crate) rich_extras: Option<crate::media_envelope::MediaRichExtras>,
}

/// Crash-scoped descriptor of an in-flight outbound media transfer.
///
/// Persisted (no chunk bytes — see commit 42d1b86's rationale: resurrected
/// chunks can never complete, and per-chunk secure-storage writes are
/// expensive) when a transfer starts and deleted whenever the in-memory
/// transfer is removed (completed, aborted, or stale-swept). A descriptor
/// found on restore therefore means the app died mid-transfer:
/// [`crate::events::Event::MediaResendRequired`] is emitted so the app can
/// re-supply the bytes via `send_media_with` under the same `file_id`,
/// validated against `file_checksum`.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct MediaTransferDescriptor {
    pub(crate) file_id: String,
    pub(crate) recipient: String,
    pub(crate) file_name: String,
    pub(crate) file_size: u64,
    /// SHA-256 hex of the plaintext file bytes (same value every chunk
    /// carries as `FileChunk::file_checksum`).
    pub(crate) file_checksum: String,
    pub(crate) content_type: ContentType,
    /// Wall-clock start of the transfer; restore prunes by
    /// `outbox_max_lifetime_ms` age.
    pub(crate) queued_at: DateTime<Utc>,
}

pub(crate) enum OutboundSendPreparation {
    Ready(String),
    Queued(MessageId),
}
