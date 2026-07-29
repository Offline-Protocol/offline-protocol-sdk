//! Storage persistence methods for protocol state.

use super::state_crypto::{StateRecordCipher, SEALED_RECORD_OVERHEAD, STATE_RECORD_KEY_BYTES};
use super::{
    lifetime_expired, storage_keys, MediaTransferDescriptor, OfflineProtocol, OutboxEntry,
    PeerCapabilities, PendingMessage, PendingMessageRecord, ReceivedKeyPackage, SessionState,
    WelcomeDeliveryState, WelcomeLifecycleRecord, MAX_KEY_PACKAGE_SENT_TO,
    MAX_PENDING_KEY_PACKAGES, MAX_PENDING_MESSAGES_GLOBAL, MAX_PENDING_MESSAGES_PER_PEER,
    MAX_PENDING_MESSAGE_BYTES_GLOBAL, MAX_PENDING_MESSAGE_BYTES_PER_PEER,
    MAX_PERSISTED_CAPABILITY_VERSIONS, MAX_PROTOCOL_STATE_RECORD_BYTES, MLS_ENVELOPE_COMPACT_V1,
    RICH_PAYLOAD_V1, WELCOME_LIFECYCLE_TTL_SECS,
};
use crate::constants::{MAX_MEDIA_DESCRIPTORS, MAX_OUTBOX_ENTRIES};
use crate::{Error, Event, ProtocolStateError, ProtocolStateResult, ProtocolStateStorage, Result};
use chrono::{Duration as ChronoDuration, Utc};
use offline_protocol_core::{LamportClock, MessageId};
use offline_protocol_mls::{MlsManager, MlsStorage};
use offline_protocol_transport::{NostrKeypair, NostrTransport, TransportType};
use serde::de::DeserializeOwned;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

/// Every protocol-state category the SDK writes through
/// [`OfflineProtocol::write_state_record`].
///
/// The enum exists for [`Self::requires_sealing`], which is an exhaustive
/// `match`: adding a variant is a compile error until someone decides whether
/// that category's values are sensitive. The previous spelling was a `matches!`
/// over `&str`, which silently answers "not sensitive" for a category nobody
/// remembered to list — and that answer means message plaintext or cloud-media
/// key material written to the app container in the clear, which is the one
/// thing this module exists to prevent.
///
/// [`Self::from_key_type`] still has to cope with an unrecognised string,
/// because the storage API is keyed by `&str`. It answers `None`, and
/// `write_state_record` refuses to write that: a category the sealing decision
/// does not cover fails closed and loudly rather than open and quietly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StateCategory {
    /// The legacy per-recipient pending layout. Read-only now — restore
    /// migrates it into [`Self::PendingMessageEntries`] — but still sealed,
    /// because reading it means decrypting it.
    PendingMessages,
    PendingMessageEntries,
    Outbox,
    MediaDescriptors,
    PeerKeyPackages,
    PeerCapabilities,
    SessionStates,
    WelcomeLifecycles,
    BlockedUsers,
    BothCreateAwaitingDecrypt,
    LamportClock,
    /// The value-less marker recording that the pre-split adoption sweep
    /// completed. Post-split only, so it is deliberately absent from
    /// [`storage_keys::ADOPTABLE_STATE_KEY_TYPES`] — but it *is* written to
    /// protocol-state storage, so it belongs to the sealing decision like
    /// every other category.
    StateAdoption,
}

impl StateCategory {
    /// The category a storage key type names, or `None` for a key type the
    /// sealing decision below does not cover.
    pub(crate) fn from_key_type(key_type: &str) -> Option<Self> {
        Some(match key_type {
            storage_keys::PENDING_MESSAGES => Self::PendingMessages,
            storage_keys::PENDING_MESSAGE_ENTRIES => Self::PendingMessageEntries,
            storage_keys::OUTBOX => Self::Outbox,
            storage_keys::MEDIA_DESCRIPTORS => Self::MediaDescriptors,
            storage_keys::PEER_KEY_PACKAGES => Self::PeerKeyPackages,
            storage_keys::PEER_CAPABILITIES => Self::PeerCapabilities,
            storage_keys::SESSION_STATES => Self::SessionStates,
            storage_keys::WELCOME_LIFECYCLES => Self::WelcomeLifecycles,
            storage_keys::BLOCKED_USERS => Self::BlockedUsers,
            storage_keys::BOTH_CREATE_AWAITING_DECRYPT => Self::BothCreateAwaitingDecrypt,
            storage_keys::LAMPORT_CLOCK => Self::LamportClock,
            storage_keys::STATE_ADOPTION => Self::StateAdoption,
            _ => return None,
        })
    }

    /// Whether this category's values must be sealed before they reach the
    /// install-scoped store.
    ///
    /// The single place record sensitivity is decided — adding a category means
    /// deciding here, not at each call site. Sealed categories are the ones
    /// whose values can carry message plaintext, user content, or media key
    /// material:
    ///
    /// - [`storage_keys::PENDING_MESSAGE_ENTRIES`] and its legacy
    ///   per-recipient predecessor [`storage_keys::PENDING_MESSAGES`]: original
    ///   plaintext, plus rich extras that can include
    ///   `MediaMetadata::encryption_key`/`iv`.
    /// - [`storage_keys::OUTBOX`]: the outgoing `Message` — ciphertext for
    ///   encrypted sends, but plaintext when the app opted out of encryption,
    ///   and its outer `media_metadata` carries the cloud-media secrets on the
    ///   forward path.
    /// - [`storage_keys::MEDIA_DESCRIPTORS`]: file names and recipients of
    ///   in-flight transfers.
    ///
    /// Everything else is public wire material (key packages, advertised
    /// capability versions), a small state enum, a logical clock, or a
    /// value-less marker whose only information is the key id — which sealing
    /// cannot hide anyway, because the store addresses records by it.
    fn requires_sealing(self) -> bool {
        match self {
            Self::PendingMessages
            | Self::PendingMessageEntries
            | Self::Outbox
            | Self::MediaDescriptors => true,
            Self::PeerKeyPackages
            | Self::PeerCapabilities
            | Self::SessionStates
            | Self::WelcomeLifecycles
            | Self::BlockedUsers
            | Self::BothCreateAwaitingDecrypt
            | Self::LamportClock
            | Self::StateAdoption => false,
        }
    }
}

/// Whether a key type's values must be sealed. Unknown key types answer `false`
/// only for *reads*, where nothing this SDK wrote can be waiting — the write
/// side refuses them outright.
fn record_requires_sealing(key_type: &str) -> bool {
    StateCategory::from_key_type(key_type).is_some_and(StateCategory::requires_sealing)
}

/// Outcome of reading one protocol-state record.
///
/// Three states, not two, because "no bytes" has three very different causes
/// and only one of them is a loss:
///
/// - [`Self::Missing`] — nothing was ever here.
/// - [`Self::Unreadable`] — a record *was* here, was examined, and is gone for
///   good (oversized, tampered, or sealed under a key this install no longer
///   has). It has been deleted.
/// - [`Self::Unavailable`] — a record is still here and may be perfectly good,
///   but cannot be read *this session* (the record key is not loaded, or the
///   backend refused the read). It is left on disk and a later launch may
///   recover it.
///
/// Restore paths that own app-visible message state settle only
/// [`Self::Unreadable`]. Settling [`Self::Unavailable`] would be wrong twice
/// over: the record survives, so the next launch restores it and re-drives
/// delivery — the app would have been told the message failed terminally and
/// then have it delivered anyway, with a hand re-send (a *new* id, so dedup
/// cannot collapse it) landing as a second copy.
pub(crate) enum StateRecord {
    /// No record under this key.
    Missing,
    /// A record existed, could not be returned, and has been deleted — or,
    /// when the reader was given a [`PruneBudget`] that has run out, left for a
    /// later launch to delete. Nothing is ever recovered from it either way, so
    /// callers treat the two identically; the budget only defers the unlink.
    Unreadable,
    /// A record exists but could not be read this session; it is still on disk.
    Unavailable,
    /// The record's plaintext value.
    Present(Vec<u8>),
}

impl StateRecord {
    /// The record's bytes, treating an unrecoverable record like an absent one.
    /// For callers whose category carries no app-visible commitment.
    fn into_bytes(self) -> Option<Vec<u8>> {
        match self {
            Self::Present(data) => Some(data),
            Self::Missing | Self::Unreadable | Self::Unavailable => None,
        }
    }
}

/// Outcome of restoring one recipient's pending queue. Mirrors
/// [`StateRecord`]'s three-way split for the same reason.
enum PendingRestore {
    /// Nothing was queued for this recipient.
    Absent,
    /// A queue existed but could not be recovered. Its message ids are inside
    /// the record that would not open, so they cannot be settled individually.
    Lost,
    /// A queue exists but could not be read this session and is still on disk.
    /// Reporting it as lost would be a lie a later launch contradicts.
    Unavailable,
    /// The recovered queue (possibly empty).
    Restored(Vec<PendingMessage>),
}

/// Outcome of a restore-path read of one JSON record
/// ([`OfflineProtocol::load_restorable_state_record`]). Mirrors
/// [`StateRecord`]'s split for the same reason, collapsing only the two states
/// a restore treats identically.
///
/// [`Self::Absent`] and [`Self::Unavailable`] must stay apart even though both
/// yield no value. A caller that re-bootstraps on absence — which is exactly
/// what `restore_session_states_from_manager` does — would otherwise persist a
/// fresh `Pending` over a record that is still on disk and may say `Confirmed`.
pub(crate) enum RestorableRecord<T> {
    /// The decoded value.
    Present(T),
    /// Nothing was ever here, or a record was examined and dropped. Either way
    /// the category holds no value for this key and re-bootstrapping one is
    /// safe.
    Absent,
    /// A record may well be here and could not be read *this session*. It is
    /// left on disk, so the caller must neither treat it as absent nor write
    /// over it; a later launch can still recover it.
    Unavailable,
}

/// Largest number of keys any single pass will process for a category with no
/// live insert-time cap of its own.
///
/// The pending queue is the widest such category, at
/// [`MAX_PENDING_MESSAGES_GLOBAL`] recipients, so a store listing more than a
/// generous multiple of that has been tampered with or was written by a build
/// with wildly different bounds. That threat matters more here than it did
/// before the storage split, not less: this state used to live in the
/// credential store and now lives in the app container, where write access is
/// easier to come by. Sealing already keeps a forged record from ever
/// deserializing; this bounds the *work* as well as the result.
///
/// Every walk stops here; what happens to the tail depends on the category:
///
/// - Most **restores** simply *ignore* it — they do not delete it. A later
///   launch still reaches it, but *not* because the listing order varies: all
///   three built-in providers return their keys sorted, so the walked prefix is
///   deterministic. It drains because the prefix is **consumed** — restored
///   entries are flushed, expired, or evicted for capacity, and their records
///   deleted — so each launch lists fewer keys and the walk reaches further.
///   The categories restore never consumes ([`storage_keys::BLOCKED_USERS`],
///   [`storage_keys::BOTH_CREATE_AWAITING_DECRYPT`],
///   [`storage_keys::WELCOME_LIFECYCLES`]) therefore keep the *same* tail on
///   every launch, indefinitely. That is tolerable only because this bound sits
///   far above anything this SDK can legitimately write, so reaching it already
///   means the store was tampered with — do not carry the "a later launch gets
///   the rest" argument over to a tighter bound.
/// - The two **cache** categories (`restore_peer_key_packages`,
///   `restore_peer_capabilities`) prune the overflow *inside* the prefix down
///   to their own much smaller live caps. Losing a cached key package or
///   capability record only costs a recoverable re-exchange, which is what
///   makes deleting the overflow the right policy there. How much they prune
///   per launch is bounded separately by [`MAX_RESTORE_PRUNE_DELETES`],
///   because this bound limits *reads* and a prune is a synchronous provider
///   delete — so the store shrinks by that budget per launch and the tail
///   drains over successive launches.
/// - **Adoption** treats hitting this bound as an incomplete pass instead: it
///   deletes each record it moves, so withholding its completion marker drains
///   the remainder over successive launches rather than abandoning it in the
///   credential store (see `adopt_legacy_protocol_state`).
///
/// Categories whose live insert path is capped get a tighter bound derived from
/// that cap instead (see [`OUTBOX_RESTORE_KEY_CAP`]).
///
/// `restore_tofu_keys` uses this bound too, even though TOFU pins never left
/// the credential store: the argument for bounding *work* on the boot path does
/// not depend on which store is easier to write to. It is the one category that
/// bounds its walk without ever pruning the tail — see the rationale there.
pub(super) const MAX_RESTORE_KEYS_PER_CATEGORY: usize = 4 * MAX_PENDING_MESSAGES_GLOBAL;

/// Restore-walk bound for the outbox.
///
/// The live insert path hard-caps the outbox at [`MAX_OUTBOX_ENTRIES`]
/// (`ensure_outbox_entry` evicts before admitting), so a durable store can
/// never legitimately hold more than that plus what a merge restored. Four
/// times the cap is ample headroom and avoids walking — and transiently
/// allocating up to [`MAX_PROTOCOL_STATE_RECORD_BYTES`] for — thirty times more
/// entries than could ever be kept.
///
/// Media transfer descriptors deliberately keep the wider bound: unlike the
/// outbox, `persist_media_descriptor` has no insert-time cap
/// ([`MAX_MEDIA_DESCRIPTORS`] is applied only on restore), so a long-running
/// session can legitimately leave more on disk than the restore cap. Walking
/// only a tight prefix would keep the wrong ones and strand the rest forever.
const OUTBOX_RESTORE_KEY_CAP: usize = 4 * MAX_OUTBOX_ENTRIES;

/// Restore-walk bound for the pending queue, counted in *entries* rather than
/// records.
///
/// [`MAX_RESTORE_KEYS_PER_CATEGORY`] bounds how many pending-queue *records*
/// one pass opens, and each record holds a whole recipient's queue — up to
/// [`MAX_PENDING_MESSAGES_PER_PEER`] entries after the trim. Bounding only the
/// record count therefore admits `16384 × 64` entries, and holding the global
/// caps across them is not free: `evict_oldest_pending_message` scans the whole
/// in-memory queue to find the oldest entry, so every entry past
/// [`MAX_PENDING_MESSAGES_GLOBAL`] costs an O(`MAX_PENDING_MESSAGES_GLOBAL`)
/// pass. That is roughly four billion comparisons on the *synchronous* boot
/// path, before counting one provider load per record.
///
/// This is the same lesson the built-in providers' `list_keys` bound learned:
/// count the work, not the results. And it is reachable without tampering — the
/// pre-split build had no pending-queue caps at all, so an upgraded install can
/// legitimately hold far more than the caps now admit, and the adoption sweep
/// moves all of it into the store this walk reads.
///
/// The tail is *ignored*, never pruned, exactly like the record-count tail
/// above it: those ids live inside records nothing has opened, so deleting them
/// would be an unsettleable loss. Sized at the same generous multiple of the
/// live cap the rest of this module uses, so no store this SDK wrote is ever
/// truncated.
///
/// The tail needs no freeze, and once did. Under the per-recipient layout a
/// record held a whole queue, so an ordinary enqueue for an unwalked recipient
/// wrote the in-memory view straight over it and the bounds' promise to leave
/// the tail "for a later launch" had to be enforced for the whole session.
/// Pending records are now keyed per message id, so a later write only ever
/// touches a different key — the same reason the outbox and media-descriptor
/// tails never needed one.
///
/// Both passes of the walk share this bound, and only the legacy one can still
/// open many entries per record; for per-message records it is simply the record
/// count.
pub(super) const MAX_PENDING_RESTORE_ENTRIES: usize = 4 * MAX_PENDING_MESSAGES_GLOBAL;

/// Durable deletes one *pool* may fund across a launch.
///
/// [`MAX_RESTORE_KEYS_PER_CATEGORY`] bounds how many records a walk *reads*.
/// It does not bound how many it *deletes*, and those are not the same cost: a
/// delete is a synchronous provider round trip that flushes the containing
/// directory on all three built-in providers — `F_FULLFSYNC` on iOS, which is a
/// full device barrier rather than a hint. Pruning every over-cap record in one
/// pass therefore costs, in the worst case, tens of thousands of device
/// barriers on the *synchronous* `initialize_mls` path: not a slow launch, a
/// launch the platform watchdog kills.
///
/// So the walk bound and the prune bound are separate numbers, because they
/// bound separate resources. The same lesson as the providers' `list_keys`
/// bound and [`MAX_PENDING_RESTORE_ENTRIES`], applied to the write side.
///
/// Safe to stop early precisely because pruning is *idempotent and resumable*:
/// an over-cap record left on disk is walked again next launch and pruned then,
/// exactly as `adopt_legacy_protocol_state` drains its own truncated pass.
///
/// # The bound is on the launch, so the allowance is a pool
///
/// The hazard above is a property of the whole `initialize_mls` call, not of
/// any one walk in it. An earlier spelling gave *each* walk a private
/// allowance of this size, so six walks cost six times the bound while every
/// one of them truthfully reported staying inside it — the same failure this
/// constant exists to prevent, one level up. Walks therefore draw from a
/// [`PruneAllowance`] pool, and the launch ceiling is the sum of the pools
/// rather than the sum of the walks.
///
/// # Two ways to spend it, because two kinds of walk owe the application
/// different things
///
/// A **cache or advisory** walk ([`PruneAllowance::refusing`]) may simply
/// refuse a delete once its share is gone: dropping a session-state record, a
/// cached key package, a capability record, a Welcome lifecycle, or a media
/// descriptor costs a recoverable re-exchange or a re-bootstrap, so the record
/// is left on disk and nothing is owed to anyone. All [`ADVISORY_PRUNE_WALKS`]
/// of them therefore share **one** pool, created by `initialize_mls` and
/// threaded through them, so their deletes add up against a single allowance.
///
/// Sharing a pool is not the same as sharing it *fairly*, and the difference
/// matters because the walks draw in a fixed order. Each therefore leaves the
/// walks after it a [`MIN_ADVISORY_PRUNE_DELETES`] floor rather than being free
/// to empty the pool — without which the first walk alone could starve the rest
/// on every launch. Being *starved* is not the same as being *deferred*: a
/// deferred record is re-walked next launch, while a walk that never draws
/// again defers nothing, and these prunes are the only thing that ever deletes
/// those records. Within that floor the allowance stays elastic: a walk may
/// take everything except what it owes the ones still to come, so a launch with
/// clean early categories still lets a later one draw deeply.
///
/// A **settlement-paired** walk ([`PruneAllowance::counting`]) cannot refuse.
/// Refusing an individual delete there would either settle an id whose record a
/// later launch restores and re-drives — the exact contradiction
/// [`StateRecord`] exists to prevent — or drop an entry from memory while
/// leaving the app holding an id nothing resolves. So it *counts* what it
/// spends, never refuses a delete mid-record, and stops at the next **record
/// boundary**, where stopping is safe because the untouched remainder is
/// exactly what a later launch reads.
/// Both [`OfflineProtocol::restore_pending_messages`] and
/// [`OfflineProtocol::restore_outbox`] spend it that way, and neither needs to
/// freeze the tail it stops at: both categories are keyed per message id, so a
/// later write only ever touches a different key. The pending walk did need a
/// freeze while its records were keyed per recipient and held a whole queue —
/// see [`MAX_PENDING_RESTORE_ENTRIES`].
///
/// The two settlement-paired walks get a pool **each**, rather than sharing the
/// advisory one. Starving an advisory walk defers a cache eviction; starving
/// the outbox walk defers *delivery* of every message the app is still holding
/// a live id for, and starving the pending walk defers every diagnostic it
/// owes. Neither may be held hostage to a key-package flood.
///
/// # The derived launch ceiling
///
/// Three pools, so `3 × MAX_RESTORE_PRUNE_DELETES` is the whole launch's
/// allowance. All three are constructed side by side by `initialize_mls` — see
/// [`PruneAllowance::pool`] for why none of them may be allocated inside the
/// walk that spends it — and
/// `test_one_launch_cannot_exceed_the_derived_restore_delete_ceiling` pins the
/// total against a provider that counts deletes, which is the invariant a
/// per-walk regression breaks and a per-pool test does not see.
///
/// The ceiling covers every durable delete on the path, not only the ones a
/// walk issues while it reads. [`OfflineProtocol::restore_outbox`]'s two
/// post-walk prunes — the absolute-lifetime drop and the capacity drain — act
/// on entries the walk already *admitted*, so a store whose records all open
/// cleanly reaches them with the pool untouched and its working set bounded
/// only by [`OUTBOX_RESTORE_KEY_CAP`]. Left ungated they cost up to
/// `OUTBOX_RESTORE_KEY_CAP - MAX_OUTBOX_ENTRIES` deletes in one launch — the
/// ordinary over-capacity case rather than the tampered one, and the figure
/// that walk's original budget exemption was argued from. Both are
/// settlement-paired, so neither may refuse an individual delete; both stop
/// *between* entries instead, and an entry the pool cannot fund is dropped from
/// memory and left on disk **unsettled**, so a later launch owns both halves.
/// `test_outbox_capacity_prune_stays_inside_the_launch_budget` and
/// `test_outbox_absolute_expiry_prune_stays_inside_the_launch_budget` pin them.
///
/// Two caveats remain, because neither can be budgeted away.
///
/// A settlement-paired walk stops *between* records, so the record it is
/// already inside may push a little past its pool.
///
/// And the ceiling is over the restore walks, not over every durable delete
/// `initialize_mls` can cause. `adopt_legacy_protocol_state` runs before them
/// and deletes from the *secure* store as it moves each record across, bounded
/// by its own truncated-and-resumable pass rather than by this constant. It is
/// a one-time upgrade sweep on a different provider, so it is deliberately
/// outside the pools — but a reader deriving "the most barriers one launch can
/// issue" should count it separately rather than reading `3 ×` as the total.
///
/// # This bounds deletes, and deletes are not the only durable cost
///
/// A *write* flushes the record **and** its directory, so it is strictly more
/// expensive than a delete. Two write paths on the restore walk are unbounded
/// on purpose, for the same reason `restore_outbox`'s deletes are: each is
/// paired with a settlement that cannot be separated from it.
///
/// - `restore_pending_messages` re-persists every recipient whose queue lost
///   entries to a capacity cap. Skipping one would leave its durable record
///   listing messages already settled as `message_failed`, which the next launch
///   would restore and deliver.
/// - `restore_outbox` re-persists refreshed and orphaned entries for the same
///   reason.
///
/// Both are bounded transitively — by [`MAX_PENDING_RESTORE_ENTRIES`] and
/// [`OUTBOX_RESTORE_KEY_CAP`] respectively — not by this constant. Tightening
/// the write volume means tightening those, not budgeting here.
pub(super) const MAX_RESTORE_PRUNE_DELETES: usize = 512;

/// Everything one pending-queue restore walk accumulates across recipients.
///
/// The running totals are maintained rather than recomputed: the global caps
/// would otherwise re-walk the whole in-memory queue once per eviction, and the
/// walk can admit up to [`MAX_PENDING_RESTORE_ENTRIES`] entries.
///
/// They start at zero rather than from the in-memory queue because that queue is
/// necessarily empty at this point: `queue_message_for_session_establishment` is
/// gated on `should_auto_encrypt()`, which requires an MLS manager, and
/// `initialize_mls` returns early once one is published — so nothing can have
/// been queued before the restore that publishes it.
#[derive(Default)]
struct RestoredPendingAdmission {
    /// Entries admitted into `pending_encrypted_messages` so far.
    global_count: usize,
    /// Their total serialized footprint.
    global_bytes: usize,
    /// Ids dropped by a count or byte cap, to settle as failed and delete.
    capacity_evicted: Vec<MessageId>,
}

/// What the two pending-restore passes accumulate before the caps are applied.
///
/// The passes read two different layouts and both feed one grouped view, so
/// this exists to keep them from having to know about each other beyond the
/// `seen` set.
#[derive(Default)]
struct PendingRestoreWalk {
    /// Recovered entries, grouped by recipient but **not yet ordered** — see
    /// [`sort_pending_queue`], which the caller applies before the trims.
    grouped: HashMap<String, Vec<PendingMessage>>,
    /// Ids the per-message pass has already accounted for — recovered, or
    /// examined and settled as destroyed.
    ///
    /// The migration pass skips these. Recovered ones because a crash between
    /// its writes and its delete leaves a queue present in both layouts and the
    /// migrated copy is the one to keep; settled ones because re-filing an id
    /// the app was just told had failed would deliver a message after its own
    /// terminal event.
    seen: HashSet<String>,
    /// Ids of per-message records that were examined and destroyed. Settleable
    /// individually, because the key is the id.
    lost_ids: Vec<String>,
    /// Ids recovered from a legacy queue, which therefore have no record of
    /// their own yet.
    ///
    /// Persisted after admission rather than during the walk, so the entries the
    /// caps drop are never written at all — see [`OfflineProtocol::restore_pending_messages`].
    migrated: HashSet<String>,
    /// Legacy records whose entries have been recovered, to drop once those
    /// entries are durable under their own ids.
    migrated_recipients: Vec<String>,
    /// Recipients whose whole legacy queue died inside a record that would not
    /// open. Only reportable per peer — every id was inside it.
    lost_recipients: Vec<String>,
    /// Ids queued for a recipient that is no longer a valid user id.
    unaddressable: Vec<MessageId>,
    /// Entries opened across both passes — what [`MAX_PENDING_RESTORE_ENTRIES`]
    /// actually bounds.
    examined_entries: usize,
    entry_bound_reached: bool,
    prune_bound_reached: bool,
}

/// Puts a recovered queue back into canonical order: oldest first, ties broken
/// by message id.
///
/// The same comparator `evict_oldest_pending_message` uses, so "index 0 is the
/// oldest" holds for a restored queue exactly as it does for a live one — which
/// every oldest-first trim on the restore path depends on.
///
/// Necessary because records are keyed per message and come back in whatever
/// order the store enumerates them, which carries no ordering information at
/// all. The per-recipient layout got this for free from the array it stored, and
/// paid for it by rewriting that array on every enqueue.
fn sort_pending_queue(messages: &mut [PendingMessage]) {
    messages.sort_by(|left, right| {
        left.queued_at
            .cmp(&right.queued_at)
            .then_with(|| left.message_id.as_str().cmp(&right.message_id.as_str()))
    });
}

/// Advisory restore walks that draw on the shared pool, in the order
/// `initialize_mls` runs them.
///
/// Keep this in step with that call site. It is the divisor behind
/// [`MIN_ADVISORY_PRUNE_DELETES`], so adding a walk without bumping it means
/// the last walk in the sequence has no reservation left and can be starved —
/// the exact failure the floor exists to prevent.
/// `test_a_flooded_advisory_category_cannot_starve_the_walks_after_it` fails if
/// the two disagree.
pub(super) const ADVISORY_PRUNE_WALKS: usize = 5;

/// Deletes each advisory walk is guaranteed, whatever the walks before it did.
///
/// The shared pool alone bounds the launch but says nothing about *who* gets to
/// spend it, and the walks draw in a fixed order. Without a floor the first one
/// can empty the pool on its own: `restore_peer_key_packages` refuses only once
/// the pool is gone, so a key-package store over its cap by more than
/// [`MAX_RESTORE_PRUNE_DELETES`] — which the flood-eviction exemption makes an
/// ordinary state, not a tampered one — leaves the four walks after it with
/// nothing, on every launch, for as long as it stays over cap. Their prunes are
/// the only thing that ever deletes those records, so "deferred to a later
/// launch" would have quietly meant "never".
///
/// Reserving is elastic rather than a fixed split: each walk may spend
/// everything in the pool *except* the floor owed to the walks still to come,
/// so a launch where the early categories are clean still lets a later one draw
/// deeply.
///
/// The number is deliberately well under an even share of
/// [`MAX_RESTORE_PRUNE_DELETES`]. An even split would be the wrong trade: the
/// category that floods is the one that needs to converge fastest, while the
/// categories it would starve hold small counts (crash-orphaned media
/// descriptors, records that will not decode, session states the bootstrap
/// write repairs anyway). So the floor is set at what makes a starved walk
/// converge in a bounded number of launches rather than at what makes the split
/// fair, leaving the rest of the pool elastic for whichever category is
/// actually large. With five walks that reserves 320 of 512 and leaves the
/// first flooded walk 320 rather than 102.
pub(super) const MIN_ADVISORY_PRUNE_DELETES: usize = 64;

const _: () = assert!(
    ADVISORY_PRUNE_WALKS * MIN_ADVISORY_PRUNE_DELETES <= MAX_RESTORE_PRUNE_DELETES,
    "the advisory floors must fit inside one pool, or the first walk's ceiling \
     underflows to zero and it can never prune at all"
);

/// A pool of durable restore-path deletes, drawn on by one or more walks.
///
/// The unit the [`MAX_RESTORE_PRUNE_DELETES`] bound is actually about. A pool
/// outlives the walk that spends from it, which is the whole point: the five
/// advisory walks share one pool for the length of an `initialize_mls`, so
/// their deletes add up against a single allowance instead of each getting a
/// private one.
pub(crate) struct PruneAllowance {
    remaining: usize,
    /// Advisory walks that have not yet drawn from this pool.
    ///
    /// Decremented by each [`Self::refusing`] draw, and what that draw reserves
    /// for is the walks *after* it — see [`MIN_ADVISORY_PRUNE_DELETES`]. Kept
    /// on the pool rather than passed in per call so no walk has to know its
    /// own position in the sequence.
    advisory_walks_left: usize,
}

impl PruneAllowance {
    /// One pool of [`MAX_RESTORE_PRUNE_DELETES`] deletes.
    ///
    /// The **only** constructor, and deliberately so. A separate one for the
    /// settlement-paired walks let those walks allocate a pool *inside the
    /// callee* — which is the same shape this type exists to fix, one level
    /// down: a walk handing itself a private allowance, where a second call in
    /// the same launch silently doubles the ceiling with nothing to show for
    /// it. The two constructors had identical bodies, which was the tell.
    ///
    /// So every pool is constructed by the caller that owns the launch —
    /// `initialize_mls` builds all three side by side — and threaded in. The
    /// launch ceiling reads off that one call site, and
    /// `test_one_launch_cannot_exceed_the_derived_restore_delete_ceiling` pins
    /// it.
    pub(crate) fn pool() -> Self {
        Self {
            remaining: MAX_RESTORE_PRUNE_DELETES,
            advisory_walks_left: ADVISORY_PRUNE_WALKS,
        }
    }

    /// A budget for a walk whose records are caches or advisory signals, which
    /// may refuse a delete outright and leave the record for a later launch.
    ///
    /// Capped so the walks after this one keep their
    /// [`MIN_ADVISORY_PRUNE_DELETES`] floor. The two settlement-paired pools
    /// never call this, so their `advisory_walks_left` is simply never spent.
    pub(super) fn refusing(&mut self) -> PruneBudget<'_> {
        let later = self.advisory_walks_left.saturating_sub(1);
        self.advisory_walks_left = later;
        let reserved = later.saturating_mul(MIN_ADVISORY_PRUNE_DELETES);
        let ceiling = self.remaining.saturating_sub(reserved);
        PruneBudget::new(&mut self.remaining, ceiling, true)
    }

    /// A budget for a settlement-paired walk, which counts every delete but
    /// never refuses one.
    ///
    /// Refusing mid-record would break the pairing between a delete and the
    /// terminal event that names what it destroyed. The caller instead polls
    /// [`PruneBudget::is_spent`] between records and stops there. Reserves
    /// nothing: each of these owns its pool outright.
    fn counting(&mut self) -> PruneBudget<'_> {
        let ceiling = self.remaining;
        PruneBudget::new(&mut self.remaining, ceiling, false)
    }
}

/// One walk's view of a [`PruneAllowance`], and whether it ran out.
///
/// Deliberately has no `Default` and no free constructor: whether a walk may
/// *refuse* a delete or only *count* it is the whole safety question, so it has
/// to be answered at the point the budget is drawn from a pool rather than
/// inherited from whichever answer happened to be the zero value.
pub(super) struct PruneBudget<'a> {
    /// Deletes the pool can still fund. Shared with every other walk drawing
    /// on the same [`PruneAllowance`].
    remaining: &'a mut usize,
    /// The most *this* walk may take out of the pool, leaving the rest for the
    /// walks after it — see [`MIN_ADVISORY_PRUNE_DELETES`].
    ceiling: usize,
    /// Taken from the pool so far, which is what the ceiling bounds.
    drawn: usize,
    /// What *this* walk spent, for its own log line. Distinct from the pool, so
    /// one walk's report never includes another's deletes, and distinct from
    /// [`Self::drawn`] because a counting walk keeps deleting past its ceiling.
    pub(super) spent: usize,
    pub(super) exhausted: bool,
    /// Whether [`Self::claim`] may turn a delete down once this walk's share is
    /// gone. False for settlement-paired walks — see
    /// [`MAX_RESTORE_PRUNE_DELETES`].
    refusable: bool,
}

impl<'a> PruneBudget<'a> {
    fn new(remaining: &'a mut usize, ceiling: usize, refusable: bool) -> Self {
        Self {
            remaining,
            ceiling,
            drawn: 0,
            spent: 0,
            exhausted: false,
            refusable,
        }
    }

    /// Claims one delete. Refuses — and records that this walk's share is gone
    /// — only for a refusing budget; a counting one always allows the delete
    /// and just charges for it.
    fn claim(&mut self) -> bool {
        if self.is_spent() {
            self.exhausted = true;
            if self.refusable {
                return false;
            }
        } else {
            *self.remaining -= 1;
            self.drawn = self.drawn.saturating_add(1);
        }
        self.spent = self.spent.saturating_add(1);
        true
    }

    /// Whether this walk's share is used up — either the pool is empty or the
    /// walk has drawn everything it may without eating a later walk's floor. A
    /// counting caller checks this at each record boundary to decide whether to
    /// walk any further.
    fn is_spent(&self) -> bool {
        *self.remaining == 0 || self.drawn >= self.ceiling
    }
}

/// Claims one delete against an optional budget, allowing it unconditionally
/// when the caller supplied none.
///
/// The `None` case is for the runtime readers, which are not on a restore walk
/// and issue at most one delete per call. See [`MAX_RESTORE_PRUNE_DELETES`].
fn claim_prune(budget: &mut Option<&mut PruneBudget<'_>>) -> bool {
    match budget {
        Some(budget) => budget.claim(),
        None => true,
    }
}

impl OfflineProtocol {
    // ========================================================================
    // PROTOCOL-STATE RECORD I/O (SEALING + SIZE POLICY CHOKEPOINT)
    // ========================================================================

    /// Writes one protocol-state record, sealing it first when its category
    /// requires that, and refusing anything over
    /// [`MAX_PROTOCOL_STATE_RECORD_BYTES`].
    ///
    /// Every value written to [`ProtocolStateStorage`] goes through here, so
    /// confidentiality and size policy live in one place rather than being
    /// re-decided per call site.
    pub(crate) fn write_state_record(
        &self,
        storage: &dyn ProtocolStateStorage,
        key_type: &str,
        key_id: &str,
        data: &[u8],
    ) -> ProtocolStateResult<()> {
        if data.len() > MAX_PROTOCOL_STATE_RECORD_BYTES {
            return Err(ProtocolStateError::StoreFailed(format!(
                "record is {} bytes, over the {} byte limit",
                data.len(),
                MAX_PROTOCOL_STATE_RECORD_BYTES
            )));
        }

        // Fail closed on a category the sealing decision does not cover. The
        // alternative is writing it in the clear because the default answer to
        // "is this sensitive?" happened to be "no", which is exactly the
        // silent-plaintext outcome `StateCategory` exists to make impossible.
        let Some(category) = StateCategory::from_key_type(key_type) else {
            return Err(ProtocolStateError::StoreFailed(format!(
                "unknown protocol-state category '{}'; refusing to persist \
                 a record whose sensitivity has not been decided",
                key_type
            )));
        };

        if !category.requires_sealing() {
            return storage.store(key_type, key_id, data);
        }

        // Fail closed: without the per-install key this record would have to be
        // written in the clear, and losing crash recovery for it is strictly
        // less bad than losing at-rest confidentiality.
        let Some(cipher) = &self.state_record_cipher else {
            return Err(ProtocolStateError::StoreFailed(
                "protocol state record key unavailable; refusing to persist in the clear"
                    .to_string(),
            ));
        };
        let Some(sealed) = cipher.seal(key_type, key_id, data) else {
            return Err(ProtocolStateError::StoreFailed(
                "failed to seal protocol state record".to_string(),
            ));
        };
        storage.store(key_type, key_id, &sealed)
    }

    /// Reads one protocol-state record, opening it when its category is sealed,
    /// and discarding the bytes for callers that do not need the
    /// missing-versus-destroyed distinction.
    pub(crate) fn read_state_record(
        &self,
        storage: &dyn ProtocolStateStorage,
        key_type: &str,
        key_id: &str,
    ) -> ProtocolStateResult<Option<Vec<u8>>> {
        Ok(self
            .read_state_record_detailed(storage, key_type, key_id)?
            .into_bytes())
    }

    /// Loads one protocol-state entry for a caller that only needs to know
    /// whether usable bytes are there.
    ///
    /// Both [`ProtocolStateError::NotFound`] and
    /// [`ProtocolStateError::Corrupted`] read as `None`. The trait documents
    /// `NotFound` as the variant for implementations whose platform API cannot
    /// express absence any other way, so it has to be *read* as absence here —
    /// otherwise a provider that honors that contract turns every record it
    /// holds into a spurious unrecoverable loss. `Corrupted` is a record that
    /// exists and can never be decoded, which for a probe is the same answer:
    /// there is nothing to inherit, and writing over it is the recovery.
    ///
    /// Callers that owe the app a settlement need the two kept apart and go
    /// through [`Self::read_state_record_detailed`] instead.
    fn load_state_bytes(
        storage: &dyn ProtocolStateStorage,
        key_type: &str,
        key_id: &str,
    ) -> ProtocolStateResult<Option<Vec<u8>>> {
        match storage.load(key_type, key_id) {
            Ok(data) => Ok(data),
            Err(ProtocolStateError::NotFound(_) | ProtocolStateError::Corrupted(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Lists one protocol-state category, treating
    /// [`ProtocolStateError::NotFound`] as an empty category.
    ///
    /// Same reasoning as [`Self::load_state_bytes`]: a backend that can only
    /// spell "nothing is filed under this key type" as `NotFound` is reporting
    /// emptiness, not failure. Every restore propagates a listing error, so
    /// without this a provider honoring that contract fails restore and rolls
    /// `initialize_mls` back over a store that is merely empty.
    fn list_state_keys(
        storage: &dyn ProtocolStateStorage,
        key_type: &str,
    ) -> ProtocolStateResult<Vec<String>> {
        match storage.list_keys(key_type) {
            Ok(keys) => Ok(keys),
            Err(ProtocolStateError::NotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// Reads one protocol-state record, distinguishing an absent record from
    /// one that was destroyed and from one that merely cannot be read now.
    ///
    /// Never yields raw bytes for a record that is oversized or will not open.
    /// Such a record is corrupt or tampered, so it is also deleted, which keeps
    /// a poison record from being re-examined on every boot — and reported as
    /// [`StateRecord::Unreadable`], because a caller holding an app-visible
    /// promise about that record (an outbox entry the app was told was queued)
    /// has to settle it rather than let it evaporate.
    ///
    /// A missing record *key* is deliberately not that: those records may be
    /// perfectly good, so they are left on disk and reported as
    /// [`StateRecord::Unavailable`]. Nothing is recovered from them this run
    /// either way, but a later launch can — which is exactly why they must not
    /// be settled as failures now.
    ///
    /// The same split applies to what the store itself reports.
    /// [`ProtocolStateError::Corrupted`] is documented as a record that exists
    /// and cannot be decoded, which is permanent by construction — so it is
    /// [`StateRecord::Unreadable`], not a read to retry. Every other error is a
    /// failure of *this* read and stays [`StateRecord::Unavailable`].
    pub(crate) fn read_state_record_detailed(
        &self,
        storage: &dyn ProtocolStateStorage,
        key_type: &str,
        key_id: &str,
    ) -> ProtocolStateResult<StateRecord> {
        self.read_state_record_detailed_budgeted(storage, key_type, key_id, None)
    }

    /// [`Self::read_state_record_detailed`], charging its own drop-deletes to
    /// `budget` when one is supplied.
    ///
    /// This reader deletes what it cannot return, and on a restore walk that is
    /// a durable delete like any other — the same synchronous provider round
    /// trip, with the same directory flush, that
    /// [`MAX_RESTORE_PRUNE_DELETES`] exists to bound. A walk whose records are
    /// *all* unreadable (a regenerated record key makes every sealed record on
    /// the install unopenable) would otherwise issue one per record with
    /// nothing counting them.
    ///
    /// A [`PruneBudget::refusing`] budget may turn the unlink down and leave the
    /// record for a later launch; the record is reported
    /// [`StateRecord::Unreadable`] either way, since nothing is ever recovered
    /// from it. A [`PruneBudget::counting`] one always performs the unlink and
    /// merely charges for it, so a settlement-paired caller
    /// ([`Self::restore_pending_messages`]) keeps its delete and its terminal
    /// event together and stops at its next record boundary instead.
    ///
    /// `None` is for the runtime callers, which are not on a restore walk and
    /// issue at most one delete per call. **Every** restore walk passes a
    /// budget, and the two that once did not are why that is worth stating: a
    /// walk bound was argued to be ceiling enough for [`Self::restore_outbox`],
    /// and a bounded-by-the-peer-count argument for
    /// `restore_session_states_from_manager`. Neither was. See
    /// [`MAX_RESTORE_PRUNE_DELETES`].
    fn read_state_record_detailed_budgeted(
        &self,
        storage: &dyn ProtocolStateStorage,
        key_type: &str,
        key_id: &str,
        mut budget: Option<&mut PruneBudget<'_>>,
    ) -> ProtocolStateResult<StateRecord> {
        let data = match storage.load(key_type, key_id) {
            Ok(Some(data)) => data,
            Ok(None) => return Ok(StateRecord::Missing),
            // Absence, spelled as an error by a backend that cannot spell it
            // any other way.
            Err(ProtocolStateError::NotFound(_)) => return Ok(StateRecord::Missing),
            // A record was there and its store cannot decode it. Nothing will
            // ever decode it, so this is a loss to settle rather than a read to
            // retry — and it is deleted so it cannot be re-examined on every
            // boot. Providers that detect corruption themselves have usually
            // deleted it already, which makes the delete a no-op.
            Err(ProtocolStateError::Corrupted(detail)) => {
                warn!(
                    key_type = %key_type,
                    key_id = %key_id,
                    detail = %detail,
                    "Protocol state record reported corrupt by its store; dropping"
                );
                if claim_prune(&mut budget) {
                    let _ = storage.delete(key_type, key_id);
                }
                return Ok(StateRecord::Unreadable);
            }
            Err(e) => return Err(e),
        };

        // Bound before deserialization: a corrupted or tampered record must not
        // become an unbounded allocation while parsing during startup. The
        // limit is on the *plaintext*, which is what the write side checks, so
        // a sealed record is allowed its envelope on top — otherwise a record
        // written at exactly the cap could never be read back.
        let sealed = record_requires_sealing(key_type);
        let limit = if sealed {
            MAX_PROTOCOL_STATE_RECORD_BYTES.saturating_add(SEALED_RECORD_OVERHEAD)
        } else {
            MAX_PROTOCOL_STATE_RECORD_BYTES
        };
        if data.len() > limit {
            warn!(
                key_type = %key_type,
                key_id = %key_id,
                len = data.len(),
                limit,
                "Dropping oversized protocol state record"
            );
            if claim_prune(&mut budget) {
                let _ = storage.delete(key_type, key_id);
            }
            return Ok(StateRecord::Unreadable);
        }

        if !sealed {
            return Ok(StateRecord::Present(data));
        }

        let Some(cipher) = &self.state_record_cipher else {
            // Left on disk on purpose: the record is very likely intact and the
            // key may be back next launch. Reporting `Unavailable` is what stops
            // restore from settling the whole outbox as failed and then
            // re-delivering it on the next boot.
            warn!(
                key_type = %key_type,
                key_id = %key_id,
                "Protocol state record key unavailable; skipping sealed record"
            );
            return Ok(StateRecord::Unavailable);
        };

        match cipher.open(key_type, key_id, &data) {
            Some(plaintext) => Ok(StateRecord::Present(plaintext)),
            None => {
                warn!(
                    key_type = %key_type,
                    key_id = %key_id,
                    sealed = StateRecordCipher::looks_sealed(&data),
                    "Protocol state record failed to open; deleting"
                );
                if claim_prune(&mut budget) {
                    let _ = storage.delete(key_type, key_id);
                }
                Ok(StateRecord::Unreadable)
            }
        }
    }

    // ========================================================================
    // PRE-SPLIT PROTOCOL STATE ADOPTION
    // ========================================================================

    /// Adopts protocol state written before the storage split, moving it out of
    /// secure storage and into the install-scoped store.
    ///
    /// Splitting the domains renamed where this state lives. Without a sweep,
    /// the first launch after an upgrade would come up with an empty outbox, an
    /// empty pending queue, and — most sharply — an **empty block list**, since
    /// all of it was previously persisted through the `MlsStorage` handle. The
    /// records would also stay in the credential store forever with nothing
    /// ever reading or deleting them, which is the worst possible resting place
    /// for `pending_messages` (message plaintext) and `outbox` (cloud-media
    /// `encryption_key`/`iv`) — the very values the split exists to seal.
    ///
    /// This is a bulk move rather than the read-through
    /// [`crate::MlsStorage`] adoption used for MLS material, because
    /// [`storage_keys::ADOPTABLE_STATE_KEY_TYPES`] is a *closed* set: the SDK
    /// declares every protocol-state category itself, where OpenMLS contributes
    /// arbitrary labels to the MLS keyspace. Enumeration therefore terminates,
    /// which read-through can never guarantee.
    ///
    /// **This sweep depends on that other adoption.** On a pre-upgrade install
    /// the records it is looking for sit in the *un-namespaced* credential
    /// store, and the handle it enumerates is the namespaced one — so the only
    /// reason `secure.list_keys` and `secure.load` reach them at all is that
    /// the built-in providers union in, and read through to, the legacy store
    /// (`MlsSecureStorage` / `SecureStorage`, see `LegacyStoreAdoption`). Two
    /// independently-designed mechanisms, one silently load-bearing for the
    /// other: a provider built without a namespace has no read-through, so this
    /// sweep finds nothing and the pre-split state stays where it is. Python
    /// warns about exactly that case at construction.
    ///
    /// Properties worth keeping if this is ever touched:
    ///
    /// - **Resumable.** Each record is deleted from secure storage only once it
    ///   is durably written to protocol-state storage, so a crash mid-sweep
    ///   leaves the remainder to be re-adopted next launch.
    /// - **Non-destructive.** A key already present in protocol-state storage
    ///   wins and its legacy twin is left alone. Post-split state is always
    ///   authoritative, and — since one backend may legitimately serve both
    ///   handles in tests — a blind copy-then-delete could otherwise delete a
    ///   record through the same store it just read it from.
    /// - **All-or-nothing marker.** The marker is written only when the sweep
    ///   completed without a storage error, so a transiently unavailable
    ///   credential store means "try again next launch", not "give up forever".
    /// - **Sealing applies.** Records are written through
    ///   [`Self::write_state_record`], so categories that require sealing are
    ///   sealed on the way in and the legacy plaintext is then deleted.
    /// - **Nothing app-visible vanishes quietly.** A legacy record too large to
    ///   have any reachable destination is settled before it is deleted, the
    ///   same way [`Self::restore_outbox`] settles one it cannot recover. This
    ///   is not hypothetical: the pre-split build had neither a content cap nor
    ///   a per-peer byte budget, only 64 entries per peer, so the installs this
    ///   branch's budgets exist for are exactly the ones whose legacy records
    ///   can exceed [`MAX_PROTOCOL_STATE_RECORD_BYTES`].
    pub(crate) fn adopt_legacy_protocol_state(&mut self) {
        let (Some(secure), Some(state)) = (
            self.secure_storage.clone(),
            self.protocol_state_storage.clone(),
        ) else {
            return;
        };

        match Self::load_state_bytes(
            state.as_ref(),
            storage_keys::STATE_ADOPTION,
            storage_keys::STATE_ADOPTION_ID,
        ) {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(e) => {
                // Unknown whether the sweep already ran. Skipping is the safe
                // side: adoption is non-destructive, so re-running it later
                // costs nothing, while running it against a store we cannot
                // read is unnecessary work on every boot.
                warn!(error = %e, "Could not read the protocol-state adoption marker; skipping adoption");
                return;
            }
        }

        let mut adopted = 0usize;
        let mut failed = false;
        // Settlements for records the sweep had to destroy. Collected rather
        // than emitted inline because the per-record helper only holds `&self`,
        // and because they belong on the same deferred path every other restore
        // settlement uses.
        let mut unadoptable: Vec<Event> = Vec::new();
        for key_type in storage_keys::ADOPTABLE_STATE_KEY_TYPES {
            let key_ids = match secure.list_keys(key_type) {
                Ok(ids) => ids,
                Err(e) => {
                    warn!(key_type = %key_type, error = %e, "Failed to list legacy protocol state");
                    failed = true;
                    continue;
                }
            };

            if key_ids.len() > MAX_RESTORE_KEYS_PER_CATEGORY {
                // Never reachable from state this SDK wrote, so bound the pass
                // — but a truncated pass is not a completed one. Withholding
                // the marker is what makes the tail drain over successive
                // launches (each one adopts and deletes a prefix) instead of
                // being abandoned in the credential store forever, which for
                // `pending_messages` and `outbox` means message plaintext and
                // cloud-media key material parked in the one place this sweep
                // exists to clear.
                warn!(
                    key_type = %key_type,
                    listed = key_ids.len(),
                    cap = MAX_RESTORE_KEYS_PER_CATEGORY,
                    "Legacy protocol state listed more entries than any legitimate run can produce; adopting a prefix and retrying the rest next launch"
                );
                failed = true;
            }
            for key_id in key_ids.into_iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
                match self.adopt_one_legacy_record(
                    secure.as_ref(),
                    state.as_ref(),
                    key_type,
                    &key_id,
                    &mut unadoptable,
                ) {
                    Ok(true) => adopted += 1,
                    Ok(false) => {}
                    Err(()) => failed = true,
                }
            }
        }

        // Settle before the early return below: these records are already gone,
        // so a partial sweep must not also withhold the only signal the app will
        // ever get about them.
        for event in unadoptable {
            self.settle_restored_message_failure(event);
        }

        if failed {
            warn!(
                adopted,
                "Pre-split protocol state only partially adopted; will retry on the next launch"
            );
            return;
        }

        if let Err(e) = self.write_state_record(
            state.as_ref(),
            storage_keys::STATE_ADOPTION,
            storage_keys::STATE_ADOPTION_ID,
            &[],
        ) {
            // Without the marker the sweep simply runs again next launch. It is
            // idempotent, so that costs one wasted enumeration, not correctness.
            warn!(error = %e, "Failed to record the protocol-state adoption marker");
        }
        if adopted > 0 {
            info!(
                count = adopted,
                "Adopted pre-split protocol state from secure storage"
            );
        }
    }

    /// Moves one legacy record. `Ok(true)` when it was adopted, `Ok(false)`
    /// when there was nothing to do, `Err(())` when a storage failure means the
    /// sweep must be retried later. Settlements for records this had to destroy
    /// are pushed onto `unadoptable` for the caller to emit.
    fn adopt_one_legacy_record(
        &self,
        secure: &dyn MlsStorage,
        state: &dyn ProtocolStateStorage,
        key_type: &str,
        key_id: &str,
        unadoptable: &mut Vec<Event>,
    ) -> std::result::Result<bool, ()> {
        // Post-split state wins: never overwrite a record this build wrote.
        //
        // The probe gets the same three-way treatment as every other read, for
        // the same reason. A destination the store itself destroyed is not
        // absent — the app holds ids for it — so the sweep proceeds but owes a
        // settlement, which the legacy record only discharges by replacing it.
        // A destination that is merely unreadable *this session* is left alone
        // entirely: adopting over it would overwrite a record a later launch
        // can still read.
        let destination_destroyed = match self.read_state_record_detailed(state, key_type, key_id) {
            Ok(StateRecord::Present(_)) => return Ok(false),
            Ok(StateRecord::Missing) => false,
            Ok(StateRecord::Unreadable) => true,
            Ok(StateRecord::Unavailable) => {
                warn!(
                    key_type = %key_type,
                    "Protocol state record unreadable this session; deferring adoption of its legacy twin"
                );
                return Err(());
            }
            Err(e) => {
                warn!(key_type = %key_type, error = %e, "Failed to probe protocol state during adoption");
                return Err(());
            }
        };

        let data = match secure.load(key_type, key_id) {
            Ok(Some(data)) => data,
            // A key that listed but no longer loads was deleted underneath us.
            Ok(None) => {
                // Nothing left to inherit *and* the destination was destroyed
                // by the probe, so this is the last moment anything can name
                // what the app is still holding.
                if destination_destroyed {
                    unadoptable.extend(Self::unadoptable_record_settlement(key_type, key_id));
                }
                return Ok(false);
            }
            Err(e) => {
                warn!(key_type = %key_type, error = %e, "Failed to read legacy protocol state");
                return Err(());
            }
        };

        if data.len() > MAX_PROTOCOL_STATE_RECORD_BYTES {
            // Over the record cap, so it could never be written or restored.
            // Delete it rather than retrying the sweep forever over a record
            // that has no reachable destination — but settle first: the app is
            // holding ids from before the upgrade, and this is the last moment
            // anything can name them.
            warn!(
                key_type = %key_type,
                len = data.len(),
                "Dropping oversized legacy protocol state record"
            );
            unadoptable.extend(Self::unadoptable_record_settlement(key_type, key_id));
            let _ = secure.delete(key_type, key_id);
            return Ok(false);
        }

        if let Err(e) = self.write_state_record(state, key_type, key_id, &data) {
            warn!(key_type = %key_type, error = %e, "Failed to adopt legacy protocol state record");
            return Err(());
        }

        // Only now is the legacy copy redundant. Ordering matters: a delete
        // before the write would lose the record on a crash in between.
        if let Err(e) = secure.delete(key_type, key_id) {
            // The record is safely adopted; the leftover is cosmetic and the
            // next sweep's "already present" check will skip it. Do not fail
            // the sweep over it.
            debug!(key_type = %key_type, error = %e, "Failed to delete adopted legacy protocol state record");
        }
        Ok(true)
    }

    /// The settlement owed for a legacy record that had to be destroyed rather
    /// than adopted, or `None` for a category the app holds no promise about.
    ///
    /// Mirrors what [`Self::restore_outbox`] and [`Self::restore_pending_messages`]
    /// emit for the same loss after the split: an outbox record is nameable
    /// without opening it, because its record key *is* the message id, while a
    /// pending queue's ids are inside the record — so the recipient is the most
    /// that can be reported.
    ///
    /// An outbox key that does not parse as a [`MessageId`] falls back to the
    /// same diagnostic the pending queue uses rather than to silence. It should
    /// not exist — this SDK has only ever keyed the outbox by message id — but
    /// "should not exist" is exactly the class of record this whole path is for,
    /// and the sweep deletes it either way. A settlement nobody can act on still
    /// beats a destruction nobody is told about, which is the invariant the rest
    /// of this module spends its time enforcing.
    fn unadoptable_record_settlement(key_type: &str, key_id: &str) -> Option<Event> {
        match key_type {
            storage_keys::OUTBOX => Some(Self::unrecoverable_message_settlement(
                key_id,
                "Outbox entry from a previous version was too large to migrate",
                0,
            )),
            storage_keys::PENDING_MESSAGES => Some(Event::convergence_diag(
                "pending_state_lost".to_string(),
                key_id.to_string(),
                "Messages queued before the storage split exceeded the protocol-state record \
                 limit and could not be migrated"
                    .to_string(),
            )),
            _ => None,
        }
    }

    /// The settlement owed for a message-keyed record that has been destroyed
    /// and cannot be recovered — an outbox entry or a pending-queue entry.
    ///
    /// The record key *is* the message id, so the loss is nameable without
    /// opening the record — but only if the key parses. A key that does not is
    /// reported as a `pending_state_lost` diagnostic carrying the raw key,
    /// rather than as silence. It should not exist (this SDK keys both
    /// categories by message id), but "should not exist" is exactly the class of
    /// record every caller of this is handling, and the record is deleted either
    /// way. A settlement nobody can act on still beats a destruction nobody is
    /// told about.
    fn unrecoverable_message_settlement(key_id: &str, reason: &str, attempts: u32) -> Event {
        MessageId::from_str(key_id).map_or_else(
            |_| {
                Event::convergence_diag(
                    "pending_state_lost".to_string(),
                    key_id.to_string(),
                    format!(
                        "{reason}, and its message id could not be recovered from the record key"
                    ),
                )
            },
            |message_id| Event::message_failed(message_id, reason.to_string(), attempts),
        )
    }

    // ========================================================================
    // PROTOCOL-STATE RECORD KEY
    // ========================================================================

    /// Loads (or, on first run, generates and persists) the per-install key that
    /// seals sensitive protocol-state records.
    ///
    /// The key lives in *secure* storage while the records it protects live in
    /// the install-scoped container: uninstalling the app drops the container,
    /// and a container lifted without the credential store yields only
    /// ciphertext.
    ///
    /// Unlike the scrub and Nostr secrets, this one does **not** degrade to a
    /// session-local value when it cannot be persisted. A key that does not
    /// survive the process would seal records nothing could ever open, so a
    /// persist failure leaves the cipher uninstalled and sensitive categories
    /// simply are not persisted this session (they stay in memory and are
    /// re-driven from there, exactly as when no storage is configured at all).
    ///
    /// Unlike the other secret-restore paths this one has no "already loaded"
    /// short circuit, so the installed cipher is always the one belonging to
    /// the secure storage currently attached. Reusing a cipher across a storage
    /// swap would seal records under a key the next launch cannot find, which
    /// reads as silent loss of every sealed record.
    ///
    /// That invariant is enforced, not just documented: any previously
    /// installed cipher is dropped *before* the new store is read, so every
    /// path out of here either installs the key belonging to the currently
    /// attached store or leaves none at all. A failed load must not fall back
    /// to the key of a store we are no longer using — that cipher would open
    /// nothing here (records written under the new store's key are deleted as
    /// unauthentic on read) and would seal records the next launch cannot
    /// find a key for.
    ///
    /// # A present-but-wrong-length key is the destructive case
    ///
    /// The two failure branches below are deliberately asymmetric, and it is
    /// worth being explicit about why, because the asymmetry looks like an
    /// oversight and is not.
    ///
    /// A *failed load* leaves the cipher uninstalled. Sealed records then read
    /// as [`StateRecord::Unavailable`]: they stay on disk, nothing is settled,
    /// and a launch that can read the key recovers them. That is the recoverable
    /// case, and it is treated as one.
    ///
    /// A blob of the *wrong length* is not the key and never will be — no
    /// process can recover the original from it — so everything sealed under
    /// that key is already unrecoverable by the time this function runs.
    /// Regenerating is therefore not what destroys those records; it is what
    /// lets the install seal again. The visible consequence is still large: the
    /// records fail to open on the next read, are reported
    /// [`StateRecord::Unreadable`], and restore settles the whole outbox and
    /// pending queue as terminal `message_failed`. That is the honest answer —
    /// the alternative, refusing to regenerate, would preserve ciphertext
    /// nobody can ever open while permanently disabling persistence for every
    /// sensitive category.
    ///
    /// What it must not do is look like routine key generation in the log, so
    /// the warning names the consequence rather than the symptom.
    pub(crate) fn restore_or_init_state_record_key(&mut self) {
        self.state_record_cipher = None;

        let Some(storage) = &self.secure_storage else {
            return;
        };

        let key: Zeroizing<[u8; STATE_RECORD_KEY_BYTES]> = match storage.load(
            storage_keys::STATE_RECORD_KEY,
            storage_keys::STATE_RECORD_KEY_ID,
        ) {
            Ok(Some(bytes)) if bytes.len() == STATE_RECORD_KEY_BYTES => {
                let bytes = Zeroizing::new(bytes);
                let mut key = Zeroizing::new([0u8; STATE_RECORD_KEY_BYTES]);
                key.copy_from_slice(&bytes);
                debug!("Restored protocol state record key from secure storage");
                key
            }
            Ok(other) => {
                // A wrong-length blob is a corrupt write, not a usable key, and
                // nothing can recover the original from it — so whatever it
                // sealed is already lost before this runs. Regenerating is what
                // lets the install seal again; see the note on this function
                // for why refusing to is worse.
                if let Some(bytes) = &other {
                    warn!(
                        len = bytes.len(),
                        expected = STATE_RECORD_KEY_BYTES,
                        "Protocol state record key is not a key and cannot be recovered; \
                         regenerating. Every record sealed under the old key is \
                         unrecoverable and will be settled as failed on restore — this is \
                         not routine key generation"
                    );
                }
                let fresh = StateRecordCipher::generate_key();
                if let Err(e) = storage.store(
                    storage_keys::STATE_RECORD_KEY,
                    storage_keys::STATE_RECORD_KEY_ID,
                    &*fresh,
                ) {
                    warn!(
                        error = %e,
                        "Failed to persist protocol state record key; \
                         sensitive protocol state will not be persisted this session"
                    );
                    return;
                }
                info!("Generated and persisted per-install protocol state record key");
                fresh
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Failed to load protocol state record key; \
                     sensitive protocol state will not be persisted this session"
                );
                return;
            }
        };

        self.state_record_cipher = Some(StateRecordCipher::new(&key));
    }

    // ========================================================================
    // PENDING MESSAGES PERSISTENCE
    // ========================================================================

    /// Persists one queued message, keyed by its own id.
    ///
    /// Best-effort and infallible, like [`Self::persist_outbox_entry`]: the send
    /// path cannot propagate storage errors, so a failed write is logged and
    /// swallowed. The message still lives in the in-memory queue and still
    /// flushes when the session establishes; it just will not survive a restart.
    ///
    /// Re-persisting an id already on disk is an ordinary overwrite of its own
    /// record, which is what makes the flush-time re-queue path idempotent.
    pub(crate) fn persist_pending_message(&self, recipient: &str, message: &PendingMessage) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        let record = PendingMessageRecord {
            recipient: recipient.to_string(),
            message: message.clone(),
        };
        match serde_json::to_vec(&record) {
            Ok(data) => {
                if let Err(e) = self.write_state_record(
                    storage.as_ref(),
                    storage_keys::PENDING_MESSAGE_ENTRIES,
                    &record.message.message_id.as_str(),
                    &data,
                ) {
                    warn!(
                        recipient = %recipient,
                        message_id = %record.message.message_id,
                        error = %e,
                        "Failed to persist pending message"
                    );
                }
            }
            Err(e) => {
                warn!(
                    recipient = %recipient,
                    message_id = %record.message.message_id,
                    error = %e,
                    "Failed to serialize pending message"
                );
            }
        }
    }

    /// Removes one persisted pending message.
    ///
    /// Best-effort and logged rather than swallowed, for the reason the old
    /// whole-queue clear was: a delete that silently failed leaves a record the
    /// next launch restores and re-flushes. The message carries its original id
    /// so receivers dedup it, but the sender re-emits `MessageSent` for traffic
    /// it already delivered, and nothing would say why.
    pub(crate) fn delete_pending_message_from_storage(&self, message_id: &MessageId) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        if let Err(e) = storage.delete(storage_keys::PENDING_MESSAGE_ENTRIES, &message_id.as_str())
        {
            warn!(
                message_id = %message_id,
                error = %e,
                "Failed to clear persisted pending message"
            );
        }
    }

    /// Removes the persisted copies of a batch of queued messages.
    pub(crate) fn delete_pending_messages_from_storage<'a>(
        &self,
        message_ids: impl IntoIterator<Item = &'a MessageId>,
    ) {
        for message_id in message_ids {
            self.delete_pending_message_from_storage(message_id);
        }
    }

    /// Reassembles one recipient's persisted queue from the per-message
    /// records, in canonical queue order.
    ///
    /// Test-only, and deliberately the expensive spelling — it lists the whole
    /// category and filters. Production restore reads every record once and
    /// groups them in a single pass ([`Self::restore_pending_messages`]);
    /// nothing on the runtime path ever needs one peer's queue *from storage*,
    /// because the in-memory map is the source of truth.
    #[cfg(test)]
    pub(crate) fn load_pending_messages_from_storage(
        &self,
        recipient: &str,
    ) -> Option<Vec<PendingMessage>> {
        let storage = self.protocol_state_storage.as_ref()?;
        let keys =
            Self::list_state_keys(storage.as_ref(), storage_keys::PENDING_MESSAGE_ENTRIES).ok()?;
        let mut messages: Vec<PendingMessage> = keys
            .iter()
            .filter_map(|key| {
                let data = self
                    .read_state_record_detailed(
                        storage.as_ref(),
                        storage_keys::PENDING_MESSAGE_ENTRIES,
                        key,
                    )
                    .ok()?
                    .into_bytes()?;
                let record = serde_json::from_slice::<PendingMessageRecord>(&data).ok()?;
                (record.recipient == recipient).then_some(record.message)
            })
            .collect();
        if messages.is_empty() {
            return None;
        }
        sort_pending_queue(&mut messages);
        for message in &mut messages {
            message.measure();
        }
        Some(messages)
    }

    /// Loads a recipient's queue from the **legacy** per-recipient record,
    /// distinguishing "nothing queued" from "a queue existed and its contents
    /// are gone" from "a queue exists and cannot be read this session".
    ///
    /// Entries come back *unmeasured*: the derived `serialized_bytes` is
    /// deliberately not persisted, and measuring re-serializes every entry, so
    /// the caller measures only what it decides to keep rather than paying for
    /// entries the per-peer count trim is about to drop. Anything that enters
    /// `pending_encrypted_messages` must be measured first or it reads as free
    /// against the queue byte budgets.
    ///
    /// `budget` counts the durable deletes this read causes — the reader's own
    /// drop of a record that will not open, and the drop below of one whose
    /// bytes will not parse. It must be a [`PruneBudget::counting`] budget, so
    /// neither delete is ever refused: both are paired with the
    /// `pending_state_lost` the caller emits, and settling a record still
    /// sitting on disk is a claim the next launch contradicts. The caller stops
    /// walking once the budget reads spent instead.
    fn load_legacy_pending_queue(
        &self,
        recipient: &str,
        mut budget: Option<&mut PruneBudget<'_>>,
    ) -> PendingRestore {
        let Some(storage) = self.protocol_state_storage.as_ref() else {
            return PendingRestore::Absent;
        };
        let record = self.read_state_record_detailed_budgeted(
            storage.as_ref(),
            storage_keys::PENDING_MESSAGES,
            recipient,
            budget.as_deref_mut(),
        );
        let data = match record {
            Ok(StateRecord::Present(data)) => data,
            Ok(StateRecord::Missing) => return PendingRestore::Absent,
            // Examined and destroyed: the queue that existed is gone for good.
            Ok(StateRecord::Unreadable) => return PendingRestore::Lost,
            // Still on disk, just not readable now. Reporting it as lost would
            // be contradicted by the next launch restoring it.
            Ok(StateRecord::Unavailable) | Err(_) => return PendingRestore::Unavailable,
        };
        let Ok(messages) = serde_json::from_slice::<Vec<PendingMessage>>(&data) else {
            // Parsed-as-garbage is the same loss as failed-to-open. Drop the
            // record so it is not re-parsed on every boot.
            warn!(recipient = %recipient, "Dropping corrupted pending message record");
            // Charged, and the answer deliberately not honoured: this delete is
            // paired with the caller's `pending_state_lost`, so a budget that
            // refused it would leave a settled record on disk. A `counting`
            // budget never refuses; ignoring the answer is what keeps a
            // mistakenly-`refusing` one failing in the safe direction.
            claim_prune(&mut budget);
            let _ = storage.delete(storage_keys::PENDING_MESSAGES, recipient);
            return PendingRestore::Lost;
        };
        PendingRestore::Restored(messages)
    }

    /// Drops a legacy per-recipient record once its entries have been migrated
    /// into the per-message layout, or once they have been settled.
    ///
    /// Only ever called with a record this session has already *read*, which is
    /// what makes it safe without the freeze the old whole-queue clear needed:
    /// nothing outside the migration walk touches this category any more, so a
    /// record no walk reached is simply read by a later launch.
    fn delete_legacy_pending_queue(&self, recipient: &str) {
        if let Some(storage) = &self.protocol_state_storage {
            if let Err(e) = storage.delete(storage_keys::PENDING_MESSAGES, recipient) {
                warn!(
                    recipient = %recipient,
                    error = %e,
                    "Failed to clear migrated legacy pending queue"
                );
            }
        }
    }

    /// Admits one recipient's recovered queue, holding it to the same caps the
    /// live admission path enforces.
    ///
    /// Split out of [`Self::restore_pending_messages`] because it is the one
    /// part of that walk with its own invariants rather than its own control
    /// flow: a record written by an older build — or by a build with no caps at
    /// all, which is every pre-split build — can hold entries the current
    /// boundary would reject, so restore has to re-apply four bounds in a fixed
    /// order.
    ///
    /// The order is load-bearing:
    ///
    /// 1. **Count trim first**, so the measure below is not paid for entries
    ///    that are about to be dropped anyway.
    /// 2. **Measure the survivors.** `serialized_bytes` is derived and not
    ///    persisted, and everything after this point — both byte budgets and the
    ///    map insert — reads an unmeasured entry as free.
    /// 3. **Per-peer byte trim**, oldest-first like every other eviction here.
    /// 4. **Global count and byte trim**, which can evict from a recipient
    ///    admitted on an earlier iteration, hence the shared accumulator.
    ///
    /// Every entry this drops goes into `capacity_evicted`, and the caller both
    /// settles those ids and deletes their records. Under the per-recipient
    /// layout the trim also had to *rewrite* the survivors' record, so it
    /// tracked which recipients had changed; now a dropped entry is just its own
    /// record, and the survivors' records are already correct.
    fn admit_restored_pending_queue(
        &mut self,
        recipient: &str,
        mut messages: Vec<PendingMessage>,
        admission: &mut RestoredPendingAdmission,
    ) {
        let overflow = messages.len().saturating_sub(MAX_PENDING_MESSAGES_PER_PEER);
        if overflow > 0 {
            admission
                .capacity_evicted
                .extend(messages.drain(..overflow).map(|message| message.message_id));
        }
        for message in &mut messages {
            message.measure();
        }

        let mut peer_bytes: usize = messages
            .iter()
            .map(|message| message.serialized_bytes)
            .sum();
        let mut dropped_for_bytes = 0usize;
        while peer_bytes > MAX_PENDING_MESSAGE_BYTES_PER_PEER && !messages.is_empty() {
            let message = messages.remove(0);
            peer_bytes = peer_bytes.saturating_sub(message.serialized_bytes);
            admission.capacity_evicted.push(message.message_id);
            dropped_for_bytes += 1;
        }
        if dropped_for_bytes > 0 {
            warn!(
                recipient = %recipient,
                dropped = dropped_for_bytes,
                limit = MAX_PENDING_MESSAGE_BYTES_PER_PEER,
                "Restored pending queue exceeded the per-peer byte budget"
            );
        }

        if !messages.is_empty() {
            info!(recipient = %recipient, count = messages.len(), "Restored pending messages from storage");
            admission.global_count += messages.len();
            admission.global_bytes += peer_bytes;
            self.pending_encrypted_messages
                .insert(recipient.to_string(), messages);
        }

        while admission.global_count > MAX_PENDING_MESSAGES_GLOBAL
            || admission.global_bytes > MAX_PENDING_MESSAGE_BYTES_GLOBAL
        {
            let Some((_, message)) = self.evict_oldest_pending_message() else {
                break;
            };
            admission.global_count = admission.global_count.saturating_sub(1);
            admission.global_bytes = admission
                .global_bytes
                .saturating_sub(message.serialized_bytes);
            admission.capacity_evicted.push(message.message_id);
        }
    }

    /// Restores all pending messages from storage on startup.
    ///
    /// Two passes over two layouts:
    ///
    /// 1. **Per-message records** ([`storage_keys::PENDING_MESSAGE_ENTRIES`]),
    ///    which is what this SDK writes. Each is one queued message keyed by its
    ///    own id, so a record that will not open still *names* the message it
    ///    destroyed and is settled with a `message_failed` for exactly that id —
    ///    the reason the layout exists.
    /// 2. **Legacy per-recipient records** ([`storage_keys::PENDING_MESSAGES`]),
    ///    migrated forward: read the queue, and once the caps have been applied
    ///    write each *surviving* entry as its own record and drop the legacy one
    ///    ([`Self::persist_migrated_pending_queues`]). A loss there is still only
    ///    reportable per peer, because every id was inside the record that would
    ///    not open.
    ///
    /// The migration writes before it deletes, so a crash in between leaves the
    /// entries in *both* layouts rather than neither. Pass 1 runs first and
    /// claims those ids — recovered or settled — so pass 2 skips them and the
    /// next launch converges.
    ///
    /// Neither pass needs the freeze the per-recipient walk did. Nothing outside
    /// this walk writes or deletes the legacy category any more, and a
    /// per-message record is only ever written or deleted under its own id — so
    /// an unwalked record is simply read by a later launch, which is the same
    /// argument [`Self::restore_outbox`] makes for the outbox.
    ///
    /// `allowance` is this walk's own pool, built by the launch — see
    /// [`PruneAllowance::pool`] and [`MAX_RESTORE_PRUNE_DELETES`].
    pub(crate) fn restore_pending_messages(
        &mut self,
        allowance: &mut PruneAllowance,
    ) -> Result<()> {
        let Some(storage) = self.protocol_state_storage.clone() else {
            return Ok(());
        };

        // Counting rather than refusing: every delete either pass issues is
        // paired with a settlement, and a settlement for a record still on disk
        // is a claim the next launch contradicts. So the walk spends its
        // allowance, then stops at the next *record boundary* — the one place
        // stopping is safe — and leaves the rest for a later launch.
        //
        // Its own pool rather than the launch-wide advisory one: being starved
        // here means every diagnostic this walk owes is deferred, which must not
        // be a consequence of a key-package flood in an unrelated category. The
        // pool is passed in rather than built here — see `PruneAllowance::pool`.
        let mut budget = allowance.counting();
        let mut walk = PendingRestoreWalk::default();

        self.restore_pending_message_entries(storage.as_ref(), &mut budget, &mut walk)?;
        self.migrate_legacy_pending_queues(storage.as_ref(), &mut budget, &mut walk)?;

        // Records come back in whatever order the store enumerates them, which
        // carries no ordering information at all, so the canonical order has to
        // be re-imposed before the oldest-first trims below mean anything.
        // Recipients are visited in a stable order for the same reason: the
        // global caps evict across peers, so a map's iteration order would make
        // *which* entries survive a coin flip.
        let mut admission = RestoredPendingAdmission::default();
        let mut grouped: Vec<(String, Vec<PendingMessage>)> =
            std::mem::take(&mut walk.grouped).into_iter().collect();
        grouped.sort_by(|left, right| left.0.cmp(&right.0));
        for (recipient, mut messages) in grouped {
            sort_pending_queue(&mut messages);
            self.admit_restored_pending_queue(&recipient, messages, &mut admission);
        }

        let RestoredPendingAdmission {
            capacity_evicted, ..
        } = admission;

        // Every id the caps dropped had its own record, so the record goes with
        // the settlement rather than the survivors being rewritten around it —
        // unless it came out of a legacy queue, which has no per-message record
        // yet because the persist below writes only what survived.
        //
        // Budgeted like the outbox's capacity drain, and stopped between entries
        // for the same reason: an entry the pool cannot fund is already out of
        // memory and is left on disk **unsettled**, so a later launch restores
        // it, re-caps it, and owns both halves then. Stopping here also stops the
        // legacy deletes below, so a migrated entry settled above always has its
        // source record deleted in the same launch.
        let mut capacity_settlements = Vec::new();
        for message_id in capacity_evicted {
            if !walk.migrated.contains(&message_id.as_str()) {
                if budget.is_spent() {
                    walk.prune_bound_reached = true;
                    break;
                }
                budget.claim();
                self.delete_pending_message_from_storage(&message_id);
            }
            capacity_settlements.push(Event::message_failed(
                message_id,
                "Pending session queue capacity exceeded".to_string(),
                0,
            ));
        }
        self.settle_restored_message_failures(capacity_settlements);

        self.persist_migrated_pending_queues(&mut budget, &mut walk);

        if walk.entry_bound_reached {
            warn!(
                examined = walk.examined_entries,
                cap = MAX_PENDING_RESTORE_ENTRIES,
                "Pending message store held more queued entries than one restore may walk; \
                 the remainder is left on disk for a later launch"
            );
        }
        if walk.prune_bound_reached {
            warn!(
                deleted = budget.spent,
                budget = MAX_RESTORE_PRUNE_DELETES,
                "Pending message restore spent its per-launch delete budget; the remainder \
                 is left on disk for a later launch"
            );
        }

        self.settle_restored_message_failures(walk.unaddressable.into_iter().map(|message_id| {
            Event::message_failed(
                message_id,
                "Recipient is not a valid user ID; queued message cannot be delivered".to_string(),
                0,
            )
        }));
        self.settle_restored_message_failures(walk.lost_ids.iter().map(|key| {
            Self::unrecoverable_message_settlement(
                key,
                "Queued message awaiting session establishment could not be recovered \
                 from protocol-state storage",
                0,
            )
        }));
        for recipient in &walk.lost_recipients {
            warn!(recipient = %recipient, "Persisted legacy pending queue was unreadable and has been dropped");
        }
        self.settle_restored_message_failures(walk.lost_recipients.into_iter().map(|recipient| {
            Event::convergence_diag(
                "pending_state_lost".to_string(),
                recipient,
                "Queued messages awaiting session establishment could not be recovered \
                 from protocol-state storage and have been dropped"
                    .to_string(),
            )
        }));

        self.recompute_next_pending_message_expiry();
        self.cleanup_expired_pending_messages();
        Ok(())
    }

    /// Gives every migrated entry that survived admission a record of its own,
    /// then drops the legacy records those entries came out of.
    ///
    /// Deliberately after the caps rather than during the walk. A pre-cap legacy
    /// install can hold far more than the current bounds admit, and writing each
    /// entry out only to delete it again moments later is the one shape that
    /// turns an ordinary upgrade into thousands of device barriers on the boot
    /// path. Writing only the survivors makes the burst bounded by
    /// [`MAX_PENDING_MESSAGES_GLOBAL`], and it is still write-before-delete: the
    /// legacy record stays on disk until its survivors are durable, so a crash
    /// in between leaves the entries in both layouts rather than neither.
    fn persist_migrated_pending_queues(
        &self,
        budget: &mut PruneBudget<'_>,
        walk: &mut PendingRestoreWalk,
    ) {
        if walk.migrated_recipients.is_empty() {
            return;
        }
        for (recipient, messages) in &self.pending_encrypted_messages {
            for message in messages {
                if walk.migrated.contains(&message.message_id.as_str()) {
                    self.persist_pending_message(recipient, message);
                }
            }
        }
        // Charged like every other delete this walk issues, and stopped between
        // records: a legacy record the pool cannot fund is simply read again by
        // a later launch, which finds its entries already claimed by the
        // per-message pass and completes the delete then.
        for recipient in &walk.migrated_recipients {
            if budget.is_spent() {
                walk.prune_bound_reached = true;
                break;
            }
            budget.claim();
            self.delete_legacy_pending_queue(recipient);
        }
    }

    /// Reads the per-message pending records, grouping them by recipient.
    ///
    /// Shaped like [`Self::restore_outbox`], because it is now the same problem:
    /// one record per id, each loss individually settleable, no freeze.
    fn restore_pending_message_entries(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        budget: &mut PruneBudget<'_>,
        walk: &mut PendingRestoreWalk,
    ) -> Result<()> {
        let keys = Self::list_state_keys(storage, storage_keys::PENDING_MESSAGE_ENTRIES)
            .map_err(|e| Error::Other(format!("Failed to list pending messages: {}", e)))?;
        let listed = keys.len();

        for key in keys.into_iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            if walk.examined_entries >= MAX_PENDING_RESTORE_ENTRIES {
                walk.entry_bound_reached = true;
                break;
            }
            if budget.is_spent() {
                walk.prune_bound_reached = true;
                break;
            }
            let data = match self.read_state_record_detailed_budgeted(
                storage,
                storage_keys::PENDING_MESSAGE_ENTRIES,
                &key,
                Some(budget),
            ) {
                Ok(StateRecord::Present(data)) => data,
                Ok(StateRecord::Missing) => continue,
                // Examined and destroyed. The key *is* the id the application is
                // holding, so unlike the legacy layout this is settleable on its
                // own without having opened the record.
                Ok(StateRecord::Unreadable) => {
                    warn!(message_id = %key, "Dropping unreadable pending message");
                    walk.seen.insert(key.clone());
                    walk.lost_ids.push(key);
                    continue;
                }
                // Still on disk and probably intact — the record key is not
                // loaded, or the backend refused this read. Settling it now would
                // be a terminal answer the next launch overturns by restoring the
                // entry and flushing it.
                Ok(StateRecord::Unavailable) | Err(_) => {
                    warn!(
                        message_id = %key,
                        "Pending message could not be read this session; leaving it in place"
                    );
                    continue;
                }
            };
            walk.examined_entries = walk.examined_entries.saturating_add(1);

            let record = match serde_json::from_slice::<PendingMessageRecord>(&data) {
                Ok(record) => record,
                Err(e) => {
                    warn!(message_id = %key, error = %e, "Dropping corrupted pending message");
                    // Charged, never refused: the settlement below is already
                    // owed, so the record has to go with it.
                    budget.claim();
                    let _ = storage.delete(storage_keys::PENDING_MESSAGE_ENTRIES, &key);
                    walk.seen.insert(key.clone());
                    walk.lost_ids.push(key);
                    continue;
                }
            };

            // A record whose id disagrees with its key is tampered or corrupt,
            // and is worse than merely unparseable: it is addressed under one id
            // and settles under another, so nothing on the runtime path could
            // ever delete it. Treat it as the loss it is.
            if record.message.message_id.as_str() != key {
                warn!(
                    message_id = %key,
                    record_id = %record.message.message_id,
                    "Dropping pending message whose record does not name its own key"
                );
                budget.claim();
                let _ = storage.delete(storage_keys::PENDING_MESSAGE_ENTRIES, &key);
                walk.seen.insert(key.clone());
                walk.lost_ids.push(key);
                continue;
            }

            if Self::validate_outbound_recipient(&record.recipient).is_err() {
                // Nothing can ever be sent to this recipient again, so the record
                // goes — but the app still holds the id from `send_message*` and
                // it has to be settled.
                warn!(
                    message_id = %key,
                    "Dropping persisted pending message for an invalid recipient"
                );
                budget.claim();
                let _ = storage.delete(storage_keys::PENDING_MESSAGE_ENTRIES, &key);
                walk.seen.insert(key);
                walk.unaddressable.push(record.message.message_id);
                continue;
            }

            walk.seen.insert(key);
            walk.grouped
                .entry(record.recipient)
                .or_default()
                .push(record.message);
        }

        if listed > MAX_RESTORE_KEYS_PER_CATEGORY {
            warn!(
                listed,
                cap = MAX_RESTORE_KEYS_PER_CATEGORY,
                "Pending message store listed more records than any legitimate run can \
                 produce; the tail is left on disk for a later launch"
            );
        }
        Ok(())
    }

    /// Recovers legacy per-recipient queues so the caps can be applied to them.
    ///
    /// Recovery only — [`Self::persist_migrated_pending_queues`] does the writing
    /// and the legacy delete, after admission, so entries the caps drop are never
    /// written at all.
    ///
    /// One-shot upgrade scaffolding, like `adopt_legacy_protocol_state`: once a
    /// launch has walked every legacy record the listing comes back empty and
    /// this costs one `list_keys`. Retire it on the same trigger as that sweep —
    /// when the oldest supported upgrade path starts at or after the release
    /// that introduced the per-message layout, not merely one release later.
    fn migrate_legacy_pending_queues(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        budget: &mut PruneBudget<'_>,
        walk: &mut PendingRestoreWalk,
    ) -> Result<()> {
        let recipients = Self::list_state_keys(storage, storage_keys::PENDING_MESSAGES)
            .map_err(|e| Error::Other(format!("Failed to list legacy pending messages: {}", e)))?;
        if recipients.is_empty() {
            return Ok(());
        }
        let listed = recipients.len();

        for recipient in recipients.into_iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            if walk.examined_entries >= MAX_PENDING_RESTORE_ENTRIES {
                walk.entry_bound_reached = true;
                break;
            }
            if budget.is_spent() {
                walk.prune_bound_reached = true;
                break;
            }
            if Self::validate_outbound_recipient(&recipient).is_err() {
                // The queue is unaddressable — nothing can ever be sent to this
                // recipient again — so it is dropped, but the app still holds
                // ids from `send_message*` that must be settled. The record has
                // to be read *before* deleting it to recover them, and the read
                // gets the same three-way treatment as every other restore.
                match self.load_legacy_pending_queue(&recipient, Some(budget)) {
                    PendingRestore::Restored(messages) => {
                        walk.examined_entries =
                            walk.examined_entries.saturating_add(messages.len());
                        walk.unaddressable
                            .extend(messages.into_iter().map(|m| m.message_id));
                    }
                    // Listed but no longer there. Nothing was destroyed, so
                    // nothing is owed — and the delete below would charge the
                    // budget, log a destruction, and make a provider round trip
                    // for a record that is already gone.
                    PendingRestore::Absent => continue,
                    // Already examined and destroyed by the read; the ids went
                    // with it, so report the loss per recipient rather than
                    // letting an unaddressable queue vanish more quietly than an
                    // addressable one would.
                    PendingRestore::Lost => {
                        walk.lost_recipients.push(recipient);
                        continue;
                    }
                    // Intact on disk, just unreadable this session. Deleting it
                    // now would destroy ids nothing can name; a later launch
                    // reads the record and settles them properly.
                    PendingRestore::Unavailable => {
                        warn!(
                            recipient = %recipient,
                            "Legacy pending queue for an invalid recipient could not be read this session; leaving it in place"
                        );
                        continue;
                    }
                }
                warn!(recipient = %recipient, "Dropping legacy pending queue for an invalid recipient");
                // Charged, never refused: the settlement this delete pairs with
                // was already recorded above, so the record has to go with it.
                budget.claim();
                self.delete_legacy_pending_queue(&recipient);
                continue;
            }

            let messages = match self.load_legacy_pending_queue(&recipient, Some(budget)) {
                PendingRestore::Restored(messages) => messages,
                PendingRestore::Absent => continue,
                // A queue existed and its contents are gone. The ids are inside
                // the record we could not open, so they cannot be settled
                // individually — surface the loss per recipient instead of
                // letting it read as "there was nothing queued". This is the
                // reporting the per-message layout exists to improve on, and it
                // cannot be improved retroactively.
                PendingRestore::Lost => {
                    walk.lost_recipients.push(recipient);
                    continue;
                }
                // Not readable this session, but still on disk. Say nothing: a
                // later launch is expected to read it, and a loss diagnostic now
                // would be a claim the next launch contradicts.
                PendingRestore::Unavailable => {
                    warn!(
                        recipient = %recipient,
                        "Legacy pending queue could not be read this session; leaving it in place"
                    );
                    continue;
                }
            };
            walk.examined_entries = walk.examined_entries.saturating_add(messages.len());

            // Recovered, not yet written. The caller persists what survives the
            // caps and then drops this record, so a queue too big for the
            // current bounds is never written out only to be deleted again.
            let mut migrated = 0usize;
            for message in messages {
                let message_id = message.message_id.as_str();
                if walk.seen.contains(&message_id) {
                    // Already accounted for by the pass before this one — either
                    // a previous launch migrated this queue and died before
                    // dropping it, or the per-message record was examined and
                    // settled. Re-filing a settled id would put a message on the
                    // wire after the app was told it failed.
                    continue;
                }
                walk.seen.insert(message_id.clone());
                walk.migrated.insert(message_id);
                walk.grouped
                    .entry(recipient.clone())
                    .or_default()
                    .push(message);
                migrated = migrated.saturating_add(1);
            }
            info!(
                recipient = %recipient,
                migrated,
                "Recovered legacy pending queue for migration"
            );
            walk.migrated_recipients.push(recipient);
        }

        if listed > MAX_RESTORE_KEYS_PER_CATEGORY {
            warn!(
                listed,
                cap = MAX_RESTORE_KEYS_PER_CATEGORY,
                "Legacy pending message store listed more recipients than any legitimate run \
                 can produce; the tail is left on disk for a later launch"
            );
        }
        Ok(())
    }

    // ========================================================================
    // PEER KEY PACKAGES PERSISTENCE
    // ========================================================================

    /// Persists a received key package for a peer so it survives restart.
    pub(crate) fn persist_peer_key_package(&self, peer_id: &str, pkg: &ReceivedKeyPackage) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        match serde_json::to_vec(pkg) {
            Ok(data) => {
                if let Err(e) = self.write_state_record(
                    storage.as_ref(),
                    storage_keys::PEER_KEY_PACKAGES,
                    peer_id,
                    &data,
                ) {
                    warn!(peer_id = %peer_id, error = %e, "Failed to persist peer key package");
                }
            }
            Err(e) => {
                warn!(peer_id = %peer_id, error = %e, "Failed to serialize peer key package");
            }
        }
    }

    /// Loads a persisted key package for a peer (if present and not expired).
    ///
    /// Unbudgeted, for the runtime callers: one expired record dropped while
    /// resolving a single peer is one provider round trip, not a storm.
    pub(crate) fn load_peer_key_package_from_storage(
        &self,
        peer_id: &str,
    ) -> Option<ReceivedKeyPackage> {
        self.load_peer_key_package_bounded(peer_id, None)
    }

    /// Loads a persisted key package, charging the drop of an expired record to
    /// `budget` when one is supplied.
    ///
    /// Restore supplies one. The expiry drop is a durable delete like any other
    /// on that path — a synchronous provider round trip that flushes the
    /// containing directory on all three built-in stores — and it is the
    /// *common* one there: an over-cap key-package store is over-cap because it
    /// is old, and [`MAX_KEY_PACKAGE_LIFETIME_MS`] is 30 days, so most of what
    /// a restore walk finds in such a store has expired. Left unbudgeted it
    /// also bypassed the over-cap prune entirely, because an expired record
    /// never enters `pending_key_packages` and so never makes the cap bind.
    ///
    /// A record the budget spares is still not returned — it has expired either
    /// way — it is simply left on disk for a later launch to drop, which is the
    /// same idempotent-and-resumable property the rest of the prune relies on.
    fn load_peer_key_package_bounded(
        &self,
        peer_id: &str,
        mut budget: Option<&mut PruneBudget<'_>>,
    ) -> Option<ReceivedKeyPackage> {
        let storage = self.protocol_state_storage.as_ref()?;
        // The reader deletes what it cannot return, so it gets the budget too —
        // that delete is as durable as the expiry one below and was the last
        // one on this walk that nothing counted.
        let data = self
            .read_state_record_detailed_budgeted(
                storage.as_ref(),
                storage_keys::PEER_KEY_PACKAGES,
                peer_id,
                budget.as_deref_mut(),
            )
            .ok()?
            .into_bytes()?;
        let pkg: ReceivedKeyPackage = serde_json::from_slice(&data).ok()?;
        let now_ms = Utc::now().timestamp_millis() as u64;
        if now_ms >= pkg.local_expires_at_ms {
            if claim_prune(&mut budget) {
                let _ = storage.delete(storage_keys::PEER_KEY_PACKAGES, peer_id);
            }
            return None;
        }
        Some(pkg)
    }

    /// Removes persisted key package for a peer (e.g. after session created).
    pub(crate) fn delete_peer_key_package_from_storage(&self, peer_id: &str) {
        if let Some(storage) = &self.protocol_state_storage {
            let _ = storage.delete(storage_keys::PEER_KEY_PACKAGES, peer_id);
        }
    }

    /// Loads key package from storage into memory if not already present. Returns true if we now have one in memory.
    pub(crate) fn try_load_key_package_from_storage_into_memory(&mut self, peer_id: &str) -> bool {
        if self.pending_key_packages.contains_key(peer_id) {
            return true;
        }
        if let Some(pkg) = self.load_peer_key_package_from_storage(peer_id) {
            self.pending_key_packages.insert(peer_id.to_string(), pkg);
            return true;
        }
        false
    }

    /// Restores peer key packages from storage for peers that have no MLS session.
    pub(crate) fn restore_peer_key_packages(
        &mut self,
        mls: &Arc<RwLock<MlsManager>>,
        allowance: &mut PruneAllowance,
    ) -> Result<()> {
        let Some(storage) = &self.protocol_state_storage else {
            return Ok(());
        };

        let peer_ids = Self::list_state_keys(storage.as_ref(), storage_keys::PEER_KEY_PACKAGES)
            .map_err(|e| Error::Other(format!("Failed to list peer key packages: {}", e)))?;
        let listed = peer_ids.len();

        let sessions = {
            let manager = mls
                .read()
                .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
            manager.list_sessions().map_err(Error::Mls)?
        };
        let session_set: std::collections::HashSet<_> = sessions.into_iter().collect();

        let mut budget = allowance.refusing();
        let mut over_cap_pruned = 0usize;
        // Bounded like every other category walk, and the prune inside it is
        // bounded again by `MAX_RESTORE_PRUNE_DELETES` — the walk bounds reads,
        // the budget bounds deletes, and a delete is the far more expensive of
        // the two (a synchronous provider round trip that flushes a directory
        // on all three built-in stores). Both tails drain over successive
        // launches rather than stranding.
        //
        // *Both* kinds of delete this walk issues share that budget: the
        // over-cap prune below and the expiry drop inside
        // `load_peer_key_package_bounded`. They are not alternatives — an
        // expired record never enters the map, so it never makes the cap bind,
        // which means an over-cap store full of expired records would otherwise
        // route every one of its deletes past the budget.
        for peer_id in peer_ids.into_iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            if session_set.contains(&peer_id) {
                continue;
            }
            // Bound restore to the same cap as the live insert path so a
            // pre-existing over-cap durable store (e.g. a flood that landed
            // before the cap existed) cannot re-inflate memory on boot. Rather
            // than leaving the overflow to linger on disk forever — where it
            // would re-inflate memory on a future boot and waste durable
            // storage — prune it so the store shrinks toward the cap. Dropping
            // a cached package only costs a recoverable re-exchange, exactly
            // like the live eviction path. Overflow is deleted without loading
            // it, so peak memory stays cap-bounded.
            if self.pending_key_packages.len() >= MAX_PENDING_KEY_PACKAGES {
                if !budget.claim() {
                    // The map only grows and the cap only binds harder, so
                    // every remaining key would take this same branch. Nothing
                    // is left to restore and nothing more may be deleted.
                    break;
                }
                self.delete_peer_key_package_from_storage(&peer_id);
                over_cap_pruned += 1;
                continue;
            }
            if let Some(pkg) = self.load_peer_key_package_bounded(&peer_id, Some(&mut budget)) {
                info!(peer_id = %peer_id, "Restored peer key package from storage");
                self.pending_key_packages.insert(peer_id, pkg);
            }
        }
        if over_cap_pruned > 0 {
            warn!(
                cap = MAX_PENDING_KEY_PACKAGES,
                pruned = over_cap_pruned,
                "Peer key package store exceeded the cap on restore; pruned overflow from durable storage"
            );
        }
        // Routine, unlike the over-cap prune above: cached packages expire and
        // nothing else collects them, so this is the ordinary way the store
        // shrinks. Reported at debug so a normal launch stays quiet. Folded in
        // with the reader's own drops, which are neither routine nor a flood —
        // separating a third counter would say more about the log line than
        // about the store.
        let dropped = budget.spent.saturating_sub(over_cap_pruned);
        if dropped > 0 {
            debug!(
                pruned = dropped,
                "Dropped expired or unreadable peer key packages from durable storage on restore"
            );
        }
        if budget.exhausted {
            warn!(
                deleted = budget.spent,
                budget = MAX_RESTORE_PRUNE_DELETES,
                "Peer key package prune hit its share of the launch delete budget; the rest is left on disk for a later launch"
            );
        }
        if listed > MAX_RESTORE_KEYS_PER_CATEGORY {
            warn!(
                listed,
                cap = MAX_RESTORE_KEYS_PER_CATEGORY,
                "Peer key package store listed more peers than any legitimate run can produce; deferring the tail to a later launch"
            );
        }

        Ok(())
    }

    // ========================================================================
    // PEER CAPABILITIES PERSISTENCE
    // ========================================================================

    /// Persists the capability versions a peer advertised in its key package
    /// so they survive restarts (see [`PeerCapabilities`] for why this is a
    /// separate record from the key package itself). Best-effort like the
    /// other persist paths: a lost record only degrades output (JSON
    /// envelope, dropped rich extras) until the next live exchange.
    pub(crate) fn persist_peer_capabilities(&self, peer_id: &str, caps: &PeerCapabilities) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        match serde_json::to_vec(caps) {
            Ok(data) => {
                if let Err(e) = self.write_state_record(
                    storage.as_ref(),
                    storage_keys::PEER_CAPABILITIES,
                    peer_id,
                    &data,
                ) {
                    warn!(peer_id = %peer_id, error = %e, "Failed to persist peer capabilities");
                }
            }
            Err(e) => {
                warn!(peer_id = %peer_id, error = %e, "Failed to serialize peer capabilities");
            }
        }
    }

    /// Removes the persisted capability record for a peer (downgrade,
    /// eviction, or peer-state cleanup).
    pub(crate) fn delete_peer_capabilities_from_storage(&self, peer_id: &str) {
        if let Some(storage) = &self.protocol_state_storage {
            let _ = storage.delete(storage_keys::PEER_CAPABILITIES, peer_id);
        }
    }

    /// Loads the persisted capability record for a peer.
    ///
    /// Three answers, not two, for the same reason [`StateRecord`] has three.
    /// `Ok(None)` is "there is nothing here to merge with"; `Err(())` is "a
    /// record may well be here and could not be read *this session*". The only
    /// caller merges into whatever this returns and then writes the result
    /// back, so collapsing the two would let one transient provider failure
    /// write an attested-only record over a peer's *directly advertised*
    /// capabilities — silently downgrading its MLS envelope and dropping its
    /// rich extras until the next live key-package exchange.
    ///
    /// A record that is present but will not deserialize is `Ok(None)`:
    /// nothing can be merged with bytes that do not parse, and the write that
    /// follows is the recovery. Same for one the store itself destroyed.
    fn load_peer_capabilities(
        &self,
        peer_id: &str,
    ) -> std::result::Result<Option<PeerCapabilities>, ()> {
        let Some(storage) = self.protocol_state_storage.as_ref() else {
            return Ok(None);
        };
        match self.read_state_record_detailed(
            storage.as_ref(),
            storage_keys::PEER_CAPABILITIES,
            peer_id,
        ) {
            Ok(StateRecord::Present(data)) => Ok(serde_json::from_slice(&data).ok()),
            // Never written, or examined and destroyed: nothing to preserve.
            Ok(StateRecord::Missing | StateRecord::Unreadable) => Ok(None),
            // Still on disk and probably intact — merging into a default and
            // persisting that would destroy it.
            Ok(StateRecord::Unavailable) | Err(_) => Err(()),
        }
    }

    /// Records an inviter-attested rich-payload capability for a peer we
    /// never directly exchanged key packages with (a group member added by
    /// someone else). In-memory recording mirrors the direct path in
    /// `handle_key_package_message`: gated by our own kill switch and
    /// bounded like `key_package_sent_to`. Persistence stores the raw
    /// attested versions merged into the peer's existing capability record
    /// (never clobbering directly-advertised fields — which is why a record
    /// that cannot be *read* this session skips the write rather than merging
    /// into a default), matching the "switches gate use, not knowledge" rule.
    /// A later direct key-package exchange overwrites the whole record and
    /// evicts the in-memory entry — direct knowledge is always authoritative.
    pub(crate) fn record_attested_rich(&mut self, peer_id: &str, versions: &[u8]) {
        if peer_id == self.config.user_id || !versions.contains(&RICH_PAYLOAD_V1) {
            return;
        }
        // Direct self-advertisement already covers this peer; an attested
        // duplicate would only go stale.
        if self.peer_rich_payload.contains(peer_id) {
            return;
        }
        if self.config.encryption.rich_payload_enabled {
            if !self.peer_rich_attested.contains(peer_id)
                && self.peer_rich_attested.len() >= MAX_KEY_PACKAGE_SENT_TO
            {
                self.peer_rich_attested.clear();
            }
            self.peer_rich_attested.insert(peer_id.to_string());
        }
        let Ok(existing) = self.load_peer_capabilities(peer_id) else {
            // The record could not be read this session, so a write now would
            // put an attested-only record over whatever is on disk — including
            // the versions this peer advertised for itself, which are the
            // authoritative ones. An attestation is advisory and the in-memory
            // set above already opens the group gate for this run; the next Add
            // commit re-attests. Skipping the write is strictly cheaper than
            // losing a peer's real capabilities to a transient read.
            warn!(
                peer_id = %peer_id,
                "Peer capability record unreadable this session; not persisting the attested capability"
            );
            return;
        };
        let mut caps = existing.unwrap_or_default();
        caps.attested_rich_versions = versions
            .iter()
            .copied()
            .take(MAX_PERSISTED_CAPABILITY_VERSIONS)
            .collect();
        self.persist_peer_capabilities(peer_id, &caps);
    }

    /// Repopulates the in-memory capability sets (`peer_compact_envelope`,
    /// `peer_rich_payload`) from the durable per-peer records, so a send
    /// right after relaunch — before any live key-package exchange — keeps
    /// sealing rich extras and emitting the compact envelope.
    ///
    /// Config gating happens here, not at persist time: a record is only
    /// applied to a set whose kill switch is on, but a record gated off is
    /// left in storage (the switch may come back on next run). Records are
    /// bounded to `MAX_KEY_PACKAGE_SENT_TO` — the same cap the live insert
    /// path enforces on the sets — and overflow is pruned from durable
    /// storage like `restore_peer_key_packages` does, so a pre-existing
    /// over-cap store shrinks to the cap in a single boot. Session peers are
    /// admitted first: they are exactly the records this feature exists for
    /// (their key-package cache entry is gone), and `list_keys` order is
    /// backend-defined, so an unprioritized over-cap prune — reachable only
    /// after a forged-sender flood — could evict them while keeping forged
    /// leftovers. Best-effort: failures degrade output, never blocking
    /// restore.
    pub(crate) fn restore_peer_capabilities(
        &mut self,
        mls: &Arc<RwLock<MlsManager>>,
        allowance: &mut PruneAllowance,
    ) {
        let Some(storage) = self.protocol_state_storage.clone() else {
            return;
        };
        let peer_ids =
            match Self::list_state_keys(storage.as_ref(), storage_keys::PEER_CAPABILITIES) {
                Ok(ids) => ids,
                Err(e) => {
                    warn!(error = %e, "Failed to list peer capabilities, skipping restore");
                    return;
                }
            };

        // Best-effort session lookup: if it fails, restore proceeds in
        // backend order rather than not at all.
        let session_set: std::collections::HashSet<String> = mls
            .read()
            .ok()
            .and_then(|manager| manager.list_sessions().ok())
            .map(|sessions| sessions.into_iter().collect())
            .unwrap_or_default();
        let listed = peer_ids.len();
        let (mut peer_ids, non_session_ids): (Vec<_>, Vec<_>) = peer_ids
            .into_iter()
            .partition(|peer_id| session_set.contains(peer_id));
        peer_ids.extend(non_session_ids);

        let mut kept = 0usize;
        let mut over_cap_pruned = 0usize;
        let mut budget = allowance.refusing();
        // Bounded like `restore_peer_key_packages`, for the same reason: the
        // prune below is a provider delete per over-cap entry, and those are
        // budgeted separately from the read walk. The take comes *after* the
        // session-peer partition, so the records this restore exists for are
        // always inside the prefix however the backend ordered its listing.
        //
        // *Every* kind of delete this walk causes shares that budget, including
        // the one it does not issue itself: a record the store reports corrupt,
        // or one over the record cap, is dropped inside the reader. Handing the
        // budget to the reader is the only way to count those — the same
        // correction `restore_media_descriptors` needed, for the same reason.
        for peer_id in peer_ids.into_iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            if kept >= MAX_KEY_PACKAGE_SENT_TO {
                if !budget.claim() {
                    // `kept` only grows, so every remaining key would take this
                    // same branch: nothing left to restore, nothing more that
                    // may be deleted.
                    break;
                }
                self.delete_peer_capabilities_from_storage(&peer_id);
                over_cap_pruned += 1;
                continue;
            }
            let Ok(StateRecord::Present(data)) = self.read_state_record_detailed_budgeted(
                storage.as_ref(),
                storage_keys::PEER_CAPABILITIES,
                &peer_id,
                Some(&mut budget),
            ) else {
                continue;
            };
            let Ok(caps) = serde_json::from_slice::<PeerCapabilities>(&data) else {
                // Corrupt record: drop it rather than re-parsing it forever —
                // unless the delete budget is gone, in which case a later
                // launch drops it instead. Re-parsing one record is cheap; the
                // device barrier a delete costs is not.
                warn!(peer_id = %peer_id, "Corrupt peer capability record, deleting");
                if budget.claim() {
                    self.delete_peer_capabilities_from_storage(&peer_id);
                }
                continue;
            };
            if !caps.is_any() {
                // Empty records are deleted at persist time; clean up any
                // that predate that rule.
                if budget.claim() {
                    self.delete_peer_capabilities_from_storage(&peer_id);
                }
                continue;
            }
            if self.config.encryption.compact_envelope_enabled
                && caps.env_versions.contains(&MLS_ENVELOPE_COMPACT_V1)
            {
                self.peer_compact_envelope.insert(peer_id.clone());
            }
            if self.config.encryption.rich_payload_enabled
                && caps.rich_versions.contains(&RICH_PAYLOAD_V1)
            {
                self.peer_rich_payload.insert(peer_id.clone());
            }
            if self.config.encryption.rich_payload_enabled
                && caps.attested_rich_versions.contains(&RICH_PAYLOAD_V1)
            {
                self.peer_rich_attested.insert(peer_id.clone());
            }
            kept += 1;
        }
        if kept > 0 {
            info!(
                count = kept,
                "Restored peer capability records from storage"
            );
        }
        if over_cap_pruned > 0 {
            warn!(
                cap = MAX_KEY_PACKAGE_SENT_TO,
                pruned = over_cap_pruned,
                "Peer capability store exceeded the cap on restore; pruned overflow from durable storage"
            );
        }
        // Reported apart from the over-cap prune, like `restore_peer_key_packages`
        // separates its expiry drops: a corrupt, empty, or unreadable record is
        // not a store that outgrew its cap, and attributing it to one sends
        // whoever reads this log looking for a flood that never happened.
        let unreadable_pruned = budget.spent.saturating_sub(over_cap_pruned);
        if unreadable_pruned > 0 {
            debug!(
                pruned = unreadable_pruned,
                "Dropped unreadable peer capability records from durable storage on restore"
            );
        }
        if budget.exhausted {
            warn!(
                deleted = budget.spent,
                budget = MAX_RESTORE_PRUNE_DELETES,
                "Peer capability prune hit its share of the launch delete budget; the rest is left on disk for a later launch"
            );
        }
        if listed > MAX_RESTORE_KEYS_PER_CATEGORY {
            warn!(
                listed,
                cap = MAX_RESTORE_KEYS_PER_CATEGORY,
                "Peer capability store listed more peers than any legitimate run can produce; deferring the tail to a later launch"
            );
        }
    }

    // ========================================================================
    // RESTORE-PATH RECORD DECODING
    // ========================================================================

    /// Reads and decodes one JSON protocol-state record *for a restore walk*,
    /// dropping a record whose bytes are not this category's type rather than
    /// failing the walk.
    ///
    /// A record that parses as garbage is the same permanent loss as one that
    /// fails to open, and every other restore on this path already treats it
    /// that way ([`Self::restore_outbox`], [`Self::restore_pending_messages`],
    /// [`Self::restore_peer_capabilities`]). Session states and Welcome
    /// lifecycles were the two that did not: their loaders map a serde failure
    /// to an error, and their restores propagate it, so one unparseable record
    /// failed `initialize_mls` outright — and, because nothing deleted it,
    /// failed it again on every launch after that. With `require_encryption`
    /// on by default that install can no longer send anything, and there is no
    /// in-app recovery. Both categories are unsealed, so they carry no
    /// integrity protection at all, and both now live in the app container
    /// rather than the credential store — the same threat model every restore
    /// walk here is bounded against.
    ///
    /// Deliberately *not* folded into the loaders themselves. Those are also
    /// read at runtime, where `is_session_confirmed` propagates the error on
    /// purpose so a send fails closed instead of silently reading a Confirmed
    /// session as Pending. Restore is the only caller that must survive the
    /// record; the runtime one must not.
    ///
    /// A **storage failure no longer fails the walk either**, and that is the
    /// other half of the same lesson. Fixing only the serde case left the
    /// identical outcome reachable through the store: `list_keys` succeeds,
    /// one record's read returns `LoadFailed`, and `initialize_mls` rolls back
    /// — every launch, for as long as that one file stays unreadable, on an
    /// install that can then never send. Every other category on this path
    /// already treats a per-record read failure as recoverable and continues
    /// ([`Self::restore_outbox`], [`Self::load_legacy_pending_queue`],
    /// [`Self::restore_media_descriptors`], [`Self::load_peer_capabilities`]).
    /// These two were the outliers, and moving both categories out of the
    /// credential store and into the app container — where `ENOSPC`, `EIO`, and
    /// protection-class failures are ordinary — is what made it matter.
    ///
    /// So this reader is infallible and answers three ways.
    /// [`RestorableRecord::Unavailable`] is the load-bearing one: the caller
    /// must not read it as absence, because for session states "absent" means
    /// *re-bootstrap and persist `Pending`*, which would write over a record
    /// that may say `Confirmed`. A *listing* failure still propagates, from the
    /// restores themselves — it is indistinguishable from an empty category and
    /// has no per-record fallback.
    ///
    /// `budget` bounds the durable deletes this causes — the drop below, and the
    /// reader's own drop of a record that will not open. Both categories reached
    /// through here are unsealed and advisory-to-restore, so a refusing budget
    /// is right: a record the budget spares is simply re-walked and dropped on a
    /// later launch, exactly like the cache prunes. Callers whose walk is
    /// bounded by something other than the store
    /// (`restore_session_states_from_manager` iterates the MLS session list)
    /// pass `None`.
    fn load_restorable_state_record<T: DeserializeOwned>(
        &self,
        key_type: &str,
        key_id: &str,
        mut budget: Option<&mut PruneBudget<'_>>,
    ) -> RestorableRecord<T> {
        let Some(storage) = &self.protocol_state_storage else {
            return RestorableRecord::Absent;
        };

        let data = match self.read_state_record_detailed_budgeted(
            storage.as_ref(),
            key_type,
            key_id,
            budget.as_deref_mut(),
        ) {
            Ok(StateRecord::Present(data)) => data,
            // Never written, or examined and destroyed: either way there is
            // nothing here to recover and the caller may re-bootstrap.
            Ok(StateRecord::Missing | StateRecord::Unreadable) => return RestorableRecord::Absent,
            // Still on disk and quite possibly intact.
            Ok(StateRecord::Unavailable) => return RestorableRecord::Unavailable,
            // `debug`, not `warn`: a store failing systemically fails *every*
            // read, and this reader runs once per record on a walk bounded at
            // `MAX_RESTORE_KEYS_PER_CATEGORY`. A warn here is up to that many
            // lines on the synchronous boot path, in exactly the degraded state
            // this three-way answer exists to survive. Each caller emits the
            // operator-facing signal itself and aggregates it:
            // `restore_welcome_lifecycles` counts them into one warn, and
            // `restore_session_states_from_manager` walks the far smaller MLS
            // session list and can afford one per peer.
            Err(e) => {
                debug!(
                    key_type = %key_type,
                    key_id = %key_id,
                    error = %e,
                    "Protocol state record could not be read this session; leaving it in place"
                );
                return RestorableRecord::Unavailable;
            }
        };

        match serde_json::from_slice::<T>(&data) {
            Ok(value) => RestorableRecord::Present(value),
            Err(e) => {
                warn!(
                    key_type = %key_type,
                    key_id = %key_id,
                    error = %e,
                    "Dropping a protocol-state record whose bytes are not the type \
                     its category holds"
                );
                if claim_prune(&mut budget) {
                    let _ = storage.delete(key_type, key_id);
                }
                RestorableRecord::Absent
            }
        }
    }

    /// Restore-path read of a peer's session state. See
    /// [`Self::load_restorable_state_record`] for why this is separate from
    /// [`Self::load_session_state_entry`].
    ///
    /// Budgeted, though its caller walks the *MLS session list* rather than a
    /// protocol-state category. The walk bound was once argued to be enough on
    /// the grounds that the volume is bounded by sessions this install actually
    /// has — but the MLS session list carries no restore cap of its own, so
    /// "bounded" there means "bounded by the peer count", and a store that
    /// reports every session-state record corrupt (or holds bytes that will not
    /// decode) turns that into one device barrier per peer on the boot path
    /// with nothing counting them. That is the same argument-from-the-wrong-
    /// number that exempted `restore_outbox`.
    ///
    /// A [`PruneAllowance::refusing`] budget is right here, and refusing costs
    /// less than anywhere else on the path: a spared record still reads
    /// [`StateRecord::Unreadable`] → [`RestorableRecord::Absent`], so the
    /// caller re-bootstraps and persists a fresh `Pending` **over** it. The
    /// record is repaired by that write whether or not the delete was funded —
    /// which makes refusing strictly cheaper than claiming, since it saves a
    /// barrier rather than deferring one.
    pub(super) fn load_session_state_for_restore(
        &self,
        peer_id: &str,
        budget: Option<&mut PruneBudget<'_>>,
    ) -> RestorableRecord<SessionState> {
        self.load_restorable_state_record(storage_keys::SESSION_STATES, peer_id, budget)
    }

    /// Reads a peer's Welcome lifecycle. See
    /// [`Self::load_restorable_state_record`].
    ///
    /// Unlike session states, this category has no strict twin: restore is its
    /// only reader (`welcome_lifecycles` is an in-memory map from then on), so
    /// there is no send-path decision that has to fail closed on a record that
    /// will not decode. It *is* walked by container-listed key, though, so its
    /// deletes are budgeted like every other such walk.
    fn load_welcome_lifecycle_for_restore(
        &self,
        peer_id: &str,
        budget: Option<&mut PruneBudget<'_>>,
    ) -> RestorableRecord<WelcomeLifecycleRecord> {
        self.load_restorable_state_record(storage_keys::WELCOME_LIFECYCLES, peer_id, budget)
    }

    // ========================================================================
    // SESSION STATE PERSISTENCE
    // ========================================================================

    /// Loads a persisted session state entry (if present).
    ///
    /// A record that will not deserialize is an error here, not an absent
    /// record: `is_session_confirmed` reads this on the send path and must
    /// fail closed rather than treat a Confirmed session as Pending. The
    /// restore walk, which must not be blocked by one poison record, goes
    /// through [`Self::load_session_state_for_restore`] instead.
    pub(crate) fn load_session_state_entry(&self, peer_id: &str) -> Result<Option<SessionState>> {
        let Some(storage) = &self.protocol_state_storage else {
            return Ok(None);
        };

        let Some(data) = self
            .read_state_record(storage.as_ref(), storage_keys::SESSION_STATES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to load session state for {}: {}",
                    peer_id, e
                ))
            })?
        else {
            return Ok(None);
        };

        let state = serde_json::from_slice::<SessionState>(&data).map_err(|e| {
            Error::Other(format!(
                "Failed to deserialize session state for {}: {}",
                peer_id, e
            ))
        })?;

        Ok(Some(state))
    }

    /// Persists session state for a single peer key.
    pub(crate) fn persist_session_state(
        &self,
        peer_id: &str,
        new_state: SessionState,
        source_event: &str,
    ) -> Result<()> {
        let Some(storage) = &self.protocol_state_storage else {
            return Err(Error::MlsNotInitialized);
        };

        let encoded = serde_json::to_vec(&new_state).map_err(|e| {
            Error::Serialization(format!("Failed to serialize session state: {}", e))
        })?;
        self.write_state_record(
            storage.as_ref(),
            storage_keys::SESSION_STATES,
            peer_id,
            &encoded,
        )
        .map_err(|e| {
            Error::Other(format!(
                "Failed to persist session state for {}: {}",
                peer_id, e
            ))
        })?;

        if matches!(new_state, SessionState::Confirmed) {
            info!(
                event = "confirmation_persisted",
                session_or_group_id = %peer_id,
                previous_state = "Pending",
                new_state = "Confirmed",
                source_event = %source_event,
                "confirmation_persisted"
            );
        }

        Ok(())
    }

    pub(crate) fn clear_session_state_entry(&self, peer_id: &str) -> Result<()> {
        let Some(storage) = &self.protocol_state_storage else {
            return Ok(());
        };
        storage
            .delete(storage_keys::SESSION_STATES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to clear session state for {}: {}",
                    peer_id, e
                ))
            })
    }

    // ========================================================================
    // WELCOME LIFECYCLE PERSISTENCE
    // ========================================================================

    pub(crate) fn persist_welcome_lifecycle_entry(
        &self,
        record: &WelcomeLifecycleRecord,
    ) -> Result<()> {
        let Some(storage) = &self.protocol_state_storage else {
            return Err(Error::MlsNotInitialized);
        };

        let encoded = serde_json::to_vec(record).map_err(|e| {
            Error::Serialization(format!("Failed to serialize welcome lifecycle: {}", e))
        })?;
        self.write_state_record(
            storage.as_ref(),
            storage_keys::WELCOME_LIFECYCLES,
            &record.peer_id,
            &encoded,
        )
        .map_err(|e| {
            Error::Other(format!(
                "Failed to persist welcome lifecycle for {}: {}",
                record.peer_id, e
            ))
        })
    }

    pub(crate) fn clear_welcome_lifecycle_entry(&self, peer_id: &str) -> Result<()> {
        let Some(storage) = &self.protocol_state_storage else {
            return Ok(());
        };
        storage
            .delete(storage_keys::WELCOME_LIFECYCLES, peer_id)
            .map_err(|e| {
                Error::Other(format!(
                    "Failed to clear welcome lifecycle for {}: {}",
                    peer_id, e
                ))
            })
    }

    /// Brings one restored Welcome lifecycle back to a state the retry ladder
    /// can act on, and reports whether anything changed.
    ///
    /// Split out so the walk can persist the result **once**, non-fatally. The
    /// repairs used to persist inline with `?`, which made a transient
    /// `StoreFailed` on any one of them fail `initialize_mls` — a persistence
    /// failure blocking restore, where every sibling path in this module logs
    /// and carries on. The in-memory map is what drives retries; a repair that
    /// could not be written is simply re-derived from the same record next
    /// launch.
    fn repair_restored_welcome_lifecycle(
        peer_id: &str,
        record: &mut WelcomeLifecycleRecord,
    ) -> bool {
        let mut repaired = false;
        if matches!(
            record.state,
            WelcomeDeliveryState::Created | WelcomeDeliveryState::SendAttempted
        ) {
            record.state = WelcomeDeliveryState::Failed;
            record.next_retry_at = Some(Utc::now());
            repaired = true;
            warn!(
                event = "welcome_lifecycle_repaired",
                session_or_group_id = %peer_id,
                repair_action = "in_flight_to_failed_retry_now",
                state = record.state.as_str(),
                attempt = record.attempt,
                "welcome_lifecycle_repaired"
            );
        }
        if matches!(record.state, WelcomeDeliveryState::Failed) && record.next_retry_at.is_none() {
            if matches!(
                record.last_reason_code,
                Some(crate::events::WelcomeReasonCode::RetryExhausted)
            ) {
                // Only genuine retry exhaustion (a present carrier that kept
                // failing) is terminal here. A stale TTL alone must NOT expire a
                // no-carrier Welcome on restart — the TTL clock is
                // carrier-relative and is refreshed below.
                record.state = WelcomeDeliveryState::Expired;
                warn!(
                    event = "welcome_lifecycle_repaired",
                    session_or_group_id = %peer_id,
                    repair_action = "failed_no_retry_to_expired",
                    state = record.state.as_str(),
                    attempt = record.attempt,
                    "welcome_lifecycle_repaired"
                );
            } else {
                // Recover from partial-crash write where Failed was persisted
                // without a retry schedule.
                record.next_retry_at = Some(Utc::now());
                warn!(
                    event = "welcome_lifecycle_repaired",
                    session_or_group_id = %peer_id,
                    repair_action = "failed_no_retry_to_failed_retry_now",
                    state = record.state.as_str(),
                    attempt = record.attempt,
                    "welcome_lifecycle_repaired"
                );
            }
            repaired = true;
        }
        // The TTL clock is carrier-relative: a Welcome must not be restored
        // already-expired after an offline period. Restart the window for any
        // non-terminal lifecycle whose TTL has lapsed so it gets a fresh chance
        // once a carrier (or the peer) reappears.
        if matches!(record.state, WelcomeDeliveryState::Failed) && record.expires_at <= Utc::now() {
            record.expires_at = Utc::now() + ChronoDuration::seconds(WELCOME_LIFECYCLE_TTL_SECS);
            repaired = true;
            warn!(
                event = "welcome_lifecycle_repaired",
                session_or_group_id = %peer_id,
                repair_action = "ttl_refreshed_carrier_relative",
                state = record.state.as_str(),
                attempt = record.attempt,
                "welcome_lifecycle_repaired"
            );
        }
        if matches!(
            record.state,
            WelcomeDeliveryState::Sent | WelcomeDeliveryState::Expired
        ) && record.next_retry_at.is_some()
        {
            record.next_retry_at = None;
            repaired = true;
            warn!(
                event = "welcome_lifecycle_repaired",
                session_or_group_id = %peer_id,
                repair_action = "terminal_clear_retry_schedule",
                state = record.state.as_str(),
                attempt = record.attempt,
                "welcome_lifecycle_repaired"
            );
        }
        repaired
    }

    pub(crate) fn restore_welcome_lifecycles(
        &mut self,
        allowance: &mut PruneAllowance,
    ) -> Result<()> {
        self.welcome_lifecycles.clear();
        let Some(storage) = &self.protocol_state_storage else {
            return Ok(());
        };

        let peers = Self::list_state_keys(storage.as_ref(), storage_keys::WELCOME_LIFECYCLES)
            .map_err(|e| Error::Other(format!("Failed to list welcome lifecycles: {}", e)))?;
        let listed = peers.len();

        // Bounded like every other container-listed walk: a record that will not
        // decode is dropped, and that drop is a provider delete with a directory
        // flush behind it. The tail waits for a later launch, which is safe for
        // the same reason it is safe for the cache prunes — dropping a Welcome
        // lifecycle is recoverable, and re-reading one costs a parse. Draws on
        // the launch-wide advisory pool, so this walk and the three cache walks
        // add up against one allowance.
        let mut budget = allowance.refusing();
        let mut unavailable = 0usize;
        for peer_id in peers.into_iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            let mut record =
                match self.load_welcome_lifecycle_for_restore(&peer_id, Some(&mut budget)) {
                    RestorableRecord::Present(record) => record,
                    RestorableRecord::Absent => continue,
                    // Left on disk for a later launch. Skipping one peer costs a
                    // Welcome retry this session; failing the whole restore —
                    // which propagating here used to do — costs the install
                    // every send it would make, on every launch, for as long as
                    // that one record stays unreadable.
                    RestorableRecord::Unavailable => {
                        unavailable += 1;
                        continue;
                    }
                };
            if Self::repair_restored_welcome_lifecycle(&peer_id, &mut record) {
                if let Err(e) = self.persist_welcome_lifecycle_entry(&record) {
                    warn!(
                        session_or_group_id = %peer_id,
                        error = %e,
                        "Failed to persist a repaired Welcome lifecycle; the repair holds in \
                         memory and is re-derived on the next launch"
                    );
                }
            }
            self.welcome_lifecycles.insert(peer_id.clone(), record);
            info!(
                event = "welcome_lifecycle_restored",
                session_or_group_id = %peer_id,
                "welcome_lifecycle_restored"
            );
        }

        if unavailable > 0 {
            warn!(
                unavailable,
                "Welcome lifecycle records could not be read this session; left on disk for a \
                 later launch"
            );
        }
        if listed > MAX_RESTORE_KEYS_PER_CATEGORY {
            warn!(
                listed,
                cap = MAX_RESTORE_KEYS_PER_CATEGORY,
                "Welcome lifecycle store listed more peers than any legitimate run can produce; ignoring the tail"
            );
        }
        if budget.exhausted {
            warn!(
                deleted = budget.spent,
                budget = MAX_RESTORE_PRUNE_DELETES,
                "Welcome lifecycle restore hit its share of the launch delete budget; the rest is left on disk for a later launch"
            );
        }

        Ok(())
    }

    // ========================================================================
    // OUTBOX PERSISTENCE
    // ========================================================================

    /// Persists a single outbox entry to storage, keyed by message id.
    ///
    /// Best-effort and infallible: the send path cannot propagate storage
    /// errors, so a failed write is logged and swallowed (the message still
    /// lives in the in-memory outbox and will retry; it just won't survive a
    /// restart). No-ops when persistence is not configured or when the entry
    /// belongs to the media outbox — file transfers are not persisted and
    /// resurrected chunks could never complete, so we never write them.
    pub(crate) fn persist_outbox_entry(&self, entry: &OutboxEntry) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        if Self::is_media_outbox_message(&entry.message) {
            return;
        }
        match serde_json::to_vec(entry) {
            Ok(data) => {
                if let Err(e) = self.write_state_record(
                    storage.as_ref(),
                    storage_keys::OUTBOX,
                    &entry.message.id.as_str(),
                    &data,
                ) {
                    warn!(message_id = %entry.message.id, error = %e, "Failed to persist outbox entry");
                }
            }
            Err(e) => {
                warn!(message_id = %entry.message.id, error = %e, "Failed to serialize outbox entry");
            }
        }
    }

    /// Removes a persisted outbox entry from storage. Best-effort: a media
    /// message id is never persisted, so deleting it is a harmless no-op.
    pub(crate) fn clear_outbox_entry_from_storage(&self, message_id: &MessageId) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        if let Err(e) = storage.delete(storage_keys::OUTBOX, &message_id.as_str()) {
            warn!(message_id = %message_id, error = %e, "Failed to clear persisted outbox entry");
        }
    }

    /// Restores the store-and-forward outbox from storage on startup.
    ///
    /// Merges persisted entries into `self.outbox` — it is *not* cleared first.
    /// An entry queued before persistence was enabled lives only in memory and,
    /// if it succeeded at the transport but is awaiting an ACK, is not in the
    /// retry queue either; clearing would strand it with no recovery path (the
    /// ACK-timeout path drops a message that is missing from the outbox). Where
    /// an id exists in both, storage is authoritative and overwrites.
    ///
    /// The retry queue and ACK manager start empty, so every restored entry
    /// lands in the "stranded" state that [`Self::flush_outbox_all`] already
    /// recovers — a flush on `start()` re-drives delivery.
    ///
    /// Recovery rules, mirroring the other restore paths:
    /// - corrupted entries are dropped from storage and skipped;
    /// - any stray media entry is dropped (they must never be resurrected);
    /// - the total is pruned to `MAX_OUTBOX_ENTRIES`, keeping the newest by
    ///   `last_sent_at`;
    /// - an entry whose total age (from `first_sent_at`) exceeds
    ///   [`OUTBOX_ABSOLUTE_LIFETIME_FACTOR`] × the outbox lifetime is dropped
    ///   terminally with a `message_failed`, so repeated restarts can't
    ///   re-grant a fresh window forever;
    /// - otherwise the TTL clock is carrier-relative: an entry whose
    ///   `last_sent_at` has already lapsed the outbox lifetime is refreshed
    ///   rather than restored already-expired, so it gets a fresh delivery
    ///   window once a carrier reappears (mirrors the Welcome lifecycle
    ///   repair). This runs *after* the prune, so a refreshed (now-stamped)
    ///   clock can't sort as the newest and crowd genuinely-fresh entries out
    ///   of the kept set;
    /// - pre-existing in-memory entries not yet in storage are persisted, so
    ///   memory and storage are consistent once restore returns.
    ///
    /// Budgeted by [`MAX_RESTORE_PRUNE_DELETES`] the way
    /// [`Self::restore_pending_messages`] is, and for the same reason: every
    /// delete here is paired with a terminal `message_failed`, so an individual
    /// one can never be *refused* — skipping the delete while settling would
    /// settle an id whose record the next launch restores and re-drives, and
    /// skipping both would drop the entry from memory while leaving the
    /// application holding an id nothing ever resolves. What the pairing does
    /// not forbid is **stopping between records**, which is what this walk does
    /// — and what its two post-walk prunes do between *entries*.
    ///
    /// [`OUTBOX_RESTORE_KEY_CAP`] alone was once argued to be enough here, on
    /// the grounds that it caps the walk near 1.5k deletes. Two things were
    /// wrong with that. The number came from the capacity prune, not the walk:
    /// the outbox is a *sealed* category, so the wrong-length record-key branch
    /// of [`Self::restore_or_init_state_record_key`] makes every entry on the
    /// install unopenable at once and each one takes the reader's drop — the
    /// full walk bound, in device barriers, on the synchronous boot path. And
    /// the number it *did* describe was never bounded either: the capacity
    /// drain and the absolute-lifetime drop run after the walk, on entries it
    /// admitted, so a store whose records all open cleanly reaches them with
    /// the pool untouched. All three now draw on the same pool and stop at an
    /// entry boundary; see [`MAX_RESTORE_PRUNE_DELETES`].
    ///
    /// It draws on its **own** pool rather than the launch-wide advisory one:
    /// an outbox record left unwalked is one the application holds a *live* id
    /// for, so deferring it defers delivery, and that must not be a consequence
    /// of an unrelated key-package flood spending the shared allowance first.
    /// No freeze is needed for the tail — outbox records are keyed per message
    /// id, so a later write only ever touches a different key.
    ///
    /// `allowance` is this walk's own pool, built by the launch — see
    /// [`PruneAllowance::pool`].
    pub(crate) fn restore_outbox(&mut self, allowance: &mut PruneAllowance) -> Result<()> {
        // Cloned rather than borrowed: the loop below both reads through this
        // handle and calls `&mut self` settlement paths, so holding a borrow of
        // `self` across it is what forced the awkward per-iteration re-fetch
        // this replaced.
        let Some(storage) = self.protocol_state_storage.clone() else {
            return Ok(());
        };

        let message_ids = Self::list_state_keys(storage.as_ref(), storage_keys::OUTBOX)
            .map_err(|e| Error::Other(format!("Failed to list outbox entries: {}", e)))?;

        let lifetime_ms = self.config.reliability.retry.outbox_max_lifetime_ms;
        let listed = message_ids.len();

        let mut restored: Vec<OutboxEntry> = Vec::new();
        // Record keys the app was told were queued but that cannot be
        // recovered. Unlike the pending queue, an outbox record is keyed *by*
        // its message id, so each loss is individually settleable without
        // opening it — kept as the raw key so one that does not parse as a
        // `MessageId` still gets a diagnostic instead of silence.
        let mut unrecoverable: Vec<String> = Vec::new();
        // Settlement-paired like the pending walk, and budgeted the same way:
        // counting, never refusing, stopping at a record boundary. Its own pool
        // rather than the advisory one, because starving this walk defers
        // *delivery* of every message the app still holds a live id for.
        //
        // The walk bound alone is not the ceiling it was argued to be. The
        // outbox is a sealed category, so a regenerated record key makes every
        // entry on the install fail to open at once and each one takes the
        // reader's drop below — `OUTBOX_RESTORE_KEY_CAP` device barriers, on the
        // synchronous boot path, with nothing counting them. That is verbatim
        // the case that put a budget on `restore_pending_messages`.
        //
        // No freeze is needed for the tail this stops at, unlike the pending
        // walk: outbox records are keyed per message id, so every later write
        // touches a different key and an unwalked record is simply restored and
        // re-driven on the next launch.
        //
        // The pool is passed in rather than built here — see
        // `PruneAllowance::pool`.
        let mut budget = allowance.counting();
        let mut prune_bound_reached = false;
        for message_id in message_ids.into_iter().take(OUTBOX_RESTORE_KEY_CAP) {
            if budget.is_spent() {
                prune_bound_reached = true;
                break;
            }
            let data = match self.read_state_record_detailed_budgeted(
                storage.as_ref(),
                storage_keys::OUTBOX,
                &message_id,
                Some(&mut budget),
            ) {
                Ok(StateRecord::Present(data)) => data,
                Ok(StateRecord::Missing) => continue,
                Ok(StateRecord::Unreadable) => {
                    warn!(message_id = %message_id, "Dropping unreadable outbox entry");
                    unrecoverable.push(message_id);
                    continue;
                }
                // Still on disk and probably intact — the record key is not
                // loaded, or the backend refused this read. Settling it as
                // failed would be a terminal answer the next launch overturns
                // by restoring the entry and re-driving delivery, so the app
                // would see `message_failed` and then a delivery (plus a second
                // copy if the user re-sent by hand, which mints a new id that
                // dedup cannot collapse). Leave it be.
                Ok(StateRecord::Unavailable) | Err(_) => {
                    warn!(
                        message_id = %message_id,
                        "Outbox entry could not be read this session; leaving it in place"
                    );
                    continue;
                }
            };

            let entry = match serde_json::from_slice::<OutboxEntry>(&data) {
                Ok(entry) => entry,
                Err(e) => {
                    warn!(message_id = %message_id, error = %e, "Dropping corrupted outbox entry");
                    // Charged, never refused: the settlement below is already
                    // owed, so the record has to go with it.
                    budget.claim();
                    self.delete_outbox_key(&message_id);
                    unrecoverable.push(message_id);
                    continue;
                }
            };

            // A media entry should never have been persisted; drop any that
            // slipped in (e.g. from an older build) so it can't be resurrected.
            if Self::is_media_outbox_message(&entry.message) {
                warn!(message_id = %message_id, "Dropping persisted media outbox entry");
                budget.claim();
                self.delete_outbox_key(&message_id);
                continue;
            }

            restored.push(entry);
        }

        // Drop entries past the absolute lifetime cap before anything else:
        // the carrier-relative refresh below would otherwise re-grant them a
        // fresh window on every restart, indefinitely. This is a terminal
        // drop — emit `message_failed` like the in-process expiry does.
        let absolute_lifetime_ms =
            lifetime_ms.saturating_mul(crate::constants::OUTBOX_ABSOLUTE_LIFETIME_FACTOR as u64);
        let now = Utc::now();
        let mut absolutely_expired: Vec<OutboxEntry> = Vec::new();
        restored.retain(|entry| {
            if lifetime_expired(now, entry.last_sent_at, lifetime_ms)
                && lifetime_expired(now, entry.first_sent_at, absolute_lifetime_ms)
            {
                absolutely_expired.push(entry.clone());
                return false;
            }
            true
        });
        // Counted and never refused *mid-entry* — this delete is inseparable
        // from the `message_failed` that pairs with it — but stopped *between*
        // entries, exactly as the walk stops between records.
        //
        // The gate is load-bearing rather than tidy. This loop and the capacity
        // drain below act on entries the walk *admitted*, so a store whose
        // records all open cleanly reaches them with the pool untouched and
        // `restored` bounded only by `OUTBOX_RESTORE_KEY_CAP`. Ungated, that is
        // up to that many device barriers in a single launch — a device offline
        // past the absolute lifetime with a full outbox, which is the *ordinary*
        // over-capacity case rather than the tampered one.
        //
        // An entry the pool cannot fund is dropped from memory and left on disk
        // **unsettled**. That is what makes stopping safe: nothing has told the
        // application anything about it, so a later launch restores it, re-ages
        // it, and owns both halves of the settlement then — the same deferral
        // the walk's own early break relies on.
        let mut expiry_settlements: Vec<Event> = Vec::new();
        for entry in &absolutely_expired {
            if budget.is_spent() {
                prune_bound_reached = true;
                break;
            }
            budget.claim();
            self.delete_outbox_key(&entry.message.id.as_str());
            info!(
                event = "outbox_entry_dropped",
                message_id = %entry.message.id,
                repair_action = "absolute_lifetime_exceeded",
                "outbox_entry_dropped"
            );
            expiry_settlements.push(Event::message_failed(
                entry.message.id.clone(),
                "Outbox lifetime exceeded".to_string(),
                entry.attempt_count,
            ));
        }
        // Batched: this is one of the two loops `settle_restored_message_failures`
        // exists for. A store past its cap drives it into the hundreds, and the
        // per-event form takes the shared-state lock once each.
        self.settle_restored_message_failures(expiry_settlements);

        // Prune to capacity BEFORE refreshing TTLs, keeping the newest by
        // last_sent_at. Delete the pruned overflow from storage so it can't
        // linger and be re-restored. Ordering matters: refreshing a lapsed
        // clock stamps it with `now`, which would otherwise sort it as the
        // newest and crowd genuinely-fresh entries out of the kept set.
        if restored.len() > MAX_OUTBOX_ENTRIES {
            restored.sort_by_key(|e| std::cmp::Reverse(e.last_sent_at));
            // `drain` removes its whole range when the iterator is dropped, so
            // breaking early still takes every over-cap entry out of memory.
            // That is what keeps the cap absolute: deferring a *delete* must
            // never defer the cap itself, or an over-cap store would re-inflate
            // memory on boot. The entries the pool could not fund are left on
            // disk unsettled and re-capped by a later launch, exactly like the
            // tail the walk never reached.
            //
            // Unbudgeted, this drain alone issues
            // `OUTBOX_RESTORE_KEY_CAP - MAX_OUTBOX_ENTRIES` deletes in one
            // launch — the very figure this walk's old budget exemption was
            // argued from. Counted, never refused mid-entry, stopped between
            // entries: see the absolute-expiry drop above.
            let mut capacity_settlements: Vec<Event> = Vec::new();
            for entry in restored.drain(MAX_OUTBOX_ENTRIES..) {
                if budget.is_spent() {
                    prune_bound_reached = true;
                    break;
                }
                budget.claim();
                self.delete_outbox_key(&entry.message.id.as_str());
                // Terminal, like the pending queue's capacity eviction: the app
                // holds this id and nothing will ever resolve it otherwise.
                capacity_settlements.push(Event::message_failed(
                    entry.message.id.clone(),
                    "Outbox capacity exceeded".to_string(),
                    entry.attempt_count,
                ));
            }
            // Batched for the same reason as the expiry settlements above.
            self.settle_restored_message_failures(capacity_settlements);
        }

        self.settle_restored_message_failures(unrecoverable.iter().map(|key| {
            Self::unrecoverable_message_settlement(
                key,
                "Outbox entry could not be recovered from protocol-state storage",
                0,
            )
        }));
        if listed > OUTBOX_RESTORE_KEY_CAP {
            warn!(
                listed,
                cap = OUTBOX_RESTORE_KEY_CAP,
                "Outbox store listed more entries than any legitimate run can produce; ignoring the tail"
            );
        }
        // Covers all three places this walk can stop: the read loop, the
        // absolute-expiry drop, and the capacity drain. `budget.exhausted` is
        // unreachable while each of those checks `is_spent()` before claiming —
        // it is ORed in so a future ungated claim still surfaces rather than
        // spending the pool in silence.
        if prune_bound_reached || budget.exhausted {
            warn!(
                deleted = budget.spent,
                budget = MAX_RESTORE_PRUNE_DELETES,
                "Outbox restore spent its delete budget; the remaining entries are left on disk \
                 and restored on a later launch"
            );
        }

        // Carrier-relative TTL: refresh any entry already past the outbox
        // lifetime so it survives the first cleanup tick and gets a fresh chance
        // once a carrier appears. Collect the refreshed clones so the repair can
        // be re-persisted below, once the mutable borrow of `restored` is gone.
        let mut refreshed: Vec<OutboxEntry> = Vec::new();
        for entry in &mut restored {
            if lifetime_expired(now, entry.last_sent_at, lifetime_ms) {
                entry.last_sent_at = now;
                refreshed.push(entry.clone());
                info!(
                    event = "outbox_entry_restored",
                    message_id = %entry.message.id,
                    repair_action = "ttl_refreshed_carrier_relative",
                    "outbox_entry_restored"
                );
            }
        }

        // Pre-existing in-memory entries not backed by storage (queued before
        // persistence was enabled) must be persisted too, so they survive the
        // next restart and memory/storage stay consistent after restore.
        let restored_ids: std::collections::HashSet<String> =
            restored.iter().map(|e| e.message.id.as_str()).collect();
        let orphans: Vec<OutboxEntry> = self
            .outbox
            .values()
            .filter(|e| !restored_ids.contains(&e.message.id.as_str()))
            .cloned()
            .collect();

        let count = restored.len();
        for entry in restored {
            self.outbox.insert(entry.message.id.clone(), entry);
        }
        for entry in &refreshed {
            self.persist_outbox_entry(entry);
        }
        for entry in &orphans {
            self.persist_outbox_entry(entry);
        }
        if count > 0 {
            info!(count = count, "Restored outbox entries from storage");
        }

        Ok(())
    }

    /// Deletes an outbox key from storage without the media/no-storage guards
    /// of [`Self::clear_outbox_entry_from_storage`] — used inside
    /// [`Self::restore_outbox`], which already holds a storage handle and
    /// operates on raw persisted keys.
    fn delete_outbox_key(&self, message_id: &str) {
        if let Some(storage) = &self.protocol_state_storage {
            if let Err(e) = storage.delete(storage_keys::OUTBOX, message_id) {
                warn!(message_id = %message_id, error = %e, "Failed to delete outbox key");
            }
        }
    }

    // ========================================================================
    // MEDIA TRANSFER DESCRIPTOR PERSISTENCE
    // ========================================================================

    /// Persists a media transfer descriptor (never chunk bytes) to storage.
    ///
    /// Best-effort and infallible, like [`Self::persist_outbox_entry`]: a
    /// failed write only costs the crash-recovery signal, not the transfer.
    pub(crate) fn persist_media_descriptor(&self, descriptor: &MediaTransferDescriptor) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        match serde_json::to_vec(descriptor) {
            Ok(data) => {
                if let Err(e) = self.write_state_record(
                    storage.as_ref(),
                    storage_keys::MEDIA_DESCRIPTORS,
                    &descriptor.file_id,
                    &data,
                ) {
                    warn!(file_id = %descriptor.file_id, error = %e, "Failed to persist media descriptor");
                }
            }
            Err(e) => {
                warn!(file_id = %descriptor.file_id, error = %e, "Failed to serialize media descriptor");
            }
        }
    }

    /// Removes a persisted media transfer descriptor (and any restored copy).
    ///
    /// Called wherever the in-memory transfer is removed — completion, abort,
    /// stale sweep — and when a restored descriptor is consumed by a resend,
    /// so a descriptor only ever survives into a restore when the process
    /// died mid-transfer.
    pub(crate) fn remove_media_descriptor(&mut self, file_id: &str) {
        self.restored_media_descriptors.remove(file_id);
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        if let Err(e) = storage.delete(storage_keys::MEDIA_DESCRIPTORS, file_id) {
            warn!(file_id = %file_id, error = %e, "Failed to clear persisted media descriptor");
        }
    }

    /// Restores persisted media transfer descriptors on startup.
    ///
    /// Recovery rules mirror [`Self::restore_outbox`]:
    /// - a record that will not open, or that the store itself calls corrupt,
    ///   is dropped and skipped;
    /// - so is one whose bytes are not a descriptor;
    /// - entries older than the outbox lifetime are dropped — the app has
    ///   long settled that message's fate, a resend signal would be noise;
    /// - the total is pruned to `MAX_MEDIA_DESCRIPTORS`, keeping the newest
    ///   by `queued_at` (overflow deleted from storage).
    ///
    /// **All four** of those deletes share one [`MAX_RESTORE_PRUNE_DELETES`]
    /// budget, including the first — which this walk does not issue itself, so
    /// it is the one that went uncounted: it happens inside
    /// [`Self::read_state_record_detailed`], which is why the budget is handed
    /// to the reader rather than only claimed around the explicit calls below.
    /// Descriptors are a sealed category, so a regenerated record key makes
    /// *every* record on the install take that path at once.
    ///
    /// A descriptor is advisory, so a record the budget spared is simply
    /// re-walked and dropped on a later launch.
    ///
    /// The survivors are parked in `restored_media_descriptors`; `start()`
    /// emits one `MediaResendRequired` each once the event pipeline is live,
    /// leaving entries parked until a same-`file_id` resend consumes them or
    /// the restore TTL prunes them.
    pub(crate) fn restore_media_descriptors(
        &mut self,
        allowance: &mut PruneAllowance,
    ) -> Result<()> {
        // Cloned rather than borrowed, like `restore_outbox`: the walk below
        // reads through this handle while holding a mutable borrow of the
        // prune budget and calling `&self` delete helpers.
        let Some(storage) = self.protocol_state_storage.clone() else {
            return Ok(());
        };

        let file_ids = Self::list_state_keys(storage.as_ref(), storage_keys::MEDIA_DESCRIPTORS)
            .map_err(|e| Error::Other(format!("Failed to list media descriptors: {}", e)))?;

        let lifetime_ms = self.config.reliability.retry.outbox_max_lifetime_ms;
        let now = Utc::now();

        let mut restored: Vec<MediaTransferDescriptor> = Vec::new();
        // Every delete this walk causes — unreadable, unparseable, expired,
        // over-cap — is budgeted together: a descriptor is purely advisory (the
        // app re-initiates the transfer), so a record left on disk one launch
        // longer costs nothing, while the device barrier each delete carries is
        // the most expensive thing this walk can do. Skipped records are simply
        // re-walked next launch and dropped then. "Causes" rather than
        // "issues": the first of the four happens inside the reader, which is
        // why the budget is handed to it below. Draws on the launch-wide
        // advisory pool it shares with the two cache walks and the Welcome
        // lifecycle walk.
        let mut budget = allowance.refusing();
        // Bounded, but deliberately by the wide ceiling rather than a multiple
        // of `MAX_MEDIA_DESCRIPTORS`: that cap is applied here, on restore, and
        // nowhere on the insert path, so a long session can legitimately leave
        // more descriptors on disk than the cap. A tight prefix would keep the
        // wrong ones (the survivors are chosen by `queued_at` among what was
        // walked) and strand the rest forever, since nothing else deletes them.
        // A descriptor loss is advisory anyway — the app re-initiates the
        // transfer — so there is nothing to settle here; only the walk itself
        // needs a ceiling.
        for file_id in file_ids.into_iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            // The reader takes the budget because it is the reader that deletes
            // a record it cannot return — absent, unreadable, and
            // unreadable-this-session are all "nothing to park" here, so this
            // walk only has to distinguish them from a record it can decode.
            let data = match self.read_state_record_detailed_budgeted(
                storage.as_ref(),
                storage_keys::MEDIA_DESCRIPTORS,
                &file_id,
                Some(&mut budget),
            ) {
                Ok(StateRecord::Present(data)) => data,
                Ok(StateRecord::Missing | StateRecord::Unreadable | StateRecord::Unavailable)
                | Err(_) => continue,
            };

            let descriptor = match serde_json::from_slice::<MediaTransferDescriptor>(&data) {
                Ok(descriptor) => descriptor,
                Err(e) => {
                    warn!(file_id = %file_id, error = %e, "Dropping corrupted media descriptor");
                    if budget.claim() {
                        self.delete_media_descriptor_key(&file_id);
                    }
                    continue;
                }
            };

            if lifetime_expired(now, descriptor.queued_at, lifetime_ms) {
                debug!(file_id = %file_id, "Dropping expired media descriptor");
                if budget.claim() {
                    self.delete_media_descriptor_key(&file_id);
                }
                continue;
            }

            restored.push(descriptor);
        }

        if restored.len() > MAX_MEDIA_DESCRIPTORS {
            restored.sort_by_key(|d| std::cmp::Reverse(d.queued_at));
            for descriptor in restored.drain(MAX_MEDIA_DESCRIPTORS..) {
                // Dropped from the parked set either way: the cap is what this
                // build will announce, and a record the budget spared is
                // re-walked (and re-capped) on the next launch.
                if budget.claim() {
                    self.delete_media_descriptor_key(&descriptor.file_id);
                }
            }
        }

        if budget.exhausted {
            warn!(
                deleted = budget.spent,
                budget = MAX_RESTORE_PRUNE_DELETES,
                "Media descriptor prune hit its share of the launch delete budget; the rest is left on disk for a later launch"
            );
        }

        let count = restored.len();
        for descriptor in restored {
            self.restored_media_descriptors
                .insert(descriptor.file_id.clone(), descriptor);
        }
        if count > 0 {
            info!(
                count = count,
                "Restored media transfer descriptors from storage"
            );
        }

        Ok(())
    }

    /// Deletes a media descriptor key from storage (restore-internal, mirrors
    /// [`Self::delete_outbox_key`]).
    fn delete_media_descriptor_key(&self, file_id: &str) {
        if let Some(storage) = &self.protocol_state_storage {
            if let Err(e) = storage.delete(storage_keys::MEDIA_DESCRIPTORS, file_id) {
                warn!(file_id = %file_id, error = %e, "Failed to delete media descriptor key");
            }
        }
    }

    // ========================================================================
    // BLOCKED USERS PERSISTENCE
    // ========================================================================

    /// Persists a blocked user entry to storage.
    pub(crate) fn persist_blocked_user(&self, user_id: &str) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        if let Err(e) =
            self.write_state_record(storage.as_ref(), storage_keys::BLOCKED_USERS, user_id, &[])
        {
            warn!(user_id = %user_id, error = %e, "Failed to persist blocked user");
        }
    }

    /// Deletes a blocked user entry from storage.
    pub(crate) fn delete_blocked_user(&self, user_id: &str) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        if let Err(e) = storage.delete(storage_keys::BLOCKED_USERS, user_id) {
            warn!(user_id = %user_id, error = %e, "Failed to delete blocked user from storage");
        }
    }

    /// Restores blocked users from persistent storage.
    ///
    /// Skips entries with invalid user IDs (best-effort restore).
    ///
    /// A *listing* failure is not best-effort: it is indistinguishable from an
    /// empty store, so swallowing it would come up with an empty block list and
    /// tell no one — every peer the user blocked silently unblocked, from a
    /// transient error. That is the same outcome this branch's release notes
    /// call out as the reason a downgrade is not a rollback. Propagating rolls
    /// `initialize_mls` back instead, so the app finds out rather than running
    /// unprotected. Blocking is a safety control; it fails closed.
    pub(crate) fn restore_blocked_users(&mut self) -> Result<()> {
        let Some(storage) = &self.protocol_state_storage else {
            return Ok(());
        };
        let user_ids = Self::list_state_keys(storage.as_ref(), storage_keys::BLOCKED_USERS)
            .map_err(|e| Error::Other(format!("Failed to list blocked users: {}", e)))?;
        let listed = user_ids.len();
        for user_id in user_ids.iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            if offline_protocol_core::UserId::new(user_id).is_err() {
                warn!(user_id = %user_id, "Skipping blocked user entry with invalid user ID");
                continue;
            }
            self.blocked_users.insert(user_id.clone());
        }
        if listed > MAX_RESTORE_KEYS_PER_CATEGORY {
            warn!(
                listed,
                cap = MAX_RESTORE_KEYS_PER_CATEGORY,
                "Blocked-user store listed more entries than any legitimate run can produce; ignoring the tail"
            );
        }
        if !self.blocked_users.is_empty() {
            info!(
                count = self.blocked_users.len(),
                "Restored blocked users from storage"
            );
        }
        Ok(())
    }

    // ========================================================================
    // BOTH-CREATE OWNER GATE PERSISTENCE
    // ========================================================================

    /// Persists a both-create owner-gate entry (value-less; the key is the peer).
    pub(crate) fn persist_both_create_awaiting_decrypt(&self, peer_id: &str) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        if let Err(e) = self.write_state_record(
            storage.as_ref(),
            storage_keys::BOTH_CREATE_AWAITING_DECRYPT,
            peer_id,
            &[],
        ) {
            warn!(peer_id = %peer_id, error = %e, "Failed to persist both-create owner gate");
        }
    }

    /// Deletes a both-create owner-gate entry once the peer has converged.
    pub(crate) fn delete_both_create_awaiting_decrypt(&self, peer_id: &str) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        if let Err(e) = storage.delete(storage_keys::BOTH_CREATE_AWAITING_DECRYPT, peer_id) {
            warn!(peer_id = %peer_id, error = %e, "Failed to delete both-create owner gate");
        }
    }

    /// Restores the both-create owner gate from storage on startup, so an owner
    /// that restarted mid-convergence keeps requiring a group-aware decrypt
    /// before confirming a still-pending peer. Stale entries for already-confirmed
    /// peers are harmless (confirmation short-circuits) and are cleared on the
    /// next confirm.
    pub(crate) fn restore_both_create_awaiting_decrypt(&mut self) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        let peer_ids = match Self::list_state_keys(
            storage.as_ref(),
            storage_keys::BOTH_CREATE_AWAITING_DECRYPT,
        ) {
            Ok(keys) => keys,
            Err(e) => {
                warn!(error = %e, "Failed to list both-create owner gate from storage");
                return;
            }
        };
        let listed = peer_ids.len();
        for peer_id in peer_ids.iter().take(MAX_RESTORE_KEYS_PER_CATEGORY) {
            self.both_create_awaiting_decrypt.insert(peer_id.clone());
        }
        if listed > MAX_RESTORE_KEYS_PER_CATEGORY {
            warn!(
                listed,
                cap = MAX_RESTORE_KEYS_PER_CATEGORY,
                "Both-create owner gate listed more peers than any legitimate run can produce; ignoring the tail"
            );
        }
        if !self.both_create_awaiting_decrypt.is_empty() {
            info!(
                count = self.both_create_awaiting_decrypt.len(),
                "Restored both-create owner gate from storage"
            );
        }
    }

    // ========================================================================
    // TELEMETRY SCRUB-SECRET PERSISTENCE
    // ========================================================================

    /// Loads (or, on first run, generates and persists) the per-install
    /// telemetry scrub secret, then installs it as the fallback secret used
    /// for opaque-identifier hashing when no explicit `scrub_secret` is set on
    /// the installed `TelemetryConfig`.
    ///
    /// This makes opaque identifiers stable across process restarts so backend
    /// telemetry can count distinct devices: the same device hashes to the same
    /// opaque id every session. Until storage is available (or if storage is
    /// never provided), the SDK keeps using the random per-instance fallback
    /// generated at construction — so this is purely an upgrade over the
    /// random fallback, never a regression.
    ///
    /// Secret precedence is unchanged: an explicit
    /// [`crate::telemetry::TelemetryConfig::with_scrub_secret`] still wins over
    /// this persistent fallback (see [`crate::telemetry::Scrubber::from_config`]).
    ///
    /// Idempotent across repeated initialization attempts via
    /// `telemetry_secret_persisted`. All
    /// storage failures degrade gracefully to the in-memory random fallback —
    /// telemetry pseudonymization must never block protocol initialization.
    pub(crate) fn restore_or_init_scrub_secret(&mut self) {
        if self.telemetry_secret_persisted {
            return;
        }
        let Some(storage) = &self.secure_storage else {
            return;
        };

        let secret: [u8; 16] = match storage
            .load(storage_keys::SCRUB_SECRET, storage_keys::SCRUB_SECRET_ID)
        {
            Ok(Some(bytes)) if bytes.len() == 16 => {
                let mut secret = [0u8; 16];
                secret.copy_from_slice(&bytes);
                debug!("Restored persistent telemetry scrub secret from storage");
                secret
            }
            Ok(other) => {
                // Absent, or present but corrupt/wrong-length: generate a fresh
                // secret and persist it. A wrong-length blob is overwritten so a
                // single corrupt write does not pin every future session to the
                // random fallback.
                if other.is_some() {
                    warn!("Persisted scrub secret had unexpected length; regenerating");
                }
                let fresh = *uuid::Uuid::new_v4().as_bytes();
                if let Err(e) = storage.store(
                    storage_keys::SCRUB_SECRET,
                    storage_keys::SCRUB_SECRET_ID,
                    &fresh,
                ) {
                    // Keep the in-memory secret for this session; next launch
                    // will retry persistence. Opaque ids stay stable within
                    // this process but may differ next session — strictly no
                    // worse than the legacy random fallback.
                    warn!(error = %e, "Failed to persist telemetry scrub secret; using session-local secret");
                    return self.adopt_fallback_secret(fresh);
                }
                info!("Generated and persisted per-install telemetry scrub secret");
                fresh
            }
            Err(e) => {
                warn!(error = %e, "Failed to load telemetry scrub secret; keeping random fallback");
                return;
            }
        };

        self.telemetry_secret_persisted = true;
        self.adopt_fallback_secret(secret);
    }

    // ========================================================================
    // NOSTR SIGNING-SECRET PERSISTENCE
    // ========================================================================

    /// Loads (or, on first run, generates and persists) the per-install Nostr
    /// signing secret and installs the derived signing key into the Nostr
    /// transport, replacing the ephemeral key it was constructed with.
    ///
    /// This gives the install a stable Nostr identity (event signatures,
    /// relay-visible pubkey) across restarts. The signing key is intentionally
    /// not derivable from any public identifier (SEC-M4); message addressing
    /// is unaffected because it uses the separate routing tag, which remains
    /// derived from the device ID.
    ///
    /// Idempotent across repeated initialization attempts via
    /// `nostr_secret_persisted`. All
    /// failures degrade gracefully to the construction-time ephemeral key,
    /// which is equally unforgeable but rotates per process — transport
    /// keying must never block protocol initialization. A secret that was
    /// installed but could not be persisted is kept in
    /// `nostr_unpersisted_secret` so a later attempt retries persisting the
    /// same identity instead of rotating it.
    pub(crate) fn restore_or_init_nostr_signing_secret(&mut self) {
        if self.nostr_secret_persisted {
            return;
        }
        let Some(storage) = self.secure_storage.clone() else {
            return;
        };
        let Some(nostr_arc) = self.transport_manager.get_transport(TransportType::Nostr) else {
            // Nostr transport not registered — nothing to key.
            return;
        };

        let (secret, persisted): (Zeroizing<[u8; 32]>, bool) = match storage.load(
            storage_keys::NOSTR_SIGNING_SECRET,
            storage_keys::NOSTR_SIGNING_SECRET_ID,
        ) {
            Ok(Some(bytes)) if bytes.len() == 32 => {
                let bytes = Zeroizing::new(bytes);
                let mut secret = Zeroizing::new([0u8; 32]);
                secret.copy_from_slice(&bytes);
                // A stored secret supersedes anything a previous attempt
                // failed to persist.
                self.nostr_unpersisted_secret = None;
                debug!("Restored persistent Nostr signing secret from storage");
                (secret, true)
            }
            Ok(other) => {
                // Absent, or present but corrupt/wrong-length: persist a
                // fresh secret (a wrong-length blob is overwritten so a
                // single corrupt write does not pin every future session to
                // the ephemeral key). Prefer a secret a previous attempt
                // installed but failed to persist, so the retry keeps the
                // identity already in use instead of rotating it again.
                if other.is_some() {
                    warn!("Persisted Nostr signing secret had unexpected length; regenerating");
                }
                let fresh = match self.nostr_unpersisted_secret.take() {
                    Some(unpersisted) => unpersisted,
                    None => match NostrKeypair::generate_install_secret() {
                        Ok(fresh) => fresh,
                        Err(e) => {
                            warn!(error = %e, "Failed to generate Nostr signing secret; keeping ephemeral key");
                            return;
                        }
                    },
                };
                match storage.store(
                    storage_keys::NOSTR_SIGNING_SECRET,
                    storage_keys::NOSTR_SIGNING_SECRET_ID,
                    &*fresh,
                ) {
                    Ok(()) => {
                        info!("Generated and persisted per-install Nostr signing secret");
                        (fresh, true)
                    }
                    Err(e) => {
                        // Install the unpersisted secret anyway: the identity
                        // is stable for this session and the next entry path
                        // or launch retries persistence — strictly no worse
                        // than the ephemeral key.
                        warn!(error = %e, "Failed to persist Nostr signing secret; Nostr identity is session-local");
                        (fresh, false)
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to load Nostr signing secret; keeping ephemeral key");
                return;
            }
        };

        let installed = match nostr_arc.as_any().downcast_ref::<NostrTransport>() {
            Some(nostr) => match nostr.install_signing_secret(&*secret) {
                Ok(()) => true,
                Err(e) => {
                    warn!(error = %e, "Failed to install Nostr signing key; keeping ephemeral key");
                    false
                }
            },
            None => {
                warn!("Transport registered as Nostr is not a NostrTransport; cannot install signing key");
                false
            }
        };

        if !persisted {
            // Keep the secret so the next entry path retries persisting this
            // same identity rather than generating a new one.
            self.nostr_unpersisted_secret = Some(secret);
        }
        self.nostr_secret_persisted = installed && persisted;
    }

    /// Installs `secret` as the telemetry fallback secret and rebuilds the
    /// pre-install scrubber so the legacy MLS observability path also hashes
    /// with the stable secret. Does not touch an already-installed
    /// [`crate::telemetry::TelemetryContext`] (rebuilding a live context would
    /// rotate opaque ids mid-run); apps that need stable ids should provide
    /// storage before installing a telemetry sink.
    fn adopt_fallback_secret(&mut self, secret: [u8; 16]) {
        self.telemetry_fallback_secret = secret;
        self.telemetry_scrubber = crate::telemetry::Scrubber::from_config(
            &crate::telemetry::TelemetryConfig::default(),
            secret,
        );
    }

    /// Returns a stable, opaque per-install telemetry identifier, or `None`
    /// until the persistent scrub secret is available.
    ///
    /// The id is `SHA-256(secret || domain)` truncated to a 32-character hex
    /// string, where `secret` is the per-install scrub secret managed by
    /// `Self::restore_or_init_scrub_secret`. The secret cannot be recovered
    /// from the id, and the fixed domain string keeps the id un-correlatable
    /// with opaque identifiers the scrubber produces for telemetry records:
    /// the domain contains `:`, which id validation
    /// (`offline_protocol_core::types::validate_id_chars`) rejects in every
    /// `UserId`/`AppId`, so no validated identifier reaching the scrubber can
    /// ever equal the domain and collide with the install id.
    ///
    /// Returns `None` while the SDK is still on the random per-instance
    /// fallback secret — i.e. before secure storage is provided via
    /// [`super::OfflineProtocol::initialize_mls`], or when persistence failed
    /// this session. In that state the id would not be stable across launches,
    /// so none is exposed.
    ///
    /// Deliberately derived from the persistent fallback secret, not from an
    /// installed [`crate::telemetry::TelemetryConfig::with_scrub_secret`]
    /// override: the install id must not rotate when a sink is (re)installed,
    /// and must not be computable from an app-chosen secret.
    ///
    /// The domain string is part of the public contract: changing it would
    /// silently rotate every device's install id. Frozen — do not edit.
    pub fn telemetry_install_id(&self) -> Option<String> {
        const TELEMETRY_INSTALL_ID_DOMAIN: &str = "telemetry:install-id";
        self.telemetry_secret_persisted.then(|| {
            crate::telemetry::scrubber::opaque_id(
                TELEMETRY_INSTALL_ID_DOMAIN,
                &self.telemetry_fallback_secret,
            )
        })
    }

    // ========================================================================
    // LAMPORT CLOCK PERSISTENCE
    // ========================================================================

    /// Debounced Lamport clock persistence. Only writes to storage when the
    /// in-memory value has advanced past `last_persisted_lamport` by at least
    /// `LAMPORT_PERSIST_INTERVAL` ticks. This avoids a Keychain/Keystore
    /// write on every sent and received message.
    pub(crate) fn persist_lamport_clock(&mut self) {
        let current = self.lamport_clock.value();
        if current.wrapping_sub(self.last_persisted_lamport) < super::LAMPORT_PERSIST_INTERVAL {
            return;
        }
        self.write_lamport_clock_to_storage(current);
    }

    /// Forces the Lamport clock to storage regardless of debounce state.
    /// Called on shutdown to avoid losing any un-flushed ticks.
    pub(crate) fn flush_lamport_clock(&mut self) {
        self.write_lamport_clock_to_storage(self.lamport_clock.value());
    }

    fn write_lamport_clock_to_storage(&mut self, value: u64) {
        let Some(storage) = self.protocol_state_storage.clone() else {
            return;
        };
        let bytes = value.to_le_bytes();
        if let Err(e) = self.write_state_record(
            storage.as_ref(),
            storage_keys::LAMPORT_CLOCK,
            storage_keys::LAMPORT_CLOCK_ID,
            &bytes,
        ) {
            warn!(error = %e, "Failed to persist Lamport clock");
            return;
        }
        self.last_persisted_lamport = value;
    }

    /// Restores the Lamport clock from storage.
    ///
    /// Uses `max(current, restored)` so the clock never goes backward even
    /// if the in-memory value has advanced before storage was attached.
    pub(crate) fn restore_lamport_clock(&mut self) {
        let Some(storage) = self.protocol_state_storage.clone() else {
            return;
        };
        if let Ok(Some(data)) = self.read_state_record(
            storage.as_ref(),
            storage_keys::LAMPORT_CLOCK,
            storage_keys::LAMPORT_CLOCK_ID,
        ) {
            if data.len() == 8 {
                let restored = u64::from_le_bytes(data.try_into().expect("verified length is 8"));
                let restored_clock = LamportClock::from_value(restored);
                if restored_clock > self.lamport_clock {
                    self.lamport_clock = restored_clock;
                }
                self.last_persisted_lamport = self.lamport_clock.value();
                debug!(clock = %self.lamport_clock, "Restored Lamport clock from storage");
            } else {
                warn!(
                    len = data.len(),
                    "Corrupted Lamport clock in storage (expected 8 bytes), starting fresh"
                );
            }
        }
    }
}
