//! The engine's custody seams (`docs/spec/custody.md`).
//!
//! The store in `custody.rs` decides; this file is where the engine feeds it:
//! the drop point at the end of the forwarding queue, redelivery on neighbour
//! discovery, the receipt in both directions, the sweep, the durable records
//! and the erase. Each seam is small on purpose, so the invariants the chapter
//! states can be read off the call sites: nothing here touches the suppression
//! cache, settles an outbox entry, or requests an acknowledgement.

use std::time::{Duration, Instant};

use chrono::Utc;
use offline_protocol_core::{Message, MessageId, MessagePriority};
use offline_protocol_router::RelayPriority;
use tracing::{debug, info, warn};

use super::custody::{
    decode_receipt, encode_receipt, CustodyCandidate, CustodyRefusal, CustodyStats, HeldFrame,
    MAX_CUSTODY_RECEIPTS_PER_MESSAGE,
};
use super::mesh_relay::{PendingRelay, RelayRejection};
use super::prefixes::internal_prefixes;
use super::storage::PruneAllowance;
use super::types::{storage_keys, CustodyRecord, CUSTODY_RECORD_VERSION};
use super::OfflineProtocol;
use crate::config::CustodyConfig;
use crate::{Error, ProtocolStateError, Result};

/// How often held records are swept for expiry, and receipt suppressions for
/// their end. Holds are hours, so a minute between sweeps costs nothing
/// observable and keeps the tick free of a walk over the store.
pub(crate) const CUSTODY_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

impl OfflineProtocol {
    /// What this device has done as a custodian and as a depositor, and what
    /// it is holding right now. See [`CustodyStats`].
    pub fn custody_stats(&self) -> CustodyStats {
        self.custody.stats()
    }

    /// The custody configuration in force.
    pub fn custody_config(&self) -> &CustodyConfig {
        self.custody.config()
    }

    /// Deletes every held frame and resets the custody counters.
    ///
    /// Callable on its own, and called by the data-layer wipe: a custody
    /// store that survived a logout would hold other people's traffic past
    /// the point the user asked for erasure. Attempts every record and
    /// reports the first failure, like the data wipe: answering `Ok` for an
    /// erase that left records behind is the worst shape this call can take.
    pub fn erase_custody(&mut self) -> Result<()> {
        let ids = self.custody.erase();
        self.custody_receipts.clear();
        let Some(storage) = self.protocol_state_storage.clone() else {
            return Ok(());
        };

        let mut first_error: Option<String> = None;
        let mut record_error = |err: ProtocolStateError| {
            if first_error.is_none() {
                first_error = Some(err.to_string());
            }
        };

        for id in ids {
            if let Err(err) = storage.delete(storage_keys::CUSTODY, &id.as_str()) {
                record_error(err);
            }
        }
        // Records the store never held this launch go too: a tail a restore
        // left in place for lack of room, or a record written by a build
        // whose version this one dropped.
        match storage.list_keys(storage_keys::CUSTODY) {
            Ok(keys) => {
                for key in keys {
                    if let Err(err) = storage.delete(storage_keys::CUSTODY, &key) {
                        record_error(err);
                    }
                }
            }
            Err(ProtocolStateError::NotFound(_)) => {}
            Err(err) => record_error(err),
        }

        match first_error {
            Some(detail) => Err(Error::Other(format!(
                "failed to delete every custody record: {detail}"
            ))),
            None => Ok(()),
        }
    }

    /// The drop point: a forward that waited past the overdue cut-off without
    /// reaching a link.
    ///
    /// An ordinary forward is judged by the acceptance table; its id was
    /// released from the suppression cache when the governor abandoned it,
    /// which is what makes "a custodian never blanks the route it is holding"
    /// a consequence of existing code. A marked held forward is returned to
    /// the store: not a deposit, not a refusal, not counted.
    pub(super) fn judge_at_drop_point(&mut self, relay: PendingRelay) {
        if relay.custody_target.is_some() {
            self.return_held_to_store(relay);
            return;
        }

        let PendingRelay {
            message,
            arrival_peer,
            custody_request,
            ..
        } = relay;

        let session_tier = arrival_peer
            .as_deref()
            .is_some_and(|peer| self.confirmed_sessions.contains(peer));
        // The battery snapshot locks and allocates across every transport;
        // skipped while custody is off, where the table refuses first anyway.
        let battery_ok = !self.custody.is_enabled() || self.battery_allows_relaying();
        let now_ms = Utc::now().timestamp_millis();

        let message_id = message.id.clone();
        let verdict = self.custody.judge(CustodyCandidate {
            message,
            request: custody_request.as_deref(),
            arrival_peer: arrival_peer.as_deref(),
            session_tier,
            battery_ok,
            now_ms,
        });

        match verdict {
            Ok(accepted) => {
                for evicted in &accepted.evicted {
                    self.delete_custody_record(evicted);
                }
                if let Some(held) = self.custody.get(&accepted.id) {
                    self.persist_custody_record(held);
                }
                debug!(
                    message_id = %message_id,
                    depositor = %accepted.depositor,
                    class = %accepted.class,
                    evicted = accepted.evicted.len(),
                    "Took a frame into custody"
                );
                self.send_custody_receipt(&accepted.depositor, &accepted.id);
            }
            Err(CustodyRefusal::Duplicate) => {
                debug!(message_id = %message_id, "Deposit of a frame already held; absorbed");
            }
            Err(refusal) => {
                debug!(
                    message_id = %message_id,
                    reason = refusal.as_str(),
                    "Frame abandoned at the drop point and not taken into custody"
                );
            }
        }
    }

    /// A marked held forward reached no link: back to the store, unjudged.
    pub(super) fn return_held_to_store(&mut self, relay: PendingRelay) {
        self.custody.returned(&relay.message.id);
    }

    /// A marked held forward went out. To its recipient, the frame leaves
    /// custody; to anyone else, it stays held until its hold ends.
    pub(super) fn record_custody_transmission(
        &mut self,
        relay: &PendingRelay,
        reached_recipient: bool,
    ) {
        let id = &relay.message.id;
        if reached_recipient {
            if self.custody.record_delivered(id).is_some() {
                self.delete_custody_record(id);
            }
        } else {
            self.custody.record_re_originated(id);
        }
    }

    /// Queues every eligible held frame toward a neighbour that just
    /// appeared, through the governor's held-frame intake.
    ///
    /// Obeys `allow_relay` and the battery floors like any forward: a
    /// custodian is a forwarder for the frames it holds. A refusal for rate
    /// or room leaves the frame held and untried, so the next discovery of
    /// the same neighbour tries again; the store itself bounds how many
    /// distinct neighbours a frame is offered to during its hold.
    pub(super) fn redeliver_custody_to(&mut self, peer: &str) {
        if !self.custody.is_enabled() || self.custody.is_empty() {
            return;
        }
        if !self.config.relay.allow_relay
            || matches!(self.config.relay.relay_priority, RelayPriority::Never)
        {
            return;
        }
        if !self.battery_allows_relaying() {
            return;
        }

        let now = Instant::now();
        for id in self.custody.candidates_for(peer) {
            let Some(message) = self.custody.get(&id).map(|held| held.message.clone()) else {
                continue;
            };
            match self.mesh_relay.admit_held(message, peer, now) {
                Ok(()) => {
                    self.custody.mark_queued(&id, peer);
                    debug!(
                        message_id = %id,
                        neighbor = %peer,
                        cause = "custody_redelivery",
                        "Queued a held frame toward a neighbor"
                    );
                }
                Err(RelayRejection::QueueFull) => break,
                Err(_) => continue,
            }
        }
    }

    /// Drops held records past the hold in force and receipt suppressions
    /// past their end. Throttled to [`CUSTODY_SWEEP_INTERVAL`].
    pub(super) fn sweep_custody(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.custody_last_sweep) < CUSTODY_SWEEP_INTERVAL {
            return;
        }
        self.custody_last_sweep = now;
        self.sweep_custody_now(now, Utc::now().timestamp_millis());
    }

    /// [`Self::sweep_custody`] without the throttle, at the given clocks.
    pub(super) fn sweep_custody_now(&mut self, now: Instant, now_ms: i64) {
        self.custody_receipts.retain(|_, custodians| {
            custodians.retain(|_, until| *until > now);
            !custodians.is_empty()
        });

        if self.custody.is_empty() {
            return;
        }
        let hold_ms = self.custody.config().hold_ms;
        let expired = self.custody.expire(now_ms, hold_ms);
        for id in &expired {
            self.delete_custody_record(id);
        }
        if !expired.is_empty() {
            debug!(
                count = expired.len(),
                "Expired held frames at the end of their hold"
            );
        }
    }

    /// Answers a deposit with the receipt, once, over the arrival link.
    ///
    /// Never through the outbox, the mesh offer, a relay or a gateway: it
    /// names a neighbour that was on a link a moment ago, and a receipt that
    /// cannot be put on that link is dropped and counted. Never requests an
    /// acknowledgement. Sent only to a depositor whose key package advertised
    /// the custody entry, because a peer without it does not know the prefix
    /// as a control frame.
    fn send_custody_receipt(&mut self, depositor: &str, held_id: &MessageId) {
        if !self.peer_data_custody.contains(depositor) {
            debug!(
                depositor = %depositor,
                "Receipt withheld: the depositor does not advertise the custody entry"
            );
            self.custody.record_receipt_dropped();
            return;
        }
        if self.mls_manager.is_none() {
            // An unsigned receipt is refused by the depositor's control gate
            // before it is read, so there is nothing to send.
            self.custody.record_receipt_dropped();
            return;
        }

        let content = encode_receipt(&held_id.as_str(), self.custody.config().hold_ms);
        let mut message = match self.create_message(
            depositor,
            content,
            Some(MessagePriority::Low),
            None,
        ) {
            Ok(message) => message,
            Err(err) => {
                warn!(depositor = %depositor, error = %err, "Could not build a custody receipt");
                self.custody.record_receipt_dropped();
                return;
            }
        };
        message.requires_ack = false;
        if let Err(err) = self.sign_control_message(&mut message) {
            warn!(depositor = %depositor, error = %err, "Could not sign a custody receipt");
            self.custody.record_receipt_dropped();
            return;
        }

        // Ours: a copy circling back through the mesh is recognised rather
        // than carried, exactly as an offered frame is.
        self.deduplicator.mark_seen_local(message.id.clone());
        self.mesh_relay.mark_handled(&message.id.as_str());

        match self.transport_manager.send_to_neighbor(depositor, &message) {
            Ok(transport) => {
                debug!(
                    depositor = %depositor,
                    held = %held_id,
                    transport = ?transport,
                    "Sent a custody receipt"
                );
                self.custody.record_receipt_sent();
            }
            Err(err) => {
                debug!(
                    depositor = %depositor,
                    held = %held_id,
                    error = %err,
                    "Dropped a custody receipt: the arrival link is gone"
                );
                self.custody.record_receipt_dropped();
            }
        }
    }

    /// A receipt from a custodian, past the control gate.
    ///
    /// Suppresses further deposit requests for that identifier toward that
    /// custodian until the hold elapses, and nothing else: the outbox entry,
    /// the acknowledgement timer, the retry entry and any park are untouched.
    /// A receipt naming nothing in the outbox is ignored and counted; that is
    /// the ordinary shape of one arriving after the recipient's
    /// acknowledgement settled the message, and the shape a forged receipt
    /// for a never-sent identifier would take.
    pub(super) fn handle_custody_receipt(&mut self, custodian: &str, body: &str) {
        let receipt = match decode_receipt(body) {
            Ok(receipt) => receipt,
            Err(err) => {
                debug!(custodian = %custodian, error = ?err, "Refused a malformed custody receipt");
                self.custody.record_receipt_ignored();
                return;
            }
        };
        let Ok(id) = MessageId::from_str(&receipt.id) else {
            self.custody.record_receipt_ignored();
            return;
        };
        if !self.outbox.contains_key(&id) {
            debug!(custodian = %custodian, message_id = %id, "Custody receipt names nothing in the outbox");
            self.custody.record_receipt_ignored();
            return;
        }

        // A hold longer than this device's own outbox window buys nothing:
        // the entry is gone before the suppression would end. Clamping here
        // also keeps the instant arithmetic in range.
        let hold_ms = receipt
            .hold_ms
            .min(self.config.reliability.retry.outbox_max_lifetime_ms);
        let until = Instant::now()
            .checked_add(Duration::from_millis(hold_ms))
            .unwrap_or_else(Instant::now);

        let custodians = self.custody_receipts.entry(id.clone()).or_default();
        if !custodians.contains_key(custodian)
            && custodians.len() >= MAX_CUSTODY_RECEIPTS_PER_MESSAGE
        {
            // Bounded against a neighbourhood that mints addresses: the
            // suppression that ends soonest is the one worth least.
            if let Some(oldest) = custodians
                .iter()
                .min_by_key(|(_, until)| **until)
                .map(|(peer, _)| peer.clone())
            {
                custodians.remove(&oldest);
            }
        }
        custodians.insert(custodian.to_string(), until);
        debug!(custodian = %custodian, message_id = %id, hold_ms, "Recorded a custody receipt");
        self.custody.record_receipt_received();
    }

    /// The class token to write on this device's own frame when it is
    /// offered to neighbours, or `None` when the frame is not a deposit.
    ///
    /// Judged from the plaintext retained for re-sealing on resend: a frame
    /// whose plaintext this device no longer holds, after a restart, is
    /// offered without the key, exactly as the chapter says. Only a sealed
    /// 1:1 frame qualifies, so a group frame, a plaintext frame and a media
    /// chunk never carry one.
    pub(super) fn custody_class_for(&self, message: &Message) -> Option<&'static str> {
        if !message.content.starts_with(internal_prefixes::ENCRYPTED) {
            return None;
        }
        let plaintext = self
            .outbox
            .get(&message.id)
            .and_then(|entry| entry.reseal.as_ref())
            .map(|reseal| reseal.content.as_str())
            .or_else(|| {
                self.pending_reseal
                    .get(&message.id)
                    .map(|reseal| reseal.content.as_str())
            })?;
        #[cfg(feature = "data")]
        {
            super::data_sync::custody_class_of_plaintext(plaintext)
        }
        #[cfg(not(feature = "data"))]
        {
            // Without the data layer there are no replication frames to
            // deposit, so nothing this device sends is Class A.
            let _ = plaintext;
            None
        }
    }

    /// Whether a receipt from `custodian` still suppresses a deposit request
    /// for `id` toward it.
    pub(super) fn custody_suppressed_toward(&self, id: &MessageId, custodian: &str) -> bool {
        self.custody_receipts
            .get(id)
            .and_then(|custodians| custodians.get(custodian))
            .is_some_and(|until| *until > Instant::now())
    }

    /// Writes one held frame under its own message id. Best-effort, like the
    /// pending-decrypt records: a failed write is logged and the frame still
    /// lives in the store, it just will not survive a restart.
    fn persist_custody_record(&self, held: &HeldFrame) {
        let Some(storage) = &self.protocol_state_storage else {
            return;
        };
        let record = CustodyRecord {
            version: CUSTODY_RECORD_VERSION,
            depositor: held.depositor.clone(),
            message: held.message.clone(),
            accepted_at_ms: held.accepted_at_ms,
            class: held.class.clone(),
        };
        let data = match serde_json::to_vec(&record) {
            Ok(data) => data,
            Err(err) => {
                warn!(message_id = %held.message.id, error = %err, "Failed to serialize a custody record");
                return;
            }
        };
        if let Err(err) = self.write_state_record(
            storage.as_ref(),
            storage_keys::CUSTODY,
            &held.message.id.as_str(),
            &data,
        ) {
            warn!(message_id = %held.message.id, error = %err, "Failed to persist a custody record");
        }
    }

    /// Removes one held frame's record. Logged rather than swallowed: a
    /// delete that silently failed is a record the next launch restores and
    /// redelivers, past the point the store had given it up.
    fn delete_custody_record(&self, id: &MessageId) {
        if let Some(storage) = &self.protocol_state_storage {
            if let Err(err) = storage.delete(storage_keys::CUSTODY, &id.as_str()) {
                warn!(message_id = %id, error = %err, "Failed to delete a custody record");
            }
        }
    }

    /// Restores held frames at launch, under a budget.
    ///
    /// With custody disabled, every record is erased rather than kept:
    /// nothing would ever deliver it. With custody enabled, each record is
    /// dropped when its acceptance time is older than the hold in force and
    /// kept exactly as stored otherwise, oldest first so the budgets in force
    /// admit the frames that have waited longest. Restore never sends
    /// anything. Deletes draw on `allowance` and may be refused, leaving the
    /// record for a later launch; a record the store has no room for is
    /// deleted rather than left, because nothing would ever restore it.
    pub(super) fn restore_custody(&mut self, allowance: &mut PruneAllowance) {
        let Some(storage) = self.protocol_state_storage.clone() else {
            return;
        };
        let keys = match storage.list_keys(storage_keys::CUSTODY) {
            Ok(keys) => keys,
            Err(ProtocolStateError::NotFound(_)) => return,
            Err(err) => {
                warn!(error = %err, "Failed to list custody records");
                return;
            }
        };
        if keys.is_empty() {
            return;
        }

        let cap = self.config.custody.max_entries.saturating_mul(4).max(1);
        let enabled = self.custody.is_enabled();
        let hold = i64::try_from(self.custody.config().hold_ms).unwrap_or(i64::MAX);
        let now_ms = Utc::now().timestamp_millis();
        // A refusing budget on the inbound pool, which no advisory walk
        // shares (see `PruneAllowance::refusing_private`).
        let mut budget = allowance.refusing_private();
        let mut erased = 0usize;
        let mut expired = 0usize;
        let mut deferred = 0usize;

        let mut records: Vec<CustodyRecord> = Vec::new();
        for key in keys.into_iter().take(cap) {
            if !enabled {
                if budget.claim() {
                    self.delete_custody_record_by_key(&key);
                    erased += 1;
                } else {
                    deferred += 1;
                }
                continue;
            }
            let data = match self.read_state_record(storage.as_ref(), storage_keys::CUSTODY, &key) {
                Ok(Some(data)) => data,
                Ok(None) => continue,
                Err(err) => {
                    debug!(key = %key, error = %err, "Custody record could not be read; leaving it");
                    continue;
                }
            };
            let record = match serde_json::from_slice::<CustodyRecord>(&data) {
                Ok(record) if record.version == CUSTODY_RECORD_VERSION => record,
                Ok(record) => {
                    warn!(key = %key, version = record.version, "Dropping a custody record of an unknown version");
                    if budget.claim() {
                        self.delete_custody_record_by_key(&key);
                    }
                    continue;
                }
                Err(err) => {
                    warn!(key = %key, error = %err, "Dropping a corrupted custody record");
                    if budget.claim() {
                        self.delete_custody_record_by_key(&key);
                    }
                    continue;
                }
            };
            if record.message.id.as_str() != key {
                warn!(key = %key, message_id = %record.message.id, "Dropping a custody record filed under another id");
                if budget.claim() {
                    self.delete_custody_record_by_key(&key);
                }
                continue;
            }
            if now_ms.saturating_sub(record.accepted_at_ms) > hold {
                if budget.claim() {
                    self.delete_custody_record_by_key(&key);
                    expired += 1;
                } else {
                    deferred += 1;
                }
                continue;
            }
            records.push(record);
        }

        records.sort_by(|left, right| {
            left.accepted_at_ms
                .cmp(&right.accepted_at_ms)
                .then_with(|| left.message.id.as_str().cmp(&right.message.id.as_str()))
        });

        let mut restored = 0usize;
        for record in records {
            let key = record.message.id.as_str();
            if self.custody.admit_restored(
                record.message,
                record.depositor,
                record.accepted_at_ms,
                record.class,
            ) {
                restored += 1;
            } else if budget.claim() {
                self.delete_custody_record_by_key(&key);
            } else {
                deferred += 1;
            }
        }

        if enabled {
            info!(restored, expired, deferred, "Restored custody records");
        } else if erased > 0 || deferred > 0 {
            info!(
                erased,
                deferred, "Erased custody records left by a launch with custody enabled"
            );
        }
    }

    fn delete_custody_record_by_key(&self, key: &str) {
        if let Some(storage) = &self.protocol_state_storage {
            if let Err(err) = storage.delete(storage_keys::CUSTODY, key) {
                warn!(key = %key, error = %err, "Failed to delete a custody record");
            }
        }
    }
}
