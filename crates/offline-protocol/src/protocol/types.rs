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
/// `PeerUnreachable`, on every carrier (see
/// `apply_recipient_unreachable_failure`). Each consecutive unreachable park
/// doubles the interval from [`WELCOME_NO_CARRIER_RETRY_SECS`] up to this
/// cap: DORS may keep selecting the internet path for the timed retry, and
/// every such round trips another relay `DeliveryError` with the attempt
/// refunded — without escalation that is an unbounded 15s resend loop into
/// the relay for as long as the peer stays offline. At the cap the steady
/// state matches the presence-rescue cadence (one send per 10 min), which is
/// cheap and self-resolving. Shared by the plain-DM unreachable probe
/// (`handle_recipient_unreachable_for_message`, per-peer
/// `dm_unreachable_parks` counter) for the same reason — and there the
/// escalation carries the whole bound, since that probe runs on every carrier
/// (see `park_unreachable_dm`), including internet-only devices.
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
/// instead of burning a retry attempt, a welcome refunds it and parks on an
/// escalating reachability probe (`apply_recipient_unreachable_failure`), and
/// a plain DM parks the same way (`park_unreachable_dm`). The escalation, not
/// a carrier guard, is what keeps those probes from becoming a resend loop
/// into the relay.
///
/// Cross-layer contract: the React Native platform bridges
/// (`InternetManager.kt` / `InternetManager.swift`) hardcode this literal when
/// calling `internet_send_failed_with_reason` — keep them in sync.
pub(crate) const SEND_FAIL_REASON_RECIPIENT_UNREACHABLE: &str = "recipient_unreachable";

/// Fallback token for a send failure that classifies as nothing more specific.
pub(crate) const SEND_FAIL_REASON_TRANSPORT: &str = "transport_send_failed";
/// A Welcome was written to a carrier that never confirmed it.
pub(crate) const SEND_FAIL_REASON_CONFIRM_TIMEOUT: &str = "send_confirmation_timed_out";

/// Every token [`classify_transport_send_error`] and
/// [`send_failure_token`] can produce.
///
/// Membership is what makes classification idempotent: a value already drawn
/// from this list re-classifies to itself, so a record written by one producer
/// and re-read by the other is not degraded to the fallback. Keep it in sync
/// with the two functions — the round-trip test
/// `every_send_failure_token_classifies_to_itself` fails otherwise.
pub(crate) const SEND_FAIL_REASON_TOKENS: &[&str] = &[
    SEND_FAIL_REASON_RECIPIENT_UNREACHABLE,
    SEND_FAIL_REASON_TRANSPORT,
    SEND_FAIL_REASON_CONFIRM_TIMEOUT,
    "transport_not_connected",
    "peer_not_reachable",
    "message_too_large",
    "serialization_failed",
    "crypto_failed",
    "session_not_ready",
    "recipient_blocked",
    "media_transfer_limit",
    "group_not_found",
    "invalid_request",
    "protocol_state_invalid",
];

/// Maps a platform-supplied or previously-stored send-failure string onto a
/// fixed local vocabulary.
///
/// # Why the string cannot be passed through
///
/// The bridges build this string from the relay's `DeliveryError.reason`
/// (`"recipient_unreachable: <relay text>"` — the shape is a cross-layer
/// contract, see [`SEND_FAIL_REASON_RECIPIENT_UNREACHABLE`]), and the Nostr
/// bridge from a relay's `OK` rejection text. Both tails are remote-chosen,
/// unbounded, and not ours. They reach `MessageUndeliverable.reason`,
/// `ConnectionRequestUndeliverable.reason` and — through
/// `WelcomeLifecycleRecord::last_transport_error` —
/// `WelcomeSendFailed.transport_error`, every one of which the telemetry
/// scrubber ships verbatim beside a `recipient`/`peer_id` it *hashes*. An
/// identifier in the tail therefore undoes the hashing in the same record, and
/// even the honest relay writes one: its own wording renders the peer it
/// concerns.
///
/// # Why the prefix survives
///
/// The `recipient_unreachable` token is not decoration — core prefix-matches it
/// to fast-fail connection requests and to park plain DMs, and the bridges
/// hardcode it. So the classification *is* the bare token: everything the
/// protocol reads is kept, and only the prose tail is dropped.
///
/// Matching is prefix-based for that one token (the bridges append to it) and
/// exact otherwise, with a closed fallback — a bridge or relay rewording
/// degrades to [`SEND_FAIL_REASON_TRANSPORT`], never back to shipping its text.
/// The `&'static str` return is load-bearing the same way
/// [`GroupErrorPayload::classify_reason`] and `MlsError::privacy_safe_reason`
/// are: it makes interpolating the input unrepresentable rather than merely
/// discouraged.
///
/// Idempotent by construction, which matters twice: a record persisted before
/// this existed is classified when it is *read* (the stored string may be raw
/// relay prose), and a record written after it is classified again on the way
/// out without changing.
pub(crate) fn classify_transport_send_error(raw: &str) -> &'static str {
    // Prefix, not equality: this is the one token the bridges extend.
    if raw.starts_with(SEND_FAIL_REASON_RECIPIENT_UNREACHABLE) {
        return SEND_FAIL_REASON_RECIPIENT_UNREACHABLE;
    }
    // Already classified: a re-read of a record either producer wrote. Returns
    // the entry from the table rather than `raw`, which is what keeps the
    // signature `&'static str`.
    if let Some(token) = SEND_FAIL_REASON_TOKENS.iter().find(|t| **t == raw) {
        return token;
    }
    match raw {
        // The fixed literals the UniFFI layer substitutes when a bridge reports
        // a failure without a reason of its own. Locally minted, but classified
        // here too so the vocabulary has exactly one source.
        "Internet transport send failed"
        | "Reticulum transport send failed"
        | "Nostr transport send failed" => SEND_FAIL_REASON_TRANSPORT,
        "Welcome send confirmation timed out" => SEND_FAIL_REASON_CONFIRM_TIMEOUT,
        // Everything else, including anything a relay or bridge chose.
        _ => SEND_FAIL_REASON_TRANSPORT,
    }
}

/// Classifies a locally raised send failure for an event field.
///
/// The sibling of [`classify_transport_send_error`] for errors this device
/// produced rather than received. It exists for the same reason: the transport
/// layer renders the counterparty into its own message —
/// `PeerNotReachable("no mesh transport holds a link to {peer_id}…")`, and the
/// same in the BLE and Wi-Fi Direct backends — so `format!("{}", err)` on a
/// routine send failure ships the peer's address to a telemetry sink. On
/// `MessageDeferred` that was the *only* identity in the record, since the
/// event carried no field for the scrubber to hash.
///
/// The match over [`crate::Error`] is exhaustive with no `_` arm: it lives in
/// the defining crate, so a new variant fails to compile here and forces the
/// privacy decision where the variant is written. The transport arm delegates
/// to `offline_protocol_transport::Error::code`, which is that crate's own
/// exhaustive, documented-stable classifier — its doc names telemetry
/// classification as the intended use. Matching its output needs a fallback
/// arm (a foreign `#[non_exhaustive]` enum can grow a variant), and that
/// fallback is closed: a new transport variant degrades to
/// [`SEND_FAIL_REASON_TRANSPORT`], never to its rendered text.
pub(crate) fn send_failure_token(err: &crate::Error) -> &'static str {
    use crate::Error as E;
    match err {
        E::Transport(inner) => match inner.code() {
            "TRANSPORT_NOT_AVAILABLE" => "transport_not_connected",
            "PEER_NOT_REACHABLE" => "peer_not_reachable",
            "MESSAGE_TOO_LARGE" => "message_too_large",
            "SERIALIZATION_ERROR" => "serialization_failed",
            "CRYPTO_ERROR" => "crypto_failed",
            _ => SEND_FAIL_REASON_TRANSPORT,
        },
        E::NotStarted | E::AlreadyStarted | E::InvalidState(_) => "protocol_state_invalid",
        E::InvalidConfiguration(_) | E::InvalidArgument(_) | E::PermissionDenied(_) => {
            "invalid_request"
        }
        E::Core(_) | E::Serialization(_) => "serialization_failed",
        E::Router(_) | E::Reliability(_) | E::Service(_) => SEND_FAIL_REASON_TRANSPORT,
        E::Mls(_) | E::EncryptFailed(_) => "crypto_failed",
        E::MlsNotInitialized | E::SessionNotReady(_) | E::NoKeyPackage(_) => "session_not_ready",
        E::UserBlocked(_) => "recipient_blocked",
        E::MediaTransferLimit(_) => "media_transfer_limit",
        E::GroupNotFound(_) => "group_not_found",
        E::Other(_) => SEND_FAIL_REASON_TRANSPORT,
    }
}
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
/// Seconds the Nostr receive watermark must advance before it is written back
/// to protocol-state storage. Same debounce role as
/// [`LAMPORT_PERSIST_INTERVAL`]: every inbound relay event moves the mark, and
/// a storage write per message is exactly the I/O the Lamport debounce exists
/// to avoid.
///
/// What an un-flushed gap costs is bounded and one-directional: a launch that
/// loses up to this much advance re-fetches a slightly wider slice of relay
/// history, which dedup absorbs. It can never *skip* messages, because the
/// stale mark is always behind the live one.
pub(crate) const NOSTR_WATERMARK_PERSIST_INTERVAL_SECS: i64 = 300;
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

/// Battery level below which a device stops carrying traffic for other
/// people even while charging.
///
/// Re-exported from the router crate rather than restated here: message
/// forwarding and the relay role apply the same floor, and a device must not
/// keep carrying traffic at a level that would have stripped it of the role.
/// Two copies of the number would eventually disagree, and nothing would
/// notice.
pub(crate) use offline_protocol_router::CRITICAL_RELAY_BATTERY_LEVEL;

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

/// Maximum number of peers retained in the encryption-capability set that gates
/// inbound plaintext.
///
/// Sized to hold every legitimate source at once — the durable capability
/// records plus the MLS session list — with room to spare, so a real deployment
/// never reaches it. The cap exists because the set is keyed by a wire-claimed
/// sender id and nothing else bounds it.
///
/// Note what does *not* bound it: requiring a signature. The control gate does
/// now reject unsigned control frames before dispatch, so every marking path
/// runs on a verified frame — and that changes nothing here. A signature proves
/// the signer owns the address it claims; it does not prove the address belongs
/// to anyone real, and minting one costs a keygen. An attacker spending keygens
/// produces unlimited distinct well-signed peers, so "written only after a
/// signature verifies" is not by itself a bound on anything.
///
/// It bounds the durable `encryption_capable_peers` storage category as well,
/// because `OfflineProtocol::record_encryption_capable` persists only for a
/// peer this cap admitted.
///
/// Unlike the other maps keyed by a wire-claimed id, this one **refuses** at
/// capacity instead of resetting or evicting: forgetting a peer here is the
/// fail-open direction. See `OfflineProtocol::mark_encryption_capable`.
pub(crate) const MAX_ENCRYPTION_CAPABLE_PEERS: usize = 8192;

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

/// Maximum number of peers tracked for once-per-peer-per-code control-gate
/// warning suppression.
///
/// Every rejection the control gate reports is reachable from an unauthenticated
/// frame carrying an attacker-chosen sender id — that is what a gate rejection
/// *is* — so without a throttle an off-path injector turns the app's event
/// stream into a flood and desensitizes operators to the one code that has no
/// benign reading (`SENDER_ADDRESS_MISMATCH`). Bounded and reset at capacity for
/// the same reason as [`MAX_PLAINTEXT_RECEIVE_WARNED_PEERS`]: the keys are
/// attacker-controlled, so a forged-sender flood degrades the throttle to
/// once-per-peer-per-generation rather than growing memory without bound.
pub(crate) const MAX_CONTROL_GATE_WARNED_PEERS: usize = 1000;

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

/// Maximum outbound messages waiting for one peer's MLS session.
///
/// This queue is durable and accepts application-controlled content, so a peer
/// that never completes session establishment must not grow it without bound.
/// The limit matches the default inbound pending-decryption per-peer bound.
///
/// A count alone does not bound memory or durable storage — message content is
/// application-supplied — so this cap works together with
/// [`MAX_PENDING_MESSAGE_BYTES_PER_PEER`]; whichever binds first wins.
pub(crate) const MAX_PENDING_MESSAGES_PER_PEER: usize = 64;

/// Maximum outbound messages waiting for MLS sessions across all peers.
///
/// At capacity the globally oldest message is settled as failed before the new
/// message is admitted. The limit matches the default inbound pending queue.
/// Paired with [`MAX_PENDING_MESSAGE_BYTES_GLOBAL`], as above.
pub(crate) const MAX_PENDING_MESSAGES_GLOBAL: usize = 4096;

/// Maximum pending entries expired in one `process()` tick.
///
/// Each expiry is a settlement *and* a durable delete, and the two have to stay
/// paired — so what gets bounded is how many entries expire per pass, not how
/// many deletes it may issue. Messages queued in a burst share a `queued_at` and
/// so come due in a burst; without this, one tick of the 100 ms bindings loop
/// could issue up to [`MAX_PENDING_MESSAGES_GLOBAL`] synchronous deletes, each a
/// device barrier on every built-in provider.
///
/// The remainder is not deferred indefinitely: the entries left behind are still
/// past their deadline, so `recompute_next_pending_message_expiry` leaves
/// `next_pending_message_expiry` in the past and the very next tick drains
/// another pass.
pub(crate) const MAX_PENDING_EXPIRIES_PER_PASS: usize = 64;

/// Maximum per-message records one launch writes migrating legacy queues.
///
/// The migration's writes are the one part of the pending restore that is not
/// already bounded by a delete budget, and a `store` is the more expensive of
/// the two: every built-in provider flushes the record *and* its directory. A
/// pre-split install sitting near [`MAX_PENDING_MESSAGES_GLOBAL`] would
/// otherwise pay thousands of device barriers inside `initialize_mls`, on the
/// launch path, where a mobile watchdog is watching.
///
/// Sized to match [`crate::protocol::storage::MAX_RESTORE_PRUNE_DELETES`], so
/// the whole walk's barrier count stays the same order of magnitude whichever
/// half of it does the work. Checked per recipient and before its first write,
/// so a queue is either migrated whole or left entirely on disk for the next
/// launch — never split across the two layouts with nothing to reconcile it.
pub(crate) const MAX_MIGRATED_PENDING_WRITES_PER_LAUNCH: usize = 512;

/// Maximum terminal settlements parked by restore for `start()` to drain.
///
/// The restore caps already bound how many can be produced, but they bound it
/// as a sum across every category, and nothing drains this queue until
/// `start()` — which an application that only calls `initialize_mls`, or that
/// retries it against a store that keeps failing, may never reach. Sized above
/// the largest single-category restore ([`MAX_PENDING_MESSAGES_GLOBAL`], the
/// pending queue's own global cap) so no legitimate restore is ever truncated;
/// past that point the count is reported instead of the events.
pub(crate) const MAX_DEFERRED_RESTORE_SETTLEMENTS: usize = 2 * MAX_PENDING_MESSAGES_GLOBAL;

/// Maximum size of application-supplied message content accepted at the public
/// send boundary, in bytes.
///
/// Enforced at the boundary — not at transmit time — because a message queued
/// behind MLS session establishment never reaches the transport's
/// `DEFAULT_MAX_MESSAGE_SIZE` (1 MiB) check: it sits in the durable pending
/// queue first. Without a boundary cap a handful of very large sends could
/// exhaust mobile memory and protocol-state disk while still being formally
/// "within" the count caps above.
///
/// 256 KiB leaves ample headroom under the 1 MiB transport ceiling for MLS
/// ciphertext expansion, base64, and the JSON wire envelope, so anything this
/// path accepts can actually be delivered. Larger payloads belong on the media
/// path (`send_media`), which chunks.
pub(crate) const MAX_MESSAGE_CONTENT_BYTES: usize = 256 * 1024;

/// Maximum total serialized bytes of one peer's pending-session queue.
///
/// At capacity the peer's oldest entries are settled as failed until the
/// incoming message fits, exactly like the count cap. Deliberately larger than
/// [`MAX_MESSAGE_CONTENT_BYTES`] plus [`MAX_RICH_EXTRAS_BYTES`] so a single
/// boundary-legal message always fits in an empty queue and admission can never
/// livelock (pinned by `pending_queue_byte_budgets_admit_any_boundary_legal_message`).
///
/// # Durable cost
///
/// Pending messages are persisted one record per message
/// ([`storage_keys::PENDING_MESSAGE_ENTRIES`]), so an enqueue writes its own
/// entry and nothing else: filling one peer to this budget costs `budget` bytes
/// written, not `budget × entries / 2`. The per-recipient layout this replaced
/// re-serialized the peer's whole queue on every enqueue, which made the byte
/// cost quadratic in the entry count.
///
/// Bytes were never the expensive half, though, and sizing this budget off them
/// would be reading the wrong meter. Every built-in provider pays two device
/// barriers per `store` (the record's own flush plus its directory's) and one
/// per `delete`, *regardless of record size* — so a 500-byte write and a 2 MiB
/// write cost the same in the resource that actually dominates. What the
/// per-message layout buys is not primarily speed but that a record which will
/// not open still names the message it lost; see
/// [`storage_keys::PENDING_MESSAGE_ENTRIES`].
///
/// So this budget is still sized to bound a pathological queue rather than to
/// describe a normal one — real queues hold a handful of short messages waiting
/// on a handshake. Lowering it below `MAX_MESSAGE_CONTENT_BYTES +
/// MAX_RICH_EXTRAS_BYTES` breaks admission outright.
pub(crate) const MAX_PENDING_MESSAGE_BYTES_PER_PEER: usize = 2 * 1024 * 1024;

/// Maximum total serialized bytes of the pending-session queue across all
/// peers. Bounds the whole feature's footprint on a mobile device regardless of
/// how many peers are mid-establishment.
pub(crate) const MAX_PENDING_MESSAGE_BYTES_GLOBAL: usize = 16 * 1024 * 1024;

/// Maximum size of a single persisted protocol-state record, in bytes.
///
/// Enforced on both sides of [`crate::ProtocolStateStorage`]: the SDK refuses to
/// write a larger record, and refuses to *deserialize* a larger one on restore
/// (dropping it instead), so a corrupted or tampered state file cannot turn
/// into an unbounded allocation during initialization.
///
/// Sized above [`MAX_PENDING_MESSAGE_BYTES_PER_PEER`] with room for JSON and
/// seal overhead. That was once tight rather than generous: under the
/// per-recipient pending layout the largest legitimate record *was* one peer's
/// full queue, so the two constants were one bound apart. Now that pending
/// messages are keyed per id the largest legitimate record is a single message
/// ([`MAX_MESSAGE_CONTENT_BYTES`] plus [`MAX_RICH_EXTRAS_BYTES`]) or one outbox
/// entry, and the headroom is real. Keep the ordering anyway — a legacy
/// per-recipient record still has to be readable for long enough to migrate it.
pub(crate) const MAX_PROTOCOL_STATE_RECORD_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of peers remembered in `key_package_sent_to` (the "already
/// sent our key package to this peer" set).
///
/// Wire-claimed ids grow this set in lockstep with a key-package flood, so it
/// resets at capacity like [`MAX_PLAINTEXT_RECEIVE_WARNED_PEERS`]: the only cost
/// of forgetting a peer is a one-time idempotent re-send of our key package.
pub(crate) const MAX_KEY_PACKAGE_SENT_TO: usize = 1000;

/// Maximum number of peers tracked in `rekey_due_at` (the per-peer epoch-desync
/// re-key floor).
///
/// The desync classification that reaches `schedule_session_rekey` is produced
/// by OpenMLS *before* it authenticates anything, so the peer id keying this map
/// is wire-claimed. The envelope/sender binding in `SessionManager::decrypt_message`
/// already confines that id to peers we hold a session with, which bounds the map
/// on its own; this cap is the same defence-in-depth every other wire-keyed map
/// carries. Resets at capacity like [`MAX_KEY_PACKAGE_SENT_TO`] — forgetting a
/// peer only costs one extra re-key being allowed through early.
pub(crate) const MAX_REKEY_TRACKED_PEERS: usize = 1000;

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

/// Durable record that a peer has proved it runs MLS.
///
/// Written whenever a control message from that peer verifies — signature
/// valid, and the signing key derives to the address the frame claims — and
/// read back on `initialize_mls` to reseed
/// [`OfflineProtocol::encryption_capable_peers`](crate::OfflineProtocol).
///
/// The timestamp is diagnostic, not a policy input. Its predecessor
/// (`TofuEntry.last_seen_ms`) drove LRU eviction of a bounded pin store; there
/// is no eviction here, because there is no longer any per-peer *secret* whose
/// size needs bounding — only the fact, which the restore walk bounds on its
/// own terms. Keeping the field costs nothing and makes a stale store readable
/// by a human.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct EncryptionCapableEntry {
    /// Milliseconds since epoch (UTC) when this peer last proved it runs MLS.
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

    /// This install's Nostr public key (x-only, 64-char lowercase hex), so a
    /// peer can seal Nostr gift wraps to a key only this install holds.
    ///
    /// `None` on legacy nodes and on installs with Nostr disabled. A peer
    /// without it seals to our *publicly computable* key instead — deliverable
    /// either way, but readable by anyone who guesses our user id, so the
    /// difference is real privacy rather than a mere optimization.
    ///
    /// Trust boundary — **unlike the three capability lists above, this one is
    /// only honoured from a signed key package.** All four ride in the same
    /// plaintext envelope, but this field is consumed as a *destination key*,
    /// not as a feature hint, so the distinction matters: a wrong capability
    /// costs a fallback, whereas a wrong key here means envelope metadata is
    /// sealed *to whoever supplied it* and is then readable off a public relay,
    /// passively, for as long as the value stands.
    ///
    /// `build_canonical_payload` covers the whole `__MLS_KEY_PKG__` body under
    /// the sender's Ed25519 signature, which the gate now verifies against the
    /// key their address derives from — so on this prefix an unsigned frame no
    /// longer reaches dispatch at all.
    /// `handle_key_package_message` still consumes this field only when the gate
    /// reports the frame was actually signed, which costs nothing:
    /// a key package exists only once MLS is initialized, and `send_key_package_to`
    /// signs unconditionally in that state, so every genuine package carrying
    /// this field is signed.
    ///
    /// Stripping it is still possible for a network attacker and downgrades us
    /// to the bootstrap key — which is a privacy downgrade, not a disclosure to
    /// the attacker, and one they could equally achieve by dropping the packet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) nostr_pubkey: Option<String>,
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
    /// Opaque conversation identifier chosen by the sender (conventionally a
    /// peer address for DMs, the group id for groups). Never parsed here.
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
    /// The relay's own wording. **Never emitted on an event** — see
    /// [`GroupErrorPayload::classify_reason`]. Treat every read of this
    /// field as a read of untrusted, unbounded wire input.
    pub(crate) reason: String,
    /// Group the error concerns, when the relay scoped it (e.g. a
    /// registration sync denial). Used to drop the group from
    /// `relay_synced` so sends fall back to per-member delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) group_id: Option<String>,
}

impl GroupErrorPayload {
    /// Maps the relay's free-text `reason` onto a fixed local vocabulary.
    ///
    /// `__GROUP_ERROR__` is a relay answer: it rides
    /// `RELAY_ANSWER_PREFIXES`, so on the relay ingest shape (Internet
    /// arrival, no transport peer identity) it is accepted **unsigned** —
    /// no key material, no session, no prior contact, just a frame on that
    /// socket. Signed, it is reachable to any peer over any transport, since
    /// the handler is deliberately not gated on the Internet path the way
    /// the `GROUP_CREATED` ack is. Either way `reason` is whatever the
    /// sender wrote: arbitrary content, arbitrary length, not ours.
    ///
    /// So the text does not travel any further. Nothing downstream needs it
    /// — the sync revocation keys off `group_id`, and the platform bridges
    /// already dual-emit the raw relay frame on the server-message channel
    /// for apps that want the relay's exact wording. What an event carries
    /// is this classification instead, which an attacker can at most steer
    /// between three harmless codes.
    ///
    /// Even an *honest* relay makes this necessary: its `Not a member of
    /// group {id}` renders an identifier into prose, smuggling past a
    /// telemetry scrubber that hashes `group_id` fields but ships free text
    /// verbatim by design.
    ///
    /// The `&'static str` return is load-bearing, the same way
    /// `MlsError::privacy_safe_reason` is: it makes interpolating wire input
    /// unrepresentable rather than merely discouraged. Matching is exact and
    /// the fallback is closed, so a relay rewording an error degrades to
    /// `error` — never back to shipping its text.
    pub(crate) fn classify_reason(&self) -> &'static str {
        match self.reason.as_str() {
            // Relay has no such group: invite links and relay fan-out for it
            // are dead, not merely refused.
            "Group not found" => "not_found",
            // Relay refused to register/sync the group for this caller.
            "Only admins can sync this group" => "sync_denied",
            // Everything else, including anything a forged frame chose.
            _ => "error",
        }
    }
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

    /// The peer's Nostr public key, from [`KeyPackagePayload::nostr_pubkey`].
    ///
    /// Persisted for the same reason as the capability lists: the cached key
    /// package is deleted once a session is established, and a peer met before
    /// a restart would otherwise silently fall back to bootstrap-key sealing —
    /// a privacy regression with no symptom — until the next live exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) nostr_pubkey: Option<String>,
}

/// Length of an x-only secp256k1 public key in lowercase hex.
pub(crate) const NOSTR_PUBKEY_HEX_LEN: usize = 64;

/// Validates a wire-supplied Nostr public key before it is persisted or used
/// as a sealing destination.
///
/// Shape only — the value is signature-bound to the sender, so this is not an
/// authenticity check. It exists so a malformed or oversized string cannot
/// enter a durable record, and it normalizes case so the same key never
/// persists under two spellings.
pub(crate) fn normalize_nostr_pubkey(candidate: &str) -> Option<String> {
    if candidate.len() != NOSTR_PUBKEY_HEX_LEN || !candidate.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(candidate.to_ascii_lowercase())
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
    pub(crate) fn from_advertised(
        env_versions: &[u8],
        rich_versions: &[u8],
        nostr_pubkey: Option<&str>,
    ) -> Self {
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
            nostr_pubkey: nostr_pubkey.and_then(normalize_nostr_pubkey),
        }
    }

    /// Whether any capability is advertised. Empty records are deleted
    /// rather than stored — the durable side of the downgrade semantics.
    pub(crate) fn is_any(&self) -> bool {
        !self.env_versions.is_empty()
            || !self.rich_versions.is_empty()
            || !self.attested_rich_versions.is_empty()
            || self.nostr_pubkey.is_some()
    }
}

/// Outcome of [`OfflineProtocol::security_gate_control_message`].
///
/// [`OfflineProtocol`]: crate::OfflineProtocol
/// [`OfflineProtocol::security_gate_control_message`]: crate::OfflineProtocol
#[derive(Debug)]
pub(crate) enum ControlGateOutcome {
    /// The message may proceed to dispatch.
    ///
    /// `signed` is `true` **only** when an Ed25519 signature was present,
    /// verified, and produced by the key the sender's address derives from. It
    /// is deliberately `false` for both of the other ways a message reaches
    /// dispatch — a relay-originated answer, which no peer signs
    /// (`RELAY_ANSWER_PREFIXES`), and a prefix the gate does not cover at all —
    /// so a handler that keys on it fails closed without having to know which
    /// case it is in.
    ///
    /// Since unsigned control traffic is now refused outright, `false` no
    /// longer reaches any *gated* handler; the bit survives because those two
    /// ungated paths still produce it, and because a handler that treats a
    /// payload field as authenticated should say so at the point it does.
    Proceed { signed: bool },
    /// The gate rejected the message; the caller must return this result
    /// without dispatching.
    Rejected(InternalMessageResult),
}

/// Result of processing an internal protocol message.
#[derive(Debug)]
pub(crate) enum InternalMessageResult {
    /// Message was consumed internally (don't surface to app).
    Consumed,
    /// Message was rejected by the security gate (spoofed sender, bad
    /// signature, unsigned control traffic, or a signing key that does not
    /// derive to the claimed sender address). Like `Consumed`, the message is not
    /// surfaced to the app — but unlike `Consumed`, a delivery ACK must NOT
    /// be sent back, to avoid confirming to the attacker that the target is
    /// online and processing messages.
    SecurityRejected,
    /// Message was not delivered, but the sender can still recover it by
    /// resending. Unlike `Consumed`, a delivery ACK must NOT be sent and the id
    /// must NOT stay dedup-marked: the message is provably not delivered, so
    /// the receiver must leave the sender's retry lever intact. The receive
    /// loop responds by unmarking the id (so a resend re-enters processing
    /// instead of hitting the duplicate re-ACK path) and skipping the ACK.
    ///
    /// Four conditions produce it, differing in whether the *frame* is worth
    /// keeping:
    ///
    /// - **Session not ready**: the MLS session/group is not established yet,
    ///   so the frame is queued for delayed decryption
    ///   (`enqueue_pending_decryption`). The queued copy is surfaced — and the
    ///   id re-marked — once the session confirms and the queue drains
    ///   (`process_pending_decryption`), which also sends the deferred delivery
    ///   ACK directly on the recorded arrival transport (so a sender that gave
    ///   up before the session confirmed still learns of delivery).
    /// - **Epoch desync**: the frame is sealed to a dead epoch, so it is *not*
    ///   queued (it could never drain) and a rate-limited re-key is triggered.
    /// - **Crypto/transport failure** with `crypto_recovery_enabled`: OpenMLS
    ///   consumed the ratchet generation on the failed attempt, so the frame is
    ///   likewise not queued — and no re-key is triggered, which stays
    ///   desync-only.
    /// - **Envelope parse failure** with `crypto_recovery_enabled`: the
    ///   `__MLS_ENC__` payload did not parse in any envelope form, so there is
    ///   no ciphertext to decrypt. Not queued either — an unparseable frame can
    ///   never become parseable.
    ///
    /// In the latter three, recovery is the sender's *resend* rather than this
    /// frame: Tier 2 re-seals each resend of an encrypted DM against a live
    /// generation, and a message that stays undeliverable settles as an honest
    /// `MessageFailed` instead of a lying "delivered". See
    /// `docs/state-machines/delivery-and-acks.md` for the deferred-acknowledgement
    /// atom and the decrypt-failure classification.
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
    /// (parse failure, resource limit, permanent refusal). The sender should
    /// stop retrying either way, so the caller ACKs as before.
    Handled,
    /// The chunk was not delivered but the sender can still recover it by
    /// resending: it could not be decrypted yet (session not ready, so queued
    /// for delayed decryption), or it is undecryptable as it stands (epoch
    /// desync, or a crypto failure while `crypto_recovery_enabled`), or its
    /// media envelope did not decode at all (also `crypto_recovery_enabled`) —
    /// the latter shapes dropped without queueing. The caller must NOT ACK and
    /// must unmark the id, so the sender keeps retrying and the resend re-enters
    /// processing. For an undecryptable or undecodable chunk that recovery is
    /// the media outbox's `MediaResendRequired` path — chunks are re-encoded,
    /// never re-sealed.
    Deferred,
    /// The chunk was refused as illegitimate rather than undeliverable. Two
    /// shapes: an unencrypted chunk refused by the encryption policy, and an
    /// encrypted chunk that failed its identity binding — the envelope named
    /// another pair's session slot, or the MLS credential authenticated a
    /// different sender than the wire envelope claims.
    ///
    /// Like [`InternalMessageResult::SecurityRejected`] for text, the caller
    /// must NOT ACK (don't confirm to an injector that the target processes
    /// their messages) and must unmark the id (so a replay re-enters this gate
    /// instead of the duplicate re-ACK path), matching the plaintext-text
    /// rejection in the receive loop.
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
    /// When the message *first* entered the pending queue.
    ///
    /// Drives `pending_message_max_lifetime_ms`, and is deliberately preserved
    /// across a re-queue (see [`PendingProvenance`]): a flush that finds the
    /// session still unavailable puts the entry back, and stamping a fresh
    /// timestamp there would let repeated reconciliation renew an "absolute"
    /// lifetime forever.
    pub(crate) queued_at: DateTime<Utc>,
    /// Serialized footprint of this entry, for the pending-queue byte budgets
    /// ([`MAX_PENDING_MESSAGE_BYTES_PER_PEER`] /
    /// [`MAX_PENDING_MESSAGE_BYTES_GLOBAL`]).
    ///
    /// Derived, so it is not persisted; it is (re)computed by
    /// [`PendingMessage::measure`] at the two points an entry comes into
    /// existence — admission and load-from-storage — and then simply travels
    /// with the entry as it is moved between the queue, a flush, and a
    /// re-queue.
    #[serde(skip)]
    pub(crate) serialized_bytes: usize,
}

/// One persisted pending-queue record: a single queued message plus the peer
/// it is queued for.
///
/// The recipient rides *inside* the record because the key is the message id,
/// and the in-memory queue is a map keyed by recipient that has to be rebuilt
/// from records read in whatever order the store enumerates them. Same shape as
/// [`OutboxEntry`], whose recipient likewise travels in the record (inside its
/// `Message`) rather than in the key.
///
/// Deliberately a wrapper rather than a `recipient` field on [`PendingMessage`]:
/// in memory the recipient is already the map key, and a second copy there
/// would be free to drift from it.
#[derive(Serialize, Deserialize)]
pub(crate) struct PendingMessageRecord {
    pub(crate) recipient: String,
    pub(crate) message: PendingMessage,
}

impl PendingMessage {
    /// Recomputes [`Self::serialized_bytes`] from the current field values.
    ///
    /// Measures what persistence actually costs (the serialized entry) rather
    /// than estimating from `content.len()`, so rich extras and forward
    /// attribution are counted. A serialization failure — which would also make
    /// the entry unpersistable — falls back to the content length so the entry
    /// still consumes budget rather than reading as free.
    pub(crate) fn measure(&mut self) {
        self.serialized_bytes = serde_json::to_vec(self)
            .map(|encoded| encoded.len())
            .unwrap_or_else(|_| self.content.len());
    }
}

/// Identity carried by an outbound message that is re-entering the
/// pending-session queue rather than being queued for the first time.
///
/// Both fields exist to keep a re-queue from looking like a fresh send: the id
/// so `MessageSent`/`MessageDelivered`/`MessageFailed` stay correlatable with
/// what `send_message*` returned, and the timestamp so the absolute pending
/// lifetime is measured from first entry.
#[derive(Debug, Clone)]
pub(crate) struct PendingProvenance {
    /// The id the caller already holds.
    pub(crate) message_id: MessageId,
    /// When the message first entered the pending queue, or `None` when the
    /// re-queue does not come *from* that queue — the resend re-seal path
    /// passes an outbox id, and an entry it (all but unreachably) enqueues is
    /// starting its pending lifetime now.
    pub(crate) first_queued_at: Option<DateTime<Utc>>,
}

impl PendingProvenance {
    /// Provenance for a message being put back into the pending queue.
    pub(crate) fn requeued(message: &PendingMessage) -> Self {
        Self {
            message_id: message.message_id.clone(),
            first_queued_at: Some(message.queued_at),
        }
    }

    /// Provenance for a known id with no pending-queue history.
    pub(crate) fn for_id(message_id: MessageId) -> Self {
        Self {
            message_id,
            first_queued_at: None,
        }
    }
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
    /// Legacy key type for pending encrypted messages, keyed by *recipient*,
    /// each record holding that peer's whole queue.
    ///
    /// Superseded by [`PENDING_MESSAGE_ENTRIES`]. Nothing writes this category
    /// any more: `restore_pending_messages` reads it, migrates each queue into
    /// the per-message layout, and deletes the record. It is read-only
    /// migration scaffolding, and retires on the same trigger as the
    /// [`ADOPTABLE_STATE_KEY_TYPES`] sweep — an install that skips the
    /// migrating release entirely still needs it.
    pub const PENDING_MESSAGES: &str = "pending_messages";
    /// Key type for pending encrypted messages, keyed by message id — one
    /// record per queued message, like [`OUTBOX`].
    ///
    /// Keying by id rather than by recipient is what makes a lost record
    /// *individually settleable*: the id the application is holding is the key,
    /// so a record that will not open still names the message it destroyed. The
    /// per-recipient layout could only report the loss per peer, because every
    /// id was inside the record that would not open.
    pub const PENDING_MESSAGE_ENTRIES: &str = "pending_message_entries";
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
    /// Key type for the durable record that a peer has proved it runs MLS.
    ///
    /// Successor to the `tofu_keys` category, which stored a pinned public key
    /// per peer. The pin is gone — an address *is* its key's hash, so nothing
    /// needs storing to check one — but the store had a second job the
    /// derivation does not do: it was the half of
    /// `OfflineProtocol::encryption_capable_peers` that survived a restart, and
    /// therefore what keeps the plaintext-downgrade gate shut for a peer whose
    /// session was torn down. That job is all this category still does, so the
    /// value is a timestamp and nothing else.
    ///
    /// A leftover `tofu_keys` record is left where it is, and no migration is
    /// written for it. Records from before the identity change are keyed by a
    /// username, so no live peer id can ever read them back — they are simply
    /// unreachable. Records written in the window *between* the identity
    /// change and the pin store's deletion are keyed by an address and would
    /// still resolve, so a peer marked capable there with no live session
    /// loses that durable mark across the upgrade: it falls back to the
    /// `is_session_confirmed` check until the next key-package exchange
    /// re-marks it. That is the fail-closed direction and it costs one
    /// exchange, which is why migrating them is not worth the code while no
    /// fleet is deployed.
    pub const ENCRYPTION_CAPABLE_PEERS: &str = "encryption_capable_peers";
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
    /// Key type for the Nostr receive watermark: the newest event `created_at`
    /// (unix seconds) this install has accepted from a relay.
    ///
    /// Unlike the signing secret this is *protocol-state*, not secure storage:
    /// it is a coarse timestamp, not key material, and losing it costs one
    /// wider backfill window rather than an identity.
    pub const NOSTR_WATERMARK: &str = "nostr_watermark";
    /// Key ID for the single Nostr receive-watermark entry.
    pub const NOSTR_WATERMARK_ID: &str = "current";
    /// Key type for the Nostr key-package publication slot map: which MLS key
    /// package currently stands in each published addressable slot.
    ///
    /// Protocol state rather than secure storage, on the same reasoning as the
    /// watermark: the values are slot labels and package ids, both of which are
    /// already public in the published record itself. Losing it costs a round
    /// of republication under fresh slot ids, not an identity — the stale
    /// records left at the old slots expire with their key packages.
    pub const NOSTR_KEY_PACKAGE_SLOTS: &str = "nostr_key_package_slots";
    /// Key ID for the single Nostr publication-slot entry.
    pub const NOSTR_KEY_PACKAGE_SLOTS_ID: &str = "current";
    /// Key type for this install's published username discovery claim.
    ///
    /// Records *which* username was last published, so a profile change or a
    /// switched-off feature can retract the previous claim. Without it a
    /// renamed install leaves its old name standing in the directory
    /// indefinitely, pointing at an address that is still live — the one
    /// failure a directory must not have, since retraction is the only control
    /// a claimant holds.
    ///
    /// Protocol state rather than secure storage, on the same reasoning as the
    /// slot map: the value is a username that this install already published
    /// in a public place. Losing it costs an un-retractable stale claim, which
    /// expires only when a resolver's second hop fails.
    pub const NOSTR_DISCOVERY_CLAIM: &str = "nostr_discovery_claim";
    /// Key ID for the single discovery-claim entry.
    pub const NOSTR_DISCOVERY_CLAIM_ID: &str = "current";
    /// Key type for the per-install key that seals sensitive protocol-state
    /// records at rest.
    ///
    /// Lives in *secure* storage — it is the one piece of the protocol-state
    /// domain that must be credential-backed, because it is what gives the
    /// install-scoped container's contents their confidentiality (see
    /// [`crate::protocol::state_crypto`]).
    pub const STATE_RECORD_KEY: &str = "protocol_state_record_key";
    /// Key ID for the single protocol-state record key entry.
    pub const STATE_RECORD_KEY_ID: &str = "current";
    /// Key type for peers we are the both-create "owner" of and are awaiting a
    /// group-aware decrypt from before confirming (see
    /// [`crate::protocol::OfflineProtocol`]'s `both_create_awaiting_decrypt`).
    /// Persisted so an owner restart mid-convergence cannot let a stale plaintext
    /// probe/ack prematurely confirm and strand the peer on a divergent group.
    pub const BOTH_CREATE_AWAITING_DECRYPT: &str = "both_create_awaiting_decrypt";
    /// Key type for the marker recording that pre-split protocol state has been
    /// adopted out of secure storage (see
    /// `OfflineProtocol::adopt_legacy_protocol_state`). Lives in *protocol
    /// state* storage, so a reinstall — which drops that container — correctly
    /// re-runs the sweep against whatever the credential store still holds.
    pub const STATE_ADOPTION: &str = "protocol_state_adoption";
    /// Key ID for the single state-adoption marker entry.
    pub const STATE_ADOPTION_ID: &str = "v1";

    /// Every key type that moved from secure storage into protocol-state
    /// storage when the two domains were split.
    ///
    /// Unlike the MLS key-type set — which is open, because OpenMLS contributes
    /// its own labels through `storage_adapter.rs` — this set is closed and
    /// declared right here, which is what makes a one-shot bulk adoption
    /// possible at all (see `OfflineProtocol::adopt_legacy_protocol_state`).
    /// A new protocol-state category added *after* the split must NOT be added
    /// here: there is no pre-split data for it to inherit, and listing it would
    /// only cost a pointless enumeration of the credential store.
    ///
    /// # Removal
    ///
    /// This list, `OfflineProtocol::adopt_legacy_protocol_state`, and the
    /// `STATE_ADOPTION` marker are one-shot migration scaffolding for installs
    /// upgrading *across* the storage split. They stop doing anything once no
    /// supported install can still be running a pre-split build — an install
    /// that skips the split release entirely still needs them, so the trigger
    /// is not "one release later". Delete them only when the oldest supported
    /// upgrade path starts at or after the release that introduced the split,
    /// and delete all three together: leaving the marker behind without the
    /// sweep would make a later re-introduction silently skip itself.
    pub const ADOPTABLE_STATE_KEY_TYPES: &[&str] = &[
        BLOCKED_USERS,
        OUTBOX,
        PENDING_MESSAGES,
        SESSION_STATES,
        WELCOME_LIFECYCLES,
        PEER_KEY_PACKAGES,
        PEER_CAPABILITIES,
        MEDIA_DESCRIPTORS,
        BOTH_CREATE_AWAITING_DECRYPT,
        LAMPORT_CLOCK,
    ];
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

/// Pins that every live signing domain is mutually non-prefixing.
///
/// # Why this test exists here, of all places
///
/// Four domains are in production and they live in three languages and two
/// repositories: `offline-ctrl-v1` (this crate), `offline-disc-v1` and
/// `offline-invite-v1` (the MLS crate), and `offline-relay-addr-v1` (the relay
/// server, plus hand-mirrored copies in the iOS and Android bridges). Nothing
/// can import all four, so this module is the one place they can be compared at
/// all: it is the highest crate that can see two of them, and the fourth is
/// pinned as a literal.
#[cfg(test)]
mod signing_domain_tests {
    use offline_protocol_mls::discovery::DISCOVERY_SIGN_DOMAIN;
    use offline_protocol_mls::invite::INVITE_SIGN_DOMAIN;

    use super::CTRL_SIGN_DOMAIN;

    /// The relay's address-proof domain.
    ///
    /// A literal because it is defined in the relay-server repository and
    /// hand-mirrored in `AddressDeclarationPolicy.swift` and
    /// `AddressDeclarationPolicy.kt` — there is no Rust constant in this
    /// workspace to import. If it ever changes there, this test does not
    /// notice, which is the accepted cost of a cross-repository constant and is
    /// why it is spelled out with this comment attached.
    const RELAY_ADDR_SIGN_DOMAIN: &[u8] = b"offline-relay-addr-v1";

    /// No live domain may be a prefix of another.
    ///
    /// The canonical payload is `domain ‖ Σ(u32be(len) ‖ field)`, and the
    /// **domain itself is not length-prefixed**. So if one domain were a prefix
    /// of another, a signature made under the shorter domain could be replayed
    /// as one made under the longer: an attacker chooses a first field whose
    /// leading bytes supply the rest of the longer domain, and the two payloads
    /// become byte-identical. Length-prefixing the fields does not prevent it,
    /// because the collision happens before the first length prefix.
    ///
    /// The concrete damage this stops is recorded in the addressing work: a
    /// hostile relay that could make an address-proof signature verify as a
    /// control-frame signature would harvest a replayable control frame from
    /// every device that ever authenticated to it.
    #[test]
    fn signing_domains_are_mutually_non_prefixing() {
        let domains: [(&str, &[u8]); 4] = [
            ("offline-ctrl-v1", CTRL_SIGN_DOMAIN),
            ("offline-disc-v1", DISCOVERY_SIGN_DOMAIN),
            ("offline-invite-v1", INVITE_SIGN_DOMAIN),
            ("offline-relay-addr-v1", RELAY_ADDR_SIGN_DOMAIN),
        ];

        for (a_name, a) in domains {
            for (b_name, b) in domains {
                if a_name == b_name {
                    continue;
                }
                // Both the constant's name and its *value* are reported: a
                // failure is usually caused by an edited value, and a message
                // naming only the constant sends the reader to the wrong place.
                assert!(
                    !a.starts_with(b),
                    "signing domain {} ({:?}) is a prefix of {} ({:?}), which \
                     lets a signature made under one verify under the other",
                    b_name,
                    String::from_utf8_lossy(b),
                    a_name,
                    String::from_utf8_lossy(a)
                );
            }
        }
    }

    /// The literals are what the rest of the system, and every other
    /// implementation, actually expects. A renamed constant that still passes
    /// the non-prefix test above would silently invalidate every signature in
    /// the field, so the spellings are pinned too.
    #[test]
    fn signing_domains_have_their_published_spellings() {
        assert_eq!(CTRL_SIGN_DOMAIN, b"offline-ctrl-v1");
        assert_eq!(DISCOVERY_SIGN_DOMAIN, b"offline-disc-v1");
        assert_eq!(INVITE_SIGN_DOMAIN, b"offline-invite-v1");
        assert_eq!(RELAY_ADDR_SIGN_DOMAIN, b"offline-relay-addr-v1");
    }

    /// All four must be distinct, which non-prefixing already implies for
    /// unequal strings but not for equal ones: two identical domains are
    /// prefixes of each other, and the loop above skips same-name pairs.
    #[test]
    fn signing_domains_are_distinct() {
        let domains: [&[u8]; 4] = [
            CTRL_SIGN_DOMAIN,
            DISCOVERY_SIGN_DOMAIN,
            INVITE_SIGN_DOMAIN,
            RELAY_ADDR_SIGN_DOMAIN,
        ];
        for (i, a) in domains.iter().enumerate() {
            for b in domains.iter().skip(i + 1) {
                assert_ne!(a, b, "two signing domains are the same string");
            }
        }
    }
}

#[cfg(test)]
mod send_failure_classification_tests {
    use super::*;

    /// Every token either producer can mint classifies to itself.
    ///
    /// This is what lets the two functions compose. A value written by
    /// `send_failure_token` is later re-read from a persisted welcome
    /// lifecycle and passed through `classify_transport_send_error`; without
    /// idempotence that round trip would degrade a precise token to the
    /// generic fallback on every emit.
    #[test]
    fn every_send_failure_token_classifies_to_itself() {
        for token in SEND_FAIL_REASON_TOKENS {
            assert_eq!(
                classify_transport_send_error(token),
                *token,
                "{token} must survive a re-classification"
            );
        }
    }

    /// The vocabulary table really is the vocabulary.
    ///
    /// `classify_transport_send_error` answers from the table, so it cannot
    /// disagree with it — but `send_failure_token` matches independently, and a
    /// token added there and forgotten here would classify to the fallback on
    /// the next round trip. Walking the transport codes catches that for the
    /// arm most likely to grow.
    #[test]
    fn transport_error_tokens_are_in_the_vocabulary() {
        use offline_protocol_transport::Error as T;
        let cases = [
            T::TransportNotAvailable("ble".into()),
            T::PeerNotReachable("bob".into()),
            T::SendFailed("x".into()),
            T::ReceiveFailed("x".into()),
            T::ConfigurationError("x".into()),
            T::SerializationError("x".into()),
            T::MessageTooLarge(2, 1),
            T::CryptoError("x".into()),
            T::Other("x".into()),
        ];
        for case in cases {
            let token = send_failure_token(&crate::Error::Transport(case.clone()));
            assert!(
                SEND_FAIL_REASON_TOKENS.contains(&token),
                "{case:?} produced {token}, which is not in SEND_FAIL_REASON_TOKENS"
            );
        }
    }

    /// A relay's prose is dropped and its token kept.
    ///
    /// The prefix is a cross-layer contract the bridges hardcode and core
    /// prefix-matches to park a DM, so it must survive verbatim; everything
    /// the relay appended to it must not.
    #[test]
    fn relay_prose_is_dropped_and_the_token_kept() {
        assert_eq!(
            classify_transport_send_error(
                "recipient_unreachable: Recipient is offline and push notification could not be sent"
            ),
            SEND_FAIL_REASON_RECIPIENT_UNREACHABLE
        );
        // The relay's real vocabulary, all of it, collapses to one token.
        for prose in [
            "Recipient connection lost and push notification could not be sent",
            "Push notification already sent for this message; recipient unreachable",
            "Recipient unreachable and the message is too large to deliver by push",
            "Group delivery did not settle for this recipient in time",
        ] {
            assert_eq!(
                classify_transport_send_error(&format!("recipient_unreachable: {prose}")),
                SEND_FAIL_REASON_RECIPIENT_UNREACHABLE
            );
        }
    }

    /// Anything unrecognized fails closed to the fallback, never to itself.
    #[test]
    fn unknown_text_falls_back_and_is_never_echoed() {
        for raw in [
            "Relay rejected event: blocked",
            "off1qyvh9kjj4vy8f943a22qvxsct5s9ydew35v2dl2c is not connected",
            "",
        ] {
            let classified = classify_transport_send_error(raw);
            assert_eq!(classified, SEND_FAIL_REASON_TRANSPORT);
            assert!(
                SEND_FAIL_REASON_TOKENS.contains(&classified),
                "the fallback must itself be a token"
            );
        }
    }

    /// The literals the UniFFI layer substitutes are classified too, so the
    /// vocabulary has exactly one source rather than two that drift.
    #[test]
    fn uniffi_substituted_literals_are_classified() {
        for literal in [
            "Internet transport send failed",
            "Reticulum transport send failed",
            "Nostr transport send failed",
        ] {
            assert_eq!(
                classify_transport_send_error(literal),
                SEND_FAIL_REASON_TRANSPORT
            );
        }
        assert_eq!(
            classify_transport_send_error("Welcome send confirmation timed out"),
            SEND_FAIL_REASON_CONFIRM_TIMEOUT
        );
    }
}
