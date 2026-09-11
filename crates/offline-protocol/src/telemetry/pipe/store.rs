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
//! `telemetry_batch_index` holding the next sequence and the FIFO order; one
//! owner record under `telemetry_batch_owner` naming the store that writes
//! the other two. A batch is written before the index names it, so a crash
//! between the two leaves an orphan the next load sweeps rather than an index
//! entry with no record. The earlier client kept the whole queue in one value;
//! through a sealed record with a 4 MiB cap that is one write away from the
//! ceiling, which is why each batch is its own record here.
//!
//! Every record is sealed with the install's state record key before it is
//! handed to the provider, so no plaintext batch ever sits in the app
//! container.
//!
//! # One writer per queue
//!
//! At most one store in the process writes a given queue. The index is
//! rewritten whole from the writer's memory, so a second writer erases what
//! the first recorded: an index written by a store that never saw a batch
//! leaves that batch unnamed, and the next load sweeps it as an orphan. A
//! second writer is the ordinary case, not the exotic one.
//! `TelemetryPipe::stop` detaches a worker it cannot join within its budget,
//! a pipe dropped on a teardown path does not wait at all, and either worker
//! goes on writing through the drain it was in and its final flush while
//! `enable_telemetry`, or a new engine over the same storage, starts the next
//! pipe.
//!
//! A store owns a queue by writing a fresh random token to the owner record
//! and registering that token in this process. Both rules below run under one
//! process-wide lock, so neither lands between the other's read and write:
//!
//! - **Adoption waits for the owner to let go.** A store offered a queue whose
//!   owner record names a token still registered here is refused
//!   ([`Attach::Busy`]) and offered it again later. An owner lets go in
//!   [`BatchStore::release`], which the pipe calls when its worker exits.
//! - **Every write re-reads the owner record.** A store writes only while that
//!   record names its own token. One that finds another token there, or none,
//!   has lost the queue and moves to memory rather than writing.
//!
//! The token lives on storage rather than in a pointer comparison because a
//! pointer does not survive a re-wrap: the FFI mints a new storage adapter on
//! every `initialize_mls`, so a torn-down engine's worker and its successor
//! hold different handles to the same records. The registry is process-local:
//! two processes sharing one container are not guarded against each other.
//!
//! # Bounds
//!
//! At most [`MAX_PENDING_BATCHES`] batches and [`MAX_PENDING_BYTES`] of
//! serialized bodies. Beyond either the oldest is dropped and counted, the
//! durable analogue of the ring's drop-oldest.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::protocol::state_crypto::StateRecordCipher;
use crate::protocol_state_storage::{ProtocolStateError, ProtocolStateStorage};

/// Key type of one queued batch.
pub(crate) const BATCH_KEY_TYPE: &str = "telemetry_batch";
/// Key type of the queue index.
pub(crate) const INDEX_KEY_TYPE: &str = "telemetry_batch_index";
/// Key id of the queue index.
pub(crate) const INDEX_KEY_ID: &str = "index";
/// Key type of the record naming the store that owns the queue.
pub(crate) const OWNER_KEY_TYPE: &str = "telemetry_batch_owner";
/// Key id of the owner record.
pub(crate) const OWNER_KEY_ID: &str = "owner";
/// Cap on queued batches.
pub(crate) const MAX_PENDING_BATCHES: usize = 64;
/// Cap on queued bytes, summed over serialized bodies.
pub(crate) const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;

/// The tokens of the queues that stores in this process own, and the lock
/// every ownership read-then-write is made under. See the module docs.
static OWNED_QUEUES: Mutex<Vec<Uuid>> = Mutex::new(Vec::new());

fn owned_queues() -> MutexGuard<'static, Vec<Uuid>> {
    OWNED_QUEUES.lock().unwrap_or_else(|e| e.into_inner())
}

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
    /// The app id the queued batches were cut under.
    ///
    /// The app id travels in a header, not in the batch body, so a queue
    /// adopted by a pipe configured for a different application would be
    /// uploaded under that application's key and counted against it. Stored
    /// here so the mismatch is visible before anything is sent. Optional so
    /// an index written before this field existed still parses; absent is
    /// read as "ours", which is what it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app_id: Option<String>,
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

/// What became of a backend offered to [`BatchStore::attach`].
#[derive(Debug)]
pub(crate) enum Attach {
    /// The store writes the queue there from now on.
    Adopted,
    /// Another store in this process owns the queue there. The backend is
    /// handed back to be offered again once that store has let go.
    Busy(Backend),
    /// Ownership could not be read or recorded. The backend is handed back to
    /// be offered again later.
    Failed(Backend),
}

/// The owner record as the provider returned it, before any key is tried.
enum OwnerRecord {
    Absent,
    Present(Vec<u8>),
    /// The read failed, which says nothing about who owns the queue.
    Unreadable,
}

/// One change to the durable queue, applied by [`BatchStore::write`].
enum Mutation<'a> {
    Store {
        key_type: &'static str,
        key_id: &'a str,
        plaintext: &'a [u8],
    },
    Delete {
        key_type: &'static str,
        key_id: &'a str,
    },
}

/// What became of a [`Mutation`].
enum Written {
    Done,
    /// Nothing was written because there is no durable queue to write to: the
    /// store is in memory, or has just found it lost the queue and moved there.
    Memory,
    /// The write, or the ownership read ahead of it, failed. The store keeps
    /// the queue, and the caller handles this as the failed write it is.
    Failed(String),
}

#[derive(Debug)]
pub(crate) struct BatchStore {
    backend: Backend,
    /// The token this store owns its queue under: `Some` exactly while the
    /// backend is sealed. See the module docs.
    owner: Option<Uuid>,
    pending: VecDeque<PendingBatch>,
    next_seq: u64,
    bytes: usize,
    /// Events lost to the caps.
    dropped_events: u64,
    /// Records that would not open or parse on load.
    corrupt_records: u64,
    /// Records discarded because they were cut for another app id.
    foreign_records: u64,
    index_dirty: bool,
    max_batches: usize,
    max_bytes: usize,
    /// The app id this store's batches are cut under; see [`Index::app_id`].
    app_id: String,
}

impl BatchStore {
    /// A store for `app_id`, offered `backend`. A backend it is refused is
    /// not kept, and the store runs in memory; the pipe offers backends
    /// through [`Self::attach`] instead, which hands a refused one back.
    pub(crate) fn new(backend: Backend, app_id: String) -> Self {
        let mut store = Self {
            backend: Backend::Memory,
            owner: None,
            pending: VecDeque::new(),
            next_seq: 0,
            bytes: 0,
            dropped_events: 0,
            corrupt_records: 0,
            foreign_records: 0,
            index_dirty: false,
            max_batches: MAX_PENDING_BATCHES,
            max_bytes: MAX_PENDING_BYTES,
            app_id,
        };
        let _ = store.attach(backend);
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

    /// Offers the store a backend.
    ///
    /// Adopting a queue loads what is persisted there ahead of what is
    /// pending in memory (it is older), then persists the in-memory batches.
    /// A queue another store in this process owns is refused, not shared; see
    /// the module docs.
    ///
    /// Offered the queue it already owns, the store keeps that queue as it
    /// stands. Reloading there would be actively wrong rather than merely
    /// wasteful: what is pending in memory is what is on disk, so the load
    /// would read those batches back and the in-memory copies would then be
    /// pushed as fresh records, queueing and persisting every pending batch
    /// twice and trimming the originals to the byte cap to make room for
    /// their own duplicates. Two engine paths attach (`enable_telemetry` and
    /// `initialize_mls`), so the ordinary logout-and-login sequence offers one
    /// queue twice, and the FFI wraps the host's provider afresh on every
    /// `initialize_mls`, so the second offer arrives through a different
    /// handle. The owner token recognizes the queue through either.
    pub(crate) fn attach(&mut self, backend: Backend) -> Attach {
        let Backend::Sealed { storage, cipher } = backend else {
            self.release();
            return Attach::Adopted;
        };
        let mut owned = owned_queues();
        let sealed = match read_owner(storage.as_ref()) {
            OwnerRecord::Unreadable => {
                return Attach::Failed(Backend::Sealed { storage, cipher });
            }
            OwnerRecord::Absent => None,
            OwnerRecord::Present(sealed) => Some(sealed),
        };
        let named = sealed
            .as_deref()
            .and_then(|sealed| open_owner(&cipher, sealed));

        if let (
            Some(token),
            Backend::Sealed {
                cipher: current, ..
            },
        ) = (self.owner, &self.backend)
        {
            let rekeyed = named != Some(token)
                && sealed
                    .as_deref()
                    .and_then(|sealed| open_owner(current, sealed))
                    == Some(token);
            if named == Some(token) || rekeyed {
                if rekeyed {
                    // A new record key for storage this store already writes:
                    // a re-`initialize_mls` that found the old key unusable.
                    // The owner record is resealed under it first, because
                    // every later write checks ownership with the new key and
                    // a record it cannot open reads as a queue lost.
                    if let Err(error) = store_owner(storage.as_ref(), &cipher, token) {
                        tracing::warn!(
                            target: "offline_protocol::telemetry::pipe",
                            error = %error,
                            "telemetry batch queue could not take the new record key; it keeps the old one until it can"
                        );
                        return Attach::Failed(Backend::Sealed { storage, cipher });
                    }
                }
                self.backend = Backend::Sealed { storage, cipher };
                return Attach::Adopted;
            }
        }

        if named.is_some_and(|theirs| owned.contains(&theirs)) {
            return Attach::Busy(Backend::Sealed { storage, cipher });
        }
        let token = Uuid::new_v4();
        if let Err(error) = store_owner(storage.as_ref(), &cipher, token) {
            tracing::warn!(
                target: "offline_protocol::telemetry::pipe",
                error = %error,
                "telemetry batch queue could not be claimed; batches stay in memory until it can"
            );
            return Attach::Failed(Backend::Sealed { storage, cipher });
        }
        if let Some(previous) = self.owner.replace(token) {
            owned.retain(|t| *t != previous);
        }
        owned.push(token);
        // Released before the load, whose deletes and pushes each take it.
        // Nothing can adopt or write this queue in between: its owner record
        // now names a token registered here.
        drop(owned);

        self.backend = Backend::Sealed { storage, cipher };
        let in_memory = std::mem::take(&mut self.pending);
        self.bytes = 0;
        self.load();
        // The batch id is stable across a round trip, so it is what decides
        // whether an in-memory batch is already on disk: a store re-adopting
        // a queue whose owner record was cleared reads back whatever of its
        // own was never deleted.
        let already: HashSet<Uuid> = self.pending.iter().map(|b| b.batch_id).collect();
        for mut batch in in_memory {
            if already.contains(&batch.batch_id) {
                continue;
            }
            batch.seq = None;
            self.push(batch);
        }
        self.flush_index();
        Attach::Adopted
    }

    /// Lets go of the queue so another store can adopt it, after one more
    /// attempt at an index write still owed. What is pending stays in memory.
    ///
    /// Nothing is deleted: the owner record goes on naming a token no store
    /// holds, which reads as unowned.
    pub(crate) fn release(&mut self) {
        self.flush_index();
        if let Some(token) = self.owner.take() {
            owned_queues().retain(|t| *t != token);
        }
        if self.is_durable() {
            self.backend = Backend::Memory;
            for batch in &mut self.pending {
                batch.seq = None;
            }
            self.next_seq = 0;
        }
    }

    /// Whether the queue is mirrored to storage.
    pub(crate) fn is_durable(&self) -> bool {
        matches!(self.backend, Backend::Sealed { .. })
    }

    fn load(&mut self) {
        let (storage, cipher) = match &self.backend {
            Backend::Sealed { storage, cipher } => (storage.clone(), cipher.clone()),
            Backend::Memory => return,
        };
        let index: Option<Index> = storage
            .load(INDEX_KEY_TYPE, INDEX_KEY_ID)
            .ok()
            .flatten()
            .and_then(|sealed| cipher.open(INDEX_KEY_TYPE, INDEX_KEY_ID, &sealed))
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        // Records to delete, collected here and deleted once the load has
        // decided, through the ownership check every write takes.
        let mut stale: Vec<String> = Vec::new();
        let mut foreign = 0u64;
        let (next_seq, order) = match index {
            // Cut for another application. The app id rides in a header, so
            // these bodies would go up under whichever key is configured now
            // and be counted against that app. The sequence counter is kept
            // so a stale record can never collide with a fresh one.
            Some(index) if index.app_id.as_deref().is_some_and(|id| id != self.app_id) => {
                foreign = index.order.len() as u64;
                stale.extend(index.order.iter().map(|seq| key_id(*seq)));
                (index.next_seq, Vec::new())
            }
            Some(index) => (index.next_seq, index.order),
            None => (0, Vec::new()),
        };
        let mut loaded = VecDeque::new();
        let mut bytes = 0usize;
        let mut corrupt = 0u64;
        for seq in &order {
            let key_id = key_id(*seq);
            let record = match storage.load(BATCH_KEY_TYPE, &key_id) {
                // Named by the index but already deleted. The index is written
                // once per drain rather than once per batch, so a crash
                // mid-drain leaves it naming batches that were accepted,
                // expired or trimmed before the crash. Each was counted when
                // it went, so none is a loss to report here.
                Ok(None) | Err(ProtocolStateError::NotFound(_)) => continue,
                Ok(Some(sealed)) => cipher
                    .open(BATCH_KEY_TYPE, &key_id, &sealed)
                    .and_then(|bytes| decode_record(*seq, &bytes)),
                Err(_) => None,
            };
            match record {
                Some(batch) => {
                    bytes += batch.bytes();
                    loaded.push_back(batch);
                }
                None => {
                    corrupt += 1;
                    stale.push(key_id);
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
                if !known && !stale.contains(&key) {
                    stale.push(key);
                }
            }
        }
        let highest = order.iter().copied().max().map_or(0, |s| s + 1);
        self.next_seq = next_seq.max(highest);
        self.pending = loaded;
        self.bytes = bytes;
        self.corrupt_records += corrupt;
        if corrupt > 0 {
            // Reported here rather than folded into `dropped_events`: how many
            // events each of these batches held is exactly what could not be
            // recovered, so adding a record count to an event count would sum
            // two units into the number that reconciles an invoice.
            tracing::warn!(
                target: "offline_protocol::telemetry::pipe",
                records = corrupt,
                "queued telemetry batches could not be opened on load and were discarded"
            );
        }
        self.foreign_records += foreign;
        if foreign > 0 {
            // Counted in records like `corrupt_records` and for the same
            // reason: the events these batches held are exactly what was not
            // read, so their count is not knowable in the unit
            // `dropped_events` reconciles.
            tracing::warn!(
                target: "offline_protocol::telemetry::pipe",
                records = foreign,
                "queued telemetry batches were cut under a different app id and were discarded"
            );
        }
        // `foreign` is a reason on its own: the index still names the app id
        // the batches were discarded for, and nothing else here would rewrite
        // it while the queue is empty.
        self.index_dirty = corrupt > 0 || foreign > 0 || order.len() != self.pending.len();
        for key_id in &stale {
            let deleted = self.write(Mutation::Delete {
                key_type: BATCH_KEY_TYPE,
                key_id,
            });
            if matches!(deleted, Written::Memory) {
                break;
            }
        }
        self.trim_to_caps();
    }

    /// Appends one batch, persisting it when the store is durable, and trims
    /// to the caps. The index is flushed by [`Self::flush_index`].
    pub(crate) fn push(&mut self, mut batch: PendingBatch) {
        if self.is_durable() {
            let seq = self.next_seq;
            let key_id = key_id(seq);
            let record = encode_record(&batch);
            let written = self.write(Mutation::Store {
                key_type: BATCH_KEY_TYPE,
                key_id: &key_id,
                plaintext: &record,
            });
            if !matches!(written, Written::Memory) {
                if let Written::Failed(error) = written {
                    tracing::warn!(
                        target: "offline_protocol::telemetry::pipe",
                        error = %error,
                        "telemetry batch could not be persisted; it stays in memory for this run"
                    );
                }
                self.next_seq += 1;
                batch.seq = Some(seq);
                self.index_dirty = true;
            }
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
        if !self.is_durable() {
            self.index_dirty = false;
            return;
        }
        let index = Index {
            next_seq: self.next_seq,
            order: self.pending.iter().filter_map(|b| b.seq).collect(),
            app_id: Some(self.app_id.clone()),
        };
        let Ok(bytes) = serde_json::to_vec(&index) else {
            return;
        };
        match self.write(Mutation::Store {
            key_type: INDEX_KEY_TYPE,
            key_id: INDEX_KEY_ID,
            plaintext: &bytes,
        }) {
            Written::Done | Written::Memory => self.index_dirty = false,
            Written::Failed(error) => tracing::warn!(
                target: "offline_protocol::telemetry::pipe",
                error = %error,
                "telemetry batch index could not be persisted"
            ),
        }
    }

    /// Applies `mutation` if, and only if, this store still owns the queue.
    ///
    /// The ownership read and the write both happen under the lock adoption
    /// takes, so no other store can adopt the queue between them.
    fn write(&mut self, mutation: Mutation<'_>) -> Written {
        let mut owned = owned_queues();
        let (Backend::Sealed { storage, cipher }, Some(token)) = (&self.backend, self.owner) else {
            return Written::Memory;
        };
        match read_owner(storage.as_ref()) {
            OwnerRecord::Present(sealed) if open_owner(cipher, &sealed) == Some(token) => {}
            OwnerRecord::Unreadable => {
                return Written::Failed("the queue's owner record could not be read".to_string());
            }
            OwnerRecord::Absent | OwnerRecord::Present(_) => {
                owned.retain(|t| *t != token);
                drop(owned);
                self.lose_queue();
                return Written::Memory;
            }
        }
        let result = match mutation {
            Mutation::Store {
                key_type,
                key_id,
                plaintext,
            } => {
                let Some(sealed) = cipher.seal(key_type, key_id, plaintext) else {
                    return Written::Failed("the record could not be sealed".to_string());
                };
                storage.store(key_type, key_id, &sealed)
            }
            Mutation::Delete { key_type, key_id } => storage.delete(key_type, key_id),
        };
        match result {
            Ok(()) => Written::Done,
            Err(error) => Written::Failed(error.to_string()),
        }
    }

    /// Moves to memory on finding the queue owned by another store, or its
    /// owner record gone. What is pending stays for this run; the records on
    /// storage belong to whoever owns the queue now, and this store does not
    /// touch them again.
    fn lose_queue(&mut self) {
        self.owner = None;
        self.backend = Backend::Memory;
        for batch in &mut self.pending {
            batch.seq = None;
        }
        self.next_seq = 0;
        self.index_dirty = false;
        tracing::warn!(
            target: "offline_protocol::telemetry::pipe",
            "the telemetry batch queue is owned by another pipe or was cleared; this pipe's batches stay in memory"
        );
    }

    fn delete_record(&mut self, batch: &PendingBatch) {
        let Some(seq) = batch.seq else {
            return;
        };
        match self.write(Mutation::Delete {
            key_type: BATCH_KEY_TYPE,
            key_id: &key_id(seq),
        }) {
            Written::Memory => {}
            Written::Done => self.index_dirty = true,
            Written::Failed(error) => {
                tracing::debug!(
                    target: "offline_protocol::telemetry::pipe",
                    error = %error,
                    "telemetry batch record could not be deleted"
                );
                self.index_dirty = true;
            }
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

    /// Events lost to the caps. Records that would not open on load are
    /// counted by [`Self::corrupt_records`] instead, in their own unit.
    pub(crate) fn dropped_events(&self) -> u64 {
        self.dropped_events
    }

    /// Records that would not open or parse on load. A count of records, not
    /// of the events they held, which is why it is never summed with
    /// [`Self::dropped_events`]; [`load`](Self::load) warns when it moves.
    #[cfg(test)]
    pub(crate) fn corrupt_records(&self) -> u64 {
        self.corrupt_records
    }

    /// Records discarded on load for naming another app id. In records for
    /// the same reason as [`Self::corrupt_records`]: nothing read them, so
    /// the events they held were never counted.
    #[cfg(test)]
    pub(crate) fn foreign_records(&self) -> u64 {
        self.foreign_records
    }
}

impl Drop for BatchStore {
    /// A store dropped without a [`BatchStore::release`] (an inline pipe, a
    /// test) must not leave its queue claimed for the rest of the process.
    /// Nothing is written here, for the reason given on `release`.
    fn drop(&mut self) {
        if let Some(token) = self.owner.take() {
            owned_queues().retain(|t| *t != token);
        }
    }
}

fn read_owner(storage: &dyn ProtocolStateStorage) -> OwnerRecord {
    match storage.load(OWNER_KEY_TYPE, OWNER_KEY_ID) {
        Ok(Some(sealed)) => OwnerRecord::Present(sealed),
        // A record the provider reports permanently unreadable names nobody.
        Ok(None) | Err(ProtocolStateError::NotFound(_) | ProtocolStateError::Corrupted(_)) => {
            OwnerRecord::Absent
        }
        Err(_) => OwnerRecord::Unreadable,
    }
}

fn open_owner(cipher: &StateRecordCipher, sealed: &[u8]) -> Option<Uuid> {
    cipher
        .open(OWNER_KEY_TYPE, OWNER_KEY_ID, sealed)
        .and_then(|bytes| Uuid::from_slice(&bytes).ok())
}

fn store_owner(
    storage: &dyn ProtocolStateStorage,
    cipher: &StateRecordCipher,
    token: Uuid,
) -> Result<(), String> {
    let sealed = cipher
        .seal(OWNER_KEY_TYPE, OWNER_KEY_ID, token.as_bytes())
        .ok_or_else(|| "the owner record could not be sealed".to_string())?;
    storage
        .store(OWNER_KEY_TYPE, OWNER_KEY_ID, &sealed)
        .map_err(|error| error.to_string())
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

    /// The app id every test store is built under.
    pub(crate) const TEST_APP_ID: &str = "app_test";

    /// A store for `TEST_APP_ID`, which is what all but the app-id tests want.
    pub(crate) fn test_store(backend: Backend) -> BatchStore {
        BatchStore::new(backend, TEST_APP_ID.to_string())
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
        let mut store = test_store(sealed(&storage));
        let first = batch(2, 1);
        let second = batch(3, 2);
        let ids = [first.batch_id, second.batch_id];
        store.push(first);
        store.push(second);
        store.flush_index();
        drop(store);

        let reloaded = test_store(sealed(&storage));
        let order: Vec<Uuid> = reloaded.pending.iter().map(|b| b.batch_id).collect();
        assert_eq!(order, ids);
        assert_eq!(reloaded.pending[0].created_ms, 1);
    }

    #[test]
    fn a_crash_between_batch_write_and_index_write_leaves_an_orphan_that_is_swept() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = test_store(sealed(&storage));
        store.push(batch(1, 1));
        store.flush_index();
        // Store calls so far: the owner record (1), a batch (2), the index
        // (3). The next batch's record write succeeds (call 4) and the index
        // write fails (call 5): the record is an orphan.
        storage.fail_store_at.store(5, Ordering::SeqCst);
        store.push(batch(1, 2));
        store.flush_index();
        assert_eq!(
            storage.records.lock().unwrap().len(),
            4,
            "two batches, the index and the owner record"
        );
        drop(store);

        let reloaded = test_store(sealed(&storage));
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
        let mut store = test_store(sealed(&storage));
        store.push(batch(1, 1));
        store.push(batch(1, 2));
        store.flush_index();
        drop(store);
        storage
            .records
            .lock()
            .unwrap()
            .insert((BATCH_KEY_TYPE.into(), key_id(0)), vec![1, 2, 3]);
        let reloaded = test_store(sealed(&storage));
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.corrupt_records(), 1);
    }

    /// A record the index names that is already gone left before the index
    /// caught up, and is not a loss.
    ///
    /// The index is written once per drain, so a crash between an accepted
    /// batch's delete and that write leaves the index naming the batch.
    /// Counting it with the records that would not open reported delivered
    /// telemetry as discarded.
    #[test]
    fn an_index_naming_an_already_deleted_batch_reports_no_loss() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = test_store(sealed(&storage));
        store.push(batch(1, 1));
        store.push(batch(1, 2));
        store.flush_index();
        // Accepted and deleted, and then the process dies before the drain's
        // index write.
        store.pop_front();
        drop(store);

        let reloaded = test_store(sealed(&storage));
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.front().map(|b| b.created_ms), Some(2));
        assert_eq!(
            reloaded.corrupt_records(),
            0,
            "a delivered batch is not a discarded one"
        );
    }

    #[test]
    fn the_caps_drop_oldest_first_and_count_events() {
        let mut store = test_store(Backend::Memory).with_caps(2, usize::MAX);
        store.push(batch(5, 1));
        store.push(batch(6, 2));
        store.push(batch(7, 3));
        assert_eq!(store.len(), 2);
        assert_eq!(store.dropped_events(), 5);
        assert_eq!(store.front().map(|b| b.created_ms), Some(2));

        let mut store = test_store(Backend::Memory).with_caps(usize::MAX, 400);
        store.push(batch(1, 1));
        store.push(batch(1, 2));
        assert!(store.bytes() <= 400);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn attaching_storage_persists_what_was_pending_in_memory_behind_what_was_stored() {
        let storage = Arc::new(MemoryStorage::default());
        let mut earlier = test_store(sealed(&storage));
        let stored = batch(1, 1);
        let stored_id = stored.batch_id;
        earlier.push(stored);
        earlier.flush_index();
        // An earlier run, gone by the time this one attaches.
        drop(earlier);

        let mut store = test_store(Backend::Memory);
        let in_memory = batch(2, 5);
        let in_memory_id = in_memory.batch_id;
        store.push(in_memory);
        assert!(!store.is_durable());
        assert!(matches!(store.attach(sealed(&storage)), Attach::Adopted));
        assert!(store.is_durable());
        let order: Vec<Uuid> = store.pending.iter().map(|b| b.batch_id).collect();
        assert_eq!(order, [stored_id, in_memory_id]);
        drop(store);

        let reloaded = test_store(sealed(&storage));
        assert_eq!(
            reloaded.len(),
            2,
            "the in-memory batch was persisted on attach"
        );
    }

    #[test]
    fn records_are_sealed_so_the_container_never_sees_a_plaintext_batch() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = test_store(sealed(&storage));
        store.push(batch(1, 1));
        store.flush_index();
        for bytes in storage.records.lock().unwrap().values() {
            assert!(StateRecordCipher::looks_sealed(bytes));
            assert!(!String::from_utf8_lossy(bytes).contains("batch_id"));
        }
    }

    /// Re-attaching the storage already in use is a no-op, not a reload.
    ///
    /// The engine attaches twice over one store in the ordinary order:
    /// `enable_telemetry` hands the backend over, and a later
    /// `initialize_mls` hands the same one over again. Reloading there would
    /// read the pending batches back off disk and then re-push the in-memory
    /// copies as fresh records, so every pending batch would be queued and
    /// persisted twice and the byte cap would trim the originals.
    #[test]
    fn re_attaching_the_same_storage_keeps_the_queue_as_it_is() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = test_store(sealed(&storage));
        store.push(batch(2, 1));
        store.push(batch(3, 2));
        store.flush_index();
        let ids: Vec<Uuid> = store.pending.iter().map(|b| b.batch_id).collect();
        let records = storage.records.lock().unwrap().len();

        assert!(matches!(store.attach(sealed(&storage)), Attach::Adopted));

        assert_eq!(store.len(), 2, "the queue was reloaded on top of itself");
        assert_eq!(
            store.pending.iter().map(|b| b.batch_id).collect::<Vec<_>>(),
            ids
        );
        assert_eq!(
            storage.records.lock().unwrap().len(),
            records,
            "no batch was written a second time"
        );
        drop(store);
        let reloaded = test_store(sealed(&storage));
        assert_eq!(reloaded.len(), 2);
    }

    /// A queue cut under one app id is never uploaded under another.
    ///
    /// The app id rides in a header, not in the batch body, so a batch
    /// adopted by a pipe configured for a different application would be
    /// counted against that application. Discarded on load instead, in
    /// records rather than events because nothing read them.
    #[test]
    fn a_queue_cut_under_another_app_id_is_discarded_on_load() {
        let storage = Arc::new(MemoryStorage::default());
        let mut theirs = BatchStore::new(sealed(&storage), "app_theirs".to_string());
        theirs.push(batch(4, 1));
        theirs.push(batch(5, 2));
        theirs.flush_index();
        drop(theirs);

        let mut ours = BatchStore::new(sealed(&storage), "app_ours".to_string());
        assert!(ours.is_empty(), "another app's queue must not be adopted");
        assert_eq!(ours.foreign_records(), 2);
        assert_eq!(
            ours.dropped_events(),
            0,
            "records, not events: nothing opened them to count"
        );
        assert!(
            !storage
                .records
                .lock()
                .unwrap()
                .keys()
                .any(|(t, _)| t == BATCH_KEY_TYPE),
            "the foreign records were deleted"
        );

        // The index is rewritten under our own id, so the next load adopts
        // what we queue from here on.
        ours.push(batch(1, 3));
        ours.flush_index();
        drop(ours);
        let reloaded = BatchStore::new(sealed(&storage), "app_ours".to_string());
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.foreign_records(), 0);
    }

    /// An index written before the app id was recorded is ours by default.
    #[test]
    fn an_index_without_an_app_id_is_adopted() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = test_store(sealed(&storage));
        store.push(batch(1, 1));
        store.flush_index();
        drop(store);
        // Rewrite the index in the shape that predates `Index::app_id`.
        let legacy = serde_json::to_vec(&serde_json::json!({
            "next_seq": 1,
            "order": [0],
        }))
        .expect("serializes");
        let sealed_index = cipher()
            .seal(INDEX_KEY_TYPE, INDEX_KEY_ID, &legacy)
            .expect("seals");
        storage
            .records
            .lock()
            .unwrap()
            .insert((INDEX_KEY_TYPE.into(), INDEX_KEY_ID.into()), sealed_index);

        let reloaded = test_store(sealed(&storage));
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.foreign_records(), 0);
    }

    /// A different `Arc` over the same provider must not duplicate the queue.
    ///
    /// A pointer comparison cannot see this case: the FFI wraps the host's
    /// provider in a fresh adapter on every `initialize_mls`, and a Rust
    /// embedder re-initialising over one store does the same. The owner token
    /// on storage recognizes it; without that the load reads every pending
    /// batch back and the in-memory copies are then persisted again beside
    /// them.
    #[test]
    fn re_attaching_an_equivalent_backend_does_not_duplicate_the_queue() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = test_store(sealed(&storage));
        store.push(batch(2, 1));
        store.push(batch(3, 2));
        store.flush_index();
        let ids: Vec<Uuid> = store.pending.iter().map(|b| b.batch_id).collect();
        let records = storage.records.lock().unwrap().len();

        // A distinct `Arc<dyn ProtocolStateStorage>` over the same provider,
        // which is what a re-wrap looks like from in here.
        let rewrapped: Arc<dyn ProtocolStateStorage> = Arc::new(SameStore {
            inner: storage.clone(),
        });
        assert!(matches!(
            store.attach(Backend::Sealed {
                storage: rewrapped,
                cipher: cipher(),
            }),
            Attach::Adopted
        ));

        assert_eq!(store.len(), 2, "the queue was duplicated");
        assert_eq!(
            store.pending.iter().map(|b| b.batch_id).collect::<Vec<_>>(),
            ids
        );
        assert_eq!(
            storage.records.lock().unwrap().len(),
            records,
            "no batch was written a second time"
        );
    }

    /// A queue another live store owns is refused, and adopted once that
    /// store lets go: the successor's in-memory batches then queue behind
    /// the owner's. The module docs name the loss sharing it caused.
    #[test]
    fn a_queue_another_store_owns_is_refused_until_that_store_lets_go() {
        let storage = Arc::new(MemoryStorage::default());
        let mut owner = test_store(sealed(&storage));
        let owned = batch(1, 1);
        let owned_id = owned.batch_id;
        owner.push(owned);
        owner.flush_index();

        let mut successor = test_store(Backend::Memory);
        let waiting = batch(2, 2);
        let waiting_id = waiting.batch_id;
        successor.push(waiting);
        // Through another handle, as a new engine over the same records holds.
        let rewrapped: Arc<dyn ProtocolStateStorage> = Arc::new(SameStore {
            inner: storage.clone(),
        });
        let offered = successor.attach(Backend::Sealed {
            storage: rewrapped,
            cipher: cipher(),
        });
        let Attach::Busy(backend) = offered else {
            panic!("a queue with a live owner was not refused: {offered:?}");
        };
        assert!(!successor.is_durable());

        owner.release();
        assert!(!owner.is_durable());
        assert!(matches!(successor.attach(backend), Attach::Adopted));
        let order: Vec<Uuid> = successor.pending.iter().map(|b| b.batch_id).collect();
        assert_eq!(
            order,
            [owned_id, waiting_id],
            "the owner's batch first, then what waited"
        );
    }

    /// A store that no longer finds its own token in the owner record writes
    /// nothing more, whatever it still holds.
    ///
    /// The registry sees only this process, and the record can change under
    /// a live writer: rewritten by an owner the registry cannot see, or
    /// cleared by a wipe while a torn-down engine's worker was still
    /// finishing. Either way the old writer has to stop, or its next index
    /// write erases the new owner's.
    #[test]
    fn a_store_that_no_longer_finds_its_token_writes_nothing_more() {
        for cleared in [false, true] {
            let storage = Arc::new(MemoryStorage::default());
            let mut store = test_store(sealed(&storage));
            store.push(batch(1, 1));
            store.flush_index();
            {
                let mut records = storage.records.lock().unwrap();
                let owner = (OWNER_KEY_TYPE.to_string(), OWNER_KEY_ID.to_string());
                if cleared {
                    records.remove(&owner);
                } else {
                    let other = cipher()
                        .seal(OWNER_KEY_TYPE, OWNER_KEY_ID, Uuid::new_v4().as_bytes())
                        .expect("seals");
                    records.insert(owner, other);
                }
            }
            let before = storage.records.lock().unwrap().clone();

            store.push(batch(1, 2));
            store.pop_front();
            store.flush_index();

            assert!(!store.is_durable(), "cleared: {cleared}");
            assert_eq!(
                store.len(),
                1,
                "what it held stays in memory (cleared: {cleared})"
            );
            assert_eq!(
                *storage.records.lock().unwrap(),
                before,
                "no record was written or deleted (cleared: {cleared})"
            );
        }
    }

    /// A new record key for storage the store already writes keeps the queue
    /// and its ownership.
    ///
    /// Every write checks ownership with the store's current key, so the
    /// owner record has to be resealed under a new one; left under the old
    /// key it would not open, and the store would read its own queue as lost.
    #[test]
    fn a_new_record_key_for_the_same_storage_keeps_the_queue_and_its_ownership() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = test_store(sealed(&storage));
        store.push(batch(1, 1));
        store.flush_index();

        let rekeyed = StateRecordCipher::new(&[4u8; 32]);
        assert!(matches!(
            store.attach(Backend::Sealed {
                storage: storage.clone(),
                cipher: rekeyed.clone(),
            }),
            Attach::Adopted
        ));
        store.push(batch(1, 2));
        store.flush_index();

        assert!(store.is_durable(), "the store read its own queue as lost");
        assert_eq!(store.len(), 2);
        let index = storage
            .load(INDEX_KEY_TYPE, INDEX_KEY_ID)
            .expect("reads")
            .expect("an index");
        assert!(
            rekeyed.open(INDEX_KEY_TYPE, INDEX_KEY_ID, &index).is_some(),
            "the index was written under the new key"
        );
    }

    /// A wrapper that forwards to another provider, so two distinct `Arc`s
    /// address the same records.
    pub(crate) struct SameStore {
        pub(crate) inner: Arc<MemoryStorage>,
    }

    impl ProtocolStateStorage for SameStore {
        fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
            self.inner.store(key_type, key_id, data)
        }
        fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
            self.inner.load(key_type, key_id)
        }
        fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
            self.inner.delete(key_type, key_id)
        }
        fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
            self.inner.list_keys(key_type)
        }
    }

    #[test]
    fn pop_front_deletes_the_record_and_the_index_follows() {
        let storage = Arc::new(MemoryStorage::default());
        let mut store = test_store(sealed(&storage));
        store.push(batch(1, 1));
        store.push(batch(1, 2));
        store.flush_index();
        store.pop_front();
        store.flush_index();
        drop(store);
        let reloaded = test_store(sealed(&storage));
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.front().map(|b| b.created_ms), Some(2));
    }
}
