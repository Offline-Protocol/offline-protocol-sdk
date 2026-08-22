//! Adapters that carry mls-rs's storage traits onto [`LeafStore`].
//!
//! mls-rs asks for two storage providers, and neither shape belongs in a
//! device integrator's lap: they carry mls-rs's own types, they are versioned
//! with a dependency [ADR 0021](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0021-a-leaf-node-speaks-mls.md)
//! calls monitored rather than settled, and getting the write ordering wrong
//! is a confidentiality bug rather than a lost message. So firmware implements
//! one blob store and these adapters do the rest.

use alloc::{format, string::String, sync::Arc, vec::Vec};
use mls_rs_core::{
    error::IntoAnyError,
    group::{EpochRecord, GroupState, GroupStateStorage},
    key_package::{KeyPackageData, KeyPackageStorage},
};
use mls_rs_core::{
    identity::BasicCredential,
    mls_rs_codec::{MlsDecode, MlsEncode},
};
use zeroize::Zeroizing;

use crate::store::{LeafStore, StoreError, KEY_TYPE_GROUP_EPOCH, KEY_TYPE_GROUP_STATE};
use crate::store::{KEY_TYPE_KEY_PACKAGE, KEY_TYPE_PEER};

impl IntoAnyError for StoreError {}

/// How many prior-epoch records a group keeps.
///
/// mls-rs delegates this to the storage provider rather than applying it
/// itself: its own in-memory provider trims to three on every write, and a
/// provider that never trims keeps every epoch a group has ever left. That is
/// two failures rather than one. The records accumulate on a part whose flash
/// is measured in hundreds of kilobytes, and each one holds that epoch's
/// secrets, so retaining them all turns "how far out of order a message may
/// arrive" into "how far back a stolen device decrypts". Three is the window
/// a phone-driven commit cadence needs on a lossy radio, and it is the bound
/// on both.
///
/// This is leaf-side storage policy, not a number the two ends must match:
/// the phone's provider keeps its own history and neither reads the other's.
/// It is therefore declared here rather than in `offline-protocol-sealed`,
/// which is for values a peer would disagree with us about.
pub(crate) const PRIOR_EPOCH_RETENTION: u64 = 3;

/// Renders bytes as lowercase hex, so an arbitrary group id can be a key id.
///
/// Group ids are chosen by whoever created the group and are not required to
/// be printable. Hex is the shortest thing that cannot collide with the `:`
/// this module uses as a separator.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // `write!` would need `core::fmt::Write` in scope and can fail; a
        // table lookup cannot.
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

/// Where a group's current state is kept.
fn state_key(group_id: &[u8]) -> String {
    hex(group_id)
}

/// Where one prior epoch is kept.
fn epoch_key(group_id: &[u8], epoch_id: u64) -> String {
    format!("{}:{}", hex(group_id), epoch_id)
}

/// Where the highest stored epoch id is kept.
fn max_epoch_key(group_id: &[u8]) -> String {
    format!("{}:max", hex(group_id))
}

/// How many bytes of a state entry carry the sequencing marker: a presence
/// flag and a big-endian epoch id.
const STATE_MARKER_LEN: usize = 9;

/// Puts the sequencing marker in front of the state mls-rs handed us.
///
/// # Why the marker rides inside the state entry
///
/// mls-rs sequences every epoch insert against
/// [`GroupStateStorage::max_epoch_id`]: an inserted record's id must be
/// exactly one above what that returns, or the operation is refused. Nothing
/// in this crate caches either value, so both are read from flash on every
/// operation, and the two therefore have to move together or not at all.
///
/// Held as separate entries they cannot. This seam is atomic per entry rather
/// than across a set, so a cut or a failed write between a marker record and
/// the state leaves the marker one ahead of the state it describes, and from
/// there **every later commit is refused, permanently**: the retry inserts the
/// epoch id the marker has already counted. Reversing the order does not fix
/// it, it moves the same wedge into the other window. What that costs is not a
/// lost frame: the device stops opening anything its peer sends until the
/// peer's own recovery drives a full reset, which on a door lock is an owner
/// standing outside it.
///
/// One entry closes the window. The marker a validation reads is a slice of
/// the same bytes as the state it validates against, so there is no moment in
/// which the two disagree.
fn encode_state_entry(marker: Option<u64>, state: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(STATE_MARKER_LEN + state.len());
    match marker {
        Some(epoch) => {
            out.push(1);
            out.extend_from_slice(&epoch.to_be_bytes());
        }
        // A group that has left no epoch behind yet. mls-rs skips the
        // sequencing check entirely for `None`, which is what a group that was
        // just joined needs.
        None => out.extend_from_slice(&[0u8; STATE_MARKER_LEN]),
    }
    out.extend_from_slice(state);
    out
}

/// Splits a state entry into its marker and the state mls-rs stored.
fn decode_state_entry(raw: &[u8]) -> Result<(Option<u64>, &[u8]), StoreError> {
    if raw.len() < STATE_MARKER_LEN {
        return Err(StoreError::Corrupt(format!(
            "group state entry is {} bytes, too short to carry its epoch marker",
            raw.len()
        )));
    }
    let (header, state) = raw.split_at(STATE_MARKER_LEN);
    let marker = match header[0] {
        0 => None,
        1 => {
            let mut epoch = [0u8; 8];
            epoch.copy_from_slice(&header[1..STATE_MARKER_LEN]);
            Some(u64::from_be_bytes(epoch))
        }
        other => {
            return Err(StoreError::Corrupt(format!(
                "group state entry carries epoch marker flag {other}, expected 0 or 1"
            )))
        }
    };
    Ok((marker, state))
}

/// The sequencing marker in a group's state entry, if it can be read.
///
/// For the erasure sweep in [`LeafDevice::unpair`](crate::LeafDevice::unpair),
/// which needs an anchor rather than a guarantee: every failure here is the
/// same answer, "this record cannot bound anything", and the caller looks
/// elsewhere.
pub(crate) fn state_marker(store: &Arc<dyn LeafStore>, group_key: &str) -> Option<u64> {
    let raw = store.load(KEY_TYPE_GROUP_STATE, group_key).ok()??;
    decode_state_entry(&raw).ok().and_then(|(marker, _)| marker)
}

/// Carries [`GroupStateStorage`] onto the device's blob store.
#[derive(Clone)]
pub(crate) struct GroupStateAdapter {
    store: Arc<dyn LeafStore>,
}

impl GroupStateAdapter {
    pub(crate) fn new(store: Arc<dyn LeafStore>) -> Self {
        Self { store }
    }
}

impl GroupStateStorage for GroupStateAdapter {
    type Error = StoreError;

    fn state(&self, group_id: &[u8]) -> Result<Option<Zeroizing<Vec<u8>>>, Self::Error> {
        let Some(raw) = self
            .store
            .load(KEY_TYPE_GROUP_STATE, &state_key(group_id))?
        else {
            return Ok(None);
        };
        let raw = Zeroizing::new(raw);
        let (_, state) = decode_state_entry(&raw)?;
        Ok(Some(Zeroizing::new(state.to_vec())))
    }

    fn epoch(
        &self,
        group_id: &[u8],
        epoch_id: u64,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, Self::Error> {
        Ok(self
            .store
            .load(KEY_TYPE_GROUP_EPOCH, &epoch_key(group_id, epoch_id))?
            .map(Zeroizing::new))
    }

    /// Writes the epoch records first and the group state last.
    ///
    /// mls-rs asks for one atomic transaction, and this seam offers atomicity
    /// per entry rather than across a set, so the ordering has to carry what
    /// the transaction would have. Prior-epoch records are additive: they let
    /// a message that arrives late still decrypt. Writing them before the
    /// state means a power cut mid-write leaves the **old** state alongside
    /// records it does not yet reference, which costs nothing. The reverse
    /// order would leave a new state whose prior epochs were never written, so
    /// the device would come back having lost exactly the out-of-order
    /// tolerance a lossy radio needs.
    ///
    /// The state and the marker that sequences it go down as **one entry**,
    /// because ordering cannot make those two safe in either direction. See
    /// [`encode_state_entry`] for what a torn write between them costs.
    ///
    /// The caller does not emit anything until this returns `Ok`, so a failure
    /// here is a frame that was never sent rather than state that fell behind
    /// one that was.
    ///
    /// The high-water record and the expired-record deletes both happen
    /// **after** the state write, and neither propagates a failure. Both
    /// follow from the same rule: once the state write has returned, the write
    /// the caller is waiting on is durable, so reporting a failure in
    /// housekeeping would suppress a frame whose state is already on flash.
    /// A record that survives a failed delete is swept by
    /// [`LeafDevice::unpair`](crate::LeafDevice::unpair).
    fn write(
        &mut self,
        state: GroupState,
        epoch_inserts: Vec<EpochRecord>,
        epoch_updates: Vec<EpochRecord>,
    ) -> Result<(), Self::Error> {
        let previous = self.max_epoch_id(&state.id)?;
        let mut marker = previous;

        for record in epoch_inserts.iter().chain(epoch_updates.iter()) {
            self.store.store(
                KEY_TYPE_GROUP_EPOCH,
                &epoch_key(&state.id, record.id),
                &record.data,
            )?;
            marker = Some(marker.map_or(record.id, |held: u64| held.max(record.id)));
        }

        self.store.store(
            KEY_TYPE_GROUP_STATE,
            &state_key(&state.id),
            &encode_state_entry(marker, &state.data),
        )?;

        // The erasure high-water mark, which is a different job from the
        // marker above and is why it is a different record rather than the
        // same one read twice.
        //
        // The marker inside the state entry answers "what may be inserted
        // next", and it goes away with the state it lives in. This record
        // answers "how far up did this group ever write", which
        // [`LeafDevice::unpair`](crate::LeafDevice::unpair) needs in order to
        // bound a sweep over records it cannot enumerate, and it has to be
        // readable in the one case the other is not: a state entry the part
        // hands back as something else. It is deliberately never lowered by
        // trimming, so it stays above every record that could still be there.
        let advanced_to = if marker == previous { None } else { marker };
        if let Some(high) = advanced_to {
            let _ = self.store.store(
                KEY_TYPE_GROUP_EPOCH,
                &max_epoch_key(&state.id),
                &high.to_be_bytes(),
            );
        }

        // One delete per record that entered, which is all the window can
        // lose: mls-rs requires each inserted epoch id to be exactly one above
        // the highest stored, so the window advances by the number of inserts
        // and never skips. Updates rewrite records already inside it.
        for record in epoch_inserts.iter() {
            if let Some(expired) = record.id.checked_sub(PRIOR_EPOCH_RETENTION) {
                let _ = self
                    .store
                    .delete(KEY_TYPE_GROUP_EPOCH, &epoch_key(&state.id, expired));
            }
        }

        Ok(())
    }

    /// The highest epoch id this group has left behind.
    ///
    /// Read from the same entry as the state it belongs to, because mls-rs
    /// sequences every epoch insert against this value and a marker that got
    /// ahead of its state wedges the group permanently. See
    /// [`encode_state_entry`].
    ///
    /// A group with no state has no marker, and that `None` is what makes
    /// mls-rs skip the sequencing check for a group that was just joined.
    fn max_epoch_id(&self, group_id: &[u8]) -> Result<Option<u64>, Self::Error> {
        let Some(raw) = self
            .store
            .load(KEY_TYPE_GROUP_STATE, &state_key(group_id))?
        else {
            return Ok(None);
        };
        let raw = Zeroizing::new(raw);
        let (marker, _) = decode_state_entry(&raw)?;
        Ok(marker)
    }
}

/// How many minted-but-unspent key packages a device keeps.
///
/// mls-rs deletes an entry when a join consumes it, so the only entries that
/// accumulate are packages nobody ever spent: a pairing that was abandoned, or
/// one a stranger provoked. Without a bound each of those is private key
/// material written to flash and never reclaimed, and provoking a mint costs
/// an attacker one signed frame.
///
/// Evicting the oldest is the trade this makes, and it is not free: a peer
/// holding an evicted package can no longer complete a join with it, so a
/// flood turns into a pairing failure rather than a full flash. Four is enough
/// for a household pairing its phones in sequence, and small enough that the
/// residue is bounded.
const MAX_UNSPENT_KEY_PACKAGES: usize = 4;

/// Where the list of unspent key package ids is kept.
///
/// Held under the same key type as the packages themselves. It cannot collide
/// with one: every other id there is [`hex`] output, and `_` is not a hex
/// digit.
const KEY_PACKAGE_INDEX: &str = "__index__";

/// Carries [`KeyPackageStorage`] onto the device's blob store.
///
/// The values here hold the init and leaf-node private keys of a key package
/// the device minted and has not yet spent. mls-rs deletes an entry when the
/// package is consumed by a join, which is why an init key is single use and
/// why a static pairing artifact must never carry one. What it does not do is
/// bound the ones never consumed, so this adapter does: see
/// [`MAX_UNSPENT_KEY_PACKAGES`].
#[derive(Clone)]
pub(crate) struct KeyPackageAdapter {
    store: Arc<dyn LeafStore>,
}

impl KeyPackageAdapter {
    pub(crate) fn new(store: Arc<dyn LeafStore>) -> Self {
        Self { store }
    }

    /// The ids of unspent packages, oldest first.
    ///
    /// A corrupt index is treated as an empty one rather than an error. It is
    /// housekeeping state, and refusing to mint a key package because a list
    /// of previous ones does not parse would turn a recoverable annoyance into
    /// a device that cannot pair.
    fn index(&self) -> Result<Vec<String>, StoreError> {
        let Some(raw) = self.store.load(KEY_TYPE_KEY_PACKAGE, KEY_PACKAGE_INDEX)? else {
            return Ok(Vec::new());
        };
        Ok(serde_json::from_slice(&raw).unwrap_or_default())
    }

    fn save_index(&self, index: &[String]) -> Result<(), StoreError> {
        let encoded = serde_json::to_vec(index)
            .map_err(|e| StoreError::Store(format!("cannot encode key package index: {e}")))?;
        self.store
            .store(KEY_TYPE_KEY_PACKAGE, KEY_PACKAGE_INDEX, &encoded)
    }
}

impl KeyPackageStorage for KeyPackageAdapter {
    type Error = StoreError;

    fn delete(&mut self, id: &[u8]) -> Result<(), Self::Error> {
        let key = hex(id);
        self.store.delete(KEY_TYPE_KEY_PACKAGE, &key)?;

        // Dropped from the index too, so a package a join consumed does not
        // hold a slot against the ones still outstanding.
        //
        // Best effort, because the package itself is already gone, which is
        // what was asked for. mls-rs calls this after it has persisted the
        // group state that consumed the package, so an error here would report
        // a deletion that did happen as one that did not, and the caller would
        // withhold a frame whose state is on flash. A stale entry costs one
        // slot and is evicted in its turn.
        if let Ok(mut index) = self.index() {
            if let Some(at) = index.iter().position(|held| held == &key) {
                index.remove(at);
                let _ = self.save_index(&index);
            }
        }
        Ok(())
    }

    fn insert(&mut self, id: Vec<u8>, pkg: KeyPackageData) -> Result<(), Self::Error> {
        let encoded = pkg
            .mls_encode_to_vec()
            .map_err(|e| StoreError::Store(format!("cannot encode key package data: {e:?}")))?;
        let key = hex(&id);

        let mut index = self.index()?;
        if !index.iter().any(|held| held == &key) {
            index.push(key.clone());
        }
        let evicted: Vec<String> = if index.len() > MAX_UNSPENT_KEY_PACKAGES {
            index
                .drain(..index.len() - MAX_UNSPENT_KEY_PACKAGES)
                .collect()
        } else {
            Vec::new()
        };

        // Two writes, ordered so the same thing survives either failure: an
        // index entry naming a package that is not there. That costs one slot
        // and is evicted in its turn. The opposite residue is private key
        // material the index has stopped naming, and **nothing reclaims that**:
        // the sweep in [`LeafDevice::unpair`](crate::LeafDevice::unpair) is
        // over epoch records, this key type has no sweep of its own, and an
        // eviction the index has already forgotten is never attempted again.
        //
        // So the evicted packages are erased before the index stops naming
        // them, and the index is written before the package it names. A delete
        // that fails takes the whole mint down with it, which is a key package
        // this device does not hand out rather than one it cannot account for.
        for stale in evicted {
            self.store.delete(KEY_TYPE_KEY_PACKAGE, &stale)?;
        }
        self.save_index(&index)?;

        self.store.store(KEY_TYPE_KEY_PACKAGE, &key, &encoded)
    }

    fn get(&self, id: &[u8]) -> Result<Option<KeyPackageData>, Self::Error> {
        let Some(raw) = self.store.load(KEY_TYPE_KEY_PACKAGE, &hex(id))? else {
            return Ok(None);
        };
        let decoded = KeyPackageData::mls_decode(&mut &raw[..])
            .map_err(|e| StoreError::Corrupt(format!("key package data does not decode: {e:?}")))?;
        Ok(Some(decoded))
    }
}

/// What a peer told us it can parse, and what we therefore may emit to it.
///
/// Persisted because these are end-to-end capabilities: they describe what the
/// recipient parses after any number of relay hops, so a device that forgot
/// them across a power cycle would silently downgrade every established peer
/// until the next key package exchange.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct PeerRecord {
    /// Envelope forms the peer parses. `[1]` means the compact envelope.
    #[serde(default)]
    pub(crate) env_versions: Vec<u8>,
    /// Frame encodings the peer decodes on the next hop.
    #[serde(default)]
    pub(crate) wire_versions: Vec<u8>,
    /// Whether this device has already given this peer a key package.
    ///
    /// Persisted rather than held in RAM because an init key is single use:
    /// a device that forgot across a power cycle would mint a second package
    /// on the next exchange and leave the peer holding two, of which only one
    /// is ever spent. Cleared when a peer resets the session, which is the
    /// one moment a fresh package is required rather than wasteful.
    #[serde(default)]
    pub(crate) key_package_sent: bool,

    /// Ids of the reset-flagged key package frames already acted on.
    ///
    /// A reset tears down a live session, so a frame carrying one is worth
    /// capturing and sending again. Remembering the last few ids means the
    /// same captured frame cannot tear down a session twice.
    ///
    /// This bounds a repeat, and does not close replay. Nothing in the signed
    /// payload states freshness, so an attacker holding a reset frame older
    /// than this ring can still spend it once. Closing that is a wire change
    /// rather than a device one, and is recorded on
    /// [`LeafDevice`](crate::LeafDevice).
    #[serde(default)]
    pub(crate) recent_reset_ids: Vec<String>,
}

/// How many reset frame ids a peer record remembers.
pub(crate) const RECENT_RESET_IDS: usize = 4;

impl PeerRecord {
    /// Records `id` as acted on, dropping the oldest beyond the ring.
    pub(crate) fn remember_reset(&mut self, id: &str) {
        if self.recent_reset_ids.iter().any(|seen| seen == id) {
            return;
        }
        self.recent_reset_ids.push(String::from(id));
        if self.recent_reset_ids.len() > RECENT_RESET_IDS {
            let excess = self.recent_reset_ids.len() - RECENT_RESET_IDS;
            self.recent_reset_ids.drain(..excess);
        }
    }

    /// Whether this reset frame has already been acted on.
    pub(crate) fn has_seen_reset(&self, id: &str) -> bool {
        self.recent_reset_ids.iter().any(|seen| seen == id)
    }

    pub(crate) fn load(store: &Arc<dyn LeafStore>, peer: &str) -> Result<Option<Self>, StoreError> {
        let Some(raw) = store.load(KEY_TYPE_PEER, peer)? else {
            return Ok(None);
        };
        serde_json::from_slice(&raw)
            .map(Some)
            .map_err(|e| StoreError::Corrupt(format!("peer record does not decode: {e}")))
    }

    pub(crate) fn save(&self, store: &Arc<dyn LeafStore>, peer: &str) -> Result<(), StoreError> {
        let encoded = serde_json::to_vec(self)
            .map_err(|e| StoreError::Store(format!("cannot encode peer record: {e}")))?;
        store.store(KEY_TYPE_PEER, peer, &encoded)
    }
}

/// Reads the address out of a basic credential.
///
/// The credential's whole content is the peer's address in its canonical text
/// form. Anything else is a credential this protocol did not mint.
pub(crate) fn credential_address(credential: &BasicCredential) -> Result<&str, crate::LeafError> {
    core::str::from_utf8(&credential.identifier).map_err(|_| {
        crate::LeafError::IdentityBinding(String::from(
            "credential identifier is not valid UTF-8, so it names no address",
        ))
    })
}
