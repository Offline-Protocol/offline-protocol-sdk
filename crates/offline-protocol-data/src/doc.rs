//! The replicated document: collections, commits, and encoding.
//!
//! This module is the only place the CRDT engine is named. Nothing it
//! exports mentions an engine type, so the engine can be replaced without a
//! breaking change to any caller, the FFI surface, or any binding. That is
//! the property the whole data layer's no-lock-in promise rests on, and it
//! is why `loro` appears in no `pub` signature below.

use std::panic::{catch_unwind, AssertUnwindSafe};

use loro::{EncodedBlobMode, ExportMode, LoroDoc, LoroValue, VersionVector};

use crate::error::{DataError, DataResult};
use crate::policy;
use crate::value::DataValue;

/// An opaque marker for "how much of this document I have already seen".
///
/// Callers treat it as bytes: store it, hand it back, compare it for
/// equality. Its interior is the engine's version vector, which is exactly
/// the kind of detail that must not leak into a wire format or an FFI
/// signature, because pinning it would pin the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionToken(Vec<u8>);

impl VersionToken {
    /// The token as bytes, for storage or transmission.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Rebuild a token from bytes produced by [`VersionToken::as_bytes`].
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }
}

/// A committed change, ready to persist and (from F3) to send.
#[derive(Debug, Clone)]
pub struct Delta {
    /// The encoded change.
    pub bytes: Vec<u8>,
    /// The document version after applying it.
    pub version: VersionToken,
}

/// Sizes and counters an owner needs to decide about compaction and caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocStats {
    /// Size of the compacted (shallow snapshot) encoding.
    pub compacted_bytes: usize,
    /// Number of commits recorded since the document was last compacted.
    pub commits_since_compaction: u32,
}

/// What happened to the changes in an imported blob.
///
/// The distinction is load-bearing and is the reason [`DataDoc::import`]
/// does not return `()`. The engine accepts a change whose causal
/// predecessor it has never seen, and answers `Ok`: the change is *parked*,
/// invisible to every read and absent from [`DataDoc::export_compacted`],
/// until the predecessor arrives. An owner that treats that `Ok` as "applied"
/// will happily compact the document and delete the very records the parked
/// change is still waiting for, which turns a recoverable gap into permanent
/// silent loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportOutcome {
    /// Every change in the blob applied and is readable now.
    Applied,
    /// Some or all of the blob is parked, waiting on a predecessor that has
    /// not arrived. The document is missing those changes until it does, so
    /// nothing that could destroy the missing predecessor may run.
    Parked,
}

impl ImportOutcome {
    /// Whether anything in the blob is still waiting on a predecessor.
    pub fn is_parked(self) -> bool {
        matches!(self, Self::Parked)
    }
}

/// What a blob claims about itself, judged against one document.
///
/// Every field is a verdict rather than a measurement, because the engine's
/// own metadata types must not cross this crate's boundary: pinning them in
/// a caller would pin the engine. The verdicts exist so a caller can decide
/// *not* to import, which is the only decision that helps when the bytes
/// came off a network rather than out of a sealed record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobMeta {
    already_applied: bool,
    spans_trimmed_history: bool,
    is_snapshot: bool,
    change_count: u32,
}

impl BlobMeta {
    /// Whether this document already holds every change the blob carries.
    ///
    /// True for the ordinary redelivery a ladder promising at-least-once
    /// produces. Importing anyway would be merely wasteful for a fresh
    /// document and actively dangerous for a compacted one, which is why
    /// this is checked before anything reaches the engine.
    pub fn already_applied(self) -> bool {
        self.already_applied
    }

    /// Whether the blob needs history this document has compacted away.
    ///
    /// A shallow snapshot trims history below a point, and the engine has an
    /// open defect where changes referring below that point panic instead of
    /// erroring, poisoning the document's lock as they go. Under the
    /// `minisize` profile the SDK ships with `panic = "abort"`, so there is
    /// no unwinding to catch: the only defense is never handing such a blob
    /// over.
    ///
    /// It is asked of a whole document as well as of a run of changes, and
    /// the two are asked differently. A snapshot carries its own base, so
    /// the question for one is not where it starts but whether it still
    /// covers what this document kept: a replica that trimmed history can
    /// merge a snapshot that contains the ops it retained, and aborts on one
    /// that does not. Exempting snapshots because they are self contained
    /// reads well and is wrong, because the missing ancestors are missing
    /// from the *receiver*.
    ///
    /// A caller seeing this on a run of changes asks for a snapshot. A
    /// caller seeing it on a snapshot has reached the end of what
    /// replication can do: the peer forked below a point this replica
    /// deleted, and nothing either side can send carries the ancestors back.
    pub fn spans_trimmed_history(self) -> bool {
        self.spans_trimmed_history
    }

    /// Whether the blob is a snapshot rather than a run of updates.
    pub fn is_snapshot(self) -> bool {
        self.is_snapshot
    }

    /// How many changes the blob claims to carry.
    pub fn change_count(self) -> u32 {
        self.change_count
    }
}

/// The outcome of importing a blob that arrived from a peer.
///
/// Distinct from [`ImportOutcome`] because two of the four outcomes never
/// reach the engine at all, and a caller has to tell "we already had this"
/// apart from "this applied" to keep its own bookkeeping honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteImport {
    /// Applied and readable now.
    Applied,
    /// Accepted but parked behind a predecessor that has not arrived.
    Parked,
    /// Every change in it was already present; the engine was not touched.
    AlreadyHave,
    /// Refused: it refers to history this document has compacted away.
    /// The caller should ask the sender for a snapshot instead.
    RefusedTrimmedHistory,
}

/// What a peer needs in order to catch up with this document.
///
/// The ladder is deliberately explicit rather than "here are some bytes":
/// only the caller knows the frame budget, and only this crate knows whether
/// the updates it would have to send still exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatchUp {
    /// The peer already has everything this document holds.
    UpToDate,
    /// The changes the peer is missing, encoded.
    Updates(Vec<u8>),
    /// The peer is missing changes this document has compacted away, so no
    /// run of updates can express the gap. Send a snapshot.
    NeedsSnapshot,
}

/// A replicated document.
///
/// Holds named collections (`map`, `list`, `text`, `counter`). Concurrent
/// edits from any replica merge deterministically, and applying the same
/// change twice is a no-op, which is what lets this layer ride a transport
/// ladder that only promises at-least-once, unordered delivery.
pub struct DataDoc {
    inner: LoroDoc,
    /// The version whose changes have already been handed out as deltas.
    /// Everything after it is what the next commit exports.
    exported: VersionVector,
    /// What `exported` was before the most recent commit.
    ///
    /// Kept so a caller whose persistence failed can put the marker back and
    /// have the same change handed out again. Without it, a commit whose
    /// delta never reached storage is lost: the document keeps the change in
    /// memory, the next commit exports only what came after it, and the
    /// change is simply missing after a restart.
    previous_exported: Option<VersionVector>,
    /// Sticky corruption flag. Set when a decode panics; every later call
    /// refuses rather than touching an engine left in an unknown state.
    poisoned: bool,
    commits_since_compaction: u32,
    last_measured_bytes: usize,
    delta_bytes_since_measure: usize,
}

impl Default for DataDoc {
    fn default() -> Self {
        Self::new()
    }
}

impl DataDoc {
    /// Create an empty document.
    pub fn new() -> Self {
        Self {
            inner: LoroDoc::new(),
            exported: VersionVector::new(),
            previous_exported: None,
            poisoned: false,
            commits_since_compaction: 0,
            last_measured_bytes: 0,
            delta_bytes_since_measure: 0,
        }
    }

    /// Whether this handle has been poisoned by a decode failure.
    ///
    /// A poisoned handle never recovers. The owner drops it and re-opens the
    /// document from storage, which is a decision only the owner can make.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    fn check_live(&self) -> DataResult<()> {
        if self.poisoned {
            return Err(DataError::Poisoned);
        }
        Ok(())
    }

    /// Apply an encoded snapshot or delta.
    ///
    /// Every blob reaching this method comes back out of a sealed
    /// protocol-state record, so an AEAD tag has already established that
    /// these are bytes this SDK wrote and nobody altered. That is the real
    /// containment for the engine's known import-panic classes, and the
    /// reason no code path may ever feed unverified bytes here: the
    /// `catch_unwind` below is a second line, and under the `minisize`
    /// profile (`panic = "abort"`) it does not exist at all.
    ///
    /// Returns whether the blob applied or is parked behind a missing
    /// predecessor. Callers that can destroy history (compaction, log
    /// truncation) must branch on it: see [`ImportOutcome`].
    pub fn import(&mut self, bytes: &[u8]) -> DataResult<ImportOutcome> {
        self.check_live()?;

        // Reject a blob the engine cannot even describe before asking it to
        // apply the contents. Checksum verification is on: it is cheap
        // relative to import and catches truncation that got past framing.
        if let Err(err) = LoroDoc::decode_import_blob_meta(bytes, true) {
            return Err(DataError::Corrupt(err.to_string()));
        }

        let outcome = catch_unwind(AssertUnwindSafe(|| self.inner.import(bytes)));
        match outcome {
            // `pending` names the changes the engine took but cannot apply
            // yet. Discarding it is how a gap in the delta log turns into
            // silent loss, so it is reported rather than dropped.
            Ok(Ok(status)) => Ok(match status.pending {
                Some(range) if !range.is_empty() => ImportOutcome::Parked,
                _ => ImportOutcome::Applied,
            }),
            Ok(Err(err)) => Err(DataError::Corrupt(err.to_string())),
            Err(_) => {
                // The engine may hold a poisoned lock now; the handle is done.
                self.poisoned = true;
                Err(DataError::Corrupt(
                    "engine panicked while importing".to_string(),
                ))
            }
        }
    }

    /// Judge a blob against this document without letting the engine touch
    /// it.
    ///
    /// The whole point is that this runs *first*. Header decoding with the
    /// checksum on is the cheap part; the verdicts it produces are what let
    /// a caller refuse the two blob shapes that are known to end a process
    /// rather than return an error.
    pub fn inspect(&self, bytes: &[u8]) -> DataResult<BlobMeta> {
        self.check_live()?;
        let meta = LoroDoc::decode_import_blob_meta(bytes, true)
            .map_err(|err| DataError::Corrupt(err.to_string()))?;

        let is_snapshot = matches!(
            meta.mode,
            EncodedBlobMode::Snapshot
                | EncodedBlobMode::OutdatedSnapshot
                | EncodedBlobMode::ShallowSnapshot
        );
        let already_applied = self.inner.oplog_vv().includes_vv(&meta.partial_end_vv);

        // A document that has never been trimmed can take anything.
        let shallow_since = self.inner.shallow_since_vv();
        let spans_trimmed_history = if shallow_since.is_empty() {
            false
        } else if is_snapshot {
            // A trimmed document cannot merge a branch at all: the ops the
            // branch depends on were deleted here, and supplying them in
            // the snapshot does not help, because they sit below this
            // document's own base. The only snapshot it can take is one
            // that supersedes it outright, so that is the test: everything
            // held here has to be in there too. Anything less is a fork,
            // and handing one over aborts the process exactly as a run of
            // changes of the same shape does.
            !meta.partial_end_vv.includes_vv(&self.inner.oplog_vv())
        } else {
            // If this document's history starts at or after where the blob's
            // does, the blob describes changes on top of ops that are no
            // longer here.
            shallow_since.to_vv().includes_vv(&meta.partial_start_vv)
        };

        Ok(BlobMeta {
            already_applied,
            spans_trimmed_history,
            is_snapshot,
            change_count: meta.change_num,
        })
    }

    /// Apply a blob that arrived from a peer.
    ///
    /// [`DataDoc::import`] exists for bytes that came back out of a sealed
    /// record, where an AEAD tag has already established that this SDK wrote
    /// them. Nothing of the sort is true here: a peer is authenticated, which
    /// says who sent the bytes and nothing at all about their shape. So this
    /// path re-judges every blob with [`DataDoc::inspect`] and declines the
    /// two shapes the engine is known to abort on, rather than relying on an
    /// unwinding catch that the shipped mobile profile does not have.
    ///
    /// Refusing is not a failure: a duplicate is the ladder keeping its
    /// at-least-once promise, and a trimmed-history gap in a run of changes
    /// is answered by asking for a snapshot. Both are recorded outcomes, not
    /// errors. The same refusal on a snapshot is the exception: it means the
    /// replicas have diverged past anything a frame can carry, and the
    /// caller has nothing left to ask for.
    pub fn import_remote(&mut self, bytes: &[u8]) -> DataResult<RemoteImport> {
        let meta = self.inspect(bytes)?;
        if meta.already_applied() {
            return Ok(RemoteImport::AlreadyHave);
        }
        if meta.spans_trimmed_history() {
            return Ok(RemoteImport::RefusedTrimmedHistory);
        }
        Ok(match self.import(bytes)? {
            ImportOutcome::Applied => RemoteImport::Applied,
            ImportOutcome::Parked => RemoteImport::Parked,
        })
    }

    /// Encode what a replica at `from` is missing.
    ///
    /// `from` is a token that replica produced with [`DataDoc::version`].
    /// A token this document cannot decode is treated as corrupt rather than
    /// as "send everything": on the sync path the token is remote input, and
    /// answering a malformed one with the whole document would make a garbled
    /// byte an amplification lever.
    pub fn export_since(&self, from: &VersionToken) -> DataResult<CatchUp> {
        self.check_live()?;
        let from_vv = VersionVector::decode(from.as_bytes())
            .map_err(|err| DataError::Corrupt(err.to_string()))?;

        if from_vv.includes_vv(&self.inner.oplog_vv()) {
            return Ok(CatchUp::UpToDate);
        }

        // The mirror of the refusal in `import_remote`: if this document's
        // history no longer reaches back to where the peer is, no run of
        // updates can express the gap, and asking the engine for one is the
        // same defect from the sending side.
        let shallow_since = self.inner.shallow_since_vv();
        if !shallow_since.is_empty() && !from_vv.includes_vv(&shallow_since.to_vv()) {
            return Ok(CatchUp::NeedsSnapshot);
        }

        self.inner
            .export(ExportMode::updates(&from_vv))
            .map(CatchUp::Updates)
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// Declare everything currently in the document as already persisted.
    ///
    /// Called after loading a snapshot and its delta log, so the next commit
    /// exports only genuinely new changes instead of re-emitting the history
    /// that was just read off disk.
    pub fn mark_persisted(&mut self) {
        self.exported = self.inner.oplog_vv();
    }

    /// Commit pending edits and return the change they produced.
    ///
    /// `Ok(None)` means nothing had changed. The returned delta is exactly
    /// the bytes needed to bring a replica at the previous version up to the
    /// current one, which is what the delta log stores and what F3 will send.
    pub fn commit(&mut self) -> DataResult<Option<Delta>> {
        self.check_live()?;
        self.inner.commit();

        let current = self.inner.oplog_vv();
        if current == self.exported {
            return Ok(None);
        }

        let bytes = self
            .inner
            .export(ExportMode::updates(&self.exported))
            .map_err(|err| DataError::Engine(err.to_string()))?;

        self.previous_exported = Some(std::mem::replace(&mut self.exported, current));
        self.commits_since_compaction = self.commits_since_compaction.saturating_add(1);
        self.delta_bytes_since_measure = self.delta_bytes_since_measure.saturating_add(bytes.len());

        Ok(Some(Delta {
            bytes,
            version: self.version(),
        }))
    }

    /// Undo the bookkeeping of the last commit, so its change is handed out
    /// again by the next one.
    ///
    /// For the caller whose write failed. The document keeps the edits (they
    /// happened, and the application is looking at them); what is undone is
    /// only the claim that someone has taken responsibility for persisting
    /// them. Without this a failed write loses the change at the next
    /// restart, silently, which is the worst shape a storage error can take.
    ///
    /// Safe to call spuriously: re-exporting a delta that did reach storage
    /// costs one duplicate record, and duplicates are free by construction.
    pub fn rewind_last_commit(&mut self) {
        if let Some(previous) = self.previous_exported.take() {
            self.exported = previous;
            self.commits_since_compaction = self.commits_since_compaction.saturating_sub(1);
        }
    }

    /// The current version of the document.
    pub fn version(&self) -> VersionToken {
        VersionToken(self.inner.oplog_vv().encode())
    }

    /// Encode the document compactly, discarding history.
    ///
    /// This is what a document record stores: a 10,000 operation document
    /// compacts to under a kilobyte here, against tens of kilobytes of full
    /// history, which is what keeps a document inside one sealed record and
    /// a catch-up inside one message.
    pub fn export_compacted(&self) -> DataResult<Vec<u8>> {
        self.check_live()?;
        let frontiers = self.inner.oplog_frontiers();
        self.inner
            .export(ExportMode::shallow_snapshot(&frontiers))
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// Encode the document with its full history.
    ///
    /// Part of the escape hatch: an application can always take this and
    /// leave. Prefer [`DataDoc::export_compacted`] for storage and transfer.
    pub fn export_raw(&self) -> DataResult<Vec<u8>> {
        self.check_live()?;
        self.inner
            .export(ExportMode::Snapshot)
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// The document's current state as plain JSON.
    ///
    /// The second half of the escape hatch, and the one that does not
    /// require the reader to know anything about this SDK at all.
    pub fn export_json(&self) -> DataResult<String> {
        self.check_live()?;
        let value = self.inner.get_deep_value();
        serde_json::to_string(&value).map_err(|err| DataError::Engine(err.to_string()))
    }

    /// Measure the compacted encoding and refresh the size bookkeeping.
    pub fn measure(&mut self) -> DataResult<usize> {
        let bytes = self.export_compacted()?.len();
        self.last_measured_bytes = bytes;
        self.delta_bytes_since_measure = 0;
        Ok(bytes)
    }

    /// Check the document against the size cap, measuring only when the
    /// running estimate says a breach is possible.
    ///
    /// Returns the verdict and, when a measurement happened, the true size.
    pub fn check_size(&mut self) -> DataResult<(policy::SizeVerdict, Option<usize>)> {
        if !policy::needs_size_check(self.last_measured_bytes, self.delta_bytes_since_measure) {
            return Ok((policy::SizeVerdict::Ok, None));
        }
        let bytes = self.measure()?;
        Ok((policy::size_verdict(bytes), Some(bytes)))
    }

    /// Statistics for compaction and cap decisions.
    pub fn stats(&mut self) -> DataResult<DocStats> {
        let compacted_bytes = self.measure()?;
        Ok(DocStats {
            compacted_bytes,
            commits_since_compaction: self.commits_since_compaction,
        })
    }

    /// Commits recorded since the last compaction.
    pub fn commits_since_compaction(&self) -> u32 {
        self.commits_since_compaction
    }

    /// Reset the compaction counter, after the owner has written a fresh
    /// compacted record and dropped the folded delta records.
    pub fn mark_compacted(&mut self, compacted_bytes: usize) {
        self.commits_since_compaction = 0;
        self.last_measured_bytes = compacted_bytes;
        self.delta_bytes_since_measure = 0;
    }

    /// Adopt the bookkeeping of a document just loaded from storage.
    ///
    /// Without this a reopened document starts as though it were empty, and
    /// the cheap size estimate then under-reports by the entire size of what
    /// was loaded: a document already near the cap would look far from it
    /// and sail past without ever triggering a real measurement. Passing the
    /// record sizes in keeps the estimate an over-estimate, which is the
    /// property the whole check depends on.
    ///
    /// `compacted_bytes` is the size of the stored snapshot and `log_bytes`
    /// the total size of the delta records replayed on top of it.
    pub fn restore_bookkeeping(&mut self, compacted_bytes: usize, log_bytes: usize, commits: u32) {
        self.last_measured_bytes = compacted_bytes;
        self.delta_bytes_since_measure = log_bytes;
        self.commits_since_compaction = commits;
    }

    // ---- collections -------------------------------------------------

    /// Set a key in a map collection.
    pub fn map_set(&mut self, collection: &str, key: &str, value: DataValue) -> DataResult<()> {
        self.check_live()?;
        crate::validate_name(collection)?;
        crate::validate_key(key)?;
        crate::validate_value(&value)?;
        let map = self.inner.get_map(collection);
        map.insert(key, to_engine_value(value))
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// Remove a key from a map collection.
    pub fn map_delete(&mut self, collection: &str, key: &str) -> DataResult<()> {
        self.check_live()?;
        crate::validate_name(collection)?;
        let map = self.inner.get_map(collection);
        map.delete(key)
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// Read a key from a map collection.
    pub fn map_get(&self, collection: &str, key: &str) -> DataResult<Option<DataValue>> {
        self.check_live()?;
        crate::validate_name(collection)?;
        let map = self.inner.get_map(collection);
        Ok(map.get(key).and_then(|entry| match entry {
            loro::ValueOrContainer::Value(value) => from_engine_value(&value),
            loro::ValueOrContainer::Container(_) => None,
        }))
    }

    /// Append to a list collection.
    pub fn list_push(&mut self, collection: &str, value: DataValue) -> DataResult<()> {
        self.check_live()?;
        crate::validate_name(collection)?;
        crate::validate_value(&value)?;
        let list = self.inner.get_list(collection);
        list.push(to_engine_value(value))
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// Insert into a list collection at `index`.
    pub fn list_insert(
        &mut self,
        collection: &str,
        index: usize,
        value: DataValue,
    ) -> DataResult<()> {
        self.check_live()?;
        crate::validate_name(collection)?;
        crate::validate_value(&value)?;
        let list = self.inner.get_list(collection);
        let length = list.len();
        if index > length {
            return Err(DataError::OutOfRange {
                position: index,
                length,
            });
        }
        list.insert(index, to_engine_value(value))
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// Delete `count` entries from a list collection at `index`.
    pub fn list_delete(&mut self, collection: &str, index: usize, count: usize) -> DataResult<()> {
        self.check_live()?;
        crate::validate_name(collection)?;
        let list = self.inner.get_list(collection);
        let length = list.len();
        if index.saturating_add(count) > length {
            return Err(DataError::OutOfRange {
                position: index.saturating_add(count),
                length,
            });
        }
        list.delete(index, count)
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// Number of entries in a list collection.
    pub fn list_len(&self, collection: &str) -> DataResult<usize> {
        self.check_live()?;
        crate::validate_name(collection)?;
        Ok(self.inner.get_list(collection).len())
    }

    /// Insert text into a text collection at a character position.
    pub fn text_insert(&mut self, collection: &str, position: usize, text: &str) -> DataResult<()> {
        self.check_live()?;
        crate::validate_name(collection)?;
        crate::validate_value_len(text.len())?;
        let handle = self.inner.get_text(collection);
        let length = handle.len_unicode();
        if position > length {
            return Err(DataError::OutOfRange { position, length });
        }
        handle
            .insert(position, text)
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// Delete `count` characters from a text collection at `position`.
    pub fn text_delete(
        &mut self,
        collection: &str,
        position: usize,
        count: usize,
    ) -> DataResult<()> {
        self.check_live()?;
        crate::validate_name(collection)?;
        let handle = self.inner.get_text(collection);
        let length = handle.len_unicode();
        if position.saturating_add(count) > length {
            return Err(DataError::OutOfRange {
                position: position.saturating_add(count),
                length,
            });
        }
        handle
            .delete(position, count)
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// The current contents of a text collection.
    pub fn text_value(&self, collection: &str) -> DataResult<String> {
        self.check_live()?;
        crate::validate_name(collection)?;
        Ok(self.inner.get_text(collection).to_string())
    }

    /// Add to a counter collection. Negative values subtract.
    pub fn counter_increment(&mut self, collection: &str, amount: f64) -> DataResult<()> {
        self.check_live()?;
        crate::validate_name(collection)?;
        self.inner
            .get_counter(collection)
            .increment(amount)
            .map_err(|err| DataError::Engine(err.to_string()))
    }

    /// The current value of a counter collection.
    pub fn counter_value(&self, collection: &str) -> DataResult<f64> {
        self.check_live()?;
        crate::validate_name(collection)?;
        Ok(self.inner.get_counter(collection).get_value())
    }
}

impl std::fmt::Debug for DataDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataDoc")
            .field("poisoned", &self.poisoned)
            .field("commits_since_compaction", &self.commits_since_compaction)
            .finish_non_exhaustive()
    }
}

/// Field names of an attachment reference inside the engine.
///
/// Deliberately the same spelling as the serde representation and the JSON
/// that crosses the FFI, so a document exported with [`DataDoc::export_json`]
/// describes its own attachments in the same words the API uses. The export
/// is the escape hatch for an application leaving this SDK, and an escape
/// hatch that needs a decoder ring is not one.
const ATTACHMENT_TAG_KEY: &str = "kind";
const ATTACHMENT_TAG: &str = "attachment";
const ATTACHMENT_HASH_KEY: &str = "hash";
const ATTACHMENT_SIZE_KEY: &str = "size";
const ATTACHMENT_NAME_KEY: &str = "name";
const ATTACHMENT_MIME_KEY: &str = "mime";

fn to_engine_value(value: DataValue) -> LoroValue {
    match value {
        DataValue::Null => LoroValue::Null,
        DataValue::Bool { value } => LoroValue::from(value),
        DataValue::Int { value } => LoroValue::from(value),
        DataValue::Float { value } => LoroValue::from(value),
        DataValue::Text { value } => LoroValue::from(value),
        DataValue::Bytes { value } => LoroValue::from(value),
        // A reference is one engine value, not a container, and that is the
        // whole of the "replaced, never edited" property: a container would
        // give two writers a way to merge half of one reference with half of
        // another and produce a hash that addresses nothing.
        DataValue::Attachment {
            hash,
            size,
            name,
            mime,
        } => {
            let mut fields: Vec<(String, LoroValue)> = vec![
                (
                    ATTACHMENT_TAG_KEY.to_string(),
                    LoroValue::from(ATTACHMENT_TAG),
                ),
                (ATTACHMENT_HASH_KEY.to_string(), LoroValue::from(hash)),
                (
                    ATTACHMENT_SIZE_KEY.to_string(),
                    // The engine has one integer type and it is signed.
                    // `validate_attachment` refuses anything past
                    // `MAX_ATTACHMENT_SIZE` at the operation that writes it,
                    // and every write path runs it, so the saturation here is
                    // a backstop rather than the bound: it exists because a
                    // value this deep has no way to report a refusal, not
                    // because the case is expected.
                    LoroValue::from(i64::try_from(size).unwrap_or(i64::MAX)),
                ),
            ];
            if let Some(name) = name {
                fields.push((ATTACHMENT_NAME_KEY.to_string(), LoroValue::from(name)));
            }
            if let Some(mime) = mime {
                fields.push((ATTACHMENT_MIME_KEY.to_string(), LoroValue::from(mime)));
            }
            LoroValue::Map(fields.into())
        }
    }
}

fn from_engine_value(value: &LoroValue) -> Option<DataValue> {
    match value {
        LoroValue::Null => Some(DataValue::Null),
        LoroValue::Bool(inner) => Some(DataValue::bool(*inner)),
        LoroValue::I64(inner) => Some(DataValue::int(*inner)),
        LoroValue::Double(inner) => Some(DataValue::float(*inner)),
        LoroValue::String(inner) => Some(DataValue::text(inner.to_string())),
        LoroValue::Binary(inner) => Some(DataValue::bytes(inner.to_vec())),
        LoroValue::Map(fields) => {
            // Only a map carrying the attachment tag is a value this layer
            // knows. Anything else is read as absent, which is exactly what a
            // build predating attachments does with this same map: it has no
            // arm for it and falls through to the wildcard below. That is the
            // compatibility story, and it is worth stating because it is the
            // reason a reference is a map rather than an encoded string. A
            // string would replicate to an older build as text and be shown
            // to a person as a hash.
            match fields.get(ATTACHMENT_TAG_KEY) {
                Some(LoroValue::String(tag)) if tag.as_str() == ATTACHMENT_TAG => {}
                _ => return None,
            }
            let hash = match fields.get(ATTACHMENT_HASH_KEY) {
                Some(LoroValue::String(hash)) => hash.to_string(),
                _ => return None,
            };
            let size = match fields.get(ATTACHMENT_SIZE_KEY) {
                Some(LoroValue::I64(size)) if *size >= 0 => *size as u64,
                _ => return None,
            };
            // Present-but-wrong is refused, absent is fine. These two are
            // optional, so a missing key is an ordinary reference with no
            // display text; a key holding something that is not text is a
            // reference that disagrees with this version's shape, and
            // reading it as though the field were simply missing would have
            // two members render one delta differently. Absent is the one
            // answer every reader already agrees on.
            let text_field = |key: &str| match fields.get(key) {
                Some(LoroValue::String(value)) => Ok(Some(value.to_string())),
                None => Ok(None),
                Some(_) => Err(()),
            };
            let (Ok(name), Ok(mime)) = (
                text_field(ATTACHMENT_NAME_KEY),
                text_field(ATTACHMENT_MIME_KEY),
            ) else {
                return None;
            };
            // Checked on the way out, not only on the way in. A local write
            // is validated at the operation, but a reference can also arrive
            // inside a peer's delta, and nothing on that path has looked at
            // it: the engine merges bytes, it does not know what an
            // attachment is. A malformed reference reads as absent, which is
            // what an implementation without attachments does with it
            // anyway.
            if crate::validate_attachment(&hash, size, name.as_deref(), mime.as_deref()).is_err() {
                return None;
            }
            Some(DataValue::Attachment {
                hash,
                size,
                name,
                mime,
            })
        }
        // Nested containers are not part of the v1 value model: a value that
        // is neither a scalar nor a reference reads as absent rather than as
        // a lossy conversion.
        _ => None,
    }
}

#[cfg(test)]
mod engine_value_tests {
    use super::*;

    /// Stand in for a build that predates attachments.
    ///
    /// Such a build has no `LoroValue::Map` arm at all, so every map falls
    /// through to the wildcard. This asserts the same outcome from the other
    /// direction: a map this build does not recognise reads as absent. The
    /// two are the same path, and it is the path that makes an attachment
    /// safe to write into a space whose other members are on an older
    /// release. Had a reference been an encoded string instead of a map,
    /// those members would read a hash as text and show it to a person.
    const TEST_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    /// A map that is a complete attachment apart from its tag.
    ///
    /// Every field the parse needs is present and well formed, so the only
    /// thing standing between this value and being read as an attachment is
    /// the tag gate. That is deliberate: cases missing a hash are refused by
    /// the hash check no matter what the gate does, and a test built from
    /// those passes with the gate deleted.
    fn tagged(tag: Option<&str>) -> LoroValue {
        let mut fields = vec![
            (ATTACHMENT_HASH_KEY.to_string(), LoroValue::from(TEST_HASH)),
            (ATTACHMENT_SIZE_KEY.to_string(), LoroValue::from(4096i64)),
        ];
        if let Some(tag) = tag {
            fields.push((ATTACHMENT_TAG_KEY.to_string(), LoroValue::from(tag)));
        }
        LoroValue::Map(fields.into())
    }

    #[test]
    fn a_map_without_the_attachment_tag_reads_as_absent() {
        let cases = [
            // No tag at all, which is what a nested container's deep value
            // looks like.
            tagged(None),
            // A tag naming something this build does not know, which is what
            // a future reference kind looks like arriving here.
            tagged(Some("something-later")),
            // Close enough to be a typo, far enough to be a different value.
            tagged(Some("Attachment")),
        ];
        for case in cases {
            assert_eq!(
                from_engine_value(&case),
                None,
                "{case:?} must read as absent"
            );
        }
        // The control: the same map, correctly tagged, IS read. Without this
        // the test above passes for a parser that reads nothing at all.
        assert!(matches!(
            from_engine_value(&tagged(Some(ATTACHMENT_TAG))),
            Some(DataValue::Attachment { .. })
        ));
    }

    #[test]
    fn an_attachment_without_an_address_reads_as_absent() {
        // Separate from the tag gate on purpose, so each refusal is pinned
        // by a case that isolates it. Everything but the hash is present
        // and well formed, for the same reason `tagged` carries a hash:
        // a fixture missing two things is refused by whichever check runs
        // first and says nothing about the other.
        let no_hash = LoroValue::Map(
            vec![
                (
                    ATTACHMENT_TAG_KEY.to_string(),
                    LoroValue::from(ATTACHMENT_TAG),
                ),
                (ATTACHMENT_SIZE_KEY.to_string(), LoroValue::from(4096i64)),
            ]
            .into(),
        );
        assert_eq!(from_engine_value(&no_hash), None);
    }

    #[test]
    fn a_display_field_of_the_wrong_type_makes_the_reference_absent() {
        // Present-but-wrong is not absent. A key holding something that is
        // not text is a reference disagreeing with this version's shape,
        // and reading it as though the key were merely missing would leave
        // two members rendering one delta differently: one an attachment
        // with no name, the other whatever its own reader made of it.
        // Absent is the one answer every reader already agrees on, and it
        // is what an unknown value kind gets.
        for wrong in [ATTACHMENT_NAME_KEY, ATTACHMENT_MIME_KEY] {
            let value = LoroValue::Map(
                vec![
                    (
                        ATTACHMENT_TAG_KEY.to_string(),
                        LoroValue::from(ATTACHMENT_TAG),
                    ),
                    (ATTACHMENT_HASH_KEY.to_string(), LoroValue::from(TEST_HASH)),
                    (ATTACHMENT_SIZE_KEY.to_string(), LoroValue::from(4096i64)),
                    (wrong.to_string(), LoroValue::from(5i64)),
                ]
                .into(),
            );
            assert_eq!(
                from_engine_value(&value),
                None,
                "a {wrong} field of the wrong type must make the whole reference absent"
            );
        }

        // The control: the same map with that field spelled as text IS
        // read, so what the cases above pin is the type and not the key.
        let good = LoroValue::Map(
            vec![
                (
                    ATTACHMENT_TAG_KEY.to_string(),
                    LoroValue::from(ATTACHMENT_TAG),
                ),
                (ATTACHMENT_HASH_KEY.to_string(), LoroValue::from(TEST_HASH)),
                (ATTACHMENT_SIZE_KEY.to_string(), LoroValue::from(4096i64)),
                (ATTACHMENT_NAME_KEY.to_string(), LoroValue::from("plan.pdf")),
            ]
            .into(),
        );
        assert!(matches!(
            from_engine_value(&good),
            Some(DataValue::Attachment { .. })
        ));
    }

    #[test]
    fn a_field_this_version_does_not_know_is_ignored() {
        // The other half of the same rule, and the one that keeps a later
        // version able to add an optional field without a flag day. A
        // change that alters what a reference MEANS cannot be a new field
        // for exactly this reason: it would be silently ignored here.
        let mut fields = vec![
            (
                ATTACHMENT_TAG_KEY.to_string(),
                LoroValue::from(ATTACHMENT_TAG),
            ),
            (ATTACHMENT_HASH_KEY.to_string(), LoroValue::from(TEST_HASH)),
            (ATTACHMENT_SIZE_KEY.to_string(), LoroValue::from(4096i64)),
        ];
        fields.push(("something_later".to_string(), LoroValue::from("v2 field")));
        assert!(matches!(
            from_engine_value(&LoroValue::Map(fields.into())),
            Some(DataValue::Attachment { .. })
        ));
    }

    #[test]
    fn an_attachment_survives_the_engine_conversion_both_ways() {
        let reference = DataValue::Attachment {
            hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
            size: 4096,
            name: Some("plan.pdf".to_string()),
            mime: Some("application/pdf".to_string()),
        };
        let engine = to_engine_value(reference.clone());
        assert_eq!(from_engine_value(&engine), Some(reference));
    }

    #[test]
    fn a_negative_size_is_refused_rather_than_wrapped() {
        // The engine's integer is signed and this crate's is not. A value
        // that could only arrive from a corrupted or hostile encoding reads
        // as absent rather than as a size near u64::MAX, which is what a
        // cast would produce and what a fetch would then try to allocate.
        let engine = LoroValue::Map(
            vec![
                (
                    ATTACHMENT_TAG_KEY.to_string(),
                    LoroValue::from(ATTACHMENT_TAG),
                ),
                (
                    ATTACHMENT_HASH_KEY.to_string(),
                    LoroValue::from(
                        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    ),
                ),
                (ATTACHMENT_SIZE_KEY.to_string(), LoroValue::from(-1i64)),
            ]
            .into(),
        );
        assert_eq!(from_engine_value(&engine), None);
    }
}
