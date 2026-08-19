//! The replicated document: collections, commits, and encoding.
//!
//! This module is the only place the CRDT engine is named. Nothing it
//! exports mentions an engine type, so the engine can be replaced without a
//! breaking change to any caller, the FFI surface, or any binding. That is
//! the property the whole data layer's no-lock-in promise rests on, and it
//! is why `loro` appears in no `pub` signature below.

use std::panic::{catch_unwind, AssertUnwindSafe};

use loro::{ExportMode, LoroDoc, LoroValue, VersionVector};

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
    pub fn import(&mut self, bytes: &[u8]) -> DataResult<()> {
        self.check_live()?;

        // Reject a blob the engine cannot even describe before asking it to
        // apply the contents. Checksum verification is on: it is cheap
        // relative to import and catches truncation that got past framing.
        if let Err(err) = LoroDoc::decode_import_blob_meta(bytes, true) {
            return Err(DataError::Corrupt(err.to_string()));
        }

        let outcome = catch_unwind(AssertUnwindSafe(|| self.inner.import(bytes)));
        match outcome {
            Ok(Ok(_)) => Ok(()),
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

fn to_engine_value(value: DataValue) -> LoroValue {
    match value {
        DataValue::Null => LoroValue::Null,
        DataValue::Bool { value } => LoroValue::from(value),
        DataValue::Int { value } => LoroValue::from(value),
        DataValue::Float { value } => LoroValue::from(value),
        DataValue::Text { value } => LoroValue::from(value),
        DataValue::Bytes { value } => LoroValue::from(value),
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
        // Nested containers are not part of the v1 value model: a value that
        // is not a scalar reads as absent rather than as a lossy conversion.
        _ => None,
    }
}
