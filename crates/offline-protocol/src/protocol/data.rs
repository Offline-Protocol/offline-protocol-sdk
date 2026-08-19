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

use offline_protocol_data::{policy, DataDoc, DataError, DataValue};
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
        offline_protocol_data::validate_name(space).map_err(map_data_error)?;
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
    fn migrate_open_docs_to_data_storage(&mut self) -> Result<()> {
        let Some(storage) = self.data_storage() else {
            return Ok(());
        };

        let keys: Vec<(String, String)> = self.data.docs.keys().cloned().collect();
        for (space, doc) in &keys {
            let map_key = (space.clone(), doc.clone());

            // Fold pending edits in first: the snapshot below has to be the
            // whole document, including what has not been flushed yet.
            let committed = {
                let Some(entry) = self.data.docs.get_mut(&map_key) else {
                    continue;
                };
                entry.doc.commit().map_err(map_data_error)?.is_some()
            };
            let compacted = {
                let Some(entry) = self.data.docs.get(&map_key) else {
                    continue;
                };
                entry.doc.export_compacted().map_err(map_data_error)?
            };

            if let Err(err) = self.write_state_record(
                storage.as_ref(),
                storage_keys::DATA_DOCS,
                &doc_key(space, doc),
                &compacted,
            ) {
                // The commit above advanced the export marker. Leaving it
                // advanced would drop those edits from every future delta.
                if committed {
                    if let Some(entry) = self.data.docs.get_mut(&map_key) {
                        entry.doc.rewind_last_commit();
                    }
                }
                return Err(Error::Other(format!(
                    "failed to move document {space}/{doc} into the new backend: {err}"
                )));
            }

            if let Some(entry) = self.data.docs.get_mut(&map_key) {
                // The new backend holds one snapshot and no delta log.
                entry.next_seq = 0;
                entry.log_bytes = 0;
                entry.compacted_bytes = compacted.len();
                entry.doc.mark_compacted(compacted.len());
            }
        }

        // The space indexes carry document names that have no records of
        // their own yet; without them the new backend forgets those names.
        let spaces: Vec<String> = self.data.spaces.keys().cloned().collect();
        for space in spaces {
            self.persist_space(storage.as_ref(), &space)?;
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
        offline_protocol_data::validate_name(space).map_err(map_data_error)?;
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
            if let Err(err) = self.flush_doc(storage.as_ref(), &space, &doc) {
                warn!(space, doc, error = %err, "Failed to flush document");
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
    pub fn data_wipe_all(&mut self) -> Result<()> {
        let Some(storage) = self.data_storage() else {
            return Ok(());
        };
        self.data.docs.clear();
        self.data.spaces.clear();
        for key_type in [
            storage_keys::DATA_DOCS,
            storage_keys::DATA_DELTA_LOG,
            storage_keys::DATA_SPACES,
        ] {
            if let Ok(keys) = storage.list_keys(key_type) {
                for key in keys {
                    let _ = storage.delete(key_type, &key);
                }
            }
        }
        Ok(())
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
