//! Custody v1: holding a neighbour's replication frame for hours.
//!
//! The store, the acceptance table, the quotas and the receipt codec from
//! `docs/spec/custody.md`. The seams that feed it sit beside the forwarding
//! path: `receive.rs` judges a frame at the drop point and transmits
//! redeliveries, `send.rs` writes the deposit request on the depositor's own
//! frame, and `mod.rs` restores, sweeps and answers receipts.
//!
//! Nothing here touches the forwarding suppression cache, settles an outbox
//! entry, or sends anything. Those are the chapter's invariants, and keeping
//! this module free of the engine's other state is what lets a test pin them
//! against the store alone.

use std::collections::{HashMap, HashSet, VecDeque};

use offline_protocol_core::{Message, MessageId};
use serde::{Deserialize, Serialize};

use crate::config::{CustodyConfig, OverflowPolicy};
use crate::protocol::prefixes::internal_prefixes;

/// The reserved metadata key a depositor writes on its own frame
/// (`docs/spec/wire-format.md`, reserved metadata keys). Engine-written,
/// unsigned, and stripped by every device that forwards the frame.
pub const CUSTODY_META_KEY: &str = "__custody";

/// The one class token this version defines: Class A replication frames.
pub const CUSTODY_CLASS_DATA: &str = "data";

/// The receipt body version this build writes and reads.
pub const CUSTODY_RECEIPT_VERSION: u8 = 1;

/// Neighbours a held frame is re-originated toward during one hold, at most.
///
/// The chapter bounds the set of neighbours tried "the way tracked neighbours
/// are bounded"; this is that bound, sized well above any neighbourhood the
/// mesh targets so it is a ceiling against a pathological churn of addresses
/// rather than a dial.
pub const MAX_CUSTODY_NEIGHBOURS_PER_HOLD: usize = 64;

/// Custodians one depositor remembers a receipt from, per outbox entry.
///
/// A receipt suppresses re-deposit toward the custodian that sent it. The set
/// is keyed by outbox id, so it is bounded by the outbox; this bounds the other
/// axis against a neighbourhood that mints addresses.
pub const MAX_CUSTODY_RECEIPTS_PER_MESSAGE: usize = 64;

/// Why a frame at the drop point was not taken into custody, in the order the
/// chapter's acceptance table applies them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CustodyRefusal {
    /// Custody is off on this device.
    Disabled,
    /// The frame carries no deposit request.
    NoRequest,
    /// The request names a class this version does not define.
    UnknownClass,
    /// The outer prefix is not `__MLS_ENC__`.
    NotSealed,
    /// The carrier did not identify the link the frame arrived on.
    UnprovenPeer,
    /// The frame's sender is not the peer it arrived from: it came through a
    /// forwarder that did not strip the request.
    NotDepositor,
    /// A frame with this identifier is already held: not stored, not
    /// answered, counted under `duplicates` rather than as a refusal.
    Duplicate,
    /// The depositor is a stranger and the stranger tier is closed.
    StrangerRefused,
    /// The depositor's own budget is full and the policy does not make room.
    DepositorFull,
    /// The global budget is full and the policy does not make room.
    StoreFull,
    /// The battery is below the soft relay floor.
    Battery,
}

impl CustodyRefusal {
    /// Every refusal reason, in table order, for the counters and their tests.
    pub const ALL: &'static [Self] = &[
        Self::Disabled,
        Self::NoRequest,
        Self::UnknownClass,
        Self::NotSealed,
        Self::UnprovenPeer,
        Self::NotDepositor,
        Self::Duplicate,
        Self::StrangerRefused,
        Self::DepositorFull,
        Self::StoreFull,
        Self::Battery,
    ];

    /// The reason as the chapter names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoRequest => "no_request",
            Self::UnknownClass => "unknown_class",
            Self::NotSealed => "not_sealed",
            Self::UnprovenPeer => "unproven_peer",
            Self::NotDepositor => "not_depositor",
            Self::Duplicate => "duplicate",
            Self::StrangerRefused => "stranger_refused",
            Self::DepositorFull => "depositor_full",
            Self::StoreFull => "store_full",
            Self::Battery => "battery",
        }
    }
}

/// What a custodian has done, and is holding.
///
/// A snapshot, safe to poll. `held` and `held_bytes` are gauges; everything
/// else is cumulative since start-up or the last erase. Aggregate and never per
/// depositor, because a per-depositor answer on any wire would be the quota
/// oracle the chapter refuses to be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustodyStats {
    /// Frames in custody right now.
    pub held: u64,
    /// Bytes in custody right now.
    pub held_bytes: u64,
    /// Deposits accepted.
    pub accepted: u64,
    /// Held frames handed to their recipient directly, and released.
    pub delivered: u64,
    /// Held frames re-originated toward a neighbour that is not the recipient.
    pub re_originated: u64,
    /// Held frames dropped at the end of their hold.
    pub expired: u64,
    /// Deposits of an identifier already held: not stored, not answered.
    pub duplicates: u64,
    /// Held frames evicted to admit a newer deposit under drop-oldest.
    pub evicted: u64,
    /// Receipts put on the arrival link.
    pub receipts_sent: u64,
    /// Receipts not sent: the depositor does not parse them, the link was
    /// gone, or this device cannot sign.
    pub receipts_dropped: u64,
    /// Receipts this device received for an entry still in its outbox.
    pub receipts_received: u64,
    /// Receipts this device received naming nothing in its outbox, or that it
    /// could not parse.
    pub receipts_ignored: u64,
    /// Refused: custody is off.
    pub refused_disabled: u64,
    /// Refused: no deposit request on the frame.
    pub refused_no_request: u64,
    /// Refused: unknown class token.
    pub refused_unknown_class: u64,
    /// Refused: not an `__MLS_ENC__` frame.
    pub refused_not_sealed: u64,
    /// Refused: the arrival link was not identified.
    pub refused_unproven_peer: u64,
    /// Refused: the sender is not the arrival peer.
    pub refused_not_depositor: u64,
    /// Refused: a stranger while the stranger tier is closed.
    pub refused_stranger: u64,
    /// Refused: the depositor's budget is full.
    pub refused_depositor_full: u64,
    /// Refused: the global budget is full.
    pub refused_store_full: u64,
    /// Refused: the battery is below the relay floor.
    pub refused_battery: u64,
}

impl CustodyStats {
    fn count(&mut self, refusal: CustodyRefusal) {
        let slot = match refusal {
            CustodyRefusal::Disabled => &mut self.refused_disabled,
            CustodyRefusal::NoRequest => &mut self.refused_no_request,
            CustodyRefusal::UnknownClass => &mut self.refused_unknown_class,
            CustodyRefusal::NotSealed => &mut self.refused_not_sealed,
            CustodyRefusal::UnprovenPeer => &mut self.refused_unproven_peer,
            CustodyRefusal::NotDepositor => &mut self.refused_not_depositor,
            CustodyRefusal::Duplicate => &mut self.duplicates,
            CustodyRefusal::StrangerRefused => &mut self.refused_stranger,
            CustodyRefusal::DepositorFull => &mut self.refused_depositor_full,
            CustodyRefusal::StoreFull => &mut self.refused_store_full,
            CustodyRefusal::Battery => &mut self.refused_battery,
        };
        *slot = slot.saturating_add(1);
    }

    /// The counter a refusal reason lands in, for tests that walk the table.
    pub fn refusals(&self, refusal: CustodyRefusal) -> u64 {
        match refusal {
            CustodyRefusal::Disabled => self.refused_disabled,
            CustodyRefusal::NoRequest => self.refused_no_request,
            CustodyRefusal::UnknownClass => self.refused_unknown_class,
            CustodyRefusal::NotSealed => self.refused_not_sealed,
            CustodyRefusal::UnprovenPeer => self.refused_unproven_peer,
            CustodyRefusal::NotDepositor => self.refused_not_depositor,
            CustodyRefusal::Duplicate => self.duplicates,
            CustodyRefusal::StrangerRefused => self.refused_stranger,
            CustodyRefusal::DepositorFull => self.refused_depositor_full,
            CustodyRefusal::StoreFull => self.refused_store_full,
            CustodyRefusal::Battery => self.refused_battery,
        }
    }
}

/// One frame in custody.
#[derive(Debug, Clone)]
pub(crate) struct HeldFrame {
    /// The frame as it arrived, hop fields as the forwarding path adjusted
    /// them, with the deposit request removed.
    pub(crate) message: Message,
    /// The frame's sender, which is also the peer it arrived from.
    pub(crate) depositor: String,
    /// Wall-clock acceptance time, milliseconds since the Unix epoch. Never a
    /// monotonic instant: that clock does not run while the device is off.
    pub(crate) accepted_at_ms: i64,
    /// The class token the depositor asserted.
    pub(crate) class: String,
    /// The frame's footprint against the byte budgets.
    bytes: usize,
    /// Neighbours this frame has been re-originated toward during its hold.
    tried: HashSet<String>,
    /// The neighbour a marked forward is queued toward right now, if any.
    in_flight: Option<String>,
}

/// A frame at the drop point, with everything the acceptance table reads.
pub(crate) struct CustodyCandidate<'a> {
    /// The forwarded copy: hop fields adjusted, deposit request removed.
    pub(crate) message: Message,
    /// The class token the frame carried on arrival, if any.
    pub(crate) request: Option<&'a str>,
    /// The peer the frame arrived from, when the carrier proved the link.
    pub(crate) arrival_peer: Option<&'a str>,
    /// Whether this device holds an established session with that peer.
    pub(crate) session_tier: bool,
    /// Whether the battery is above the soft relay floor.
    pub(crate) battery_ok: bool,
    /// Wall-clock now, milliseconds since the Unix epoch.
    pub(crate) now_ms: i64,
}

/// A deposit the store took on.
pub(crate) struct Accepted {
    /// The held frame's identifier.
    pub(crate) id: MessageId,
    /// The depositor, which is the arrival peer and the frame's sender.
    pub(crate) depositor: String,
    /// The class token, for the durable record.
    pub(crate) class: String,
    /// Frames evicted under drop-oldest to admit this one; their records are
    /// the caller's to delete.
    pub(crate) evicted: Vec<MessageId>,
}

/// The footprint a held frame is charged against the byte budgets.
///
/// The same accounting the pending-decrypt queue uses: the variable fields
/// (content, binary content, metadata, media metadata) plus the addresses. The
/// fixed cost of a record is bounded by the entry caps, so it is not charged
/// here; the byte budget exists to bound payload bloat.
fn frame_footprint(message: &Message) -> usize {
    let metadata_bytes: usize = message
        .metadata
        .iter()
        .map(|(key, value)| key.len() + value.len())
        .sum();
    let media_bytes = message
        .media_metadata
        .as_ref()
        .map(|media| {
            media.mime_type.len()
                + media.file_name.len()
                + media
                    .thumbnail_base64
                    .as_ref()
                    .map(String::len)
                    .unwrap_or(0)
        })
        .unwrap_or(0);
    message.content.len()
        + message
            .binary_content
            .as_ref()
            .map(|binary| binary.len())
            .unwrap_or(0)
        + message.sender.as_str().len()
        + message.recipient.as_str().len()
        + message.app_id.as_str().len()
        + metadata_bytes
        + media_bytes
}

/// Which quota a depositor is judged under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Session,
    Stranger,
}

#[derive(Debug, Clone, Copy, Default)]
struct Usage {
    entries: usize,
    bytes: usize,
}

/// The custody store: held frames, per-depositor and global budgets, and the
/// counters.
///
/// Memory-only; the sealed records are written and deleted by the engine's
/// storage seam beside the pending-decrypt records, and restored through
/// [`Self::admit_restored`].
pub(crate) struct CustodyStore {
    config: CustodyConfig,
    entries: HashMap<MessageId, HeldFrame>,
    /// Acceptance order, oldest first. Ids are removed lazily: an id here that
    /// is no longer in `entries` is skipped.
    order: VecDeque<MessageId>,
    per_depositor: HashMap<String, Usage>,
    total: Usage,
    stats: CustodyStats,
}

impl CustodyStore {
    pub(crate) fn new(config: CustodyConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            order: VecDeque::new(),
            per_depositor: HashMap::new(),
            total: Usage::default(),
            stats: CustodyStats::default(),
        }
    }

    pub(crate) fn config(&self) -> &CustodyConfig {
        &self.config
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, id: &MessageId) -> bool {
        self.entries.contains_key(id)
    }

    pub(crate) fn get(&self, id: &MessageId) -> Option<&HeldFrame> {
        self.entries.get(id)
    }

    /// The counters, with the gauges filled from the store.
    pub(crate) fn stats(&self) -> CustodyStats {
        let mut stats = self.stats.clone();
        stats.held = self.entries.len() as u64;
        stats.held_bytes = self.total.bytes as u64;
        stats
    }

    /// Judges a frame at the drop point and takes it into custody when every
    /// row of the acceptance table holds.
    ///
    /// Counts the refusal or the acceptance. Touches nothing outside the
    /// store: the caller persists the record, deletes the evicted ones and
    /// answers with the receipt.
    pub(crate) fn judge(
        &mut self,
        candidate: CustodyCandidate<'_>,
    ) -> Result<Accepted, CustodyRefusal> {
        let tier = match self.check(&candidate) {
            Ok(tier) => tier,
            Err(refusal) => {
                // Every row is counted, `disabled` included, so an operator
                // who expected deposits and sees none can tell "off" from
                // "nobody asked"; a duplicate lands in its own counter.
                self.stats.count(refusal);
                return Err(refusal);
            }
        };

        let depositor = candidate
            .arrival_peer
            .expect("check proved the arrival peer")
            .to_string();
        let bytes = frame_footprint(&candidate.message);
        let (max_entries, max_bytes) = match tier {
            Tier::Session => (
                self.config.max_entries_per_depositor,
                self.config.max_bytes_per_depositor,
            ),
            Tier::Stranger => (
                self.config.stranger_max_entries,
                self.config.stranger_max_bytes,
            ),
        };

        let mut evicted = Vec::new();

        // The depositor's own budget first: under drop-oldest it is the
        // depositor's oldest frame that goes, which is what keeps one
        // depositor's flood from evicting everyone else's frames.
        if bytes > max_bytes {
            self.stats.count(CustodyRefusal::DepositorFull);
            return Err(CustodyRefusal::DepositorFull);
        }
        while !self.fits_depositor(&depositor, bytes, max_entries, max_bytes) {
            match self.config.overflow_policy {
                OverflowPolicy::DropOldest => match self.oldest_of(&depositor) {
                    Some(victim) => {
                        self.remove_inner(&victim);
                        self.stats.evicted = self.stats.evicted.saturating_add(1);
                        evicted.push(victim);
                    }
                    None => {
                        self.stats.count(CustodyRefusal::DepositorFull);
                        return Err(CustodyRefusal::DepositorFull);
                    }
                },
                OverflowPolicy::DropNewest => {
                    self.stats.count(CustodyRefusal::DepositorFull);
                    return Err(CustodyRefusal::DepositorFull);
                }
            }
        }

        if bytes > self.config.max_bytes {
            self.stats.count(CustodyRefusal::StoreFull);
            return Err(CustodyRefusal::StoreFull);
        }
        while !self.fits_store(bytes) {
            match self.config.overflow_policy {
                OverflowPolicy::DropOldest => match self.oldest() {
                    Some(victim) => {
                        self.remove_inner(&victim);
                        self.stats.evicted = self.stats.evicted.saturating_add(1);
                        evicted.push(victim);
                    }
                    None => {
                        self.stats.count(CustodyRefusal::StoreFull);
                        return Err(CustodyRefusal::StoreFull);
                    }
                },
                OverflowPolicy::DropNewest => {
                    self.stats.count(CustodyRefusal::StoreFull);
                    return Err(CustodyRefusal::StoreFull);
                }
            }
        }

        let id = candidate.message.id.clone();
        let class = candidate
            .request
            .expect("check proved the request")
            .to_string();
        self.insert(HeldFrame {
            message: candidate.message,
            depositor: depositor.clone(),
            accepted_at_ms: candidate.now_ms,
            class: class.clone(),
            bytes,
            tried: HashSet::new(),
            in_flight: None,
        });
        self.stats.accepted = self.stats.accepted.saturating_add(1);

        Ok(Accepted {
            id,
            depositor,
            class,
            evicted,
        })
    }

    /// The acceptance table, in the chapter's order.
    fn check(&self, c: &CustodyCandidate<'_>) -> Result<Tier, CustodyRefusal> {
        if !self.config.enabled {
            return Err(CustodyRefusal::Disabled);
        }
        let Some(token) = c.request else {
            return Err(CustodyRefusal::NoRequest);
        };
        if token != CUSTODY_CLASS_DATA {
            return Err(CustodyRefusal::UnknownClass);
        }
        if !c.message.content.starts_with(internal_prefixes::ENCRYPTED) {
            return Err(CustodyRefusal::NotSealed);
        }
        let Some(peer) = c.arrival_peer else {
            return Err(CustodyRefusal::UnprovenPeer);
        };
        if c.message.sender.as_str() != peer {
            return Err(CustodyRefusal::NotDepositor);
        }
        if self.entries.contains_key(&c.message.id) {
            // Not a refusal in the table's sense: the first receipt already
            // suppressed re-deposit at a depositor that received it, and one
            // that did not deposits again on its next retry. Absorbed here.
            return Err(CustodyRefusal::Duplicate);
        }
        let tier = if c.session_tier {
            Tier::Session
        } else {
            Tier::Stranger
        };
        if tier == Tier::Stranger
            && (self.config.stranger_max_entries == 0 || self.config.stranger_max_bytes == 0)
        {
            return Err(CustodyRefusal::StrangerRefused);
        }
        if !c.battery_ok {
            return Err(CustodyRefusal::Battery);
        }
        Ok(tier)
    }

    fn fits_depositor(
        &self,
        depositor: &str,
        bytes: usize,
        max_entries: usize,
        max_bytes: usize,
    ) -> bool {
        let usage = self
            .per_depositor
            .get(depositor)
            .copied()
            .unwrap_or_default();
        usage.entries < max_entries && usage.bytes.saturating_add(bytes) <= max_bytes
    }

    fn fits_store(&self, bytes: usize) -> bool {
        self.total.entries < self.config.max_entries
            && self.total.bytes.saturating_add(bytes) <= self.config.max_bytes
    }

    fn oldest(&self) -> Option<MessageId> {
        self.order
            .iter()
            .find(|id| self.entries.contains_key(*id))
            .cloned()
    }

    fn oldest_of(&self, depositor: &str) -> Option<MessageId> {
        self.order
            .iter()
            .find(|id| {
                self.entries
                    .get(*id)
                    .is_some_and(|held| held.depositor == depositor)
            })
            .cloned()
    }

    fn insert(&mut self, held: HeldFrame) {
        let usage = self
            .per_depositor
            .entry(held.depositor.clone())
            .or_default();
        usage.entries += 1;
        usage.bytes = usage.bytes.saturating_add(held.bytes);
        self.total.entries += 1;
        self.total.bytes = self.total.bytes.saturating_add(held.bytes);
        self.order.push_back(held.message.id.clone());
        self.entries.insert(held.message.id.clone(), held);
    }

    fn remove_inner(&mut self, id: &MessageId) -> Option<HeldFrame> {
        let held = self.entries.remove(id)?;
        if let Some(usage) = self.per_depositor.get_mut(&held.depositor) {
            usage.entries = usage.entries.saturating_sub(1);
            usage.bytes = usage.bytes.saturating_sub(held.bytes);
            if usage.entries == 0 {
                self.per_depositor.remove(&held.depositor);
            }
        }
        self.total.entries = self.total.entries.saturating_sub(1);
        self.total.bytes = self.total.bytes.saturating_sub(held.bytes);
        self.order.retain(|queued| queued != id);
        Some(held)
    }

    /// Puts a record read back from storage into the store, or refuses it.
    ///
    /// Refused when the identifier is already held or the budgets in force
    /// have no room: restore keeps records "exactly as stored" but under the
    /// configuration of the launch, so a lowered cap drops the excess rather
    /// than exceeding the cap the operator set. Never evicts, never counts an
    /// acceptance: nothing was deposited at restore.
    pub(crate) fn admit_restored(
        &mut self,
        message: Message,
        depositor: String,
        accepted_at_ms: i64,
        class: String,
    ) -> bool {
        if self.entries.contains_key(&message.id) {
            return false;
        }
        let bytes = frame_footprint(&message);
        let (max_entries, max_bytes) = (
            self.config
                .max_entries_per_depositor
                .max(self.config.stranger_max_entries),
            self.config
                .max_bytes_per_depositor
                .max(self.config.stranger_max_bytes),
        );
        if !self.fits_depositor(&depositor, bytes, max_entries, max_bytes)
            || !self.fits_store(bytes)
        {
            return false;
        }
        self.insert(HeldFrame {
            message,
            depositor,
            accepted_at_ms,
            class,
            bytes,
            tried: HashSet::new(),
            in_flight: None,
        });
        true
    }

    /// Drops every record whose acceptance time is older than `hold_ms` at
    /// `now_ms`, returning their identifiers so the caller deletes the
    /// records. Judged against the hold in force now, not the one at
    /// acceptance, so lowering the dial expires records already held.
    ///
    /// A record dated in the future is kept: a clock that moved back is not a
    /// reason to drop other people's traffic, and it ages out once the clock
    /// passes its acceptance time again.
    pub(crate) fn expire(&mut self, now_ms: i64, hold_ms: u64) -> Vec<MessageId> {
        let hold = i64::try_from(hold_ms).unwrap_or(i64::MAX);
        let expired: Vec<MessageId> = self
            .entries
            .values()
            .filter(|held| now_ms.saturating_sub(held.accepted_at_ms) > hold)
            .map(|held| held.message.id.clone())
            .collect();
        for id in &expired {
            self.remove_inner(id);
            self.stats.expired = self.stats.expired.saturating_add(1);
        }
        expired
    }

    /// Held frames eligible for re-origination toward `peer`, oldest first.
    ///
    /// Never the depositor's own frames back to it; never a frame already
    /// queued toward someone; at most once per distinct neighbour during the
    /// hold, except toward the frame's own recipient, which is the delivery
    /// attempt itself and is retried whenever that neighbour appears.
    pub(crate) fn candidates_for(&self, peer: &str) -> Vec<MessageId> {
        self.order
            .iter()
            .filter_map(|id| self.entries.get(id))
            .filter(|held| held.depositor != peer && held.in_flight.is_none())
            .filter(|held| {
                held.message.recipient.as_str() == peer
                    || (!held.tried.contains(peer)
                        && held.tried.len() < MAX_CUSTODY_NEIGHBOURS_PER_HOLD)
            })
            .map(|held| held.message.id.clone())
            .collect()
    }

    /// Records that a marked forward of `id` is queued toward `peer`.
    pub(crate) fn mark_queued(&mut self, id: &MessageId, peer: &str) {
        if let Some(held) = self.entries.get_mut(id) {
            held.tried.insert(peer.to_string());
            held.in_flight = Some(peer.to_string());
        }
    }

    /// A marked forward reached no link and came back from the drop point.
    /// Not a deposit, not a refusal, not counted.
    pub(crate) fn returned(&mut self, id: &MessageId) {
        if let Some(held) = self.entries.get_mut(id) {
            held.in_flight = None;
        }
    }

    /// A marked forward was transmitted to a neighbour that is not the
    /// recipient. The frame stays held until its hold ends.
    pub(crate) fn record_re_originated(&mut self, id: &MessageId) {
        if let Some(held) = self.entries.get_mut(id) {
            held.in_flight = None;
        }
        self.stats.re_originated = self.stats.re_originated.saturating_add(1);
    }

    /// A marked forward was handed to its recipient: the frame leaves custody.
    /// Returns the record so the caller deletes it.
    pub(crate) fn record_delivered(&mut self, id: &MessageId) -> Option<HeldFrame> {
        let held = self.remove_inner(id)?;
        self.stats.delivered = self.stats.delivered.saturating_add(1);
        Some(held)
    }

    pub(crate) fn record_receipt_sent(&mut self) {
        self.stats.receipts_sent = self.stats.receipts_sent.saturating_add(1);
    }

    pub(crate) fn record_receipt_dropped(&mut self) {
        self.stats.receipts_dropped = self.stats.receipts_dropped.saturating_add(1);
    }

    pub(crate) fn record_receipt_received(&mut self) {
        self.stats.receipts_received = self.stats.receipts_received.saturating_add(1);
    }

    pub(crate) fn record_receipt_ignored(&mut self) {
        self.stats.receipts_ignored = self.stats.receipts_ignored.saturating_add(1);
    }

    /// Drops every held frame and resets the counters, returning the
    /// identifiers so the caller deletes the records.
    pub(crate) fn erase(&mut self) -> Vec<MessageId> {
        let ids: Vec<MessageId> = self.entries.keys().cloned().collect();
        self.entries.clear();
        self.order.clear();
        self.per_depositor.clear();
        self.total = Usage::default();
        self.stats = CustodyStats::default();
        ids
    }
}

/// The receipt body: `{"v":1,"id":"<held frame id>","hold_ms":<u64>}`.
///
/// `hold_ms` is relative to the receipt's own timestamp, so the depositor
/// applies it to its own clock and skew cannot expire a valid receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CustodyReceipt {
    pub(crate) v: u8,
    pub(crate) id: String,
    pub(crate) hold_ms: u64,
}

/// Why a receipt body was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReceiptDecodeError {
    /// Not a JSON object, or a field of the wrong shape.
    Malformed,
    /// A body version this build does not know.
    UnknownVersion,
    /// An empty identifier names nothing.
    EmptyId,
}

/// Encodes a receipt for `id`, prefix included.
pub(crate) fn encode_receipt(id: &str, hold_ms: u64) -> String {
    let body = CustodyReceipt {
        v: CUSTODY_RECEIPT_VERSION,
        id: id.to_string(),
        hold_ms,
    };
    // Three scalar fields cannot fail to serialize.
    let json = serde_json::to_string(&body).unwrap_or_default();
    format!("{}{}", internal_prefixes::CUSTODY_RECEIPT, json)
}

/// Decodes a receipt body (the text after the prefix).
///
/// The version is read before the body, so a future shape is refused on its
/// version rather than failing to parse; unknown fields are ignored.
pub(crate) fn decode_receipt(body: &str) -> Result<CustodyReceipt, ReceiptDecodeError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| ReceiptDecodeError::Malformed)?;
    if !value.is_object() {
        return Err(ReceiptDecodeError::Malformed);
    }
    match value.get("v").and_then(serde_json::Value::as_u64) {
        Some(v) if v == u64::from(CUSTODY_RECEIPT_VERSION) => {}
        Some(_) => return Err(ReceiptDecodeError::UnknownVersion),
        None => return Err(ReceiptDecodeError::Malformed),
    }
    let receipt: CustodyReceipt =
        serde_json::from_value(value).map_err(|_| ReceiptDecodeError::Malformed)?;
    if receipt.id.is_empty() {
        return Err(ReceiptDecodeError::EmptyId);
    }
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, UserId};

    fn config() -> CustodyConfig {
        CustodyConfig {
            enabled: true,
            ..CustodyConfig::default()
        }
    }

    /// A message id derived from a label: ids are UUIDs on the wire, and a
    /// test reads better naming them.
    fn mid(label: &str) -> MessageId {
        let mut acc: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in label.bytes() {
            acc ^= u64::from(byte);
            acc = acc.wrapping_mul(0x0100_0000_01b3);
        }
        MessageId::from_str(&format!(
            "00000000-0000-4000-8000-{:012x}",
            acc & 0xffff_ffff_ffff
        ))
        .unwrap()
    }

    fn sealed(from: &str, to: &str, id: &str) -> Message {
        let mut message = Message::new(
            UserId::new(from).unwrap(),
            UserId::new(to).unwrap(),
            AppId::new("app").unwrap(),
            format!("{}opaque-{id}", internal_prefixes::ENCRYPTED),
        );
        message.id = mid(id);
        message
    }

    fn candidate<'a>(message: Message, from: &'a str) -> CustodyCandidate<'a> {
        CustodyCandidate {
            message,
            request: Some(CUSTODY_CLASS_DATA),
            arrival_peer: Some(from),
            session_tier: true,
            battery_ok: true,
            now_ms: 1_000,
        }
    }

    #[test]
    fn a_deposit_from_the_depositor_over_its_own_link_is_accepted() {
        let mut store = CustodyStore::new(config());
        let accepted = store
            .judge(candidate(sealed("alice", "carol", "m1"), "alice"))
            .expect("accepted");
        assert_eq!(accepted.depositor, "alice");
        assert_eq!(accepted.class, CUSTODY_CLASS_DATA);
        assert!(store.contains(&mid("m1")));
        assert_eq!(store.stats().accepted, 1);
        assert_eq!(store.stats().held, 1);
    }

    #[test]
    fn every_refusal_reason_is_reachable_and_counted() {
        let refuse = |store: &mut CustodyStore, c: CustodyCandidate<'_>, expected| {
            let before = store.stats().refusals(expected);
            assert_eq!(store.judge(c).err(), Some(expected));
            assert_eq!(store.stats().refusals(expected), before + 1);
        };

        let mut off = CustodyStore::new(CustodyConfig::default());
        refuse(
            &mut off,
            candidate(sealed("alice", "carol", "m1"), "alice"),
            CustodyRefusal::Disabled,
        );

        let mut store = CustodyStore::new(config());
        let mut no_request = candidate(sealed("alice", "carol", "m1"), "alice");
        no_request.request = None;
        refuse(&mut store, no_request, CustodyRefusal::NoRequest);

        let mut unknown = candidate(sealed("alice", "carol", "m1"), "alice");
        unknown.request = Some("media");
        refuse(&mut store, unknown, CustodyRefusal::UnknownClass);

        let mut plain = sealed("alice", "carol", "m1");
        plain.content = "hello".to_string();
        refuse(
            &mut store,
            candidate(plain, "alice"),
            CustodyRefusal::NotSealed,
        );

        let mut unproven = candidate(sealed("alice", "carol", "m1"), "alice");
        unproven.arrival_peer = None;
        refuse(&mut store, unproven, CustodyRefusal::UnprovenPeer);

        refuse(
            &mut store,
            candidate(sealed("alice", "carol", "m1"), "bob"),
            CustodyRefusal::NotDepositor,
        );

        let mut stranger = candidate(sealed("alice", "carol", "m1"), "alice");
        stranger.session_tier = false;
        refuse(&mut store, stranger, CustodyRefusal::StrangerRefused);

        let mut flat = candidate(sealed("alice", "carol", "m1"), "alice");
        flat.battery_ok = false;
        refuse(&mut store, flat, CustodyRefusal::Battery);

        // Depositor full, under drop-newest so nothing is evicted.
        let mut tight = CustodyStore::new(CustodyConfig {
            max_entries_per_depositor: 1,
            overflow_policy: OverflowPolicy::DropNewest,
            ..config()
        });
        tight
            .judge(candidate(sealed("alice", "carol", "m1"), "alice"))
            .unwrap();
        refuse(
            &mut tight,
            candidate(sealed("alice", "carol", "m2"), "alice"),
            CustodyRefusal::DepositorFull,
        );

        // Store full: two depositors, a global cap of one.
        let mut global = CustodyStore::new(CustodyConfig {
            max_entries: 1,
            overflow_policy: OverflowPolicy::DropNewest,
            ..config()
        });
        global
            .judge(candidate(sealed("alice", "carol", "m1"), "alice"))
            .unwrap();
        refuse(
            &mut global,
            candidate(sealed("bob", "carol", "m2"), "bob"),
            CustodyRefusal::StoreFull,
        );
    }

    #[test]
    fn a_duplicate_is_neither_stored_nor_counted_as_a_refusal() {
        let mut store = CustodyStore::new(config());
        store
            .judge(candidate(sealed("alice", "carol", "m1"), "alice"))
            .unwrap();
        let verdict = store.judge(candidate(sealed("alice", "carol", "m1"), "alice"));
        assert_eq!(verdict.err(), Some(CustodyRefusal::Duplicate));
        assert_eq!(store.len(), 1);
        assert_eq!(store.stats().accepted, 1);
        assert_eq!(store.stats().duplicates, 1);
        for reason in CustodyRefusal::ALL
            .iter()
            .filter(|reason| **reason != CustodyRefusal::Duplicate)
        {
            assert_eq!(store.stats().refusals(*reason), 0, "{reason:?}");
        }
    }

    #[test]
    fn drop_oldest_evicts_the_depositors_own_oldest_frame_first() {
        let mut store = CustodyStore::new(CustodyConfig {
            max_entries_per_depositor: 1,
            ..config()
        });
        store
            .judge(candidate(sealed("alice", "carol", "a1"), "alice"))
            .unwrap();
        store
            .judge(candidate(sealed("bob", "carol", "b1"), "bob"))
            .unwrap();
        let accepted = store
            .judge(candidate(sealed("alice", "carol", "a2"), "alice"))
            .unwrap();
        assert_eq!(accepted.evicted, vec![mid("a1")]);
        assert!(store.contains(&mid("b1")));
        assert!(store.contains(&mid("a2")));
        assert_eq!(store.stats().evicted, 1);
    }

    #[test]
    fn a_stranger_is_admitted_only_under_the_stranger_tier() {
        let mut store = CustodyStore::new(CustodyConfig {
            stranger_max_entries: 1,
            stranger_max_bytes: 128 * 1024,
            ..config()
        });
        let mut stranger = candidate(sealed("mallory", "carol", "s1"), "mallory");
        stranger.session_tier = false;
        store.judge(stranger).expect("one stranger frame");
        let mut second = candidate(sealed("mallory", "carol", "s2"), "mallory");
        second.session_tier = false;
        // The tier is one entry deep, and drop-oldest keeps it that deep.
        let accepted = store.judge(second).expect("evicts the first");
        assert_eq!(accepted.evicted.len(), 1);
    }

    #[test]
    fn expiry_is_judged_in_wall_time_against_the_hold_in_force() {
        let mut store = CustodyStore::new(config());
        store
            .judge(candidate(sealed("alice", "carol", "m1"), "alice"))
            .unwrap();
        assert!(store.expire(1_000 + 5_000, 6_000).is_empty());
        // A lowered hold expires a record accepted under a longer one.
        assert_eq!(store.expire(1_000 + 5_000, 4_000).len(), 1);
        assert_eq!(store.stats().expired, 1);
        assert!(store.is_empty());
    }

    #[test]
    fn a_record_dated_in_the_future_is_kept() {
        let mut store = CustodyStore::new(config());
        let mut future = candidate(sealed("alice", "carol", "m1"), "alice");
        future.now_ms = 10_000;
        store.judge(future).unwrap();
        assert!(store.expire(1_000, 1).is_empty());
    }

    #[test]
    fn redelivery_candidates_skip_the_depositor_and_neighbours_already_tried() {
        let mut store = CustodyStore::new(config());
        store
            .judge(candidate(sealed("alice", "carol", "m1"), "alice"))
            .unwrap();
        let id = mid("m1");
        assert!(
            store.candidates_for("alice").is_empty(),
            "never back to the depositor"
        );
        assert_eq!(store.candidates_for("bob"), vec![id.clone()]);
        store.mark_queued(&id, "bob");
        assert!(store.candidates_for("bob").is_empty(), "in flight");
        store.record_re_originated(&id);
        assert!(store.candidates_for("bob").is_empty(), "once per neighbour");
        assert_eq!(store.candidates_for("dave"), vec![id.clone()]);
        // The recipient is always worth trying again.
        store.mark_queued(&id, "carol");
        store.returned(&id);
        assert_eq!(store.candidates_for("carol"), vec![id]);
    }

    #[test]
    fn delivery_releases_the_frame_and_re_origination_keeps_it() {
        let mut store = CustodyStore::new(config());
        store
            .judge(candidate(sealed("alice", "carol", "m1"), "alice"))
            .unwrap();
        let id = mid("m1");
        store.mark_queued(&id, "bob");
        store.record_re_originated(&id);
        assert!(store.contains(&id));
        assert_eq!(store.stats().re_originated, 1);
        store.mark_queued(&id, "carol");
        assert!(store.record_delivered(&id).is_some());
        assert!(!store.contains(&id));
        assert_eq!(store.stats().delivered, 1);
        assert_eq!(store.stats().held, 0);
    }

    #[test]
    fn erase_drops_everything_and_resets_the_counters() {
        let mut store = CustodyStore::new(config());
        store
            .judge(candidate(sealed("alice", "carol", "m1"), "alice"))
            .unwrap();
        let ids = store.erase();
        assert_eq!(ids, vec![mid("m1")]);
        assert!(store.is_empty());
        assert_eq!(store.stats(), CustodyStats::default());
    }

    #[test]
    fn restore_refuses_a_duplicate_and_never_counts_an_acceptance() {
        let mut store = CustodyStore::new(config());
        assert!(store.admit_restored(
            sealed("alice", "carol", "m1"),
            "alice".into(),
            5,
            CUSTODY_CLASS_DATA.into()
        ));
        assert!(!store.admit_restored(
            sealed("alice", "carol", "m1"),
            "alice".into(),
            5,
            CUSTODY_CLASS_DATA.into()
        ));
        assert_eq!(store.stats().accepted, 0);
        assert_eq!(store.stats().held, 1);
    }

    #[test]
    fn the_receipt_round_trips_and_refuses_what_the_chapter_refuses() {
        let wire = encode_receipt("m1", 21_600_000);
        assert_eq!(
            wire,
            format!(
                "{}{{\"v\":1,\"id\":\"m1\",\"hold_ms\":21600000}}",
                internal_prefixes::CUSTODY_RECEIPT
            )
        );
        let body = wire
            .strip_prefix(internal_prefixes::CUSTODY_RECEIPT)
            .unwrap();
        let receipt = decode_receipt(body).unwrap();
        assert_eq!(receipt.id, "m1");
        assert_eq!(receipt.hold_ms, 21_600_000);

        assert_eq!(
            decode_receipt("{\"v\":2,\"id\":\"m1\",\"hold_ms\":1}").err(),
            Some(ReceiptDecodeError::UnknownVersion)
        );
        assert_eq!(
            decode_receipt("{\"v\":1,\"id\":\"\",\"hold_ms\":1}").err(),
            Some(ReceiptDecodeError::EmptyId)
        );
        assert_eq!(
            decode_receipt("{\"id\":\"m1\",\"hold_ms\":1}").err(),
            Some(ReceiptDecodeError::Malformed)
        );
        assert_eq!(
            decode_receipt("not json").err(),
            Some(ReceiptDecodeError::Malformed)
        );
        // Unknown fields are ignored, as the chapter requires.
        assert!(decode_receipt("{\"v\":1,\"id\":\"m1\",\"hold_ms\":1,\"x\":true}").is_ok());
    }
}

/// The frozen wire vectors for the custody receipt.
///
/// The conformance surface a second implementation is written against. When
/// one of these fails the wire format has changed: bump the body version and
/// negotiate a new `data_versions` entry. Do NOT edit the expected values to
/// make the test pass; every shipped install still speaks the old ones.
#[cfg(test)]
mod golden_vectors {
    use super::*;

    const VECTORS: &str = include_str!("../../tests/data/custody-receipt-v1.vectors.json");

    fn vectors() -> serde_json::Value {
        serde_json::from_str(VECTORS).expect("the vector file is JSON")
    }

    fn cases<'a>(vectors: &'a serde_json::Value, name: &str) -> &'a Vec<serde_json::Value> {
        vectors[name]
            .as_array()
            .unwrap_or_else(|| panic!("{name} must be an array"))
    }

    #[test]
    fn the_vector_file_is_the_size_it_was() {
        let vectors = vectors();
        assert_eq!(cases(&vectors, "frames").len(), 3);
        assert_eq!(cases(&vectors, "decode_only").len(), 2);
        assert_eq!(cases(&vectors, "rejects").len(), 6);
        assert_eq!(
            vectors["version"].as_u64(),
            Some(u64::from(CUSTODY_RECEIPT_VERSION))
        );
        assert_eq!(
            vectors["prefix"].as_str(),
            Some(internal_prefixes::CUSTODY_RECEIPT)
        );
    }

    #[test]
    fn every_frame_encodes_to_its_vector_and_decodes_back() {
        let vectors = vectors();
        for case in cases(&vectors, "frames") {
            let name = case["name"].as_str().unwrap();
            let id = case["id"].as_str().unwrap();
            let hold_ms = case["hold_ms"].as_u64().unwrap();
            let wire = case["wire"].as_str().unwrap();
            assert_eq!(encode_receipt(id, hold_ms), wire, "{name}: encode");
            let body = wire
                .strip_prefix(internal_prefixes::CUSTODY_RECEIPT)
                .unwrap_or_else(|| panic!("{name}: the vector carries the prefix"));
            let decoded = decode_receipt(body).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(decoded.id, id, "{name}: id");
            assert_eq!(decoded.hold_ms, hold_ms, "{name}: hold_ms");
            assert_eq!(decoded.v, CUSTODY_RECEIPT_VERSION, "{name}: version");
        }
    }

    #[test]
    fn every_decode_only_case_is_accepted() {
        let vectors = vectors();
        for case in cases(&vectors, "decode_only") {
            let name = case["name"].as_str().unwrap();
            let wire = case["wire"].as_str().unwrap();
            let body = wire
                .strip_prefix(internal_prefixes::CUSTODY_RECEIPT)
                .unwrap();
            let decoded = decode_receipt(body).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(decoded.id, case["id"].as_str().unwrap(), "{name}");
            assert_eq!(decoded.hold_ms, case["hold_ms"].as_u64().unwrap(), "{name}");
        }
    }

    #[test]
    fn every_reject_case_is_refused() {
        let vectors = vectors();
        for case in cases(&vectors, "rejects") {
            let name = case["name"].as_str().unwrap();
            let wire = case["wire"].as_str().unwrap();
            let body = wire
                .strip_prefix(internal_prefixes::CUSTODY_RECEIPT)
                .unwrap();
            assert!(decode_receipt(body).is_err(), "{name} must be refused");
        }
    }
}
