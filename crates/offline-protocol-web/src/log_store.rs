//! A durable store as a sealed append log over two files.
//!
//! The browser's durable storage is an origin-private file (OPFS) behind a
//! synchronous access handle, which the platform gives a dedicated worker
//! only (W1). Creating a file or a directory is asynchronous, so a store
//! cannot make a file per record the way the POSIX file stores do: every file
//! it will ever use is opened before the engine starts. This store therefore
//! keeps one account category (MLS material, or protocol state) in two files,
//! one live and one for compaction, and holds the current value of every key
//! in memory.
//!
//! # Invariants
//!
//! - **`Ok` means flushed.** A write appends one record and flushes the file
//!   before it returns. The engine writes state that depends on a returned
//!   `Ok` immediately (the protocol-state record key is the sharp case), so a
//!   write that is only staged is a data-loss bug.
//! - **Every record is sealed and bound to its place.** A record is
//!   ChaCha20-Poly1305 under a key derived from the store key for this
//!   category and this file's generation, and its associated data is the
//!   category, the generation and the record's sequence number. A record
//!   copied from the other category, from another generation, or to another
//!   position in the log does not authenticate. A key per generation also
//!   bounds how many random nonces one key ever seals under: a compaction
//!   starts the count again.
//! - **A wrong key is refused, never repaired.** The header carries a key
//!   check. A log whose header does not authenticate is refused at open,
//!   before a record is read, and nothing is truncated: the common cause is
//!   the wrong store key, and truncating on that would destroy the identity.
//! - **Only a torn tail is dropped.** Once the header authenticates, the
//!   first record that is short or does not authenticate ends the log, and
//!   the file is truncated there. A crash during a write leaves exactly that
//!   (a partial record after the last whole one), and the record it was
//!   writing was never acknowledged. A damaged record with an intact one
//!   anywhere after it is not a tear: cutting there would roll back
//!   acknowledged state (ratchet generations an AEAD nonce depends on), so
//!   the log is refused and left untouched, as for a wrong key. The search
//!   for that intact record does not trust the damaged record's length
//!   prefix, because a corrupted prefix is the damage that otherwise looks
//!   exactly like a tear. Damage to the very last record is still
//!   indistinguishable from a tear and is cut.
//! - **Compaction commits by its header.** It truncates the spare file, writes
//!   every live record into it, flushes, and only then writes and flushes the
//!   header that makes it valid, under the next generation. A crash at any
//!   point leaves either the old file as the only valid one, or both valid
//!   and the new one ahead, and open takes the highest valid generation.
//!   An I/O error once the header write has begun is different: the header
//!   may be on the disk, and if it is, the next open takes the new file and
//!   every later append to the old one is lost. So the spare is emptied
//!   again, and if even that fails, the log refuses every later write
//!   rather than acknowledge one that a restart would drop.
//! - **A failed compaction is not retried on every write.** Compacting seals
//!   every live record again, so retrying after each append would multiply
//!   the cost of every write while the cause lasts (a full disk). The next
//!   attempt waits until the log has grown by another compaction floor.
//!
//! What this does not protect against is in the threat model's browser entry:
//! the store key is reachable from the origin, so sealing keeps the records
//! from anything that reads the files alone (a copied profile directory, a
//! backup), not from a script running on the page's origin.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use offline_protocol::mls::{MlsStorage, StorageError};
use offline_protocol::{
    ProtocolStateError, ProtocolStateResult, ProtocolStateStorage,
    MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES,
};
use offline_protocol_mls::storage::StorageResult;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use tracing::warn;
use zeroize::Zeroizing;

/// Length of the store key the page supplies.
pub const STORE_KEY_BYTES: usize = 32;

/// Bytes before the first record: the header and its padding.
pub const HEADER_BYTES: u64 = 64;

const MAGIC: &[u8; 4] = b"OPWL";
const VERSION: u8 = 1;
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;
const HKDF_SALT: &[u8] = b"offline-protocol web log store v1";

/// Longest accepted `key_type` or `key_id`, in UTF-8 bytes, as the file
/// stores and the bindings' providers.
const MAX_COMPONENT_BYTES: usize = 4096;

/// Largest value accepted: the protocol-state ceiling, which the file stores
/// also apply to MLS material.
const MAX_VALUE_BYTES: usize = MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES;

const MAX_PLAIN_BYTES: usize =
    1 + 2 + MAX_COMPONENT_BYTES + 2 + MAX_COMPONENT_BYTES + MAX_VALUE_BYTES;
const MAX_SEALED_BYTES: usize = NONCE_BYTES + MAX_PLAIN_BYTES + TAG_BYTES;

/// A log shorter than this is never compacted.
const COMPACT_FLOOR: u64 = 1 << 20;

const PUT: u8 = 1;
const DELETE: u8 = 2;

/// The two categories a node keeps, each in its own pair of files under its
/// own derived key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Identity keys, group state and the protocol-state record key.
    Mls,
    /// Restartable delivery state.
    ProtocolState,
}

impl Category {
    fn domain(self) -> &'static [u8] {
        match self {
            Self::Mls => b"offline-protocol web log/mls",
            Self::ProtocolState => b"offline-protocol web log/protocol-state",
        }
    }
}

/// One file the log writes through. The browser's is an OPFS sync access
/// handle; the host tests use memory.
pub trait LogFile: Send {
    /// The file's length in bytes.
    fn size(&mut self) -> Result<u64, String>;
    /// Fills `buffer` from `at`, or fails. The caller never reads past the end.
    fn read_exact_at(&mut self, at: u64, buffer: &mut [u8]) -> Result<(), String>;
    /// Writes all of `data` at `at`.
    fn write_all_at(&mut self, at: u64, data: &[u8]) -> Result<(), String>;
    /// Sets the length to `len`.
    fn truncate(&mut self, len: u64) -> Result<(), String>;
    /// Makes every write so far durable.
    fn flush(&mut self) -> Result<(), String>;
}

/// Why a log could not be opened.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// A file holds a log, and no log authenticates under this key.
    WrongKey,
    /// A record inside the log is damaged and records after it are intact.
    Damaged,
    /// A file could not be read or written.
    Io(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongKey => f.write_str(
                "the stored log does not open under this store key; it was left untouched",
            ),
            Self::Damaged => f.write_str(
                "the stored log has a damaged record before intact ones; it was left \
                 untouched rather than cut back",
            ),
            Self::Io(detail) => write!(f, "the log could not be opened: {detail}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// What a log has cost, for a page that measures it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogStats {
    /// Flushes since open: one per acknowledged write, four per compaction.
    pub flushes: u64,
    /// Bytes in the live file.
    pub log_bytes: u64,
    /// Bytes the live records would take after a compaction.
    pub live_bytes: u64,
    /// Compactions since open.
    pub compactions: u64,
}

type Records = BTreeMap<(String, String), Vec<u8>>;

struct Log<F> {
    files: [F; 2],
    active: usize,
    generation: u64,
    /// The sequence number the next record is sealed under.
    seq: u64,
    /// Where the next record goes.
    end: u64,
    records: Records,
    live: u64,
    stats: LogStats,
    /// The active generation's cipher.
    cipher: ChaCha20Poly1305,
    store_key: Zeroizing<[u8; STORE_KEY_BYTES]>,
    category: Category,
    /// Set when a failed compaction could not be undone: every later write
    /// is refused, because the spare may win the next open.
    poisoned: Option<String>,
    /// The log length a compaction waits for after one failed.
    compact_after: u64,
}

/// One category's records, durable in two files. Implements both storage
/// traits; a node opens one per category, never one behind both, so the
/// categories of one cannot collide with the other's.
pub struct LogStore<F: LogFile> {
    log: Mutex<Log<F>>,
}

impl<F: LogFile> std::fmt::Debug for LogStore<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogStore").finish_non_exhaustive()
    }
}

enum HeaderState {
    /// No bytes at all.
    Empty,
    /// Zeros where the header goes (a compaction that never committed), or
    /// fewer bytes than a header (a crash during the first header write).
    /// Not a log, and not live.
    Uncommitted,
    Valid(u64),
    Invalid,
}

impl<F: LogFile> LogStore<F> {
    /// Opens `category`'s log over its two files, creating it if both are
    /// empty, and replays it. Refuses a log that holds records under another
    /// key, without changing either file.
    pub fn open(
        store_key: &[u8; STORE_KEY_BYTES],
        category: Category,
        first: F,
        second: F,
    ) -> Result<Self, OpenError> {
        let store_key = Zeroizing::new(*store_key);
        let cipher = derive(&store_key, category, 1).map_err(OpenError::Io)?;
        let mut log = Log {
            files: [first, second],
            active: 0,
            generation: 0,
            seq: 0,
            end: HEADER_BYTES,
            records: Records::new(),
            live: 0,
            stats: LogStats::default(),
            cipher,
            store_key,
            category,
            poisoned: None,
            compact_after: 0,
        };

        let states = [log.header_state(0)?, log.header_state(1)?];
        let best = (0..2)
            .filter_map(|i| match states[i] {
                HeaderState::Valid(generation) => Some((generation, i)),
                _ => None,
            })
            .max();
        match best {
            Some((generation, index)) => {
                log.active = index;
                log.generation = generation;
                log.cipher = log.cipher_for(generation).map_err(OpenError::Io)?;
                log.replay()?;
                // The other file is an older generation, a compaction that
                // never committed, or empty. None of it is live.
                if !matches!(states[1 - index], HeaderState::Empty) {
                    let other = &mut log.files[1 - index];
                    other.truncate(0).map_err(OpenError::Io)?;
                    other.flush().map_err(OpenError::Io)?;
                    log.stats.flushes += 1;
                }
            }
            None if states.iter().any(|s| matches!(s, HeaderState::Invalid)) => {
                return Err(OpenError::WrongKey);
            }
            None => {
                // Nothing committed yet. A first run, or a crash during the
                // very first write of the header.
                log.generation = 1;
                let spare = &mut log.files[1];
                spare.truncate(0).map_err(OpenError::Io)?;
                let header = log.header(&log.cipher, 1).map_err(OpenError::Io)?;
                let file = &mut log.files[0];
                file.truncate(0).map_err(OpenError::Io)?;
                file.write_all_at(0, &header).map_err(OpenError::Io)?;
                file.flush().map_err(OpenError::Io)?;
                log.stats.flushes += 1;
            }
        }
        log.stats.log_bytes = log.end;
        log.stats.live_bytes = log.live;
        Ok(Self {
            log: Mutex::new(log),
        })
    }

    /// What the log has cost since it opened.
    pub fn stats(&self) -> LogStats {
        self.lock().map(|log| log.stats).unwrap_or_default()
    }

    fn lock(&self) -> Result<MutexGuard<'_, Log<F>>, String> {
        // wasm32-unknown-unknown aborts on a panic, so a poisoned lock is
        // never observed there; on a host it means a test thread panicked
        // inside a write, which a flushed append cannot have torn.
        Ok(self
            .log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    fn put(&self, key_type: &str, key_id: &str, value: &[u8]) -> Result<(), String> {
        check_key(key_type, key_id)?;
        if value.len() > MAX_VALUE_BYTES {
            return Err(format!(
                "value of {} bytes is over the {MAX_VALUE_BYTES}-byte ceiling",
                value.len()
            ));
        }
        self.lock()?.append(PUT, key_type, key_id, value)
    }

    fn remove(&self, key_type: &str, key_id: &str) -> Result<(), String> {
        check_key(key_type, key_id)?;
        let mut log = self.lock()?;
        if !log
            .records
            .contains_key(&(key_type.to_string(), key_id.to_string()))
        {
            return Ok(());
        }
        log.append(DELETE, key_type, key_id, &[])
    }

    fn get(&self, key_type: &str, key_id: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self
            .lock()?
            .records
            .get(&(key_type.to_string(), key_id.to_string()))
            .cloned())
    }

    fn keys(&self, key_type: &str) -> Result<Vec<String>, String> {
        let log = self.lock()?;
        let from = (key_type.to_string(), String::new());
        Ok(log
            .records
            .range(from..)
            .map(|((kind, id), _)| (kind, id))
            .take_while(|(kind, _)| kind.as_str() == key_type)
            .map(|(_, id)| id.clone())
            .collect())
    }
}

impl<F: LogFile> Log<F> {
    fn cipher_for(&self, generation: u64) -> Result<ChaCha20Poly1305, String> {
        derive(&self.store_key, self.category, generation)
    }

    fn header_aad(&self, generation: u64) -> Vec<u8> {
        let mut aad = self.category.domain().to_vec();
        aad.extend_from_slice(b"/header/");
        aad.extend_from_slice(&generation.to_be_bytes());
        aad
    }

    fn record_aad(&self, generation: u64, seq: u64) -> Vec<u8> {
        let mut aad = self.category.domain().to_vec();
        aad.extend_from_slice(b"/record/");
        aad.extend_from_slice(&generation.to_be_bytes());
        aad.extend_from_slice(&seq.to_be_bytes());
        aad
    }

    fn header(&self, cipher: &ChaCha20Poly1305, generation: u64) -> Result<Vec<u8>, String> {
        let mut nonce = [0u8; NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let aad = self.header_aad(generation);
        let tag = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &[],
                    aad: &aad,
                },
            )
            .map_err(|_| "sealing the header failed".to_string())?;
        let mut header = Vec::with_capacity(HEADER_BYTES as usize);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&[VERSION, 0, 0, 0]);
        header.extend_from_slice(&generation.to_be_bytes());
        header.extend_from_slice(&nonce);
        header.extend_from_slice(&tag);
        header.resize(HEADER_BYTES as usize, 0);
        Ok(header)
    }

    fn header_state(&mut self, index: usize) -> Result<HeaderState, OpenError> {
        let file = &mut self.files[index];
        let len = file.size().map_err(OpenError::Io)?;
        if len == 0 {
            return Ok(HeaderState::Empty);
        }
        // Every header is written whole, in one write, before any record
        // after it can matter, so a file shorter than one never held a
        // record: a crash during the first header write. Calling it a wrong
        // key would refuse a fresh account for ever.
        if len < HEADER_BYTES {
            return Ok(HeaderState::Uncommitted);
        }
        let mut raw = vec![0u8; HEADER_BYTES as usize];
        file.read_exact_at(0, &mut raw).map_err(OpenError::Io)?;
        // A compaction truncates the spare file to nothing before it writes,
        // and writes the header last, so zeros where the header goes are a
        // compaction that never committed, not a log.
        if raw.iter().all(|&b| b == 0) {
            return Ok(HeaderState::Uncommitted);
        }
        if &raw[..4] != MAGIC || raw[4] != VERSION {
            return Ok(HeaderState::Invalid);
        }
        let generation = u64::from_be_bytes(raw[8..16].try_into().unwrap_or_default());
        let aad = self.header_aad(generation);
        let cipher = self.cipher_for(generation).map_err(OpenError::Io)?;
        let opened = cipher.decrypt(
            Nonce::from_slice(&raw[16..28]),
            Payload {
                msg: &raw[28..44],
                aad: &aad,
            },
        );
        Ok(match opened {
            Ok(_) => HeaderState::Valid(generation),
            Err(_) => HeaderState::Invalid,
        })
    }

    fn replay(&mut self) -> Result<(), OpenError> {
        let len = self.files[self.active].size().map_err(OpenError::Io)?;
        // One read for the whole log: a record at a time would cost two
        // synchronous OPFS calls per record, about ten thousand for a
        // mebibyte of ratchet updates.
        let mut file =
            vec![0u8; usize::try_from(len).map_err(|_| OpenError::Io("log too large".into()))?];
        self.files[self.active]
            .read_exact_at(0, &mut file)
            .map_err(OpenError::Io)?;
        let mut at = HEADER_BYTES as usize;
        let mut seq = 0u64;
        while let Some(sealed) = record_at(&file, at) {
            let Some(plain) = self.open_record(seq, sealed) else {
                break;
            };
            let Some((op, key_type, key_id, value)) = decode(&plain) else {
                break;
            };
            self.apply(op, key_type, key_id, value);
            seq += 1;
            at += 4 + sealed.len();
        }
        if (at as u64) < len {
            // A tear leaves nothing whole after it. An intact record next is
            // damage inside the log, and cutting there would drop
            // acknowledged writes.
            if self.intact_record_after(&file, at, seq + 1) {
                return Err(OpenError::Damaged);
            }
            warn!(
                dropped = len - at as u64,
                "web log store: dropping a torn tail after the last whole record"
            );
            let file = &mut self.files[self.active];
            file.truncate(at as u64).map_err(OpenError::Io)?;
            file.flush().map_err(OpenError::Io)?;
            self.stats.flushes += 1;
        }
        self.seq = seq;
        self.end = at as u64;
        Ok(())
    }

    fn open_record(&self, seq: u64, sealed: &[u8]) -> Option<Vec<u8>> {
        let aad = self.record_aad(self.generation, seq);
        self.cipher
            .decrypt(
                Nonce::from_slice(&sealed[..NONCE_BYTES]),
                Payload {
                    msg: &sealed[NONCE_BYTES..],
                    aad: &aad,
                },
            )
            .ok()
    }

    /// Whether a whole record sealed under `seq` starts anywhere after the
    /// record at `at` that failed. Every offset is tried rather than the one
    /// the failed record's length prefix names, since that prefix may be
    /// the damage. Reached only at open, and only when the log does not end
    /// cleanly.
    fn intact_record_after(&self, file: &[u8], at: usize, seq: u64) -> bool {
        let first = at + 4 + NONCE_BYTES + TAG_BYTES;
        (first..file.len()).any(|offset| {
            record_at(file, offset).is_some_and(|sealed| self.open_record(seq, sealed).is_some())
        })
    }

    fn apply(&mut self, op: u8, key_type: String, key_id: String, value: Vec<u8>) {
        let cost = record_bytes(&key_type, &key_id, value.len());
        let key = (key_type, key_id);
        if let Some(old) = self.records.remove(&key) {
            self.live -= record_bytes(&key.0, &key.1, old.len());
        }
        if op == PUT {
            self.live += cost;
            self.records.insert(key, value);
        }
    }

    fn seal(
        &self,
        cipher: &ChaCha20Poly1305,
        generation: u64,
        seq: u64,
        plain: &[u8],
    ) -> Result<Vec<u8>, String> {
        let mut nonce = [0u8; NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let aad = self.record_aad(generation, seq);
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: &aad,
                },
            )
            .map_err(|_| "sealing failed".to_string())?;
        let size = (NONCE_BYTES + sealed.len()) as u32;
        let mut record = Vec::with_capacity(4 + size as usize);
        record.extend_from_slice(&size.to_be_bytes());
        record.extend_from_slice(&nonce);
        record.extend_from_slice(&sealed);
        Ok(record)
    }

    fn append(&mut self, op: u8, key_type: &str, key_id: &str, value: &[u8]) -> Result<(), String> {
        if let Some(reason) = &self.poisoned {
            return Err(format!("the log refuses writes: {reason}"));
        }
        let plain = Zeroizing::new(encode(op, key_type, key_id, value));
        let record = self.seal(&self.cipher, self.generation, self.seq, &plain)?;
        let file = &mut self.files[self.active];
        // On a failure the end does not move, so the next append overwrites
        // whatever part of this record reached the file.
        file.write_all_at(self.end, &record)?;
        file.flush()?;
        self.stats.flushes += 1;
        self.end += record.len() as u64;
        self.seq += 1;
        self.apply(op, key_type.to_string(), key_id.to_string(), value.to_vec());
        if self.end > COMPACT_FLOOR
            && self.end > 2 * (self.live + HEADER_BYTES)
            && self.end >= self.compact_after
        {
            match self.compact() {
                Ok(()) => self.compact_after = 0,
                Err(error) => {
                    warn!(%error, "web log store: compaction failed; the log stays as it was");
                    self.compact_after = self.end + COMPACT_FLOOR;
                }
            }
        }
        self.stats.log_bytes = self.end;
        self.stats.live_bytes = self.live;
        Ok(())
    }

    fn compact(&mut self) -> Result<(), String> {
        let spare = 1 - self.active;
        let generation = self.generation + 1;
        let cipher = self.cipher_for(generation)?;
        let mut body = Vec::with_capacity(self.live as usize);
        for (seq, ((key_type, key_id), value)) in self.records.iter().enumerate() {
            let plain = Zeroizing::new(encode(PUT, key_type, key_id, value));
            body.extend_from_slice(&self.seal(&cipher, generation, seq as u64, &plain)?);
        }
        let header = self.header(&cipher, generation)?;
        let file = &mut self.files[spare];
        // Until the header write begins, the spare holds zeros where the
        // header goes, which open ignores, so a failure here costs nothing.
        file.truncate(0)?;
        file.flush()?;
        file.write_all_at(HEADER_BYTES, &body)?;
        file.flush()?;
        self.stats.flushes += 2;
        if let Err(error) = file.write_all_at(0, &header).and_then(|()| file.flush()) {
            // The header may be on the disk. If it is, the next open takes
            // the spare, and every append to the live file from here on
            // would be lost after a restart. Empty the spare again; if that
            // fails too, refuse every later write instead.
            if let Err(undo) = file.truncate(0).and_then(|()| file.flush()) {
                self.poisoned = Some(format!(
                    "a compaction failed ({error}) and could not be undone ({undo})"
                ));
            }
            return Err(error);
        }
        self.stats.flushes += 1;
        // Committed. A failure from here on leaves two valid files, and the
        // next open takes this one, the higher generation.
        let old = &mut self.files[self.active];
        if old.truncate(0).and_then(|()| old.flush()).is_ok() {
            self.stats.flushes += 1;
        }
        self.active = spare;
        self.generation = generation;
        self.cipher = cipher;
        self.seq = self.records.len() as u64;
        self.end = HEADER_BYTES + body.len() as u64;
        self.stats.compactions += 1;
        Ok(())
    }
}

/// The sealed bytes of the record whose length prefix starts at `at`, when
/// the prefix names a plausible size that fits in `file`.
fn record_at(file: &[u8], at: usize) -> Option<&[u8]> {
    let prefix: [u8; 4] = file.get(at..at.checked_add(4)?)?.try_into().ok()?;
    let size = u32::from_be_bytes(prefix) as usize;
    if !(NONCE_BYTES + TAG_BYTES..=MAX_SEALED_BYTES).contains(&size) {
        return None;
    }
    file.get(at + 4..(at + 4).checked_add(size)?)
}

/// The cipher for one category's records in one generation of its log.
fn derive(
    store_key: &[u8; STORE_KEY_BYTES],
    category: Category,
    generation: u64,
) -> Result<ChaCha20Poly1305, String> {
    let mut info = category.domain().to_vec();
    info.extend_from_slice(b"/generation/");
    info.extend_from_slice(&generation.to_be_bytes());
    let mut derived = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(HKDF_SALT), store_key)
        .expand(&info, derived.as_mut())
        .map_err(|_| "key derivation failed".to_string())?;
    Ok(ChaCha20Poly1305::new(Key::from_slice(derived.as_ref())))
}

fn check_key(key_type: &str, key_id: &str) -> Result<(), String> {
    for component in [key_type, key_id] {
        if component.len() > MAX_COMPONENT_BYTES {
            return Err(format!(
                "a key component of {} bytes is over the {MAX_COMPONENT_BYTES}-byte ceiling",
                component.len()
            ));
        }
    }
    Ok(())
}

fn record_bytes(key_type: &str, key_id: &str, value: usize) -> u64 {
    (4 + NONCE_BYTES + 5 + key_type.len() + key_id.len() + value + TAG_BYTES) as u64
}

fn encode(op: u8, key_type: &str, key_id: &str, value: &[u8]) -> Vec<u8> {
    let mut plain = Vec::with_capacity(5 + key_type.len() + key_id.len() + value.len());
    plain.push(op);
    for component in [key_type, key_id] {
        // check_key bounds both components at 4096 bytes.
        plain.extend_from_slice(&(component.len() as u16).to_be_bytes());
        plain.extend_from_slice(component.as_bytes());
    }
    plain.extend_from_slice(value);
    plain
}

fn decode(plain: &[u8]) -> Option<(u8, String, String, Vec<u8>)> {
    let (&op, mut rest) = plain.split_first()?;
    if op != PUT && op != DELETE {
        return None;
    }
    let mut component = || -> Option<String> {
        let len = u16::from_be_bytes(rest.get(..2)?.try_into().ok()?) as usize;
        let text = std::str::from_utf8(rest.get(2..2 + len)?).ok()?.to_string();
        rest = &rest[2 + len..];
        Some(text)
    };
    let key_type = component()?;
    let key_id = component()?;
    if op == DELETE && !rest.is_empty() {
        return None;
    }
    Some((op, key_type, key_id, rest.to_vec()))
}

impl<F: LogFile> MlsStorage for LogStore<F> {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> StorageResult<()> {
        self.put(key_type, key_id, data)
            .map_err(StorageError::StoreFailed)
    }

    fn load(&self, key_type: &str, key_id: &str) -> StorageResult<Option<Vec<u8>>> {
        self.get(key_type, key_id).map_err(StorageError::LoadFailed)
    }

    fn delete(&self, key_type: &str, key_id: &str) -> StorageResult<()> {
        self.remove(key_type, key_id)
            .map_err(StorageError::DeleteFailed)
    }

    fn list_keys(&self, key_type: &str) -> StorageResult<Vec<String>> {
        self.keys(key_type).map_err(StorageError::LoadFailed)
    }
}

impl<F: LogFile> ProtocolStateStorage for LogStore<F> {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
        self.put(key_type, key_id, data)
            .map_err(ProtocolStateError::StoreFailed)
    }

    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
        self.get(key_type, key_id)
            .map_err(ProtocolStateError::LoadFailed)
    }

    fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
        self.remove(key_type, key_id)
            .map_err(ProtocolStateError::DeleteFailed)
    }

    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
        self.keys(key_type).map_err(ProtocolStateError::LoadFailed)
    }
}

/// A file in memory, shared between clones so a test can reopen what a store
/// wrote, or cut it short.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct MemoryFile(pub(crate) std::sync::Arc<Mutex<Vec<u8>>>);

#[cfg(test)]
impl LogFile for MemoryFile {
    fn size(&mut self) -> Result<u64, String> {
        Ok(self.0.lock().map_err(|e| e.to_string())?.len() as u64)
    }

    fn read_exact_at(&mut self, at: u64, buffer: &mut [u8]) -> Result<(), String> {
        let data = self.0.lock().map_err(|e| e.to_string())?;
        let at = at as usize;
        let slice = data
            .get(at..at + buffer.len())
            .ok_or_else(|| "read past the end".to_string())?;
        buffer.copy_from_slice(slice);
        Ok(())
    }

    fn write_all_at(&mut self, at: u64, bytes: &[u8]) -> Result<(), String> {
        let mut data = self.0.lock().map_err(|e| e.to_string())?;
        let at = at as usize;
        if data.len() < at + bytes.len() {
            data.resize(at + bytes.len(), 0);
        }
        data[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    fn truncate(&mut self, len: u64) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|e| e.to_string())?
            .resize(len as usize, 0);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; STORE_KEY_BYTES] = [7; STORE_KEY_BYTES];

    fn open(
        key: &[u8; STORE_KEY_BYTES],
        files: &(MemoryFile, MemoryFile),
    ) -> Result<LogStore<MemoryFile>, OpenError> {
        LogStore::open(key, Category::Mls, files.0.clone(), files.1.clone())
    }

    fn put(store: &LogStore<MemoryFile>, id: &str, value: &[u8]) {
        MlsStorage::store(store, "kind", id, value).expect("store");
    }

    fn get(store: &LogStore<MemoryFile>, id: &str) -> Option<Vec<u8>> {
        MlsStorage::load(store, "kind", id).expect("load")
    }

    #[test]
    fn the_log_store_passes_both_conformance_suites() {
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &files).expect("open");
        let report = offline_protocol::mls_storage_conformance::run(&store);
        assert!(report.is_green(), "{}", report.summary());
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = LogStore::open(
            &KEY,
            Category::ProtocolState,
            files.0.clone(),
            files.1.clone(),
        )
        .expect("open");
        let report = offline_protocol::storage_conformance::run(&store);
        assert!(report.is_green(), "{}", report.summary());
    }

    #[test]
    fn a_reopened_log_holds_every_acknowledged_write() {
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &files).expect("open");
        put(&store, "a", b"one");
        put(&store, "b", b"two");
        put(&store, "a", b"three");
        MlsStorage::delete(&store, "kind", "b").expect("delete");
        drop(store);
        let store = open(&KEY, &files).expect("reopen");
        assert_eq!(get(&store, "a").as_deref(), Some(&b"three"[..]));
        assert_eq!(get(&store, "b"), None);
    }

    #[test]
    fn a_torn_tail_is_dropped_and_the_previous_value_survives() {
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &files).expect("open");
        put(&store, "a", b"kept");
        let whole = files.0 .0.lock().expect("file").len();
        put(&store, "a", b"torn by a crash");
        drop(store);
        // Cut inside the last record, as a crash during its write would.
        files.0 .0.lock().expect("file").truncate(whole + 9);
        let store = open(&KEY, &files).expect("reopen");
        assert_eq!(get(&store, "a").as_deref(), Some(&b"kept"[..]));
        assert_eq!(files.0 .0.lock().expect("file").len(), whole);
        // The next write lands where the torn one was, and survives.
        put(&store, "a", b"after");
        drop(store);
        let store = open(&KEY, &files).expect("reopen");
        assert_eq!(get(&store, "a").as_deref(), Some(&b"after"[..]));
    }

    #[test]
    fn a_wrong_key_is_refused_and_nothing_is_truncated() {
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &files).expect("open");
        put(&store, "identity", b"the only copy");
        drop(store);
        let before = files.0 .0.lock().expect("file").clone();
        let other = [9u8; STORE_KEY_BYTES];
        assert_eq!(open(&other, &files).err(), Some(OpenError::WrongKey));
        assert_eq!(*files.0 .0.lock().expect("file"), before);
        let store = open(&KEY, &files).expect("the right key still opens it");
        assert_eq!(
            get(&store, "identity").as_deref(),
            Some(&b"the only copy"[..])
        );
    }

    #[test]
    fn a_record_from_the_other_category_does_not_authenticate() {
        let mls = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &mls).expect("open");
        put(&store, "a", b"mls");
        drop(store);
        // The same bytes opened as the other category: the header's key check
        // is bound to the category, so it is refused whole.
        let opened = LogStore::open(&KEY, Category::ProtocolState, mls.0.clone(), mls.1.clone());
        assert_eq!(opened.err(), Some(OpenError::WrongKey));
    }

    #[test]
    fn a_reordered_record_ends_the_log_there() {
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &files).expect("open");
        put(&store, "a", b"first");
        let first_end = files.0 .0.lock().expect("file").len();
        put(&store, "b", b"second");
        drop(store);
        // Move the second record ahead of the first: its sequence number no
        // longer matches its position.
        {
            let mut data = files.0 .0.lock().expect("file");
            let first = data[HEADER_BYTES as usize..first_end].to_vec();
            let second = data[first_end..].to_vec();
            data.truncate(HEADER_BYTES as usize);
            data.extend_from_slice(&second);
            data.extend_from_slice(&first);
        }
        let store = open(&KEY, &files).expect("reopen");
        assert_eq!(get(&store, "a"), None);
        assert_eq!(get(&store, "b"), None);
    }

    #[test]
    fn compaction_keeps_every_live_value_and_shrinks_the_log() {
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &files).expect("open");
        let value = vec![1u8; 4096];
        for n in 0..600 {
            put(&store, &format!("k{}", n % 3), &value);
        }
        put(&store, "last", b"value");
        let stats = store.stats();
        assert!(stats.compactions >= 1, "{stats:?}");
        assert!(stats.log_bytes < 2 * COMPACT_FLOOR, "{stats:?}");
        drop(store);
        let store = open(&KEY, &files).expect("reopen");
        for n in 0..3 {
            assert_eq!(get(&store, &format!("k{n}")), Some(value.clone()));
        }
        assert_eq!(get(&store, "last").as_deref(), Some(&b"value"[..]));
        // One file holds the log; the other was emptied when it committed.
        let lens = [
            files.0 .0.lock().expect("file").len(),
            files.1 .0.lock().expect("file").len(),
        ];
        assert!(lens.contains(&0), "{lens:?}");
    }

    #[test]
    fn a_compaction_that_never_committed_is_ignored() {
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &files).expect("open");
        put(&store, "a", b"live");
        drop(store);
        // What a crash leaves after the spare file got its records and before
        // its header: zeros where the header goes, then bytes.
        {
            let mut spare = files.1 .0.lock().expect("file");
            spare.resize(HEADER_BYTES as usize, 0);
            spare.extend_from_slice(&[0xAB; 200]);
        }
        let store = open(&KEY, &files).expect("reopen");
        assert_eq!(get(&store, "a").as_deref(), Some(&b"live"[..]));
        assert!(files.1 .0.lock().expect("file").is_empty());
    }

    #[test]
    fn an_oversized_value_or_component_is_refused() {
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &files).expect("open");
        let long = "x".repeat(MAX_COMPONENT_BYTES + 1);
        assert!(MlsStorage::store(&store, &long, "id", b"v").is_err());
        assert!(MlsStorage::store(&store, "kind", "id", &vec![0; MAX_VALUE_BYTES + 1]).is_err());
    }
    #[test]
    fn a_header_torn_on_the_first_run_starts_a_fresh_log() {
        let files = (MemoryFile::default(), MemoryFile::default());
        // A crash during the very first header write: the magic, no more.
        files.0 .0.lock().expect("file").extend_from_slice(b"OP");
        let store = open(&KEY, &files).expect("a torn first header is not a wrong key");
        put(&store, "a", b"fresh");
        drop(store);
        let store = open(&KEY, &files).expect("reopen");
        assert_eq!(get(&store, "a").as_deref(), Some(&b"fresh"[..]));
    }

    #[test]
    fn a_damaged_record_before_intact_ones_is_refused_untouched() {
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &files).expect("open");
        put(&store, "a", b"first");
        put(&store, "b", b"second");
        drop(store);
        // Flip a byte inside the first record's ciphertext.
        let before = {
            let mut data = files.0 .0.lock().expect("file");
            data[HEADER_BYTES as usize + 4 + NONCE_BYTES] ^= 1;
            data.clone()
        };
        assert_eq!(open(&KEY, &files).err(), Some(OpenError::Damaged));
        assert_eq!(*files.0 .0.lock().expect("file"), before);
    }

    /// A file that fails on command: `flush` number `fail_flush_at`, and
    /// every `truncate` once `fail_truncate_after` flushes have happened.
    #[derive(Clone)]
    struct FaultFile {
        inner: MemoryFile,
        flushes: std::sync::Arc<std::sync::atomic::AtomicU64>,
        fail_flush_at: u64,
        fail_truncate_after: u64,
    }

    impl FaultFile {
        fn new(inner: &MemoryFile, fail_flush_at: u64, fail_truncate_after: u64) -> Self {
            Self {
                inner: inner.clone(),
                flushes: Default::default(),
                fail_flush_at,
                fail_truncate_after,
            }
        }
    }

    impl LogFile for FaultFile {
        fn size(&mut self) -> Result<u64, String> {
            self.inner.size()
        }
        fn read_exact_at(&mut self, at: u64, buffer: &mut [u8]) -> Result<(), String> {
            self.inner.read_exact_at(at, buffer)
        }
        fn write_all_at(&mut self, at: u64, data: &[u8]) -> Result<(), String> {
            self.inner.write_all_at(at, data)
        }
        fn truncate(&mut self, len: u64) -> Result<(), String> {
            if self.flushes.load(std::sync::atomic::Ordering::SeqCst) >= self.fail_truncate_after {
                return Err("truncate failed".to_string());
            }
            self.inner.truncate(len)
        }
        fn flush(&mut self) -> Result<(), String> {
            let n = self
                .flushes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            if n == self.fail_flush_at {
                return Err("flush failed".to_string());
            }
            Ok(())
        }
    }

    /// Writes until the log tries to compact into its (faulty) spare.
    fn fill_until_compaction(store: &LogStore<FaultFile>) {
        let value = vec![1u8; 4096];
        for n in 0..600 {
            let _ = MlsStorage::store(store, "kind", &format!("k{}", n % 3), &value);
        }
    }

    #[test]
    fn a_compaction_whose_header_flush_fails_is_undone_and_later_writes_survive() {
        let (first, second) = (MemoryFile::default(), MemoryFile::default());
        // The spare's third flush follows the header write; it fails once.
        let store = LogStore::open(
            &KEY,
            Category::Mls,
            FaultFile::new(&first, u64::MAX, u64::MAX),
            FaultFile::new(&second, 3, u64::MAX),
        )
        .expect("open");
        fill_until_compaction(&store);
        MlsStorage::store(&store, "kind", "after", b"acknowledged").expect("store");
        drop(store);
        let store = LogStore::open(&KEY, Category::Mls, first, second).expect("reopen");
        assert_eq!(
            MlsStorage::load(&store, "kind", "after")
                .expect("load")
                .as_deref(),
            Some(&b"acknowledged"[..]),
        );
    }

    #[test]
    fn a_compaction_that_cannot_be_undone_refuses_later_writes() {
        let (first, second) = (MemoryFile::default(), MemoryFile::default());
        // The header flush fails and so does every truncate after it.
        let store = LogStore::open(
            &KEY,
            Category::Mls,
            FaultFile::new(&first, u64::MAX, u64::MAX),
            FaultFile::new(&second, 3, 3),
        )
        .expect("open");
        let value = vec![1u8; 4096];
        let mut acknowledged = Vec::new();
        for n in 0..600 {
            let id = format!("k{}", n % 3);
            if MlsStorage::store(&store, "kind", &id, &value).is_ok() {
                acknowledged.push(id);
            }
        }
        assert!(
            MlsStorage::store(&store, "kind", "after", b"never acknowledged").is_err(),
            "a write after an unrecoverable compaction was acknowledged"
        );
        drop(store);
        let store = LogStore::open(&KEY, Category::Mls, first, second).expect("reopen");
        for id in acknowledged {
            assert_eq!(
                MlsStorage::load(&store, "kind", &id).expect("load"),
                Some(value.clone())
            );
        }
    }

    #[test]
    fn each_generation_seals_under_its_own_key() {
        let a = derive(&KEY, Category::Mls, 1).expect("derive");
        let b = derive(&KEY, Category::Mls, 2).expect("derive");
        let nonce = Nonce::from_slice(&[0u8; NONCE_BYTES]);
        let sealed = a.encrypt(nonce, &b"x"[..]).expect("seal");
        assert!(b.decrypt(nonce, sealed.as_slice()).is_err());
    }

    #[test]
    fn a_damaged_length_prefix_before_intact_records_is_refused_untouched() {
        let files = (MemoryFile::default(), MemoryFile::default());
        let store = open(&KEY, &files).expect("open");
        put(&store, "a", b"first");
        put(&store, "b", b"second");
        put(&store, "c", b"third");
        drop(store);
        // A length prefix that names more bytes than the file holds is what
        // a tear looks like, so this damage used to cut "b" and "c".
        let before = {
            let mut data = files.0 .0.lock().expect("file");
            data[HEADER_BYTES as usize + 1] = 0x40;
            data.clone()
        };
        assert_eq!(open(&KEY, &files).err(), Some(OpenError::Damaged));
        assert_eq!(*files.0 .0.lock().expect("file"), before);
    }

    /// A file that counts its truncates and, when `refuse` is set, fails
    /// every one after the first. Open truncates the spare once; each
    /// compaction starts with one.
    #[derive(Clone, Default)]
    struct RefusingSpare {
        inner: MemoryFile,
        truncates: std::sync::Arc<std::sync::atomic::AtomicU64>,
        refuse: bool,
    }

    impl LogFile for RefusingSpare {
        fn size(&mut self) -> Result<u64, String> {
            self.inner.size()
        }
        fn read_exact_at(&mut self, at: u64, buffer: &mut [u8]) -> Result<(), String> {
            self.inner.read_exact_at(at, buffer)
        }
        fn write_all_at(&mut self, at: u64, data: &[u8]) -> Result<(), String> {
            self.inner.write_all_at(at, data)
        }
        fn truncate(&mut self, len: u64) -> Result<(), String> {
            if self
                .truncates
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                > 0
                && self.refuse
            {
                return Err("the disk is full".to_string());
            }
            self.inner.truncate(len)
        }
        fn flush(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn a_failed_compaction_waits_for_the_log_to_grow_before_it_is_retried() {
        let spare = RefusingSpare {
            refuse: true,
            ..Default::default()
        };
        let store = LogStore::open(&KEY, Category::Mls, RefusingSpare::default(), spare.clone())
            .expect("open");
        // About 2.4 MiB of overwrites: every append past the first mebibyte
        // wants a compaction, and each one fails.
        let value = vec![1u8; 4096];
        for n in 0..600 {
            MlsStorage::store(&store, "kind", &format!("k{}", n % 3), &value).expect("store");
        }
        let attempts = spare.truncates.load(std::sync::atomic::Ordering::SeqCst) - 1;
        assert!(
            (1..=3).contains(&attempts),
            "{attempts} compaction attempts in 600 appends"
        );
        assert_eq!(store.stats().compactions, 0);
    }
}
