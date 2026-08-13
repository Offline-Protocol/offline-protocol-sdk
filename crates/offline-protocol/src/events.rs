//! Event types and callbacks.

use offline_protocol_core::{ContentType, MediaMetadata, Message, MessageId};
use offline_protocol_services::ServiceEvent;
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Presence status for a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    /// Peer is online and available.
    Online,
    /// Peer is away / idle.
    Away,
    /// Peer is explicitly offline.
    Offline,
}

/// Where a `PresenceUpdated` event came from.
///
/// Apps that render relay-style presence UI (e.g. a direct-chat header's
/// "Online" / "Last seen …") should filter on `Internet` so a nearby peer's
/// self-report can't flip a header that is defined as relay-observed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceSource {
    /// Relay-observed presence: the relay's authoritative answer to
    /// `CheckPresence` (`PresenceStatus` / `PresenceStatusWithLastSeen`),
    /// or relay-derived reachability — the platform bridges also report
    /// `offline` here when a relay `DeliveryError` names the recipient
    /// unreachable, so a failed send can produce an `Internet`-sourced
    /// offline event without any explicit presence query.
    Internet,
    /// A peer-sent `__PRESENCE__` self-report. Transport-agnostic: it may
    /// arrive over BLE, WiFi Direct, or even relay-forwarded frames — hence
    /// "peer", not "mesh". Default for deserializing legacy events that
    /// predate the field.
    #[default]
    Peer,
}

/// Event callback type for handling protocol events.
pub type EventCallback = Arc<dyn Fn(Event) + Send + Sync>;

/// Machine-readable reason taxonomy for welcome delivery failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WelcomeReasonCode {
    /// No transport was available or all transport sends failed.
    TransportUnavailable,
    /// The transport is up but this specific peer is unreachable on it
    /// (e.g. the internet relay reported the recipient offline). Treated as
    /// per-peer no-carrier: the welcome parks instead of aging.
    PeerUnreachable,
    /// Peer was disconnected or became unavailable during send.
    PeerDisconnected,
    /// Send operation timed out.
    Timeout,
    /// Local/internal error prevented send.
    InternalError,
    /// Retry budget or lifecycle TTL was exhausted.
    RetryExhausted,
}

impl WelcomeReasonCode {
    /// Returns the stable machine-readable reason code.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TransportUnavailable => "TRANSPORT_UNAVAILABLE",
            Self::PeerUnreachable => "PEER_UNREACHABLE",
            Self::PeerDisconnected => "PEER_DISCONNECTED",
            Self::Timeout => "TIMEOUT",
            Self::InternalError => "INTERNAL_ERROR",
            Self::RetryExhausted => "RETRY_EXHAUSTED",
        }
    }
}

/// Machine-readable taxonomy for [`Event::SecurityWarning`], so consumers can
/// branch on a stable code instead of matching the human-readable `reason`
/// string (which is for logs/UI and may change between versions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SecurityWarningCode {
    /// A control message's signature verified, but the key that signed it does
    /// not derive to the address the frame claims to come from.
    ///
    /// There is no benign reading. An address *is* the hash of its identity
    /// key, so a legitimate peer's frames always re-derive to their own
    /// address: a rotated key is a *different address*, i.e. a different peer,
    /// not the same peer with new bytes. This is either an impersonation
    /// attempt or a peer running a build with a broken derivation.
    ///
    /// Replaces `TOFU_KEY_MISMATCH`, whose remedy (`resetTofuForPeer`) no longer
    /// exists because there is no pin to reset.
    SenderAddressMismatch,
    /// A control message's signed sender did not match its transport peer
    /// identity.
    TransportIdentityMismatch,
    /// A control message's signature failed verification (invalid signature or
    /// malformed metadata).
    ControlSignatureInvalid,
    /// An unsigned control message was rejected.
    ///
    /// All security-gated control traffic must be signed by the key its sender
    /// address derives from. This is unconditional: it does not depend on
    /// `require_transport_identity`, nor on whether the sender was seen before.
    /// A peer that will not sign is making a claim that cannot be checked.
    UnsignedControlRejected,
    /// An encrypted media chunk's MLS group did not match the 1:1 session
    /// group of the claimed wire sender — a valid ciphertext (from some
    /// session) delivered under a different sender's name is a media
    /// forgery/misattribution attempt.
    MediaSenderGroupMismatch,
    /// An outbound message left this node as plaintext because encryption is
    /// disabled or MLS is uninitialized while `require_encryption` is `false`
    /// (an explicit opt-out — the default fails closed). Emitted at most once
    /// per peer per protocol instance.
    PlaintextSend,
    /// Inbound plaintext content (text or legacy media) was rejected by the
    /// encryption policy: either `require_encryption` is enabled, or a
    /// confirmed MLS session exists with the claimed sender and plaintext
    /// from that peer is a downgrade/forgery attempt (plaintext carries no
    /// sender authentication). Emitted at most once per peer; the per-peer
    /// tracking set is bounded, so after a flood of forged sender ids a
    /// peer may warn again.
    PlaintextReceiveRejected,
    /// An encrypted 1:1 envelope named an MLS group that is not the session
    /// slot shared with the claimed wire sender. The text-path counterpart to
    /// [`Self::MediaSenderGroupMismatch`]: it catches the same misattribution
    /// on paths where MLS cannot, because the envelope failed before it
    /// authenticated anything.
    SessionSenderGroupMismatch,
    /// A 1:1 MLS session was torn down and re-advertised after an epoch
    /// desync. Expected occasionally (a genuine fork heals this way), but the
    /// triggering frame is not authenticated — see `schedule_session_rekey` —
    /// so a sustained rate of these for one peer indicates injected frames
    /// rather than a real fork.
    SessionRekeyTriggered,
    /// A Nostr key-package publication slot could not be refilled, so cold
    /// first contact over Nostr is degraded or unavailable until it succeeds.
    ///
    /// Slots are consumed locally — an init key leaves provider storage only
    /// when *this* node processes a Welcome built against it — so consumption
    /// itself is normal traffic, and a stranger can drive it only by actually
    /// establishing sessions with us. What this warns about is the refill
    /// failing (MLS or storage error), which is the failure the publication
    /// design must never absorb silently: a stale published record makes every
    /// stranger who fetches it build a Welcome that can never be processed.
    NostrKeyPackageSlotExhausted,
    /// The per-peer key-package pool hit its ceiling, so a peer was advertised
    /// an init key another peer already holds.
    ///
    /// The push path normally gives each peer its own single-use init key. Past
    /// the ceiling it reuses one instead of growing the pool without bound —
    /// weakening forward secrecy at session establishment (one compromised init
    /// key opens every Welcome built against it) and re-introducing the race
    /// where the second peer to use a package finds it already consumed. Both
    /// clear as packages are consumed or expire.
    ///
    /// Reaching it means a device has accumulated unconsumed advertisements:
    /// many peers met that never established a session. Sustained emission is
    /// the signal that the ceiling, not peer churn, is now the binding
    /// constraint.
    PushKeyPackagePoolExhausted,
    /// The relay acknowledged an address declaration naming an address that is
    /// not this node's.
    ///
    /// There is no benign reading. The bridge declares `local_address()` and
    /// signs a proof over a per-connection challenge; the relay verifies that
    /// the declared address derives from the key that signed it, and echoes
    /// back what it bound. So the echo can only differ if the relay bound
    /// something other than what it verified — a broken or hostile relay, or a
    /// frame injected into the socket.
    ///
    /// Nothing is torn down: a relay that controls the socket already controls
    /// everything a local refusal could protect, so the mitigation is that the
    /// signal is loud rather than that the connection dies. What it means in
    /// practice is that this connection's frames are attributed to an identity
    /// this node cannot prove, so receivers strict-matching the sender against
    /// the transport identity will reject its security-gated control traffic.
    RelayAddressBindingMismatch,
    /// The relay refused this connection's address declaration, so it stays
    /// attributed by account name.
    ///
    /// Operational degradation, not an attack signal: the refusal path is
    /// deliberately non-fatal on both sides, and the connection keeps working
    /// exactly as it did before addresses existed. What is lost is *new* MLS
    /// session establishment over the relay — `__MLS_KEY_PKG__` and
    /// `__MLS_WELCOME__` are security-gated, so a receiver that strict-matches
    /// the account name against the `off1…` sender rejects them, while already
    /// established sessions keep working because `__MLS_ENC__` is data-plane.
    ///
    /// The relay's own text is **not** carried on the event. It is
    /// remote-chosen, and this event's `reason` is shipped verbatim by the
    /// telemetry scrubber, so the wording stays bounded in the device log
    /// while the code above is the classification. The causes it distinguishes
    /// range from unusable key material to this socket having been displaced
    /// by a newer login; none of them changes what an app should do. No retry
    /// is attempted — the next reconnect declares from scratch.
    RelayAddressDeclarationRefused,
    /// A group leaf carries a credential its own signature key does not derive
    /// to — an identity claimed inside a group without the key to prove it.
    ///
    /// There is no benign reading, and unlike most codes here it accuses
    /// someone the user is already in a room with. An address *is* the hash of
    /// its identity key, so every honest leaf reproduces its own credential;
    /// producing one that does not means deliberately building a leaf around
    /// someone else's name.
    ///
    /// `peer_id` is **who the finding concerns, not always who to blame**. On
    /// the three refusal sites it is the peer that delivered the forgery — the
    /// Welcome's inviter, or the sender of the frame carrying the commit — and
    /// never the address the forged leaf claimed. That attribution is worth
    /// something because it is proved independently: `__GRP_MLS_WELCOME__`,
    /// `__GRP_MLS_COMMIT__` and `__MLS_WELCOME__` are all security-gated, so
    /// the sender signed with the key its own address derives from. On the
    /// fourth site there is no delivering peer at all and `peer_id` is this
    /// device's own id.
    ///
    /// `reason` is diagnostic text, must not be parsed, and deliberately
    /// carries **no identifier** — the impersonated address appears only in
    /// this device's logs. Telemetry scrubbing hashes `peer_id` and passes
    /// `reason` through verbatim, and the identity at stake belongs to a third
    /// party who is not even part of the exchange.
    ///
    /// Emitted from four sites. The first three refuse a claim *arriving*, so
    /// nothing is installed and the frame is dropped:
    ///
    /// 1. joining a group Welcome whose ratchet tree contains such a leaf (the
    ///    invite is declined outright);
    /// 2. processing a membership commit that would install one (the commit is
    ///    not merged, and is never buffered for retry);
    /// 3. joining a 1:1 session Welcome whose ratchet tree contains one —
    ///    including when we already hold a session with that peer, where the
    ///    refusal is non-destructive and the existing session stays live (so a
    ///    `secure_session_failed` alongside this does *not* mean the working
    ///    session ended).
    ///
    /// The fourth is different in kind and needs a different response:
    ///
    /// 4. a roster read skipping a leaf **already seated in local group
    ///    state**. No wire gate can have admitted it, so it arrived by a direct
    ///    write to this device's secure store or via a group joined by a build
    ///    predating those gates. Such a leaf is kept out of every roster and
    ///    cannot speak — but it holds live group secrets and reads everything
    ///    sent to the group, which no later refusal undoes. The remedy is to
    ///    abandon the group, not to evict a member of it.
    ///
    /// Apps should treat a group that produces this as untrusted for
    /// attribution — a message shown as coming from a member is exactly what
    /// the forged leaf was for.
    GroupLeafIdentityUnproven,
}

impl SecurityWarningCode {
    /// Returns the stable machine-readable reason code.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SenderAddressMismatch => "SENDER_ADDRESS_MISMATCH",
            Self::TransportIdentityMismatch => "TRANSPORT_IDENTITY_MISMATCH",
            Self::ControlSignatureInvalid => "CONTROL_SIGNATURE_INVALID",
            Self::UnsignedControlRejected => "UNSIGNED_CONTROL_REJECTED",
            Self::MediaSenderGroupMismatch => "MEDIA_SENDER_GROUP_MISMATCH",
            Self::PlaintextSend => "PLAINTEXT_SEND",
            Self::PlaintextReceiveRejected => "PLAINTEXT_RECEIVE_REJECTED",
            Self::SessionSenderGroupMismatch => "SESSION_SENDER_GROUP_MISMATCH",
            Self::SessionRekeyTriggered => "SESSION_REKEY_TRIGGERED",
            Self::NostrKeyPackageSlotExhausted => "NOSTR_KEY_PACKAGE_SLOT_EXHAUSTED",
            Self::PushKeyPackagePoolExhausted => "PUSH_KEY_PACKAGE_POOL_EXHAUSTED",
            Self::RelayAddressBindingMismatch => "RELAY_ADDRESS_BINDING_MISMATCH",
            Self::RelayAddressDeclarationRefused => "RELAY_ADDRESS_DECLARATION_REFUSED",
            Self::GroupLeafIdentityUnproven => "GROUP_LEAF_IDENTITY_UNPROVEN",
        }
    }
}

/// Machine-readable reason for DORS selection or switch (observability).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DorsReasonCode {
    /// First selection; no previous transport.
    InitialSelection,
    /// DORS selected primary (before send attempt).
    PrimarySelected,
    /// Primary send succeeded; active transport is now primary.
    PrimarySuccess,
    /// Primary failed; fallback send succeeded.
    FallbackSuccess,
    /// BLE → WiFi fallback succeeded (escalation applied).
    EscalationApplied,
    /// Previous transport unavailable; switched to best available.
    CurrentUnavailable,
}

impl DorsReasonCode {
    /// Returns the stable machine-readable reason code.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InitialSelection => "INITIAL_SELECTION",
            Self::PrimarySelected => "PRIMARY_SELECTED",
            Self::PrimarySuccess => "PRIMARY_SUCCESS",
            Self::FallbackSuccess => "FALLBACK_SUCCESS",
            Self::EscalationApplied => "ESCALATION_APPLIED",
            Self::CurrentUnavailable => "CURRENT_UNAVAILABLE",
        }
    }
}

/// Phase of DORS escalation: recommendation (trigger boundary) vs actual transition (applied).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DorsEscalationPhase {
    /// DORS decided to escalate (typed trigger reason); fallback may not succeed.
    Triggered,
    /// BLE→WiFi fallback send succeeded; escalation was applied.
    Applied,
}

impl DorsEscalationPhase {
    /// Returns the stable string representation (TRIGGERED / APPLIED).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Triggered => "TRIGGERED",
            Self::Applied => "APPLIED",
        }
    }
}

/// Machine-readable reason for DORS escalation (BLE→Wi‑Fi) observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DorsEscalationReasonCode {
    /// Fallback to WiFi succeeded after primary (BLE) send failed.
    FallbackSuccess,
    /// Escalation suggested due to retry threshold (e.g. ble_to_wifi_retry_threshold).
    RetryThreshold,
    /// Escalation suggested due to sustained poor BLE signal.
    PoorSignal,
    /// Escalation suggested due to congestion.
    Congestion,
    /// Escalation suggested due to low TTL on messages.
    LowTtl,
    /// Escalation suggested due to BLE success rate below configured minimum (quality degradation).
    LowSuccessRate,
}

impl DorsEscalationReasonCode {
    /// Returns the stable machine-readable reason code.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FallbackSuccess => "FALLBACK_SUCCESS",
            Self::RetryThreshold => "RETRY_THRESHOLD",
            Self::PoorSignal => "POOR_SIGNAL",
            Self::Congestion => "CONGESTION",
            Self::LowTtl => "LOW_TTL",
            Self::LowSuccessRate => "LOW_SUCCESS_RATE",
        }
    }
}

/// Machine-readable reason taxonomy for inbound decryption failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DecryptionFailureCode {
    /// Incoming encrypted payload is malformed and cannot be parsed.
    InvalidPayload,
    /// MLS subsystem is not initialized locally.
    NotInitialized,
    /// Ciphertext is invalid or cannot be decrypted.
    InvalidCiphertext,
    /// Sender identity/signature verification failed.
    IdentityMismatch,
    /// Cryptographic operation failed.
    CryptoFailure,
    /// A media chunk was evicted from the pending-decryption queue (overflow or
    /// TTL expiry) before the sender's session became ready, so the file
    /// transfer it belongs to is currently stalled.
    ///
    /// Under the deferred-ACK model this is **advisory, not terminal**: the
    /// evicted chunk was never ACKed, so the sender keeps retransmitting and a
    /// later resend re-enters the queue and can still complete the transfer once
    /// the session confirms. Treat this as "the transfer is stalled and may need
    /// a resend", not "the transfer has permanently failed" — the terminal
    /// failure signal for media is `FileReceiveFailed`. The same is true of a
    /// hard decrypt failure while `crypto_recovery_enabled`, which surfaces
    /// under its own code but is equally un-ACKed and equally recoverable by a
    /// resend; see the `MessageDecryptionFailed` docs.
    PendingQueueDropped,
    /// Failure class is unknown.
    Unknown,
}

/// Forwarding attribution in events (mirrors `ForwardInfo` from core).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardInfoEvent {
    /// The original sender of the message.
    pub original_sender: String,
    /// The original message ID.
    pub original_message_id: String,
    /// The original timestamp (wall-clock ms).
    pub original_timestamp: i64,
    /// Number of times this message has been forwarded.
    pub forward_count: u32,
}

impl From<&offline_protocol_core::ForwardInfo> for ForwardInfoEvent {
    fn from(fwd: &offline_protocol_core::ForwardInfo) -> Self {
        Self {
            original_sender: fwd.original_sender.as_str().to_string(),
            original_message_id: fwd.original_message_id.as_str(),
            original_timestamp: fwd.original_timestamp.as_millis(),
            forward_count: fwd.forward_count,
        }
    }
}

/// Quoted-reply context in events (mirrors `ReplyContext` from core).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyContextEvent {
    /// Sender of the message being replied to.
    pub sender: String,
    /// Text (or excerpt) of the message being replied to.
    pub text: String,
    /// Timestamp of the quoted message (wall-clock ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// Short human-readable label for quoted media (e.g. a file name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_media_label: Option<String>,
    /// Content type of the quoted message, as a display string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_content_type: Option<String>,
}

impl From<&offline_protocol_core::ReplyContext> for ReplyContextEvent {
    fn from(rc: &offline_protocol_core::ReplyContext) -> Self {
        Self {
            sender: rc.sender.as_str().to_string(),
            text: rc.text.clone(),
            timestamp: rc.timestamp.map(|t| t.as_millis()),
            reply_media_label: rc.reply_media_label.clone(),
            reply_content_type: rc.reply_content_type.clone(),
        }
    }
}

/// Events that can occur in the protocol.
///
/// Note: This type implements a custom Debug that redacts sensitive fields
/// (message content) to prevent accidental logging of sensitive data.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A message was sent (queued for delivery).
    MessageSent {
        /// ID of the sent message.
        message_id: String,
        /// Sender's user ID.
        sender: String,
        /// Recipient's user ID.
        recipient: String,
        /// Content of the sent message.
        content: String,
        /// Priority of the message when sent.
        priority: String,
        /// Whether the message requires an acknowledgement.
        requires_ack: bool,
        /// When the message was queued for delivery (wall-clock, for display).
        timestamp: i64,
        /// Lamport logical clock value for causal ordering.
        #[serde(default)]
        lamport_clock: u64,
        /// Forwarding attribution (present when this is a forwarded message).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forward_info: Option<ForwardInfoEvent>,
    },

    /// A message was received.
    MessageReceived {
        /// ID of the received message.
        message_id: String,
        /// Sender's user ID.
        sender: String,
        /// Recipient's user ID.
        recipient: String,
        /// Message content.
        content: String,
        /// Number of hops the message traversed.
        hop_count: u8,
        /// Transport used for final delivery.
        transport: String,
        /// When the message was received (wall-clock, for display).
        timestamp: i64,
        /// Lamport logical clock value for causal ordering.
        #[serde(default)]
        lamport_clock: u64,
        /// ID of the message this is replying to (optional).
        #[serde(skip_serializing_if = "Option::is_none")]
        reply_to_msg: Option<String>,
        /// Quoted-reply context (present when this message quotes another).
        /// Boxed to keep the `Event` enum's by-value size in check.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_context: Option<Box<ReplyContextEvent>>,
        /// The type of content (text, image, video, etc.).
        #[serde(default)]
        content_type: String,
        /// Media metadata (present for non-text content).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_metadata: Option<MediaMetadata>,
        /// Forwarding attribution (present when this is a forwarded message).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forward_info: Option<ForwardInfoEvent>,
        /// `true` when the content arrived MLS-encrypted and was decrypted by
        /// this node; `false` for plaintext accepted under the
        /// `require_encryption = false` opt-out.
        #[serde(default)]
        encrypted: bool,
    },

    /// A message was successfully delivered (ACK received).
    MessageDelivered {
        /// ID of the delivered message.
        message_id: String,
        /// Latency in milliseconds.
        latency_ms: u64,
        /// Number of hops traversed.
        hop_count: u8,
        /// Transport used.
        transport: String,
    },

    /// A message failed to deliver.
    MessageFailed {
        /// ID of the failed message.
        message_id: String,
        /// Reason for failure.
        reason: String,
        /// Number of retries attempted.
        retry_count: u32,
    },

    /// An inbound encrypted message failed to decrypt on this attempt.
    ///
    /// **Advisory, not terminal, and emitted once per failed *attempt*.** While
    /// `EncryptionConfig::crypto_recovery_enabled` is on (the default) a
    /// message that fails to decrypt is not delivery-ACKed, so the sender keeps
    /// retrying — and each resend of a DM is re-sealed against the peer's
    /// current session, so a resend can actually land. Every failing attempt
    /// reports again, bounded by the sender's ACK retry budget. Read this as
    /// "this attempt did not decrypt"; the terminal signals are
    /// [`Event::MessageFailed`] on the sender and `FileReceiveFailed` for
    /// media.
    MessageDecryptionFailed {
        /// ID of the message that could not be decrypted.
        message_id: String,
        /// Sender's user ID.
        sender: String,
        /// Machine-readable decryption failure code.
        code: DecryptionFailureCode,
        /// Clear failure reason for application handling/logging.
        reason: String,
    },

    /// Transport was switched by DORS.
    TransportSwitched {
        /// Previous transport (if any).
        from: Option<String>,
        /// New transport.
        to: String,
        /// Reason for switch.
        reason: String,
    },

    /// This device was promoted to relay role.
    RelayPromoted {
        /// Number of connections when promoted.
        connection_count: usize,
        /// Battery level when promoted.
        battery_level: u8,
    },

    /// This device was demoted from relay role.
    RelayDemoted {
        /// Reason for demotion.
        reason: String,
    },

    /// A new neighbor was discovered.
    NeighborDiscovered {
        /// The peer's canonical user id — the same value the peer supplied
        /// as `ProtocolConfig.user_id`, on every transport. Valid directly
        /// as the `recipient` of `send_message` / `send_connection_request`.
        peer_id: String,
        /// Transport used to discover.
        transport: String,
        /// RSSI signal strength (if available).
        rssi: Option<i16>,
    },

    /// A neighbor was lost (disconnected).
    NeighborLost {
        /// Peer ID of the lost neighbor.
        peer_id: String,
    },

    /// This device's own address is known: MLS storage was opened and the
    /// identity key in it derived to `address`.
    ///
    /// Fires once per successful `initialize_mls`, before any frame can be
    /// sent. `address` is this device's `Message.sender` from here on, and what
    /// peers must be given to reach it. It is stable across restarts of the
    /// same profile, so an app that already stored it will be handed the same
    /// value rather than a new one.
    IdentityReady {
        /// This device's self-certifying address (`off1…`).
        address: String,
    },

    /// Network metrics update.
    NetworkMetrics {
        /// Number of active neighbors.
        neighbor_count: usize,
        /// Number of active relays.
        relay_count: usize,
        /// Message delivery ratio (0.0-1.0).
        delivery_ratio: f32,
        /// Average message latency in milliseconds.
        avg_latency_ms: u64,
    },

    /// File transfer progress update.
    FileProgress {
        /// File identifier.
        file_id: String,
        /// Number of chunks sent so far.
        chunks_sent: u32,
        /// Total number of chunks.
        total_chunks: u32,
        /// Progress percentage (0-100).
        percentage: u8,
    },

    /// File was completely received.
    FileReceived {
        /// File identifier.
        file_id: String,
        /// File name.
        file_name: String,
        /// File size in bytes.
        file_size: u64,
        /// Sender's user ID.
        sender: String,
        /// The content type of the media (image, video, file, etc.).
        content_type: String,
        /// Media metadata from the first chunk (present for typed media).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_metadata: Option<MediaMetadata>,
        /// Base64-encoded reassembled file data.
        file_data: String,
        /// When the sender queued the transfer (chunk-0 outer message
        /// timestamp, wall-clock ms) — for display ordering alongside
        /// `MessageReceived.timestamp`. Absent only if the chunk-0 record
        /// was evicted before completion.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<i64>,
        /// Caption text from the sealed chunk-0 rich extras.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        /// ID of the message this media replies to (sealed chunk-0 extras).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to_msg: Option<String>,
        /// Quoted-reply context (sealed chunk-0 extras). Boxed to keep the
        /// `Event` enum's by-value size in check.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_context: Option<Box<ReplyContextEvent>>,
        /// Forwarding attribution (sealed chunk-0 extras).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forward_info: Option<ForwardInfoEvent>,
    },

    /// An inbound file transfer was dropped before completion — the receiver
    /// hit a resource limit (too many concurrent transfers, per-sender
    /// quota, or the buffered-bytes budget), the reassembled file failed
    /// its integrity checks, or the transfer went stale (no chunks within
    /// the stale timeout). Terminal and fired at most once per transfer:
    /// the failed transfer's remaining in-flight chunks are dropped
    /// silently. No `FileReceived` will follow for this `file_id`; the
    /// sender must re-send the file (under a fresh `file_id`) to retry.
    FileReceiveFailed {
        /// File identifier.
        file_id: String,
        /// File name from the chunk metadata.
        file_name: String,
        /// Sender's user ID.
        sender: String,
        /// Machine-readable reason the transfer was dropped.
        reason: String,
    },

    /// All chunks of a media attachment were ACK-delivered.
    MediaSent {
        /// File identifier for tracking.
        file_id: String,
        /// The content type of the media.
        content_type: String,
        /// Recipient's user ID.
        recipient: String,
    },

    /// An outbound media transfer was aborted before all chunks were
    /// delivered — a chunk failed to encrypt, or a chunk failed terminally
    /// in the outbox. No `MediaSent` will follow for this `file_id`; retry
    /// with a new `send_media` call.
    MediaSendFailed {
        /// File identifier for tracking.
        file_id: String,
        /// Recipient's user ID.
        recipient: String,
        /// Reason the transfer was aborted.
        reason: String,
    },

    /// A message was deferred due to network conditions.
    /// The message will be retried automatically.
    MessageDeferred {
        /// ID of the deferred message.
        message_id: String,
        /// Reason for deferral.
        reason: String,
        /// Current retry count.
        retry_count: u32,
        /// Next retry scheduled time (Unix timestamp ms).
        next_retry_at: Option<i64>,
    },

    /// A retry for a previously deferred/unacked message was scheduled.
    ///
    /// Emitted each time the retry machinery re-queues a message after a
    /// failed attempt (transport send error or ACK timeout). Non-terminal:
    /// `MessageDelivered` or `MessageFailed` still settles the message.
    MessageRetrying {
        /// ID of the retrying message.
        message_id: String,
        /// Recipient's user ID.
        recipient: String,
        /// Retry count this schedule corresponds to.
        retry_count: u32,
        /// Absolute time the retry is scheduled for (Unix timestamp ms).
        next_retry_at: i64,
    },

    /// The transport layer reported the recipient unreachable for an
    /// in-flight message (e.g. the internet relay's DeliveryError).
    ///
    /// Non-terminal: the message stays in the outbox. A plain DM is
    /// *parked* — its ACK retry budget stops burning while the peer is
    /// provably offline — and is re-driven on reachability edges (transport
    /// reconnect, peer discovery, presence-online; the SDK adds parked
    /// recipients to the presence watchlist). It settles only via
    /// `MessageDelivered` or, at outbox-lifetime expiry, `MessageFailed`.
    /// Media chunks are not parked and keep the normal retry machinery.
    /// May fire multiple times for the same `message_id` while the
    /// recipient remains offline: a parked DM keeps an escalating
    /// reachability probe on every carrier (15s → 600s cap), and each probe
    /// that reaches the relay while the peer is still offline earns a fresh
    /// verdict and re-emits this event. Apps must treat it as a repeatable
    /// status signal, never as a terminal one. `file_id` is set when the
    /// message is a chunk of an outbound media transfer.
    MessageUndeliverable {
        /// ID of the affected message.
        message_id: String,
        /// Recipient's user ID.
        recipient: String,
        /// Transport-reported reason (starts with `recipient_unreachable`).
        reason: String,
        /// Owning media transfer when the message is a media chunk.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
    },

    /// An outbound media transfer was in flight when the previous process
    /// died. The SDK persists only the transfer descriptor (never chunk
    /// bytes), so it cannot resume the transfer itself: the app must
    /// re-supply the file bytes via `send_media` with this `file_id` —
    /// they are checksum-validated against the original transfer.
    MediaResendRequired {
        /// File identifier of the interrupted transfer.
        file_id: String,
        /// Recipient's user ID.
        recipient: String,
        /// Original file name.
        file_name: String,
        /// Total file size in bytes.
        file_size: u64,
    },

    /// A pending ACK was evicted due to capacity constraints.
    AckEvicted {
        /// ID of the message whose ACK was evicted.
        message_id: String,
        /// Priority of the evicted message.
        priority: String,
        /// Reason for eviction.
        reason: String,
    },

    /// A fragment assembly was evicted to make room for new fragments.
    FragmentAssemblyEvicted {
        /// ID of the message whose fragments were evicted.
        message_id: String,
        /// Completion percentage when evicted.
        completion_percent: u8,
        /// Reason for eviction.
        reason: String,
    },

    /// Relay role was demoted due to battery constraints.
    RelayDemotedBattery {
        /// Battery level at time of demotion.
        battery_level: u8,
        /// Minimum required battery level.
        min_required: u8,
    },

    /// A secure MLS session was successfully established.
    SecureSessionEstablished {
        /// Peer ID of the other party.
        peer_id: String,
        /// MLS group ID for the session.
        group_id: String,
        /// Whether this is a 1:1 session (true) or a multi-party group (false).
        is_session: bool,
        /// Whether the local device initiated the session (sent the Welcome).
        initiated_by_local: bool,
    },

    /// Failed to establish a secure MLS session.
    ///
    /// Reports a failed *handshake attempt*, which is not the same as "the
    /// session with this peer is gone". One case makes the difference visible:
    /// a Welcome refused for carrying an unprovable identity
    /// ([`SecurityWarningCode::GroupLeafIdentityUnproven`], emitted alongside)
    /// is refused non-destructively, so any session already held with that peer
    /// stays live and usable. Apps must not tear down session state on this
    /// event alone.
    ///
    /// `reason` is diagnostic text and must not be parsed. It carries no
    /// identifier, of this peer or of anyone else: the telemetry scrubber
    /// hashes `peer_id` and ships `reason` verbatim, so every arm that fills it
    /// contributes either a fixed string or
    /// [`MlsError::privacy_safe_reason`](offline_protocol_mls::MlsError::privacy_safe_reason),
    /// never a rendered error. The full error stays in the device log at the
    /// refusal site. This is a classification, so apps get the failure *class*
    /// here and must look to the log for the specifics.
    SecureSessionFailed {
        /// Peer ID of the other party.
        peer_id: String,
        /// Reason for the failure.
        reason: String,
    },

    /// Diagnostic breadcrumb for the 1:1 MLS convergence (Welcome receive /
    /// adopt / confirm) path. Emitted so the receiver side — which otherwise
    /// produces no event until it fully establishes or loudly fails — is
    /// observable in Metro. Pure instrumentation; carries no protocol effect.
    ConvergenceDiag {
        /// Fixed stage label, e.g. "welcome_received" or "welcome_branch".
        stage: String,
        /// Peer ID this breadcrumb concerns.
        peer_id: String,
        /// Free-form `key=value` context (counts, branch taken, errors).
        detail: String,
    },

    /// Welcome send lifecycle entered attempted state.
    WelcomeSendAttempted {
        /// Peer identifier for this welcome lifecycle.
        peer_id: String,
        /// Welcome message identifier for lifecycle correlation.
        message_id: String,
        /// MLS group identifier associated with the welcome.
        group_id: String,
        /// 1-based send attempt number.
        attempt: u32,
    },

    /// Welcome send lifecycle reached successful sent state.
    WelcomeSendSucceeded {
        /// Peer identifier for this welcome lifecycle.
        peer_id: String,
        /// Welcome message identifier for lifecycle correlation.
        message_id: String,
        /// MLS group identifier associated with the welcome.
        group_id: String,
        /// 1-based send attempt number.
        attempt: u32,
    },

    /// Welcome send attempt failed and may be retried.
    WelcomeSendFailed {
        /// Peer identifier for this welcome lifecycle.
        peer_id: String,
        /// Welcome message identifier for lifecycle correlation.
        message_id: String,
        /// MLS group identifier associated with the welcome.
        group_id: String,
        /// 1-based send attempt number.
        attempt: u32,
        /// Machine-readable reason code.
        reason_code: WelcomeReasonCode,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Optional transport/native error detail.
        transport_error: Option<String>,
        /// Whether this failure will be retried.
        retryable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Next scheduled retry timestamp in Unix ms.
        next_retry_at: Option<i64>,
    },

    /// Welcome send lifecycle reached terminal expiry.
    WelcomeSendExpired {
        /// Peer identifier for this welcome lifecycle.
        peer_id: String,
        /// Welcome message identifier for lifecycle correlation.
        message_id: String,
        /// Final attempt number when lifecycle expired.
        attempt: u32,
        /// Terminal machine-readable reason code.
        reason_code: WelcomeReasonCode,
    },

    /// A connection request was received from another user.
    ConnectionRequestReceived {
        /// User ID of the sender.
        sender: String,
        /// Display name of the sender.
        sender_name: String,
        /// Timestamp of the request (Unix ms).
        timestamp: i64,
        /// MLS key package data (if provided for encrypted session setup).
        #[serde(skip_serializing_if = "Option::is_none")]
        key_package: Option<Vec<u8>>,
        /// Optional first message sent along with the request.
        #[serde(skip_serializing_if = "Option::is_none")]
        initial_message: Option<String>,
    },

    /// An outbound connection request could not be delivered: the transport
    /// reported the recipient unreachable (e.g. the relay's DeliveryError
    /// for an offline peer), or the request exhausted its ACK retry budget
    /// (in which case a generic `MessageFailed` also fires for the same
    /// id). This is a status signal, not proof of permanent failure — the
    /// original request may still be delivered by the retry machinery if
    /// the peer comes back online. Apps typically surface "user is
    /// offline"; a user-initiated resend can therefore arrive alongside the
    /// retried original (the recipient sees the request event again, which
    /// accept/reject flows already tolerate).
    ConnectionRequestUndeliverable {
        /// Recipient the request was addressed to.
        recipient: String,
        /// Message id returned by `send_connection_request`.
        message_id: String,
        /// Failure reason: starts with `recipient_unreachable`, or is
        /// `max_retries_exceeded` when the retry budget ran out.
        reason: String,
    },

    /// A previously sent connection request was accepted.
    ConnectionAccepted {
        /// User ID of the accepting party.
        accepted_by: String,
        /// Display name of the accepting party.
        accepted_by_name: String,
        /// Timestamp of the acceptance (Unix ms).
        timestamp: i64,
        /// MLS key package data (if provided for encrypted session setup).
        #[serde(skip_serializing_if = "Option::is_none")]
        key_package: Option<Vec<u8>>,
    },

    /// A previously sent connection request was rejected.
    ConnectionRejected {
        /// User ID of the rejecting party.
        rejected_by: String,
    },

    /// A previously sent connection request was cancelled by the sender.
    ConnectionRequestCancelled {
        /// User ID of the party who cancelled their request.
        cancelled_by: String,
    },

    // --- Group (relay) events ---
    /// A group was created (from relay).
    GroupCreated {
        /// Group identifier.
        group_id: String,
        /// Human-readable group name.
        name: String,
    },

    /// A message was received in a group (from relay).
    GroupMessageReceived {
        /// Group identifier.
        group_id: String,
        /// User ID of the sender.
        sender: String,
        /// Message content.
        content: String,
        /// ISO-8601 timestamp.
        timestamp: String,
        /// Unique message identifier.
        message_id: String,
        /// Optional reply-to message ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to_msg: Option<String>,
        /// Forwarding attribution (present when this is a forwarded message).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forward_info: Option<ForwardInfoEvent>,
        /// Media metadata restored from the sealed `__RICH_V1__` body of a
        /// rich group message (cloud attachments — including the
        /// `encryption_key`/`iv` secrets, which only ever travel inside the
        /// group MLS ciphertext). Absent on plain group messages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_metadata: Option<MediaMetadata>,
        /// Content-type rendering hint from the sealed body (text, image,
        /// video, ...). Absent on plain group messages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },

    /// A member was added to a group (from relay).
    GroupMemberAdded {
        /// Group identifier.
        group_id: String,
        /// User ID of the added member.
        user_id: String,
        /// User ID of who performed the add.
        added_by: String,
        /// Human-readable group name (present when the invitee first joins).
        #[serde(skip_serializing_if = "Option::is_none")]
        group_name: Option<String>,
        /// Whether `added_by` was authorized to make this change.
        ///
        /// `Some(false)` means the change **did** happen — MLS accepted the
        /// commit and the roster really has changed — but the committer was
        /// not a known admin. The membership event is still emitted because
        /// the local roster must not diverge from MLS state; see
        /// [`Event::GroupUnauthorizedMembershipChange`] for the full signal.
        ///
        /// `None` (absent in JSON) means authorization was **not evaluated**
        /// on the path that produced this event: our own join accepted from
        /// a Welcome (there is no prior group state to judge the inviter
        /// against) and relay reconciliation frames (no authenticated
        /// committer to judge). Events from an older core also omit the
        /// field. Only `Some(_)` is a positive statement either way.
        ///
        /// The judgment is made against this member's **local replica** of
        /// role state, which replicates best-effort and can lag — a
        /// legitimate change may be flagged `Some(false)`, and different
        /// members can disagree about the same commit. Do not act on it
        /// automatically.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authorized: Option<bool>,
    },

    /// A member was removed from a group (from relay).
    GroupMemberRemoved {
        /// Group identifier.
        group_id: String,
        /// User ID of the removed member.
        user_id: String,
        /// User ID of who performed the removal.
        removed_by: String,
        /// Whether this change was authorized (`None` = not evaluated on
        /// this path). See [`Event::GroupMemberAdded::authorized`]. On the
        /// relay reconciliation path the judgment applies to the frame's
        /// authenticated wire sender, which is not necessarily the
        /// `removed_by` named here.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authorized: Option<bool>,
    },

    /// Group info was received (from relay).
    GroupInfo {
        /// Group identifier.
        group_id: String,
        /// Human-readable group name.
        name: String,
        /// User ID of the group creator.
        created_by: String,
        /// ISO-8601 creation timestamp.
        created_at: String,
        /// Group member list.
        members: Vec<GroupInfoMember>,
    },

    /// User's groups list was received (from relay).
    UserGroups {
        /// List of group summaries.
        groups: Vec<UserGroupSummary>,
    },

    /// A group operation failed (from relay).
    ///
    /// The relay's own wording is **not** carried here. `__GROUP_ERROR__` is
    /// a relay answer, accepted unsigned on the relay ingest path (the
    /// bridges inject it unattributed), so its text is chosen by whoever put
    /// the frame on that socket — arbitrary content, arbitrary length, and
    /// not something this SDK can vouch for. `reason` is a fixed code
    /// classified locally instead; the
    /// relay's exact text is logged on-device and reaches apps, when they
    /// want it, as the raw `GroupError` frame the bridges dual-emit on the
    /// server-message channel.
    GroupError {
        /// Why the operation failed, from a fixed set: `not_found` (the
        /// relay has no such group — invite links and relay fan-out for it
        /// are dead, not merely refused), `sync_denied` (the relay refused
        /// to register or sync the group for this caller), or `error` (any
        /// other refusal).
        reason: String,
        /// Group the error concerns, when the relay scoped it. Absent for
        /// unscoped errors.
        ///
        /// This is the group-scoping the free-text `reason` used to smuggle
        /// past the telemetry scrubber — as a real field it is hashed like
        /// every other `group_id` instead.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_id: Option<String>,
    },

    /// The relay-side registration state of a group changed.
    ///
    /// `synced: true` fires only on the relay's positive registration
    /// acknowledgment (a `GroupCreated` answering a registration this
    /// device actually sent, over the Internet transport) — including the
    /// idempotent re-sync ack after a membership change. It is the signal
    /// that relay-dependent server commands for the group (invite links,
    /// server-side fan-out) can be issued. `synced: false` fires when that
    /// trust is revoked; `reason` says why. Not emitted for groups the
    /// relay was never asked about.
    GroupRelaySyncChanged {
        /// MLS group identifier.
        group_id: String,
        /// Whether the relay currently holds a positively acknowledged
        /// registration for this group.
        synced: bool,
        /// Why the state changed: `registered` (relay ack), `error`
        /// (group-scoped relay error), `removed` (we were removed from the
        /// group), `left` (local leave), `internet_dropped` (Internet
        /// transport lost; re-registration re-arms on reconnect), or
        /// `ack_timeout` (relay never answered the registration — likely a
        /// relay without group support).
        reason: String,
    },

    /// A group message was sent to all members via mesh (MLS-encrypted fan-out).
    GroupMessageSent {
        /// MLS group identifier.
        group_id: String,
        /// Per-member message IDs from fan-out.
        message_ids: Vec<String>,
        /// Number of members the message was sent to.
        member_count: u32,
    },

    /// A group message was only partially delivered (some members failed).
    GroupMessagePartialFailure {
        /// MLS group identifier.
        group_id: String,
        /// Members for whom send failed.
        failed_members: Vec<String>,
        /// Members for whom send succeeded.
        succeeded_members: Vec<String>,
    },

    /// The relay's settled per-recipient delivery report for a group
    /// message sent via relay broadcast, after the SDK acted on it.
    ///
    /// Fires once per broadcast whose report arrived, seconds after
    /// `group_message_sent` (the relay reports only when its whole fan-out
    /// has settled — correlate by `message_id`, never by order). Members in
    /// `delivered` took the message over a live relay socket; members in
    /// `pushed` took a device push carrying the ciphertext. Every other
    /// MLS roster member — the ones the relay reported missed *and* the
    /// ones it did not know about at all — has already been re-sent a
    /// per-member copy through the ordinary outbox/ACK/park delivery
    /// ladder (`missed_reissued`), so no app action is required; the event
    /// is delivery observability, not a failure signal. Not emitted when
    /// the report itself is lost — the SDK then re-broadcasts (bounded)
    /// and finally downgrades the whole message to per-member fan-out,
    /// which reports per-member like any mesh send.
    GroupMessageDeliveryReport {
        /// MLS group identifier.
        group_id: String,
        /// The logical group message id, as returned by the send and echoed
        /// by the relay.
        message_id: String,
        /// Members whose relay socket write was confirmed (a relay-side
        /// write-ack, not an end-to-end delivery ACK).
        delivered: Vec<String>,
        /// Members offline at the relay who took a device push.
        pushed: Vec<String>,
        /// Members re-sent a per-member copy because the relay reached them
        /// neither way (or never knew them).
        missed_reissued: Vec<String>,
    },

    /// Rich media metadata was dropped from an outbound group message
    /// because the group is not fully rich-capable (not every member
    /// advertised sealed rich payload support, or the local kill switch
    /// disabled it). The text was still sent; members receive it without
    /// the media attachment. Apps can use this to warn the sender that an
    /// attachment did not go through.
    GroupRichExtrasDropped {
        /// MLS group identifier.
        group_id: String,
        /// Members not known to parse the sealed rich payload — the ones
        /// holding the seal gate closed. Absence of knowledge and known
        /// non-support are indistinguishable here. Empty when the drop was
        /// caused by the local `rich_payload_enabled` kill switch instead.
        /// The SDK probes these members for their capability automatically;
        /// apps can retry the send once a later attempt stops dropping.
        unknown_members: Vec<String>,
    },

    /// An MLS membership change was applied that its committer was not
    /// authorized to make.
    ///
    /// The change **has been applied** — MLS authenticated the committer as a
    /// group member and accepted the commit, and the local epoch advanced with
    /// it. The SDK's admin model is an application-layer overlay on top of MLS
    /// (RFC 9420 leaves membership access control to the application), enforced
    /// on the *sending* side; refusing the change on receipt would mean
    /// refusing the merge, which permanently forks this member away from every
    /// peer that accepted it. So the change stands and is reported here instead.
    ///
    /// **This signal can false-positive.** "Unauthorized" is judged against
    /// this member's local replica of role state, which replicates
    /// best-effort (a joiner receives a point-in-time snapshot, role changes
    /// ride unreconciled mesh notifications) and can lag — so a legitimate
    /// change, e.g. a voluntary leave committed by a member whose admin role
    /// this device has not yet learned, may be reported here, and different
    /// members can disagree about the same commit.
    ///
    /// Apps should treat this as a moderation signal for a *human* admin: an
    /// admin can undo the change with `remove_from_group` /
    /// `invite_to_group`. Never reverse a change automatically off a single
    /// member's event — corroborate first. A member added this way can read
    /// all subsequent group traffic until removed.
    ///
    /// Reports are rate-limited per `(group, committer)`: a repeat within a
    /// short window is not re-emitted (divergent role metadata would
    /// otherwise re-fire it on every commit), but every affected
    /// [`Event::GroupMemberAdded`] / [`Event::GroupMemberRemoved`] still
    /// carries `authorized: Some(false)`.
    ///
    /// Known limitation: the member *removed* by an unauthorized Remove does
    /// not receive this event — it can no longer decrypt the commit that
    /// removed it, and it refuses to trust a non-admin's unencrypted claim
    /// that it was removed. Only the remaining members report the removal.
    GroupUnauthorizedMembershipChange {
        /// MLS group identifier.
        group_id: String,
        /// The MLS-authenticated committer that made the change.
        committer: String,
        /// Members the commit added, sorted. Empty for a pure removal.
        added: Vec<String>,
        /// Members the commit removed, sorted. Empty for a pure addition.
        removed: Vec<String>,
        /// Why the change was judged unauthorized. Currently
        /// `"sender_not_admin"` (the committer is not a known admin) or
        /// `"affected_member_mismatch"` (an admin committed, but the commit's
        /// unencrypted framing named a different member than the MLS delta
        /// actually changed). Treat as an opaque string — values may be added.
        reason: String,
        /// Whether the commit was *refused* rather than applied.
        ///
        /// `false` (the default configuration) means the membership change
        /// happened and is being reported after the fact — `added`/`removed`
        /// describe real roster changes an admin can undo. `true` means
        /// `GroupConfig::enforce_admin_commits` was on and the commit was
        /// rejected before merging: no roster event accompanies this one, no
        /// membership changed locally, and `added`/`removed` describe what the
        /// commit *would* have done.
        ///
        /// A `true` here also means this device declined an epoch every
        /// accepting member advanced to, so it can no longer decrypt that
        /// group's traffic and needs re-inviting — treat it as a partition
        /// alarm, not just a moderation signal. Note the re-invite arrives as
        /// a Welcome, which is not policy-gated, so it readmits us to the
        /// group *including* whatever the refused commit did; the change
        /// itself still has to be resolved separately.
        enforced: bool,
    },

    /// An epoch fork was detected in a group — concurrent commits caused
    /// members to diverge onto different MLS branches. The deterministic
    /// leader will attempt automatic resolution via a key-update commit.
    GroupEpochForkDetected {
        /// MLS group identifier.
        group_id: String,
        /// Our local epoch when the fork was detected, or `None` if MLS
        /// state was unavailable at detection time.
        local_epoch: Option<u64>,
    },

    /// An epoch fork was successfully resolved by the leader issuing a
    /// resync commit that re-established a canonical epoch.
    ///
    /// Members in `failed_members` could not be reached with the resolution
    /// commit and may still be on a forked branch — the application layer
    /// should consider re-inviting them.
    GroupEpochForkResolved {
        /// MLS group identifier.
        group_id: String,
        /// The new canonical epoch after resolution.
        resolved_epoch: u64,
        /// Members to whom the resolution commit could not be sent.
        /// These members may still be on a forked epoch branch and
        /// may need a re-invite to rejoin the canonical group state.
        failed_members: Vec<String>,
    },

    /// A member's role was changed in a group.
    GroupRoleChanged {
        /// Group identifier.
        group_id: String,
        /// User ID of the member whose role changed.
        user_id: String,
        /// New role (e.g. "admin", "member").
        new_role: String,
        /// User ID of who changed the role.
        changed_by: String,
    },

    /// A group was renamed.
    GroupRenamed {
        /// Group identifier.
        group_id: String,
        /// New group name.
        new_name: String,
        /// Previous group name (if known).
        old_name: Option<String>,
        /// User ID of who renamed the group.
        renamed_by: String,
    },

    // --- Service discovery ---
    /// A service was discovered on the mesh in response to a discovery query.
    ServiceDiscovered {
        /// Query ID that triggered this discovery.
        query_id: String,
        /// Service identifier.
        service_id: String,
        /// Service version.
        version: String,
        /// Peer user ID of the provider.
        provider_peer_id: String,
        /// Service capabilities.
        capabilities: HashMap<String, String>,
        /// Number of hops from the provider.
        hop_count: u8,
    },

    /// A service request was received from another peer.
    ServiceRequestReceived {
        /// Unique request identifier.
        request_id: String,
        /// Service identifier being invoked.
        service_id: String,
        /// Method name or action.
        method: String,
        /// Request body (JSON or arbitrary string).
        body: String,
        /// Peer user ID of the requester.
        sender: String,
    },

    /// A response to a service request was received.
    ServiceResponseReceived {
        /// Request identifier this response corresponds to.
        request_id: String,
        /// Service identifier.
        service_id: String,
        /// Status: "ok", "not_found", or "error".
        status: String,
        /// Response body.
        body: String,
        /// Peer user ID of the provider.
        provider_peer_id: String,
    },

    // --- Presence, typing, and read receipts ---
    /// A peer's presence status was updated.
    PresenceUpdated {
        /// Peer whose presence changed.
        peer_id: String,
        /// Presence status.
        status: PresenceStatus,
        /// Timestamp of the update (Unix ms).
        timestamp: i64,
        /// When the peer was last seen (Unix ms), if the source knows it —
        /// e.g. the internet relay's PresenceStatusWithLastSeen. Absent for
        /// peer-sent `__PRESENCE__` updates.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_seen_ms: Option<i64>,
        /// Which channel produced this update: the internet relay's
        /// authoritative answer, or a peer-sent self-report. Always
        /// serialized; defaults to `peer` when deserializing legacy events.
        #[serde(default)]
        source: PresenceSource,
    },

    /// A typing indicator was received from a peer.
    TypingIndicatorReceived {
        /// User who is typing (or stopped typing).
        sender: String,
        /// Conversation identifier, echoed back verbatim from the sender.
        ///
        /// Opaque to the SDK — never parsed or routed on. Whatever key the
        /// sender chose (conventionally a peer address for a DM, the group
        /// id for a group) arrives here unchanged.
        conversation_id: String,
        /// Whether the user is currently typing.
        is_typing: bool,
        /// Timestamp of the indicator (Unix ms).
        timestamp: i64,
    },

    /// A read receipt was received from a peer.
    ReadReceiptReceived {
        /// User who read the messages.
        sender: String,
        /// IDs of the messages that were read.
        message_ids: Vec<String>,
        /// Timestamp when the messages were read (Unix ms).
        timestamp: i64,
    },

    // --- DORS observability (OFF-258) ---
    /// DORS scored all available transports for this decision cycle.
    DorsScoreUpdated {
        /// Transport type and score pairs (descending by score).
        scores: Vec<(String, f32)>,
    },
    /// DORS selected a transport for the current message.
    DorsTransportSelected {
        /// Previous transport (if any); provides decision boundary context.
        from: Option<String>,
        /// Selected transport.
        transport: String,
        /// Typed reason for selection (initial vs primary selected).
        reason_code: DorsReasonCode,
        /// Score of the selected transport (supplemental).
        score: f32,
    },
    /// DORS switched from one transport to another. Emitted only when active transport
    /// actually changes after a successful send.
    DorsTransportSwitched {
        /// Previous transport (if any).
        from: Option<String>,
        /// New transport (now active).
        to: String,
        /// Stable enum-backed reason for the transition.
        reason_code: DorsReasonCode,
        /// Optional human-readable context (e.g. "primary failed, fallback succeeded").
        #[serde(skip_serializing_if = "Option::is_none")]
        reason_detail: Option<String>,
    },
    /// DORS escalation from BLE to Wi‑Fi. Use `phase` to distinguish recommendation vs applied.
    DorsEscalationTriggered {
        /// TRIGGERED = DORS decided to escalate (reason = trigger); APPLIED = fallback succeeded.
        phase: DorsEscalationPhase,
        /// Transport we escalated from (e.g. "ble").
        from: String,
        /// Transport we escalated to (e.g. "wifiDirect").
        to: String,
        /// Stable enum-backed reason code.
        reason_code: DorsEscalationReasonCode,
        /// Optional human-readable context.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason_detail: Option<String>,
    },

    /// A security-relevant anomaly was detected (e.g. sender spoofing, a
    /// signing key that does not derive to its claimed address, an unsigned
    /// control message).
    SecurityWarning {
        /// Peer ID involved in the warning.
        peer_id: String,
        /// Stable machine-readable classification of the warning. Branch on
        /// this rather than parsing `reason`.
        reason_code: SecurityWarningCode,
        /// Human-readable description of the security issue.
        reason: String,
    },

    /// A message was relayed (forwarded for a third party).
    MessageRelayed {
        /// ID of the relayed message.
        message_id: String,
        /// Original sender.
        sender: String,
        /// Intended recipient.
        recipient: String,
        /// Current hop count after increment.
        hop_count: u8,
        /// Remaining TTL after decrement.
        remaining_ttl: u8,
    },

    /// A user was blocked. Emitted for local UI notification only.
    UserBlocked {
        /// User ID that was blocked.
        user_id: String,
    },

    /// A user was unblocked. Emitted for local UI notification only.
    UserUnblocked {
        /// User ID that was unblocked.
        user_id: String,
    },
}

/// Member entry in GroupInfo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfoMember {
    /// User ID of the member.
    pub user_id: String,
    /// Role within the group (e.g. "admin", "member").
    pub role: String,
    /// ISO-8601 timestamp when the member joined.
    pub joined_at: String,
}

/// Group summary in UserGroups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserGroupSummary {
    /// Group identifier.
    pub group_id: String,
    /// Human-readable group name.
    pub name: String,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
}

impl Event {
    /// Creates a MessageSent event.
    pub fn message_sent(message: &Message) -> Self {
        use offline_protocol_core::MessagePriority;

        let priority = match message.priority {
            MessagePriority::Low => "low",
            MessagePriority::Medium => "medium",
            MessagePriority::High => "high",
            MessagePriority::Critical => "critical",
        };

        let forward_info = message.forwarded_from.as_ref().map(ForwardInfoEvent::from);

        Self::MessageSent {
            message_id: message.id.as_str(),
            sender: message.sender.as_str().to_string(),
            recipient: message.recipient.as_str().to_string(),
            content: message.content.clone(),
            priority: priority.to_string(),
            requires_ack: message.requires_ack,
            timestamp: message.timestamp.as_millis(),
            lamport_clock: message.lamport_clock.value(),
            forward_info,
        }
    }

    /// Creates a MessageDelivered event.
    pub fn message_delivered(
        message_id: MessageId,
        latency_ms: u64,
        hop_count: u8,
        transport: TransportType,
    ) -> Self {
        Self::MessageDelivered {
            message_id: message_id.as_str(),
            latency_ms,
            hop_count,
            transport: transport.to_string(),
        }
    }

    /// Creates a MessageFailed event.
    pub fn message_failed(message_id: MessageId, reason: String, retry_count: u32) -> Self {
        Self::MessageFailed {
            message_id: message_id.as_str(),
            reason,
            retry_count,
        }
    }

    /// Creates a MessageDecryptionFailed event.
    pub fn message_decryption_failed(
        message_id: MessageId,
        sender: String,
        code: DecryptionFailureCode,
        reason: String,
    ) -> Self {
        Self::MessageDecryptionFailed {
            message_id: message_id.as_str(),
            sender,
            code,
            reason,
        }
    }

    /// Creates a TransportSwitched event.
    pub fn transport_switched(
        from: Option<TransportType>,
        to: TransportType,
        reason: String,
    ) -> Self {
        Self::TransportSwitched {
            from: from.map(|t| t.to_string()),
            to: to.to_string(),
            reason,
        }
    }

    /// Creates a RelayPromoted event.
    pub fn relay_promoted(connection_count: usize, battery_level: u8) -> Self {
        Self::RelayPromoted {
            connection_count,
            battery_level,
        }
    }

    /// Creates a RelayDemoted event.
    pub fn relay_demoted(reason: String) -> Self {
        Self::RelayDemoted { reason }
    }

    /// Creates a NeighborDiscovered event.
    pub fn neighbor_discovered(
        peer_id: String,
        transport: TransportType,
        rssi: Option<i16>,
    ) -> Self {
        Self::NeighborDiscovered {
            peer_id,
            transport: format!("{:?}", transport),
            rssi,
        }
    }

    /// Creates a NeighborLost event.
    pub fn neighbor_lost(peer_id: String) -> Self {
        Self::NeighborLost { peer_id }
    }

    /// Creates an IdentityReady event.
    pub fn identity_ready(address: impl Into<String>) -> Self {
        Self::IdentityReady {
            address: address.into(),
        }
    }

    /// Creates a NetworkMetrics event.
    pub fn network_metrics(
        neighbor_count: usize,
        relay_count: usize,
        delivery_ratio: f32,
        avg_latency_ms: u64,
    ) -> Self {
        Self::NetworkMetrics {
            neighbor_count,
            relay_count,
            delivery_ratio,
            avg_latency_ms,
        }
    }

    /// Creates a FileProgress event.
    pub fn file_progress(file_id: String, chunks_sent: u32, total_chunks: u32) -> Self {
        let percentage = if total_chunks > 0 {
            ((chunks_sent as f32 / total_chunks as f32) * 100.0) as u8
        } else {
            0
        };

        Self::FileProgress {
            file_id,
            chunks_sent,
            total_chunks,
            percentage,
        }
    }

    /// Creates a FileReceived event.
    #[allow(clippy::too_many_arguments)]
    pub fn file_received(
        file_id: String,
        file_name: String,
        file_size: u64,
        sender: String,
        content_type: ContentType,
        media_metadata: Option<MediaMetadata>,
        file_data: Vec<u8>,
        timestamp: Option<i64>,
        caption: Option<String>,
        reply_to_msg: Option<String>,
        reply_context: Option<&offline_protocol_core::ReplyContext>,
        forward_info: Option<&offline_protocol_core::ForwardInfo>,
    ) -> Self {
        use base64::{engine::general_purpose::STANDARD, Engine};
        Self::FileReceived {
            file_id,
            file_name,
            file_size,
            sender,
            content_type: content_type.to_string(),
            media_metadata,
            file_data: STANDARD.encode(&file_data),
            timestamp,
            caption,
            reply_to_msg,
            reply_context: reply_context.map(|rc| Box::new(ReplyContextEvent::from(rc))),
            forward_info: forward_info.map(ForwardInfoEvent::from),
        }
    }

    /// Creates a FileReceiveFailed event.
    pub fn file_receive_failed(
        file_id: String,
        file_name: String,
        sender: String,
        reason: String,
    ) -> Self {
        Self::FileReceiveFailed {
            file_id,
            file_name,
            sender,
            reason,
        }
    }

    /// Creates a MediaSent event.
    pub fn media_sent(file_id: String, content_type: ContentType, recipient: String) -> Self {
        Self::MediaSent {
            file_id,
            content_type: content_type.to_string(),
            recipient,
        }
    }

    /// Creates a MediaSendFailed event.
    pub fn media_send_failed(file_id: String, recipient: String, reason: String) -> Self {
        Self::MediaSendFailed {
            file_id,
            recipient,
            reason,
        }
    }

    /// Creates a MessageDeferred event.
    pub fn message_deferred(
        message_id: MessageId,
        reason: String,
        retry_count: u32,
        next_retry_at: Option<i64>,
    ) -> Self {
        Self::MessageDeferred {
            message_id: message_id.as_str(),
            reason,
            retry_count,
            next_retry_at,
        }
    }

    /// Creates a MessageRetrying event.
    pub fn message_retrying(
        message_id: MessageId,
        recipient: String,
        retry_count: u32,
        next_retry_at: i64,
    ) -> Self {
        Self::MessageRetrying {
            message_id: message_id.as_str(),
            recipient,
            retry_count,
            next_retry_at,
        }
    }

    /// Creates a MessageUndeliverable event.
    pub fn message_undeliverable(
        message_id: MessageId,
        recipient: String,
        reason: String,
        file_id: Option<String>,
    ) -> Self {
        Self::MessageUndeliverable {
            message_id: message_id.as_str(),
            recipient,
            reason,
            file_id,
        }
    }

    /// Creates a MediaResendRequired event.
    pub fn media_resend_required(
        file_id: String,
        recipient: String,
        file_name: String,
        file_size: u64,
    ) -> Self {
        Self::MediaResendRequired {
            file_id,
            recipient,
            file_name,
            file_size,
        }
    }

    /// Creates an AckEvicted event.
    pub fn ack_evicted(message_id: MessageId, priority: &str, reason: String) -> Self {
        Self::AckEvicted {
            message_id: message_id.as_str(),
            priority: priority.to_string(),
            reason,
        }
    }

    /// Creates a FragmentAssemblyEvicted event.
    pub fn fragment_assembly_evicted(
        message_id: String,
        completion_percent: u8,
        reason: String,
    ) -> Self {
        Self::FragmentAssemblyEvicted {
            message_id,
            completion_percent,
            reason,
        }
    }

    /// Creates a RelayDemotedBattery event.
    pub fn relay_demoted_battery(battery_level: u8, min_required: u8) -> Self {
        Self::RelayDemotedBattery {
            battery_level,
            min_required,
        }
    }

    /// Creates a SecureSessionEstablished event.
    pub fn secure_session_established(
        peer_id: String,
        group_id: String,
        is_session: bool,
        initiated_by_local: bool,
    ) -> Self {
        Self::SecureSessionEstablished {
            peer_id,
            group_id,
            is_session,
            initiated_by_local,
        }
    }

    /// Creates a SecureSessionFailed event.
    pub fn secure_session_failed(peer_id: String, reason: String) -> Self {
        Self::SecureSessionFailed { peer_id, reason }
    }

    /// Creates a ConvergenceDiag event (1:1 MLS convergence instrumentation).
    pub fn convergence_diag(stage: String, peer_id: String, detail: String) -> Self {
        Self::ConvergenceDiag {
            stage,
            peer_id,
            detail,
        }
    }

    /// Creates a WelcomeSendAttempted event.
    pub fn welcome_send_attempted(
        peer_id: String,
        message_id: String,
        group_id: String,
        attempt: u32,
    ) -> Self {
        Self::WelcomeSendAttempted {
            peer_id,
            message_id,
            group_id,
            attempt,
        }
    }

    /// Creates a WelcomeSendSucceeded event.
    pub fn welcome_send_succeeded(
        peer_id: String,
        message_id: String,
        group_id: String,
        attempt: u32,
    ) -> Self {
        Self::WelcomeSendSucceeded {
            peer_id,
            message_id,
            group_id,
            attempt,
        }
    }

    /// Creates a WelcomeSendFailed event.
    #[allow(clippy::too_many_arguments)]
    pub fn welcome_send_failed(
        peer_id: String,
        message_id: String,
        group_id: String,
        attempt: u32,
        reason_code: WelcomeReasonCode,
        transport_error: Option<String>,
        retryable: bool,
        next_retry_at: Option<i64>,
    ) -> Self {
        Self::WelcomeSendFailed {
            peer_id,
            message_id,
            group_id,
            attempt,
            reason_code,
            transport_error,
            retryable,
            next_retry_at,
        }
    }

    /// Creates a WelcomeSendExpired event.
    pub fn welcome_send_expired(
        peer_id: String,
        message_id: String,
        attempt: u32,
        reason_code: WelcomeReasonCode,
    ) -> Self {
        Self::WelcomeSendExpired {
            peer_id,
            message_id,
            attempt,
            reason_code,
        }
    }

    /// Creates a ConnectionRequestReceived event.
    pub fn connection_request_received(
        sender: String,
        sender_name: String,
        timestamp: i64,
        key_package: Option<Vec<u8>>,
        initial_message: Option<String>,
    ) -> Self {
        Self::ConnectionRequestReceived {
            sender,
            sender_name,
            timestamp,
            key_package,
            initial_message,
        }
    }

    /// Creates a ConnectionRequestUndeliverable event.
    pub fn connection_request_undeliverable(
        recipient: String,
        message_id: String,
        reason: String,
    ) -> Self {
        Self::ConnectionRequestUndeliverable {
            recipient,
            message_id,
            reason,
        }
    }

    /// Creates a ConnectionAccepted event.
    pub fn connection_accepted(
        accepted_by: String,
        accepted_by_name: String,
        timestamp: i64,
        key_package: Option<Vec<u8>>,
    ) -> Self {
        Self::ConnectionAccepted {
            accepted_by,
            accepted_by_name,
            timestamp,
            key_package,
        }
    }

    /// Creates a ConnectionRejected event.
    pub fn connection_rejected(rejected_by: String) -> Self {
        Self::ConnectionRejected { rejected_by }
    }

    /// Creates a ConnectionRequestCancelled event.
    pub fn connection_request_cancelled(cancelled_by: String) -> Self {
        Self::ConnectionRequestCancelled { cancelled_by }
    }

    /// Creates a PresenceUpdated event for a peer-sent `__PRESENCE__`
    /// self-report (`source: peer`).
    pub fn presence_updated(peer_id: String, status: PresenceStatus, timestamp: i64) -> Self {
        Self::PresenceUpdated {
            peer_id,
            status,
            timestamp,
            last_seen_ms: None,
            source: PresenceSource::Peer,
        }
    }

    /// Creates a PresenceUpdated event carrying a last-seen timestamp
    /// (relay-sourced presence, `source: internet`).
    pub fn presence_updated_with_last_seen(
        peer_id: String,
        status: PresenceStatus,
        timestamp: i64,
        last_seen_ms: Option<i64>,
    ) -> Self {
        Self::PresenceUpdated {
            peer_id,
            status,
            timestamp,
            last_seen_ms,
            source: PresenceSource::Internet,
        }
    }

    /// Creates a TypingIndicatorReceived event.
    pub fn typing_indicator_received(
        sender: String,
        conversation_id: String,
        is_typing: bool,
        timestamp: i64,
    ) -> Self {
        Self::TypingIndicatorReceived {
            sender,
            conversation_id,
            is_typing,
            timestamp,
        }
    }

    /// Creates a ReadReceiptReceived event.
    pub fn read_receipt_received(sender: String, message_ids: Vec<String>, timestamp: i64) -> Self {
        Self::ReadReceiptReceived {
            sender,
            message_ids,
            timestamp,
        }
    }

    /// Creates a GroupCreated event.
    pub fn group_created(group_id: String, name: String) -> Self {
        Self::GroupCreated { group_id, name }
    }

    /// Creates a GroupMessageReceived event.
    #[allow(clippy::too_many_arguments)]
    pub fn group_message_received(
        group_id: String,
        sender: String,
        content: String,
        timestamp: String,
        message_id: String,
        reply_to_msg: Option<String>,
        forward_info: Option<ForwardInfoEvent>,
        media_metadata: Option<MediaMetadata>,
        content_type: Option<String>,
    ) -> Self {
        Self::GroupMessageReceived {
            group_id,
            sender,
            content,
            timestamp,
            message_id,
            reply_to_msg,
            forward_info,
            media_metadata,
            content_type,
        }
    }

    /// Creates a GroupMemberAdded event.
    pub fn group_member_added(
        group_id: String,
        user_id: String,
        added_by: String,
        group_name: Option<String>,
        authorized: Option<bool>,
    ) -> Self {
        Self::GroupMemberAdded {
            group_id,
            user_id,
            added_by,
            group_name,
            authorized,
        }
    }

    /// Creates a GroupMemberRemoved event.
    pub fn group_member_removed(
        group_id: String,
        user_id: String,
        removed_by: String,
        authorized: Option<bool>,
    ) -> Self {
        Self::GroupMemberRemoved {
            group_id,
            user_id,
            removed_by,
            authorized,
        }
    }

    /// Creates a GroupRoleChanged event.
    pub fn group_role_changed(
        group_id: String,
        user_id: String,
        new_role: String,
        changed_by: String,
    ) -> Self {
        Self::GroupRoleChanged {
            group_id,
            user_id,
            new_role,
            changed_by,
        }
    }

    /// Creates a GroupRenamed event.
    pub fn group_renamed(
        group_id: String,
        new_name: String,
        old_name: Option<String>,
        renamed_by: String,
    ) -> Self {
        Self::GroupRenamed {
            group_id,
            new_name,
            old_name,
            renamed_by,
        }
    }

    /// Creates a GroupInfo event.
    pub fn group_info(
        group_id: String,
        name: String,
        created_by: String,
        created_at: String,
        members: Vec<GroupInfoMember>,
    ) -> Self {
        Self::GroupInfo {
            group_id,
            name,
            created_by,
            created_at,
            members,
        }
    }

    /// Creates a UserGroups event.
    pub fn user_groups(groups: Vec<UserGroupSummary>) -> Self {
        Self::UserGroups { groups }
    }

    /// Creates a GroupError event.
    ///
    /// `reason` must be a locally-minted code, never relay text — callers on
    /// the wire path get theirs from `GroupErrorPayload::classify_reason`.
    pub fn group_error(reason: String, group_id: Option<String>) -> Self {
        Self::GroupError { reason, group_id }
    }

    /// Creates a GroupRelaySyncChanged event.
    pub fn group_relay_sync_changed(group_id: String, synced: bool, reason: &str) -> Self {
        Self::GroupRelaySyncChanged {
            group_id,
            synced,
            reason: reason.to_string(),
        }
    }

    /// Creates a GroupMessageSent event.
    pub fn group_message_sent(
        group_id: String,
        message_ids: Vec<String>,
        member_count: u32,
    ) -> Self {
        Self::GroupMessageSent {
            group_id,
            message_ids,
            member_count,
        }
    }

    /// Creates a GroupMessagePartialFailure event.
    pub fn group_message_partial_failure(
        group_id: String,
        failed_members: Vec<String>,
        succeeded_members: Vec<String>,
    ) -> Self {
        Self::GroupMessagePartialFailure {
            group_id,
            failed_members,
            succeeded_members,
        }
    }

    /// Creates a GroupMessageDeliveryReport event.
    pub fn group_message_delivery_report(
        group_id: String,
        message_id: String,
        delivered: Vec<String>,
        pushed: Vec<String>,
        missed_reissued: Vec<String>,
    ) -> Self {
        Self::GroupMessageDeliveryReport {
            group_id,
            message_id,
            delivered,
            pushed,
            missed_reissued,
        }
    }

    /// Creates a GroupRichExtrasDropped event.
    pub fn group_rich_extras_dropped(group_id: String, unknown_members: Vec<String>) -> Self {
        Self::GroupRichExtrasDropped {
            group_id,
            unknown_members,
        }
    }

    /// Creates a GroupUnauthorizedMembershipChange event.
    pub fn group_unauthorized_membership_change(
        group_id: String,
        committer: String,
        added: Vec<String>,
        removed: Vec<String>,
        reason: String,
        enforced: bool,
    ) -> Self {
        Self::GroupUnauthorizedMembershipChange {
            group_id,
            committer,
            added,
            removed,
            reason,
            enforced,
        }
    }

    /// Creates a GroupEpochForkDetected event.
    pub fn group_epoch_fork_detected(group_id: String, local_epoch: Option<u64>) -> Self {
        Self::GroupEpochForkDetected {
            group_id,
            local_epoch,
        }
    }

    /// Creates a GroupEpochForkResolved event.
    pub fn group_epoch_fork_resolved(
        group_id: String,
        resolved_epoch: u64,
        failed_members: Vec<String>,
    ) -> Self {
        Self::GroupEpochForkResolved {
            group_id,
            resolved_epoch,
            failed_members,
        }
    }

    /// Creates a ServiceDiscovered event.
    pub fn service_discovered(
        query_id: String,
        service_id: String,
        version: String,
        provider_peer_id: String,
        capabilities: HashMap<String, String>,
        hop_count: u8,
    ) -> Self {
        Self::ServiceDiscovered {
            query_id,
            service_id,
            version,
            provider_peer_id,
            capabilities,
            hop_count,
        }
    }

    /// Creates a ServiceRequestReceived event.
    pub fn service_request_received(
        request_id: String,
        service_id: String,
        method: String,
        body: String,
        sender: String,
    ) -> Self {
        Self::ServiceRequestReceived {
            request_id,
            service_id,
            method,
            body,
            sender,
        }
    }

    /// Creates a ServiceResponseReceived event.
    pub fn service_response_received(
        request_id: String,
        service_id: String,
        status: String,
        body: String,
        provider_peer_id: String,
    ) -> Self {
        Self::ServiceResponseReceived {
            request_id,
            service_id,
            status,
            body,
            provider_peer_id,
        }
    }

    /// Creates a DorsScoreUpdated event (DORS observability).
    pub fn dors_score_updated(scores: Vec<(String, f32)>) -> Self {
        Self::DorsScoreUpdated { scores }
    }

    /// Creates a DorsTransportSelected event (DORS observability).
    pub fn dors_transport_selected(
        from: Option<String>,
        transport: String,
        reason_code: DorsReasonCode,
        score: f32,
    ) -> Self {
        Self::DorsTransportSelected {
            from,
            transport,
            reason_code,
            score,
        }
    }

    /// Creates a DorsTransportSwitched event (DORS observability).
    /// Emitted only when active transport actually changes after a successful send.
    pub fn dors_transport_switched(
        from: Option<String>,
        to: String,
        reason_code: DorsReasonCode,
        reason_detail: Option<String>,
    ) -> Self {
        Self::DorsTransportSwitched {
            from,
            to,
            reason_code,
            reason_detail,
        }
    }

    /// Creates a DorsEscalationTriggered event (DORS observability).
    /// Use phase Triggered at the DORS trigger boundary, Applied when fallback succeeds.
    pub fn dors_escalation_triggered(
        phase: DorsEscalationPhase,
        from: String,
        to: String,
        reason_code: DorsEscalationReasonCode,
        reason_detail: Option<String>,
    ) -> Self {
        Self::DorsEscalationTriggered {
            phase,
            from,
            to,
            reason_code,
            reason_detail,
        }
    }

    /// Creates a SecurityWarning event.
    pub fn security_warning(
        peer_id: String,
        reason_code: SecurityWarningCode,
        reason: String,
    ) -> Self {
        Self::SecurityWarning {
            peer_id,
            reason_code,
            reason,
        }
    }

    /// Creates a MessageRelayed event.
    pub fn message_relayed(
        message_id: String,
        sender: String,
        recipient: String,
        hop_count: u8,
        remaining_ttl: u8,
    ) -> Self {
        Self::MessageRelayed {
            message_id,
            sender,
            recipient,
            hop_count,
            remaining_ttl,
        }
    }

    /// Creates a UserBlocked event.
    pub fn user_blocked(user_id: String) -> Self {
        Self::UserBlocked { user_id }
    }

    /// Creates a UserUnblocked event.
    pub fn user_unblocked(user_id: String) -> Self {
        Self::UserUnblocked { user_id }
    }

    /// Converts the event to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Parses an event from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Returns the stable `snake.dot.case` telemetry name for this event.
    ///
    /// Names are compatible with OpenTelemetry naming conventions and are
    /// considered stable across minor versions of the SDK.
    pub fn telemetry_name(&self) -> &'static str {
        match self {
            Self::MessageSent { .. } => "protocol.message.sent",
            Self::MessageReceived { .. } => "protocol.message.received",
            Self::MessageDelivered { .. } => "protocol.message.delivered",
            Self::MessageFailed { .. } => "protocol.message.failed",
            Self::MessageDecryptionFailed { .. } => "protocol.message.decryption_failed",
            Self::TransportSwitched { .. } => "protocol.transport.switched",
            Self::RelayPromoted { .. } => "protocol.relay.promoted",
            Self::RelayDemoted { .. } => "protocol.relay.demoted",
            Self::NeighborDiscovered { .. } => "protocol.neighbor.discovered",
            Self::NeighborLost { .. } => "protocol.neighbor.lost",
            Self::IdentityReady { .. } => "protocol.identity.ready",
            Self::NetworkMetrics { .. } => "protocol.network.metrics",
            Self::FileProgress { .. } => "protocol.file.progress",
            Self::FileReceived { .. } => "protocol.file.received",
            Self::FileReceiveFailed { .. } => "protocol.file.receive_failed",
            Self::MediaSent { .. } => "protocol.media.sent",
            Self::MediaSendFailed { .. } => "protocol.media.send_failed",
            Self::MessageDeferred { .. } => "protocol.message.deferred",
            Self::MessageRetrying { .. } => "protocol.message.retrying",
            Self::MessageUndeliverable { .. } => "protocol.message.undeliverable",
            Self::MediaResendRequired { .. } => "protocol.media.resend_required",
            Self::AckEvicted { .. } => "protocol.ack.evicted",
            Self::FragmentAssemblyEvicted { .. } => "protocol.fragment.assembly_evicted",
            Self::RelayDemotedBattery { .. } => "protocol.relay.demoted_battery",
            Self::SecureSessionEstablished { .. } => "protocol.secure_session.established",
            Self::SecureSessionFailed { .. } => "protocol.secure_session.failed",
            Self::ConvergenceDiag { .. } => "protocol.convergence.diag",
            Self::WelcomeSendAttempted { .. } => "protocol.welcome.send_attempted",
            Self::WelcomeSendSucceeded { .. } => "protocol.welcome.send_succeeded",
            Self::WelcomeSendFailed { .. } => "protocol.welcome.send_failed",
            Self::WelcomeSendExpired { .. } => "protocol.welcome.send_expired",
            Self::ConnectionRequestReceived { .. } => "protocol.connection.request_received",
            Self::ConnectionRequestUndeliverable { .. } => {
                "protocol.connection.request_undeliverable"
            }
            Self::ConnectionAccepted { .. } => "protocol.connection.accepted",
            Self::ConnectionRejected { .. } => "protocol.connection.rejected",
            Self::ConnectionRequestCancelled { .. } => "protocol.connection.request_cancelled",
            Self::GroupCreated { .. } => "protocol.group.created",
            Self::GroupMessageReceived { .. } => "protocol.group.message_received",
            Self::GroupMemberAdded { .. } => "protocol.group.member_added",
            Self::GroupMemberRemoved { .. } => "protocol.group.member_removed",
            Self::GroupInfo { .. } => "protocol.group.info",
            Self::UserGroups { .. } => "protocol.group.user_groups",
            Self::GroupError { .. } => "protocol.group.error",
            Self::GroupRelaySyncChanged { .. } => "protocol.group.relay_sync_changed",
            Self::GroupMessageSent { .. } => "protocol.group.message_sent",
            Self::GroupMessagePartialFailure { .. } => "protocol.group.message_partial_failure",
            Self::GroupMessageDeliveryReport { .. } => "protocol.group.delivery_report",
            Self::GroupRichExtrasDropped { .. } => "protocol.group.rich_extras_dropped",
            Self::GroupUnauthorizedMembershipChange { .. } => {
                "protocol.group.unauthorized_membership_change"
            }
            Self::GroupEpochForkDetected { .. } => "protocol.group.epoch_fork_detected",
            Self::GroupEpochForkResolved { .. } => "protocol.group.epoch_fork_resolved",
            Self::GroupRoleChanged { .. } => "protocol.group.role_changed",
            Self::GroupRenamed { .. } => "protocol.group.renamed",
            Self::ServiceDiscovered { .. } => "protocol.service.discovered",
            Self::ServiceRequestReceived { .. } => "protocol.service.request_received",
            Self::ServiceResponseReceived { .. } => "protocol.service.response_received",
            Self::PresenceUpdated { .. } => "protocol.presence.updated",
            Self::TypingIndicatorReceived { .. } => "protocol.typing.received",
            Self::ReadReceiptReceived { .. } => "protocol.read_receipt.received",
            Self::DorsScoreUpdated { .. } => "protocol.dors.score_updated",
            Self::DorsTransportSelected { .. } => "protocol.dors.transport_selected",
            Self::DorsTransportSwitched { .. } => "protocol.dors.transport_switched",
            Self::DorsEscalationTriggered { .. } => "protocol.dors.escalation_triggered",
            Self::SecurityWarning { .. } => "protocol.security.warning",
            Self::MessageRelayed { .. } => "protocol.message.relayed",
            Self::UserBlocked { .. } => "protocol.user.blocked",
            Self::UserUnblocked { .. } => "protocol.user.unblocked",
        }
    }
}

impl From<ServiceEvent> for Event {
    fn from(event: ServiceEvent) -> Self {
        match event {
            ServiceEvent::ServiceDiscovered {
                query_id,
                service_id,
                version,
                provider_peer_id,
                capabilities,
                hop_count,
            } => Self::ServiceDiscovered {
                query_id,
                service_id,
                version,
                provider_peer_id,
                capabilities,
                hop_count,
            },
            ServiceEvent::ServiceRequestReceived {
                request_id,
                service_id,
                method,
                body,
                sender,
            } => Self::ServiceRequestReceived {
                request_id,
                service_id,
                method,
                body,
                sender,
            },
            ServiceEvent::ServiceResponseReceived {
                request_id,
                service_id,
                status,
                body,
                provider_peer_id,
            } => Self::ServiceResponseReceived {
                request_id,
                service_id,
                status,
                body,
                provider_peer_id,
            },
        }
    }
}

/// Custom Debug implementation that redacts sensitive fields to prevent
/// accidental logging of message content and other potentially sensitive data.
impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MessageSent {
                message_id,
                sender: _,
                recipient: _,
                content,
                priority,
                requires_ack,
                timestamp,
                lamport_clock,
                forward_info,
            } => f
                .debug_struct("MessageSent")
                .field("message_id", message_id)
                .field("sender", &"[REDACTED]")
                .field("recipient", &"[REDACTED]")
                .field("content", &format!("[REDACTED {} bytes]", content.len()))
                .field("priority", priority)
                .field("requires_ack", requires_ack)
                .field("timestamp", timestamp)
                .field("lamport_clock", lamport_clock)
                .field("forward_info", &forward_info.is_some())
                .finish(),
            Self::MessageReceived {
                message_id,
                sender: _,
                recipient: _,
                content,
                hop_count,
                transport,
                timestamp,
                lamport_clock,
                reply_to_msg: _,
                reply_context,
                content_type,
                media_metadata: _,
                forward_info,
                encrypted,
            } => f
                .debug_struct("MessageReceived")
                .field("message_id", message_id)
                .field("sender", &"[REDACTED]")
                .field("recipient", &"[REDACTED]")
                .field("content", &format!("[REDACTED {} bytes]", content.len()))
                .field("hop_count", hop_count)
                .field("transport", transport)
                .field("timestamp", timestamp)
                .field("lamport_clock", lamport_clock)
                .field("reply_to_msg", &"[REDACTED]")
                .field("reply_context", &reply_context.is_some())
                .field("content_type", content_type)
                .field("forward_info", &forward_info.is_some())
                .field("encrypted", encrypted)
                .finish(),
            Self::MessageDelivered {
                message_id,
                latency_ms,
                hop_count,
                transport,
            } => f
                .debug_struct("MessageDelivered")
                .field("message_id", message_id)
                .field("latency_ms", latency_ms)
                .field("hop_count", hop_count)
                .field("transport", transport)
                .finish(),
            Self::MessageFailed {
                message_id,
                reason,
                retry_count,
            } => f
                .debug_struct("MessageFailed")
                .field("message_id", message_id)
                .field("reason", reason)
                .field("retry_count", retry_count)
                .finish(),
            Self::MessageDecryptionFailed {
                message_id,
                sender: _,
                code,
                reason,
            } => f
                .debug_struct("MessageDecryptionFailed")
                .field("message_id", message_id)
                .field("sender", &"[REDACTED]")
                .field("code", code)
                .field("reason", reason)
                .finish(),
            Self::TransportSwitched { from, to, reason } => f
                .debug_struct("TransportSwitched")
                .field("from", from)
                .field("to", to)
                .field("reason", reason)
                .finish(),
            Self::RelayPromoted {
                connection_count,
                battery_level,
            } => f
                .debug_struct("RelayPromoted")
                .field("connection_count", connection_count)
                .field("battery_level", battery_level)
                .finish(),
            Self::RelayDemoted { reason } => f
                .debug_struct("RelayDemoted")
                .field("reason", reason)
                .finish(),
            Self::NeighborDiscovered {
                peer_id: _,
                transport,
                rssi,
            } => f
                .debug_struct("NeighborDiscovered")
                .field("peer_id", &"[REDACTED]")
                .field("transport", transport)
                .field("rssi", rssi)
                .finish(),
            Self::NeighborLost { peer_id: _ } => f
                .debug_struct("NeighborLost")
                .field("peer_id", &"[REDACTED]")
                .finish(),
            // Redacted like any other identifier: this one names *this* device,
            // which is the strongest correlator a log can carry.
            Self::IdentityReady { address: _ } => f
                .debug_struct("IdentityReady")
                .field("address", &"[REDACTED]")
                .finish(),
            Self::NetworkMetrics {
                neighbor_count,
                relay_count,
                delivery_ratio,
                avg_latency_ms,
            } => f
                .debug_struct("NetworkMetrics")
                .field("neighbor_count", neighbor_count)
                .field("relay_count", relay_count)
                .field("delivery_ratio", delivery_ratio)
                .field("avg_latency_ms", avg_latency_ms)
                .finish(),
            Self::FileProgress {
                file_id,
                chunks_sent,
                total_chunks,
                percentage,
            } => f
                .debug_struct("FileProgress")
                .field("file_id", file_id)
                .field("chunks_sent", chunks_sent)
                .field("total_chunks", total_chunks)
                .field("percentage", percentage)
                .finish(),
            Self::FileReceived {
                file_id,
                file_name,
                file_size,
                sender: _,
                content_type,
                media_metadata: _,
                file_data,
                timestamp,
                caption,
                reply_to_msg: _,
                reply_context,
                forward_info,
            } => f
                .debug_struct("FileReceived")
                .field("file_id", file_id)
                .field("file_name", file_name)
                .field("file_size", file_size)
                .field("sender", &"[REDACTED]")
                .field("content_type", content_type)
                .field("file_data", &format!("[{} bytes base64]", file_data.len()))
                .field("timestamp", timestamp)
                .field("caption", &caption.is_some())
                .field("reply_to_msg", &"[REDACTED]")
                .field("reply_context", &reply_context.is_some())
                .field("forward_info", &forward_info.is_some())
                .finish(),
            Self::FileReceiveFailed {
                file_id,
                file_name,
                sender: _,
                reason,
            } => f
                .debug_struct("FileReceiveFailed")
                .field("file_id", file_id)
                .field("file_name", file_name)
                .field("sender", &"[REDACTED]")
                .field("reason", reason)
                .finish(),
            Self::MediaSent {
                file_id,
                content_type,
                recipient: _,
            } => f
                .debug_struct("MediaSent")
                .field("file_id", file_id)
                .field("content_type", content_type)
                .field("recipient", &"[REDACTED]")
                .finish(),
            Self::MediaSendFailed {
                file_id,
                recipient: _,
                reason,
            } => f
                .debug_struct("MediaSendFailed")
                .field("file_id", file_id)
                .field("recipient", &"[REDACTED]")
                .field("reason", reason)
                .finish(),
            Self::MessageDeferred {
                message_id,
                reason,
                retry_count,
                next_retry_at,
            } => f
                .debug_struct("MessageDeferred")
                .field("message_id", message_id)
                .field("reason", reason)
                .field("retry_count", retry_count)
                .field("next_retry_at", next_retry_at)
                .finish(),
            Self::MessageRetrying {
                message_id,
                recipient: _,
                retry_count,
                next_retry_at,
            } => f
                .debug_struct("MessageRetrying")
                .field("message_id", message_id)
                .field("recipient", &"[REDACTED]")
                .field("retry_count", retry_count)
                .field("next_retry_at", next_retry_at)
                .finish(),
            Self::MessageUndeliverable {
                message_id,
                recipient: _,
                reason,
                file_id,
            } => f
                .debug_struct("MessageUndeliverable")
                .field("message_id", message_id)
                .field("recipient", &"[REDACTED]")
                .field("reason", reason)
                .field("file_id", file_id)
                .finish(),
            Self::MediaResendRequired {
                file_id,
                recipient: _,
                file_name,
                file_size,
            } => f
                .debug_struct("MediaResendRequired")
                .field("file_id", file_id)
                .field("recipient", &"[REDACTED]")
                .field("file_name", file_name)
                .field("file_size", file_size)
                .finish(),
            Self::AckEvicted {
                message_id,
                priority,
                reason,
            } => f
                .debug_struct("AckEvicted")
                .field("message_id", message_id)
                .field("priority", priority)
                .field("reason", reason)
                .finish(),
            Self::FragmentAssemblyEvicted {
                message_id,
                completion_percent,
                reason,
            } => f
                .debug_struct("FragmentAssemblyEvicted")
                .field("message_id", message_id)
                .field("completion_percent", completion_percent)
                .field("reason", reason)
                .finish(),
            Self::RelayDemotedBattery {
                battery_level,
                min_required,
            } => f
                .debug_struct("RelayDemotedBattery")
                .field("battery_level", battery_level)
                .field("min_required", min_required)
                .finish(),
            Self::SecureSessionEstablished {
                peer_id: _,
                group_id,
                is_session,
                initiated_by_local,
            } => f
                .debug_struct("SecureSessionEstablished")
                .field("peer_id", &"[REDACTED]")
                .field("group_id", group_id)
                .field("is_session", is_session)
                .field("initiated_by_local", initiated_by_local)
                .finish(),
            Self::SecureSessionFailed { peer_id: _, reason } => f
                .debug_struct("SecureSessionFailed")
                .field("peer_id", &"[REDACTED]")
                .field("reason", reason)
                .finish(),
            Self::ConvergenceDiag {
                stage,
                peer_id: _,
                detail,
            } => f
                .debug_struct("ConvergenceDiag")
                .field("stage", stage)
                .field("peer_id", &"[REDACTED]")
                .field("detail", detail)
                .finish(),
            Self::WelcomeSendAttempted {
                peer_id: _,
                message_id,
                group_id,
                attempt,
            } => f
                .debug_struct("WelcomeSendAttempted")
                .field("peer_id", &"[REDACTED]")
                .field("message_id", message_id)
                .field("group_id", group_id)
                .field("attempt", attempt)
                .finish(),
            Self::WelcomeSendSucceeded {
                peer_id: _,
                message_id,
                group_id,
                attempt,
            } => f
                .debug_struct("WelcomeSendSucceeded")
                .field("peer_id", &"[REDACTED]")
                .field("message_id", message_id)
                .field("group_id", group_id)
                .field("attempt", attempt)
                .finish(),
            Self::WelcomeSendFailed {
                peer_id: _,
                message_id,
                group_id,
                attempt,
                reason_code,
                transport_error,
                retryable,
                next_retry_at,
            } => f
                .debug_struct("WelcomeSendFailed")
                .field("peer_id", &"[REDACTED]")
                .field("message_id", message_id)
                .field("group_id", group_id)
                .field("attempt", attempt)
                .field("reason_code", reason_code)
                .field("transport_error", transport_error)
                .field("retryable", retryable)
                .field("next_retry_at", next_retry_at)
                .finish(),
            Self::WelcomeSendExpired {
                peer_id: _,
                message_id,
                attempt,
                reason_code,
            } => f
                .debug_struct("WelcomeSendExpired")
                .field("peer_id", &"[REDACTED]")
                .field("message_id", message_id)
                .field("attempt", attempt)
                .field("reason_code", reason_code)
                .finish(),
            Self::ConnectionRequestReceived {
                sender: _,
                sender_name: _,
                timestamp,
                key_package,
                initial_message,
            } => f
                .debug_struct("ConnectionRequestReceived")
                .field("sender", &"[REDACTED]")
                .field("sender_name", &"[REDACTED]")
                .field("timestamp", timestamp)
                .field("has_key_package", &key_package.is_some())
                .field("has_initial_message", &initial_message.is_some())
                .finish(),
            Self::ConnectionRequestUndeliverable {
                recipient: _,
                message_id,
                reason,
            } => f
                .debug_struct("ConnectionRequestUndeliverable")
                .field("recipient", &"[REDACTED]")
                .field("message_id", message_id)
                .field("reason", reason)
                .finish(),
            Self::ConnectionAccepted {
                accepted_by: _,
                accepted_by_name: _,
                timestamp,
                key_package,
            } => f
                .debug_struct("ConnectionAccepted")
                .field("accepted_by", &"[REDACTED]")
                .field("accepted_by_name", &"[REDACTED]")
                .field("timestamp", timestamp)
                .field("has_key_package", &key_package.is_some())
                .finish(),
            Self::ConnectionRejected { rejected_by: _ } => f
                .debug_struct("ConnectionRejected")
                .field("rejected_by", &"[REDACTED]")
                .finish(),
            Self::ConnectionRequestCancelled { cancelled_by: _ } => f
                .debug_struct("ConnectionRequestCancelled")
                .field("cancelled_by", &"[REDACTED]")
                .finish(),
            Self::ServiceDiscovered {
                query_id,
                service_id,
                version,
                provider_peer_id: _,
                capabilities,
                hop_count,
            } => f
                .debug_struct("ServiceDiscovered")
                .field("query_id", query_id)
                .field("service_id", service_id)
                .field("version", version)
                .field("provider_peer_id", &"[REDACTED]")
                .field("capabilities_count", &capabilities.len())
                .field("hop_count", hop_count)
                .finish(),
            Self::ServiceRequestReceived {
                request_id,
                service_id,
                method,
                body,
                sender: _,
            } => f
                .debug_struct("ServiceRequestReceived")
                .field("request_id", request_id)
                .field("service_id", service_id)
                .field("method", method)
                .field("body", &format!("[REDACTED {} bytes]", body.len()))
                .field("sender", &"[REDACTED]")
                .finish(),
            Self::ServiceResponseReceived {
                request_id,
                service_id,
                status,
                body,
                provider_peer_id: _,
            } => f
                .debug_struct("ServiceResponseReceived")
                .field("request_id", request_id)
                .field("service_id", service_id)
                .field("status", status)
                .field("body", &format!("[REDACTED {} bytes]", body.len()))
                .field("provider_peer_id", &"[REDACTED]")
                .finish(),
            Self::GroupCreated { group_id, name } => f
                .debug_struct("GroupCreated")
                .field("group_id", group_id)
                .field("name", name)
                .finish(),
            Self::GroupMessageReceived {
                group_id,
                sender: _,
                content,
                forward_info,
                ..
            } => f
                .debug_struct("GroupMessageReceived")
                .field("group_id", group_id)
                .field("sender", &"[REDACTED]")
                .field("content", &format!("[REDACTED {} bytes]", content.len()))
                .field("forward_info", &forward_info.is_some())
                .finish(),
            Self::GroupMemberAdded {
                group_id,
                user_id: _,
                added_by: _,
                group_name,
                authorized,
            } => f
                .debug_struct("GroupMemberAdded")
                .field("group_id", group_id)
                .field("user_id", &"[REDACTED]")
                .field("added_by", &"[REDACTED]")
                .field("group_name", group_name)
                .field("authorized", authorized)
                .finish(),
            Self::GroupMemberRemoved {
                group_id,
                user_id: _,
                removed_by: _,
                authorized,
            } => f
                .debug_struct("GroupMemberRemoved")
                .field("group_id", group_id)
                .field("user_id", &"[REDACTED]")
                .field("removed_by", &"[REDACTED]")
                .field("authorized", authorized)
                .finish(),
            Self::GroupInfo {
                group_id,
                name,
                created_by: _,
                created_at,
                members,
            } => f
                .debug_struct("GroupInfo")
                .field("group_id", group_id)
                .field("name", name)
                .field("created_by", &"[REDACTED]")
                .field("created_at", created_at)
                .field("members_count", &members.len())
                .finish(),
            Self::UserGroups { groups } => f
                .debug_struct("UserGroups")
                .field("groups_count", &groups.len())
                .finish(),
            Self::GroupError { reason, group_id } => f
                .debug_struct("GroupError")
                .field("reason", reason)
                .field("group_id", group_id)
                .finish(),
            Self::GroupRelaySyncChanged {
                group_id,
                synced,
                reason,
            } => f
                .debug_struct("GroupRelaySyncChanged")
                .field("group_id", group_id)
                .field("synced", synced)
                .field("reason", reason)
                .finish(),
            Self::GroupMessageSent {
                group_id,
                message_ids,
                member_count,
            } => f
                .debug_struct("GroupMessageSent")
                .field("group_id", group_id)
                .field("message_count", &message_ids.len())
                .field("member_count", member_count)
                .finish(),
            Self::GroupMessagePartialFailure {
                group_id,
                failed_members,
                succeeded_members,
            } => f
                .debug_struct("GroupMessagePartialFailure")
                .field("group_id", group_id)
                .field("failed_count", &failed_members.len())
                .field("succeeded_count", &succeeded_members.len())
                .finish(),
            Self::GroupMessageDeliveryReport {
                group_id,
                message_id,
                delivered,
                pushed,
                missed_reissued,
            } => f
                .debug_struct("GroupMessageDeliveryReport")
                .field("group_id", group_id)
                .field("message_id", message_id)
                .field("delivered_count", &delivered.len())
                .field("pushed_count", &pushed.len())
                .field("missed_reissued_count", &missed_reissued.len())
                .finish(),
            Self::GroupRichExtrasDropped {
                group_id,
                unknown_members,
            } => f
                .debug_struct("GroupRichExtrasDropped")
                .field("group_id", group_id)
                .field("unknown_count", &unknown_members.len())
                .finish(),
            Self::GroupUnauthorizedMembershipChange {
                group_id,
                committer: _,
                added,
                removed,
                reason,
                enforced,
            } => f
                .debug_struct("GroupUnauthorizedMembershipChange")
                .field("group_id", group_id)
                .field("committer", &"[REDACTED]")
                .field("added_count", &added.len())
                .field("removed_count", &removed.len())
                .field("reason", reason)
                .field("enforced", enforced)
                .finish(),
            Self::GroupEpochForkDetected {
                group_id,
                local_epoch,
            } => f
                .debug_struct("GroupEpochForkDetected")
                .field("group_id", group_id)
                .field("local_epoch", local_epoch)
                .finish(),
            Self::GroupEpochForkResolved {
                group_id,
                resolved_epoch,
                failed_members,
            } => f
                .debug_struct("GroupEpochForkResolved")
                .field("group_id", group_id)
                .field("resolved_epoch", resolved_epoch)
                .field("failed_members", failed_members)
                .finish(),
            Self::PresenceUpdated {
                peer_id: _,
                status,
                timestamp,
                last_seen_ms,
                source,
            } => f
                .debug_struct("PresenceUpdated")
                .field("peer_id", &"[REDACTED]")
                .field("status", status)
                .field("timestamp", timestamp)
                .field("last_seen_ms", last_seen_ms)
                .field("source", source)
                .finish(),
            Self::TypingIndicatorReceived {
                sender: _,
                conversation_id: _,
                is_typing,
                timestamp,
            } => f
                .debug_struct("TypingIndicatorReceived")
                .field("sender", &"[REDACTED]")
                .field("conversation_id", &"[REDACTED]")
                .field("is_typing", is_typing)
                .field("timestamp", timestamp)
                .finish(),
            Self::ReadReceiptReceived {
                sender: _,
                message_ids,
                timestamp,
            } => f
                .debug_struct("ReadReceiptReceived")
                .field("sender", &"[REDACTED]")
                .field("message_count", &message_ids.len())
                .field("timestamp", timestamp)
                .finish(),
            Self::DorsScoreUpdated { scores } => f
                .debug_struct("DorsScoreUpdated")
                .field("scores", scores)
                .finish(),
            Self::DorsTransportSelected {
                from,
                transport,
                reason_code,
                score,
            } => f
                .debug_struct("DorsTransportSelected")
                .field("from", from)
                .field("transport", transport)
                .field("reason_code", reason_code)
                .field("score", score)
                .finish(),
            Self::DorsTransportSwitched {
                from,
                to,
                reason_code,
                reason_detail,
            } => f
                .debug_struct("DorsTransportSwitched")
                .field("from", from)
                .field("to", to)
                .field("reason_code", reason_code)
                .field("reason_detail", reason_detail)
                .finish(),
            Self::DorsEscalationTriggered {
                phase,
                from,
                to,
                reason_code,
                reason_detail,
            } => f
                .debug_struct("DorsEscalationTriggered")
                .field("phase", phase)
                .field("from", from)
                .field("to", to)
                .field("reason_code", reason_code)
                .field("reason_detail", reason_detail)
                .finish(),
            // The one arm that used to print `peer_id` verbatim, and the worst
            // one to: a security warning is precisely the event an operator
            // dumps into a log while investigating, and the peer it names is
            // usually attacker-controlled — an injected frame carries whatever
            // sender it likes, so this field doubles as a log-injection
            // surface. The telemetry scrubber already hashed it; only `{:?}`
            // disagreed.
            Self::SecurityWarning {
                peer_id: _,
                reason_code,
                reason,
            } => f
                .debug_struct("SecurityWarning")
                .field("peer_id", &"[REDACTED]")
                .field("reason_code", reason_code)
                .field("reason", reason)
                .finish(),
            Self::MessageRelayed {
                message_id,
                sender: _,
                recipient: _,
                hop_count,
                remaining_ttl,
            } => f
                .debug_struct("MessageRelayed")
                .field("message_id", message_id)
                .field("sender", &"[REDACTED]")
                .field("recipient", &"[REDACTED]")
                .field("hop_count", hop_count)
                .field("remaining_ttl", remaining_ttl)
                .finish(),
            Self::UserBlocked { user_id: _ } => f
                .debug_struct("UserBlocked")
                .field("user_id", &"[REDACTED]")
                .finish(),
            Self::UserUnblocked { user_id: _ } => f
                .debug_struct("UserUnblocked")
                .field("user_id", &"[REDACTED]")
                .finish(),
            Self::GroupRoleChanged {
                group_id,
                user_id: _,
                new_role,
                changed_by: _,
            } => f
                .debug_struct("GroupRoleChanged")
                .field("group_id", group_id)
                .field("user_id", &"[REDACTED]")
                .field("new_role", new_role)
                .field("changed_by", &"[REDACTED]")
                .finish(),
            Self::GroupRenamed {
                group_id,
                new_name,
                old_name: _,
                renamed_by: _,
            } => f
                .debug_struct("GroupRenamed")
                .field("group_id", group_id)
                .field("new_name", new_name)
                .field("renamed_by", &"[REDACTED]")
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, Message, MessagePriority, UserId};

    #[test]
    fn test_message_sent_event() {
        let message = Message::builder(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("app").unwrap(),
        )
        .content("Hello")
        .priority(MessagePriority::High)
        .build();

        let event = Event::message_sent(&message);

        match event {
            Event::MessageSent {
                message_id,
                sender,
                recipient,
                content,
                priority,
                requires_ack,
                timestamp,
                lamport_clock,
                forward_info,
            } => {
                assert_eq!(message_id, message.id.as_str());
                assert_eq!(sender, message.sender.as_str());
                assert_eq!(recipient, message.recipient.as_str());
                assert_eq!(content, message.content);
                assert_eq!(priority, "high");
                assert!(requires_ack);
                assert_eq!(timestamp, message.timestamp.as_millis());
                assert_eq!(lamport_clock, message.lamport_clock.value());
                assert!(forward_info.is_none());
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_message_delivered_event() {
        let msg_id = MessageId::new();
        let event = Event::message_delivered(msg_id.clone(), 100, 3, TransportType::BLE);

        match event {
            Event::MessageDelivered {
                message_id,
                latency_ms,
                hop_count,
                transport,
            } => {
                assert_eq!(message_id, msg_id.as_str());
                assert_eq!(latency_ms, 100);
                assert_eq!(hop_count, 3);
                assert_eq!(transport, "ble");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_transport_switched_event() {
        let event = Event::transport_switched(
            Some(TransportType::BLE),
            TransportType::WiFiDirect,
            "Poor signal".to_string(),
        );

        match event {
            Event::TransportSwitched { from, to, reason } => {
                assert_eq!(from.as_deref(), Some("ble"));
                assert_eq!(to, "wifiDirect");
                assert_eq!(reason, "Poor signal");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_relay_promoted_event() {
        let event = Event::relay_promoted(5, 80);

        match event {
            Event::RelayPromoted {
                connection_count,
                battery_level,
            } => {
                assert_eq!(connection_count, 5);
                assert_eq!(battery_level, 80);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_file_progress_event() {
        let event = Event::file_progress("file123".to_string(), 50, 100);

        match event {
            Event::FileProgress {
                file_id,
                chunks_sent,
                total_chunks,
                percentage,
            } => {
                assert_eq!(file_id, "file123");
                assert_eq!(chunks_sent, 50);
                assert_eq!(total_chunks, 100);
                assert_eq!(percentage, 50);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_event_serialization() {
        let event = Event::relay_promoted(5, 80);
        let json = event.to_json().unwrap();
        let deserialized = Event::from_json(&json).unwrap();

        match deserialized {
            Event::RelayPromoted {
                connection_count,
                battery_level,
            } => {
                assert_eq!(connection_count, 5);
                assert_eq!(battery_level, 80);
            }
            _ => panic!("Wrong event type after deserialization"),
        }
    }

    /// Pins the JSON wire shape the RN `types.ts` declarations model
    /// (`ConnectionRequestUndeliverableEvent` / `initial_message` on
    /// `ConnectionRequestReceivedEvent`) — events cross the FFI as JSON, so
    /// this contract is otherwise enforced only by convention.
    #[test]
    fn test_connection_request_event_wire_shapes() {
        let event = Event::connection_request_undeliverable(
            "bob".to_string(),
            "m1".to_string(),
            "recipient_unreachable: User is offline".to_string(),
        );
        let json: serde_json::Value = serde_json::from_str(&event.to_json().unwrap()).unwrap();
        assert_eq!(json["type"], "connection_request_undeliverable");
        assert_eq!(json["recipient"], "bob");
        assert_eq!(json["message_id"], "m1");
        assert_eq!(json["reason"], "recipient_unreachable: User is offline");

        let received = Event::connection_request_received(
            "alice".to_string(),
            "Alice".to_string(),
            12345,
            None,
            Some("hey".to_string()),
        );
        let json: serde_json::Value = serde_json::from_str(&received.to_json().unwrap()).unwrap();
        assert_eq!(json["type"], "connection_request_received");
        assert_eq!(json["initial_message"], "hey");

        // None must omit the key entirely (skip_serializing_if), keeping the
        // event byte-shape identical to pre-initial-message builds.
        let received = Event::connection_request_received(
            "alice".to_string(),
            "Alice".to_string(),
            12345,
            None,
            None,
        );
        assert!(!received.to_json().unwrap().contains("initial_message"));
    }

    /// Pins the `group_error` wire shape (#349).
    ///
    /// `group_id` is additive, so a build that never scopes an error must
    /// serialize byte-identically to pre-change builds — the same
    /// `skip_serializing_if` contract `initial_message` is pinned on above.
    #[test]
    fn test_group_error_event_wire_shape() {
        let scoped = Event::group_error("sync_denied".to_string(), Some("g-1".to_string()));
        let json: serde_json::Value = serde_json::from_str(&scoped.to_json().unwrap()).unwrap();
        assert_eq!(json["type"], "group_error");
        assert_eq!(json["reason"], "sync_denied");
        assert_eq!(json["group_id"], "g-1");

        let unscoped = Event::group_error("error".to_string(), None);
        assert!(!unscoped.to_json().unwrap().contains("group_id"));
    }

    #[test]
    fn test_network_metrics_event() {
        let event = Event::network_metrics(10, 3, 0.95, 150);

        match event {
            Event::NetworkMetrics {
                neighbor_count,
                relay_count,
                delivery_ratio,
                avg_latency_ms,
            } => {
                assert_eq!(neighbor_count, 10);
                assert_eq!(relay_count, 3);
                assert_eq!(delivery_ratio, 0.95);
                assert_eq!(avg_latency_ms, 150);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_file_received_event() {
        let event = Event::file_received(
            "file123".to_string(),
            "photo.jpg".to_string(),
            1024000,
            "alice".to_string(),
            ContentType::Image,
            None,
            vec![1, 2, 3, 4],
            None,
            None,
            None,
            None,
            None,
        );

        match event {
            Event::FileReceived {
                file_id,
                file_name,
                file_size,
                sender,
                content_type,
                file_data,
                ..
            } => {
                assert_eq!(file_id, "file123");
                assert_eq!(file_name, "photo.jpg");
                assert_eq!(file_size, 1024000);
                assert_eq!(sender, "alice");
                assert_eq!(content_type, "image");
                assert!(!file_data.is_empty());
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_file_received_event_rich_fields_absent_from_json_when_none() {
        // The rich fields are additive on the event's JSON wire form: a
        // plain transfer must serialize exactly as it did before they
        // existed, so older consumers see no new keys.
        let event = Event::file_received(
            "file123".to_string(),
            "f.bin".to_string(),
            4,
            "alice".to_string(),
            ContentType::File,
            None,
            vec![1, 2, 3, 4],
            None,
            None,
            None,
            None,
            None,
        );
        let json = serde_json::to_value(&event).unwrap();
        for key in [
            "timestamp",
            "caption",
            "reply_to_msg",
            "reply_context",
            "forward_info",
        ] {
            assert!(json.get(key).is_none(), "{} must be absent when unset", key);
        }
    }

    #[test]
    fn test_file_received_event_rich_fields_round_trip() {
        use offline_protocol_core::UserId;
        let rc = offline_protocol_core::ReplyContext {
            sender: UserId::new("carol").unwrap(),
            text: "the original".to_string(),
            timestamp: None,
            reply_media_label: None,
            reply_content_type: Some("text".to_string()),
        };
        let fwd = offline_protocol_core::ForwardInfo {
            original_sender: UserId::new("dave").unwrap(),
            original_message_id: offline_protocol_core::MessageId::new(),
            original_timestamp: offline_protocol_core::Timestamp::now(),
            forward_count: 1,
        };
        let event = Event::file_received(
            "file123".to_string(),
            "photo.jpg".to_string(),
            1024,
            "alice".to_string(),
            ContentType::Image,
            None,
            vec![1, 2, 3],
            Some(1_700_000_000_000),
            Some("look".to_string()),
            Some("0192aaaa-bbbb-cccc-dddd-eeeeffff0000".to_string()),
            Some(&rc),
            Some(&fwd),
        );
        let json = serde_json::to_string(&event).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();
        match parsed {
            Event::FileReceived {
                timestamp,
                caption,
                reply_to_msg,
                reply_context,
                forward_info,
                ..
            } => {
                assert_eq!(timestamp, Some(1_700_000_000_000));
                assert_eq!(caption.as_deref(), Some("look"));
                assert_eq!(
                    reply_to_msg.as_deref(),
                    Some("0192aaaa-bbbb-cccc-dddd-eeeeffff0000")
                );
                let rc = reply_context.expect("reply_context must round-trip");
                assert_eq!(rc.sender, "carol");
                assert_eq!(rc.text, "the original");
                let fwd = forward_info.expect("forward_info must round-trip");
                assert_eq!(fwd.original_sender, "dave");
                assert_eq!(fwd.forward_count, 1);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_security_warning_code_as_str_matches_serde_wire_form() {
        // `SecurityWarningCode::as_str()` is hand-written, while the JSON wire
        // form is derived from `#[serde(rename_all = "SCREAMING_SNAKE_CASE")]`.
        // JS consumers branch on the wire string (e.g. keying reinstall handling
        // off `SENDER_ADDRESS_MISMATCH`), so the two must never drift. Pin every
        // variant to its serialized form — a single-variant test would let the
        // others rot.
        let all = [
            SecurityWarningCode::SenderAddressMismatch,
            SecurityWarningCode::TransportIdentityMismatch,
            SecurityWarningCode::ControlSignatureInvalid,
            SecurityWarningCode::UnsignedControlRejected,
            SecurityWarningCode::MediaSenderGroupMismatch,
            SecurityWarningCode::PlaintextSend,
            SecurityWarningCode::PlaintextReceiveRejected,
            SecurityWarningCode::SessionSenderGroupMismatch,
            SecurityWarningCode::SessionRekeyTriggered,
            SecurityWarningCode::NostrKeyPackageSlotExhausted,
            SecurityWarningCode::PushKeyPackagePoolExhausted,
            SecurityWarningCode::RelayAddressBindingMismatch,
            SecurityWarningCode::RelayAddressDeclarationRefused,
            SecurityWarningCode::GroupLeafIdentityUnproven,
        ];
        for code in all {
            // serde renders a unit enum variant as a quoted JSON string.
            let wire = serde_json::to_string(&code).unwrap();
            assert_eq!(
                format!("\"{}\"", code.as_str()),
                wire,
                "as_str() drifted from the serde wire form for {code:?}",
            );
            // Exhaustiveness guard (no wildcard): adding a variant makes this
            // match fail to compile until it is also added to `all` above and
            // pinned to its wire form.
            match code {
                SecurityWarningCode::SenderAddressMismatch
                | SecurityWarningCode::TransportIdentityMismatch
                | SecurityWarningCode::ControlSignatureInvalid
                | SecurityWarningCode::UnsignedControlRejected
                | SecurityWarningCode::MediaSenderGroupMismatch
                | SecurityWarningCode::PlaintextSend
                | SecurityWarningCode::PlaintextReceiveRejected
                | SecurityWarningCode::SessionSenderGroupMismatch
                | SecurityWarningCode::SessionRekeyTriggered
                | SecurityWarningCode::NostrKeyPackageSlotExhausted
                | SecurityWarningCode::PushKeyPackagePoolExhausted
                | SecurityWarningCode::RelayAddressBindingMismatch
                | SecurityWarningCode::RelayAddressDeclarationRefused
                | SecurityWarningCode::GroupLeafIdentityUnproven => {}
            }
        }
    }

    #[test]
    fn test_debug_redacts_every_peer_identifying_field() {
        // `SecurityWarning` printed its `peer_id` verbatim while the other
        // twelve peer-bearing arms redacted theirs, so a single missed arm was
        // invisible: `{:?}` on any *other* event looked correct. Pinning one
        // arm would leave the next omission just as invisible, so this
        // enumerates every variant carrying a peer-identifying field and
        // asserts the sentinel never survives formatting.
        //
        // Deliberately a *value* assertion rather than a field-name one: the
        // question is whether the id reaches the log, not how the arm spells
        // its placeholder.
        const SENTINEL: &str = "off1qy4aspkf0u8qptc6rlpn9ra8vw5jd9ereq4cwpfs";

        let events = vec![
            Event::NeighborDiscovered {
                peer_id: SENTINEL.to_string(),
                transport: "ble".to_string(),
                rssi: Some(-40),
            },
            Event::NeighborLost {
                peer_id: SENTINEL.to_string(),
            },
            Event::SecureSessionEstablished {
                peer_id: SENTINEL.to_string(),
                group_id: "session:a:b".to_string(),
                is_session: true,
                initiated_by_local: true,
            },
            Event::SecureSessionFailed {
                peer_id: SENTINEL.to_string(),
                reason: "nope".to_string(),
            },
            Event::ConvergenceDiag {
                stage: "stage".to_string(),
                peer_id: SENTINEL.to_string(),
                detail: "detail".to_string(),
            },
            Event::WelcomeSendAttempted {
                peer_id: SENTINEL.to_string(),
                message_id: "m1".to_string(),
                group_id: "g1".to_string(),
                attempt: 1,
            },
            Event::WelcomeSendSucceeded {
                peer_id: SENTINEL.to_string(),
                message_id: "m1".to_string(),
                group_id: "g1".to_string(),
                attempt: 1,
            },
            Event::WelcomeSendFailed {
                peer_id: SENTINEL.to_string(),
                message_id: "m1".to_string(),
                group_id: "g1".to_string(),
                attempt: 1,
                reason_code: WelcomeReasonCode::Timeout,
                transport_error: None,
                retryable: true,
                next_retry_at: None,
            },
            Event::WelcomeSendExpired {
                peer_id: SENTINEL.to_string(),
                message_id: "m1".to_string(),
                attempt: 3,
                reason_code: WelcomeReasonCode::RetryExhausted,
            },
            Event::ServiceDiscovered {
                query_id: "q1".to_string(),
                service_id: "s1".to_string(),
                version: "1".to_string(),
                provider_peer_id: SENTINEL.to_string(),
                capabilities: HashMap::new(),
                hop_count: 1,
            },
            Event::ServiceResponseReceived {
                request_id: "r1".to_string(),
                service_id: "s1".to_string(),
                status: "ok".to_string(),
                body: "body".to_string(),
                provider_peer_id: SENTINEL.to_string(),
            },
            Event::PresenceUpdated {
                peer_id: SENTINEL.to_string(),
                status: PresenceStatus::Online,
                timestamp: 0,
                last_seen_ms: None,
                source: PresenceSource::Peer,
            },
            Event::SecurityWarning {
                peer_id: SENTINEL.to_string(),
                reason_code: SecurityWarningCode::SenderAddressMismatch,
                reason: "mismatch".to_string(),
            },
        ];

        for event in &events {
            let rendered = format!("{:?}", event);
            assert!(
                !rendered.contains(SENTINEL),
                "Debug leaked a peer identifier: {rendered}",
            );
        }
    }

    /// Mirrors serde's `rename_all = "snake_case"` rule.
    fn serde_snake_case(name: &str) -> String {
        let mut out = String::new();
        for (i, ch) in name.chars().enumerate() {
            if ch.is_ascii_uppercase() {
                if i > 0 {
                    out.push('_');
                }
                out.push(ch.to_ascii_lowercase());
            } else {
                out.push(ch);
            }
        }
        out
    }

    /// Drift guard: every `Event` variant must have a typed declaration in the
    /// React Native bindings. Events cross UniFFI as tagged JSON, so nothing
    /// else fails when `types.ts` lags — the event just arrives untyped and
    /// invisible to the `ProtocolEvent` union (7 variants had silently drifted
    /// before this guard existed).
    #[test]
    fn react_native_types_cover_all_event_variants() {
        use std::collections::BTreeSet;

        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let events_rs = std::fs::read_to_string(manifest_dir.join("src/events.rs")).unwrap();
        let types_ts_path = manifest_dir.join("../../bindings/react-native/src/types.ts");
        let types_ts = std::fs::read_to_string(&types_ts_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", types_ts_path.display()));

        // Collect each `Event` variant's serde tag by scanning the enum block:
        // variant names sit at 4-space indent, brace depth 1.
        let mut rust_tags = BTreeSet::new();
        let mut in_enum = false;
        let mut depth = 0usize;
        for line in events_rs.lines() {
            if !in_enum {
                if line.starts_with("pub enum Event {") {
                    in_enum = true;
                    depth = 1;
                }
                continue;
            }
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("#[") {
                continue;
            }
            if depth == 1
                && line.starts_with("    ")
                && !line.starts_with("     ")
                && trimmed
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase())
            {
                let name: String = trimmed
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                rust_tags.insert(serde_snake_case(&name));
            }
            depth += line.matches('{').count();
            depth = depth.saturating_sub(line.matches('}').count());
            if depth == 0 {
                break;
            }
        }
        assert!(
            rust_tags.len() >= 60,
            "enum scan looks broken: only {} variants found",
            rust_tags.len()
        );

        // Event interface discriminants in types.ts: `  type: '<tag>';` at
        // exactly 2-space indent (nested object fields sit deeper).
        let ts_tags: BTreeSet<String> = types_ts
            .lines()
            .filter_map(|l| l.strip_prefix("  type: '"))
            .filter_map(|rest| rest.strip_suffix("';"))
            .map(str::to_string)
            .collect();

        // Emitted by the native iOS/Android bridges, not the core enum.
        let bridge_only: BTreeSet<String> = [
            "diagnostic",
            "internet_server_message",
            "internet_status_changed",
            "internet_session_superseded",
            // Android only: the mesh foreground service's notification has no
            // iOS counterpart.
            "mesh_stopped_by_user",
        ]
        .map(str::to_string)
        .into();

        let missing: Vec<_> = rust_tags.difference(&ts_tags).collect();
        assert!(
            missing.is_empty(),
            "Event variants missing a typed declaration in bindings/react-native/src/types.ts \
             (add the interface AND its ProtocolEvent union entry): {missing:?}"
        );

        let stale: Vec<_> = ts_tags
            .difference(&rust_tags)
            .filter(|t| !bridge_only.contains(*t))
            .collect();
        assert!(
            stale.is_empty(),
            "types.ts declares event tags with no core Event variant \
             (stale, or a new bridge-only event that needs allowlisting here): {stale:?}"
        );
    }

    /// Drift guard for the *payload* side of the same seam: apps switch on
    /// `reason_code`, so a code with no entry in the RN union is invisible to
    /// exhaustive handling on the app side and silently falls into whatever
    /// default a `switch` has. `NOSTR_KEY_PACKAGE_SLOT_EXHAUSTED` had already
    /// drifted this way before this guard existed.
    #[test]
    fn react_native_types_cover_all_security_warning_codes() {
        use std::collections::BTreeSet;

        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let events_rs = std::fs::read_to_string(manifest_dir.join("src/events.rs")).unwrap();
        let types_ts_path = manifest_dir.join("../../bindings/react-native/src/types.ts");
        let types_ts = std::fs::read_to_string(&types_ts_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", types_ts_path.display()));

        // The wire forms are exactly the string literals `as_str` maps to, and
        // the test above already pins those to serde's rendering.
        // Anchored on the impl block: several enums in this file have an
        // identical `as_str` signature, and the first match is a different one.
        let as_str_body = events_rs
            .split_once("impl SecurityWarningCode {")
            .expect("impl SecurityWarningCode not found")
            .1
            .split_once("    pub fn as_str(&self) -> &'static str {")
            .expect("SecurityWarningCode::as_str not found")
            .1
            .split_once("\n    }")
            .expect("as_str body not terminated")
            .0;
        let rust_codes: BTreeSet<String> = as_str_body
            .lines()
            .filter_map(|l| l.split_once("=> \""))
            .filter_map(|(_, rest)| rest.split_once('"'))
            .map(|(code, _)| code.to_string())
            .collect();

        // Negative control: a scan that silently matched nothing would make
        // every assertion below vacuously true. The floor tracks the variant
        // count loosely — it exists to catch a broken scan, not to freeze the
        // taxonomy, so it moves when variants are deliberately added or (as
        // when the TOFU codes were deleted) removed.
        assert!(
            rust_codes.len() >= 11,
            "as_str scan looks broken: only {} codes found",
            rust_codes.len()
        );

        let union = types_ts
            .split_once("export type SecurityWarningCode =")
            .expect("SecurityWarningCode union not found in types.ts")
            .1
            .split_once(';')
            .expect("SecurityWarningCode union not terminated")
            .0;
        let ts_codes: BTreeSet<String> = union
            .lines()
            .filter_map(|l| l.trim().strip_prefix("| '"))
            .filter_map(|rest| rest.strip_suffix('\''))
            .map(str::to_string)
            .collect();

        let missing: Vec<_> = rust_codes.difference(&ts_codes).collect();
        assert!(
            missing.is_empty(),
            "SecurityWarningCode variants missing from the union in \
             bindings/react-native/src/types.ts: {missing:?}"
        );

        let stale: Vec<_> = ts_codes.difference(&rust_codes).collect();
        assert!(
            stale.is_empty(),
            "types.ts declares security warning codes with no Rust variant: {stale:?}"
        );
    }
}
