//! Group messaging — MLS-encrypted, transport-agnostic group operations.
//!
//! This module implements group creation, member invite/remove/leave,
//! encrypted message fan-out, commit distribution, and pending commit
//! buffering for out-of-order delivery.
//!
//! Groups are transport-agnostic: messages route through whichever transport
//! DORS selects (BLE, WiFi Direct, or Internet). When Internet is available
//! and a relay server supports it, the protocol can send a single relay
//! broadcast instead of O(N) per-member fan-out.

use crate::protocol::{
    base64_decode, base64_encode, internal_prefixes, GroupMemberRemovedPayload,
    InternalMessageResult, OfflineProtocol, RichPayloadV1, RichSendExtras, RICH_PAYLOAD_V1,
};
use crate::{Error, Event, Result};
use chrono::{DateTime, Utc};
use offline_protocol_core::{
    ContentType, ForwardInfo, MediaMetadata, Message, MessageId, MessagePriority,
};
use offline_protocol_mls::GroupRole;
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration as StdDuration, Instant};
use tracing::{debug, error, info, warn};

/// TTL for group message dedup entries (5 minutes).
pub(super) const GROUP_MESSAGE_DEDUP_TTL_SECS: u64 = 300;
/// Maximum number of group message dedup entries before forced cleanup.
pub(super) const MAX_GROUP_MESSAGE_DEDUP_ENTRIES: usize = 10_000;
/// Maximum allowed base64-encoded payload size for incoming group messages (1 MB).
pub(crate) const MAX_BASE64_PAYLOAD_SIZE: usize = 1_048_576;
/// How long an outstanding relay group registration waits for its
/// `__GROUP_CREATED__` answer before the correlation entry is expired. The
/// relay answers within a round trip, so this is generous slack; what it
/// bounds is the *forged-ack acceptance window* against a relay that never
/// answers (prefix-unaware echo relay, legacy server).
pub(super) const RELAY_REGISTER_ACK_TIMEOUT_SECS: i64 = 30;
/// Registration sends per connection before giving up on relay sync for a
/// group. An unanswered registration is re-sent on expiry (the frame may
/// have been lost); past this budget the group simply stays unsynced and
/// sends take the always-correct per-member fan-out path. Reset by the
/// internet 0→1 re-sync, which re-registers from scratch.
pub(super) const RELAY_REGISTER_MAX_ATTEMPTS: u32 = 3;
/// How long a relay group broadcast waits for its settled `GroupMessageSent`
/// delivery report before being re-sent. The relay emits the report only
/// after its whole fan-out settles, which is bounded by its 45 s wall-clock
/// budget (`GROUP_FANOUT_DEADLINE_SECS` relay-side) plus a possible wait for
/// a relay-wide fan-out slot — so this sits above that budget, not above a
/// round trip. An entry older than this is presumed lost with its frame or
/// connection, not merely slow.
pub(super) const RELAY_BROADCAST_REPORT_TIMEOUT_SECS: i64 = 60;
/// Broadcast sends per logical group message (initial send plus re-sends on
/// report timeout) before the sender stops trusting the broadcast path and
/// falls back to per-member fan-out for the whole roster. Re-sent broadcasts
/// are safe: the relay echoes the client-supplied `message_id`, so receiver
/// dedup and the relay's push dedup both hold across attempts.
pub(super) const RELAY_BROADCAST_MAX_ATTEMPTS: u32 = 3;
/// Upper bound on broadcasts awaiting a delivery report at once. Each entry
/// holds a full ciphertext copy and lives for up to
/// `RELAY_BROADCAST_REPORT_TIMEOUT_SECS * RELAY_BROADCAST_MAX_ATTEMPTS`, so
/// against a relay that accepts broadcasts but never reports, an app sending
/// steadily would otherwise grow the tracker without limit. At the cap the
/// oldest entry is downgraded to per-member fan-out, which needs no report
/// to be correct — the bound costs uplink, never delivery.
pub(super) const MAX_RELAY_BROADCAST_PENDING: usize = 64;
/// Relay capability token gating the group broadcast path: the v2 settled
/// delivery contract (client `message_id` accept/echo, group push fallback,
/// the settled `GroupMessageSent` delivery report, `forward_info`
/// passthrough) plus — what v3 adds — an address-aware relay group path:
/// members are named in the identifier space the roster was registered in
/// (this SDK registers the MLS roster, i.e. `off1…` addresses), fan-out and
/// push resolve those names to accounts relay-side, and group sender
/// attribution carries the sender's declared address. The relay advertises
/// it (with the granular `group_*` tokens it aggregates) in its
/// `Authenticated` answer; the bridge injects the list via
/// `set_relay_capabilities` before reporting the transport up.
///
/// A `group_delivery_v2` relay deliberately fails this gate. Against its
/// username-keyed group path, a post-addressing broadcast is worse than
/// useless: the relay cannot resolve address-registered members to
/// connections (the broadcast reaches nobody it was registered for), its
/// report names members in username space (the roster set-difference
/// re-issues to the whole roster), and any copy it does deliver arrives
/// attributed by username — failing the SEC-M1 wire-sender/credential match
/// and burning the ciphertext on a copy that is then rejected. Failing the
/// gate keeps every group send on the per-member fan-out path, which post-#27
/// relays route and attribute correctly by address.
pub(crate) const RELAY_CAP_GROUP_DELIVERY_V3: &str = "group_delivery_v3";
/// Upper bound on stored relay capability tokens, and on the byte length of
/// each token. The list is wire-supplied by the relay — bounded so a hostile
/// or broken relay cannot grow resident memory with it.
pub(super) const MAX_RELAY_CAPABILITIES: usize = 64;
/// See [`MAX_RELAY_CAPABILITIES`].
pub(super) const MAX_RELAY_CAPABILITY_TOKEN_BYTES: usize = 128;
/// Maximum number of buffered out-of-order commits per group.
pub(super) const MAX_PENDING_COMMITS_PER_GROUP: usize = 8;
/// TTL for buffered pending commits. Must be at least
/// `GROUP_MESSAGE_DEDUP_TTL_SECS`: the commit's message ID is already in the
/// dedup table, so the buffered copy is its only path to processing — a
/// shorter TTL would open a window where the buffer has given up while the
/// dedup entry still rejects every redelivery, losing the commit for good.
/// Matching the dedup TTL means that by the time the buffer expires, a
/// sender-side redelivery can be accepted fresh.
pub(super) const PENDING_COMMIT_TTL_SECS: u64 = GROUP_MESSAGE_DEDUP_TTL_SECS;
/// Global cap on buffered pending commits across all groups.
pub(super) const MAX_PENDING_COMMITS_TOTAL: usize = 64;
/// Global cap on buffered commit payload bytes across all groups (4 MiB).
pub(super) const MAX_PENDING_COMMIT_TOTAL_BYTES: usize = 4 * 1024 * 1024;
/// Global cap on distinct group IDs holding buffered pending commits.
pub(super) const MAX_PENDING_COMMIT_GROUPS: usize = 16;
/// Maximum number of buffered out-of-order group application messages per group.
pub(super) const MAX_PENDING_GROUP_MESSAGES_PER_GROUP: usize = 16;
/// TTL for buffered out-of-order group application messages. Must be at
/// least `GROUP_MESSAGE_DEDUP_TTL_SECS` — same reasoning as
/// `PENDING_COMMIT_TTL_SECS`: the buffered copy is the message's only path
/// to delivery while its dedup entry blocks redelivery.
pub(super) const PENDING_GROUP_MESSAGE_TTL_SECS: u64 = GROUP_MESSAGE_DEDUP_TTL_SECS;
/// Global cap on buffered group application messages across all groups.
pub(super) const MAX_PENDING_GROUP_MESSAGES_TOTAL: usize = 256;
/// Global cap on buffered group application ciphertext bytes (base64 length)
/// across all groups (8 MiB).
pub(super) const MAX_PENDING_GROUP_MESSAGE_TOTAL_BYTES: usize = 8 * 1024 * 1024;
/// Global cap on distinct group IDs holding buffered group application
/// messages. Bounds the width of a one-entry-per-fabricated-group spread
/// flood and the map-key memory it retains (group IDs are wire-supplied,
/// up to `GroupId::MAX_LEN` bytes each).
pub(super) const MAX_PENDING_GROUP_MESSAGE_GROUPS: usize = 32;
/// Timeout before the next eligible member re-elects itself for a leave
/// remove-commit when the original elected remover fails (30 seconds).
pub(super) const LEAVE_ELECTION_TIMEOUT_SECS: u64 = 30;
/// Delay after detecting a potential epoch fork before the leader issues a
/// resync commit (30 seconds — gives time for buffered commits to drain and
/// delayed commits to arrive, reducing false positive fork detections).
pub(super) const EPOCH_FORK_RESOLUTION_DELAY_SECS: u64 = 30;
/// Maximum number of tracked epoch fork states to prevent unbounded growth.
pub(super) const MAX_EPOCH_FORK_ENTRIES: usize = 32;
/// Maximum number of tracked pending leave elections to prevent unbounded growth.
pub(super) const MAX_PENDING_LEAVE_ELECTIONS: usize = 64;
/// Maximum lifetime for a leave election before it is abandoned (5 minutes).
/// After this, the election is cleaned up regardless of whether the member
/// was removed — prevents infinite retry loops when all candidates fail.
pub(super) const LEAVE_ELECTION_MAX_LIFETIME_SECS: u64 = 300;
/// Minimum cooldown between re-election attempts for the same leave election
/// to prevent spamming MLS operations on every process tick (5 seconds).
pub(super) const LEAVE_ELECTION_ATTEMPT_COOLDOWN_SECS: u64 = 5;
/// Minimum interval between `GroupUnauthorizedMembershipChange` reports for
/// the same (group, committer) pair (5 minutes). Divergent role metadata —
/// e.g. two members auto-promoting different admins — re-fires the judgment
/// on every commit from the "wrong" admin until Stage 2 adds reconciliation;
/// without a window that is an unbounded stream of security events and log
/// lines for one underlying condition. Roster events still carry
/// `authorized: Some(false)` on every occurrence.
pub(super) const UNAUTHORIZED_REPORT_SUPPRESS_SECS: u64 = 300;
/// Maximum number of tracked (group, committer) report timestamps to prevent
/// unbounded growth (both key halves are wire-influenced strings).
pub(super) const MAX_UNAUTHORIZED_REPORT_ENTRIES: usize = 256;

/// Correlation state for one in-flight relay group registration.
#[derive(Debug, Clone)]
pub(crate) struct RelayRegisterPending {
    /// When the outstanding `__GRP_RELAY_REG__` frame was (last) sent;
    /// entries older than [`RELAY_REGISTER_ACK_TIMEOUT_SECS`] are expired
    /// on the process tick.
    pub(crate) armed_at: DateTime<Utc>,
    /// Registration sends on this connection, counted against
    /// [`RELAY_REGISTER_MAX_ATTEMPTS`].
    pub(crate) attempts: u32,
}

/// Correlation state for one in-flight relay group broadcast awaiting its
/// settled `GroupMessageSent` delivery report.
///
/// Keyed (in `GroupMeshState::relay_broadcast_pending`) by the logical group
/// message id carried in the broadcast payload and echoed back by a v2
/// relay. Holds everything needed to (a) re-send the broadcast under the
/// same logical id when the report is lost, and (b) rebuild per-member
/// `__GRP_MLS_MSG__` copies for members the report names as missed or does
/// not name at all (the relay's roster can be a strict subset of the MLS
/// roster).
#[derive(Debug, Clone)]
pub(crate) struct RelayBroadcastPending {
    /// Group the broadcast belongs to.
    pub(crate) group_id: String,
    /// Base64-encoded MLS application ciphertext, reused verbatim for
    /// broadcast re-sends and per-member backstop copies (MLS application
    /// ciphertext is recipient-independent within a group).
    pub(crate) ciphertext_b64: String,
    /// MLS epoch at which the message was encrypted (informational).
    pub(crate) epoch: u64,
    /// Optional reply-to message id.
    pub(crate) reply_to: Option<String>,
    /// Hop-visible forward attribution; `None` when sealed into the
    /// ciphertext as rich extras.
    pub(crate) forward_info: Option<ForwardInfo>,
    /// Priority of the original send, inherited by backstop copies.
    pub(crate) priority: MessagePriority,
    /// When the outstanding broadcast frame was (last) sent; entries older
    /// than [`RELAY_BROADCAST_REPORT_TIMEOUT_SECS`] are re-sent or downgraded
    /// on the process tick.
    pub(crate) armed_at: DateTime<Utc>,
    /// Broadcast sends for this logical message, counted against
    /// [`RELAY_BROADCAST_MAX_ATTEMPTS`].
    pub(crate) attempts: u32,
}

/// Bundled state for group messaging.
///
/// Groups together the cached member lists, dedup table, pending commit
/// buffer, and relay sync tracking so that `OfflineProtocol` doesn't carry
/// these as individual fields.
#[derive(Default)]
pub(crate) struct GroupMeshState {
    /// Cached group membership lists for fan-out without holding MLS lock.
    /// Maps group_id -> list of member user IDs.
    pub(crate) members: HashMap<String, Vec<String>>,

    /// Deduplication cache for group messages received via multiple paths.
    /// Key: message ID, Value: when first seen.
    pub(crate) message_dedup: HashMap<String, Instant>,

    /// Buffer for out-of-order MLS commits that failed to decrypt.
    /// Maps group_id -> deque of pending commits awaiting retry.
    /// When a commit succeeds for a group, buffered commits are drained and retried.
    pub(crate) pending_commits: HashMap<String, VecDeque<PendingCommit>>,

    /// Buffer for group application messages that failed MLS decryption
    /// because local group state lagged the sender's (Welcome not yet
    /// processed, or epoch behind). Drained after a successful Welcome join
    /// or commit for the group. Maps group_id -> deque of pending messages.
    pub(crate) pending_group_messages: HashMap<String, VecDeque<PendingGroupMessage>>,

    /// Group IDs that have been successfully registered with the relay server.
    /// Cleared when Internet transport disconnects so that groups are re-synced
    /// when connectivity returns.
    pub(crate) relay_synced: HashSet<String>,

    /// Group IDs with a relay registration in flight (a `__GRP_RELAY_REG__`
    /// frame enqueued, no relay answer yet). The `__GROUP_CREATED__` ack only
    /// sets `relay_synced` for groups in this map: the relay forwards peer
    /// message content verbatim, so without this correlation any peer that
    /// knows a group id could forge the ack over the internet path and route
    /// our broadcasts into a relay that never registered the group. Cleared
    /// per group on ack, group-scoped `__GROUP_ERROR__`, leave/removal, and
    /// wholesale when the Internet transport drops (the answer can never
    /// arrive). Entries a relay never answers are expired on the process
    /// tick ([`RELAY_REGISTER_ACK_TIMEOUT_SECS`]) and re-registered up to
    /// [`RELAY_REGISTER_MAX_ATTEMPTS`] times — a relay that never acks
    /// (prefix-unaware echo relay, legacy server) must not leave the
    /// acceptance window armed indefinitely for a forged ack to claim.
    pub(crate) relay_register_pending: HashMap<String, RelayRegisterPending>,

    /// Capability tokens the connected relay advertised in its
    /// `Authenticated` answer, injected by the platform bridge via
    /// `set_relay_capabilities` *before* it reports the Internet transport
    /// up — the false→true flush must already see them. Empty for relays
    /// that predate the advertisement. Cleared when Internet drops: a
    /// reconnect may land on a different relay.
    pub(crate) relay_capabilities: HashSet<String>,

    /// In-flight relay group broadcasts awaiting their settled
    /// `GroupMessageSent` delivery report, keyed by logical group message
    /// id. Settled by `handle_relay_group_delivery_report` (which re-sends
    /// per-member copies to members the relay did not reach), re-sent on
    /// report timeout, and downgraded to full per-member fan-out when the
    /// attempt budget is exhausted or the Internet transport drops (the
    /// report can never arrive on a dead connection).
    pub(crate) relay_broadcast_pending: HashMap<String, RelayBroadcastPending>,

    /// Whether Internet transport was available on the last `process()` tick.
    /// Used for edge-detection: sync groups on 0→1 transition, clear on 1→0.
    pub(crate) internet_was_available: bool,

    /// Pending leave elections awaiting a remove-commit from the elected member.
    /// Key: (group_id, leaving_member_id) — uses a tuple to avoid ambiguity
    /// from string concatenation when IDs contain separator characters.
    pub(crate) pending_leave_elections: HashMap<(String, String), PendingLeaveElection>,

    /// Suspected epoch forks awaiting resolution. Key: group_id.
    pub(crate) epoch_forks: HashMap<String, EpochForkState>,

    /// When an unauthorized membership change by (group_id, committer,
    /// enforced) was last reported, for rate-limiting the security event —
    /// see [`UNAUTHORIZED_REPORT_SUPPRESS_SECS`]. Bounded by
    /// [`MAX_UNAUTHORIZED_REPORT_ENTRIES`] with oldest-entry eviction.
    ///
    /// The `enforced` flag is part of the key on purpose: an applied change
    /// and a refused one are different signals with different urgency (the
    /// refusal doubles as a partition alarm), so neither may suppress the
    /// other's first report inside one window.
    pub(crate) unauthorized_change_reports: HashMap<(String, String, bool), Instant>,
}

/// Outcome of attempting to process an MLS Commit.
enum CommitOutcome {
    /// Successfully processed; contains the group ID.
    Success(String),
    /// MLS decryption failed transiently (e.g., out-of-order delivery);
    /// the commit may succeed after a prior commit advances the epoch.
    /// Contains the group ID for buffering.
    Retriable(String),
    /// Permanently failed (parse error, bad ciphertext, MLS unavailable);
    /// should not be retried.
    Rejected,
}

/// Verdict from [`OfflineProtocol::judge_membership_change`] for one commit's
/// applied membership delta.
pub(crate) struct MembershipJudgment {
    /// Whether the authenticated committer is a known admin of the group.
    /// Gates trust in the commit-payload metadata (role, rich attestation).
    pub(crate) sender_is_admin: bool,
    /// Whether the change as a whole was authorized: an admin committed it
    /// *and* the commit's own framing agrees with the delta it produced.
    pub(crate) authorized: bool,
}

/// Outcome of attempting to decrypt a group application message.
enum GroupDecryptOutcome {
    /// Decrypted application data.
    Plaintext(Vec<u8>),
    /// MLS consumed a Commit or Proposal that arrived via the message
    /// channel (e.g., due to reordering) — not application data.
    NonApplication,
    /// Wire sender does not match the MLS-authenticated sender (SEC-M1).
    SecurityRejected,
    /// Local group state lags the sender's (no group yet, or epoch behind);
    /// the message may decrypt after a Welcome or commit lands, so it is
    /// worth buffering for deferred retry.
    Retriable,
    /// No local group state AND the payload is not MLS wire framing at
    /// all — this is not a welcome-racing ciphertext. On the mesh path
    /// that means garbage (dropped); on the relay path it means a legacy
    /// relay-only group message whose plaintext happens to be valid
    /// base64, which must be emitted raw rather than buffered and lost.
    NotMlsCiphertext,
    /// A commit riding the application channel was refused before merging by
    /// `GroupConfig::enforce_admin_commits`.
    ///
    /// This variant is why enforcement lives in the MLS layer rather than in
    /// the commit handler: a commit ciphertext wrapped in an ordinary
    /// `__GRP_MLS_MSG__` frame reaches `merge_staged_commit` through *this*
    /// path, which otherwise treats it as benign reordering
    /// ([`Self::NonApplication`]). Enforcing only on the commit-framed path
    /// would leave that reframing as a bypass.
    ///
    /// Permanent, like the commit-path rejection: the same ciphertext can
    /// never become authorized, so it must not be buffered and retried.
    PolicyRejected {
        /// The MLS-authenticated committer whose commit was refused.
        committer: String,
        /// User ids the refused commit would have added, sorted.
        added: Vec<String>,
        /// User ids the refused commit would have removed, sorted.
        removed: Vec<String>,
    },
    /// Permanently failed; not worth retrying.
    Failed,
}

// --- Group (mesh/MLS) payloads ---

/// Options for `OfflineProtocol::send_group_message_with`: priority and
/// reply threading (as on `send_group_message`), plus the rich media fields.
///
/// The rich fields (`content_type`, `media_metadata` — including any
/// cloud-media `encryption_key`/`iv` secrets) only ever travel inside the
/// MLS-sealed `__RICH_V1__` body of the group ciphertext, and only when
/// *every* other group member advertised `rich_versions` support (one
/// ciphertext serves the whole group, so one legacy or unknown-capability
/// member forces the drop). Otherwise they are silently dropped — never
/// sent cleartext — and the message degrades to what `send_group_message`
/// sends.
#[derive(Debug, Clone, Default)]
pub struct GroupSendOptions {
    /// Message priority (defaults to Medium).
    pub priority: Option<MessagePriority>,
    /// ID of the message this is replying to.
    pub reply_to_msg: Option<String>,
    /// Content-type rendering hint (text, image, video, ...), delivered
    /// sealed-only. Must not be [`ContentType::FileChunk`].
    pub content_type: Option<ContentType>,
    /// Rich media metadata (cloud attachments — including the
    /// `encryption_key`/`iv` secrets), delivered sealed-only.
    pub media_metadata: Option<MediaMetadata>,
}

/// Snapshot of whether a rich group send right now would seal its extras,
/// from `OfflineProtocol::group_rich_readiness`. Point-in-time and
/// advisory: capability knowledge changes with key-package exchanges,
/// attested adds, and restarts, and the send path re-evaluates the gate
/// itself — this exists so apps can warn before sending (e.g. gray out the
/// attachment button) instead of learning from `GroupRichExtrasDropped`
/// after the drop.
#[derive(Debug, Clone)]
pub struct GroupRichReadiness {
    /// True when every other member is known rich-capable and the local
    /// kill switch is on — rich extras would seal.
    pub ready: bool,
    /// Members not known to parse the sealed rich payload (unknown and
    /// known-non-support are indistinguishable). Empty when `ready`, and
    /// also empty when only the local kill switch blocks sealing.
    pub unknown_members: Vec<String>,
}

/// Relay-side registration state of a group, from
/// `OfflineProtocol::group_relay_sync_state`. Point-in-time: transitions
/// are surfaced as `GroupRelaySyncChanged` events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaySyncState {
    /// The relay positively acknowledged the group's registration on the
    /// current connection — relay-dependent server commands for the group
    /// (invite links, server-side fan-out) can be issued.
    Synced,
    /// A registration was sent and its acknowledgment is outstanding.
    /// The SDK re-sends on a timer and gives up after a bounded number of
    /// attempts (`GroupRelaySyncChanged { reason: "ack_timeout" }`).
    Pending,
    /// No registration is in flight — the group is unknown locally, the
    /// Internet transport is down, relay grouping is disabled, or a prior
    /// attempt was answered with an error / timed out.
    Unsynced,
}

/// Payload for MLS-encrypted group messages sent via mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMlsMessagePayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// Base64-encoded MLS ciphertext.
    pub(crate) ciphertext: String,
    /// MLS epoch at which the message was encrypted.
    pub(crate) epoch: u64,
    /// Optional reply-to message ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reply_to: Option<String>,
    /// Optional forwarding attribution (present when message was forwarded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) forward_info: Option<offline_protocol_core::ForwardInfo>,
    /// Logical id of the group message this frame carries, present when the
    /// frame re-issues a relay broadcast (the delivery-report backstop or
    /// the lost-report fallback). Lets the receiver emit the same app-facing
    /// message id every member sees for this logical message, and absorb a
    /// cross-path duplicate when the relay's own copy also arrived. Absent
    /// on the ordinary per-member fan-out path, where the envelope id is
    /// the only id. Unauthenticated at parse time — the receiver never
    /// *marks* it as seen before the ciphertext MLS-decrypts (see
    /// `handle_group_mls_msg_via`), so a non-member cannot poison the id to
    /// suppress a message it could not itself have sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) message_id: Option<String>,
}

/// Payload for MLS Welcome messages (group invites) sent via mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMlsWelcomePayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// Human-readable group name.
    pub(crate) group_name: Option<String>,
    /// Base64-encoded MLS Welcome data.
    pub(crate) welcome_data: String,
    /// Current member list (user IDs) at the time of invite.
    pub(crate) member_list: Vec<String>,
    /// Role assignments: user_id -> role.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) member_roles: HashMap<String, GroupRole>,
    /// Rich-payload versions the inviter attests per existing member
    /// (additive; absent from old SDKs), so the joiner can seal rich extras
    /// toward members it never directly exchanged key packages with.
    /// Entries only appear for members the inviter knows capable — absence
    /// means "no information", never a downgrade. Recipient-side trust
    /// bounds: entries are ignored for user ids outside the joined MLS
    /// roster, and a later direct key-package exchange with a member
    /// overrides its attested entry.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) member_rich: HashMap<String, Vec<u8>>,
    /// The group creator the inviter has on record (additive; absent from
    /// old SDKs and from inviters whose own metadata never learned it).
    ///
    /// Without this, a joiner's metadata is materialized by `set_member_role`
    /// via `GroupMetadata::new(None)`, leaving `created_by` permanently
    /// `None` — so `check_is_admin`'s creator fallback is dead code for
    /// every joiner, and a group whose role snapshot arrived incomplete
    /// resolves to deny-by-default. Propagating it makes the fallback
    /// reachable.
    ///
    /// Trust bounds mirror [`Self::member_roles`]: this is the inviter's
    /// unauthenticated claim, adopted only when the joiner has no creator on
    /// record (first write wins, see `MlsManager::set_group_creator`), and
    /// deliberately **not** bounded to the joined MLS roster — the creator of
    /// record may have already left the group. It feeds only the admin
    /// *fallback* used for send-side gating and the authorization report; it
    /// is never consulted by receive-side commit enforcement, which fails
    /// open on an unknown admin set rather than trusting a single claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) created_by: Option<String>,
}

/// Type of group membership commit operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum GroupCommitType {
    /// A member was added to the group.
    Add,
    /// A member was removed from the group.
    Remove,
    /// A key update commit (epoch advancement without membership change).
    /// Used for epoch fork resolution.
    KeyUpdate,
}

/// Payload for MLS Commit messages (membership changes) sent via mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMlsCommitPayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// Type of commit operation.
    pub(crate) commit_type: GroupCommitType,
    /// Base64-encoded MLS commit ciphertext.
    pub(crate) ciphertext: String,
    /// MLS epoch at which the commit was created.
    pub(crate) epoch: u64,
    /// User ID of the affected member (added or removed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) affected_member: Option<String>,
    /// Role assigned to the affected member (for Add commits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<GroupRole>,
    /// Rich-payload versions the inviter attests for the added member
    /// (additive; absent from old SDKs or when the inviter has no
    /// knowledge), so existing members — who never exchange key packages
    /// with the newcomer — can keep sealing rich extras. Recipient-side
    /// trust bounds mirror `role`: honored only for the member the MLS
    /// state delta actually added and only from an admin sender, and a
    /// later direct key-package exchange overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) affected_member_rich: Option<Vec<u8>>,
}

/// Payload for group leave notifications sent via mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupMlsLeavePayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// User ID of the leaving member.
    pub(crate) leaving_member: String,
}

/// Payload for group role change notifications sent via mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupRoleChangePayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// User ID of the member whose role changed.
    pub(crate) target_user_id: String,
    /// New role.
    pub(crate) new_role: GroupRole,
    /// User ID of who changed the role.
    pub(crate) changed_by: String,
}

/// Payload for group rename notifications sent via mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupRenamePayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// New human-readable group name.
    pub(crate) new_name: String,
    /// User ID of who renamed the group.
    pub(crate) renamed_by: String,
}

// --- Relay optimization payloads ---

/// Payload for registering/updating a group with the relay server.
///
/// Sent as a `__GRP_RELAY_REG__`-prefixed internal message to the user's
/// own ID via Internet transport. The relay server does NOT intercept the
/// prefix — a prefix-unaware relay just echoes the self-addressed frame
/// back. It is the platform bridge translator (the relay adapter) that
/// recognizes the frame via `internet_control_op` and replaces it with a
/// relay-native `CreateGroup` plus admin-gated member deltas, so the relay
/// learns the group → member mapping for server-side fan-out of future
/// `__GRP_RELAY_BCAST__` messages. A transport without such an adapter
/// leaves the group unsynced (per-member fan-out) — see
/// `try_relay_register_group`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RelayGroupRegistrationPayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// Human-readable group name (for display/search on relay).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) group_name: Option<String>,
    /// Current member user IDs.
    pub(crate) members: Vec<String>,
    /// Whether the registering user is a group admin, when locally
    /// determinable. The bridge translator uses this to skip the
    /// `AddGroupMember`/`RemoveGroupMember` deltas a non-admin would be
    /// denied (the relay restricts membership mutation to admins) — the
    /// denial would otherwise arrive as a group-scoped `GroupError` that
    /// revokes `relay_synced` and surfaces as an app-visible error on every
    /// reconnect. Absent means unknown: the bridge falls back to
    /// send-and-learn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) is_admin: Option<bool>,
}

/// Payload for a relay-broadcast of an MLS-encrypted group message.
///
/// Sent as a `__GRP_RELAY_BCAST__`-prefixed internal message to the user's
/// own ID via Internet transport. The relay server does NOT intercept the
/// prefix; the platform bridge translator replaces the frame with a
/// relay-native `SendGroupMessage`, and the relay's fan-out arrives at each
/// member as a `__GROUP_MSG__` frame injected by *their* bridge. This path
/// is only taken once the relay has positively acknowledged the group's
/// registration (`relay_synced`) — on any other relay the frame would be
/// echoed back and the content silently lost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RelayGroupBroadcastPayload {
    /// MLS group identifier.
    pub(crate) group_id: String,
    /// Base64-encoded MLS ciphertext.
    pub(crate) ciphertext: String,
    /// MLS epoch at which the message was encrypted.
    pub(crate) epoch: u64,
    /// Optional reply-to message ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reply_to: Option<String>,
    /// Optional forwarding attribution (present when message was forwarded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) forward_info: Option<offline_protocol_core::ForwardInfo>,
    /// Logical message id for this group message, stable across broadcast
    /// re-sends of the same logical message. The bridge translator stamps it
    /// onto the relay `SendGroupMessage` frame; a v2 relay echoes it to
    /// every member (receiver dedup), keys its push dedup by it, and names
    /// it in the settled `GroupMessageSent` delivery report (sender
    /// correlation). `default` only so old captured frames in tests still
    /// deserialize — production frames always carry it.
    #[serde(default)]
    pub(crate) message_id: String,
}

/// A relay `GroupMessageSent` settled delivery report, forwarded verbatim
/// (as the raw server frame JSON) by the platform bridge into
/// `handle_relay_group_delivery_report`. Field names follow the relay wire
/// contract; the enclosing `type` tag and `timestamp` are ignored. All
/// three member lists default to empty so a v1 relay's bare receipt (no
/// lists) still parses — it settles the entry with everyone unreached,
/// which per-member re-issue then covers.
///
/// Member-name namespace: a `group_delivery_v3` relay names members by the
/// identifiers the roster was registered under — for this SDK, the MLS
/// roster's `off1…` addresses — which is what makes the set difference in
/// `handle_relay_group_delivery_report` meaningful. A v2 relay names
/// members by relay-account *username*, a namespace that never intersects
/// the address roster; the subtraction then removes nothing and the whole
/// roster is re-issued per-member. That degradation is the fail-safe, not
/// the contract — the broadcast gate requires v3 precisely so reports in
/// the wrong namespace only ever arrive from relays this SDK no longer
/// broadcasts through (e.g. a capability set that changed mid-flight).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RelayGroupDeliveryReport {
    /// Group the report is for.
    pub(crate) group_id: String,
    /// The logical message id, echoed from the broadcast frame.
    pub(crate) message_id: String,
    /// Members whose relay socket write was confirmed (a server-side
    /// write-ack, not an application-level delivery ACK).
    #[serde(default)]
    pub(crate) delivered: Vec<String>,
    /// Members unreachable over a socket who took a device push.
    #[serde(default)]
    pub(crate) pushed: Vec<String>,
    /// Members the relay reached neither way.
    #[serde(default)]
    pub(crate) missed: Vec<RelayGroupDeliveryMiss>,
}

/// One recipient a relay group fan-out could not reach.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RelayGroupDeliveryMiss {
    /// The member's identifier as the relay knows it. Wire field name is
    /// `username` for relay-contract stability, but under `group_delivery_v3`
    /// the value is the registered member id (an `off1…` address for this
    /// SDK); only v2 relays put an account username here (see the namespace
    /// note on [`RelayGroupDeliveryReport`]).
    pub(crate) username: String,
    /// Stable machine-readable code (`offline_no_push`,
    /// `connection_lost_no_push`, `undeliverable`, `already_pushed`,
    /// `too_large_for_push`, `fanout_deadline`, …). Treated as opaque —
    /// logged, never matched on — so new relay codes cannot break parsing.
    #[serde(default)]
    pub(crate) reason: String,
}

/// A commit that arrived out-of-order and is waiting to be processed.
///
/// In mesh networks, messages can arrive out of order. If a Commit arrives
/// before a prior Commit, MLS decryption will fail. We buffer it here and
/// retry after successfully processing a later commit for the same group.
#[derive(Debug, Clone)]
pub(crate) struct PendingCommit {
    /// The original sender of this commit.
    pub(crate) sender: String,
    /// The original message ID (already recorded in the dedup table).
    /// Released when the buffered copy is dropped undelivered by cap
    /// eviction, so a redelivery is accepted fresh instead of being
    /// rejected as a replay of a copy that no longer exists anywhere.
    pub(crate) message_id: String,
    /// The raw JSON data (after prefix strip) for replay.
    pub(crate) data: String,
    /// When this pending commit was first buffered.
    pub(crate) buffered_at: Instant,
    /// Number of times this commit has been retried and still failed.
    /// A commit that expires with `retry_count > 0` is a strong signal
    /// of epoch mismatch (true fork), not just slow delivery.
    pub(crate) retry_count: u32,
}

/// A group application message that arrived before local MLS state could
/// decrypt it (Welcome not yet processed, or epoch behind the sender's).
///
/// The Welcome and the first group message are sent back-to-back and can
/// arrive out of order across transports. The ciphertext is buffered at
/// first arrival — the message-ID dedup table keeps rejecting redeliveries,
/// so this buffered copy is the message's only path to delivery.
#[derive(Debug, Clone)]
pub(crate) struct PendingGroupMessage {
    /// The original wire sender of this message.
    pub(crate) sender: String,
    /// The original message ID (already recorded in the dedup table).
    pub(crate) message_id: String,
    /// Logical group message id carried in the frame's payload, when the
    /// frame re-issued a relay broadcast (`GroupMlsMessagePayload::message_id`).
    /// On delivery the drain emits this id, so every member sees the same
    /// app-facing id whichever path reached them; the deferred delivery ACK
    /// is keyed on `message_id` regardless, since that is the envelope the
    /// sender's ack_manager awaits. A rival copy of the same logical message
    /// cannot double-deliver: whichever copy decrypts first spends the
    /// ratchet generation, and the drain drops any later copy of an id it
    /// already delivered. `None` on the ordinary fan-out and relay paths.
    pub(crate) logical_id: Option<String>,
    /// Base64-encoded MLS application ciphertext.
    pub(crate) ciphertext_b64: String,
    /// Relay-provided timestamp; `None` for mesh messages (stamped at emit).
    pub(crate) timestamp: Option<String>,
    /// Optional ID of the message this one replies to.
    pub(crate) reply_to: Option<String>,
    /// Optional forwarding attribution.
    pub(crate) forward_info: Option<offline_protocol_core::ForwardInfo>,
    /// When this message was first buffered.
    pub(crate) buffered_at: Instant,
    /// Transport the frame arrived on, recorded so the drain can send the
    /// deferred delivery ACK directly on it once the message finally decrypts
    /// (see the deferred-ACK atom in CLAUDE.md). `None` for the relay path
    /// (the relay sender is not ACK-gated) and for transport-less test enqueue
    /// — in both cases the drain ACK is a correct no-op.
    pub(crate) received_via: Option<TransportType>,
}

/// Tracks a leave election where we're waiting for the elected remover to
/// issue a remove-commit. If the timeout expires and the member is still
/// in the group, the next eligible member re-elects itself.
#[derive(Debug, Clone)]
pub(crate) struct PendingLeaveElection {
    /// Group the leave is for.
    pub(crate) group_id: String,
    /// User ID of the member who is leaving.
    pub(crate) leaving_member: String,
    /// When we received the leave notification.
    pub(crate) received_at: Instant,
    /// When we last attempted to issue a remove-commit for this election.
    /// Used as a cooldown to prevent spamming MLS operations on every
    /// process tick when the current candidate fails repeatedly.
    pub(crate) last_attempt_at: Option<Instant>,
}

/// Tracks a suspected epoch fork for a group.
///
/// An epoch fork occurs when two members issue concurrent commits at the
/// same epoch (e.g., two admins adding different members simultaneously).
/// Members who accepted commit A are on one branch, those who accepted
/// commit B are on another. Detected when buffered commits expire without
/// resolution.
#[derive(Debug, Clone)]
pub(crate) struct EpochForkState {
    /// Group with the suspected fork.
    pub(crate) group_id: String,
    /// Our local epoch when the fork was detected, or `None` if MLS was unavailable.
    pub(crate) local_epoch: Option<u64>,
    /// When the fork was first suspected.
    pub(crate) detected_at: Instant,
    /// Whether the leader has already attempted resolution.
    pub(crate) resolution_attempted: bool,
}

/// Global limits enforced by [`enforce_global_buffer_bound`], grouped so the
/// three same-typed caps cannot be transposed at a call site.
struct GlobalBufferCaps {
    /// Maximum distinct group IDs in the buffer map.
    max_groups: usize,
    /// Maximum buffered entries across all groups.
    max_entries: usize,
    /// Maximum buffered payload bytes across all groups.
    max_bytes: usize,
}

const PENDING_COMMIT_CAPS: GlobalBufferCaps = GlobalBufferCaps {
    max_groups: MAX_PENDING_COMMIT_GROUPS,
    max_entries: MAX_PENDING_COMMITS_TOTAL,
    max_bytes: MAX_PENDING_COMMIT_TOTAL_BYTES,
};

const PENDING_GROUP_MESSAGE_CAPS: GlobalBufferCaps = GlobalBufferCaps {
    max_groups: MAX_PENDING_GROUP_MESSAGE_GROUPS,
    max_entries: MAX_PENDING_GROUP_MESSAGES_TOTAL,
    max_bytes: MAX_PENDING_GROUP_MESSAGE_TOTAL_BYTES,
};

/// Enforces a global bound — entry count, payload bytes, and distinct group
/// count — across all per-group pending buffers, evicting until an incoming
/// entry of `incoming_bytes` for `incoming_group` fits. Returns the entries
/// evicted to make room so callers can release per-entry state (e.g. the
/// replay-dedup ID that would otherwise keep rejecting a redelivery of the
/// evicted message), or `None` (evicting nothing) when the incoming entry
/// alone exceeds the byte budget — the caller must drop it. Without that
/// guard a single oversized entry would purge every buffered entry across
/// all groups and then land anyway, blowing the cap it was meant to respect.
///
/// Group IDs on these buffers come straight off the wire, and the buffered
/// case (`GroupNotFound`) is precisely the pre-join, unauthenticated one —
/// per-group caps alone leave total retention open-ended across
/// attacker-chosen group IDs.
///
/// Freeing a *group slot* (admitting a new group ID at the group cap)
/// evicts the single largest per-group buffer wholesale: emptying some
/// group is the only way to free a slot, and doing it one entry at a time
/// would round-robin across level-sized buffers — purging nearly every
/// buffered entry map-wide before any single group emptied. Evicting one
/// group as a unit bounds the damage at one per-group buffer and
/// preferentially removes a concentrated flood over honest few-entry
/// groups.
///
/// Entry/byte pressure then evicts the oldest entry of the *largest*
/// remaining buffer (tie-broken by age), so a flood that concentrates in a
/// few group IDs is evicted before honest welcome-race entries, which are
/// few per group. A flood that instead spreads one entry per fabricated
/// group ID levels the buffer sizes and degrades eviction to
/// globally-oldest; the group cap bounds how wide such a spread can reach
/// (and the map-key memory it retains), but within it a sustained spread
/// flood can still push out an honest entry. Callers mitigate that by
/// releasing the evicted entry's dedup ID, turning the loss into a
/// redelivery retry instead of a permanent one — full protection would
/// need sender-level accounting, which none of these pre-join paths have.
fn enforce_global_buffer_bound<T>(
    map: &mut HashMap<String, VecDeque<T>>,
    incoming_group: &str,
    caps: &GlobalBufferCaps,
    incoming_bytes: usize,
    entry_bytes: impl Fn(&T) -> usize,
    buffered_at: impl Fn(&T) -> Instant,
) -> Option<Vec<T>> {
    if incoming_bytes > caps.max_bytes {
        return None;
    }
    let (mut entries, mut bytes) = map
        .values()
        .flat_map(|buf| buf.iter())
        .fold((0usize, 0usize), |(n, b), e| (n + 1, b + entry_bytes(e)));
    // Each deque is in arrival order, so its front is its oldest entry.
    // Rank groups by (buffer length, entry age): largest buffer first,
    // oldest front entry breaking ties.
    let pick_victim = |map: &HashMap<String, VecDeque<T>>| {
        map.iter()
            .filter_map(|(gid, buf)| {
                buf.front()
                    .map(|e| ((buf.len(), std::cmp::Reverse(buffered_at(e))), gid))
            })
            .max_by_key(|(rank, _)| *rank)
            .map(|(_, gid)| gid.clone())
    };
    let mut all_evicted = Vec::new();
    while map.len() >= caps.max_groups && !map.contains_key(incoming_group) {
        let Some(victim_group) = pick_victim(map) else {
            break;
        };
        warn!(
            group_id = %victim_group,
            "Pending buffers at group capacity, evicting largest group buffer wholesale"
        );
        if let Some(buf) = map.remove(&victim_group) {
            entries -= buf.len();
            bytes -= buf.iter().map(&entry_bytes).sum::<usize>();
            all_evicted.extend(buf);
        }
    }
    while entries >= caps.max_entries || bytes + incoming_bytes > caps.max_bytes {
        let Some(victim_group) = pick_victim(map) else {
            break;
        };
        warn!(
            group_id = %victim_group,
            "Pending buffer at global capacity, evicting oldest entry of largest group buffer"
        );
        if let Some(buf) = map.get_mut(&victim_group) {
            if let Some(evicted) = buf.pop_front() {
                entries -= 1;
                bytes -= entry_bytes(&evicted);
                all_evicted.push(evicted);
            }
            if buf.is_empty() {
                map.remove(&victim_group);
            }
        }
    }
    Some(all_evicted)
}

impl OfflineProtocol {
    /// Transport-less convenience wrapper over [`Self::handle_group_mls_msg_via`],
    /// used only by tests; production always knows the arrival transport.
    #[cfg(test)]
    pub(crate) fn handle_group_mls_msg(
        &mut self,
        message: &Message,
        sender: &str,
        data: &str,
    ) -> InternalMessageResult {
        self.handle_group_mls_msg_via(message, sender, data, None)
    }

    /// Handles an incoming MLS-encrypted group message, with the transport the
    /// frame arrived on.
    ///
    /// Returns [`InternalMessageResult::SecurityRejected`] when the wire
    /// sender does not match the MLS-authenticated sender (SEC-M1), so the
    /// caller suppresses the delivery ACK exactly like the `__MLS_ENC__`
    /// path; a genuine crypto/parse failure consumes the message.
    ///
    /// Deferred-ACK atom (mesh-group analog of the DM/media fix, PR #223): a
    /// message buffered because local group state is not ready yet returns
    /// [`InternalMessageResult::Deferred`], so the receive loop skips the
    /// delivery ACK and unmarks the transport-level dedup — keeping the
    /// sender's per-member `ack_manager` retransmitting until the message is
    /// actually delivered on drain (`drain_pending_group_messages` sends the
    /// deferred ACK on `received_via`). Without this the buffered-but-ACKed
    /// copy could be evicted/expired before a commit drained it and be lost,
    /// even though the group-level `release_replay_protection` already clears
    /// dedup on drop. The group-level `message_dedup` stays marked across the
    /// pending lifetime (replay-amplification defense + authoritative
    /// double-delivery guard), so — unlike the DM path — the drain does not
    /// re-mark the transport dedup.
    pub(crate) fn handle_group_mls_msg_via(
        &mut self,
        message: &Message,
        sender: &str,
        data: &str,
        arrival_transport: Option<TransportType>,
    ) -> InternalMessageResult {
        let payload = match serde_json::from_str::<GroupMlsMessagePayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupMlsMessage payload");
                return InternalMessageResult::Consumed;
            }
        };

        // Dedup check using the unique message ID. When the payload carries a
        // logical id (a re-issued relay broadcast), the same logical message
        // may already have arrived via the relay path under that id — check
        // both keys so a cross-path duplicate is absorbed here instead of
        // burning an MLS decrypt on a ciphertext whose ratchet generation was
        // already consumed (which would classify Retriable and buffer noise).
        let dedup_key = message.id.as_str().to_string();
        let logical_key = payload.message_id.clone();
        let seen = self.group_mesh.message_dedup.contains_key(&dedup_key)
            || logical_key
                .as_deref()
                .is_some_and(|l| self.group_mesh.message_dedup.contains_key(l));
        if seen {
            // Already seen. If the buffered copy is still awaiting decryption,
            // this is a sender retransmit of an undelivered message: defer (no
            // ACK) so the sender keeps retrying until we actually deliver on
            // drain. Returning before decrypt also preserves the
            // replay-amplification defense (one MLS crypto op per id). If the
            // id is not buffered, it was already delivered (a dropped-and-
            // released id would not be in the dedup table at all), so treat it
            // as a normal duplicate and re-ACK to let the sender stop.
            let pending = self.is_group_message_pending(&payload.group_id, &dedup_key)
                || logical_key
                    .as_deref()
                    .is_some_and(|l| self.is_group_message_pending(&payload.group_id, l));
            if pending {
                debug!(
                    group_id = %payload.group_id,
                    msg_id = %dedup_key,
                    "Duplicate of a still-pending group message, deferring ACK"
                );
                return InternalMessageResult::Deferred;
            }
            debug!(
                group_id = %payload.group_id,
                msg_id = %dedup_key,
                "Duplicate group message, skipping"
            );
            return InternalMessageResult::Consumed;
        }

        // Mark as seen BEFORE attempting decode/decrypt to prevent replay
        // amplification: an adversary replaying the same message (even with a bad
        // epoch) should only trigger one MLS crypto operation. Only the envelope
        // id is marked here — the payload's logical id is unauthenticated wire
        // input until the ciphertext MLS-decrypts, and marking it pre-decrypt
        // would let a non-member poison a logical id to suppress the genuine
        // copy. It is marked below, on successful decrypt only.
        self.group_mesh
            .message_dedup
            .insert(dedup_key, Instant::now());
        if self.group_mesh.message_dedup.len() > MAX_GROUP_MESSAGE_DEDUP_ENTRIES {
            self.cleanup_group_message_dedup();
        }

        // Size guard before base64 decode
        let ciphertext_bytes = match base64_decode(&payload.ciphertext) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Failed to decode group message ciphertext");
                return InternalMessageResult::Consumed;
            }
        };

        match self.decrypt_group_application(&payload.group_id, ciphertext_bytes, sender) {
            GroupDecryptOutcome::Plaintext(plaintext) => {
                let Ok(text) = String::from_utf8(plaintext) else {
                    warn!(
                        group_id = %payload.group_id,
                        "Decrypted group payload is not valid UTF-8, dropping"
                    );
                    return InternalMessageResult::Consumed;
                };
                // Decryption authenticated the sender as a group member, so
                // the payload's logical id (if any) can now be trusted: mark
                // it delivered so the relay path's copy of the same logical
                // message dedups against this one, and emit it as the
                // app-facing id so every member sees the same id for this
                // logical message regardless of arrival path.
                let msg_id = logical_key.unwrap_or_else(|| message.id.as_str().to_string());
                if msg_id != message.id.as_str() {
                    self.group_mesh
                        .message_dedup
                        .insert(msg_id.clone(), Instant::now());
                }
                let timestamp = chrono::Utc::now().to_rfc3339();
                info!(group_id = %payload.group_id, "Decrypted mesh group message");
                let (content, media_metadata, content_type, forward_info_event) =
                    Self::restore_group_rich(text, payload.forward_info, sender);
                self.emit_event(Event::group_message_received(
                    payload.group_id,
                    sender.to_string(),
                    content,
                    timestamp,
                    msg_id,
                    payload.reply_to,
                    forward_info_event,
                    media_metadata,
                    content_type,
                ));
                InternalMessageResult::Consumed
            }
            GroupDecryptOutcome::SecurityRejected => InternalMessageResult::SecurityRejected,
            GroupDecryptOutcome::Retriable => {
                let group_id = payload.group_id.clone();
                self.buffer_pending_group_message(
                    &group_id,
                    PendingGroupMessage {
                        sender: sender.to_string(),
                        message_id: message.id.as_str().to_string(),
                        logical_id: logical_key,
                        ciphertext_b64: payload.ciphertext,
                        timestamp: None,
                        reply_to: payload.reply_to,
                        forward_info: payload.forward_info,
                        buffered_at: Instant::now(),
                        received_via: arrival_transport,
                    },
                );
                // Buffered, not delivered: defer the ACK so the sender keeps
                // retransmitting until the drain surfaces it. The drain then
                // ACKs directly on `arrival_transport`.
                InternalMessageResult::Deferred
            }
            GroupDecryptOutcome::NonApplication => {
                // MLS consumed a commit or proposal riding the application
                // channel — group state may have advanced, which can unblock
                // buffered commits and messages exactly like a commit-channel
                // success.
                self.drain_pending_commits(&payload.group_id);
                self.drain_pending_group_messages(&payload.group_id);
                InternalMessageResult::Consumed
            }
            GroupDecryptOutcome::PolicyRejected {
                committer,
                added,
                removed,
            } => {
                // A commit reframed onto the application channel, refused by
                // enforcement before merging. Consume it: the rejection is
                // permanent, so buffering would retry a ciphertext that can
                // never be authorized.
                self.report_unauthorized_membership_change(
                    &payload.group_id,
                    &committer,
                    &added,
                    &removed,
                    "sender_not_admin",
                    true,
                );
                InternalMessageResult::Consumed
            }
            // The mesh channel carries MLS ciphertext by protocol, so a
            // payload that is not MLS framing is garbage, not legacy content.
            GroupDecryptOutcome::NotMlsCiphertext | GroupDecryptOutcome::Failed => {
                InternalMessageResult::Consumed
            }
        }
    }

    /// Decrypts a group application ciphertext, classifying the failure
    /// modes shared by the mesh (`handle_group_mls_msg`), relay
    /// (`handle_relay_group_message_with_mls`), and deferred-retry
    /// (`drain_pending_group_messages`) inbound paths.
    fn decrypt_group_application(
        &self,
        group_id: &str,
        ciphertext: Vec<u8>,
        sender: &str,
    ) -> GroupDecryptOutcome {
        let mls_guard = match self.read_mls_guard() {
            Ok(guard) => guard,
            Err(e) => {
                warn!(group_id = %group_id, error = %e, "MLS unavailable, dropping group message");
                return GroupDecryptOutcome::Failed;
            }
        };
        let gid = match offline_protocol_mls::GroupId::new(group_id) {
            Ok(gid) => gid,
            Err(e) => {
                warn!(group_id = %group_id, error = %e, "Dropping group message with invalid group id");
                return GroupDecryptOutcome::Failed;
            }
        };
        // The epoch field is not used for decryption — OpenMLS determines
        // the epoch from the ciphertext header itself.
        let encrypted = offline_protocol_mls::EncryptedMessage {
            group_id: gid,
            message_type: offline_protocol_mls::MlsMessageType::Application,
            epoch: 0,
            ciphertext,
            sender_id: sender.to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        };
        match mls_guard.decrypt_from_group(&encrypted, sender) {
            Ok(Some(plaintext)) => GroupDecryptOutcome::Plaintext(plaintext),
            Ok(None) => {
                // Ok(None) means MLS consumed a Commit or Proposal — not application
                // data. This is normal for non-application messages that arrive via the
                // group message channel (e.g., due to message reordering).
                debug!(
                    group_id = %group_id,
                    "MLS returned no plaintext (commit/proposal consumed), not an application message"
                );
                GroupDecryptOutcome::NonApplication
            }
            Err(offline_protocol_mls::MlsError::SenderIdentityMismatch {
                claimed,
                authenticated,
            }) => {
                error!(
                    group_id = %group_id,
                    claimed = %claimed,
                    authenticated = %authenticated,
                    "SECURITY: wire sender does not match MLS-authenticated sender, rejecting group message"
                );
                GroupDecryptOutcome::SecurityRejected
            }
            Err(offline_protocol_mls::MlsError::CommitNotAuthorized {
                committer,
                added,
                removed,
            }) => GroupDecryptOutcome::PolicyRejected {
                committer,
                added,
                removed,
            },
            Err(e @ offline_protocol_mls::MlsError::GroupNotFound(_)) => {
                // No local group state yet. Only buffer what is actually MLS
                // wire framing — a racing ciphertext parses, legacy relay
                // plaintext that happens to be valid base64 does not.
                // (`Decryption` failures below skip this check: decryption
                // only runs after framing already parsed.)
                if offline_protocol_mls::is_mls_framed(&encrypted.ciphertext) {
                    warn!(
                        group_id = %group_id,
                        error = %e,
                        "Failed to decrypt group message (may be out-of-order, buffering)"
                    );
                    GroupDecryptOutcome::Retriable
                } else {
                    debug!(
                        group_id = %group_id,
                        "Group message payload is not MLS framing and no local group state exists"
                    );
                    GroupDecryptOutcome::NotMlsCiphertext
                }
            }
            Err(
                e @ (offline_protocol_mls::MlsError::Decryption(_)
                | offline_protocol_mls::MlsError::SessionDesync(_)),
            ) => {
                // Epoch-lagged ciphertext — may decrypt once a commit
                // advances local group state. `SessionDesync` is the same
                // epoch-mismatch condition surfaced with a distinct variant for
                // the 1:1 re-key path; on the group path it buffers exactly as
                // `Decryption` always has (the group's own recovery is the
                // commit-driven drain, not a re-key).
                warn!(
                    group_id = %group_id,
                    error = %e,
                    "Failed to decrypt group message (may be out-of-order, buffering)"
                );
                GroupDecryptOutcome::Retriable
            }
            Err(e) => {
                warn!(group_id = %group_id, error = %e, "Failed to decrypt group message");
                GroupDecryptOutcome::Failed
            }
        }
    }

    /// Handles an incoming MLS Welcome message (group invite).
    pub(crate) fn handle_group_mls_welcome(&mut self, message_id: &str, sender: &str, data: &str) {
        let payload = match serde_json::from_str::<GroupMlsWelcomePayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupMlsWelcome payload");
                return;
            }
        };

        info!(group_id = %payload.group_id, "Received mesh group Welcome");

        // Dedup check — prevents double-join when the same Welcome arrives via
        // multiple transports nearly simultaneously (TOCTOU race on members cache).
        let dedup_key = message_id.to_string();
        if self.group_mesh.message_dedup.contains_key(&dedup_key) {
            debug!(group_id = %payload.group_id, msg_id = %dedup_key, "Duplicate Welcome message, skipping");
            return;
        }
        self.group_mesh
            .message_dedup
            .insert(dedup_key, Instant::now());
        if self.group_mesh.message_dedup.len() > MAX_GROUP_MESSAGE_DEDUP_ENTRIES {
            self.cleanup_group_message_dedup();
        }

        // Skip duplicate Welcome — we're already a member
        if self.group_mesh.members.contains_key(&payload.group_id) {
            debug!(group_id = %payload.group_id, "Ignoring duplicate Welcome — already a member of this group");
            return;
        }

        let welcome_bytes = match base64_decode(&payload.welcome_data) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Failed to decode welcome data");
                return;
            }
        };

        // Join group via MLS, then update cache
        let mls_guard = match self.read_mls_guard() {
            Ok(guard) => guard,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "MLS unavailable, dropping group welcome");
                return;
            }
        };
        let gid = match offline_protocol_mls::GroupId::new(&payload.group_id) {
            Ok(gid) => gid,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Dropping group welcome with invalid group id");
                return;
            }
        };
        let welcome = offline_protocol_mls::WelcomeMessage {
            group_id: gid.clone(),
            welcome_data: welcome_bytes,
            inviter_id: sender.to_string(),
            group_name: payload.group_name.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        };
        let join_result = match mls_guard.join_group(&welcome) {
            Ok(group_info) => Some(group_info.members.clone()),
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Failed to join mesh group");
                None
            }
        };
        drop(mls_guard);

        if let Some(members) = join_result {
            let group_id = payload.group_id.clone();

            // Record the inviter's rich-capability attestations for members
            // we have never (and may never) directly exchange key packages
            // with — without them every rich send into this group drops its
            // extras. Bounded to the authoritative MLS roster we just
            // joined: entries for non-members are attacker-suppliable dead
            // weight and are ignored. `record_attested_rich` skips self,
            // non-V1 lists, and peers we already know directly.
            for (user_id, versions) in &payload.member_rich {
                if members.contains(user_id) {
                    self.record_attested_rich(user_id, versions);
                }
            }

            self.group_mesh.members.insert(group_id.clone(), members);

            // Store member roles and the creator of record from the welcome
            // payload. The creator is what makes `check_is_admin`'s fallback
            // reachable for a joiner: without it our metadata is materialized
            // by `set_member_role` via `GroupMetadata::new(None)`, so a group
            // whose role snapshot arrived incomplete would resolve to
            // deny-by-default with no way back. Deliberately not bounded to
            // the roster we just joined — the creator of record may already
            // have left. `set_group_creator` is first-write-wins, so a
            // duplicate or later Welcome cannot rewrite it.
            if !payload.member_roles.is_empty() || payload.created_by.is_some() {
                if let Ok(mls_guard) = self.read_mls_guard() {
                    for (user_id, role) in &payload.member_roles {
                        if let Err(e) = mls_guard.set_member_role(&gid, user_id, *role) {
                            warn!(user_id = %user_id, error = %e, "Failed to store member role from welcome");
                        }
                    }
                    if let Some(creator) = &payload.created_by {
                        if let Err(e) = mls_guard.set_group_creator(&gid, creator) {
                            warn!(creator = %creator, error = %e, "Failed to store group creator from welcome");
                        }
                    }
                }
            }

            // Send a fresh key package to the inviter. The inviter consumed our
            // previous key package to create this Welcome, so they need a new one
            // for future group invites. Clear the tracking flag first so the send
            // isn't suppressed as a duplicate.
            self.key_package_sent_to.remove(sender);
            if let Err(e) = self.send_key_package_to(sender, false) {
                debug!(
                    inviter = %sender,
                    error = %e,
                    "Failed to send fresh key package to inviter after Welcome (will retry on discovery)"
                );
            }

            self.emit_event(Event::group_member_added(
                group_id.clone(),
                self.local_id.clone(),
                sender.to_string(),
                payload.group_name.clone(),
                // Not evaluated: this Welcome is what creates our view of the
                // group, so there is no prior role state to judge the inviter
                // against. `Some(true)` here would claim a check that cannot
                // exist on this path.
                None,
            ));

            // A commit or group message can race ahead of its Welcome (they
            // are sent back-to-back and may take different paths); now that
            // the group exists locally, retry anything that was buffered.
            // Commits first — an epoch-advancing commit may be exactly what
            // a buffered message is waiting for.
            self.drain_pending_commits(&group_id);
            self.drain_pending_group_messages(&group_id);
        }
    }

    /// Handles an incoming MLS Commit message (membership change).
    ///
    /// Validates the `affected_member` claim against actual MLS state delta
    /// to prevent forged membership events.
    ///
    /// If decryption fails (e.g., due to out-of-order delivery in a mesh network),
    /// the commit is buffered for deferred retry. When a subsequent commit for the
    /// same group succeeds, buffered commits are drained and retried.
    pub(crate) fn handle_group_mls_commit(&mut self, message_id: &str, sender: &str, data: &str) {
        // Dedup check — prevents processing the same commit twice when it
        // arrives via multiple transports. Without this, the second copy fails
        // at MLS (wrong epoch) and gets buffered as a "retriable" pending
        // commit, wasting space and potentially triggering false fork detection.
        let dedup_key = message_id.to_string();
        if self.group_mesh.message_dedup.contains_key(&dedup_key) {
            debug!(msg_id = %dedup_key, "Duplicate commit message, skipping");
            return;
        }
        self.group_mesh
            .message_dedup
            .insert(dedup_key, Instant::now());
        if self.group_mesh.message_dedup.len() > MAX_GROUP_MESSAGE_DEDUP_ENTRIES {
            self.cleanup_group_message_dedup();
        }

        match self.process_commit_core(sender, data) {
            CommitOutcome::Success(group_id) => {
                self.drain_pending_commits(&group_id);
                // A commit that advanced the epoch may unblock buffered
                // application messages encrypted at the newer epoch.
                self.drain_pending_group_messages(&group_id);
            }
            CommitOutcome::Retriable(group_id) => {
                self.buffer_pending_commit(&group_id, message_id, sender, data);
            }
            CommitOutcome::Rejected => {}
        }
    }

    /// Core commit processing logic. Returns an outcome indicating success,
    /// retriable failure, or permanent rejection.
    ///
    /// Does **not** buffer on failure — callers decide whether to buffer based
    /// on the returned outcome.
    fn process_commit_core(&mut self, sender: &str, data: &str) -> CommitOutcome {
        let payload = match serde_json::from_str::<GroupMlsCommitPayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupMlsCommit payload");
                return CommitOutcome::Rejected;
            }
        };

        info!(
            group_id = %payload.group_id,
            commit_type = ?payload.commit_type,
            "Received mesh group Commit"
        );

        if payload.ciphertext.is_empty() {
            warn!(
                group_id = %payload.group_id,
                "Received Commit with empty ciphertext, cannot advance MLS epoch"
            );
            return CommitOutcome::Rejected;
        }

        let ciphertext_bytes = match base64_decode(&payload.ciphertext) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Failed to decode commit ciphertext");
                return CommitOutcome::Rejected;
            }
        };

        let mls_guard = match self.read_mls_guard() {
            Ok(guard) => guard,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "MLS unavailable, dropping group commit");
                return CommitOutcome::Rejected;
            }
        };

        let gid = match offline_protocol_mls::GroupId::new(&payload.group_id) {
            Ok(gid) => gid,
            Err(e) => {
                warn!(group_id = %payload.group_id, error = %e, "Rejecting group commit with invalid group id");
                return CommitOutcome::Rejected;
            }
        };

        // Capture members before commit for delta validation. `None` marks a
        // failed read: this and the post-commit refresh below are the two
        // inputs to the membership delta, and a transient storage failure
        // (`MlsStorage` is a platform keychain/keystore) silently defaulting
        // to an empty roster would fabricate a full-roster delta — and, with
        // it, a security report naming an innocent committer as having added
        // every member. `Ok(None)` (no local group yet) stays an empty
        // roster: past a successful decrypt it is unreachable anyway.
        //
        // Deliberately MLS-derived, not the `group_mesh.members` cache: relay
        // reconciliation can splice ids into the cache that MLS never
        // admitted, and a cache-derived "before" would report such a phantom
        // as an unauthorized removal on the next real commit.
        let members_before: Option<HashSet<String>> = match mls_guard.get_group_info(&gid) {
            Ok(info) => Some(
                info.map(|i| i.members.into_iter().collect())
                    .unwrap_or_default(),
            ),
            Err(e) => {
                warn!(
                    group_id = %payload.group_id,
                    error = %e,
                    "Failed to read pre-commit roster — this commit's membership delta will not be derived or judged"
                );
                None
            }
        };

        // Process Commit via MLS to advance epoch (single lock acquisition)
        let encrypted = offline_protocol_mls::EncryptedMessage {
            group_id: gid.clone(),
            message_type: offline_protocol_mls::MlsMessageType::Commit,
            epoch: payload.epoch,
            ciphertext: ciphertext_bytes,
            sender_id: sender.to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        };
        let mls_result = mls_guard.decrypt_from_group(&encrypted, sender);
        drop(mls_guard);

        match mls_result {
            Ok(_) => {}
            Err(ref e) => {
                // If this is a Remove commit targeting us, we can't decrypt it
                // because the admin already removed us from the MLS group. But the
                // commit payload metadata tells us we were removed, so handle it
                // gracefully: emit the event, clean up local state, and reject the
                // commit (don't buffer it — it will never succeed).
                //
                // SECURITY: Verify the sender is an admin before trusting the
                // unencrypted commit metadata. Without this check, a non-admin
                // group member could forge a commit with garbage ciphertext and
                // affected_member = victim to force group eviction.
                let self_id = self.local_id.clone();
                if matches!(payload.commit_type, GroupCommitType::Remove)
                    && payload.affected_member.as_deref() == Some(&self_id)
                    && self
                        .check_is_admin(&payload.group_id, sender)
                        .unwrap_or(false)
                {
                    info!(
                        group_id = %payload.group_id,
                        "Received remove-commit targeting us — cleaning up local state"
                    );
                    // Clean up local MLS state
                    if let Ok(mls_guard) = self.read_mls_guard() {
                        let _ = mls_guard.leave_group(&gid);
                    }
                    self.group_mesh.members.remove(&payload.group_id);
                    let was_synced = self.group_mesh.relay_synced.remove(&payload.group_id);
                    let was_pending = self
                        .group_mesh
                        .relay_register_pending
                        .remove(&payload.group_id)
                        .is_some();
                    if was_synced || was_pending {
                        self.emit_event(Event::group_relay_sync_changed(
                            payload.group_id.clone(),
                            false,
                            "removed",
                        ));
                    }
                    self.group_mesh
                        .pending_group_messages
                        .remove(&payload.group_id);
                    // Also drop buffered commits: they can never apply now,
                    // and a stale one retried after a rapid re-invite would
                    // expire with retry_count > 0 and falsely flag an epoch
                    // fork.
                    self.group_mesh.pending_commits.remove(&payload.group_id);
                    self.emit_event(Event::group_member_removed(
                        payload.group_id.clone(),
                        self_id,
                        sender.to_string(),
                        // Reached only through the `check_is_admin` gate above.
                        Some(true),
                    ));
                    return CommitOutcome::Rejected;
                }

                // Enforcement refused this commit before merging (only
                // possible with `GroupConfig::enforce_admin_commits` on).
                // Report it on the same signal the applied case uses, marked
                // `enforced` so the app can tell "this happened, undo it"
                // from "this was blocked and we are now behind the group's
                // epoch". Nothing merged, so no roster event accompanies it.
                if let offline_protocol_mls::MlsError::CommitNotAuthorized {
                    committer,
                    added,
                    removed,
                } = e
                {
                    let (committer, added, removed) =
                        (committer.clone(), added.clone(), removed.clone());
                    self.report_unauthorized_membership_change(
                        &payload.group_id,
                        &committer,
                        &added,
                        &removed,
                        "sender_not_admin",
                        true,
                    );
                    return CommitOutcome::Rejected;
                }

                // Classify MLS errors: permanent failures should not be retried,
                // only failures caused by lagging local group state are worth
                // buffering. GroupNotFound is NOT permanent — a commit can
                // outrun our Welcome (they may take different transports), so
                // it is buffered and retried after the join lands, exactly
                // like application messages. A spoofed sender (SEC-M1) is
                // permanent — retrying the same forged commit can never
                // succeed.
                let is_permanent = matches!(
                    e,
                    offline_protocol_mls::MlsError::Deserialization(_)
                        | offline_protocol_mls::MlsError::InvalidMessage(_)
                        | offline_protocol_mls::MlsError::Storage(_)
                        | offline_protocol_mls::MlsError::SenderIdentityMismatch { .. }
                );
                if is_permanent {
                    error!(
                        group_id = %payload.group_id,
                        epoch = payload.epoch,
                        error = %e,
                        "Permanently failed to process group commit (not retriable)"
                    );
                    return CommitOutcome::Rejected;
                }
                error!(
                    group_id = %payload.group_id,
                    epoch = payload.epoch,
                    error = %e,
                    "Failed to process group commit (may be out-of-order, buffering)"
                );
                return CommitOutcome::Retriable(payload.group_id);
            }
        }

        // Refresh cache and compute the actual membership delta. Like the
        // pre-commit read above, a failed refresh must not default to an
        // empty roster — so when either read failed, the delta is unknowable
        // and everything derived from it (roster events, the authorization
        // judgment, leave-election cleanup, auto-promotion) is skipped for
        // this commit rather than fabricated. The commit itself has already
        // merged; the member cache and events self-heal on the next
        // successful commit or refresh.
        let members_after: Option<HashSet<String>> = match self
            .refresh_group_members(&payload.group_id)
        {
            Ok(members) => Some(members.into_iter().collect()),
            Err(e) => {
                warn!(
                    group_id = %payload.group_id,
                    error = %e,
                    "Failed to refresh post-commit roster — this commit's membership delta will not be derived or judged"
                );
                None
            }
        };
        let (Some(members_before), Some(members_after)) = (members_before, members_after) else {
            if matches!(payload.commit_type, GroupCommitType::KeyUpdate) {
                self.group_mesh.epoch_forks.remove(&payload.group_id);
            }
            return CommitOutcome::Success(payload.group_id);
        };

        // Sorted so event payloads and log lines are deterministic.
        let mut actual_added: Vec<String> =
            members_after.difference(&members_before).cloned().collect();
        actual_added.sort();
        let mut actual_removed: Vec<String> =
            members_before.difference(&members_after).cloned().collect();
        actual_removed.sort();

        let has_membership_change = !actual_added.is_empty() || !actual_removed.is_empty();

        // Validate claimed affected_member against actual MLS delta. A
        // mismatch means the commit's unencrypted framing disagrees with the
        // authenticated MLS state change it actually produced — forged or
        // replayed metadata.
        let mut claim_matches_delta = true;
        if let Some(claimed) = &payload.affected_member {
            let valid = match payload.commit_type {
                GroupCommitType::Add => actual_added.contains(claimed),
                GroupCommitType::Remove => actual_removed.contains(claimed),
                GroupCommitType::KeyUpdate => true, // No membership change expected
            };
            if !valid && has_membership_change {
                claim_matches_delta = false;
                error!(
                    group_id = %payload.group_id,
                    sender = %sender,
                    claimed = %claimed,
                    actual_added = ?actual_added,
                    actual_removed = ?actual_removed,
                    "SECURITY: Commit affected_member does not match actual MLS state delta — possible forgery"
                );
            }
        }

        // No delta, no judgment: a pure KeyUpdate changes no membership and
        // needs no admin (fork-resolution KeyUpdates are issued by the
        // deterministic leader, who is often not one).
        let judgment = if has_membership_change {
            Some(self.judge_membership_change(
                &payload.group_id,
                sender,
                &actual_added,
                &actual_removed,
                claim_matches_delta,
            ))
        } else {
            None
        };

        // Commit-payload metadata (role, rich attestation) is honored only
        // for the member the MLS delta actually added and only from an
        // admin sender — adds are admin-only, so a non-admin sender means
        // forged metadata on a replayed/crafted frame.
        let metadata_sender_is_admin =
            judgment.as_ref().is_some_and(|j| j.sender_is_admin) && !actual_added.is_empty();
        // What the roster events below carry. `None` never reaches them: an
        // empty delta emits no events.
        let authorized = judgment.map(|j| j.authorized);

        // Emit events based on actual MLS membership changes, not claimed affected_member
        for member in &actual_added {
            // Store the role from the commit payload only for the specific affected member,
            // and only if the sender is an admin (prevents non-admins from injecting elevated roles).
            if let (Some(role), Some(affected)) = (&payload.role, &payload.affected_member) {
                if member == affected {
                    if metadata_sender_is_admin {
                        if let Ok(mls_guard) = self.read_mls_guard() {
                            if let Err(e) = mls_guard.set_member_role(&gid, member, *role) {
                                warn!(user_id = %member, error = %e, "Failed to store member role from commit");
                            }
                        }
                    } else {
                        warn!(
                            sender = %sender,
                            group_id = %payload.group_id,
                            "Ignoring role from commit: sender is not admin"
                        );
                    }
                }
            }
            // Record the inviter's rich-capability attestation for the
            // newcomer — we existing members never exchange key packages
            // with them, and without this one unknown member drops rich
            // extras for the whole group. Same trust bounds as the role
            // above; a later direct exchange overrides it.
            if let (Some(versions), Some(affected)) =
                (&payload.affected_member_rich, &payload.affected_member)
            {
                if member == affected && metadata_sender_is_admin {
                    self.record_attested_rich(member, versions);
                }
            }
            self.emit_event(Event::group_member_added(
                payload.group_id.clone(),
                member.clone(),
                sender.to_string(),
                None,
                authorized,
            ));
        }
        for member in &actual_removed {
            // Clean up role metadata for removed members
            if let Ok(mls_guard) = self.read_mls_guard() {
                if let Err(e) = mls_guard.remove_member_role(&gid, member) {
                    warn!(user_id = %member, error = %e, "Failed to clean up role metadata for removed member");
                }
            }
            self.emit_event(Event::group_member_removed(
                payload.group_id.clone(),
                member.clone(),
                sender.to_string(),
                authorized,
            ));
            // Clear any pending leave election for this member — the remove
            // has been committed successfully.
            self.group_mesh
                .pending_leave_elections
                .remove(&(payload.group_id.clone(), member.to_string()));
        }

        // Auto-promote if removal left zero admins and other members remain
        if !actual_removed.is_empty() {
            let remaining = self
                .group_mesh
                .members
                .get(&payload.group_id)
                .cloned()
                .unwrap_or_default();
            self.auto_promote_if_no_admin(&payload.group_id, &remaining);
        }

        // Only clear fork tracking when a KeyUpdate commit succeeds — these
        // are the resolution commits issued by the leader. Regular Add/Remove
        // commits from the same branch succeeding doesn't prove the fork is
        // resolved; members on the other branch are still diverged.
        if matches!(payload.commit_type, GroupCommitType::KeyUpdate) {
            self.group_mesh.epoch_forks.remove(&payload.group_id);
        }

        CommitOutcome::Success(payload.group_id)
    }

    /// Judges one **applied** membership delta against the local admin
    /// overlay, reporting an unauthorized change (rate-limited) via
    /// `GroupUnauthorizedMembershipChange`.
    ///
    /// This deliberately does **not** gate the membership change. MLS
    /// authorizes membership by MLS membership — RFC 9420 leaves access
    /// control to the application — and by the time the delta exists,
    /// `decrypt_from_group` has already merged the commit, advancing our
    /// epoch irreversibly. The only way to reject would be to refuse the
    /// merge, which forks us permanently from every peer that accepted it
    /// (see `check_epoch_forks`: members left on a forked branch have to be
    /// re-invited by the app, and a fork the local role metadata caused —
    /// it is best-effort and can legitimately lag — would be a partition
    /// with no attacker involved). An unrecoverable partition is a worse
    /// failure than an insider membership change, so the change is applied
    /// and reported truthfully.
    ///
    /// Unlike the commit-metadata gate in `process_commit_core`, this covers
    /// removals too. When receive-side enforcement is enabled
    /// (`GroupConfig::enforce_admin_commits`) the *rejection* happens earlier,
    /// pre-merge in the MLS layer, and never reaches this function — but it
    /// reports through the same [`Self::report_unauthorized_membership_change`]
    /// so both outcomes surface on one signal.
    pub(crate) fn judge_membership_change(
        &mut self,
        group_id: &str,
        sender: &str,
        added: &[String],
        removed: &[String],
        claim_matches_delta: bool,
    ) -> MembershipJudgment {
        let sender_is_admin = self.check_is_admin(group_id, sender).unwrap_or(false);

        // A membership change is authorized only when an admin committed it
        // *and* the commit's own framing agrees with the delta it produced.
        let authorized = sender_is_admin && claim_matches_delta;

        if !authorized {
            let reason = if sender_is_admin {
                "affected_member_mismatch"
            } else {
                "sender_not_admin"
            };
            self.report_unauthorized_membership_change(
                group_id, sender, added, removed, reason, false,
            );
        }

        MembershipJudgment {
            sender_is_admin,
            authorized,
        }
    }

    /// Emits the rate-limited unauthorized-membership-change report.
    ///
    /// Shared by both outcomes so apps see one signal regardless of
    /// configuration: `enforced == false` means the change was applied and is
    /// reported after the fact (the default), `enforced == true` means the
    /// commit was refused before merging and no membership change occurred.
    ///
    /// Rate-limited per `(group, committer, enforced)` because divergent role
    /// metadata would otherwise re-fire on every commit from the same peer.
    /// `enforced` is part of the key so the two outcomes never suppress each
    /// other: a refusal is a partition alarm — the device stopped merging and
    /// now trails the group's epoch — and losing the first one of those behind
    /// an earlier report-only event would hide the very condition the app is
    /// told to act on. Within one outcome class the window still collapses
    /// repeats, which is what the limit is for. The tracking map is bounded:
    /// lapsed windows are dropped first, then the oldest live entry.
    fn report_unauthorized_membership_change(
        &mut self,
        group_id: &str,
        committer: &str,
        added: &[String],
        removed: &[String],
        reason: &str,
        enforced: bool,
    ) {
        let key = (group_id.to_string(), committer.to_string(), enforced);
        let now = Instant::now();
        let suppressed = self
            .group_mesh
            .unauthorized_change_reports
            .get(&key)
            .is_some_and(|last| {
                now.duration_since(*last).as_secs() < UNAUTHORIZED_REPORT_SUPPRESS_SECS
            });
        if suppressed {
            debug!(
                group_id = %group_id,
                sender = %committer,
                reason,
                enforced,
                "Suppressing repeated unauthorized-membership-change report within the rate-limit window"
            );
            return;
        }

        // Bound the tracking map: drop lapsed windows first, then the
        // oldest live entry if still at cap.
        if self.group_mesh.unauthorized_change_reports.len() >= MAX_UNAUTHORIZED_REPORT_ENTRIES {
            self.group_mesh
                .unauthorized_change_reports
                .retain(|_, last| {
                    now.duration_since(*last).as_secs() < UNAUTHORIZED_REPORT_SUPPRESS_SECS
                });
        }
        if self.group_mesh.unauthorized_change_reports.len() >= MAX_UNAUTHORIZED_REPORT_ENTRIES {
            if let Some(oldest) = self
                .group_mesh
                .unauthorized_change_reports
                .iter()
                .min_by_key(|(_, t)| **t)
                .map(|(k, _)| k.clone())
            {
                self.group_mesh.unauthorized_change_reports.remove(&oldest);
            }
        }
        self.group_mesh.unauthorized_change_reports.insert(key, now);
        if enforced {
            warn!(
                group_id = %group_id,
                sender = %committer,
                reason,
                added = ?added,
                removed = ?removed,
                "SECURITY: Unauthorized group membership change refused — the commit was not merged, so our epoch now trails members that accepted it"
            );
        } else {
            warn!(
                group_id = %group_id,
                sender = %committer,
                reason,
                added = ?added,
                removed = ?removed,
                "SECURITY: Unauthorized group membership change applied — MLS accepted the commit, but the SDK's admin policy did not authorize it"
            );
        }
        self.emit_event(Event::group_unauthorized_membership_change(
            group_id.to_string(),
            committer.to_string(),
            added.to_vec(),
            removed.to_vec(),
            reason.to_string(),
            enforced,
        ));
    }

    /// Releases every replay-protection record for a buffered entry dropped
    /// undelivered (cap eviction or TTL expiry), so a sender-side redelivery
    /// is accepted fresh instead of being rejected as a replay of a copy
    /// that no longer exists anywhere.
    ///
    /// Two layers must be released. The group-level dedup table rejects
    /// redeliveries inside the group handlers; the transport-level
    /// deduplicator rejects them earlier, at the receive loop — on the mesh
    /// path the envelope ID doubles as the group dedup key, so without this
    /// release a redelivery would be swallowed (and re-ACKed as delivered)
    /// for up to the deduplicator's retention window without ever reaching
    /// the group handlers. Relay-path IDs are relay payload IDs that never
    /// enter the transport deduplicator (the platform bridges mint a fresh
    /// envelope UUID per injected message), so `from_str` fails or the
    /// unmark is a no-op there — both harmless.
    pub(crate) fn release_replay_protection(&mut self, message_id: &str) {
        self.group_mesh.message_dedup.remove(message_id);
        if let Ok(envelope_id) = MessageId::from_str(message_id) {
            self.deduplicator.unmark_seen(&envelope_id);
        }
    }

    /// Buffers a failed commit for deferred retry.
    ///
    /// The commit's message ID is already in the dedup table, so this
    /// buffered copy is its only path to processing — whenever a buffered
    /// commit is dropped unprocessed (cap eviction here, TTL expiry at
    /// drain), its replay protection is released so a redelivery gets a
    /// fresh chance instead of being permanently rejected, mirroring
    /// `buffer_pending_group_message`.
    pub(crate) fn buffer_pending_commit(
        &mut self,
        group_id: &str,
        message_id: &str,
        sender: &str,
        data: &str,
    ) {
        let pending = PendingCommit {
            sender: sender.to_string(),
            message_id: message_id.to_string(),
            data: data.to_string(),
            buffered_at: Instant::now(),
            retry_count: 0,
        };
        let Some(evicted) = enforce_global_buffer_bound(
            &mut self.group_mesh.pending_commits,
            group_id,
            &PENDING_COMMIT_CAPS,
            pending.data.len(),
            |c| c.data.len(),
            |c| c.buffered_at,
        ) else {
            // Only the ciphertext field is size-guarded at decode time; the
            // JSON around it is attacker-sized, so an oversized payload must
            // be dropped rather than allowed to purge the whole buffer.
            warn!(
                group_id = %group_id,
                size = pending.data.len(),
                "Pending commit exceeds the global byte budget, dropping"
            );
            return;
        };
        // An evicted commit was never processed — release its replay
        // protection so a sender-side redelivery is accepted fresh instead
        // of being rejected as a replay of a copy that no longer exists.
        for entry in &evicted {
            self.release_replay_protection(&entry.message_id);
        }
        let displaced = {
            let buf = self
                .group_mesh
                .pending_commits
                .entry(group_id.to_string())
                .or_default();
            let displaced = if buf.len() >= MAX_PENDING_COMMITS_PER_GROUP {
                warn!(
                    group_id = %group_id,
                    "Pending commit buffer full, dropping oldest"
                );
                buf.pop_front()
            } else {
                None
            };
            buf.push_back(pending);
            debug!(
                group_id = %group_id,
                buffered_count = buf.len(),
                "Buffered out-of-order commit for deferred retry"
            );
            displaced
        };
        if let Some(displaced) = displaced {
            self.release_replay_protection(&displaced.message_id);
        }
    }

    /// Drains and retries buffered pending commits for a group after a
    /// successful commit advanced the epoch. Each successful retry triggers
    /// another drain pass (at most `MAX_PENDING_COMMITS_PER_GROUP` iterations).
    ///
    /// Call-site contract: a commit success inside this drain can unblock
    /// buffered application messages, but this function does not drain them —
    /// every caller must follow with `drain_pending_group_messages`.
    ///
    /// Uses `process_commit_core` (not `handle_group_mls_commit`) to avoid
    /// double-buffering: this method owns the retry lifecycle and re-buffers
    /// only via `still_pending`, never through the core processing path.
    pub(crate) fn drain_pending_commits(&mut self, group_id: &str) {
        // Track commits that were retried at least once and still expired —
        // these are strong evidence of epoch mismatch (true fork), not just
        // slow delivery from the mesh network.
        let mut retried_expired_count: usize = 0;

        // Limit iterations to avoid unbounded looping
        for _ in 0..MAX_PENDING_COMMITS_PER_GROUP {
            let pending = match self.group_mesh.pending_commits.get_mut(group_id) {
                Some(buf) if !buf.is_empty() => std::mem::take(buf),
                _ => break,
            };

            let mut any_succeeded = false;
            let mut still_pending = VecDeque::new();

            for entry in pending {
                // Drop expired entries — only count retried ones for fork detection
                if entry.buffered_at.elapsed() > StdDuration::from_secs(PENDING_COMMIT_TTL_SECS) {
                    debug!(
                        group_id = %group_id,
                        retry_count = entry.retry_count,
                        "Dropping expired pending commit"
                    );
                    if entry.retry_count > 0 {
                        retried_expired_count += 1;
                    }
                    // Never processed — release replay protection so a
                    // redelivery is accepted fresh even before the periodic
                    // dedup sweep runs.
                    self.release_replay_protection(&entry.message_id);
                    continue;
                }
                match self.process_commit_core(&entry.sender, &entry.data) {
                    CommitOutcome::Success(_) => {
                        any_succeeded = true;
                    }
                    CommitOutcome::Retriable(_) => {
                        // Still out-of-order — keep for next pass with incremented retry count
                        let mut entry = entry;
                        entry.retry_count += 1;
                        still_pending.push_back(entry);
                    }
                    CommitOutcome::Rejected => {
                        // Permanently bad — drop silently
                    }
                }
            }

            // Re-buffer commits that still failed
            if !still_pending.is_empty() {
                self.group_mesh
                    .pending_commits
                    .entry(group_id.to_string())
                    .or_default()
                    .extend(still_pending);
            }

            if !any_succeeded {
                break;
            }
            // Another commit succeeded — loop again in case it unblocked more
        }

        // Clean up empty entries
        if self
            .group_mesh
            .pending_commits
            .get(group_id)
            .is_none_or(|v| v.is_empty())
        {
            self.group_mesh.pending_commits.remove(group_id);
        }

        // If commits that were retried (and kept failing) expired, this is a
        // strong signal of epoch fork — the commits were valid but for a
        // different epoch branch. Commits that expired without any retries
        // are more likely just slow mesh delivery and are not flagged.
        if retried_expired_count > 0 && !self.group_mesh.epoch_forks.contains_key(group_id) {
            self.flag_potential_epoch_fork(group_id);
        }
    }

    /// Buffers a group application message whose decryption failed because
    /// local group state lagged (Welcome not yet processed / epoch behind).
    ///
    /// The message ID is already in the dedup table, so redeliveries via
    /// other transports are rejected there — this buffered copy is the
    /// message's only path to delivery, via `drain_pending_group_messages`.
    /// Correspondingly, whenever a buffered copy is dropped undelivered
    /// (cap eviction here, TTL expiry at drain), its replay protection is
    /// released so a redelivery gets a fresh chance instead of being
    /// permanently lost.
    pub(crate) fn buffer_pending_group_message(
        &mut self,
        group_id: &str,
        pending: PendingGroupMessage,
    ) {
        let Some(evicted) = enforce_global_buffer_bound(
            &mut self.group_mesh.pending_group_messages,
            group_id,
            &PENDING_GROUP_MESSAGE_CAPS,
            pending.ciphertext_b64.len(),
            |m| m.ciphertext_b64.len(),
            |m| m.buffered_at,
        ) else {
            // Unreachable in practice — the ciphertext passed the 1 MiB
            // base64_decode guard on both inbound paths — but the invariant
            // belongs here, not in the callers.
            warn!(
                group_id = %group_id,
                size = pending.ciphertext_b64.len(),
                "Pending group message exceeds the global byte budget, dropping"
            );
            return;
        };
        // An evicted message was never delivered — release its replay
        // protection so a sender-side redelivery is accepted fresh instead
        // of being rejected as a replay of a copy that no longer exists.
        for entry in &evicted {
            self.release_replay_protection(&entry.message_id);
        }
        let displaced = {
            let buf = self
                .group_mesh
                .pending_group_messages
                .entry(group_id.to_string())
                .or_default();
            let displaced = if buf.len() >= MAX_PENDING_GROUP_MESSAGES_PER_GROUP {
                warn!(
                    group_id = %group_id,
                    "Pending group message buffer full, dropping oldest"
                );
                buf.pop_front()
            } else {
                None
            };
            buf.push_back(pending);
            debug!(
                group_id = %group_id,
                buffered_count = buf.len(),
                "Buffered out-of-order group message for deferred retry"
            );
            displaced
        };
        if let Some(displaced) = displaced {
            self.release_replay_protection(&displaced.message_id);
        }
    }

    /// Whether a message id is currently buffered awaiting decryption for the
    /// given group. Drives the deferred-ACK decision in
    /// [`Self::handle_group_mls_msg_via`]'s duplicate branch: a retransmit of a
    /// still-pending message must be deferred (no ACK), while a duplicate of an
    /// already-delivered one is re-ACKed so the sender can stop.
    /// Matches both the envelope id and (for re-issued broadcast copies) the
    /// logical id, so a cross-path retransmit of a still-pending logical
    /// message is deferred rather than re-ACKed as delivered.
    fn is_group_message_pending(&self, group_id: &str, message_id: &str) -> bool {
        self.group_mesh
            .pending_group_messages
            .get(group_id)
            .is_some_and(|buf| {
                buf.iter().any(|m| {
                    m.message_id == message_id || m.logical_id.as_deref() == Some(message_id)
                })
            })
    }

    /// Sends the deferred delivery ACK for a group message surfaced by the
    /// drain, on the transport it originally arrived on.
    ///
    /// Mirrors the DM `ack_drained_message`: it closes the ACK-latency window
    /// so a sender that exhausts its retry budget before local group state
    /// catches up still learns of delivery, instead of marking a
    /// locally-delivered message undeliverable. `received_via` is `None` for
    /// relay-path entries (the relay sender is not ACK-gated) and transport-less
    /// test enqueue, in which case this is a correct no-op and recovery falls
    /// back to the sender's next resend hitting the duplicate re-ACK path.
    fn ack_drained_group_message(
        &mut self,
        ack_to: &str,
        acked_message_id: &str,
        received_via: Option<TransportType>,
    ) {
        let Some(transport) = received_via else {
            return;
        };
        if let Err(err) = self.send_group_delivery_ack(ack_to, acked_message_id, transport) {
            error!(
                msg_id = %acked_message_id,
                error = %err,
                "Failed to send deferred delivery ACK for drained group message"
            );
        }
    }

    /// Drains buffered group application messages after local MLS state for
    /// the group advanced (successful Welcome join or commit). Messages are
    /// retried in arrival order; entries that still fail retriably are
    /// re-buffered until their TTL expires.
    ///
    /// A buffered entry can turn out to be a commit or proposal riding the
    /// application channel (`NonApplication`): MLS consumes it, so group
    /// state may have advanced mid-drain. When that happens, buffered
    /// commits are drained and another pass runs, since entries re-buffered
    /// earlier in the same pass may now decrypt. Terminates: a pass loops
    /// only after consuming a `NonApplication` entry, which is never
    /// re-buffered, so each looping pass shrinks the buffer.
    ///
    /// Double delivery is impossible by construction rather than by
    /// bookkeeping: MLS consumes a ratchet generation on decrypt, so a copy
    /// of a ciphertext that was already decrypted elsewhere fails here
    /// (`Decryption` → `Retriable`) and never reaches the emit. Reaching the
    /// `Plaintext` branch *proves* this logical message has not been
    /// delivered before. `delivered_here` exists only to stop a second
    /// buffered copy from burning that pointless decrypt within one drain —
    /// see the declaration.
    pub(crate) fn drain_pending_group_messages(&mut self, group_id: &str) {
        // Ids (envelope and logical) this drain has already delivered. Both
        // the mesh re-issue of a relay broadcast and the relay's own copy of
        // it can sit buffered at once while group state lags, and they carry
        // the same ciphertext. Once one is delivered, the other must be
        // dropped without a decrypt attempt: its generation is spent, so MLS
        // would classify it `Retriable` and re-buffer it as noise until its
        // TTL — whose expiry would then call `release_replay_protection` on
        // an id that WAS delivered, re-opening a replay window for it.
        let mut delivered_here: HashSet<String> = HashSet::new();
        loop {
            let pending = match self.group_mesh.pending_group_messages.get_mut(group_id) {
                Some(buf) if !buf.is_empty() => std::mem::take(buf),
                _ => break,
            };

            let mut state_advanced = false;
            let mut still_pending = VecDeque::new();
            for entry in pending {
                // A second buffered copy of something this drain just
                // delivered. Checked before the TTL branch so it can never
                // release replay protection for a delivered id.
                if delivered_here.contains(&entry.message_id)
                    || entry
                        .logical_id
                        .as_deref()
                        .is_some_and(|l| delivered_here.contains(l))
                {
                    debug!(
                        group_id = %group_id,
                        msg_id = %entry.message_id,
                        "Buffered copy of a message this drain already delivered, dropping"
                    );
                    // The frame *is* delivered (as a duplicate), so ACK it or
                    // the sender keeps retransmitting a message we hold.
                    // `received_via` is `None` on the relay path, where the
                    // sender is not ACK-gated — a correct no-op there.
                    self.ack_drained_group_message(
                        &entry.sender,
                        &entry.message_id,
                        entry.received_via,
                    );
                    continue;
                }
                if entry.buffered_at.elapsed()
                    > StdDuration::from_secs(PENDING_GROUP_MESSAGE_TTL_SECS)
                {
                    debug!(
                        group_id = %group_id,
                        msg_id = %entry.message_id,
                        "Dropping expired pending group message"
                    );
                    // Never delivered — release replay protection so a
                    // redelivery is accepted fresh even before the periodic
                    // dedup sweep runs.
                    self.release_replay_protection(&entry.message_id);
                    continue;
                }
                // Validated at buffer time; failure here is defensive only.
                let Ok(ciphertext_bytes) = base64_decode(&entry.ciphertext_b64) else {
                    continue;
                };
                match self.decrypt_group_application(group_id, ciphertext_bytes, &entry.sender) {
                    GroupDecryptOutcome::Plaintext(plaintext) => {
                        let Ok(text) = String::from_utf8(plaintext) else {
                            warn!(
                                group_id = %group_id,
                                "Decrypted group payload is not valid UTF-8, dropping"
                            );
                            continue;
                        };
                        info!(
                            group_id = %group_id,
                            msg_id = %entry.message_id,
                            "Delivered buffered group message after group state caught up"
                        );
                        // Capture the ACK targets before the fields move into
                        // the event: the message is delivered now, so the drain
                        // sends the deferred delivery ACK directly on the
                        // recorded arrival transport instead of waiting for the
                        // sender's next resend. The ACK is always keyed on the
                        // envelope id (`message_id`) — that is what the sender's
                        // ack_manager awaits.
                        let received_via = entry.received_via;
                        let ack_sender = entry.sender.clone();
                        let ack_message_id = entry.message_id.clone();
                        // A re-issued broadcast copy carries the logical id the
                        // whole group knows this message by: emit that id and
                        // mark it delivered, so the relay path's copy of the
                        // same logical message dedups against this one.
                        //
                        // No "already delivered elsewhere?" check is needed
                        // here, and none would be correct: the successful
                        // decrypt above already proves this ciphertext's
                        // ratchet generation was unspent, i.e. that no other
                        // copy was delivered before it. A copy that *was*
                        // beaten to it fails decrypt and lands in `Retriable`,
                        // never here.
                        let emit_id = match entry.logical_id {
                            Some(logical) => {
                                self.group_mesh
                                    .message_dedup
                                    .insert(logical.clone(), Instant::now());
                                logical
                            }
                            None => entry.message_id,
                        };
                        delivered_here.insert(ack_message_id.clone());
                        delivered_here.insert(emit_id.clone());
                        let (content, media_metadata, content_type, forward_info_event) =
                            Self::restore_group_rich(text, entry.forward_info, &entry.sender);
                        let timestamp = entry
                            .timestamp
                            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
                        self.emit_event(Event::group_message_received(
                            group_id.to_string(),
                            entry.sender,
                            content,
                            timestamp,
                            emit_id,
                            entry.reply_to,
                            forward_info_event,
                            media_metadata,
                            content_type,
                        ));
                        self.ack_drained_group_message(&ack_sender, &ack_message_id, received_via);
                    }
                    GroupDecryptOutcome::Retriable => {
                        still_pending.push_back(entry);
                    }
                    GroupDecryptOutcome::NonApplication => {
                        state_advanced = true;
                    }
                    GroupDecryptOutcome::PolicyRejected {
                        committer,
                        added,
                        removed,
                    } => {
                        // Dropped, not re-buffered: the refusal is permanent,
                        // so re-queueing would retry it until the entry ages
                        // out and then misreport the expiry as an epoch fork.
                        self.report_unauthorized_membership_change(
                            group_id,
                            &committer,
                            &added,
                            &removed,
                            "sender_not_admin",
                            true,
                        );
                    }
                    GroupDecryptOutcome::SecurityRejected
                    | GroupDecryptOutcome::NotMlsCiphertext
                    | GroupDecryptOutcome::Failed => {
                        // Dropped unprocessed — release replay protection, for
                        // the same reason the arrival path unmarks a rejected
                        // relay copy (see the marking note atop
                        // `handle_relay_group_message_with_mls`) and the TTL
                        // arm above releases an expired one. A relay copy can
                        // outrun its Welcome, so a mis-attributed one is judged
                        // *here* rather than at arrival — and a hostile relay
                        // picks that ordering for free. Left marked, the id
                        // reads as delivered while no longer being pending, so
                        // the per-member re-issue that is this message's own
                        // delivery safety net is absorbed as an
                        // already-delivered duplicate and re-ACKed: delivered
                        // nowhere, sender told otherwise.
                        //
                        // `PolicyRejected` above deliberately does not release:
                        // that refusal is permanent, so a later copy could only
                        // waste work.
                        self.release_replay_protection(&entry.message_id);
                    }
                }
            }

            if !still_pending.is_empty() {
                self.group_mesh
                    .pending_group_messages
                    .entry(group_id.to_string())
                    .or_default()
                    .extend(still_pending);
            }

            if !state_advanced {
                break;
            }
            // The consumed commit may also unblock buffered commit-channel
            // commits; drain them before the next message pass (the pass itself
            // satisfies drain_pending_commits' call-site contract).
            self.drain_pending_commits(group_id);
        }

        // Clean up empty entries
        if self
            .group_mesh
            .pending_group_messages
            .get(group_id)
            .is_none_or(|v| v.is_empty())
        {
            self.group_mesh.pending_group_messages.remove(group_id);
        }
    }

    /// Flags a group as having a potential epoch fork.
    ///
    /// Called when buffered commits expire without resolution, indicating
    /// the group may have diverged due to concurrent commits.
    fn flag_potential_epoch_fork(&mut self, group_id: &str) {
        if self.group_mesh.epoch_forks.len() >= MAX_EPOCH_FORK_ENTRIES {
            // Evict the oldest entry to prevent unbounded growth
            if let Some(oldest_key) = self
                .group_mesh
                .epoch_forks
                .values()
                .min_by_key(|f| f.detected_at)
                .map(|f| f.group_id.clone())
            {
                self.group_mesh.epoch_forks.remove(&oldest_key);
            }
        }

        let local_epoch = self.refresh_group_members(group_id).ok().and_then(|_| {
            let mls_guard = self.read_mls_guard().ok()?;
            let gid = offline_protocol_mls::GroupId::new(group_id).ok()?;
            mls_guard
                .get_group_info(&gid)
                .ok()
                .flatten()
                .map(|i| i.epoch)
        });

        if local_epoch.is_none() {
            warn!(
                group_id = %group_id,
                "Could not read local MLS epoch during fork detection — MLS unavailable or group not found"
            );
        }

        warn!(
            group_id = %group_id,
            local_epoch = ?local_epoch,
            "Potential epoch fork detected — buffered commits expired without resolution"
        );

        self.group_mesh.epoch_forks.insert(
            group_id.to_string(),
            EpochForkState {
                group_id: group_id.to_string(),
                local_epoch,
                detected_at: Instant::now(),
                resolution_attempted: false,
            },
        );

        self.emit_event(Event::group_epoch_fork_detected(
            group_id.to_string(),
            local_epoch,
        ));
    }

    /// Handles an incoming group leave notification.
    ///
    /// After verifying the sender, the lexicographically-first remaining member
    /// (deterministic election) issues an MLS remove-commit to advance the group
    /// epoch and revoke the leaving member's keys. Other members will receive
    /// the commit via `handle_group_mls_commit`.
    ///
    /// # Security limitations
    ///
    /// Leave notifications are **not** MLS-authenticated because a member cannot
    /// issue a self-removal Commit in MLS. The `sender` field comes from the
    /// `Message` envelope which is not cryptographically bound. In an adversarial
    /// mesh environment a relay node could forge a leave notification to force-
    /// remove a legitimate member. Mitigations:
    ///
    /// 1. `sender == leaving_member` check prevents cross-member impersonation
    ///    when the transport layer preserves sender identity.
    /// 2. Membership is verified against the **MLS group state** (authoritative),
    ///    not just the local cache.
    /// 3. The elected remover issues a real MLS Commit that all members verify
    ///    cryptographically.
    ///
    /// For fully adversarial environments, consider requiring admin-only removal
    /// or adding an MLS application-message-signed leave proof.
    pub(crate) fn handle_group_mls_leave(&mut self, message_id: &str, sender: &str, data: &str) {
        let payload = match serde_json::from_str::<GroupMlsLeavePayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupMlsLeave payload");
                return;
            }
        };

        // Dedup check — prevents duplicate leave notifications from resetting
        // the election timer or triggering redundant remove-commit attempts.
        let dedup_key = message_id.to_string();
        if self.group_mesh.message_dedup.contains_key(&dedup_key) {
            debug!(
                group_id = %payload.group_id,
                msg_id = %dedup_key,
                "Duplicate leave notification, skipping"
            );
            return;
        }
        self.group_mesh
            .message_dedup
            .insert(dedup_key, Instant::now());
        if self.group_mesh.message_dedup.len() > MAX_GROUP_MESSAGE_DEDUP_ENTRIES {
            self.cleanup_group_message_dedup();
        }

        // Verify sender matches the claimed leaving member to prevent spoofing
        if payload.leaving_member != sender {
            error!(
                sender = %sender,
                claimed = %payload.leaving_member,
                group_id = %payload.group_id,
                "SECURITY: Leave notification sender mismatch — possible spoofing attempt, ignoring"
            );
            return;
        }

        // Verify sender is actually a member of the group using MLS state
        // (authoritative) rather than only the local cache.
        let self_id = self.local_id.clone();
        let members = self
            .refresh_group_members(&payload.group_id)
            .ok()
            .or_else(|| self.group_mesh.members.get(&payload.group_id).cloned())
            .unwrap_or_default();

        if !members.iter().any(|m| m == sender) {
            error!(
                sender = %sender,
                group_id = %payload.group_id,
                "SECURITY: Leave notification from non-member, ignoring"
            );
            return;
        }

        info!(
            group_id = %payload.group_id,
            leaving_member = %payload.leaving_member,
            "Received mesh group leave notification"
        );

        // Deterministic election: lexicographically-first remaining member
        // (excluding the leaver) issues the MLS remove-commit to advance epoch.
        let mut remaining: Vec<String> = members
            .iter()
            .filter(|m| m.as_str() != sender)
            .cloned()
            .collect();
        remaining.sort();

        let should_remove = remaining
            .first()
            .map(|first| first == &self_id)
            .unwrap_or(false);

        if should_remove {
            debug!(
                group_id = %payload.group_id,
                leaving_member = %sender,
                "Elected to issue MLS remove-commit for leaving member"
            );
            // Issue MLS remove + distribute commit to advance epoch and revoke keys.
            // remove_from_group emits GroupMemberRemoved, so we skip the event below.
            if let Err(e) = self.remove_from_group(&payload.group_id, sender) {
                warn!(
                    group_id = %payload.group_id,
                    leaving_member = %sender,
                    error = %e,
                    "Failed to issue MLS remove-commit for leaving member"
                );
            }
            // Clear any pending election for this leave (we handled it)
            let key = (payload.group_id.clone(), sender.to_string());
            self.group_mesh.pending_leave_elections.remove(&key);
        } else {
            // We're not the elected remover — record the election so we can
            // take over if the elected member fails to issue the commit in time.
            if self.group_mesh.pending_leave_elections.len() >= MAX_PENDING_LEAVE_ELECTIONS {
                // Evict the oldest entry to prevent unbounded growth.
                if let Some(oldest_key) = self
                    .group_mesh
                    .pending_leave_elections
                    .iter()
                    .min_by_key(|(_, e)| e.received_at)
                    .map(|(k, _)| k.clone())
                {
                    self.group_mesh.pending_leave_elections.remove(&oldest_key);
                }
            }
            let key = (payload.group_id.clone(), sender.to_string());
            self.group_mesh.pending_leave_elections.insert(
                key,
                PendingLeaveElection {
                    group_id: payload.group_id.clone(),
                    leaving_member: sender.to_string(),
                    received_at: Instant::now(),
                    last_attempt_at: None,
                },
            );
        }

        // Do NOT emit GroupMemberRemoved here for non-elected nodes.
        // The elected node issues an MLS remove-commit which all members
        // (including us) will process via `handle_group_mls_commit` →
        // `process_commit_core`, which emits the event based on the actual
        // MLS membership delta. Emitting here would cause a duplicate event
        // because we'd emit once now (premature, before MLS state changes)
        // and once when the commit arrives.
    }

    /// Refreshes the cached member list for a group from MlsManager.
    pub(crate) fn refresh_group_members(&mut self, group_id: &str) -> Result<Vec<String>> {
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id)?;
        let info = mls_guard
            .get_group_info(&gid)?
            .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
        let members = info.members.clone();
        drop(mls_guard);
        self.group_mesh
            .members
            .insert(group_id.to_string(), members.clone());
        Ok(members)
    }

    /// Creates a new MLS group.
    ///
    /// The group is created locally via MLS. Members can be invited with
    /// `invite_to_group()`. Messages sent via `send_group_message()` are
    /// MLS-encrypted and route through whichever transport DORS selects.
    /// If Internet is available the group is also registered with the
    /// relay server for optimized fan-out.
    pub fn create_group(&mut self, group_name: &str) -> Result<offline_protocol_mls::GroupInfo> {
        let trimmed = group_name.trim();
        if trimmed.is_empty() {
            return Err(Error::InvalidArgument(
                "Group name cannot be empty".to_string(),
            ));
        }
        let mls_guard = self.read_mls_guard()?;
        let group_info = mls_guard.create_group(group_name)?;
        let group_id = group_info.group_id.as_str().to_string();
        let members = group_info.members.clone();
        drop(mls_guard);

        self.group_mesh
            .members
            .insert(group_id.clone(), members.clone());

        self.emit_event(Event::group_created(
            group_id.clone(),
            group_name.to_string(),
        ));

        // Best-effort relay registration
        let _ = self.try_relay_register_group(&group_id, Some(group_name), &members);

        Ok(group_info)
    }

    /// Invites a user to an MLS group.
    ///
    /// Requires the invitee's key package to be available in `pending_key_packages`.
    /// Sends a Welcome to the invitee and a Commit to all existing members.
    ///
    /// Admission is where recipient validation belongs. The removal and role
    /// paths deliberately do *not* validate: a roster entry admitted by an
    /// older build (before the id rules tightened) must stay removable, and a
    /// gate there would turn "a bad member exists" into "a bad member cannot be
    /// removed" — the wrong direction for a moderation control.
    pub fn invite_to_group(&mut self, group_id: &str, invitee_user_id: &str) -> Result<()> {
        Self::validate_outbound_recipient(invitee_user_id)?;

        // Admin check
        let self_id = self.local_id.clone();
        if !self.check_is_admin(group_id, &self_id)? {
            return Err(Error::PermissionDenied(
                "Only admins can invite members".to_string(),
            ));
        }

        // Check group member cap before adding
        let current_count = self
            .group_mesh
            .members
            .get(group_id)
            .map(|m| m.len())
            .or_else(|| {
                self.read_mls_guard().ok().and_then(|g| {
                    g.get_group_info(&offline_protocol_mls::GroupId::new(group_id).ok()?)
                        .ok()
                        .flatten()
                        .map(|info| info.members.len())
                })
            })
            .unwrap_or(0);
        let max_members = self.config.group.max_group_members;
        if current_count >= max_members {
            return Err(Error::InvalidState(format!(
                "Group has {} members, cannot exceed {} limit",
                current_count, max_members
            )));
        }

        // Try loading key package from storage (e.g. after restart or rapid
        // re-invite where a fresh package arrived between calls).
        self.try_load_key_package_from_storage_into_memory(invitee_user_id);

        // Get the invitee's key package and check expiry
        let now_ms = Utc::now().timestamp_millis() as u64;
        let received_pkg = self
            .pending_key_packages
            .get(invitee_user_id)
            .ok_or_else(|| Error::NoKeyPackage(invitee_user_id.to_string()))?;
        if now_ms >= received_pkg.local_expires_at_ms {
            self.pending_key_packages.remove(invitee_user_id);
            return Err(Error::InvalidState(format!(
                "Key package for {} has expired",
                invitee_user_id
            )));
        }
        let key_pkg = received_pkg.key_package_data.clone();

        // Add member via MLS — returns both Welcome (for invitee) and Commit (for existing members)
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id)?;
        let (welcome, commit) = mls_guard.add_group_member(&gid, invitee_user_id, &key_pkg)?;
        let group_name = welcome.group_name.clone();
        drop(mls_guard);

        // MLS key packages are single-use (RFC 9420). Now that add_group_member
        // consumed the package, remove it so subsequent group invites for the
        // same peer don't reuse a stale package.
        self.pending_key_packages.remove(invitee_user_id);
        self.delete_peer_key_package_from_storage(invitee_user_id);

        // Clear key-package-sent tracking so the invitee can reciprocate.
        // Without this, the invitee's handler sees us in `key_package_sent_to`
        // and skips reciprocation, leaving us without a key package for
        // future group invites.
        self.key_package_sent_to.remove(invitee_user_id);

        // Send our key package to the invitee. Their message dispatch handler
        // will reciprocate with a fresh key package, replenishing our supply
        // for future group invites or 1:1 session creation.
        if let Err(e) = self.send_key_package_to(invitee_user_id, false) {
            debug!(
                invitee = %invitee_user_id,
                error = %e,
                "Key package send to invitee deferred (will retry on next discovery)"
            );
        }

        // Refresh member list after add
        let members = self.refresh_group_members(group_id)?;

        // Store invitee as member role, then read back both role-overlay
        // fields the Welcome carries (roles and the creator of record) from a
        // single metadata load.
        let (member_roles, created_by) = {
            let mls_guard = self.read_mls_guard()?;
            let gid = offline_protocol_mls::GroupId::new(group_id)?;
            if let Err(e) = mls_guard.set_member_role(&gid, invitee_user_id, GroupRole::Member) {
                warn!(invitee = %invitee_user_id, error = %e, "Failed to store invitee role");
            }
            mls_guard
                .get_group_metadata(&gid)
                .ok()
                .flatten()
                .map(|m| (m.get_all_roles(), m.created_by.clone()))
                .unwrap_or_default()
        };

        // Attest rich-payload capability both directions of the knowledge
        // gap an add creates: existing members never exchange key packages
        // with the invitee (commit field below), and the invitee never
        // exchanges with anyone but us (welcome map here). Entries only
        // exist for members we know capable — directly or via an earlier
        // attestation, which is how knowledge chains across successive adds
        // — so absence means "no information", never a downgrade. Our own
        // entry self-advertises from config (belt to the key package sent
        // above, whose delivery is best-effort).
        let invitee_rich = self.attestable_rich_versions(invitee_user_id);
        let member_rich: HashMap<String, Vec<u8>> = members
            .iter()
            .filter(|m| m.as_str() != invitee_user_id)
            .filter_map(|m| {
                if *m == self_id {
                    self.config
                        .encryption
                        .rich_payload_enabled
                        .then(|| (m.clone(), vec![RICH_PAYLOAD_V1]))
                } else {
                    self.attestable_rich_versions(m).map(|v| (m.clone(), v))
                }
            })
            .collect();

        // Send Welcome to invitee
        let welcome_payload = GroupMlsWelcomePayload {
            group_id: group_id.to_string(),
            group_name,
            welcome_data: base64_encode(&welcome.welcome_data),
            member_list: members.clone(),
            member_roles,
            member_rich,
            // Carries our creator of record so the joiner's `check_is_admin`
            // has a fallback when the role snapshot above is incomplete.
            // Absent when we never learned it ourselves (joined via an older
            // SDK's Welcome) — absence is "no information", not a downgrade.
            created_by,
        };
        let welcome_content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_WELCOME,
            serde_json::to_string(&welcome_payload)
                .map_err(|e| Error::Serialization(format!("Serialize welcome: {}", e)))?
        );
        self.send_internal_message(invitee_user_id, welcome_content, MessagePriority::High)?;

        // Send Commit to all existing members (excluding self and invitee)
        // so they can process it and advance their MLS epoch
        let commit_payload = GroupMlsCommitPayload {
            group_id: group_id.to_string(),
            commit_type: GroupCommitType::Add,
            ciphertext: base64_encode(&commit.ciphertext),
            epoch: commit.epoch,
            affected_member: Some(invitee_user_id.to_string()),
            role: Some(GroupRole::Member),
            affected_member_rich: invitee_rich,
        };
        let commit_content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_COMMIT,
            serde_json::to_string(&commit_payload)
                .map_err(|e| Error::Serialization(format!("Serialize commit: {}", e)))?
        );
        let mut failed_commit_members: Vec<String> = Vec::new();
        for member in &members {
            if member == &self_id || member == invitee_user_id {
                continue;
            }
            if let Err(e) =
                self.send_internal_message(member, commit_content.clone(), MessagePriority::High)
            {
                warn!(
                    group_id = %group_id,
                    member = %member,
                    error = %e,
                    "Failed to send commit to group member during invite"
                );
                failed_commit_members.push(member.clone());
            }
        }
        // Single best-effort retry pass for transient failures
        for member in &failed_commit_members {
            let _ =
                self.send_internal_message(member, commit_content.clone(), MessagePriority::High);
        }

        self.emit_event(Event::group_member_added(
            group_id.to_string(),
            invitee_user_id.to_string(),
            self_id,
            None,
            // Our own invite, gated by the admin check at the top of this fn.
            Some(true),
        ));

        // Sync membership update to relay
        let _ = self.try_relay_register_group(group_id, None, &members);

        info!(group_id = %group_id, invitee = %invitee_user_id, "Invited member to group");
        Ok(())
    }

    /// Removes a member from an MLS group.
    ///
    /// Sends a Commit to all remaining members.
    pub fn remove_from_group(&mut self, group_id: &str, member_id: &str) -> Result<()> {
        // Deliberately unvalidated — see `invite_to_group`. Removal must work
        // on whatever is actually on the roster.
        let self_id = self.local_id.clone();

        // Admin check + last-admin guard (single metadata load)
        {
            let mls_guard = self.read_mls_guard()?;
            let gid = offline_protocol_mls::GroupId::new(group_id)?;
            let metadata = Self::group_metadata_or_not_found(&mls_guard, &gid, group_id)?;

            let is_admin = if let Some(ref meta) = metadata {
                if meta.has_any_admin() {
                    meta.get_role(&self_id) == GroupRole::Admin
                } else if let Some(ref creator) = meta.created_by {
                    creator == &self_id
                } else {
                    false
                }
            } else {
                false
            };
            if !is_admin {
                return Err(Error::PermissionDenied(
                    "Only admins can remove members".to_string(),
                ));
            }

            if let Some(ref meta) = metadata {
                if meta.get_role(member_id) == GroupRole::Admin {
                    let admin_count = meta
                        .get_all_roles()
                        .values()
                        .filter(|r| **r == GroupRole::Admin)
                        .count();
                    let member_count = self
                        .group_mesh
                        .members
                        .get(group_id)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    // member_count > 2: the count is *before* removal, so > 2
                    // means at least 2 members will remain after the removal.
                    // When only 2 members exist (the admin being removed + one
                    // other), the remaining sole member gets auto-promoted, so
                    // we allow the removal.
                    if admin_count <= 1 && member_count > 2 {
                        return Err(Error::InvalidState(
                            "Cannot remove the last admin while other members remain. Promote another member to admin first.".to_string(),
                        ));
                    }
                }
            }
        }

        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id)?;
        let commit_msg = mls_guard.remove_group_member(&gid, member_id)?;
        drop(mls_guard);

        // Refresh member list after removal
        let members = self.refresh_group_members(group_id)?;
        // Clean up removed member's role metadata
        if let Ok(mls_guard) = self.read_mls_guard() {
            if let Ok(gid) = offline_protocol_mls::GroupId::new(group_id) {
                if let Err(e) = mls_guard.remove_member_role(&gid, member_id) {
                    warn!(member = %member_id, error = %e, "Failed to clean up role metadata for removed member");
                }
            }
        }
        // Auto-promote if removal left zero admins
        self.auto_promote_if_no_admin(group_id, &members);

        let commit_payload = GroupMlsCommitPayload {
            group_id: group_id.to_string(),
            commit_type: GroupCommitType::Remove,
            ciphertext: base64_encode(&commit_msg.ciphertext),
            epoch: commit_msg.epoch,
            affected_member: Some(member_id.to_string()),
            role: None,
            affected_member_rich: None,
        };
        let commit_content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_COMMIT,
            serde_json::to_string(&commit_payload)
                .map_err(|e| Error::Serialization(format!("Serialize commit: {}", e)))?
        );
        let mut failed_commit_members: Vec<String> = Vec::new();
        for member in &members {
            if member == &self_id {
                continue;
            }
            if let Err(e) =
                self.send_internal_message(member, commit_content.clone(), MessagePriority::High)
            {
                warn!(
                    group_id = %group_id,
                    member = %member,
                    error = %e,
                    "Failed to send commit to group member during remove"
                );
                failed_commit_members.push(member.clone());
            }
        }
        // Single best-effort retry pass for transient failures
        for member in &failed_commit_members {
            let _ =
                self.send_internal_message(member, commit_content.clone(), MessagePriority::High);
        }

        // Send a plaintext removal notification directly to the removed member.
        // The MLS commit alone is insufficient: the removed member cannot decrypt
        // it (they're no longer in the group), so they would never learn they were
        // removed. The plaintext notification uses the existing GROUP_MEMBER_REMOVED
        // prefix which the message dispatcher handles by emitting a
        // GroupMemberRemoved event and cleaning up local state.
        let removed_payload = GroupMemberRemovedPayload {
            group_id: group_id.to_string(),
            user_id: member_id.to_string(),
            removed_by: self_id.clone(),
        };
        if let Ok(json) = serde_json::to_string(&removed_payload) {
            let removed_content = format!("{}{}", internal_prefixes::GROUP_MEMBER_REMOVED, json);
            if let Err(e) =
                self.send_internal_message(member_id, removed_content, MessagePriority::High)
            {
                warn!(
                    group_id = %group_id,
                    member = %member_id,
                    error = %e,
                    "Failed to send removal notification to removed member"
                );
            }
        }

        self.emit_event(Event::group_member_removed(
            group_id.to_string(),
            member_id.to_string(),
            self_id,
            // Our own removal, gated by the admin check at the top of this fn.
            Some(true),
        ));

        // Sync membership update to relay
        let _ = self.try_relay_register_group(group_id, None, &members);

        info!(group_id = %group_id, member = %member_id, "Removed member from group");
        Ok(())
    }

    /// Leaves an MLS group.
    ///
    /// Notifies remaining members and then removes local group state.
    /// Note: The MLS layer does not support self-removal Commits, so remaining
    /// members receive a plaintext leave notification. The deterministic election
    /// in `handle_group_mls_leave` will select one member to issue the MLS
    /// remove-commit to properly advance the epoch.
    ///
    /// **Ordering note:** Leave notifications are sent *before* deleting local
    /// MLS state. If all notification sends fail, local state is preserved and
    /// the caller receives an error so the leave can be retried. This prevents
    /// orphaned membership where the leaver is gone locally but peers never
    /// learn about the departure.
    pub fn leave_group(&mut self, group_id: &str) -> Result<()> {
        // Get members before leaving
        let members = self
            .group_mesh
            .members
            .get(group_id)
            .cloned()
            .or_else(|| self.refresh_group_members(group_id).ok())
            .unwrap_or_default();

        let self_id = self.local_id.clone();

        // Block last admin from leaving if other members remain
        let other_members_exist = members.iter().any(|m| m != &self_id);
        if other_members_exist {
            let mls_guard = self.read_mls_guard()?;
            let gid = offline_protocol_mls::GroupId::new(group_id)?;
            if let Some(metadata) = mls_guard.get_group_metadata(&gid)? {
                let is_admin = metadata.get_role(&self_id) == GroupRole::Admin;
                if is_admin {
                    let admin_count = metadata
                        .get_all_roles()
                        .values()
                        .filter(|r| **r == GroupRole::Admin)
                        .count();
                    if admin_count <= 1 {
                        return Err(Error::InvalidState(
                            "Cannot leave group as the last admin while other members remain. Promote another member to admin first.".to_string(),
                        ));
                    }
                }
            }
            drop(mls_guard);
        }

        // Build leave notification payload before touching MLS state
        let leave_payload = GroupMlsLeavePayload {
            group_id: group_id.to_string(),
            leaving_member: self_id.clone(),
        };
        let leave_content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_LEAVE,
            serde_json::to_string(&leave_payload)
                .map_err(|e| Error::Serialization(format!("Serialize leave: {}", e)))?
        );

        // Send notifications first — if all fail, keep local state intact for retry
        let mut any_sent = false;
        let mut had_recipients = false;
        for member in &members {
            if member == &self_id {
                continue;
            }
            had_recipients = true;
            match self.send_internal_message(member, leave_content.clone(), MessagePriority::Medium)
            {
                Ok(_) => {
                    any_sent = true;
                }
                Err(e) => {
                    warn!(
                        group_id = %group_id,
                        member = %member,
                        error = %e,
                        "Failed to send leave notification to group member"
                    );
                }
            }
        }

        // If there were other members but no notification succeeded, fail so the
        // caller can retry rather than silently orphaning the membership.
        if had_recipients && !any_sent {
            return Err(Error::Transport(
                offline_protocol_transport::Error::SendFailed(
                    "All leave notifications failed — local state preserved for retry".to_string(),
                ),
            ));
        }

        // Now safe to delete local MLS state — at least one peer was notified
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id)?;
        mls_guard.leave_group(&gid)?;
        drop(mls_guard);

        // Remove from caches. Buffered commits go too — they can never apply
        // after we leave, and a stale one retried after a rapid re-invite
        // would expire with retry_count > 0 and falsely flag an epoch fork.
        self.group_mesh.members.remove(group_id);
        let was_synced = self.group_mesh.relay_synced.remove(group_id);
        let was_pending = self
            .group_mesh
            .relay_register_pending
            .remove(group_id)
            .is_some();
        if was_synced || was_pending {
            self.emit_event(Event::group_relay_sync_changed(
                group_id.to_string(),
                false,
                "left",
            ));
        }
        self.group_mesh.pending_group_messages.remove(group_id);
        self.group_mesh.pending_commits.remove(group_id);

        info!(group_id = %group_id, "Left group");
        Ok(())
    }

    /// Sends a message to all members of an MLS group.
    ///
    /// If Internet transport is available and relay-enabled, sends a single
    /// relay broadcast (`__GRP_RELAY_BCAST__`). Otherwise falls back to
    /// per-member fan-out via `send_internal_message()` where each member's
    /// delivery goes through the full DORS/ACK/retry stack independently.
    pub fn send_group_message(
        &mut self,
        group_id: &str,
        content: &str,
        priority: Option<MessagePriority>,
        reply_to_msg: Option<&str>,
    ) -> Result<Vec<MessageId>> {
        // Reject reserved internal prefixes: receivers parse decrypted group
        // plaintext for the sealed `__RICH_V1__` body, so user content must
        // never be able to impersonate one (same rule as `send_message` and
        // the forward paths).
        if Self::is_internal_prefix(content) {
            return Err(Error::InvalidArgument(
                "Message content must not start with a reserved internal prefix".to_string(),
            ));
        }
        self.send_group_message_inner(
            group_id,
            content,
            priority.unwrap_or(MessagePriority::Medium),
            reply_to_msg,
            None,
            None,
            ContentType::Text,
        )
    }

    /// Like [`Self::send_group_message`], with rich media fields.
    ///
    /// The rich fields travel inside the MLS-sealed `__RICH_V1__` body of
    /// the group ciphertext, and only when every other member advertised
    /// `rich_versions` (see [`GroupSendOptions`]); otherwise they are
    /// silently dropped, never cleartext.
    pub fn send_group_message_with(
        &mut self,
        group_id: &str,
        content: &str,
        options: GroupSendOptions,
    ) -> Result<Vec<MessageId>> {
        // Same internal-prefix rejection as `send_group_message`.
        if Self::is_internal_prefix(content) {
            return Err(Error::InvalidArgument(
                "Message content must not start with a reserved internal prefix".to_string(),
            ));
        }
        // FileChunk is an internal transport content type — same boundary
        // rule as `send_message_with`.
        if options.content_type == Some(ContentType::FileChunk) {
            return Err(Error::InvalidArgument(
                "FileChunk is an internal content type and cannot be sent directly".to_string(),
            ));
        }
        Self::check_group_rich_extras_size(options.media_metadata.as_ref(), None)?;
        self.send_group_message_inner(
            group_id,
            content,
            options.priority.unwrap_or(MessagePriority::Medium),
            options.reply_to_msg.as_deref(),
            None,
            options.media_metadata,
            options.content_type.unwrap_or_default(),
        )
    }

    /// Forwards a message to all members of a group with forwarding attribution.
    ///
    /// Similar to `send_group_message` but attaches `ForwardInfo` to preserve
    /// the original sender and forward count. The message content is encrypted
    /// via MLS for the group and fan-out follows the same path as regular
    /// group messages (including relay broadcast when available).
    ///
    /// When every other group member advertised the sealed rich payload, the
    /// attribution and the original `media_metadata` — including cloud-media
    /// `encryption_key`/`iv` secrets, which only the sealed body may carry —
    /// travel inside the group MLS ciphertext, so forwarded cloud media stays
    /// openable. Otherwise the media metadata is dropped (never cleartext)
    /// and only the hop-visible payload attribution survives.
    pub fn forward_message_to_group(
        &mut self,
        original_message: &Message,
        group_id: &str,
        priority: Option<MessagePriority>,
    ) -> Result<Vec<MessageId>> {
        // Reject content that starts with an internal control prefix to prevent
        // injection of protocol-level messages through the forwarding API.
        if Self::is_internal_prefix(&original_message.content) {
            return Err(Error::InvalidArgument(
                "Cannot forward a message with reserved internal prefix content".to_string(),
            ));
        }

        // FileChunk is an internal transport content type — same boundary
        // rule as the DM forward path.
        if original_message.content_type == ContentType::FileChunk {
            return Err(Error::InvalidArgument(
                "FileChunk is an internal content type and cannot be forwarded".to_string(),
            ));
        }

        let forward_info = ForwardInfo::from_message(original_message);

        if forward_info.forward_count > crate::constants::MAX_FORWARD_COUNT {
            return Err(Error::InvalidArgument(format!(
                "Forward count {} exceeds maximum of {}",
                forward_info.forward_count,
                crate::constants::MAX_FORWARD_COUNT,
            )));
        }

        Self::check_group_rich_extras_size(
            original_message.media_metadata.as_ref(),
            Some(&forward_info),
        )?;

        self.send_group_message_inner(
            group_id,
            &original_message.content,
            priority.unwrap_or(MessagePriority::Medium),
            None,
            Some(forward_info),
            original_message.media_metadata.clone(),
            original_message.content_type,
        )
    }

    /// Boundary cap for group rich extras, mirroring `send_message_with`:
    /// bound the serialized extras here so the seal (or a relay-broadcast
    /// fallback re-send) can never fail on size later.
    fn check_group_rich_extras_size(
        media_metadata: Option<&MediaMetadata>,
        forward_info: Option<&ForwardInfo>,
    ) -> Result<()> {
        RichSendExtras {
            reply_context: None,
            media_metadata: media_metadata.cloned(),
            forward_info: forward_info.cloned(),
        }
        .check_size()
    }

    /// Whether a rich group send right now would seal its extras, and which
    /// members are in the way. See [`GroupRichReadiness`]. Read-only: uses
    /// the fan-out cache with an MLS fallback but never mutates state, and
    /// does not probe unknown members (the send path's drop branch does).
    pub fn group_rich_readiness(&self, group_id: &str) -> Result<GroupRichReadiness> {
        let members = match self.group_mesh.members.get(group_id) {
            Some(m) => m.clone(),
            None => {
                let mls_guard = self.read_mls_guard()?;
                let gid = offline_protocol_mls::GroupId::new(group_id)?;
                mls_guard
                    .get_group_info(&gid)?
                    .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?
                    .members
            }
        };
        let unknown_members = if self.config.encryption.rich_payload_enabled {
            self.group_rich_unknown_members(&members)
        } else {
            Vec::new()
        };
        Ok(GroupRichReadiness {
            ready: self.group_rich_seal_active(&members),
            unknown_members,
        })
    }

    /// The relay-side registration state of a group. See [`RelaySyncState`].
    ///
    /// `Synced` wins over an outstanding re-registration: once the relay has
    /// positively acknowledged the group on this connection, its registry
    /// entry survives regardless of any in-flight idempotent re-sync, so
    /// relay-dependent commands are already safe to issue.
    pub fn group_relay_sync_state(&self, group_id: &str) -> RelaySyncState {
        if self.group_mesh.relay_synced.contains(group_id) {
            RelaySyncState::Synced
        } else if self
            .group_mesh
            .relay_register_pending
            .contains_key(group_id)
        {
            RelaySyncState::Pending
        } else {
            RelaySyncState::Unsynced
        }
    }

    /// Registers (or re-registers) a group with the relay server on demand.
    ///
    /// The supported path for making a mesh-created group known to the
    /// relay before issuing relay-dependent server commands for it (invite
    /// links, server-side fan-out) — never raw-send `CreateGroup`. The
    /// outcome arrives asynchronously as `GroupRelaySyncChanged`:
    /// `synced: true, reason: "registered"` on the relay's ack, `false`
    /// with `"error"` / `"ack_timeout"` on denial or a relay that never
    /// answers. Idempotent: an already-`Synced` group returns `Ok(true)`
    /// without re-sending (the automatic re-sync on membership changes
    /// covers roster updates).
    ///
    /// Returns `Ok(true)` when the registration frame was queued (or the
    /// group is already synced), `Ok(false)` when relay grouping is
    /// disabled or the Internet transport is unavailable, `Err` when the
    /// group is unknown locally.
    pub fn request_group_relay_registration(&mut self, group_id: &str) -> Result<bool> {
        if self.group_mesh.relay_synced.contains(group_id) {
            return Ok(true);
        }
        // Fresh membership, like the retry processor: the MLS roster is
        // authoritative, the fan-out cache is the fallback.
        let members = self
            .refresh_group_members(group_id)
            .ok()
            .or_else(|| self.group_mesh.members.get(group_id).cloned())
            .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
        self.try_relay_register_group(group_id, None, &members)
    }

    /// Shared implementation for sending and forwarding group messages.
    ///
    /// Handles the sealed-rich decision, MLS encryption, member list
    /// caching, relay broadcast attempt, per-member fan-out, and event
    /// emission.
    #[allow(clippy::too_many_arguments)]
    fn send_group_message_inner(
        &mut self,
        group_id: &str,
        content: &str,
        priority: MessagePriority,
        reply_to_msg: Option<&str>,
        forward_info: Option<ForwardInfo>,
        media_metadata: Option<MediaMetadata>,
        content_type: ContentType,
    ) -> Result<Vec<MessageId>> {
        // Read member list from cache, falling back to MLS on cache miss.
        // Fetched before encryption because the sealed-rich decision below
        // needs the full membership: one ciphertext serves every member.
        let members = match self.group_mesh.members.get(group_id) {
            Some(m) => m.clone(),
            None => {
                let mls_guard = self.read_mls_guard()?;
                let gid = offline_protocol_mls::GroupId::new(group_id)?;
                let info = mls_guard
                    .get_group_info(&gid)?
                    .ok_or_else(|| Error::GroupNotFound(group_id.to_string()))?;
                info.members.clone()
            }
        };

        // Update cache if it was a miss
        if !self.group_mesh.members.contains_key(group_id) {
            self.group_mesh
                .members
                .insert(group_id.to_string(), members.clone());
        }

        // Rich extras travel only inside the sealed `__RICH_V1__` body of
        // the group ciphertext (media secrets must never leave the AEAD
        // boundary), and only when every other member advertised the
        // capability — a legacy member would render the sealed body as
        // literal JSON text. For unsealed sends the attribution rides the
        // hop-visible payload below as the fallback; media metadata has no
        // such fallback and simply drops (surfaced to the app via
        // `GroupRichExtrasDropped` — the send itself still succeeds). A
        // non-Text hint seals a hint-only body even without extras
        // (mirroring the DM path): the group payload has no outer
        // content_type carrier, so an unsealed hint would not merely go
        // unprotected — it would be lost.
        let extras = RichSendExtras {
            reply_context: None,
            media_metadata,
            forward_info: forward_info.clone(),
        };
        let wants_seal = extras.is_any() || content_type != ContentType::Text;
        let sealed = wants_seal && self.group_rich_seal_active(&members);
        let sealed_body;
        let plaintext: &str = if sealed {
            sealed_body = Self::seal_rich_payload(content, &extras, content_type)?;
            &sealed_body
        } else {
            // Members holding the gate closed (empty when the local kill
            // switch is the cause — with recording disabled, listing every
            // member would be noise, and probing them could not reopen the
            // gate anyway). Probing sends them our key package so their
            // auto-exchange reply teaches us their capability: unknown
            // members are typically ones somebody else added, healed here
            // for groups predating attestation or behind an old-SDK inviter.
            // Only sends that wanted to seal consume the set (backfill here,
            // blame on the drop event below) — plain text sends skip the
            // membership scan entirely.
            let unknown = if wants_seal && self.config.encryption.rich_payload_enabled {
                self.group_rich_unknown_members(&members)
            } else {
                Vec::new()
            };
            if !unknown.is_empty() {
                self.backfill_group_rich_capabilities(&unknown);
            }
            if extras.media_metadata.is_some() {
                warn!(
                    group_id = %group_id,
                    "Group not fully rich-capable, dropping rich media metadata (members \
                     will receive the text without the media attachment)"
                );
                self.emit_event(Event::group_rich_extras_dropped(
                    group_id.to_string(),
                    unknown,
                ));
            }
            content
        };

        // When the body sealed, every member reads the sealed attribution
        // and the receiver ignores the payload copy wholesale — omit it
        // rather than expose original-sender metadata to relays and mesh
        // hops for nobody's benefit. (A stale-cache just-added non-capable
        // member loses degraded-display attribution in that window;
        // accepted, consistent with them rendering the sealed body as
        // literal JSON anyway.)
        let payload_forward_info = if sealed { None } else { forward_info };

        // Encrypt via MLS — release the guard immediately after encryption
        // to minimize lock contention during the fan-out phase.
        let encrypted = {
            let mls_guard = self.read_mls_guard()?;
            let gid = offline_protocol_mls::GroupId::new(group_id)?;
            mls_guard.encrypt_for_group(&gid, plaintext.as_bytes())?
        };

        let ciphertext_b64 = base64_encode(&encrypted.ciphertext);
        let epoch = encrypted.epoch;

        let self_id = self.local_id.clone();

        // --- Attempt relay broadcast first ---
        // If the group is registered, the app opted in, and the relay
        // advertised the v3 group-delivery contract, a single relay
        // broadcast is O(1) instead of O(N) individual sends.
        //
        // The capability gate is what makes the broadcast safe to take by
        // default: a `group_delivery_v3` relay answers every broadcast with
        // a settled per-recipient delivery report, which the sender consumes
        // (`handle_relay_group_delivery_report`) to re-send per-member
        // copies to anyone the relay did not reach — so the broadcast keeps
        // a delivery contract instead of being fire-and-forget — and names
        // members in the same address space this SDK registered, so that
        // report is comparable against the MLS roster at all (see
        // [`RELAY_CAP_GROUP_DELIVERY_V3`] for why a v2 relay must fail this
        // gate). Against an older relay the gate simply fails and sends take
        // the always-correct per-member fan-out.
        //
        // `is_internet_available()` is re-checked here and not inferred from
        // `relay_synced`: sync state is cleared by the `process()` tick, so
        // between Internet dropping and the next tick a stale `relay_synced`
        // would otherwise route the broadcast into the mesh.
        if self.config.group.relay_broadcast_enabled
            && self.group_mesh.relay_synced.contains(group_id)
            && self
                .group_mesh
                .relay_capabilities
                .contains(RELAY_CAP_GROUP_DELIVERY_V3)
            && self.is_internet_available()
        {
            if let Ok(mid) = self.try_relay_broadcast(
                group_id,
                &ciphertext_b64,
                epoch,
                reply_to_msg,
                payload_forward_info.clone(),
                priority,
            ) {
                let member_count = members.iter().filter(|m| m.as_str() != self_id).count() as u32;
                self.emit_event(Event::group_message_sent(
                    group_id.to_string(),
                    vec![mid.as_str().to_string()],
                    member_count,
                ));
                return Ok(vec![mid]);
            }
            // Relay broadcast failed — fall through to per-member fan-out
        }

        // Per-member fan-out (mesh or DORS-selected transport)
        let (message_ids, succeeded_members, failed_members) = self
            .fanout_group_frames_per_member(
                group_id,
                &members,
                None,
                &ciphertext_b64,
                epoch,
                reply_to_msg,
                payload_forward_info,
                priority,
            )?;

        let member_count = succeeded_members.len() as u32;

        // Check for total delivery failure: there were members to send to but
        // every send failed. Return an error so callers don't confuse this with
        // a solo-group scenario (which legitimately returns Ok(vec![])).
        let had_recipients = members.iter().any(|m| m != &self_id);
        if had_recipients && message_ids.is_empty() {
            self.emit_event(Event::group_message_partial_failure(
                group_id.to_string(),
                failed_members,
                succeeded_members,
            ));
            return Err(Error::Transport(
                offline_protocol_transport::Error::SendFailed(
                    "All group message sends failed".to_string(),
                ),
            ));
        }

        // Emit appropriate event
        if failed_members.is_empty() {
            self.emit_event(Event::group_message_sent(
                group_id.to_string(),
                message_ids.iter().map(|m| m.as_str().to_string()).collect(),
                member_count,
            ));
        } else {
            self.emit_event(Event::group_message_partial_failure(
                group_id.to_string(),
                failed_members,
                succeeded_members,
            ));
        }

        Ok(message_ids)
    }

    /// Lists all MLS groups (excluding 1:1 sessions).
    pub fn list_groups(&self) -> Result<Vec<String>> {
        let mls_guard = self.read_mls_guard()?;
        let groups = mls_guard.list_groups()?;
        Ok(groups.into_iter().map(|g| g.as_str().to_string()).collect())
    }

    /// Gets information about an MLS group.
    pub fn get_group_info(
        &self,
        group_id: &str,
    ) -> Result<Option<offline_protocol_mls::GroupInfo>> {
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id)?;
        Ok(mls_guard.get_group_info(&gid)?)
    }

    // ========================================================================
    // GROUP ROLES
    // ========================================================================

    /// Deterministically promotes the lexicographically-first remaining member
    /// to admin if no admin role remains. All nodes use the same sort order,
    /// so the promotion converges to the same candidate network-wide.
    fn auto_promote_if_no_admin(&self, group_id: &str, members: &[String]) {
        if members.is_empty() {
            return;
        }
        if let Ok(mls_guard) = self.read_mls_guard() {
            let Ok(gid) = offline_protocol_mls::GroupId::new(group_id) else {
                return;
            };
            if let Some(metadata) = mls_guard.get_group_metadata(&gid).ok().flatten() {
                if metadata.has_any_admin() {
                    return;
                }
                let mut sorted = members.to_vec();
                sorted.sort();
                let promote_id = &sorted[0];
                if let Err(e) = mls_guard.set_member_role(&gid, promote_id, GroupRole::Admin) {
                    warn!(user_id = %promote_id, error = %e, "Failed to auto-promote member to admin");
                } else {
                    info!(user_id = %promote_id, group_id = %group_id, "Auto-promoted member to admin after last admin was removed");
                }
            }
        }
    }

    /// Loads a group's role metadata, failing with [`Error::GroupNotFound`]
    /// when the group has neither metadata nor MLS state. Admin-gated
    /// operations use this so a missing group is not misreported as a
    /// permissions failure.
    fn group_metadata_or_not_found(
        mls_guard: &offline_protocol_mls::MlsManager,
        gid: &offline_protocol_mls::GroupId,
        group_id: &str,
    ) -> Result<Option<offline_protocol_mls::GroupMetadata>> {
        let metadata = mls_guard.get_group_metadata(gid)?;
        if metadata.is_none() && mls_guard.get_group_info(gid)?.is_none() {
            return Err(Error::GroupNotFound(group_id.to_string()));
        }
        Ok(metadata)
    }

    /// Checks if a user is an admin of the given group.
    ///
    /// Falls back to `created_by` when no admin role has been stored yet
    /// (handles groups created before role tracking was introduced).
    ///
    /// Returns `GroupNotFound` when no MLS group exists locally, so
    /// admin-gated operations don't misreport a missing group as a
    /// permissions failure. Inbound handlers that verify senders treat
    /// this error the same as a deny.
    pub(crate) fn check_is_admin(&self, group_id: &str, user_id: &str) -> Result<bool> {
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id)?;
        let metadata = Self::group_metadata_or_not_found(&mls_guard, &gid, group_id)?;
        drop(mls_guard);

        if let Some(meta) = &metadata {
            if meta.has_any_admin() {
                return Ok(meta.get_role(user_id) == GroupRole::Admin);
            }
            // No admin role stored — fall back to created_by if available
            if let Some(creator) = &meta.created_by {
                return Ok(creator == user_id);
            }
        }

        // Group exists but has no role metadata — deny by default
        Ok(false)
    }

    /// Changes a member's role in a group (admin only).
    /// Broadcasts the change to all group members.
    pub fn set_member_role(
        &mut self,
        group_id: &str,
        target_user_id: &str,
        role: GroupRole,
    ) -> Result<()> {
        // Deliberately unvalidated — see `invite_to_group`. Demoting a member
        // admitted under older id rules must stay possible.
        let self_id = self.local_id.clone();
        if !self.check_is_admin(group_id, &self_id)? {
            return Err(Error::PermissionDenied(
                "Only admins can change roles".to_string(),
            ));
        }

        // Verify target is a member (before acquiring guard for role operations)
        let members = self
            .group_mesh
            .members
            .get(group_id)
            .cloned()
            .or_else(|| self.refresh_group_members(group_id).ok())
            .unwrap_or_default();
        if !members.iter().any(|m| m == target_user_id) {
            return Err(Error::InvalidState(format!(
                "User {} is not a member of group {}",
                target_user_id, group_id
            )));
        }

        // Single guard acquisition for both last-admin validation and role write
        {
            let mls_guard = self.read_mls_guard()?;
            let gid = offline_protocol_mls::GroupId::new(group_id)?;

            // Prevent demoting the last admin (whether self or another admin)
            if role == GroupRole::Member {
                if let Some(metadata) = mls_guard.get_group_metadata(&gid).ok().flatten() {
                    if metadata.get_role(target_user_id) == GroupRole::Admin {
                        let admin_count = metadata
                            .get_all_roles()
                            .values()
                            .filter(|r| **r == GroupRole::Admin)
                            .count();
                        if admin_count <= 1 {
                            return Err(Error::InvalidState(
                                "Cannot demote the last admin".to_string(),
                            ));
                        }
                    }
                }
            }

            // Store locally
            mls_guard.set_member_role(&gid, target_user_id, role)?;
        }

        // Broadcast to all members
        let payload = GroupRoleChangePayload {
            group_id: group_id.to_string(),
            target_user_id: target_user_id.to_string(),
            new_role: role,
            changed_by: self_id.clone(),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_ROLE_CHANGE,
            serde_json::to_string(&payload)
                .map_err(|e| Error::Serialization(format!("Serialize role change: {}", e)))?
        );
        for member in &members {
            if member == &self_id {
                continue;
            }
            if let Err(e) =
                self.send_internal_message(member, content.clone(), MessagePriority::High)
            {
                warn!(member = %member, group_id = %group_id, error = %e, "Failed to send role change broadcast");
            }
        }

        self.emit_event(Event::group_role_changed(
            group_id.to_string(),
            target_user_id.to_string(),
            role.to_string(),
            self_id,
        ));

        info!(group_id = %group_id, target = %target_user_id, role = %role, "Changed member role");
        Ok(())
    }

    /// Gets a member's role in a group.
    ///
    /// A group that exists but has no role metadata (created before role
    /// tracking) reports the same default an unrecorded user gets: `Member`.
    pub fn get_member_role(&self, group_id: &str, user_id: &str) -> Result<GroupRole> {
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id)?;
        let metadata = Self::group_metadata_or_not_found(&mls_guard, &gid, group_id)?;
        Ok(metadata.map(|m| m.get_role(user_id)).unwrap_or_default())
    }

    /// Gets all member roles in a group.
    ///
    /// A group that exists but has no role metadata (created before role
    /// tracking) reports no explicit roles.
    pub fn get_group_roles(&self, group_id: &str) -> Result<HashMap<String, GroupRole>> {
        let mls_guard = self.read_mls_guard()?;
        let gid = offline_protocol_mls::GroupId::new(group_id)?;
        let metadata = Self::group_metadata_or_not_found(&mls_guard, &gid, group_id)?;
        Ok(metadata.map(|m| m.get_all_roles()).unwrap_or_default())
    }

    /// Handles an incoming group role change notification.
    pub(crate) fn handle_group_role_change(&mut self, message_id: &str, sender: &str, data: &str) {
        let payload = match serde_json::from_str::<GroupRoleChangePayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupRoleChange payload");
                return;
            }
        };

        // Dedup
        let dedup_key = message_id.to_string();
        if self.group_mesh.message_dedup.contains_key(&dedup_key) {
            return;
        }
        self.group_mesh
            .message_dedup
            .insert(dedup_key, Instant::now());

        // Verify sender is admin (use transport-authenticated sender, not payload.changed_by)
        match self.check_is_admin(&payload.group_id, sender) {
            Ok(true) => {}
            Ok(false) => {
                error!(
                    sender = %sender,
                    group_id = %payload.group_id,
                    "SECURITY: Role change from non-admin, ignoring"
                );
                return;
            }
            Err(e) => {
                warn!(
                    sender = %sender,
                    group_id = %payload.group_id,
                    error = %e,
                    "Failed to verify admin status for role change"
                );
                return;
            }
        }

        // Store role locally
        let Ok(gid) = offline_protocol_mls::GroupId::new(&payload.group_id) else {
            warn!(group_id = %payload.group_id, "Dropping role change with invalid group id");
            return;
        };
        if let Ok(mls_guard) = self.read_mls_guard() {
            if let Err(e) =
                mls_guard.set_member_role(&gid, &payload.target_user_id, payload.new_role)
            {
                warn!(
                    target = %payload.target_user_id,
                    error = %e,
                    "Failed to store role from incoming role change"
                );
            }
        }

        // Use transport-authenticated sender as changed_by, not the untrusted payload field
        self.emit_event(Event::group_role_changed(
            payload.group_id,
            payload.target_user_id,
            payload.new_role.to_string(),
            sender.to_string(),
        ));
    }

    // ========================================================================
    // GROUP RENAME
    // ========================================================================

    /// Renames a group (admin only).
    /// Updates the local group name and broadcasts the change to all members.
    pub fn rename_group(&mut self, group_id: &str, new_name: &str) -> Result<()> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(Error::InvalidArgument(
                "Group name cannot be empty".to_string(),
            ));
        }
        let self_id = self.local_id.clone();
        if !self.check_is_admin(group_id, &self_id)? {
            return Err(Error::PermissionDenied(
                "Only admins can rename groups".to_string(),
            ));
        }

        // Load old name before updating
        let old_name = {
            let mls_guard = self.read_mls_guard()?;
            let gid = offline_protocol_mls::GroupId::new(group_id)?;
            mls_guard
                .get_group_metadata(&gid)?
                .and_then(|m| m.name.clone())
        };

        // Update locally
        {
            let mls_guard = self.read_mls_guard()?;
            let gid = offline_protocol_mls::GroupId::new(group_id)?;
            mls_guard.set_group_name(&gid, new_name)?;
        }

        // Broadcast to all members
        let members = self
            .group_mesh
            .members
            .get(group_id)
            .cloned()
            .or_else(|| self.refresh_group_members(group_id).ok())
            .unwrap_or_default();

        let payload = GroupRenamePayload {
            group_id: group_id.to_string(),
            new_name: new_name.to_string(),
            renamed_by: self_id.clone(),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_RENAME,
            serde_json::to_string(&payload)
                .map_err(|e| Error::Serialization(format!("Serialize group rename: {}", e)))?
        );
        for member in &members {
            if member == &self_id {
                continue;
            }
            if let Err(e) =
                self.send_internal_message(member, content.clone(), MessagePriority::High)
            {
                warn!(member = %member, group_id = %group_id, error = %e, "Failed to send group rename broadcast");
            }
        }

        self.emit_event(Event::group_renamed(
            group_id.to_string(),
            new_name.to_string(),
            old_name,
            self_id,
        ));

        // Update relay registration with new name
        let _ = self.try_relay_register_group(group_id, Some(new_name), &members);

        info!(group_id = %group_id, new_name = %new_name, "Renamed group");
        Ok(())
    }

    /// Handles an incoming group rename notification.
    pub(crate) fn handle_group_rename(&mut self, message_id: &str, sender: &str, data: &str) {
        let payload = match serde_json::from_str::<GroupRenamePayload>(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse GroupRename payload");
                return;
            }
        };

        // Dedup
        let dedup_key = message_id.to_string();
        if self.group_mesh.message_dedup.contains_key(&dedup_key) {
            return;
        }
        self.group_mesh
            .message_dedup
            .insert(dedup_key, Instant::now());

        // Verify sender is admin (use transport-authenticated sender)
        match self.check_is_admin(&payload.group_id, sender) {
            Ok(true) => {}
            Ok(false) => {
                error!(
                    sender = %sender,
                    group_id = %payload.group_id,
                    "SECURITY: Group rename from non-admin, ignoring"
                );
                return;
            }
            Err(e) => {
                warn!(
                    sender = %sender,
                    group_id = %payload.group_id,
                    error = %e,
                    "Failed to verify admin status for group rename"
                );
                return;
            }
        }

        let Ok(gid) = offline_protocol_mls::GroupId::new(&payload.group_id) else {
            warn!(group_id = %payload.group_id, "Dropping group rename with invalid group id");
            return;
        };

        // Load old name before updating
        let old_name = if let Ok(mls_guard) = self.read_mls_guard() {
            mls_guard
                .get_group_metadata(&gid)
                .ok()
                .flatten()
                .and_then(|m| m.name.clone())
        } else {
            None
        };

        // Store new name locally
        if let Ok(mls_guard) = self.read_mls_guard() {
            if let Err(e) = mls_guard.set_group_name(&gid, &payload.new_name) {
                warn!(
                    group_id = %payload.group_id,
                    error = %e,
                    "Failed to store new group name from incoming rename"
                );
            }
        }

        // Use transport-authenticated sender as renamed_by
        self.emit_event(Event::group_renamed(
            payload.group_id,
            payload.new_name,
            old_name,
            sender.to_string(),
        ));
    }

    /// Cleans up expired group message dedup entries and enforces size cap.
    pub(crate) fn cleanup_group_message_dedup(&mut self) {
        // Ages are compared with `saturating_duration_since` rather than a
        // precomputed `now - TTL` cutoff: on platforms where the monotonic
        // clock starts at boot, that subtraction underflows (and panics)
        // when the process is younger than the TTL.
        let now = Instant::now();
        let dedup_ttl = StdDuration::from_secs(GROUP_MESSAGE_DEDUP_TTL_SECS);
        self.group_mesh
            .message_dedup
            .retain(|_, seen_at| now.saturating_duration_since(*seen_at) < dedup_ttl);
        // If still over cap after TTL cleanup, drop oldest entries using O(N) selection
        let len = self.group_mesh.message_dedup.len();
        if len > MAX_GROUP_MESSAGE_DEDUP_ENTRIES {
            let mut entries: Vec<_> = self.group_mesh.message_dedup.drain().collect();
            // Partition so the newest MAX entries are in [..MAX]
            entries.select_nth_unstable_by_key(MAX_GROUP_MESSAGE_DEDUP_ENTRIES, |(_, ts)| {
                std::cmp::Reverse(*ts)
            });
            entries.truncate(MAX_GROUP_MESSAGE_DEDUP_ENTRIES);
            self.group_mesh.message_dedup = entries.into_iter().collect();
        }

        // Expire stale pending commits, tracking groups where retried commits
        // expired — these are strong epoch fork signals (same as drain_pending_commits).
        // Expired IDs are collected for replay-protection release below: the
        // group-level dedup entry is at least as old as the buffered copy and
        // was already dropped by the retain above (same `now`, same TTL), but
        // the transport-level deduplicator has a longer retention window and
        // must be released explicitly or a redelivery is swallowed at the
        // receive loop.
        let commit_ttl = StdDuration::from_secs(PENDING_COMMIT_TTL_SECS);
        let mut fork_candidates: Vec<String> = Vec::new();
        let mut expired_ids: Vec<String> = Vec::new();
        self.group_mesh.pending_commits.retain(|group_id, commits| {
            let mut retried_expired = false;
            commits.retain(|c| {
                let alive = now.saturating_duration_since(c.buffered_at) < commit_ttl;
                if !alive {
                    if c.retry_count > 0 {
                        retried_expired = true;
                    }
                    expired_ids.push(c.message_id.clone());
                }
                alive
            });
            if retried_expired {
                fork_candidates.push(group_id.clone());
            }
            !commits.is_empty()
        });
        // Flag forks for groups where retried commits expired during periodic
        // cleanup — this covers the case where drain_pending_commits is never
        // called because no new commits succeed for the group.
        for group_id in fork_candidates {
            if !self.group_mesh.epoch_forks.contains_key(&group_id) {
                self.flag_potential_epoch_fork(&group_id);
            }
        }

        // Expire stale pending group messages — covers groups where no
        // Welcome or commit ever arrives to trigger a drain.
        let msg_ttl = StdDuration::from_secs(PENDING_GROUP_MESSAGE_TTL_SECS);
        self.group_mesh.pending_group_messages.retain(|_, msgs| {
            msgs.retain(|m| {
                let alive = now.saturating_duration_since(m.buffered_at) < msg_ttl;
                if !alive {
                    expired_ids.push(m.message_id.clone());
                }
                alive
            });
            !msgs.is_empty()
        });

        // Entries that expired here were never processed/delivered — release
        // their replay protection so a redelivery is accepted fresh.
        for message_id in expired_ids {
            self.release_replay_protection(&message_id);
        }
    }

    // ========================================================================
    // RELAY OPTIMIZATION
    // ========================================================================

    /// Best-effort admin hint for relay group registration: `Some` only when
    /// role metadata (or the creator fallback) makes the answer definite,
    /// `None` when it cannot be determined. Unlike [`Self::check_is_admin`],
    /// this must not deny by default — a wrong `false` would make the bridge
    /// silently skip member deltas the relay would have accepted, and the
    /// relay registry would never learn the membership.
    fn relay_registration_admin_hint(&self, group_id: &str) -> Option<bool> {
        let mls_guard = self.read_mls_guard().ok()?;
        let gid = offline_protocol_mls::GroupId::new(group_id).ok()?;
        let metadata = Self::group_metadata_or_not_found(&mls_guard, &gid, group_id).ok()??;
        if metadata.has_any_admin() {
            return Some(metadata.get_role(&self.local_id) == GroupRole::Admin);
        }
        metadata
            .created_by
            .as_ref()
            .map(|creator| creator == &self.local_id)
    }

    /// Attempts to register (or update) a group with the relay server.
    ///
    /// Sends a `__GRP_RELAY_REG__` message to the user's own ID via Internet
    /// transport, for a relay (or platform bridge acting as relay adapter)
    /// that translates the prefix into a server-side group registration.
    ///
    /// Enqueueing this frame proves nothing about relay support: a relay
    /// without prefix interception simply echoes a self-addressed message
    /// back to the sender. `relay_synced` must therefore only be set by a
    /// positive acknowledgment from the relay (an inbound `__GROUP_CREATED__`
    /// for the group), never here — otherwise `send_group_message` takes the
    /// O(1) broadcast path and the messages vanish into the echo.
    ///
    /// Fire-and-forget: returns `Ok(true)` if sent, `Ok(false)` if Internet
    /// is unavailable, `Err` on serialization failure.
    fn try_relay_register_group(
        &mut self,
        group_id: &str,
        group_name: Option<&str>,
        members: &[String],
    ) -> Result<bool> {
        if !self.config.group.relay_enabled || !self.is_internet_available() {
            return Ok(false);
        }

        let payload = RelayGroupRegistrationPayload {
            group_id: group_id.to_string(),
            group_name: group_name.map(|s| s.to_string()),
            members: members.to_vec(),
            is_admin: self.relay_registration_admin_hint(group_id),
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_RELAY_REGISTER,
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(format!(
                "Serialize relay registration: {}",
                e
            )))?
        );

        // Send to self — a prefix-aware relay (or bridge adapter) translates it.
        // Deliberately does NOT set `relay_synced`: Ok here only means the frame
        // was queued locally, and a prefix-unaware relay echoes it back instead
        // of registering anything. Sync is confirmed only by the relay's
        // `__GROUP_CREATED__` acknowledgment.
        //
        // Un-ACKed and Internet-pinned (see `send_relay_hint_message`): the
        // retry policy for registration is the `relay_register_pending`
        // tracker armed just below, not the message ACK ladder — which could
        // never settle this frame and would re-send it ~10 times.
        match self.send_relay_hint_message(content, MessagePriority::Medium) {
            Ok(_) => {
                // Arm the ack correlation: only a `__GROUP_CREATED__` that
                // answers an outstanding registration may set `relay_synced`.
                // Attempts accumulate across re-sends on this connection so
                // the expiry processor can give up on a relay that never
                // answers; the counter resets whenever the entry is removed
                // (ack, error, leave, internet drop).
                let entry = self
                    .group_mesh
                    .relay_register_pending
                    .entry(group_id.to_string())
                    .or_insert(RelayRegisterPending {
                        armed_at: Utc::now(),
                        attempts: 0,
                    });
                entry.armed_at = Utc::now();
                entry.attempts = entry.attempts.saturating_add(1);
                debug!(group_id = %group_id, attempts = entry.attempts, "Sent group registration to relay server");
                Ok(true)
            }
            Err(e) => {
                debug!(group_id = %group_id, error = %e, "Failed to register group with relay");
                Ok(false)
            }
        }
    }

    /// Applies the sealed `__RICH_V1__` restore to a decrypted group
    /// message plaintext, returning the event-ready fields: content, media
    /// metadata, content-type hint, and forward attribution.
    ///
    /// Parsing is never capability-gated (whatever a peer chose to seal, we
    /// try to read). When the body parses, the sealed attribution is
    /// authoritative wholesale — absence included: a sealing sender always
    /// seals its `forward_info` when one exists, so the hop-visible payload
    /// copy is consulted only for unsealed messages. Falling back to it on
    /// a sealed body would let a relay attach fabricated attribution to a
    /// rich message that carried none (mirrors the DM restore, which
    /// overwrites the outer fields wholesale). A body that fails to parse
    /// surfaces as raw text with nothing restored: never drop an
    /// authenticated message.
    fn restore_group_rich(
        text: String,
        payload_forward_info: Option<ForwardInfo>,
        sender: &str,
    ) -> (
        String,
        Option<MediaMetadata>,
        Option<String>,
        Option<crate::events::ForwardInfoEvent>,
    ) {
        if let Some(rich) = RichPayloadV1::parse_sealed(&text, sender) {
            return (
                rich.text,
                rich.media_metadata,
                rich.content_type.map(|ct| ct.to_string()),
                rich.forward_info
                    .as_ref()
                    .map(crate::events::ForwardInfoEvent::from),
            );
        }
        (
            text,
            None,
            None,
            payload_forward_info
                .as_ref()
                .map(crate::events::ForwardInfoEvent::from),
        )
    }

    /// Attempts to send a group message via relay broadcast.
    ///
    /// Sends a single `__GRP_RELAY_BCAST__` message to the user's own ID.
    /// The bridge translator (not the relay — see
    /// [`RelayGroupBroadcastPayload`]) turns it into a relay-native
    /// `SendGroupMessage`; the relay fans out to registered members, whose
    /// bridges inject the delivery as `__GROUP_MSG__` frames.
    ///
    /// Callers must only take this path when all four of
    /// `GroupConfig::relay_broadcast_enabled`, `relay_synced` (the relay
    /// acknowledged the group; otherwise a prefix-unaware relay swallows the
    /// frame), the relay's [`RELAY_CAP_GROUP_DELIVERY_V3`] capability, and
    /// `is_internet_available()` hold.
    ///
    /// Mints the logical message id the whole group will know this message
    /// by, carries it in the payload (the bridge stamps it onto the relay
    /// frame), and arms a [`RelayBroadcastPending`] entry keyed by it: the
    /// relay's settled `GroupMessageSent` report settles the entry and
    /// re-sends per-member copies to members the relay did not reach; a
    /// lost report re-sends the broadcast (bounded) and finally downgrades
    /// to full per-member fan-out. Returns that logical id on success — it
    /// is what `group_message_sent` reports to the app.
    ///
    /// On error (relay unreachable) the caller falls back to per-member
    /// fan-out. The frame goes out via [`Self::send_relay_hint_message`], so
    /// it is a pinned, un-ACKed one-shot: a failure surfaces here
    /// immediately instead of being deferred into a retry ladder that could
    /// never settle — the *report* tracker above is the bounded retry.
    fn try_relay_broadcast(
        &mut self,
        group_id: &str,
        ciphertext_b64: &str,
        epoch: u64,
        reply_to: Option<&str>,
        forward_info: Option<ForwardInfo>,
        priority: MessagePriority,
    ) -> Result<MessageId> {
        let logical_id = MessageId::new();
        let payload = RelayGroupBroadcastPayload {
            group_id: group_id.to_string(),
            ciphertext: ciphertext_b64.to_string(),
            epoch,
            reply_to: reply_to.map(|s| s.to_string()),
            forward_info: forward_info.clone(),
            message_id: logical_id.as_str(),
        };
        self.send_relay_broadcast_frame(&payload)?;
        // Keep the tracker bounded (see `MAX_RELAY_BROADCAST_PENDING`).
        // Terminates: every iteration removes exactly one entry.
        while self.group_mesh.relay_broadcast_pending.len() >= MAX_RELAY_BROADCAST_PENDING {
            let Some(oldest) = self
                .group_mesh
                .relay_broadcast_pending
                .iter()
                .min_by_key(|(_, pending)| pending.armed_at)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            let Some(stale) = self.group_mesh.relay_broadcast_pending.remove(&oldest) else {
                break;
            };
            warn!(
                msg_id = %oldest,
                "Relay broadcast tracker full, downgrading oldest to per-member fan-out"
            );
            self.downgrade_broadcast_to_per_member(&oldest, stale);
        }
        self.group_mesh.relay_broadcast_pending.insert(
            logical_id.as_str(),
            RelayBroadcastPending {
                group_id: group_id.to_string(),
                ciphertext_b64: ciphertext_b64.to_string(),
                epoch,
                reply_to: reply_to.map(|s| s.to_string()),
                forward_info,
                priority,
                armed_at: Utc::now(),
                attempts: 1,
            },
        );
        debug!(
            group_id = %group_id,
            msg_id = %logical_id,
            "Sent group message via relay broadcast, awaiting delivery report"
        );
        Ok(logical_id)
    }

    /// Serializes and sends one `__GRP_RELAY_BCAST__` hint frame. Shared by
    /// the initial broadcast and the lost-report re-send, which must reuse
    /// the same payload (same logical `message_id`) so the relay's push
    /// dedup and every receiver's dedup hold across attempts.
    fn send_relay_broadcast_frame(&mut self, payload: &RelayGroupBroadcastPayload) -> Result<()> {
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_RELAY_BROADCAST,
            serde_json::to_string(payload)
                .map_err(|e| Error::Serialization(format!("Serialize relay broadcast: {}", e)))?
        );
        self.send_relay_hint_message(content, MessagePriority::Medium)?;
        Ok(())
    }

    /// Fans a group message's MLS ciphertext out as one `__GRP_MLS_MSG__`
    /// frame per member (skipping self), each through
    /// `send_internal_message` and therefore the whole per-member delivery
    /// ladder. `logical_id` is `Some` when the copies re-issue a relay
    /// broadcast — receivers then dedup/emit under that id (see
    /// [`GroupMlsMessagePayload::message_id`]).
    ///
    /// Returns `(frame ids, succeeded members, failed members)`; errors only
    /// on payload serialization.
    #[allow(clippy::too_many_arguments)]
    fn fanout_group_frames_per_member(
        &mut self,
        group_id: &str,
        members: &[String],
        logical_id: Option<&str>,
        ciphertext_b64: &str,
        epoch: u64,
        reply_to: Option<&str>,
        forward_info: Option<ForwardInfo>,
        priority: MessagePriority,
    ) -> Result<(Vec<MessageId>, Vec<String>, Vec<String>)> {
        let msg_payload = GroupMlsMessagePayload {
            group_id: group_id.to_string(),
            ciphertext: ciphertext_b64.to_string(),
            epoch,
            reply_to: reply_to.map(|s| s.to_string()),
            forward_info,
            message_id: logical_id.map(|s| s.to_string()),
        };
        let base_content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_MSG,
            serde_json::to_string(&msg_payload)
                .map_err(|e| Error::Serialization(format!("Serialize group message: {}", e)))?
        );

        let self_id = self.local_id.clone();
        let mut message_ids = Vec::new();
        let mut succeeded_members = Vec::new();
        let mut failed_members = Vec::new();
        for member in members {
            if member == &self_id {
                continue;
            }
            match self.send_internal_message(member, base_content.clone(), priority) {
                Ok(mid) => {
                    message_ids.push(mid);
                    succeeded_members.push(member.clone());
                }
                Err(e) => {
                    warn!(
                        group_id = %group_id,
                        member = %member,
                        error = %e,
                        "Failed to send group message to member"
                    );
                    failed_members.push(member.clone());
                }
            }
        }
        Ok((message_ids, succeeded_members, failed_members))
    }

    /// Replaces the set of capability tokens advertised by the connected
    /// relay.
    ///
    /// Called by the platform internet bridge with the `capabilities` list
    /// from the relay's `Authenticated` answer, **before** it reports the
    /// transport up via `internet_status_changed(true)` — the false→true
    /// flush must already see the capabilities, or the first sends after a
    /// reconnect would take the capability-less path. Wholesale replace:
    /// each authentication describes the relay actually connected now.
    /// Relays that predate the advertisement yield an empty list, which
    /// reads as "no v2 contract" and keeps group sends on per-member
    /// fan-out.
    ///
    /// The list is wire-supplied: it is bounded to
    /// `MAX_RELAY_CAPABILITIES` tokens of at most
    /// `MAX_RELAY_CAPABILITY_TOKEN_BYTES` bytes each; excess entries and
    /// oversized tokens are dropped.
    pub fn set_relay_capabilities(&mut self, capabilities: Vec<String>) {
        let caps: HashSet<String> = capabilities
            .into_iter()
            .filter(|c| !c.is_empty() && c.len() <= MAX_RELAY_CAPABILITY_TOKEN_BYTES)
            .take(MAX_RELAY_CAPABILITIES)
            .collect();
        debug!(count = caps.len(), "Relay capabilities updated");
        self.group_mesh.relay_capabilities = caps;
    }

    /// Consumes a relay `GroupMessageSent` settled delivery report
    /// (forwarded verbatim by the platform bridge) for a broadcast this
    /// sender has in flight.
    ///
    /// Settles the `RelayBroadcastPending` entry correlated by
    /// `message_id`, then re-sends a per-member `__GRP_MLS_MSG__` copy —
    /// through the ordinary outbox/ACK/park ladder — to every MLS roster
    /// member the relay did not reach over a socket or a push: the members
    /// it names in `missed`, plus every roster member it does not name at
    /// all (the relay's registered roster can be a strict subset of the MLS
    /// roster, so "not mentioned" must read as "not reached", never as
    /// "covered"). Duplicates are safe: the copies carry the logical id, so
    /// a member that did receive the relay's copy absorbs them by dedup.
    ///
    /// Reports that correlate with nothing (already settled by a previous
    /// attempt's report, or downgraded meanwhile) are ignored. A report
    /// whose `group_id` does not match the armed entry is ignored too — the
    /// entry stays armed and the timeout path recovers.
    ///
    /// Emits [`Event::group_message_delivery_report`] with the relay's
    /// `delivered`/`pushed` lists and the members re-issued per-member.
    pub fn handle_relay_group_delivery_report(&mut self, report_json: &str) -> Result<()> {
        let report: RelayGroupDeliveryReport = serde_json::from_str(report_json)
            .map_err(|e| Error::Serialization(format!("Parse group delivery report: {}", e)))?;

        let Some(entry) = self
            .group_mesh
            .relay_broadcast_pending
            .get(&report.message_id)
        else {
            debug!(
                group_id = %report.group_id,
                msg_id = %report.message_id,
                "Group delivery report correlates with no in-flight broadcast, ignoring"
            );
            return Ok(());
        };
        if entry.group_id != report.group_id {
            warn!(
                report_group = %report.group_id,
                armed_group = %entry.group_id,
                msg_id = %report.message_id,
                "Group delivery report names a different group than the armed broadcast, ignoring"
            );
            return Ok(());
        }
        let entry = self
            .group_mesh
            .relay_broadcast_pending
            .remove(&report.message_id)
            .expect("checked above");

        for miss in &report.missed {
            debug!(
                group_id = %report.group_id,
                member = %miss.username,
                reason = %miss.reason,
                "Relay reports group member missed"
            );
        }

        // Authoritative roster from MLS, cache as fallback (idiom shared
        // with the relay registration paths).
        let members = self
            .refresh_group_members(&entry.group_id)
            .ok()
            .or_else(|| self.group_mesh.members.get(&entry.group_id).cloned())
            .unwrap_or_default();
        let reached: HashSet<&str> = report
            .delivered
            .iter()
            .chain(report.pushed.iter())
            .map(String::as_str)
            .collect();
        let self_id = self.local_id.clone();
        let unreached: Vec<String> = members
            .iter()
            .filter(|m| m.as_str() != self_id && !reached.contains(m.as_str()))
            .cloned()
            .collect();

        let mut reissued = Vec::new();
        if !unreached.is_empty() {
            info!(
                group_id = %entry.group_id,
                msg_id = %report.message_id,
                count = unreached.len(),
                "Re-sending group message per-member to members the relay did not reach"
            );
            // Never `?` here: the tracker entry is already removed, so
            // propagating would strand the whole contract — no re-issue, no
            // event, and no timeout left to recover it. Report what happened
            // instead and let the app see an empty `missed_reissued`.
            match self.fanout_group_frames_per_member(
                &entry.group_id,
                &unreached,
                Some(&report.message_id),
                &entry.ciphertext_b64,
                entry.epoch,
                entry.reply_to.as_deref(),
                entry.forward_info.clone(),
                entry.priority,
            ) {
                Ok((_ids, succeeded, _failed)) => reissued = succeeded,
                Err(e) => {
                    error!(
                        group_id = %entry.group_id,
                        msg_id = %report.message_id,
                        error = %e,
                        "Failed to re-issue per-member copies for unreached members"
                    );
                }
            }
        }

        self.emit_event(Event::group_message_delivery_report(
            entry.group_id,
            report.message_id,
            report.delivered,
            report.pushed,
            reissued,
        ));
        Ok(())
    }

    /// Returns `true` if the Internet transport is currently available.
    pub(crate) fn is_internet_available(&self) -> bool {
        self.transport_manager
            .get_available_transports()
            .contains_key(&TransportType::Internet)
    }

    /// Reads the current MLS epoch for a group, if available.
    fn read_current_epoch(&self, group_id: &str) -> Option<u64> {
        let guard = self.read_mls_guard().ok()?;
        let gid = offline_protocol_mls::GroupId::new(group_id).ok()?;
        let info = guard.get_group_info(&gid).ok()??;
        Some(info.epoch)
    }

    // ========================================================================
    // EPOCH FORK RESOLUTION
    // ========================================================================

    /// Checks for pending epoch forks and attempts resolution.
    ///
    /// Called from the `process()` tick. After the resolution delay, the
    /// deterministic leader (lex-first member) issues an `update_keys` commit
    /// to re-establish a canonical epoch. Members on the same branch will
    /// process the commit normally. Members on a different branch will fail
    /// to process it, at which point they need a re-invite (the leader
    /// emits `GroupEpochForkResolved` and the application layer can trigger
    /// re-invites for unreachable members).
    ///
    /// ## Stuck fork recovery
    ///
    /// If `update_keys` fails (e.g., the leader's MLS state is corrupted),
    /// `resolution_attempted` is set to `true` and no further automatic
    /// resolution is tried. The fork entry remains until the 5-minute stale
    /// cleanup removes it, at which point the fork may be re-detected if
    /// the underlying issue persists (creating a detect → fail → stale-cleanup
    /// → re-detect cycle). This is intentional: a persistent `update_keys`
    /// failure indicates the group likely needs manual intervention (e.g.,
    /// re-creating the group or re-inviting all members). The application
    /// layer should monitor `GroupEpochForkDetected` events and escalate if
    /// the same group is repeatedly detected.
    pub(crate) fn check_epoch_forks(&mut self) {
        let delay = StdDuration::from_secs(EPOCH_FORK_RESOLUTION_DELAY_SECS);
        let self_id = self.local_id.clone();

        // Collect forks ready for resolution
        let ready: Vec<EpochForkState> = self
            .group_mesh
            .epoch_forks
            .values()
            .filter(|f| !f.resolution_attempted && f.detected_at.elapsed() > delay)
            .cloned()
            .collect();

        for fork in ready {
            // Re-check: if the epoch has advanced since detection, the fork
            // was a delayed commit, not a real fork — cancel it.
            if let Some(detected_epoch) = fork.local_epoch {
                let current_epoch = self.read_current_epoch(&fork.group_id);
                if let Some(current) = current_epoch {
                    if current > detected_epoch {
                        debug!(
                            group_id = %fork.group_id,
                            detected_epoch,
                            current_epoch = current,
                            "Epoch fork auto-cancelled: epoch advanced since detection"
                        );
                        self.group_mesh.epoch_forks.remove(&fork.group_id);
                        continue;
                    }
                }
            }

            // Check if we're the leader for this group
            let members = self
                .refresh_group_members(&fork.group_id)
                .ok()
                .or_else(|| self.group_mesh.members.get(&fork.group_id).cloned())
                .unwrap_or_default();

            let mut sorted = members.clone();
            sorted.sort();
            let am_leader = sorted.first().map(|f| f == &self_id).unwrap_or(false);

            // Mark as attempted regardless — only the leader acts, but all
            // members should stop re-checking.
            if let Some(state) = self.group_mesh.epoch_forks.get_mut(&fork.group_id) {
                state.resolution_attempted = true;
            }

            if !am_leader {
                continue;
            }

            info!(
                group_id = %fork.group_id,
                local_epoch = ?fork.local_epoch,
                "Attempting epoch fork resolution as elected leader"
            );

            // Issue a key update commit to establish a canonical next epoch.
            // Members on our branch will process this normally and advance.
            // Members on a forked branch will fail — the app layer should
            // handle re-inviting them.
            //
            // NOTE: We use `read_mls_guard` (RwLock read lock) even though
            // `update_keys` mutates MLS state. This is correct because
            // `MlsManager` uses interior mutability (internal Mutex on the
            // group store), so a read guard is sufficient for all operations.
            //
            // Acquire MLS lock, perform key update, and fully release the
            // guard before touching any other &mut self state.
            let update_result = {
                if let Ok(guard) = self.read_mls_guard() {
                    let r = offline_protocol_mls::GroupId::new(&fork.group_id)
                        .and_then(|gid| guard.update_keys(&gid));
                    drop(guard);
                    Some(r)
                } else {
                    None
                }
            };
            // Guard is fully dropped here — safe to use &mut self.
            let update_result = match update_result {
                Some(r) => r,
                None => {
                    warn!(group_id = %fork.group_id, "MLS unavailable during epoch fork resolution");
                    if let Some(state) = self.group_mesh.epoch_forks.get_mut(&fork.group_id) {
                        state.resolution_attempted = false;
                    }
                    continue;
                }
            };

            match update_result {
                Ok(commit_msg) => {
                    // Distribute the key-update commit to all members
                    let commit_payload = GroupMlsCommitPayload {
                        group_id: fork.group_id.clone(),
                        commit_type: GroupCommitType::KeyUpdate,
                        ciphertext: base64_encode(&commit_msg.ciphertext),
                        epoch: commit_msg.epoch,
                        affected_member: None,
                        role: None,
                        affected_member_rich: None,
                    };
                    let commit_content = match serde_json::to_string(&commit_payload) {
                        Ok(json) => format!("{}{}", internal_prefixes::GROUP_MLS_COMMIT, json),
                        Err(e) => {
                            warn!(
                                group_id = %fork.group_id,
                                error = %e,
                                "Failed to serialize fork resolution commit"
                            );
                            continue;
                        }
                    };
                    let mut failed_members = Vec::new();
                    for member in &members {
                        if member == &self_id {
                            continue;
                        }
                        if let Err(e) = self.send_internal_message(
                            member,
                            commit_content.clone(),
                            MessagePriority::High,
                        ) {
                            warn!(
                                group_id = %fork.group_id,
                                member = %member,
                                error = %e,
                                "Failed to send fork resolution commit to member"
                            );
                            failed_members.push(member.clone());
                        }
                    }

                    info!(
                        group_id = %fork.group_id,
                        new_epoch = commit_msg.epoch,
                        failed_count = failed_members.len(),
                        "Epoch fork resolution commit distributed"
                    );

                    self.emit_event(Event::group_epoch_fork_resolved(
                        fork.group_id.clone(),
                        commit_msg.epoch,
                        failed_members,
                    ));

                    // Clean up the fork state
                    self.group_mesh.epoch_forks.remove(&fork.group_id);
                }
                Err(e) => {
                    warn!(
                        group_id = %fork.group_id,
                        error = %e,
                        "Failed to issue key update for epoch fork resolution"
                    );
                    // Leave resolution_attempted = true — a persistent failure
                    // means the group may need manual intervention.
                }
            }
        }

        // Clean up stale fork entries (older than 5 minutes)
        let stale_threshold = StdDuration::from_secs(300);
        self.group_mesh
            .epoch_forks
            .retain(|_, f| f.detected_at.elapsed() < stale_threshold);
    }

    // ========================================================================
    // LEAVE ELECTION FALLBACK
    // ========================================================================

    /// Checks for timed-out leave elections and re-elects the next eligible
    /// member to issue the MLS remove-commit.
    ///
    /// Called from the `process()` tick. If the originally elected remover
    /// hasn't issued a commit within `LEAVE_ELECTION_TIMEOUT_SECS`, we check
    /// whether the leaving member is still in the group (via MLS state). If
    /// so, the next eligible member in the sorted remaining list takes over.
    pub(crate) fn check_leave_election_timeouts(&mut self) {
        let timeout = StdDuration::from_secs(LEAVE_ELECTION_TIMEOUT_SECS);
        let max_lifetime = StdDuration::from_secs(LEAVE_ELECTION_MAX_LIFETIME_SECS);
        let attempt_cooldown = StdDuration::from_secs(LEAVE_ELECTION_ATTEMPT_COOLDOWN_SECS);
        let self_id = self.local_id.clone();

        // Collect timed-out elections (keys + values) for processing.
        let timed_out: Vec<((String, String), PendingLeaveElection)> = self
            .group_mesh
            .pending_leave_elections
            .iter()
            .filter(|(_, e)| e.received_at.elapsed() > timeout)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (election_key, election) in timed_out {
            // Circuit breaker: abandon the election after max lifetime to
            // prevent infinite per-tick retry loops when all candidates fail.
            if election.received_at.elapsed() > max_lifetime {
                warn!(
                    group_id = %election.group_id,
                    leaving_member = %election.leaving_member,
                    "Leave election exceeded max lifetime, abandoning — member may need manual removal"
                );
                self.group_mesh
                    .pending_leave_elections
                    .remove(&election_key);
                continue;
            }

            // Refresh membership from MLS (authoritative) to avoid using
            // stale election-time lists where members may have since left.
            let current_members = self
                .refresh_group_members(&election.group_id)
                .ok()
                .or_else(|| self.group_mesh.members.get(&election.group_id).cloned())
                .unwrap_or_default();

            let still_member = current_members
                .iter()
                .any(|id| id == &election.leaving_member);

            if !still_member {
                // Already removed — clean up
                self.group_mesh
                    .pending_leave_elections
                    .remove(&election_key);
                continue;
            }

            // Use current membership for candidate selection, not the stale
            // election-time list — members may have left since the election.
            let mut remaining: Vec<String> = current_members
                .iter()
                .filter(|m| m.as_str() != election.leaving_member)
                .cloned()
                .collect();
            remaining.sort();

            // Re-elect: walk the sorted remaining members. For each timeout
            // interval that has passed, advance one position in the list.
            // This staggers re-election so members don't all fire at once.
            // Interval 1 = first fallback (idx 0), interval 2 = second (idx 1), etc.
            // The original elected member (lex-first) already had their shot
            // during handle_group_mls_leave; this path only fires after timeout.
            let elapsed_intervals =
                (election.received_at.elapsed().as_secs() / LEAVE_ELECTION_TIMEOUT_SECS) as usize;
            let candidate_idx = elapsed_intervals.min(remaining.len().saturating_sub(1));

            if remaining
                .get(candidate_idx)
                .map(|c| c == &self_id)
                .unwrap_or(false)
            {
                // Rate-limit: skip if we already attempted within the cooldown window.
                if let Some(last) = election.last_attempt_at {
                    if last.elapsed() < attempt_cooldown {
                        continue;
                    }
                }

                info!(
                    group_id = %election.group_id,
                    leaving_member = %election.leaving_member,
                    attempt = candidate_idx + 1,
                    "Re-elected to issue MLS remove-commit (prior elected member timed out)"
                );

                // Record the attempt timestamp before trying, so the cooldown
                // applies even on failure.
                if let Some(state) = self
                    .group_mesh
                    .pending_leave_elections
                    .get_mut(&election_key)
                {
                    state.last_attempt_at = Some(Instant::now());
                }

                if let Err(e) = self.remove_from_group(&election.group_id, &election.leaving_member)
                {
                    warn!(
                        group_id = %election.group_id,
                        leaving_member = %election.leaving_member,
                        error = %e,
                        "Failed to issue MLS remove-commit during re-election"
                    );
                    // Don't remove from pending — cooldown will prevent spam,
                    // next interval will advance to the next candidate.
                } else {
                    self.group_mesh
                        .pending_leave_elections
                        .remove(&election_key);
                }
            }
            // If we're not the candidate at this interval, leave the election
            // in place — either the current candidate will handle it, or the
            // next interval will advance to us.
        }
    }

    // ========================================================================
    // TRANSPORT-RESILIENT SYNC
    // ========================================================================

    /// Checks for Internet availability transitions and syncs group state.
    ///
    /// Called from the `process()` tick. On a 0→1 transition (Internet just
    /// became available), all unsynced groups are registered with the relay.
    /// On a 1→0 transition (Internet lost), relay sync flags are cleared so
    /// groups are re-registered when connectivity returns.
    pub(crate) fn check_relay_group_sync(&mut self) {
        if !self.config.group.relay_enabled {
            return;
        }
        let internet_available = self.is_internet_available();
        let was_available = self.group_mesh.internet_was_available;
        self.group_mesh.internet_was_available = internet_available;

        if internet_available && !was_available {
            // Internet just became available — sync all groups
            self.sync_groups_to_relay();
        } else if !internet_available && was_available {
            // Internet just went down — clear sync state. In-flight
            // registration acks can never arrive on this connection, so the
            // pending set goes too (re-armed by the 0→1 re-sync). Each
            // affected group gets a sync-changed event: apps gating
            // relay-dependent commands (invite links) must re-wait for the
            // reconnect's re-registration ack.
            let mut affected: Vec<String> = self.group_mesh.relay_synced.drain().collect();
            for group_id in self.group_mesh.relay_register_pending.drain() {
                if !affected.contains(&group_id.0) {
                    affected.push(group_id.0);
                }
            }
            for group_id in affected {
                self.emit_event(Event::group_relay_sync_changed(
                    group_id,
                    false,
                    "internet_dropped",
                ));
            }
            // Capabilities described the relay on the dead connection; a
            // reconnect re-injects them (possibly from a different relay)
            // before the transport is reported up again.
            self.group_mesh.relay_capabilities.clear();
            // In-flight broadcast delivery reports can never arrive on this
            // connection either. Downgrade each to per-member fan-out now
            // rather than after the report timeout: the copies enter the
            // ordinary outbox/park machinery, which flushes on whatever
            // transport (mesh now, Internet on reconnect) becomes usable —
            // and members the relay did reach absorb them via the logical
            // id.
            let stranded: Vec<(String, RelayBroadcastPending)> =
                self.group_mesh.relay_broadcast_pending.drain().collect();
            for (logical_id, entry) in stranded {
                self.downgrade_broadcast_to_per_member(&logical_id, entry);
            }
        }
    }

    /// Expires outstanding relay registrations the relay never answered.
    ///
    /// Called from the `process()` tick. An entry older than
    /// [`RELAY_REGISTER_ACK_TIMEOUT_SECS`] either gets its registration
    /// re-sent (the frame may have been lost) or, past
    /// [`RELAY_REGISTER_MAX_ATTEMPTS`], is dropped so the ack-acceptance
    /// window closes: against a relay that never answers (prefix-unaware
    /// echo relay, legacy server, or a `__GROUP_ERROR__` that arrived
    /// without a `group_id` and so could not consume the correlation) an
    /// armed entry would otherwise sit indefinitely for a forged
    /// `__GROUP_CREATED__` to claim. Giving up is safe — the group stays
    /// unsynced and sends take the always-correct per-member fan-out path.
    pub(crate) fn process_relay_register_retries(&mut self) {
        if !self.config.group.relay_enabled
            || self.group_mesh.relay_register_pending.is_empty()
            || !self.is_internet_available()
        {
            return;
        }
        let cutoff = Utc::now() - chrono::Duration::seconds(RELAY_REGISTER_ACK_TIMEOUT_SECS);
        let expired: Vec<(String, u32)> = self
            .group_mesh
            .relay_register_pending
            .iter()
            .filter(|(_, pending)| pending.armed_at <= cutoff)
            .map(|(group_id, pending)| (group_id.clone(), pending.attempts))
            .collect();
        for (group_id, attempts) in expired {
            if attempts >= RELAY_REGISTER_MAX_ATTEMPTS
                || !self.group_mesh.members.contains_key(&group_id)
                || self.group_mesh.relay_synced.contains(&group_id)
            {
                self.group_mesh.relay_register_pending.remove(&group_id);
                debug!(
                    group_id = %group_id,
                    attempts,
                    "Expired unanswered relay group registration"
                );
                // Giving up on a still-tracked, unsynced group is an
                // app-visible outcome: without this event a caller awaiting
                // the registration ack (`ensure_group_registered`) only
                // learns via its own timeout. The other two removal causes
                // stay silent — a vanished group already surfaced its
                // teardown, and an already-synced group's stale entry is
                // pure bookkeeping.
                if attempts >= RELAY_REGISTER_MAX_ATTEMPTS
                    && self.group_mesh.members.contains_key(&group_id)
                    && !self.group_mesh.relay_synced.contains(&group_id)
                {
                    self.emit_event(Event::group_relay_sync_changed(
                        group_id.clone(),
                        false,
                        "ack_timeout",
                    ));
                }
                continue;
            }
            // Re-send with membership refreshed from MLS, like the 0→1
            // re-sync; try_relay_register_group re-arms the entry in place
            // with the attempt count carried forward.
            let members = self
                .refresh_group_members(&group_id)
                .ok()
                .or_else(|| self.group_mesh.members.get(&group_id).cloned());
            if let Some(members) = members {
                let _ = self.try_relay_register_group(&group_id, None, &members);
            } else {
                self.group_mesh.relay_register_pending.remove(&group_id);
            }
        }
    }

    /// Expires in-flight relay broadcasts whose settled delivery report
    /// never arrived.
    ///
    /// Called from the `process()` tick. An entry older than
    /// [`RELAY_BROADCAST_REPORT_TIMEOUT_SECS`] gets its broadcast re-sent
    /// under the same logical id (safe: the relay echoes the id, so
    /// receiver and push dedup absorb the duplicate fan-out) while the
    /// attempt budget and the broadcast gate still hold; past
    /// [`RELAY_BROADCAST_MAX_ATTEMPTS`] — or as soon as the gate fails —
    /// the message is downgraded to full per-member fan-out, which needs no
    /// report to be correct.
    pub(crate) fn process_relay_broadcast_report_timeouts(&mut self) {
        if self.group_mesh.relay_broadcast_pending.is_empty() {
            return;
        }
        let cutoff = Utc::now() - chrono::Duration::seconds(RELAY_BROADCAST_REPORT_TIMEOUT_SECS);
        let expired: Vec<String> = self
            .group_mesh
            .relay_broadcast_pending
            .iter()
            .filter(|(_, pending)| pending.armed_at <= cutoff)
            .map(|(logical_id, _)| logical_id.clone())
            .collect();
        for logical_id in expired {
            let Some(mut entry) = self.group_mesh.relay_broadcast_pending.remove(&logical_id)
            else {
                continue;
            };
            let gate_holds = self.config.group.relay_broadcast_enabled
                && self.group_mesh.relay_synced.contains(&entry.group_id)
                && self
                    .group_mesh
                    .relay_capabilities
                    .contains(RELAY_CAP_GROUP_DELIVERY_V3)
                && self.is_internet_available();
            if entry.attempts < RELAY_BROADCAST_MAX_ATTEMPTS && gate_holds {
                let payload = RelayGroupBroadcastPayload {
                    group_id: entry.group_id.clone(),
                    ciphertext: entry.ciphertext_b64.clone(),
                    epoch: entry.epoch,
                    reply_to: entry.reply_to.clone(),
                    forward_info: entry.forward_info.clone(),
                    message_id: logical_id.clone(),
                };
                if self.send_relay_broadcast_frame(&payload).is_ok() {
                    warn!(
                        group_id = %entry.group_id,
                        msg_id = %logical_id,
                        attempts = entry.attempts + 1,
                        "Group broadcast delivery report lost, re-sent broadcast"
                    );
                    entry.attempts += 1;
                    entry.armed_at = Utc::now();
                    self.group_mesh
                        .relay_broadcast_pending
                        .insert(logical_id, entry);
                    continue;
                }
                // Re-send failed — fall through to per-member downgrade.
            }
            self.downgrade_broadcast_to_per_member(&logical_id, entry);
        }
    }

    /// Downgrades one in-flight broadcast to full per-member fan-out (every
    /// roster member; receivers that did get the relay's copy absorb the
    /// duplicate via the logical id). Terminal for the broadcast attempt:
    /// the per-member copies own delivery from here through the ordinary
    /// outbox/ACK/park ladder.
    fn downgrade_broadcast_to_per_member(
        &mut self,
        logical_id: &str,
        entry: RelayBroadcastPending,
    ) {
        let members = self
            .refresh_group_members(&entry.group_id)
            .ok()
            .or_else(|| self.group_mesh.members.get(&entry.group_id).cloned())
            .unwrap_or_default();
        info!(
            group_id = %entry.group_id,
            msg_id = %logical_id,
            members = members.len(),
            "Downgrading relay group broadcast to per-member fan-out"
        );
        if let Err(e) = self.fanout_group_frames_per_member(
            &entry.group_id,
            &members,
            Some(logical_id),
            &entry.ciphertext_b64,
            entry.epoch,
            entry.reply_to.as_deref(),
            entry.forward_info.clone(),
            entry.priority,
        ) {
            error!(
                group_id = %entry.group_id,
                msg_id = %logical_id,
                error = %e,
                "Failed to downgrade group broadcast to per-member fan-out"
            );
        }
    }

    /// Re-registers all unsynced groups with the relay server.
    ///
    /// Refreshes each group's member list from MLS state before syncing so
    /// that the relay receives authoritative membership, not a stale cache.
    fn sync_groups_to_relay(&mut self) {
        let group_ids: Vec<String> = self.group_mesh.members.keys().cloned().collect();
        for group_id in group_ids {
            if self.group_mesh.relay_synced.contains(&group_id) {
                continue;
            }
            // Refresh from MLS to avoid syncing stale membership to relay
            let members = self
                .refresh_group_members(&group_id)
                .ok()
                .or_else(|| self.group_mesh.members.get(&group_id).cloned());
            if let Some(members) = members {
                let _ = self.try_relay_register_group(&group_id, None, &members);
            }
        }
    }

    // ========================================================================
    // RELAY INBOUND — MLS-AWARE ROUTING
    // ========================================================================

    /// Returns `true` if we hold local MLS state for `group_id`.
    ///
    /// The raw-emit fallbacks in [`Self::handle_relay_group_message_with_mls`]
    /// attribute unauthenticated plaintext to a caller-supplied `sender`. That
    /// is only ever acceptable for a genuine legacy relay-only (unencrypted)
    /// group, which has no MLS state. If we secure this group with MLS, a real
    /// member always sends MLS ciphertext, so plaintext naming the group is a
    /// spoof — this test lets the caller drop it rather than surface a message
    /// forged in a trusted member's name.
    ///
    /// Fail-closed: when MLS is initialized but the lookup cannot complete
    /// (lock poisoned, invalid id), assume state may exist and return `true`
    /// so a transient fault can never be leveraged to force a plaintext spoof
    /// through. When MLS is not initialized at all, there is no group to
    /// secure, so return `false` (genuine legacy).
    fn has_mls_group_state(&self, group_id: &str) -> bool {
        if !self.is_mls_initialized() {
            return false;
        }
        let Ok(gid) = offline_protocol_mls::GroupId::new(group_id) else {
            // An id we cannot even construct names no real MLS group.
            return false;
        };
        match self.read_mls_guard() {
            Ok(guard) => guard.has_group(&gid).unwrap_or(true),
            Err(_) => true,
        }
    }

    /// Handles an inbound relay group message by routing through MLS
    /// decryption.
    ///
    /// Called from `process_internal_message` for every relay `__GROUP_MSG__`
    /// when MLS is initialized (or when the group already has local MLS
    /// state) — including groups we have not joined yet, because a relay
    /// group message can outrun its Welcome exactly like a mesh one; such
    /// messages are buffered for deferred retry. Content that is not MLS
    /// ciphertext (base64-undecodable, or base64 that is not MLS wire
    /// framing for a group without local state) is a legacy relay-only
    /// group message and is emitted raw — but only for a group we do *not*
    /// secure with MLS. Plaintext naming a group we hold MLS state for is a
    /// sender-spoofing attempt (a real member sends ciphertext) and is
    /// dropped, since the raw emit would attribute attacker content to a
    /// trusted member with no authentication.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn handle_relay_group_message_with_mls(
        &mut self,
        group_id: &str,
        sender: &str,
        content: &str,
        timestamp: &str,
        message_id: &str,
        reply_to_msg: Option<String>,
        forward_info: Option<offline_protocol_core::ForwardInfo>,
    ) {
        // Dedup check — same pattern as handle_group_mls_msg, with one
        // deliberate difference. The relay-supplied id IS the logical id on
        // this path, and it is marked here, pre-decrypt, as the replay-
        // amplification defense (one MLS crypto op per id). But the mesh
        // handler's marking discipline exists for a reason this path is not
        // exempt from: a marked id suppresses every later copy of the same
        // logical message, including the per-member re-issue that is this
        // message's own delivery safety net. So every arm below that ends
        // with the frame neither delivered, nor buffered, nor consumed by
        // MLS must unmark before returning — otherwise a copy that was
        // *rejected* (e.g. mis-attributed by the relay) reads as *delivered*
        // to the duplicate check, and the genuine copy that follows is
        // absorbed and re-ACKed without ever being surfaced: silent loss.
        let dedup_key = message_id.to_string();
        if self.group_mesh.message_dedup.contains_key(&dedup_key) {
            debug!(group_id = %group_id, msg_id = %dedup_key, "Duplicate relay group message, skipping");
            return;
        }
        self.group_mesh
            .message_dedup
            .insert(dedup_key.clone(), Instant::now());
        if self.group_mesh.message_dedup.len() > MAX_GROUP_MESSAGE_DEDUP_ENTRIES {
            self.cleanup_group_message_dedup();
        }

        let forward_info_event = forward_info
            .as_ref()
            .map(crate::events::ForwardInfoEvent::from);

        // Attempt base64 decode — if it fails, the content is plaintext (legacy)
        let ciphertext_bytes = match base64_decode(content) {
            Ok(bytes) => bytes,
            Err(_) => {
                // Plaintext content naming a group we secure with MLS cannot be
                // legitimate: a real member always sends MLS ciphertext (which
                // is base64 and decodes here). Emitting it raw would attribute
                // attacker-chosen content to the unauthenticated inner `sender`
                // — a message forged in a trusted member's name. Drop it. Only
                // a genuine legacy relay-only group (no local MLS state) may be
                // emitted as plaintext.
                if self.has_mls_group_state(group_id) {
                    warn!(
                        group_id = %group_id,
                        sender = %sender,
                        "SECURITY: dropping non-MLS plaintext naming an MLS-secured group (sender spoofing)"
                    );
                    self.emit_event(Event::security_warning(
                        sender.to_string(),
                        crate::events::SecurityWarningCode::PlaintextReceiveRejected,
                        "Plaintext group message rejected: the named group is secured with MLS, \
                         so unauthenticated plaintext cannot be attributed to the claimed sender"
                            .to_string(),
                    ));
                    // Nothing was delivered and no MLS state was touched: the
                    // id on this frame is attacker-chosen wire input, and
                    // leaving it marked would let a spoofed frame suppress the
                    // genuine logical message it names (see the marking note
                    // at the top of this function).
                    self.group_mesh.message_dedup.remove(&dedup_key);
                    return;
                }
                // Not ciphertext and no MLS state — legacy relay-only group.
                self.emit_event(Event::group_message_received(
                    group_id.to_string(),
                    sender.to_string(),
                    content.to_string(),
                    timestamp.to_string(),
                    message_id.to_string(),
                    reply_to_msg,
                    forward_info_event,
                    None,
                    None,
                ));
                return;
            }
        };

        match self.decrypt_group_application(group_id, ciphertext_bytes, sender) {
            GroupDecryptOutcome::Plaintext(pt) => match String::from_utf8(pt) {
                Ok(text) => {
                    let (content, media_metadata, content_type, forward_info_event) =
                        Self::restore_group_rich(text, forward_info, sender);
                    self.emit_event(Event::group_message_received(
                        group_id.to_string(),
                        sender.to_string(),
                        content,
                        timestamp.to_string(),
                        message_id.to_string(),
                        reply_to_msg,
                        forward_info_event,
                        media_metadata,
                        content_type,
                    ));
                }
                Err(_) => {
                    warn!(
                        group_id = %group_id,
                        "Decrypted relay group payload is not valid UTF-8, dropping"
                    );
                }
            },
            GroupDecryptOutcome::Retriable => {
                self.buffer_pending_group_message(
                    group_id,
                    PendingGroupMessage {
                        sender: sender.to_string(),
                        message_id: message_id.to_string(),
                        // The relay-supplied id IS the logical id on this
                        // path (a v2 relay echoes the broadcast's id), so a
                        // separate copy would be redundant.
                        logical_id: None,
                        ciphertext_b64: content.to_string(),
                        timestamp: Some(timestamp.to_string()),
                        reply_to: reply_to_msg,
                        forward_info,
                        buffered_at: Instant::now(),
                        // Relay path: the relay sender is not ACK-gated (it uses
                        // try_relay_broadcast, not per-member ensure_ack_registration),
                        // so there is no deferred ACK to send on drain.
                        received_via: None,
                    },
                );
            }
            GroupDecryptOutcome::NonApplication => {
                // Same as the mesh path: MLS consumed a commit or proposal
                // riding the message channel — group state may have advanced.
                self.drain_pending_commits(group_id);
                self.drain_pending_group_messages(group_id);
            }
            GroupDecryptOutcome::PolicyRejected {
                committer,
                added,
                removed,
            } => {
                // Same as the mesh path: a commit reframed onto the message
                // channel, refused before merging. Dropped, never buffered.
                self.report_unauthorized_membership_change(
                    group_id,
                    &committer,
                    &added,
                    &removed,
                    "sender_not_admin",
                    true,
                );
            }
            GroupDecryptOutcome::NotMlsCiphertext => {
                // Valid base64 that is not MLS framing, for a group without
                // local MLS state: a legacy relay-only group message whose
                // plaintext happens to decode (e.g. a base64 media blob).
                // Emit it raw, exactly like the base64-undecodable path.
                self.emit_event(Event::group_message_received(
                    group_id.to_string(),
                    sender.to_string(),
                    content.to_string(),
                    timestamp.to_string(),
                    message_id.to_string(),
                    reply_to_msg,
                    forward_info_event,
                    None,
                    None,
                ));
            }
            GroupDecryptOutcome::SecurityRejected | GroupDecryptOutcome::Failed => {
                warn!(
                    group_id = %group_id,
                    "Failed to decrypt relay group message via MLS, dropping"
                );
                // Rejected, not delivered — unmark so a later copy of the
                // same logical message is processed instead of absorbed.
                // `SecurityRejected` is the load-bearing case: a v2 relay
                // attributes group frames by username, which fails the
                // SEC-M1 wire-sender/credential match after the decrypt —
                // leaving the id marked would then swallow the per-member
                // re-issue of the very same message (delivered exactly
                // nowhere, ACKed as a duplicate). `Failed` covers pre-decrypt
                // refusals (MLS unavailable, invalid group id) that a later
                // copy may outlive. The cost of unmarking is one MLS crypto
                // op per replayed copy — bounded by the same reasoning as
                // the mesh handler's post-decrypt-only logical-id marking.
                self.group_mesh.message_dedup.remove(&dedup_key);
            }
        }
    }
}
