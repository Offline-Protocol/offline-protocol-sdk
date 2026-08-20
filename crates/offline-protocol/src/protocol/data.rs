//! The replicated-document store: persistence, caps, and compaction.
//!
//! The engine lives in `offline-protocol-data` and knows nothing about
//! storage. This module is the other half: it decides when a document is
//! read, when its changes reach disk, and when history is folded away. It
//! owns no cryptography of its own — records go through the same sealing
//! chokepoint every other protocol-state category uses, which is what makes
//! a swapped-in storage backend unable to change the at-rest posture.
//!
//! # Record layout
//!
//! | Category | Key id | Contents |
//! |---|---|---|
//! | `data_docs` | `{space}/{doc}` | The compacted document |
//! | `data_delta_log` | `{space}/{doc}/{seq:016x}` | One persisted commit |
//! | `data_spaces` | `{space}` | The document index for the space |
//!
//! # Why the listing is the truth and the index is a cache
//!
//! Everything needed to re-open a document is derived from what is actually
//! on disk: `list_keys` names the delta records, and their zero-padded hex
//! sequence sorts into apply order. The space record only remembers document
//! names, so a document created and never written still lists. Any crash
//! therefore leaves a readable store rather than a bookkeeping claim that
//! has to be reconciled against reality.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use offline_protocol_data::{
    policy, CatchUp, DataDoc, DataError, DataValue, RemoteImport, VersionToken,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::{Error, Result};
use crate::events::Event;
use crate::protocol::storage::StateRecord;
use crate::protocol::types::{storage_keys, MAX_PROTOCOL_STATE_RECORD_BYTES};
use crate::protocol::OfflineProtocol;
use crate::protocol_state_storage::ProtocolStateStorage;

/// The document index for one space.
///
/// Deliberately thin. Sizes and sequence numbers are derived from the
/// records themselves at open, so this record can never disagree with the
/// store about what exists; it only remembers names, which nothing else
/// records for a document that has never been written.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SpaceRecord {
    #[serde(default)]
    docs: BTreeSet<String>,
}

/// A document held open in memory.
struct LoadedDoc {
    doc: DataDoc,
    /// Sequence number the next persisted commit will use.
    next_seq: u64,
    /// Total size of the delta records currently on disk for this document.
    log_bytes: usize,
    /// Size of the last compacted record written for this document.
    compacted_bytes: usize,
    /// Set when the document's compacted encoding passed the cap.
    ///
    /// Growth is refused while it is set; deletions are not, so an
    /// application that hit the cap can shrink its way back out. Cleared by
    /// the next flush that measures under the cap.
    over_cap: bool,
    /// Whether every delta record on disk actually applied at open.
    ///
    /// False when a delta was unreadable, or when one imported but parked
    /// behind a predecessor that never arrived. Both mean the document in
    /// memory is missing changes the records still describe, and both make
    /// compaction destructive: folding the log would write a snapshot without
    /// those changes and then delete the records holding them. A growing log
    /// is the cheaper failure, so compaction is simply switched off until an
    /// open sees a complete log again.
    history_complete: bool,
}

/// In-memory state for the data layer.
#[derive(Default)]
pub(crate) struct DataLayer {
    docs: BTreeMap<(String, String), LoadedDoc>,
    spaces: BTreeMap<String, SpaceRecord>,
}

impl std::fmt::Debug for DataLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataLayer")
            .field("open_docs", &self.docs.len())
            .field("loaded_spaces", &self.spaces.len())
            .finish()
    }
}

fn map_data_error(err: DataError) -> Error {
    match err {
        DataError::DocTooLarge { actual, limit } => Error::DocTooLarge { actual, limit },
        DataError::Corrupt(detail) | DataError::Engine(detail) => Error::DataCorrupted(detail),
        DataError::Poisoned => {
            Error::DataCorrupted("document handle poisoned by an earlier decode failure".into())
        }
        DataError::InvalidName { name, reason } => {
            Error::InvalidArgument(format!("invalid name {name:?}: {reason}"))
        }
        DataError::ValueTooLarge { actual, limit } => Error::InvalidArgument(format!(
            "value is {actual} bytes, over the {limit} byte limit"
        )),
        DataError::InvalidAttachment { reason } => {
            Error::InvalidArgument(format!("invalid attachment: {reason}"))
        }
        DataError::OutOfRange { position, length } => Error::InvalidArgument(format!(
            "position {position} is out of range for length {length}"
        )),
        // Forced by `#[non_exhaustive]` on `DataError`, which the data crate
        // carries so a new variant is not a semver break for published
        // consumers. That trade costs the compile error that would otherwise
        // catch an unmapped variant, so it is replaced with a `warn!`: an
        // unmapped variant shows up in logs instead of vanishing. When you add
        // a variant, add its arm here — this is a tripwire, not a destination.
        ref other => {
            warn!(
                error = %other,
                "unmapped DataError variant reached the protocol boundary; \
                 add an explicit arm in protocol/data.rs"
            );
            Error::Other(other.to_string())
        }
    }
}

/// Compose the record key for a document's compacted snapshot.
fn doc_key(space: &str, doc: &str) -> String {
    format!("{space}/{doc}")
}

/// Compose the record key for one persisted commit.
///
/// The sequence is zero-padded hex so that lexicographic order — which is
/// the only order `list_keys` promises anything about — is apply order.
fn delta_key(space: &str, doc: &str, seq: u64) -> String {
    format!("{space}/{doc}/{seq:016x}")
}

/// Parse the sequence out of a delta record key belonging to this document.
fn delta_seq(key: &str, prefix: &str) -> Option<u64> {
    let suffix = key.strip_prefix(prefix)?;
    u64::from_str_radix(suffix, 16).ok()
}

impl OfflineProtocol {
    /// The backend documents are stored in.
    ///
    /// The application's override when it set one, and otherwise the store
    /// protocol state already uses — which is what makes the layer work with
    /// no storage configuration at all.
    fn data_storage(&self) -> Option<Arc<dyn ProtocolStateStorage>> {
        self.config
            .data
            .storage
            .clone()
            .or_else(|| self.protocol_state_storage.clone())
    }

    /// The backend, or the typed error explaining which precondition failed.
    fn require_data_storage(&self) -> Result<Arc<dyn ProtocolStateStorage>> {
        if !self.config.data.enabled {
            return Err(Error::DataDisabled);
        }
        self.data_storage().ok_or(Error::DataStorageUnavailable)
    }

    fn validate_ids(space: &str, doc: &str) -> Result<()> {
        offline_protocol_data::validate_space_name(space).map_err(map_data_error)?;
        offline_protocol_data::validate_name(doc).map_err(map_data_error)?;
        Ok(())
    }

    /// Read a space's document index, loading it on first touch.
    fn load_space(&mut self, storage: &dyn ProtocolStateStorage, space: &str) -> Result<()> {
        if self.data.spaces.contains_key(space) {
            return Ok(());
        }
        let mut record = SpaceRecord::default();
        match self.read_state_record(storage, storage_keys::DATA_SPACES, space) {
            Ok(Some(bytes)) => match serde_json::from_slice::<SpaceRecord>(&bytes) {
                Ok(parsed) => record = parsed,
                Err(err) => {
                    // The index is a cache: a damaged one costs the names of
                    // documents that have no records yet, never a document.
                    warn!(space, error = %err, "Unreadable data space index; rebuilding from records");
                }
            },
            Ok(None) => {}
            Err(err) => {
                warn!(space, error = %err, "Failed to read data space index");
            }
        }

        // Reconcile against what is actually stored, so the index can never
        // hide a document that exists.
        for key_type in [storage_keys::DATA_DOCS, storage_keys::DATA_DELTA_LOG] {
            if let Ok(keys) = storage.list_keys(key_type) {
                for key in keys {
                    if let Some(rest) = key.strip_prefix(&format!("{space}/")) {
                        let name = rest.split('/').next().unwrap_or(rest);
                        if !name.is_empty() {
                            record.docs.insert(name.to_string());
                        }
                    }
                }
            }
        }

        self.data.spaces.insert(space.to_string(), record);
        Ok(())
    }

    fn persist_space(&mut self, storage: &dyn ProtocolStateStorage, space: &str) -> Result<()> {
        let Some(record) = self.data.spaces.get(space) else {
            return Ok(());
        };
        let bytes =
            serde_json::to_vec(record).map_err(|err| Error::Serialization(err.to_string()))?;
        self.write_state_record(storage, storage_keys::DATA_SPACES, space, &bytes)
            .map_err(|err| Error::Other(format!("failed to persist data space index: {err}")))
    }

    /// Open a document, loading its compacted record and delta log.
    ///
    /// Applying the compacted record and then every delta is safe in any
    /// order and safe to repeat: duplicates are absorbed by the merge, which
    /// is exactly why a crash at any point during compaction leaves a
    /// recoverable store.
    fn open_doc(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        space: &str,
        doc: &str,
    ) -> Result<()> {
        if self
            .data
            .docs
            .contains_key(&(space.to_string(), doc.to_string()))
        {
            return Ok(());
        }
        self.load_space(storage, space)?;

        let mut document = DataDoc::new();
        let mut compacted_bytes = 0usize;
        let mut history_complete = true;

        match self.read_state_record_detailed(
            storage,
            storage_keys::DATA_DOCS,
            &doc_key(space, doc),
        ) {
            Ok(StateRecord::Present(bytes)) => {
                compacted_bytes = bytes.len();
                // A snapshot is self-contained, so parking here should not be
                // reachable. Recorded rather than assumed away: if it ever is,
                // the document is missing its own base and compaction would
                // write that gap over the delta log.
                if document.import(&bytes).map_err(map_data_error)?.is_parked() {
                    history_complete = false;
                    warn!(space, doc, "Document snapshot did not fully apply");
                }
            }
            // `Unavailable` means it is still on disk and may be fine, so
            // refusing here keeps a transient failure from looking like data
            // loss to the application.
            Ok(StateRecord::Unavailable) => {
                return Err(Error::Other(format!(
                    "document {space}/{doc} could not be read this session"
                )));
            }
            // `Unreadable` means the record was examined and is permanently
            // gone; the delta log may still carry the document forward. The
            // document has lost its base either way, so compaction stays off:
            // folding now would write whatever partial state survived over the
            // records that still hold the rest.
            Ok(StateRecord::Unreadable) => {
                history_complete = false;
                warn!(space, doc, "Document snapshot is permanently unreadable");
            }
            Ok(StateRecord::Missing) => {}
            Err(err) => {
                return Err(Error::Other(format!("failed to read document: {err}")));
            }
        }

        let prefix = format!("{space}/{doc}/");
        let mut sequences: Vec<u64> = storage
            .list_keys(storage_keys::DATA_DELTA_LOG)
            .unwrap_or_default()
            .iter()
            .filter_map(|key| delta_seq(key, &prefix))
            .collect();
        sequences.sort_unstable();

        let mut log_bytes = 0usize;
        let mut next_seq = 0u64;
        let mut replayed = 0u32;
        for seq in sequences {
            next_seq = next_seq.max(seq.saturating_add(1));
            let key = delta_key(space, doc, seq);
            match self.read_state_record_detailed(storage, storage_keys::DATA_DELTA_LOG, &key) {
                Ok(StateRecord::Present(bytes)) => {
                    log_bytes = log_bytes.saturating_add(bytes.len());
                    replayed = replayed.saturating_add(1);
                    match document.import(&bytes) {
                        // The engine accepted the delta but cannot apply it:
                        // its predecessor is missing, so it and everything
                        // after it are invisible. Recording that is what stops
                        // compaction from writing a snapshot without these
                        // changes and then deleting the records holding them.
                        Ok(outcome) if outcome.is_parked() => {
                            history_complete = false;
                            warn!(
                                space,
                                doc,
                                seq,
                                "Document delta is waiting on a predecessor that never \
                                 arrived; compaction is disabled for this document"
                            );
                        }
                        Ok(_) => {}
                        Err(err) => {
                            // A permanently undecodable delta is a hole. The
                            // document still opens with everything that did
                            // apply, but the log must not be folded away: F3
                            // resend is what closes a hole, and compaction
                            // would destroy what is left to resend against.
                            history_complete = false;
                            warn!(space, doc, seq, error = %err, "Skipping undecodable document delta");
                        }
                    }
                }
                // Still on disk and possibly perfectly good next launch.
                // Refusing the open keeps a transient read failure from
                // looking like data loss, exactly as the snapshot path above
                // does, and keeps compaction away from records that
                // everything after this delta depends on.
                Ok(StateRecord::Unavailable) => {
                    return Err(Error::Other(format!(
                        "delta {seq} of document {space}/{doc} could not be read this session"
                    )));
                }
                // Listed but gone or permanently unreadable: a hole, not a
                // reason to refuse the document.
                Ok(_) => {
                    history_complete = false;
                    warn!(space, doc, seq, "Document delta is missing or unreadable");
                }
                Err(err) => {
                    return Err(Error::Other(format!(
                        "failed to read delta {seq} of document {space}/{doc}: {err}"
                    )));
                }
            }
        }

        document.mark_persisted();
        if compacted_bytes == 0 {
            compacted_bytes = document.measure().map_err(map_data_error)?;
        }
        // Hand the document what its own bookkeeping cannot know after a
        // fresh open: how big the record it came from was, how much delta
        // log sits on top, and how many commits that log represents. Skip
        // this and a reopened document looks empty to the size check and to
        // the compaction trigger, both of which then under-fire.
        document.restore_bookkeeping(compacted_bytes, log_bytes, replayed);

        if let Some(record) = self.data.spaces.get_mut(space) {
            record.docs.insert(doc.to_string());
        }
        self.data.docs.insert(
            (space.to_string(), doc.to_string()),
            LoadedDoc {
                doc: document,
                next_seq,
                log_bytes,
                compacted_bytes,
                over_cap: false,
                history_complete,
            },
        );
        Ok(())
    }

    /// Run an edit against an open document.
    ///
    /// `growth` marks operations that can make the document bigger. Those are
    /// refused once the cap has been passed; deletions are not, so an
    /// application that filled a document can still empty it.
    fn with_doc<F>(&mut self, space: &str, doc: &str, growth: bool, edit: F) -> Result<()>
    where
        F: FnOnce(&mut DataDoc) -> std::result::Result<(), DataError>,
    {
        Self::validate_ids(space, doc)?;
        let storage = self.require_data_storage()?;
        self.open_doc(storage.as_ref(), space, doc)?;

        let entry = self
            .data
            .docs
            .get_mut(&(space.to_string(), doc.to_string()))
            .ok_or_else(|| Error::Other("document vanished after open".to_string()))?;

        if growth && entry.over_cap {
            return Err(Error::DocTooLarge {
                actual: entry.compacted_bytes,
                limit: policy::MAX_DOC_BYTES,
            });
        }
        edit(&mut entry.doc).map_err(map_data_error)
    }

    /// Read from an open document.
    fn read_doc<F, T>(&mut self, space: &str, doc: &str, read: F) -> Result<T>
    where
        F: FnOnce(&DataDoc) -> std::result::Result<T, DataError>,
    {
        Self::validate_ids(space, doc)?;
        let storage = self.require_data_storage()?;
        self.open_doc(storage.as_ref(), space, doc)?;
        let entry = self
            .data
            .docs
            .get(&(space.to_string(), doc.to_string()))
            .ok_or_else(|| Error::Other("document vanished after open".to_string()))?;
        read(&entry.doc).map_err(map_data_error)
    }

    // ---- public surface ---------------------------------------------

    /// Point documents at a backend the application supplies.
    ///
    /// Documents move; protocol secrets do not. Applied immediately, so a
    /// store opened afterwards reads and writes through the new backend while
    /// everything else stays where it is.
    ///
    /// Any document already open is written into the new backend as a
    /// self-contained snapshot before this returns. That is not a
    /// convenience, it is the whole correctness of a mid-session swap: a
    /// delta record only describes the change *since* the previous one, so a
    /// document that merely kept writing deltas into the new backend would
    /// leave every earlier delta behind in the old one. The engine accepts
    /// such an orphan delta and parks it, which means the document reads
    /// **empty** from the new backend and the next compaction deletes the
    /// orphans for good.
    ///
    /// If the migration cannot be written the swap does not happen at all and
    /// the error is returned, because a half-swapped layer is precisely the
    /// stranding this method exists to prevent.
    ///
    /// Documents that are not open are not migrated: they live wholly in the
    /// old backend, which is what switching backends means.
    pub fn set_data_storage(&mut self, storage: Arc<dyn ProtocolStateStorage>) -> Result<()> {
        let previous = self.config.data.storage.take();
        self.config.data.storage = Some(storage);

        match self.migrate_open_docs_to_data_storage() {
            Ok(()) => Ok(()),
            Err(err) => {
                self.config.data.storage = previous;
                Err(err)
            }
        }
    }

    /// Write every open document into the current backend as a self-contained
    /// snapshot, so nothing depends on history that lives somewhere else.
    ///
    /// Two phases, and the split is the correctness. Every write happens
    /// first; only once all of them are durable does any document's
    /// bookkeeping move. Interleaving the two would leave a document that
    /// migrated before the failure claiming a fresh empty log, while the
    /// caller rolls the swap back to the old backend: its next flush would
    /// then write sequence zero over the delta already there and park
    /// everything that followed it, which is permanent loss of exactly the
    /// kind this migration exists to prevent.
    fn migrate_open_docs_to_data_storage(&mut self) -> Result<()> {
        let Some(storage) = self.data_storage() else {
            return Ok(());
        };

        let keys: Vec<(String, String)> = self.data.docs.keys().cloned().collect();
        let mut committed: Vec<(String, String)> = Vec::new();
        let mut migrated: Vec<((String, String), usize)> = Vec::new();

        if let Err(err) =
            self.write_open_docs_into(storage.as_ref(), &keys, &mut committed, &mut migrated)
        {
            // Nothing durable is being claimed, so every export marker this
            // advanced goes back. Leaving one advanced would drop the edits
            // it covers from every future delta, which is the same silent
            // loss a failed delta write already rewinds for.
            for map_key in committed {
                if let Some(entry) = self.data.docs.get_mut(&map_key) {
                    entry.doc.rewind_last_commit();
                }
            }
            return Err(err);
        }

        // Phase two. Every record above is durable, so the swap is going to
        // stand and the bookkeeping can follow it.
        for (map_key, compacted_bytes) in migrated {
            if let Some(entry) = self.data.docs.get_mut(&map_key) {
                // The new backend holds one snapshot and no delta log.
                entry.next_seq = 0;
                entry.log_bytes = 0;
                entry.compacted_bytes = compacted_bytes;
                entry.doc.mark_compacted(compacted_bytes);
            }
        }
        Ok(())
    }

    /// Phase one of a backend swap: every open document and space index
    /// written into the new backend, with nothing in memory moved yet.
    ///
    /// `committed` collects the documents whose export marker this advanced,
    /// so a failure can put every one of them back. `migrated` collects the
    /// bookkeeping to apply once every write has succeeded.
    fn write_open_docs_into(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        keys: &[(String, String)],
        committed: &mut Vec<(String, String)>,
        migrated: &mut Vec<((String, String), usize)>,
    ) -> Result<()> {
        for (space, doc) in keys {
            let map_key = (space.clone(), doc.clone());

            // Fold pending edits in first: the snapshot below has to be the
            // whole document, including what has not been flushed yet.
            {
                let Some(entry) = self.data.docs.get_mut(&map_key) else {
                    continue;
                };
                if entry.doc.commit().map_err(map_data_error)?.is_some() {
                    committed.push(map_key.clone());
                }
            }
            let compacted = {
                let Some(entry) = self.data.docs.get(&map_key) else {
                    continue;
                };
                entry.doc.export_compacted().map_err(map_data_error)?
            };

            self.write_state_record(
                storage,
                storage_keys::DATA_DOCS,
                &doc_key(space, doc),
                &compacted,
            )
            .map_err(|err| {
                Error::Other(format!(
                    "failed to move document {space}/{doc} into the new backend: {err}"
                ))
            })?;

            // Snapshot first, then drop the log, in that order and for the
            // same reason as compaction. A delta record already sitting under
            // this name in the new backend belongs to some earlier document
            // that held it: the snapshot just written does not descend from
            // it, so at the next open it parks and switches compaction off
            // for good. Best-effort, again as compaction: the snapshot is
            // already durable, and a leftover costs a growing log rather than
            // a wrong document.
            let prefix = format!("{space}/{doc}/");
            if let Ok(stale) = storage.list_keys(storage_keys::DATA_DELTA_LOG) {
                for key in stale.iter().filter(|key| key.starts_with(&prefix)) {
                    let _ = storage.delete(storage_keys::DATA_DELTA_LOG, key);
                }
            }

            migrated.push((map_key, compacted.len()));
        }

        // The space indexes carry document names that have no records of
        // their own yet; without them the new backend forgets those names.
        let spaces: Vec<String> = self.data.spaces.keys().cloned().collect();
        for space in spaces {
            self.persist_space(storage, &space)?;
        }
        Ok(())
    }

    /// Create a document, or do nothing if it already exists.
    pub fn data_create_doc(&mut self, space: &str, doc: &str) -> Result<()> {
        Self::validate_ids(space, doc)?;
        let storage = self.require_data_storage()?;
        self.open_doc(storage.as_ref(), space, doc)?;
        self.persist_space(storage.as_ref(), space)
    }

    // ---- replication seams ------------------------------------------
    //
    // The sync module owns the frames and the peer bookkeeping; these are
    // the four things it needs from the store, and they are here rather
    // than there so nothing outside this module touches a document handle.

    /// The backend documents live in, when the layer is on.
    ///
    /// Unlike `require_data_storage` this answers `None` rather than an
    /// error: the sync path treats a switched-off layer as "nothing to do",
    /// not as a failure to report.
    pub(crate) fn data_storage_for_sync(&self) -> Option<Arc<dyn ProtocolStateStorage>> {
        if !self.config.data.enabled {
            return None;
        }
        self.data_storage()
    }

    /// Every document in a space with the version we hold of it.
    ///
    /// A document whose version cannot be read is left out rather than
    /// failing the whole space. Propagating the error costs every other
    /// document in the space its replication, and does it silently: one
    /// unreadable record on either leg would leave the offer empty and both
    /// replicas believing they had nothing to say to each other. Leaving it
    /// out instead means the peer reads that document as one we have never
    /// seen and offers it back, which is the recovery this layer already
    /// has, and the rest of the space keeps converging meanwhile.
    pub(crate) fn data_sync_versions(&mut self, space: &str) -> Result<BTreeMap<String, String>> {
        let docs = self.data_list_docs(space)?;
        let mut versions = BTreeMap::new();
        for doc in docs {
            match self.data_doc_version(space, &doc) {
                Ok(encoded) => {
                    versions.insert(doc, encoded);
                }
                Err(err) => {
                    warn!(space, doc, error = %err, "Leaving a document out of the version offer: its version cannot be read");
                }
            }
        }
        Ok(versions)
    }

    /// The version we hold of one document, encoded as a frame carries it.
    pub(crate) fn data_doc_version(&mut self, space: &str, doc: &str) -> Result<String> {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

        let token = self.read_doc(space, doc, |document| Ok(document.version()))?;
        Ok(BASE64.encode(token.as_bytes()))
    }

    /// The names a space holds, or `None` if there is nothing to read.
    ///
    /// Answered off the space index, which `load_space` has already
    /// reconciled against the records themselves and cached. Opening a
    /// document to find out would defeat both callers, whose whole question
    /// is whether opening one is allowed.
    fn space_docs(&mut self, space: &str) -> Option<&BTreeSet<String>> {
        if offline_protocol_data::validate_space_name(space).is_err() {
            return None;
        }
        let storage = self.data_storage_for_sync()?;
        self.load_space(storage.as_ref(), space).ok()?;
        self.data.spaces.get(space).map(|record| &record.docs)
    }

    /// Whether `space` already holds `doc`.
    pub(crate) fn data_holds_doc(&mut self, space: &str, doc: &str) -> bool {
        self.space_docs(space)
            .is_some_and(|docs| docs.contains(doc))
    }

    /// Whether a document a peer named may be stored in `space`.
    ///
    /// One already held always may; a new one only while the space is under
    /// `cap`. The cap is a parameter rather than a constant here because it
    /// bounds what a *peer* can talk this device into storing, which is a
    /// replication policy: an application creating its own documents is not
    /// subject to it.
    ///
    /// A space that cannot be read admits, rather than refusing on a storage
    /// hiccup: whatever follows fails on the same storage and reports it
    /// where the failure actually is.
    pub(crate) fn data_space_admits_doc(&mut self, space: &str, doc: &str, cap: usize) -> bool {
        self.space_docs(space)
            .is_none_or(|docs| docs.contains(doc) || docs.len() < cap)
    }

    /// What a replica at `theirs` is missing from a document.
    pub(crate) fn data_catch_up(
        &mut self,
        space: &str,
        doc: &str,
        theirs: &VersionToken,
    ) -> Result<CatchUp> {
        self.read_doc(space, doc, |document| document.export_since(theirs))
    }

    /// A self-contained encoding of a document, for a peer no run of
    /// changes can catch up.
    pub(crate) fn data_export_snapshot(&mut self, space: &str, doc: &str) -> Result<Vec<u8>> {
        self.read_doc(space, doc, |document| document.export_raw())
    }

    /// Apply a blob that arrived from a peer, and persist what it changed.
    ///
    /// The import goes through the engine's remote path, which judges the
    /// blob before touching it. A change that applies is flushed here rather
    /// than left in memory: an unflushed remote change would be lost to a
    /// restart, and the sender has already been told (by the acknowledgement
    /// the frame rides on) that it arrived.
    ///
    /// `Err` means the change is not durable. A document held over its cap
    /// still answers `Applied`, because the cap refuses growth *after* the
    /// change is on disk; the caller cannot tell the two apart from an error
    /// alone, and one of them is a change it must not report as refused.
    pub(crate) fn data_apply_remote(
        &mut self,
        space: &str,
        doc: &str,
        blob: &[u8],
    ) -> Result<RemoteImport> {
        Self::validate_ids(space, doc)?;
        let storage = self.require_data_storage()?;
        self.open_doc(storage.as_ref(), space, doc)?;

        let map_key = (space.to_string(), doc.to_string());
        let outcome = {
            let entry = self
                .data
                .docs
                .get_mut(&map_key)
                .ok_or_else(|| Error::Other("document vanished after open".to_string()))?;
            let outcome = entry.doc.import_remote(blob).map_err(map_data_error)?;
            // A parked change is invisible to the engine's own version, so
            // nothing downstream may fold history while one is outstanding:
            // compaction would write a snapshot without it and then delete
            // the records it is waiting for.
            //
            // It stays false for the rest of this document's time in memory,
            // deliberately. The engine reports what one import left pending,
            // never whether a document still holds anything parked, so a
            // later import that applies cleanly is not evidence that the
            // earlier gap closed. Clearing it on that evidence would re-arm
            // compaction while a change is still waiting, which is the loss
            // this flag exists to prevent; leaving it set costs a document
            // its compaction until it is next opened, where the flag is
            // recomputed from the records themselves.
            if outcome == RemoteImport::Parked {
                entry.history_complete = false;
            }
            outcome
        };

        if outcome == RemoteImport::Applied {
            // Remote work is not this replica's to push onward, so the flush
            // is told where the change came from.
            //
            // `DocTooLarge` is not an import failure and is not reported as
            // one. It is raised after the delta record was written and the
            // push decided, so the imported change is already durable; what
            // failed is the size verdict that follows, and what it refuses is
            // further *growth*. Propagating it would say the change was
            // refused when it applied, skip the space record below, and cost
            // the caller the one signal that a local edit may have been
            // folded into this import and suppressed with it, because that
            // compensation is gated on hearing `Applied`. A document over its
            // cap is exactly where that matters: the pending edit it still
            // accepts is a deletion, which is the route back under.
            match self.flush_doc_from(storage.as_ref(), space, doc, Some(space)) {
                Ok(()) => {}
                Err(Error::DocTooLarge { .. }) => {
                    debug!(
                        space,
                        doc, "Applied a remote change into a document that is over its cap"
                    );
                }
                Err(err) => return Err(err),
            }
            self.persist_space(storage.as_ref(), space)?;
        }
        Ok(outcome)
    }

    /// Delete a document and every record belonging to it.
    pub fn data_delete_doc(&mut self, space: &str, doc: &str) -> Result<()> {
        Self::validate_ids(space, doc)?;
        let storage = self.require_data_storage()?;
        self.load_space(storage.as_ref(), space)?;

        self.data.docs.remove(&(space.to_string(), doc.to_string()));

        // Every record is attempted even after one fails, so a partial
        // failure still removes as much as it can; the first error is what
        // the caller hears. Reporting matters because a record that survives
        // a delete is replayed into the *next* document of the same name,
        // which resurrects the deleted incarnation's contents.
        let mut first_error: Option<String> = None;
        let mut record_error = |err: crate::protocol_state_storage::ProtocolStateError| {
            if first_error.is_none() {
                first_error = Some(err.to_string());
            }
        };

        if let Err(err) = storage.delete(storage_keys::DATA_DOCS, &doc_key(space, doc)) {
            record_error(err);
        }
        let prefix = format!("{space}/{doc}/");
        match storage.list_keys(storage_keys::DATA_DELTA_LOG) {
            Ok(keys) => {
                for key in keys.iter().filter(|key| key.starts_with(&prefix)) {
                    if let Err(err) = storage.delete(storage_keys::DATA_DELTA_LOG, key) {
                        record_error(err);
                    }
                }
            }
            Err(err) => record_error(err),
        }
        if let Some(record) = self.data.spaces.get_mut(space) {
            record.docs.remove(doc);
        }
        self.persist_space(storage.as_ref(), space)?;

        match first_error {
            Some(detail) => Err(Error::Other(format!(
                "failed to delete every record of document {space}/{doc}: {detail}"
            ))),
            None => Ok(()),
        }
    }

    /// The documents in a space.
    pub fn data_list_docs(&mut self, space: &str) -> Result<Vec<String>> {
        offline_protocol_data::validate_space_name(space).map_err(map_data_error)?;
        let storage = self.require_data_storage()?;
        self.load_space(storage.as_ref(), space)?;
        Ok(self
            .data
            .spaces
            .get(space)
            .map(|record| record.docs.iter().cloned().collect())
            .unwrap_or_default())
    }

    /// Every space the store knows of.
    ///
    /// Derived from what is on disk, so a space whose last document was
    /// deleted still lists while its index record survives.
    pub fn data_list_spaces(&mut self) -> Result<Vec<String>> {
        let storage = self.require_data_storage()?;
        let mut spaces = BTreeSet::new();
        for key_type in [
            storage_keys::DATA_SPACES,
            storage_keys::DATA_DOCS,
            storage_keys::DATA_DELTA_LOG,
        ] {
            if let Ok(keys) = storage.list_keys(key_type) {
                for key in keys {
                    let name = key.split('/').next().unwrap_or(&key);
                    if !name.is_empty() {
                        spaces.insert(name.to_string());
                    }
                }
            }
        }
        for space in self.data.spaces.keys() {
            spaces.insert(space.clone());
        }
        Ok(spaces.into_iter().collect())
    }

    /// Set a key in a map collection.
    pub fn data_map_set(
        &mut self,
        space: &str,
        doc: &str,
        collection: &str,
        key: &str,
        value: DataValue,
    ) -> Result<()> {
        self.with_doc(space, doc, true, |document| {
            document.map_set(collection, key, value)
        })
    }

    /// Remove a key from a map collection.
    pub fn data_map_delete(
        &mut self,
        space: &str,
        doc: &str,
        collection: &str,
        key: &str,
    ) -> Result<()> {
        self.with_doc(space, doc, false, |document| {
            document.map_delete(collection, key)
        })
    }

    /// Read a key from a map collection.
    pub fn data_map_get(
        &mut self,
        space: &str,
        doc: &str,
        collection: &str,
        key: &str,
    ) -> Result<Option<DataValue>> {
        self.read_doc(space, doc, |document| document.map_get(collection, key))
    }

    /// Append to a list collection.
    pub fn data_list_push(
        &mut self,
        space: &str,
        doc: &str,
        collection: &str,
        value: DataValue,
    ) -> Result<()> {
        self.with_doc(space, doc, true, |document| {
            document.list_push(collection, value)
        })
    }

    /// Insert into a list collection.
    pub fn data_list_insert(
        &mut self,
        space: &str,
        doc: &str,
        collection: &str,
        index: u32,
        value: DataValue,
    ) -> Result<()> {
        self.with_doc(space, doc, true, |document| {
            document.list_insert(collection, index as usize, value)
        })
    }

    /// Delete entries from a list collection.
    pub fn data_list_delete(
        &mut self,
        space: &str,
        doc: &str,
        collection: &str,
        index: u32,
        count: u32,
    ) -> Result<()> {
        self.with_doc(space, doc, false, |document| {
            document.list_delete(collection, index as usize, count as usize)
        })
    }

    /// The number of entries in a list collection.
    pub fn data_list_len(&mut self, space: &str, doc: &str, collection: &str) -> Result<u32> {
        let len = self.read_doc(space, doc, |document| document.list_len(collection))?;
        Ok(len as u32)
    }

    /// Insert into a text collection at a character position.
    pub fn data_text_insert(
        &mut self,
        space: &str,
        doc: &str,
        collection: &str,
        position: u32,
        text: &str,
    ) -> Result<()> {
        self.with_doc(space, doc, true, |document| {
            document.text_insert(collection, position as usize, text)
        })
    }

    /// Delete characters from a text collection.
    pub fn data_text_delete(
        &mut self,
        space: &str,
        doc: &str,
        collection: &str,
        position: u32,
        count: u32,
    ) -> Result<()> {
        self.with_doc(space, doc, false, |document| {
            document.text_delete(collection, position as usize, count as usize)
        })
    }

    /// The contents of a text collection.
    pub fn data_text_value(&mut self, space: &str, doc: &str, collection: &str) -> Result<String> {
        self.read_doc(space, doc, |document| document.text_value(collection))
    }

    /// Add to a counter collection. Negative values subtract.
    pub fn data_counter_increment(
        &mut self,
        space: &str,
        doc: &str,
        collection: &str,
        amount: f64,
    ) -> Result<()> {
        self.with_doc(space, doc, true, |document| {
            document.counter_increment(collection, amount)
        })
    }

    /// The value of a counter collection.
    pub fn data_counter_value(&mut self, space: &str, doc: &str, collection: &str) -> Result<f64> {
        self.read_doc(space, doc, |document| document.counter_value(collection))
    }

    /// The document's current state as plain JSON.
    pub fn data_doc_json(&mut self, space: &str, doc: &str) -> Result<String> {
        self.read_doc(space, doc, |document| document.export_json())
    }

    /// The document's full history, engine-encoded.
    ///
    /// Half of the escape hatch: this and [`Self::data_doc_json`] are what
    /// let an application take its data and leave.
    pub fn data_export_raw(&mut self, space: &str, doc: &str) -> Result<Vec<u8>> {
        self.read_doc(space, doc, |document| document.export_raw())
    }

    /// Persist any pending edits to a document.
    ///
    /// Writes one delta record, then compacts if the log has outgrown the
    /// document. The order is load-bearing: the delta is durable before any
    /// bookkeeping assumes it, and at compaction the fresh snapshot is
    /// durable before the deltas it folded are deleted. A crash at any point
    /// leaves duplicate history, which the merge absorbs, rather than a gap,
    /// which it cannot.
    pub fn data_flush(&mut self, space: &str, doc: &str) -> Result<()> {
        Self::validate_ids(space, doc)?;
        let storage = self.require_data_storage()?;
        self.open_doc(storage.as_ref(), space, doc)?;
        self.flush_doc(storage.as_ref(), space, doc)
    }

    fn flush_doc(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        space: &str,
        doc: &str,
    ) -> Result<()> {
        self.flush_doc_from(storage, space, doc, None)
    }

    /// [`Self::flush_doc`], told which peer the change came from.
    ///
    /// `origin` is `None` for a local edit and the peer's address for one
    /// applied from a sync frame. It exists so replication does not send a
    /// change straight back to whoever sent it: the merge would absorb the
    /// echo, but on a mesh it is a frame nobody needed.
    fn flush_doc_from(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        space: &str,
        doc: &str,
        origin: Option<&str>,
    ) -> Result<()> {
        let map_key = (space.to_string(), doc.to_string());

        let Some(entry) = self.data.docs.get_mut(&map_key) else {
            return Ok(());
        };
        let Some(delta) = entry.doc.commit().map_err(map_data_error)? else {
            return Ok(());
        };
        let seq = entry.next_seq;
        let delta_bytes = delta.bytes.len();
        let history_complete = entry.history_complete;

        // A commit too large for one record can never be stored as a delta,
        // and refusing it is not a recovery: the rewind below puts the change
        // back in the pending set, so the *next* commit exports a strictly
        // larger delta and fails the same way forever. A compacted snapshot
        // is bounded by the document cap rather than by history, so it is the
        // route that still works, and the commit's changes are inside it.
        let oversized = delta_bytes > MAX_PROTOCOL_STATE_RECORD_BYTES;

        let rewind = |protocol: &mut Self| {
            if let Some(entry) = protocol.data.docs.get_mut(&map_key) {
                entry.doc.rewind_last_commit();
            }
        };

        if oversized && !history_complete {
            // Compaction is the only way out and it is unavailable: folding a
            // log with a hole in it would delete records the document still
            // needs. Say so rather than looping on an unwritable delta.
            rewind(self);
            return Err(Error::Other(format!(
                "commit for document {space}/{doc} is {delta_bytes} bytes, over the \
                 {MAX_PROTOCOL_STATE_RECORD_BYTES} byte record limit, and its delta log \
                 is incomplete so it cannot be compacted instead"
            )));
        }

        if oversized {
            warn!(
                space,
                doc, delta_bytes, "Commit is too large for one delta record; compacting instead"
            );
            if let Err(err) = self.compact_doc(storage, space, doc) {
                rewind(self);
                return Err(err);
            }
        } else if let Err(err) = self.write_state_record(
            storage,
            storage_keys::DATA_DELTA_LOG,
            &delta_key(space, doc, seq),
            &delta.bytes,
        ) {
            // Put the export marker back. The commit already advanced it, so
            // leaving it advanced would mean this change is never handed out
            // again: the document keeps it in memory, the next commit exports
            // only what came after, and the change is simply gone at the next
            // restart. Re-exporting it later costs at worst a duplicate
            // record, which the merge absorbs.
            rewind(self);
            return Err(Error::Other(format!(
                "failed to persist document delta: {err}"
            )));
        }

        let (verdict, measured, should_compact, compacted_before) = {
            let entry = self
                .data
                .docs
                .get_mut(&map_key)
                .ok_or_else(|| Error::Other("document vanished during flush".to_string()))?;
            if !oversized {
                entry.next_seq = seq.saturating_add(1);
                entry.log_bytes = entry.log_bytes.saturating_add(delta_bytes);
            }

            let (verdict, measured) = entry.doc.check_size().map_err(map_data_error)?;
            if let Some(bytes) = measured {
                entry.compacted_bytes = bytes;
            }
            // Never fold a log that did not fully apply, and never compact
            // twice for one flush.
            let should_compact = !oversized
                && entry.history_complete
                && policy::should_compact(
                    entry.log_bytes,
                    entry.compacted_bytes,
                    entry.doc.commits_since_compaction(),
                );
            (verdict, measured, should_compact, entry.compacted_bytes)
        };

        // Replicated here for the same reason the event fires here: the
        // claim being made is that the change is durable, and that became
        // true at the write above. Pushing before it would advertise a
        // change that a crash could still take back, and pushing after
        // compaction would let a compaction failure suppress replication of
        // something already on disk.
        //
        // The oversized branch wrote a compacted snapshot instead of a delta
        // record, so there is no delta to push. Offering versions is how the
        // peer finds the gap: waiting for its next offer would leave both
        // replicas believing they agree for as long as the link holds, since
        // a link that never drops never fires a trigger.
        if oversized {
            self.nudge_data_sync(space, origin, "oversized_commit");
        } else {
            self.push_data_delta(space, doc, &delta.bytes, origin);
        }

        // Emitted here, before compaction: the event's claim is that the
        // change reached storage, and that became true at the write above.
        // Emitting after compaction would let a compaction failure suppress
        // the notification for a change that is already durable.
        self.emit_event(Event::DataChanged {
            space_id: space.to_string(),
            doc_id: doc.to_string(),
            delta_bytes: delta_bytes as u64,
        });

        if should_compact {
            self.compact_doc(storage, space, doc)?;
        }

        match verdict {
            policy::SizeVerdict::TooLarge => {
                let actual = measured.unwrap_or(compacted_before);
                if let Some(entry) = self.data.docs.get_mut(&map_key) {
                    entry.over_cap = true;
                }
                // The change is already durable: refusing to persist it would
                // lose work the application believed it had made. What is
                // refused is further growth, and deletions stay open so the
                // document can be brought back under the cap.
                Err(Error::DocTooLarge {
                    actual,
                    limit: policy::MAX_DOC_BYTES,
                })
            }
            policy::SizeVerdict::Warn => {
                if let Some(entry) = self.data.docs.get_mut(&map_key) {
                    entry.over_cap = false;
                }
                self.emit_event(Event::DataDocSizeWarning {
                    space_id: space.to_string(),
                    doc_id: doc.to_string(),
                    compacted_bytes: measured.unwrap_or(compacted_before) as u64,
                    cap_bytes: policy::MAX_DOC_BYTES as u64,
                });
                Ok(())
            }
            policy::SizeVerdict::Ok => {
                if let Some(entry) = self.data.docs.get_mut(&map_key) {
                    entry.over_cap = false;
                }
                Ok(())
            }
        }
    }

    /// Fold a document's delta log into a fresh compacted record.
    fn compact_doc(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        space: &str,
        doc: &str,
    ) -> Result<()> {
        let map_key = (space.to_string(), doc.to_string());
        let compacted = {
            let Some(entry) = self.data.docs.get(&map_key) else {
                return Ok(());
            };
            // Folding a log that did not fully apply would write a snapshot
            // missing those changes and then delete the records that still
            // hold them, turning a recoverable gap into permanent loss. An
            // oversized delta log is the cheaper failure.
            if !entry.history_complete {
                warn!(
                    space,
                    doc, "Skipping compaction: the document's delta log did not fully apply"
                );
                return Ok(());
            }
            entry.doc.export_compacted().map_err(map_data_error)?
        };

        // Durable first. Everything after this point is cleanup, and a crash
        // in the middle of cleanup costs disk, not data.
        self.write_state_record(
            storage,
            storage_keys::DATA_DOCS,
            &doc_key(space, doc),
            &compacted,
        )
        .map_err(|err| Error::Other(format!("failed to persist compacted document: {err}")))?;

        let prefix = format!("{space}/{doc}/");
        if let Ok(keys) = storage.list_keys(storage_keys::DATA_DELTA_LOG) {
            for key in keys.iter().filter(|key| key.starts_with(&prefix)) {
                let _ = storage.delete(storage_keys::DATA_DELTA_LOG, key);
            }
        }

        if let Some(entry) = self.data.docs.get_mut(&map_key) {
            entry.log_bytes = 0;
            entry.compacted_bytes = compacted.len();
            entry.doc.mark_compacted(compacted.len());
        }
        debug!(space, doc, bytes = compacted.len(), "Compacted document");
        Ok(())
    }

    /// Persist every open document with pending edits.
    ///
    /// Called on shutdown, so the debounce window between an edit and its
    /// record can never be the reason a change is lost.
    pub fn data_flush_all(&mut self) -> Result<()> {
        let Ok(storage) = self.require_data_storage() else {
            return Ok(());
        };
        let keys: Vec<(String, String)> = self.data.docs.keys().cloned().collect();
        for (space, doc) in keys {
            match self.flush_doc(storage.as_ref(), &space, &doc) {
                Ok(()) => {}
                // Not a failed flush, and not reported as one: the record was
                // written and only the size verdict after it refused further
                // growth. On a shutdown path the distinction is the whole
                // question an operator is asking, which is whether anything
                // was lost.
                Err(Error::DocTooLarge { .. }) => {
                    debug!(
                        space,
                        doc, "Flushed a document that is over its cap on shutdown"
                    );
                }
                Err(err) => {
                    warn!(space, doc, error = %err, "Failed to flush document");
                }
            }
        }
        Ok(())
    }

    /// Delete every record the data layer owns.
    ///
    /// The logout path for an application that pointed documents at its own
    /// backend. `wipePersistedState` clears the account directory of the
    /// *default* provider, which a custom backend is not inside, so without
    /// this call those documents would outlive the account that made them.
    ///
    /// **Only durable once replication has stopped.** There are no deletion
    /// tombstones in this release, so nothing distinguishes a space this
    /// device wiped from one it has never seen. Called while the engine is
    /// running with live sessions, every document comes back: the peer's next
    /// version offer names them and they are recreated and refilled from the
    /// peer's copy, and an offer of our own naming nothing reads as a replica
    /// that has never seen the space. On the logout path the engine is being
    /// torn down anyway. Anywhere else, stop it first, and only for as long
    /// as it stays stopped: the peer still holds the documents, so they return
    /// when replication resumes with it. The call clears this device, it does
    /// not delete content.
    pub fn data_wipe_all(&mut self) -> Result<()> {
        let Some(storage) = self.data_storage() else {
            return Ok(());
        };
        self.data.docs.clear();
        self.data.spaces.clear();
        // The offer windows are keyed by peer, by member and group, and by peer and blob hash and outlives nothing else here.
        // Left behind it would suppress the first offer made after the wipe,
        // which is the same shape [`Self::forget_data_sync_peer`] exists to
        // prevent: a window that survives the thing it was measuring.
        self.last_data_sync_offer.clear();

        // Attempt every record and report the first failure, as
        // [`Self::data_delete_doc`] does. Answering `Ok` for a wipe that left
        // records behind is the worst shape this call can take: it is the
        // logout path, so the records it failed to remove outlive the account
        // that made them, and the application has no symptom to notice it by.
        let mut first_error: Option<String> = None;
        let mut record_error = |err: crate::protocol_state_storage::ProtocolStateError| {
            if first_error.is_none() {
                first_error = Some(err.to_string());
            }
        };

        // Every key type the layer owns, replication bookkeeping included.
        // A key type missing from this list is not a partial wipe that
        // reports itself, it is a silent one: the records are keyed by peer
        // address, so what survives a logout is who this account replicated
        // with.
        for key_type in [
            storage_keys::DATA_DOCS,
            storage_keys::DATA_DELTA_LOG,
            storage_keys::DATA_SPACES,
            storage_keys::DATA_SYNC,
        ] {
            match storage.list_keys(key_type) {
                Ok(keys) => {
                    for key in keys {
                        if let Err(err) = storage.delete(key_type, &key) {
                            record_error(err);
                        }
                    }
                }
                Err(err) => record_error(err),
            }
        }

        match first_error {
            Some(detail) => Err(Error::Other(format!(
                "failed to delete every data record: {detail}"
            ))),
            None => Ok(()),
        }
    }

    /// The compacted size of a document, in bytes.
    pub fn data_doc_size(&mut self, space: &str, doc: &str) -> Result<u64> {
        Self::validate_ids(space, doc)?;
        let storage = self.require_data_storage()?;
        self.open_doc(storage.as_ref(), space, doc)?;
        let entry = self
            .data
            .docs
            .get_mut(&(space.to_string(), doc.to_string()))
            .ok_or_else(|| Error::Other("document vanished after open".to_string()))?;
        let bytes = entry.doc.measure().map_err(map_data_error)?;
        entry.compacted_bytes = bytes;
        Ok(bytes as u64)
    }
}
