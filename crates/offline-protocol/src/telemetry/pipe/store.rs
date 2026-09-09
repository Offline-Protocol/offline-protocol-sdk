//! The durable batch queue.
//!
//! Batches wait here between being cut and being acknowledged, in memory
//! always and on the protocol-state storage seam whenever one is attached.
//! On mobile the seam arrives with `initialize_mls`, so a pipe enabled
//! earlier runs in memory until then and persists whatever is pending the
//! moment storage attaches; a Rust or Python host that constructs a store
//! before enabling telemetry is durable from the first batch.
//!
//! # Layout
//!
//! One record per batch under key type `telemetry_batch`, key id a
//! zero-padded sequence number; one index record under
//! `telemetry_batch_index` holding the next sequence and the FIFO order. A
//! batch is written before the index names it, so a crash between the two
//! leaves an orphan the next load sweeps rather than an index entry with no
//! record. The earlier client kept the whole queue in one value; through a
//! sealed record with a 4 MiB cap that is one write away from the ceiling,
//! which is why each batch is its own record here.
//!
//! Every record is sealed with the install's state record key before it is
//! handed to the provider, so no plaintext batch ever sits in the app
//! container.
//!
//! # Bounds
//!
//! At most [`MAX_PENDING_BATCHES`] batches and [`MAX_PENDING_BYTES`] of
//! serialized bodies. Beyond either the oldest is dropped and counted, the
//! durable analogue of the ring's drop-oldest.

use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::protocol::state_crypto::StateRecordCipher;
use crate::protocol_state_storage::ProtocolStateStorage;

/// Key type of one queued batch.
pub(crate) const BATCH_KEY_TYPE: &str = "telemetry_batch";
/// Key type of the queue index.
pub(crate) const INDEX_KEY_TYPE: &str = "telemetry_batch_index";
/// Key id of the queue index.
pub(crate) const INDEX_KEY_ID: &str = "index";
/// Cap on queued batches.
pub(crate) const MAX_PENDING_BATCHES: usize = 64;
/// Cap on queued bytes, summed over serialized bodies.
pub(crate) const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;

/// One batch waiting to be sent.
#[derive(Debug, Clone)]
pub(crate) struct PendingBatch {
    /// Position in the durable queue; `None` while the store is in memory.
    pub(crate) seq: Option<u64>,
    pub(crate) batch_id: Uuid,
    /// When the batch was cut; drives the TTL, never sent.
    pub(crate) created_ms: i64,
    pub(crate) event_count: u32,
    /// The serialized `Batch`, which is the POST body.
    pub(crate) body: String,
}

impl PendingBatch {
    fn bytes(&self) -> usize {
        self.body.len()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Index {
    next_seq: u64,
    order: Vec<u64>,
}

/// Where the queue is mirrored.
pub(crate) enum Backend {
    Memory,
    Sealed {
        storage: Arc<dyn ProtocolStateStorage>,
        cipher: StateRecordCipher,
    },
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Memory => f.write_str("Memory"),
            Self::Sealed { .. } => f.write_str("Sealed"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct BatchStore {
    backend: Backend,
    pending: VecDeque<PendingBatch>,
    next_seq: u64,
    bytes: usize,
    /// Events lost to the caps.
    dropped_events: u64,
    /// Records that would not open or parse on load.
    corrupt_records: u64,
    index_dirty: bool,
    max_batches: usize,
    max_bytes: usize,
}

impl BatchStore {
    pub(crate) fn new(backend: Backend) -> Self {
        let mut store = Self {
            backend: Backend::Memory,
            pending: VecDeque::new(),
            next_seq: 0,
            bytes: 0,
            dropped_events: 0,
            corrupt_records: 0,
            index_dirty: false,
            max_batches: MAX_PENDING_BATCHES,
            max_bytes: MAX_PENDING_BYTES,
        };
        store.attach(backend);
        store
    }

    #[cfg(test)]
    pub(crate) fn with_caps(mut self, max_batches: usize, max_bytes: usize) -> Self {
        self.max_batches = max_batches;
        self.max_bytes = max_bytes;
        self.trim_to_caps();
        self.flush_index();
        self
    }

    /// Adopts a backend. Anything persisted there is loaded ahead of what is
    /// pending in memory (it is older), and the in-memory pending list is
    /// then persisted.
    pub(crate) fn attach(&mut self, backend: Backend) {
        self.backend = backend;
        if matches!(self.backend, Backend::Memory) {
            for batch in &mut self.pending {
                batch.seq = None;
            }
            self.next_seq = 0;
            return;
        }
        let in_memory = std::mem::take(&mut self.pending);
        self.bytes = 0;
        self.load();
        for mut batch in in_memory {
            batch.seq = None;
            self.push(batch);
        }
        self.flush_index();
    }

    /// Whether the queue is mirrored to storage.
    pub(crate) fn is_durable(&self) -> bool {
        matches!(self.backend, Backend::Sealed { .. })
    }

    fn load(&mut self) {
        let Backend::Sealed { storage, cipher } = &self.backend else {
            return;
        };
        let index: Option<Index> = storage
            .load(INDEX_KEY_TYPE, INDEX_KEY_ID)
            .ok()
            .flatten()
            .and_then(|sealed| cipher.open(INDEX_KEY_TYPE, INDEX_KEY_ID, &sealed))
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let (next_seq, order) = match index {
            Some(index) => (index.next_seq, index.order),
            None => (0, Vec::new()),
        };
        let mut loaded = VecDeque::new();
        let mut bytes = 0usize;
        let mut corrupt = 0u64;
        for seq in &order {
            let key_id = key_id(*seq);
            let record = storage
                .load(BATCH_KEY_TYPE, &key_id)
                .ok()
                .flatten()
                .and_then(|sealed| cipher.open(BATCH_KEY_TYPE, &key_id, &sealed))
                .and_then(|bytes| decode_record(*seq, &bytes));
            match record {
                Some(batch) => {
                    bytes += batch.bytes();
                    loaded.push_back(batch);
                }
                None => {
                    corrupt += 1;
                    let _ = storage.delete(BATCH_KEY_TYPE, &key_id);
                }
            }
        }
        // Orphans: a batch written before a crash that never reached the
        // index. Its sequence is unknown to the index, so it cannot be
        // ordered; it is swept rather than adopted.
        if let Ok(keys) = storage.list_keys(BATCH_KEY_TYPE) {
            for key in keys {
                let known = key
                    .parse::<u64>()
                    .ok()
                    .is_some_and(|seq| order.contains(&seq));
                if !known {
                    let _ = storage.delete(BATCH_KEY_TYPE, &key);
                }
            }
        }
        let highest = order.iter().copied().max().map_or(0, |s| s + 1);
        self.next_seq = next_seq.max(highest);
        self.pending = loaded;
        self.bytes = bytes;
        self.corrupt_records += corrupt;
        self.index_dirty = corrupt > 0 || order.len() != self.pending.len();
        self.trim_to_caps();
    }

    /// Appends one batch, persisting it when the store is durable, and trims
    /// to the caps. The index is flushed by [`Self::flush_index`].
    pub(crate) fn push(&mut self, mut batch: PendingBatch) {
        if let Backend::Sealed { storage, cipher } = &self.backend {
            let seq = self.next_seq;
            self.next_seq += 1;
            let key_id = key_id(seq);
            let bytes = encode_record(&batch);
            if let Some(sealed) = cipher.seal(BATCH_KEY_TYPE, &key_id, &bytes) {
                if let Err(err) = storage.store(BATCH_KEY_TYPE, &key_id, &sealed) {
                    tracing::warn!(
                        target: "offline_protocol::telemetry::pipe",
                        error = %err,
                        "telemetry batch could not be persisted; it stays in memory for this run"
                    );
                }
            }
            batch.seq = Some(seq);
            self.index_dirty = true;
        }
        self.bytes += batch.bytes();
        self.pending.push_back(batch);
        self.trim_to_caps();
    }

    /// The oldest pending batch.
    pub(crate) fn front(&self) -> Option<&PendingBatch> {
        self.pending.front()
    }

    /// Removes the oldest pending batch, deleting its record.
    pub(crate) fn pop_front(&mut self) -> Option<PendingBatch> {
        let batch = self.pending.pop_front()?;
        self.bytes -= batch.bytes();
        self.delete_record(&batch);
        Some(batch)
    }

    /// Writes the index if anything changed it since the last write.
    pub(crate) fn flush_index(&mut self) {
        if !self.index_dirty {
            return;
        }
        let Backend::Sealed { storage, cipher } = &self.backend else {
            self.index_dirty = false;
            return;
        };
        let index = Index {
            next_seq: self.next_seq,
            order: self.pending.iter().filter_map(|b| b.seq).collect(),
        };
        let Ok(bytes) = serde_json::to_vec(&index) else {
            return;
        };
        let Some(sealed) = cipher.seal(INDEX_KEY_TYPE, INDEX_KEY_ID, &bytes) else {
            return;
        };
        match storage.store(INDEX_KEY_TYPE, INDEX_KEY_ID, &sealed) {
            Ok(()) => self.index_dirty = false,
            Err(err) => tracing::warn!(
                target: "offline_protocol::telemetry::pipe",
                error = %err,
                "telemetry batch index could not be persisted"
            ),
        }
    }

    fn delete_record(&mut self, batch: &PendingBatch) {
        if let (Backend::Sealed { storage, .. }, Some(seq)) = (&self.backend, batch.seq) {
            if let Err(err) = storage.delete(BATCH_KEY_TYPE, &key_id(seq)) {
                tracing::debug!(
                    target: "offline_protocol::telemetry::pipe",
                    error = %err,
                    "telemetry batch record could not be deleted"
                );
            }
            self.index_dirty = true;
        }
    }

    fn trim_to_caps(&mut self) {
        while self.pending.len() > self.max_batches || self.bytes > self.max_bytes {
            let Some(dropped) = self.pop_front() else {
                break;
            };
            self.dropped_events += u64::from(dropped.event_count);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// Events lost to the caps plus records that would not open on load.
    pub(crate) fn dropped_events(&self) -> u64 {
        self.dropped_events
    }

    pub(crate) fn corrupt_records(&self) -> u64 {
        self.corrupt_records
    }
}

fn key_id(seq: u64) -> String {
    format!("{seq:012}")
}

/// Record framing: the creation time as eight little-endian bytes, then the
/// serialized batch body verbatim. The body is what goes on the wire, so it
/// is never re-encoded on the way in or out.
fn encode_record(batch: &PendingBatch) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + batch.body.len());
    bytes.extend_from_slice(&batch.created_ms.to_le_bytes());
    bytes.extend_from_slice(batch.body.as_bytes());
    bytes
}

fn decode_record(seq: u64, bytes: &[u8]) -> Option<PendingBatch> {
    let (header, body) = bytes.split_at_checked(8)?;
    let created_ms = i64::from_le_bytes(header.try_into().ok()?);
    let body = std::str::from_utf8(body).ok()?.to_string();
    // The body is parsed once on load to recover the id and count; a body
    // that does not parse as a batch is a record this SDK never wrote.
    let parsed: offline_protocol_telemetry_wire::Batch = serde_json::from_str(&body).ok()?;
    Some(PendingBatch {
        seq: Some(seq),
        batch_id: parsed.batch_id.0,
        created_ms,
        event_count: parsed.events.len().min(u32::MAX as usize) as u32,
        body,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::protocol_state_storage::{ProtocolStateError, ProtocolStateResult};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// An in-memory provider with a switch that fails the Nth store call, to
    /// simulate a crash between the batch write and the index write.
    #[derive(Default)]
    pub(crate) struct MemoryStorage {
        pub(crate) records: Mutex<HashMap<(String, String), Vec<u8>>>,
        pub(crate) stores: AtomicUsize,
        pub(crate) fail_store_at: AtomicUsize,
    }

    impl ProtocolStateStorage for MemoryStorage {
        fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
            let n = self.stores.fetch_add(1, Ordering::SeqCst) + 1;
            if n == self.fail_store_at.load(Ordering::SeqCst) {
                return Err(ProtocolStateError::StoreFailed("simulated crash".into()));
            }
            self.records
                .lock()
                .unwrap()
                .insert((key_type.into(), key_id.into()), data.to_vec());
            Ok(())
        }
        fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .get(&(key_type.into(), key_id.into()))
                .cloned())
        }
        fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
            self.records
                .lock()
                .unwrap()
                .remove(&(key_type.into(), key_id.into()));
            Ok(())
        }
        fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .keys()
                .filter(|(t, _)| t == key_type)
                .map(|(_, id)| id.clone())
                .collect())
        }
    }

    pub(crate) fn cipher() -> StateRecordCipher {
        StateRecordCipher::new(&[9u8; 32])
    }

    pub(crate) fn batch(n: u32, created_ms: i64) -> PendingBatch {
        let id = Uuid::new_v4();
        let body = serde_json::json!({
            "batch_id": id.to_string(),
            "session_id": Uuid::new_v4().to_string(),
            "wire_version": 1,
            "app_version": "1",
            "os": "ios",
            "os_major": 18,
            "sdk_version": "0.26.0",
            "events": (0..n).map(|i| serde_json::json!({"type": "protocol.message.relayed", "ts_ms": i, "data": {"hop_count": 1}})).collect::<Vec<_>>(),
        })
        .to_string();
        PendingBatch {
            seq: None,
            batch_id: id,
            created_ms,
            event_count: n,
            body,
        }
    }

    fn sealed(storage: &Arc<MemoryStorage>) -> Backend {
        Backend::Sealed {
            storage: storage.clone(),
            cipher: cipher(),
        }
    }

    #[test]
    fn a_durable_store_survives_a_reload_in_fifo_order() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = BatchStore::new(sealed(&storage));
        let first = batch(2, 1);
        let second = batch(3, 2);
        let ids = [first.batch_id, second.batch_id];
        store.push(first);
        store.push(second);
        store.flush_index();

        let reloaded = BatchStore::new(sealed(&storage));
        let order: Vec<Uuid> = reloaded.pending.iter().map(|b| b.batch_id).collect();
        assert_eq!(order, ids);
        assert_eq!(reloaded.pending[0].created_ms, 1);
    }

    #[test]
    fn a_crash_between_batch_write_and_index_write_leaves_an_orphan_that_is_swept() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = BatchStore::new(sealed(&storage));
        store.push(batch(1, 1));
        store.flush_index();
        // The next batch's record write succeeds (store call 3) and the index
        // write fails (call 4): the record is an orphan.
        storage.fail_store_at.store(4, Ordering::SeqCst);
        store.push(batch(1, 2));
        store.flush_index();
        assert_eq!(
            storage.records.lock().unwrap().len(),
            3,
            "two batches + index"
        );

        let reloaded = BatchStore::new(sealed(&storage));
        assert_eq!(reloaded.len(), 1, "only the indexed batch is adopted");
        assert_eq!(
            storage
                .records
                .lock()
                .unwrap()
                .keys()
                .filter(|(t, _)| t == BATCH_KEY_TYPE)
                .count(),
            1,
            "the orphan record was deleted"
        );
    }

    #[test]
    fn a_corrupt_record_is_dropped_and_counted() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = BatchStore::new(sealed(&storage));
        store.push(batch(1, 1));
        store.push(batch(1, 2));
        store.flush_index();
        storage
            .records
            .lock()
            .unwrap()
            .insert((BATCH_KEY_TYPE.into(), key_id(0)), vec![1, 2, 3]);
        let reloaded = BatchStore::new(sealed(&storage));
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.corrupt_records(), 1);
    }

    #[test]
    fn the_caps_drop_oldest_first_and_count_events() {
        let mut store = BatchStore::new(Backend::Memory).with_caps(2, usize::MAX);
        store.push(batch(5, 1));
        store.push(batch(6, 2));
        store.push(batch(7, 3));
        assert_eq!(store.len(), 2);
        assert_eq!(store.dropped_events(), 5);
        assert_eq!(store.front().map(|b| b.created_ms), Some(2));

        let mut store = BatchStore::new(Backend::Memory).with_caps(usize::MAX, 400);
        store.push(batch(1, 1));
        store.push(batch(1, 2));
        assert!(store.bytes() <= 400);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn attaching_storage_persists_what_was_pending_in_memory_behind_what_was_stored() {
        let storage = Arc::new(MemoryStorage::default());
        let mut earlier = BatchStore::new(sealed(&storage));
        let stored = batch(1, 1);
        let stored_id = stored.batch_id;
        earlier.push(stored);
        earlier.flush_index();

        let mut store = BatchStore::new(Backend::Memory);
        let in_memory = batch(2, 5);
        let in_memory_id = in_memory.batch_id;
        store.push(in_memory);
        assert!(!store.is_durable());
        store.attach(sealed(&storage));
        assert!(store.is_durable());
        let order: Vec<Uuid> = store.pending.iter().map(|b| b.batch_id).collect();
        assert_eq!(order, [stored_id, in_memory_id]);

        let reloaded = BatchStore::new(sealed(&storage));
        assert_eq!(
            reloaded.len(),
            2,
            "the in-memory batch was persisted on attach"
        );
    }

    #[test]
    fn records_are_sealed_so_the_container_never_sees_a_plaintext_batch() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = BatchStore::new(sealed(&storage));
        store.push(batch(1, 1));
        store.flush_index();
        for bytes in storage.records.lock().unwrap().values() {
            assert!(StateRecordCipher::looks_sealed(bytes));
            assert!(!String::from_utf8_lossy(bytes).contains("batch_id"));
        }
    }

    #[test]
    fn pop_front_deletes_the_record_and_the_index_follows() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = BatchStore::new(sealed(&storage));
        store.push(batch(1, 1));
        store.push(batch(1, 2));
        store.flush_index();
        store.pop_front();
        store.flush_index();
        let reloaded = BatchStore::new(sealed(&storage));
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.front().map(|b| b.created_ms), Some(2));
    }
}
