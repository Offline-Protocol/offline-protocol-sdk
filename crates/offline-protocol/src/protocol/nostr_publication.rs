//! Nostr key-package publication slots and peer key-package resolution.
//!
//! Cold first contact over Nostr — reaching a peer known only by username, with
//! no prior key-package exchange over some other transport — needs the peer's
//! key package to be *fetchable* rather than pushed. This module owns the
//! publishing half of that: it keeps a small set of single-use MLS key packages
//! standing in addressable relay records, replaces each as it is consumed, and
//! feeds the answers to outbound resolution queries back into the ordinary
//! key-package handler.
//!
//! # Why slots, rather than one record
//!
//! An MLS key package's init key is consumed by the first peer who uses it, so
//! a single replaceable record would mean the second stranger to fetch it
//! builds a Welcome that can never be processed. Each slot therefore holds its
//! own package under its own `d` tag, and
//! [`MlsManager::key_package_by_id`](offline_protocol_mls::MlsManager::key_package_by_id)
//! reporting a package missing is the consumption signal that refills it.
//!
//! Consumption is *local*: an init key leaves OpenMLS provider storage only
//! when this node processes a Welcome built against it. A stranger can drive it
//! only by actually establishing sessions with us, and each one they burn is
//! refilled on the next tick — so the exposure is a narrow window in which cold
//! contact degrades, not a way to disable it.

use super::{internal_prefixes, storage_keys, OfflineProtocol};
use crate::events::SecurityWarningCode;
use crate::{Error, Result};
use offline_protocol_core::{Message, MessagePriority};
use offline_protocol_transport::constants::NOSTR_KEY_PACKAGE_SLOTS;
use offline_protocol_transport::TransportType;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// How often the publication slots are re-scanned. See
/// [`OfflineProtocol::nostr_slot_refresh_due`] for what the interval trades.
const NOSTR_SLOT_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Ceiling on the republish backoff after consecutive publication failures,
/// and the quiet period after which a slot's failure streak resets.
///
/// Republication is otherwise unconditional on every refresh, so a relay that
/// rejects the kind outright — or rate-limits a fresh pubkey bursting the whole
/// slot set at once — would be retried once per slot per refresh forever, never
/// converging and never backing off. Every other retry ladder in this engine is
/// bounded; this one was not.
const NOSTR_PUBLICATION_MAX_BACKOFF: Duration = Duration::from_secs(1800);

/// Minimum gap between [`SecurityWarningCode::NostrKeyPackageSlotExhausted`]
/// emissions.
///
/// The condition it reports (an MLS or storage error refilling a slot) is
/// exactly the kind that persists, and without suppression it would emit once
/// per failing slot per refresh — five events a minute, indefinitely, for a
/// single stuck cause. Matches the suppression the unauthorized-membership
/// report uses for the same reason.
const NOSTR_SLOT_WARNING_SUPPRESS_INTERVAL: Duration = Duration::from_secs(300);

/// One publication slot's persisted state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NostrPublicationSlot {
    /// The addressable event's `d` tag: stable and random, so republishing
    /// replaces this slot's record rather than adding another, and so the set
    /// of tags reveals no ordering or count convention.
    pub(crate) slot_id: String,
    /// The MLS key package currently standing in this slot.
    pub(crate) package_id: String,
}

/// A slot's publication-failure state, driving the republish backoff.
///
/// Memory-only: it exists to stop hammering a relay within one process, and a
/// launch that starts over costs one prompt retry per slot.
#[derive(Debug, Clone)]
pub(crate) struct PublicationBackoff {
    /// Consecutive failures, which set the delay.
    failures: u32,
    /// When the most recent failure was reported.
    last_failure: Instant,
    /// Earliest this slot may be republished.
    retry_at: Instant,
}

impl OfflineProtocol {
    /// Brings the published key-package slots back up to strength.
    ///
    /// Runs on the process tick. Three things can put a slot out of date, and
    /// all three are handled the same way — mint a fresh package, publish it
    /// under the slot's existing `d` tag:
    ///
    /// 1. the slot has never existed (first run, or the persisted map was lost);
    /// 2. its package was consumed by a Welcome we processed;
    /// 3. its package expired.
    ///
    /// Every slot is also republished once per process, because an addressable
    /// record lives on the relays rather than here: a relay that dropped it, or
    /// one added to the configuration since, would otherwise hold nothing.
    /// Republication under the same slot id is idempotent — it replaces.
    pub(crate) fn refresh_nostr_key_package_slots(&mut self) {
        if !self.transport_manager.nostr_cold_contact_active() {
            return;
        }
        if self.mls_manager.is_none() {
            return;
        }
        if !self.nostr_slot_refresh_due() {
            return;
        }

        let now = Instant::now();

        // A slot is marked published when its record is *queued*, which is the
        // only point this layer hears about — so a record that then failed to
        // reach a relay would leave the slot looking healthy forever while the
        // relays hold nothing. The transport reports those back here. Drained
        // after the throttle check on purpose: draining is destructive, and a
        // throttled tick that dropped the reports would lose them outright.
        for slot_id in self.transport_manager.take_failed_nostr_publications() {
            self.nostr_published_slots.remove(&slot_id);
            self.note_publication_failure(&slot_id, now);
        }

        let mut changed = false;
        let mut failed_slots = 0usize;
        for index in 0..NOSTR_KEY_PACKAGE_SLOTS {
            match self.refresh_one_slot(index, now) {
                Ok(true) => changed = true,
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        slot_index = index,
                        error = %e,
                        "Failed to refresh a Nostr key-package publication slot"
                    );
                    failed_slots += 1;
                }
            }
        }

        // The one failure the publication design must not absorb quietly: a
        // slot left standing with a consumed package makes every stranger who
        // fetches it build an unprocessable Welcome, and nothing else in the
        // system would ever say so. Reported once for the pass rather than once
        // per slot, and suppressed between passes — the causes here (MLS or
        // storage errors) persist, and five events a minute forever would bury
        // the signal in its own repetition.
        if failed_slots > 0 {
            self.report_nostr_slot_refresh_failure(failed_slots, now);
        }

        self.prune_nostr_publication_backoff();

        if changed {
            self.persist_nostr_publication_slots();
        }
    }

    /// Emits the slot-exhaustion warning, at most once per suppression window.
    fn report_nostr_slot_refresh_failure(&mut self, failed_slots: usize, now: Instant) {
        if let Some(last) = self.last_nostr_slot_warning {
            if now.duration_since(last) < NOSTR_SLOT_WARNING_SUPPRESS_INTERVAL {
                return;
            }
        }
        self.last_nostr_slot_warning = Some(now);
        self.emit_security_warning(
            &self.config.user_id.clone(),
            SecurityWarningCode::NostrKeyPackageSlotExhausted,
            format!(
                "could not refill {} Nostr key-package publication slot(s); \
                 cold contact over Nostr is degraded until it succeeds",
                failed_slots
            ),
        );
    }

    /// Records a failed publication and pushes the slot's next attempt out.
    ///
    /// The delay doubles from [`NOSTR_SLOT_REFRESH_INTERVAL`] with each
    /// consecutive failure and stops at [`NOSTR_PUBLICATION_MAX_BACKOFF`], so a
    /// relay that rejects the kind is retried on a schedule that converges
    /// instead of once per slot per minute forever. A slot quiet for longer
    /// than the ceiling starts its streak over, so a genuine one-off failure
    /// long after a bad patch still retries promptly.
    fn note_publication_failure(&mut self, slot_id: &str, now: Instant) {
        let entry = self
            .nostr_publication_backoff
            .entry(slot_id.to_string())
            .or_insert(PublicationBackoff {
                failures: 0,
                last_failure: now,
                retry_at: now,
            });

        if now.duration_since(entry.last_failure) > NOSTR_PUBLICATION_MAX_BACKOFF {
            entry.failures = 1;
        } else {
            entry.failures = entry.failures.saturating_add(1);
        }
        entry.last_failure = now;

        // The *first* failure retries on the next refresh: a relay hiccup or a
        // socket dropped mid-flight should not cost a window. Only a slot that
        // keeps failing climbs the ladder. Shift capped well below `u32`'s
        // width; the ceiling clamps the result long before it matters.
        let delay = if entry.failures <= 1 {
            Duration::ZERO
        } else {
            let shift = (entry.failures - 2).min(16);
            NOSTR_SLOT_REFRESH_INTERVAL
                .saturating_mul(1u32 << shift)
                .min(NOSTR_PUBLICATION_MAX_BACKOFF)
        };
        entry.retry_at = now + delay;
    }

    /// Whether `slot_id` is still waiting out a publication backoff.
    fn publication_backoff_active(&self, slot_id: &str, now: Instant) -> bool {
        self.nostr_publication_backoff
            .get(slot_id)
            .is_some_and(|backoff| now < backoff.retry_at)
    }

    /// Drops backoff state for slots that no longer exist.
    ///
    /// The keys are our own slot ids, of which there are at most
    /// `NOSTR_KEY_PACKAGE_SLOTS` — but a lost slot map mints fresh ones, and
    /// the abandoned entries would otherwise linger for the life of the
    /// process.
    fn prune_nostr_publication_backoff(&mut self) {
        if self.nostr_publication_backoff.is_empty() {
            return;
        }
        let live: HashSet<String> = self
            .nostr_publication_slots
            .iter()
            .map(|slot| slot.slot_id.clone())
            .collect();
        self.nostr_publication_backoff
            .retain(|slot_id, _| live.contains(slot_id));
    }

    /// Whether enough time has passed to re-scan the slots.
    ///
    /// Throttled for the same reason session reconciliation is: checking a slot
    /// means asking the MLS layer whether its key package is still there, and
    /// that is a storage read plus a TLS deserialize, validate, and provider
    /// lookup — five of them per pass. On the raw tick that is a steady trickle
    /// of Keychain/Keystore traffic to answer a question whose answer changes
    /// only when a Welcome arrives.
    ///
    /// The interval bounds how long a consumed slot can stand stale, which is
    /// the only thing being traded: a stranger who fetches the stale record in
    /// that window builds a Welcome we cannot process, and retries once their
    /// own send ladder brings them back around.
    fn nostr_slot_refresh_due(&mut self) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_nostr_slot_refresh {
            if now.duration_since(last) < NOSTR_SLOT_REFRESH_INTERVAL {
                return false;
            }
        }
        self.last_nostr_slot_refresh = Some(now);
        true
    }

    /// Refreshes slot `index`, returning whether the persisted map changed.
    fn refresh_one_slot(&mut self, index: usize, now: Instant) -> Result<bool> {
        // The map is dense: position `i` always holds slot `i`. An earlier slot
        // that failed this pass leaves a gap, and filling it out of order would
        // put this slot's record at the wrong position — harmless today, since
        // slots are identified by `slot_id` and never by index, but it makes the
        // index mean two different things. Leave the gap for the next pass.
        if index > self.nostr_publication_slots.len() {
            return Ok(false);
        }

        let existing = self.nostr_publication_slots.get(index).cloned();

        // Checked before the MLS read and before any mint: a slot whose
        // publication keeps failing should cost nothing at all while it waits.
        if let Some(slot) = existing.as_ref() {
            if self.publication_backoff_active(&slot.slot_id, now) {
                return Ok(false);
            }
        }

        let live_package = match existing.as_ref() {
            Some(slot) => self.load_publication_package(&slot.package_id)?,
            None => None,
        };

        // A live package that has already been published this process is
        // nothing to do — the common case, every tick, for every slot.
        if let (Some(slot), Some(_)) = (existing.as_ref(), live_package.as_ref()) {
            if self.nostr_published_slots.contains(&slot.slot_id) {
                return Ok(false);
            }
        }

        let slot_id = match existing.as_ref() {
            Some(slot) => slot.slot_id.clone(),
            None => new_slot_id(),
        };

        // `live_package` is `Some` only when `existing` is, so a live package
        // never adds a slot to the map — it is the republish-this-process case.
        let (bundle, map_changed) = match live_package {
            Some(bundle) => (bundle, false),
            None => {
                let bundle = {
                    let mls = self.mls_manager.as_ref().ok_or(Error::MlsNotInitialized)?;
                    let manager = mls
                        .read()
                        .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
                    manager.generate_publication_key_package()?
                };
                (bundle, true)
            }
        };

        let queued = match self.build_published_key_package_message(&bundle) {
            Ok(message) => self
                .transport_manager
                .publish_nostr_key_package(&slot_id, &message),
            Err(e) => Err(e),
        };

        if let Err(e) = queued {
            // A package minted for a record that was never built or queued is
            // reserved, so the push path will never hand it out, and no slot
            // references it — nothing would reclaim it before its lifetime runs
            // out. Left alone, a persistently failing build would strand fresh
            // provider key material on every refresh.
            if map_changed {
                self.delete_publication_package(&bundle.package_id);
            }
            return Err(e);
        }

        self.nostr_published_slots.insert(slot_id.clone());

        if map_changed {
            let record = NostrPublicationSlot {
                slot_id,
                package_id: bundle.package_id,
            };
            if index < self.nostr_publication_slots.len() {
                self.nostr_publication_slots[index] = record;
            } else {
                self.nostr_publication_slots.push(record);
            }
        }

        Ok(map_changed)
    }

    /// Loads the key package standing in a slot, or `None` if it is gone.
    ///
    /// `None` covers consumed *and* expired: the MLS loader prunes both, and
    /// the slot's response to either is identical.
    fn load_publication_package(
        &self,
        package_id: &str,
    ) -> Result<Option<offline_protocol_mls::KeyPackageBundle>> {
        let mls = self.mls_manager.as_ref().ok_or(Error::MlsNotInitialized)?;
        let manager = mls
            .read()
            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
        Ok(manager.key_package_by_id(package_id)?)
    }

    /// Drops a publication package that never made it into a queued record.
    fn delete_publication_package(&self, package_id: &str) {
        let Some(mls) = self.mls_manager.as_ref() else {
            return;
        };
        let Ok(manager) = mls.read() else {
            return;
        };
        if let Err(e) = manager.delete_key_package(package_id) {
            warn!(
                package_id = %package_id,
                error = %e,
                "Failed to reclaim an unpublished Nostr key package"
            );
        }
    }

    /// Builds the signed protocol message a published record carries.
    ///
    /// Self-addressed: a published record has no recipient in advance, which is
    /// the point of it. The address is inside the Ed25519 signature, so it is
    /// also not something a fetcher may rewrite — which is why the resolution
    /// path feeds the message to the internal handler directly instead of
    /// through the receive loop, whose relay decision keys on the recipient
    /// being us. See [`Self::handle_resolved_key_package`].
    fn build_published_key_package_message(
        &mut self,
        bundle: &offline_protocol_mls::KeyPackageBundle,
    ) -> Result<Message> {
        let payload = self.build_key_package_payload(bundle, false);
        let serialized =
            serde_json::to_string(&payload).map_err(|e| Error::Serialization(e.to_string()))?;
        let content = format!("{}{}", internal_prefixes::KEY_PACKAGE, serialized);

        let self_id = self.config.user_id.clone();
        let mut message =
            self.create_message(&self_id, content, Some(MessagePriority::Low), None)?;
        // Nothing ACKs a published record: it is not delivered to anyone, it is
        // left somewhere to be found. An outbox entry and a retry ladder behind
        // it would retransmit against a peer that does not exist.
        message.requires_ack = false;
        self.sign_control_message(&mut message)?;
        Ok(message)
    }

    /// Handles a key-package record returned by a resolution query.
    ///
    /// Deliberately narrow: this channel answers exactly one question, so
    /// anything that is not a key package is dropped rather than dispatched.
    /// The records come from a public routing tag that anyone may publish to,
    /// and a query returns whatever the relay holds there — without this gate,
    /// a squatter could deliver any internal control frame through a path that
    /// never went past the receive loop's dedup and block checks.
    ///
    /// What survives the gate is *not* trusted here either. It goes through
    /// `process_internal_message_via`, so the Ed25519 control gate and TOFU
    /// decide whose key package it is: a record planted at the queried peer's
    /// tag by somebody else registers under **that** signer's identity, never
    /// under the peer we asked about.
    pub fn handle_resolved_key_package(&mut self, data: &[u8]) -> Result<()> {
        let transport = self
            .transport_manager
            .get_transport(TransportType::Nostr)
            .ok_or_else(|| Error::Other("Nostr transport not installed".to_string()))?;
        let message = transport
            .deserialize_message(data)
            .map_err(|e| Error::Serialization(e.to_string()))?;

        if !message.content.starts_with(internal_prefixes::KEY_PACKAGE) {
            debug!(
                sender = %message.sender,
                "Discarding a non-key-package record returned by a Nostr resolution query"
            );
            return Ok(());
        }

        let sender = message.sender.as_str().to_string();
        if self.is_user_blocked(&sender) {
            debug!(sender = %sender, "Discarding a published key package from a blocked user");
            return Ok(());
        }

        self.process_internal_message_via(&message, Some(TransportType::Nostr));
        Ok(())
    }
}

#[cfg(test)]
impl OfflineProtocol {
    /// The current publication slot map.
    pub(crate) fn nostr_publication_slots_for_test(&self) -> Vec<NostrPublicationSlot> {
        self.nostr_publication_slots.clone()
    }

    /// Drives the slot-exhaustion report directly, standing in for a refresh
    /// pass whose slots failed — which needs a broken MLS or storage layer to
    /// reach naturally.
    pub(crate) fn report_nostr_slot_refresh_failure_for_test(&mut self, failed_slots: usize) {
        self.report_nostr_slot_refresh_failure(failed_slots, Instant::now());
    }

    /// Lets the next tick actually re-scan.
    ///
    /// Every test that ticks twice must call this, or the throttle makes the
    /// second pass a no-op — which for an idempotency test looks exactly like
    /// the property under test holding.
    pub(crate) fn reset_nostr_slot_throttle_for_test(&mut self) {
        self.last_nostr_slot_refresh = None;
    }

    /// Points slot `index` at `package_id` and marks it unpublished, standing
    /// in for "a Welcome consumed this slot's key package" without needing a
    /// real session handshake. Returns the slot's stable id.
    pub(crate) fn force_nostr_slot_package_for_test(
        &mut self,
        index: usize,
        package_id: &str,
    ) -> String {
        let slot_id = self.nostr_publication_slots[index].slot_id.clone();
        self.nostr_publication_slots[index].package_id = package_id.to_string();
        slot_id
    }
}

/// Mints a stable, random addressable-slot id.
///
/// Random rather than an index, following the same reasoning Marmot gives for
/// its publication-slot ids: a `d` tag of `0`..`4` would advertise both the
/// convention and the slot count to anyone who fetches one record, and would
/// make two installs' records trivially alignable.
fn new_slot_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl OfflineProtocol {
    /// Persists the publication slot map.
    pub(crate) fn persist_nostr_publication_slots(&mut self) {
        let Some(storage) = self.protocol_state_storage.clone() else {
            return;
        };
        let bytes = match serde_json::to_vec(&self.nostr_publication_slots) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "Failed to serialize Nostr publication slots");
                return;
            }
        };
        if let Err(e) = self.write_state_record(
            storage.as_ref(),
            storage_keys::NOSTR_KEY_PACKAGE_SLOTS,
            storage_keys::NOSTR_KEY_PACKAGE_SLOTS_ID,
            &bytes,
        ) {
            warn!(error = %e, "Failed to persist Nostr publication slots");
        }
    }

    /// Restores the publication slot map.
    ///
    /// Every failure mode lands on the same benign outcome — an empty map, so
    /// the next tick mints fresh slots under fresh ids. The records left at the
    /// abandoned slot ids are not orphans that need collecting: they expire
    /// with the key packages inside them, and until then they resolve to
    /// packages this install still holds.
    pub(crate) fn restore_nostr_publication_slots(&mut self) {
        let Some(storage) = self.protocol_state_storage.clone() else {
            return;
        };

        let data = match self.read_state_record(
            storage.as_ref(),
            storage_keys::NOSTR_KEY_PACKAGE_SLOTS,
            storage_keys::NOSTR_KEY_PACKAGE_SLOTS_ID,
        ) {
            Ok(Some(data)) => data,
            Ok(None) => return,
            Err(e) => {
                warn!(error = %e, "Failed to read Nostr publication slots; starting fresh");
                return;
            }
        };

        match serde_json::from_slice::<Vec<NostrPublicationSlot>>(&data) {
            Ok(slots) => {
                // Truncate rather than reject if the configured slot count has
                // shrunk: the surplus records expire on their own.
                self.nostr_publication_slots = slots;
                self.nostr_publication_slots
                    .truncate(NOSTR_KEY_PACKAGE_SLOTS);
                debug!(
                    slots = self.nostr_publication_slots.len(),
                    "Restored Nostr publication slots"
                );
            }
            Err(e) => {
                warn!(error = %e, "Corrupted Nostr publication slots; starting fresh");
            }
        }
    }
}
