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
//! | `data_doc_meta` | `{space}/{doc}` | What a name says after a removal |
//!
//! # Why the listing is the truth and the index is a cache
//!
//! Everything needed to re-open a document is derived from what is actually
//! on disk: `list_keys` names the delta records, and their zero-padded hex
//! sequence sorts into apply order. The space record only remembers document
//! names, so a document created and never written still lists. Any crash
//! therefore leaves a readable store rather than a bookkeeping claim that
//! has to be reconciled against reality.
//!
//! Removals are the one thing the listing cannot express, and the reason
//! `data_doc_meta` exists. A removed document has no records, and neither
//! does a document nobody ever created: reading the truth off the listing
//! would make those two the same state, which is exactly what lets a peer's
//! next offer refill something this device deleted on purpose. The removal
//! record is therefore written *before* the records it removes, so a crash
//! in between leaves a name already marked removed rather than a name whose
//! surviving records vote it back into existence.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use offline_protocol_data::{
    policy, CatchUp, DataDoc, DataError, DataValue, RemoteImport, VersionToken,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::{Error, Result};
use crate::events::{DocRemovedBy, Event};
use crate::protocol::data_sync::MAX_REMOVAL_FLOOR_BYTES;
use crate::protocol::storage::StateRecord;
use crate::protocol::types::{storage_keys, MAX_PROTOCOL_STATE_RECORD_BYTES};
use crate::protocol::OfflineProtocol;
use crate::protocol_state_storage::ProtocolStateStorage;

/// How many interest patterns one space accepts.
///
/// A bound rather than a policy: the list is walked once per document per
/// offer, so an unbounded one would let an application make its own peers
/// expensive to answer.
pub(crate) const MAX_INTEREST_PATTERNS: usize = 32;

/// Whether one interest pattern matches a document name.
///
/// `*` alone is everything; a trailing `*` is a prefix; anything else is an
/// exact name. The document charset (`A-Z a-z 0-9 . _ -`) has no `*` in it,
/// so a wildcard is never ambiguous with a name that contains one.
pub(crate) fn interest_matches(pattern: &str, doc: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => doc.starts_with(prefix),
        None => pattern == doc,
    }
}

/// Reject a pattern that could never match, or that names something the
/// document charset cannot express.
///
/// Checked where it is written rather than where it is used, so an
/// application hears about a typo at the call that made it instead of
/// silently receiving nothing.
fn validate_interest_pattern(pattern: &str) -> Result<()> {
    let body = pattern.strip_suffix('*').unwrap_or(pattern);
    if body.is_empty() {
        // Either `*` (everything) or an empty exact name. The first is
        // valid, the second cannot match a name, since names are non-empty.
        return if pattern == "*" {
            Ok(())
        } else {
            Err(Error::InvalidArgument(
                "an interest pattern may not be empty".to_string(),
            ))
        };
    }
    if body.len() > offline_protocol_data::MAX_NAME_LEN {
        return Err(Error::InvalidArgument(format!(
            "an interest pattern may not exceed {} bytes, got {}",
            offline_protocol_data::MAX_NAME_LEN,
            body.len()
        )));
    }
    if !body
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(Error::InvalidArgument(format!(
            "an interest pattern may only use the document charset \
             (A-Z a-z 0-9 . _ -), with an optional trailing `*`: {pattern}"
        )));
    }
    Ok(())
}

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

/// What a document's name says after a removal.
///
/// The `gone` field is the removal *floor*: the version the document stood
/// at when it was removed, which is the whole of what a removal decided
/// about. Content at or below it was removed; content beyond it is an edit
/// made concurrently with the removal or after it, which the removal never
/// saw. That is what makes an edit win over a removal without either side
/// needing to know which happened first.
///
/// `alive` is separate from the floor on purpose, and is not derivable from
/// whether records exist. A crash between the removal record and the
/// deletes leaves records behind, and reading those as "alive" is precisely
/// the resurrection this record prevents. The floor survives a document
/// coming back, so a stale replica's copy of the removed content is still
/// refused afterwards.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DocMeta {
    /// Base64 of the version the document was removed at.
    gone: String,
    /// Whether the name holds a document again.
    ///
    /// Defaults false so a record that fails to carry the field reads as
    /// removed, which is the answer that loses no data: a document read as
    /// removed comes back from any replica that still holds it, while one
    /// wrongly read as live is a resurrection nothing reports.
    #[serde(default)]
    alive: bool,
}

/// What applying a peer's removal did to this device's copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TombstoneOutcome {
    /// The copy held here was content the floor covered, and is gone.
    Deleted,
    /// The copy held here carries an edit the removal never saw, so it
    /// stays and goes back to the peer through the ordinary exchange.
    Kept,
    /// Nothing was held under that name; the floor is recorded so a replica
    /// that still holds the removed content cannot seed it back.
    Recorded,
    /// A floor already held here covers this one, so it carried nothing
    /// new and nothing was written or opened. The common case: floors
    /// travel on every offer for the life of the space.
    Unchanged,
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
    /// Removal state per document, loaded with the space that holds it.
    meta: BTreeMap<(String, String), DocMeta>,
    /// What this device wants from its peers, per space.
    ///
    /// Absent means everything, which is what a space nobody has narrowed
    /// says and what every release before interest existed said. Deliberately
    /// not persisted: it is application policy about this launch, not a fact
    /// about the store, and a durable copy would be a second thing to
    /// reconcile against an application that has changed its mind.
    interest: BTreeMap<String, Vec<String>>,
}

impl std::fmt::Debug for DataLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataLayer")
            .field("open_docs", &self.docs.len())
            .field("loaded_spaces", &self.spaces.len())
            .field("removed_docs", &self.meta.len())
            .field("narrowed_spaces", &self.interest.len())
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

        // Removals last, because they overrule the listing. A name the
        // reconciliation above just voted back into existence may be one a
        // previous run removed and did not finish deleting, and the records
        // it left are what that vote was counted from.
        let mut half_removed = Vec::new();
        if let Ok(keys) = storage.list_keys(storage_keys::DATA_DOC_META) {
            let prefix = format!("{space}/");
            for key in keys {
                let Some(name) = key.strip_prefix(&prefix) else {
                    continue;
                };
                if name.is_empty() || name.contains('/') {
                    continue;
                }
                let meta = match self.read_state_record(storage, storage_keys::DATA_DOC_META, &key)
                {
                    Ok(Some(bytes)) => match serde_json::from_slice::<DocMeta>(&bytes) {
                        Ok(meta) => meta,
                        Err(err) => {
                            // Unreadable here is not the index's "costs a
                            // name" case: this record is the only thing that
                            // says a document was removed, so a damaged one
                            // is left in place and the name is left alone
                            // rather than being silently revived.
                            warn!(space, doc = name, error = %err, "Unreadable document removal record");
                            continue;
                        }
                    },
                    Ok(None) => continue,
                    Err(err) => {
                        warn!(space, doc = name, error = %err, "Failed to read a document removal record");
                        continue;
                    }
                };
                if !meta.alive && record.docs.remove(name) {
                    half_removed.push(name.to_string());
                }
                self.data
                    .meta
                    .insert((space.to_string(), name.to_string()), meta);
            }
        }

        self.data.spaces.insert(space.to_string(), record);

        // Finish what a crash interrupted. Best effort, and repeated on
        // every open until it succeeds: the name is already removed as far
        // as every reader is concerned, so what is left is storage to
        // reclaim rather than a decision to make.
        for doc in half_removed {
            debug!(
                space,
                doc, "Finishing a removal a previous run did not complete"
            );
            self.purge_doc_records(storage, space, &doc, None);
        }
        Ok(())
    }

    /// Delete every record belonging to one document, reporting the first
    /// failure and attempting the rest regardless.
    ///
    /// A record that survives is replayed into the *next* document of the
    /// same name, which resurrects the removed contents, so a partial
    /// failure is worth hearing about even though every caller here can only
    /// log it.
    ///
    /// `delta_keys` is the delta log's listing when the caller already holds
    /// one, so a caller purging many documents lists once rather than once
    /// per document. It has to postdate the document's last flush, or the
    /// delta that flush wrote is exactly the record that survives.
    fn purge_doc_records(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        space: &str,
        doc: &str,
        delta_keys: Option<&[String]>,
    ) -> Option<String> {
        fn note(
            first_error: &mut Option<String>,
            err: crate::protocol_state_storage::ProtocolStateError,
        ) {
            if first_error.is_none() {
                *first_error = Some(err.to_string());
            }
        }
        let mut first_error: Option<String> = None;

        if let Err(err) = storage.delete(storage_keys::DATA_DOCS, &doc_key(space, doc)) {
            note(&mut first_error, err);
        }
        let prefix = format!("{space}/{doc}/");
        let listed;
        let keys: &[String] = match delta_keys {
            Some(keys) => keys,
            None => match storage.list_keys(storage_keys::DATA_DELTA_LOG) {
                Ok(keys) => {
                    listed = keys;
                    &listed
                }
                Err(err) => {
                    note(&mut first_error, err);
                    &[]
                }
            },
        };
        for key in keys.iter().filter(|key| key.starts_with(&prefix)) {
            if let Err(err) = storage.delete(storage_keys::DATA_DELTA_LOG, key) {
                note(&mut first_error, err);
            }
        }
        first_error
    }

    /// Write one document's removal record.
    fn persist_doc_meta(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        space: &str,
        doc: &str,
        meta: &DocMeta,
    ) -> Result<()> {
        let bytes =
            serde_json::to_vec(meta).map_err(|err| Error::Serialization(err.to_string()))?;
        self.write_state_record(
            storage,
            storage_keys::DATA_DOC_META,
            &doc_key(space, doc),
            &bytes,
        )
        .map_err(|err| {
            Error::Other(format!(
                "failed to record the removal of {space}/{doc}: {err}"
            ))
        })
    }

    /// Write one document's removal record and then remember it, in that
    /// order: what memory says must never be ahead of what a relaunch will
    /// read.
    fn set_doc_meta(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        space: &str,
        doc: &str,
        meta: DocMeta,
    ) -> Result<()> {
        self.persist_doc_meta(storage, space, doc, &meta)?;
        self.data
            .meta
            .insert((space.to_string(), doc.to_string()), meta);
        Ok(())
    }

    /// Whether this name currently stands removed.
    ///
    /// The space must already be loaded.
    fn doc_is_removed(&self, space: &str, doc: &str) -> bool {
        self.data
            .meta
            .get(&(space.to_string(), doc.to_string()))
            .is_some_and(|meta| !meta.alive)
    }

    /// Let a removed name hold a document again, keeping its floor.
    ///
    /// The floor outlives the removal it recorded, because a replica that
    /// never heard about the removal still holds the old contents and will
    /// offer them back. Dropping the floor here would let exactly that
    /// offer refill the new document with the old one.
    fn revive_doc(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        space: &str,
        doc: &str,
    ) -> Result<()> {
        let key = (space.to_string(), doc.to_string());
        let Some(meta) = self.data.meta.get(&key) else {
            return Ok(());
        };
        if meta.alive {
            return Ok(());
        }
        let revived = DocMeta {
            gone: meta.gone.clone(),
            alive: true,
        };
        self.set_doc_meta(storage, space, doc, revived)
    }

    /// The removal floor recorded for one document, if it has one.
    ///
    /// The space must already be loaded; every caller reaches this through a
    /// path that loads it.
    fn doc_floor(&self, space: &str, doc: &str) -> Option<VersionToken> {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

        let meta = self.data.meta.get(&(space.to_string(), doc.to_string()))?;
        match BASE64.decode(&meta.gone) {
            Ok(bytes) => Some(VersionToken::from_bytes(bytes)),
            Err(err) => {
                warn!(space, doc, error = %err, "Undecodable removal floor; treating the document as never removed");
                None
            }
        }
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
    ///
    /// A removed name is refused rather than opened empty. Opening it would
    /// create a live document out of a removal, and every write to it would
    /// then be a change to something no replica holds. Bringing the name
    /// back is a decision with a caller: `data_create_doc` locally, or a
    /// peer's change from beyond the removal floor.
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
        if self.doc_is_removed(space, doc) {
            return Err(Error::InvalidState(format!(
                "document {space}/{doc} was removed; create it again to reuse the name"
            )));
        }

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

        // Removal records have no document to carry them, so a backend swap
        // that left them behind would move a space into a store where every
        // removed name reads as one nobody ever created, and the next offer
        // from any peer would refill all of them.
        let removals: Vec<((String, String), DocMeta)> = self
            .data
            .meta
            .iter()
            .map(|(key, meta)| (key.clone(), meta.clone()))
            .collect();
        for ((space, doc), meta) in removals {
            self.persist_doc_meta(storage, &space, &doc, &meta)?;
        }
        Ok(())
    }

    /// Create a document, or do nothing if it already exists.
    ///
    /// A name that was removed comes back empty, keeping its removal floor,
    /// so the old contents cannot arrive from a replica that never heard
    /// about the removal.
    pub fn data_create_doc(&mut self, space: &str, doc: &str) -> Result<()> {
        Self::validate_ids(space, doc)?;
        let storage = self.require_data_storage()?;
        self.load_space(storage.as_ref(), space)?;
        self.revive_doc(storage.as_ref(), space, doc)?;
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

    /// Every removal this space knows about, with the floor it recorded.
    ///
    /// Names that came back are included, because the floor still describes
    /// content that was removed: a third replica that never heard about the
    /// removal is holding exactly that content, and this is what tells it
    /// to drop it.
    pub(crate) fn data_sync_tombstones(&mut self, space: &str) -> Result<BTreeMap<String, String>> {
        let Some(storage) = self.data_storage_for_sync() else {
            return Ok(BTreeMap::new());
        };
        offline_protocol_data::validate_space_name(space).map_err(map_data_error)?;
        self.load_space(storage.as_ref(), space)?;
        Ok(self
            .data
            .meta
            .range((space.to_string(), String::new())..)
            .take_while(|((known, _), _)| known == space)
            .map(|((_, doc), meta)| (doc.clone(), meta.gone.clone()))
            .collect())
    }

    /// Apply a removal a peer told us about.
    ///
    /// The floor decides, never the message: a copy the floor covers is
    /// removed, and a copy carrying anything beyond it is kept, because that
    /// is an edit the removal never saw. Either way the floor is recorded,
    /// including for a name this device has never held, so a third replica
    /// still holding the removed content cannot seed it back later.
    ///
    /// A floor this device already holds, or one a held floor covers, is
    /// answered from memory with no write and no open. Floors are carried on
    /// every offer for the life of the space, so that is the common case,
    /// and it has to be free for two reasons. The obvious one is cost: one
    /// sealed record per removed name per exchange, from every peer. The
    /// other is a name that was brought back on purpose and not yet written
    /// to: every version covers the empty one, so a floor that was already
    /// judged would read the fresh document as removed content and delete
    /// it. A floor decides once, when it first arrives.
    pub(crate) fn data_apply_tombstone(
        &mut self,
        space: &str,
        doc: &str,
        floor: &VersionToken,
    ) -> Result<TombstoneOutcome> {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

        Self::validate_ids(space, doc)?;
        // Checked once on the way in. A floor that does not decode is stored
        // for the life of the space and covers nothing, which costs a record
        // for no removal at all.
        floor.validate().map_err(map_data_error)?;
        let storage = self.require_data_storage()?;
        self.load_space(storage.as_ref(), space)?;

        let held = self
            .data
            .spaces
            .get(space)
            .is_some_and(|record| record.docs.contains(doc));

        let merged = match self.doc_floor(space, doc) {
            Some(existing) if existing.includes(floor).map_err(map_data_error)? => {
                return Ok(TombstoneOutcome::Unchanged);
            }
            Some(existing) => existing.union(floor).map_err(map_data_error)?,
            None => floor.clone(),
        };
        // The union only ever grows, and a peer can grow it by naming a
        // fresh replica in every offer. Bounded here rather than at the
        // frame, because the frame bounds one floor and this bounds the
        // sum of every floor ever merged into this name.
        if merged.as_bytes().len() > MAX_REMOVAL_FLOOR_BYTES {
            return Err(Error::Other(format!(
                "removal floor for {space}/{doc} would exceed {MAX_REMOVAL_FLOOR_BYTES} bytes"
            )));
        }
        let encoded = BASE64.encode(merged.as_bytes());

        if !held {
            let alive = self
                .data
                .meta
                .get(&(space.to_string(), doc.to_string()))
                .is_some_and(|meta| meta.alive);
            self.set_doc_meta(
                storage.as_ref(),
                space,
                doc,
                DocMeta {
                    gone: encoded,
                    alive,
                },
            )?;
            return Ok(TombstoneOutcome::Recorded);
        }

        // Pending edits count as ours and are flushed before the comparison,
        // or an edit made a moment ago would read as content the removal
        // covered and be deleted by it.
        if let Err(err) = self.data_flush(space, doc) {
            debug!(space, doc, error = %err, "Could not flush before judging a removal; judging the last durable version");
        }
        let ours =
            VersionToken::from_bytes(BASE64.decode(self.data_doc_version(space, doc)?).map_err(
                |err| Error::Serialization(format!("undecodable document version: {err}")),
            )?);

        if !merged.includes(&ours).map_err(map_data_error)? {
            // Beyond the floor: an edit the removal never saw. The document
            // stays, the floor is kept, and the peer gets the edit back
            // through the ordinary exchange.
            self.set_doc_meta(
                storage.as_ref(),
                space,
                doc,
                DocMeta {
                    gone: encoded,
                    alive: true,
                },
            )?;
            return Ok(TombstoneOutcome::Kept);
        }

        self.set_doc_meta(
            storage.as_ref(),
            space,
            doc,
            DocMeta {
                gone: encoded,
                alive: false,
            },
        )?;

        self.data.docs.remove(&(space.to_string(), doc.to_string()));
        let first_error = self.purge_doc_records(storage.as_ref(), space, doc, None);
        if let Some(record) = self.data.spaces.get_mut(space) {
            record.docs.remove(doc);
        }
        self.persist_space(storage.as_ref(), space)?;
        if let Some(detail) = first_error {
            warn!(
                space,
                doc,
                detail,
                "Removed a document a peer removed, but not every record could be deleted"
            );
        }

        self.emit_event(Event::DataDocRemoved {
            space_id: space.to_string(),
            doc_id: doc.to_string(),
            by: DocRemovedBy::Peer,
        });
        Ok(TombstoneOutcome::Deleted)
    }

    /// The documents this device wants from its peers in one space.
    ///
    /// `patterns` is a list of document names, each of which MAY end in `*`
    /// to match a prefix. `["*"]` is everything and is the default; `[]` is
    /// nothing. The document charset excludes `*`, so the wildcard can never
    /// be part of a name it is matching.
    ///
    /// Two halves, and only one of them is a request. Toward a peer this is
    /// carried in every version offer, so the peer sends nothing else: that
    /// is the traffic saving, and it depends on the peer reading the field.
    /// Locally it is a refusal, so a document outside it is never created
    /// from an offer and never imported however it arrives. The refusal is
    /// what makes this safe against a peer that ignores the request, which
    /// includes every peer on a build that predates it.
    ///
    /// Narrowing does not delete what is already held. A document that falls
    /// outside the new patterns stops being updated and stays where it is,
    /// because deleting on a policy change would make a narrowing lose data
    /// no caller asked to lose. `data_delete_doc` is how it leaves.
    ///
    /// Widening asks: the newly wanted documents are absent from this
    /// device's next offer and inside its declared want, so the peer answers
    /// with them.
    pub fn data_set_interest(&mut self, space: &str, patterns: Vec<String>) -> Result<()> {
        offline_protocol_data::validate_space_name(space).map_err(map_data_error)?;
        if patterns.len() > MAX_INTEREST_PATTERNS {
            return Err(Error::InvalidArgument(format!(
                "at most {MAX_INTEREST_PATTERNS} interest patterns per space, got {}",
                patterns.len()
            )));
        }
        for pattern in &patterns {
            validate_interest_pattern(pattern)?;
        }
        let everything = patterns.len() == 1 && patterns[0] == "*";
        if everything {
            // The default, expressed explicitly. Held as absence so the wire
            // and every read below take the cheap path.
            self.data.interest.remove(space);
        } else {
            self.data.interest.insert(space.to_string(), patterns);
        }
        self.nudge_data_sync(space, None, "interest_changed");
        Ok(())
    }

    /// The patterns declared for a space, or `None` for the default.
    pub(crate) fn data_interest(&self, space: &str) -> Option<&Vec<String>> {
        self.data.interest.get(space)
    }

    /// Whether this device wants `doc` in `space`.
    pub(crate) fn data_wants(&self, space: &str, doc: &str) -> bool {
        match self.data.interest.get(space) {
            None => true,
            Some(patterns) => patterns
                .iter()
                .any(|pattern| interest_matches(pattern, doc)),
        }
    }

    /// Let a removed name hold a document again, because something beyond
    /// its floor is about to be imported into it.
    ///
    /// Separate from the refusal, and called only after it: content beyond a
    /// removal floor is an edit the removal never saw, and an edit beats a
    /// removal. The name comes back keeping its floor, so the *rest* of the
    /// removed content still cannot arrive from a replica that has not heard
    /// about the removal.
    pub(crate) fn data_revive_removed_doc(&mut self, space: &str, doc: &str) -> Result<()> {
        let storage = self.require_data_storage()?;
        self.load_space(storage.as_ref(), space)?;
        self.revive_doc(storage.as_ref(), space, doc)
    }

    /// The removal floor for a document, with its space loaded first.
    ///
    /// [`Self::doc_floor`] answers off memory and needs the space loaded.
    /// The sync paths below can be the first thing to touch a space after
    /// a launch, on the media road in particular, and a floor missed there
    /// is a removed document refilled.
    fn loaded_doc_floor(&mut self, space: &str, doc: &str) -> Option<VersionToken> {
        let storage = self.data_storage_for_sync()?;
        self.load_space(storage.as_ref(), space).ok()?;
        self.doc_floor(space, doc)
    }

    /// Whether a document has a removal floor at all.
    ///
    /// What decides whether a blob for it has to be judged against one.
    /// That judgement is an engine decode and has to run inside an
    /// in-flight marker, so a name with no floor, which is every name in a
    /// space nobody has removed anything from, skips the marker and the
    /// decode both.
    pub(crate) fn data_doc_has_floor(&mut self, space: &str, doc: &str) -> bool {
        self.loaded_doc_floor(space, doc).is_some()
    }

    /// Whether a version a peer offers is content a removal already covered.
    ///
    /// Cheap and answered from memory: the overwhelming case is a space with
    /// no removals at all, where there is nothing to decode.
    pub(crate) fn data_version_below_floor(
        &mut self,
        space: &str,
        doc: &str,
        theirs: &VersionToken,
    ) -> bool {
        let Some(floor) = self.loaded_doc_floor(space, doc) else {
            return false;
        };
        floor.includes(theirs).unwrap_or(false)
    }

    /// Whether a blob carries nothing beyond a removal floor.
    ///
    /// An engine decode, not a header read: the blob's changes are decoded
    /// into a scratch document to find the version they end at, with the
    /// same decoder an import runs. The caller owes it the containment an
    /// import gets, an in-flight marker written before and cleared after;
    /// see [`offline_protocol_data::blob_end_version`]. A document with no
    /// floor skips the decode entirely, which is every document in a space
    /// nobody has removed anything from.
    pub(crate) fn data_blob_below_floor(&mut self, space: &str, doc: &str, blob: &[u8]) -> bool {
        let Some(floor) = self.loaded_doc_floor(space, doc) else {
            return false;
        };
        match offline_protocol_data::blob_end_version(blob) {
            Ok(end) => floor.includes(&end).unwrap_or(false),
            Err(err) => {
                // Undecodable bytes are refused a few steps later by the
                // import itself, which reports properly. Not below the
                // floor, because nothing here established that.
                debug!(space, doc, error = %err, "Could not read a blob's version to judge it against a removal");
                false
            }
        }
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
    /// Removed names count against the cap alongside held ones. Each one is
    /// a record a peer talked this device into storing, and the record
    /// outlives the document by design, so counting only what is held would
    /// let a peer name a fresh thousand documents after every removal
    /// sweep.
    ///
    /// A name already known here, held or removed, always admits: nothing a
    /// peer says about it costs a record that does not already exist. That
    /// matters at the ceiling, where a space stays for good once it gets
    /// there, because floors never expire. Refusing a known name there
    /// would refuse the floor a peer re-sends on every offer, and a removal
    /// for a held document sorted after it.
    pub(crate) fn data_space_admits_doc(&mut self, space: &str, doc: &str, cap: usize) -> bool {
        // Called once per name in an inbound frame, so it reads both fields
        // in place rather than cloning the index: a peer may send offers as
        // fast as the link carries them, and this is one of the things each
        // one pays for.
        if self.space_docs(space).is_none() {
            return true;
        }
        let Some(record) = self.data.spaces.get(space) else {
            return true;
        };
        if record.docs.contains(doc)
            || self
                .data
                .meta
                .contains_key(&(space.to_string(), doc.to_string()))
        {
            return true;
        }
        let removed = self
            .data
            .meta
            .range((space.to_string(), String::new())..)
            .take_while(|((known, _), _)| known == space)
            .filter(|((_, name), _)| !record.docs.contains(name.as_str()))
            .count();
        record.docs.len() + removed < cap
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

    /// Drop this device's copy of a document, leaving the peers' copies
    /// alone.
    ///
    /// Eviction, not removal: it reclaims the space a document occupies here
    /// and records nothing about the name, so the next version exchange with
    /// a replica that still holds it recreates and refills it. That is the
    /// right call for a cache and the wrong one for content somebody wants
    /// gone from the room, which is what [`Self::data_remove_doc`] is for.
    pub fn data_delete_doc(&mut self, space: &str, doc: &str) -> Result<()> {
        Self::validate_ids(space, doc)?;
        let storage = self.require_data_storage()?;
        self.load_space(storage.as_ref(), space)?;

        self.data.docs.remove(&(space.to_string(), doc.to_string()));
        let first_error = self.purge_doc_records(storage.as_ref(), space, doc, None);
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

    /// Remove a document from every replica of its space.
    ///
    /// Records the version the document stood at as its removal floor, then
    /// deletes it here and tells the space. A replica whose copy the floor
    /// covers deletes it too; a replica holding an edit the floor does not
    /// cover keeps its copy and hands the document back, contents and all,
    /// because that edit's history *is* those contents. An edit therefore
    /// beats a removal, whichever happened first, and the pair never
    /// produces a half-emptied document.
    ///
    /// Pending edits here are flushed first so the floor covers them:
    /// removing a document must not leave this device's own unsent work
    /// looking like somebody else's concurrent edit.
    ///
    /// Removing a name this device does not hold does nothing. There is no
    /// version to take a floor from, and an empty floor would name no
    /// content at all.
    ///
    /// Re-using the name afterwards starts a document whose history is
    /// disjoint from the floor. What it does not do is fence the old
    /// contents off: a replica that edited them concurrently keeps them and
    /// the ordinary exchange merges them into the new document, and a
    /// replica that never learned of the removal merges the new contents
    /// into its old copy. A name is cheap; a fresh one is the safe choice.
    pub fn data_remove_doc(&mut self, space: &str, doc: &str) -> Result<()> {
        Self::validate_ids(space, doc)?;
        let storage = self.require_data_storage()?;
        self.load_space(storage.as_ref(), space)?;
        if !self.removable(space, doc) {
            return Ok(());
        }

        // Errors here are logged and stepped over, exactly as the import
        // path's pre-flush does. A document over its cap fails every flush
        // and must still be removable; refusing would leave the only way out
        // of the cap closed. The floor is taken from the version held in
        // memory either way, which already includes what the flush could
        // not write.
        if let Err(err) = self.data_flush(space, doc) {
            debug!(space, doc, error = %err, "Could not flush before removing; the floor is taken from the version held in memory");
        }

        let first_error = self.record_removal(storage.as_ref(), space, doc, None)?;
        self.persist_space(storage.as_ref(), space)?;
        self.nudge_data_sync(space, None, "document_removed");

        match first_error {
            Some(detail) => Err(Error::Other(format!(
                "document {space}/{doc} is removed, but not every record could be deleted: \
                 {detail}"
            ))),
            None => Ok(()),
        }
    }

    /// Whether a removal has anything to act on: the document is held here
    /// and does not already stand removed.
    ///
    /// Removing a name this device does not hold does nothing. There is no
    /// version to take a floor from, and an empty floor would name no
    /// content at all. The space must already be loaded.
    fn removable(&self, space: &str, doc: &str) -> bool {
        if self.doc_is_removed(space, doc) {
            return false;
        }
        let held = self
            .data
            .spaces
            .get(space)
            .is_some_and(|record| record.docs.contains(doc));
        if !held {
            debug!(
                space,
                doc, "Nothing to remove: this device does not hold the document"
            );
        }
        held
    }

    /// Record a document's removal floor, drop the copy held here and purge
    /// its records, reporting the first record that would not delete.
    ///
    /// What one removal writes, without what a batch of them shares: the
    /// space index is not persisted here and the space is not told, so a
    /// caller removing every document in a space does each once at the end
    /// rather than once per document.
    ///
    /// The floor is written before the records it removes. A crash in
    /// between leaves a name every reader already treats as removed, which
    /// the next open finishes; the reverse order leaves records that vote
    /// the document back into existence.
    ///
    /// `delta_keys` is the delta log's listing when the caller already has
    /// one. It has to postdate the document's last flush, or the delta that
    /// flush wrote is missed by the purge and replayed into the next
    /// document of the same name.
    fn record_removal(
        &mut self,
        storage: &dyn ProtocolStateStorage,
        space: &str,
        doc: &str,
        delta_keys: Option<&[String]>,
    ) -> Result<Option<String>> {
        let floor = self.data_doc_version(space, doc)?;
        let gone = match self.doc_floor(space, doc) {
            // Two removals of one name, ours and one we learned from a
            // peer, each cover changes the other does not.
            Some(existing) => {
                use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
                let taken = VersionToken::from_bytes(BASE64.decode(&floor).map_err(|err| {
                    Error::Serialization(format!("undecodable document version: {err}"))
                })?);
                BASE64.encode(existing.union(&taken).map_err(map_data_error)?.as_bytes())
            }
            None => floor,
        };
        self.set_doc_meta(storage, space, doc, DocMeta { gone, alive: false })?;

        self.data.docs.remove(&(space.to_string(), doc.to_string()));
        let first_error = self.purge_doc_records(storage, space, doc, delta_keys);
        if let Some(record) = self.data.spaces.get_mut(space) {
            record.docs.remove(doc);
        }

        self.emit_event(Event::DataDocRemoved {
            space_id: space.to_string(),
            doc_id: doc.to_string(),
            by: DocRemovedBy::Local,
        });
        Ok(first_error)
    }

    /// Remove every document a space holds, from every replica of it.
    ///
    /// Each document is attempted even after one fails, and the first error
    /// is what the caller hears. What every removal shares is done once for
    /// the lot: the delta log is listed once, the space index is written
    /// once, and the space is told once, with one offer carrying every
    /// removal. Per document that is two full listings and one sealed index
    /// write fewer, which on a custom backend is three round trips under the
    /// engine lock, for each of up to a thousand names.
    pub fn data_remove_space(&mut self, space: &str) -> Result<()> {
        offline_protocol_data::validate_space_name(space).map_err(map_data_error)?;
        let storage = self.require_data_storage()?;
        self.load_space(storage.as_ref(), space)?;
        let docs: Vec<String> = self
            .data_list_docs(space)?
            .into_iter()
            .filter(|doc| self.removable(space, doc))
            .collect();
        if docs.is_empty() {
            return Ok(());
        }

        // Every document is flushed before the delta log is listed, and
        // nothing writes to the log between the listing and the purges. A
        // flush after the listing would write a delta the listing does not
        // name, the purge would leave it behind, and the next document under
        // that name would replay it: the removed contents, back.
        for doc in &docs {
            if let Err(err) = self.data_flush(space, doc) {
                debug!(space, doc, error = %err, "Could not flush before removing; the floor is taken from the version held in memory");
            }
        }
        let delta_keys = match storage.list_keys(storage_keys::DATA_DELTA_LOG) {
            Ok(keys) => Some(keys),
            Err(err) => {
                // Each removal lists for itself, as a single one does.
                warn!(space, error = %err, "Could not list the delta log once for the space; listing per document");
                None
            }
        };

        let mut first_error: Option<Error> = None;
        let mut note = |err: Error| {
            if first_error.is_none() {
                first_error = Some(err);
            }
        };
        for doc in &docs {
            match self.record_removal(storage.as_ref(), space, doc, delta_keys.as_deref()) {
                Ok(None) => {}
                Ok(Some(detail)) => note(Error::Other(format!(
                    "document {space}/{doc} is removed, but not every record could be deleted: \
                     {detail}"
                ))),
                Err(err) => note(err),
            }
        }

        // The index once, at the end. A crash before it leaves an index
        // that still lists the removed names, and the next open reads the
        // removal records after it and drops them: the index is a cache,
        // and the removal record is what decides.
        if let Err(err) = self.persist_space(storage.as_ref(), space) {
            note(err);
        }
        // Told regardless of errors. An error past a removal record is
        // still a removal, and one before it costs at worst an offer that
        // says nothing new.
        self.nudge_data_sync(space, None, "space_removed");

        match first_error {
            Some(err) => Err(err),
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
    /// deleted still lists while its index record survives, and a space
    /// whose documents were all removed lists for as long as its removal
    /// records do, which is the life of the space.
    pub fn data_list_spaces(&mut self) -> Result<Vec<String>> {
        let storage = self.require_data_storage()?;
        let mut spaces = BTreeSet::new();
        // Removal records count: a space whose documents were all removed
        // still has something to say to its peers, and leaving it out of
        // this list is what would stop the start-up sweep telling them.
        for key_type in [
            storage_keys::DATA_SPACES,
            storage_keys::DATA_DOCS,
            storage_keys::DATA_DELTA_LOG,
            storage_keys::DATA_DOC_META,
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
        self.data.meta.clear();
        // Every window and every outstanding question goes with the content
        // they were about. Left behind, a window suppresses the first offer
        // or the first blob request made after the wipe, which is the same
        // shape [`Self::forget_data_sync_peer`] exists to prevent: a window
        // that survives the thing it was measuring.
        //
        // The pending fetches are not bookkeeping. An entry there is what
        // admits arriving attachment bytes, so one surviving a wipe would let
        // bytes land for a space this device no longer has, and hand them to
        // the application as an answer to a question the wipe erased.
        self.last_data_sync_offer.clear();
        self.pending_attachment_fetches.clear();
        self.blob_request_windows.clear();

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
            storage_keys::DATA_DOC_META,
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
