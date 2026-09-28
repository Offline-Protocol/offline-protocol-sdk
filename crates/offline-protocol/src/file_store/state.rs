//! [`FileProtocolStateStorage`]: the bindings' protocol-state file format,
//! written from Rust.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use offline_protocol_mls::storage::MlsStorage;

use super::pair::PAIRING_ID_BYTES;
use super::records::{self, BoundedRead, StoreLock, SweepMemo, MAX_FRAME_BYTES, MAX_HEADER_BYTES};
use super::{account_directory, lock_directory, FileStoreError};
use crate::protocol::state_crypto::StateRecordCipher;
use crate::protocol::{sealed_state_key_types, stored_state_record_key, StoredRecordKey};
use crate::protocol_state_storage::{
    ProtocolStateError, ProtocolStateResult, ProtocolStateStorage,
};

/// Prefix of a type directory: `t_` and the digest of the key type.
const TYPE_PREFIX: &str = "t_";

/// Prefix of a record file: `k_` and the digest of the key type and id.
const ENTRY_PREFIX: &str = "k_";

/// The lock file this store holds in its account directory. Distinct from
/// the sealed store's, so the two can share a root.
const LOCK_FILE: &str = "state-store.lock";

/// The pairing id of the MLS store this state belongs with, in the clear. In
/// the account directory, outside every type directory, so
/// [`FileProtocolStateStorage::holds_records`] never takes it for state, and
/// the binding providers, which only read type directories, never see it.
const PAIRING_FILE: &str = "state-store.pair";

/// Sealed records [`FileProtocolStateStorage::sealed_state`] tries in each
/// category before it moves to the next. One that opens settles the whole
/// question, so the ceiling is only reached in a category none of whose
/// records open. Per category, not overall: damage confined to one category
/// must not speak for a store whose other categories open.
const SEALED_STATE_PROBE_LIMIT: usize = 64;

/// What the record key an MLS store holds makes of the sealed records in a
/// protocol-state store.
///
/// The engine seals protocol state under a key it keeps in the MLS store, and
/// a record that does not open is deleted as the restore reaches it: a
/// record nothing can read is a loss to settle, not a read to retry. That is
/// right for one damaged record and wrong for a store sealed under another
/// identity's key, where the first restore deletes the queued and parked
/// messages of an identity that is merely somewhere else. This is the
/// question to ask before the engine is handed the pair.
///
/// It is asked of the categories the engine seals. The telemetry queue is
/// sealed under the same key and is not examined: a batch that does not open
/// is the uploader's to drop, and no message is lost with it.
///
/// The records alone cannot tell another identity's state from this
/// identity's state after the engine regenerated a damaged record key. The
/// launch that regenerates it cannot delete everything sealed under the lost
/// key (documents are read when they are opened, and each restore walk stops
/// at its delete allowance), so every later launch finds records that open
/// under no key the MLS store holds, and this answers `Foreign`. Open the two
/// stores with [`crate::FileStorePair`], which records which MLS store a state
/// store belongs with and asks the records only of a state store never bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SealedState {
    /// No record in any category the engine seals.
    Empty,
    /// A sealed record opens under the MLS store's record key: the two
    /// stores belong together.
    Opens,
    /// Sealed records exist, in no category does one of those tried open,
    /// and the MLS store holds a usable record key or none at all: they were
    /// sealed under another key, another identity's or one this identity
    /// lost and replaced (see above). The engine deletes the ones its restore
    /// reads, queued and parked messages among them.
    Foreign,
    /// Sealed records exist and the MLS store's record key is damaged, so
    /// nothing will open them again, whoever sealed them. Refusing the pair
    /// saves nothing. The engine generates a new key and settles what its
    /// restore reads as failed, which is how the application learns which
    /// messages were lost.
    KeyDamaged,
}

/// What [`FileProtocolStateStorage::probe_category`] found in one category.
enum SealedCategory {
    Empty,
    Opens,
    NoneOpens,
}

/// Protocol-state storage in plain files, in the format the Python, Swift and
/// Kotlin providers write.
///
/// A record lives at `<root>/<namespace>/t_<sha256(type)>/k_<sha256(type, id)>`
/// and carries its own key in a header (see the module docs), so every file is
/// attributable and a read verifies it opened the record it asked for. The
/// directory is portable: a store written by the Python binding opens here for
/// the same account, and the reverse.
///
/// Values are stored verbatim. The engine seals every category that can carry
/// message plaintext or key material before it reaches this trait, so those
/// arrive as ciphertext under the record key held in the MLS store. Record
/// *keys* are peer and message ids and are not sealed; the file names are an
/// unsalted digest of them, so anyone who can list the directory can confirm a
/// guessed id. Directories are created owner-only for that reason.
pub struct FileProtocolStateStorage {
    directory: PathBuf,
    lock: StoreLock,
    swept: SweepMemo,
}

impl std::fmt::Debug for FileProtocolStateStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileProtocolStateStorage")
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

fn digest(components: &[&str]) -> String {
    let mut sha = Sha256::new();
    for component in components {
        sha.update(component.as_bytes());
        sha.update([0u8]);
    }
    hex::encode(sha.finalize())
}

fn type_directory_name(key_type: &str) -> String {
    format!("{TYPE_PREFIX}{}", digest(&[key_type]))
}

fn entry_name(key_type: &str, key_id: &str) -> String {
    format!("{ENTRY_PREFIX}{}", digest(&[key_type, key_id]))
}

impl FileProtocolStateStorage {
    /// Opens (creating if needed) the store at `<root>/<namespace>/`.
    ///
    /// `namespace` must be [`super::account_storage_namespace`] of the
    /// application id and profile; anything else is refused before a
    /// directory is created. `root` should be a directory the host removes
    /// when the installation is removed: protocol state is scoped to an
    /// installation, and a store that outlives it resends settled work.
    ///
    /// A second store over the same directory is refused with
    /// [`FileStoreError::InUse`] until this one is closed or dropped.
    pub fn open(root: impl AsRef<Path>, namespace: &str) -> Result<Self, FileStoreError> {
        let directory = account_directory(root.as_ref(), namespace)?;
        let directory_lock = lock_directory(&directory, LOCK_FILE)?;
        Ok(Self {
            directory,
            lock: StoreLock::new(directory_lock),
            swept: SweepMemo::default(),
        })
    }

    /// Releases the directory and refuses every later operation.
    ///
    /// The lock goes at once, whoever still holds the store. An operation in
    /// flight finishes first. Afterwards every operation is an error naming
    /// the directory, and a new store may open over it. Closing twice is
    /// closing once.
    pub fn close(&self) {
        self.lock.close();
    }

    /// What the record key `secure` holds makes of the sealed records here.
    ///
    /// Read-only on both stores: nothing is deleted, and no record key is
    /// generated. A record whose framing does not parse is passed over, since
    /// it says nothing about any key. A record that cannot be read at all is
    /// an [`FileStoreError::Io`], and a record key the MLS store cannot read
    /// (as opposed to one it does not hold, or holds damaged) is an
    /// [`FileStoreError::RecordKeyUnreadable`]: neither is taken for an
    /// answer.
    pub fn sealed_state(&self, secure: &dyn MlsStorage) -> Result<SealedState, FileStoreError> {
        let _guard = self
            .lock
            .open()
            .ok_or_else(|| FileStoreError::Closed(self.directory.clone()))?;
        let key = stored_state_record_key(secure)
            .map_err(|err| FileStoreError::RecordKeyUnreadable(err.to_string()))?;
        let cipher = match &key {
            StoredRecordKey::Usable(cipher) => Some(cipher),
            StoredRecordKey::Absent | StoredRecordKey::Unrecoverable(_) => None,
        };
        let mut sealed_records = false;
        for key_type in sealed_state_key_types() {
            match self.probe_category(key_type, cipher)? {
                SealedCategory::Empty => {}
                SealedCategory::Opens => return Ok(SealedState::Opens),
                SealedCategory::NoneOpens => sealed_records = true,
            }
        }
        Ok(match key {
            _ if !sealed_records => SealedState::Empty,
            StoredRecordKey::Unrecoverable(_) => SealedState::KeyDamaged,
            StoredRecordKey::Usable(_) | StoredRecordKey::Absent => SealedState::Foreign,
        })
    }

    /// Tries the records of one sealed category against `cipher`, up to the
    /// ceiling, stopping at the first that opens. Without a cipher nothing
    /// can open, so one record is enough to know the category is not empty.
    fn probe_category(
        &self,
        key_type: &str,
        cipher: Option<&StateRecordCipher>,
    ) -> Result<SealedCategory, FileStoreError> {
        let directory = self.type_directory(key_type);
        let ceiling = if cipher.is_some() {
            SEALED_STATE_PROBE_LIMIT
        } else {
            1
        };
        let mut tried = 0usize;
        let mut opened = false;
        let mut unreadable = None;
        records::for_each_entry(
            &directory,
            ENTRY_PREFIX,
            records::MAX_LISTED_ENTRIES,
            |path, _| {
                if opened || unreadable.is_some() || tried >= ceiling {
                    return;
                }
                let raw = match records::read_bounded(path, MAX_FRAME_BYTES) {
                    Ok(BoundedRead::Bytes(raw)) => raw,
                    Ok(BoundedRead::Absent | BoundedRead::Oversized) => return,
                    Err(source) => {
                        unreadable = Some(FileStoreError::Io {
                            path: path.to_path_buf(),
                            source,
                        });
                        return;
                    }
                };
                let Some(header) = records::parse_header(&raw) else {
                    return;
                };
                if header.key_type != key_type {
                    return;
                }
                tried += 1;
                opened = cipher.is_some_and(|cipher| {
                    cipher
                        .open(key_type, &header.key_id, &raw[header.value_offset..])
                        .is_some()
                });
            },
        )
        .map_err(|source| FileStoreError::Io {
            path: directory.clone(),
            source,
        })?;
        if let Some(err) = unreadable {
            return Err(err);
        }
        Ok(match (opened, tried) {
            (true, _) => SealedCategory::Opens,
            (false, 0) => SealedCategory::Empty,
            (false, _) => SealedCategory::NoneOpens,
        })
    }

    /// The pairing id this store was bound to, or `None` for a store never
    /// bound (written by an engine over stores opened one by one, or by a
    /// binding's provider). A file of the wrong length names no MLS store,
    /// and reads as `None`.
    pub(super) fn pairing(&self) -> Result<Option<[u8; PAIRING_ID_BYTES]>, FileStoreError> {
        let _guard = self
            .lock
            .open()
            .ok_or_else(|| FileStoreError::Closed(self.directory.clone()))?;
        let path = self.directory.join(PAIRING_FILE);
        match records::read_bounded(&path, PAIRING_ID_BYTES)
            .map_err(|source| FileStoreError::Io { path, source })?
        {
            BoundedRead::Bytes(raw) if raw.len() == PAIRING_ID_BYTES => {
                let mut id = [0u8; PAIRING_ID_BYTES];
                id.copy_from_slice(&raw);
                Ok(Some(id))
            }
            BoundedRead::Bytes(_) | BoundedRead::Absent | BoundedRead::Oversized => Ok(None),
        }
    }

    /// Binds this store to the MLS store whose pairing id is `id`, durably.
    pub(super) fn set_pairing(&self, id: &[u8; PAIRING_ID_BYTES]) -> Result<(), FileStoreError> {
        let _guard = self
            .lock
            .open()
            .ok_or_else(|| FileStoreError::Closed(self.directory.clone()))?;
        #[cfg(test)]
        if tests::FAIL_PAIRING_WRITE.with(|fail| fail.replace(false)) {
            return Err(FileStoreError::Io {
                path: self.directory.join(PAIRING_FILE),
                source: std::io::Error::other("injected by a test"),
            });
        }
        // The only file written into the account directory, so a crash
        // mid-write orphans its temporary here, where no record write sweeps.
        self.swept.sweep_once(&self.directory);
        let path = self.directory.join(PAIRING_FILE);
        records::write_atomic(&self.directory, &path, id)
            .map_err(|source| FileStoreError::Io { path, source })
    }

    /// What a closed store answers.
    fn closed(&self) -> String {
        format!("the store at {} is closed", self.directory.display())
    }

    /// The directory this store owns.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Whether `<root>/<namespace>/` holds protocol state, without opening,
    /// creating or locking anything.
    ///
    /// A record file in a type directory, which is where this store and
    /// every binding's provider put a record. Not the lock file an open
    /// leaves, not a type directory every record has since been deleted
    /// from, and not what the MLS store leaves in a directory the two were
    /// wrongly made to share: none of those is state a new identity could
    /// strand. A directory that cannot be read, at either level, is an
    /// [`FileStoreError::Io`], never "no state".
    pub fn holds_records(root: impl AsRef<Path>, namespace: &str) -> Result<bool, FileStoreError> {
        super::account_holds(root.as_ref(), namespace, |path, name| {
            if !name.starts_with(TYPE_PREFIX) {
                return Ok(false);
            }
            // `metadata`, not `is_dir`: see `SealedFileMlsStorage::holds_records`.
            if !std::fs::metadata(path)?.is_dir() {
                return Ok(false);
            }
            records::holds_entry(path, ENTRY_PREFIX)
        })
    }

    fn type_directory(&self, key_type: &str) -> PathBuf {
        self.directory.join(type_directory_name(key_type))
    }

    fn entry_path(&self, key_type: &str, key_id: &str) -> PathBuf {
        self.type_directory(key_type)
            .join(entry_name(key_type, key_id))
    }

    /// Removes a record that can never be read and returns the error saying
    /// so. `Corrupted`, not `Ok(None)`: the engine settles the message ids the
    /// application holds for a destroyed record, while absence is reported to
    /// no one.
    fn discard(path: &Path, reason: &str) -> ProtocolStateError {
        tracing::warn!(path = %path.display(), reason, "dropping protocol-state record");
        let _ = records::remove(path);
        ProtocolStateError::Corrupted(format!(
            "dropped unreadable protocol-state record: {reason}"
        ))
    }
}

impl ProtocolStateStorage for FileProtocolStateStorage {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
        let framed = records::frame(key_type, key_id, data)
            .map_err(|err| ProtocolStateError::StoreFailed(err.to_string()))?;
        let _guard = self
            .lock
            .open()
            .ok_or_else(|| ProtocolStateError::StoreFailed(self.closed()))?;
        let directory = self.type_directory(key_type);
        records::private_mkdir(&directory)
            .map_err(|err| ProtocolStateError::StoreFailed(err.to_string()))?;
        self.swept.sweep_once(&directory);
        records::write_atomic(&directory, &self.entry_path(key_type, key_id), &framed)
            .map_err(|err| ProtocolStateError::StoreFailed(err.to_string()))
    }

    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
        let _guard = self
            .lock
            .open()
            .ok_or_else(|| ProtocolStateError::LoadFailed(self.closed()))?;
        let path = self.entry_path(key_type, key_id);
        let raw = match records::read_bounded(&path, MAX_FRAME_BYTES)
            .map_err(|err| ProtocolStateError::LoadFailed(err.to_string()))?
        {
            BoundedRead::Absent => return Ok(None),
            BoundedRead::Oversized => {
                return Err(Self::discard(&path, "record is over the ceiling"))
            }
            BoundedRead::Bytes(raw) => raw,
        };
        match records::parse_header(&raw) {
            Some(header) if header.key_type == key_type && header.key_id == key_id => {
                let mut raw = raw;
                raw.drain(..header.value_offset);
                Ok(Some(raw))
            }
            // Malformed framing, or a name that resolves to some other
            // record: either way this is not the entry that was asked for.
            _ => Err(Self::discard(
                &path,
                "record framing does not name the requested key",
            )),
        }
    }

    fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
        let _guard = self
            .lock
            .open()
            .ok_or_else(|| ProtocolStateError::DeleteFailed(self.closed()))?;
        records::remove(&self.entry_path(key_type, key_id))
            .map(|_| ())
            .map_err(|err| ProtocolStateError::DeleteFailed(err.to_string()))
    }

    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
        let _guard = self
            .lock
            .open()
            .ok_or_else(|| ProtocolStateError::LoadFailed(self.closed()))?;
        // Deduped: a planted copy of a record under another name must not
        // make its id appear twice.
        let mut keys = BTreeSet::new();
        records::for_each_entry(
            &self.type_directory(key_type),
            ENTRY_PREFIX,
            records::MAX_LISTED_ENTRIES,
            |path, _| {
                if let Some(header) = records::read_prefix(path, MAX_HEADER_BYTES)
                    .as_deref()
                    .and_then(records::parse_header)
                {
                    if header.key_type == key_type {
                        keys.insert(header.key_id);
                    }
                }
            },
        )
        .map_err(|err| ProtocolStateError::LoadFailed(err.to_string()))?;
        Ok(keys.into_iter().collect())
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::file_store::account_storage_namespace;
    use crate::file_store::records::MAX_VALUE_BYTES;
    use crate::file_store::test_support::TempRoot;
    use offline_protocol_mls::storage::{StorageError, StorageResult};

    thread_local! {
        /// Set by a test to make this thread's next pairing write fail, as a
        /// full disk or a crash would.
        pub(in crate::file_store) static FAIL_PAIRING_WRITE: std::cell::Cell<bool> =
            const { std::cell::Cell::new(false) };
    }

    fn namespace() -> String {
        account_storage_namespace("com.example.state", "alice")
    }

    fn open(root: &TempRoot) -> FileProtocolStateStorage {
        FileProtocolStateStorage::open(root.path(), &namespace()).expect("open")
    }

    /// Only a record file is state. Not the lock file an open leaves, not a
    /// type directory emptied since, and not what the MLS store leaves in a
    /// directory the two were wrongly made to share: taking any of those for
    /// state refuses a fresh identity over a directory that holds nothing to
    /// lose. The probe creates nothing.
    #[test]
    fn holds_records_counts_record_files_and_nothing_else() {
        let root = TempRoot::new("state-holds");
        let probe = || FileProtocolStateStorage::holds_records(root.path(), &namespace());
        assert!(!probe().expect("absent"));
        assert!(
            !root.path().join(namespace()).exists(),
            "the probe creates nothing"
        );
        let store = open(&root);
        assert!(!probe().expect("lock file only"));

        for stray in ["mls-store.lock", "store-key-check", "t_not-a-directory"] {
            std::fs::write(store.directory().join(stray), b"").expect("stray file");
        }
        std::fs::create_dir(store.directory().join("c_an-mls-type-directory")).expect("mls");
        assert!(!probe().expect("what another store left"));

        store.store("outbox", "one", b"sealed").expect("store");
        store.delete("outbox", "one").expect("delete");
        assert!(store.type_directory("outbox").is_dir());
        assert!(!probe().expect("a type directory with no record left in it"));

        store
            .store("lamport_clock", "current", b"1")
            .expect("store");
        assert!(probe().expect("a record"));
    }

    /// Closing releases the directory while the store is still held. What
    /// the closed store then answers is a failed operation, never
    /// `Corrupted`: the engine settles a corrupt record as a message that
    /// failed for good.
    #[test]
    fn a_closed_store_releases_the_directory_and_refuses_every_operation() {
        let root = TempRoot::new("state-close");
        let store = open(&root);
        store.store("outbox", "one", b"kept").expect("store");
        assert!(matches!(
            FileProtocolStateStorage::open(root.path(), &namespace()),
            Err(FileStoreError::InUse(_))
        ));

        store.close();
        store.close();
        let next = open(&root);
        assert_eq!(
            next.load("outbox", "one").expect("load"),
            Some(b"kept".to_vec())
        );

        assert!(matches!(
            store.store("outbox", "one", b"other"),
            Err(ProtocolStateError::StoreFailed(m)) if m.contains("closed")
        ));
        assert!(matches!(
            store.load("outbox", "one"),
            Err(ProtocolStateError::LoadFailed(m)) if m.contains("closed")
        ));
        assert!(matches!(
            store.delete("outbox", "one"),
            Err(ProtocolStateError::DeleteFailed(m)) if m.contains("closed")
        ));
        assert!(matches!(
            store.list_keys("outbox"),
            Err(ProtocolStateError::LoadFailed(m)) if m.contains("closed")
        ));
        assert!(matches!(
            store.sealed_state(&NoRecordKey),
            Err(FileStoreError::Closed(_))
        ));
        assert_eq!(
            next.load("outbox", "one").expect("load"),
            Some(b"kept".to_vec()),
            "a closed store changes nothing in a directory it no longer holds"
        );
    }

    /// A secure store that holds no record key, one that cannot say, one
    /// that holds a given key, and one whose key record is damaged.
    struct NoRecordKey;
    struct UnreadableRecordKey;
    struct RecordKey([u8; 32]);
    struct DamagedRecordKey;

    impl MlsStorage for RecordKey {
        fn store(&self, _: &str, _: &str, _: &[u8]) -> StorageResult<()> {
            Ok(())
        }
        fn load(&self, _: &str, _: &str) -> StorageResult<Option<Vec<u8>>> {
            Ok(Some(self.0.to_vec()))
        }
        fn delete(&self, _: &str, _: &str) -> StorageResult<()> {
            Ok(())
        }
        fn list_keys(&self, _: &str) -> StorageResult<Vec<String>> {
            Ok(Vec::new())
        }
    }

    impl MlsStorage for DamagedRecordKey {
        fn store(&self, _: &str, _: &str, _: &[u8]) -> StorageResult<()> {
            Ok(())
        }
        fn load(&self, _: &str, _: &str) -> StorageResult<Option<Vec<u8>>> {
            Err(StorageError::CorruptedData(
                "the record does not authenticate".to_string(),
            ))
        }
        fn delete(&self, _: &str, _: &str) -> StorageResult<()> {
            Ok(())
        }
        fn list_keys(&self, _: &str) -> StorageResult<Vec<String>> {
            Ok(Vec::new())
        }
    }

    /// Seals `count` records of `key_type` under `key`, as the engine does.
    fn seal_records(store: &FileProtocolStateStorage, key: [u8; 32], key_type: &str, count: usize) {
        let cipher = StateRecordCipher::new(&key);
        for index in 0..count {
            let key_id = format!("record-{index}");
            let sealed = cipher.seal(key_type, &key_id, b"parked").expect("seal");
            store.store(key_type, &key_id, &sealed).expect("store");
        }
    }

    /// The ceiling is per category. Damage that fills one category (or a
    /// category another key sealed) must not speak for a store whose other
    /// categories open: the engine would drop those records and keep the
    /// rest, and a verdict of `Foreign` would refuse the whole store.
    #[test]
    fn records_that_do_not_open_in_one_category_do_not_speak_for_the_store() {
        let root = TempRoot::new("state-per-category");
        let store = open(&root);
        let mut sealed_types = sealed_state_key_types();
        let (first, second) = (
            sealed_types.next().expect("a sealed category"),
            sealed_types.next().expect("a second sealed category"),
        );
        seal_records(&store, [2; 32], first, SEALED_STATE_PROBE_LIMIT + 6);
        assert_eq!(
            store.sealed_state(&RecordKey([1; 32])).expect("foreign"),
            SealedState::Foreign
        );

        seal_records(&store, [1; 32], second, 1);
        assert_eq!(
            store.sealed_state(&RecordKey([1; 32])).expect("opens"),
            SealedState::Opens
        );
    }

    /// A damaged record key is its own answer. Nothing opens the sealed
    /// records again, whoever sealed them, so it is not `Foreign`: a host
    /// that refuses a foreign store must not refuse this one, where the
    /// engine's regeneration is what tells the application what was lost.
    #[test]
    fn a_damaged_record_key_is_not_a_foreign_store() {
        let root = TempRoot::new("state-damaged-key");
        let store = open(&root);
        assert_eq!(
            store.sealed_state(&DamagedRecordKey).expect("empty"),
            SealedState::Empty
        );
        let sealed_type = sealed_state_key_types().next().expect("a sealed category");
        seal_records(&store, [1; 32], sealed_type, 1);
        assert_eq!(
            store.sealed_state(&DamagedRecordKey).expect("damaged"),
            SealedState::KeyDamaged
        );
        assert_eq!(
            store.sealed_state(&NoRecordKey).expect("no key"),
            SealedState::Foreign
        );
    }

    impl MlsStorage for NoRecordKey {
        fn store(&self, _: &str, _: &str, _: &[u8]) -> StorageResult<()> {
            Ok(())
        }
        fn load(&self, _: &str, _: &str) -> StorageResult<Option<Vec<u8>>> {
            Ok(None)
        }
        fn delete(&self, _: &str, _: &str) -> StorageResult<()> {
            Ok(())
        }
        fn list_keys(&self, _: &str) -> StorageResult<Vec<String>> {
            Ok(Vec::new())
        }
    }

    impl MlsStorage for UnreadableRecordKey {
        fn store(&self, _: &str, _: &str, _: &[u8]) -> StorageResult<()> {
            Ok(())
        }
        fn load(&self, _: &str, _: &str) -> StorageResult<Option<Vec<u8>>> {
            Err(StorageError::LoadFailed(
                "the disk did not answer".to_string(),
            ))
        }
        fn delete(&self, _: &str, _: &str) -> StorageResult<()> {
            Ok(())
        }
        fn list_keys(&self, _: &str) -> StorageResult<Vec<String>> {
            Ok(Vec::new())
        }
    }

    /// A read of the record key that failed is not "no record key": taking
    /// it for one would refuse a healthy pair, and taking it for a usable
    /// key is not possible. It is reported as what it is.
    #[test]
    fn a_record_key_that_cannot_be_read_is_not_an_answer() {
        let root = TempRoot::new("state-unreadable-key");
        let store = open(&root);
        assert!(matches!(
            store.sealed_state(&UnreadableRecordKey),
            Err(FileStoreError::RecordKeyUnreadable(m)) if m.contains("did not answer")
        ));
    }

    /// Only a record in a sealed category counts, only one whose framing
    /// parses, and none is changed by being looked at.
    #[test]
    fn the_sealed_state_probe_reads_sealed_categories_and_changes_nothing() {
        let root = TempRoot::new("state-probe");
        let store = open(&root);
        assert_eq!(
            store.sealed_state(&NoRecordKey).expect("empty"),
            SealedState::Empty
        );

        // Not a sealed category, so it says nothing about any key.
        store
            .store("lamport_clock", "current", b"1")
            .expect("store");
        assert_eq!(
            store.sealed_state(&NoRecordKey).expect("unsealed only"),
            SealedState::Empty
        );

        // A file in a sealed category whose framing does not parse.
        let torn = store.type_directory("outbox").join("k_torn");
        std::fs::create_dir_all(torn.parent().expect("parent")).expect("type directory");
        std::fs::write(&torn, b"not a frame").expect("torn");
        assert_eq!(
            store.sealed_state(&NoRecordKey).expect("unparseable"),
            SealedState::Empty
        );

        store
            .store("outbox", "one", b"sealed elsewhere")
            .expect("store");
        let before = crate::file_store::test_support::files_under(root.path());
        assert_eq!(
            store.sealed_state(&NoRecordKey).expect("foreign"),
            SealedState::Foreign
        );
        assert_eq!(
            crate::file_store::test_support::files_under(root.path()),
            before,
            "the probe deletes nothing, the torn file included"
        );
        assert!(torn.exists());
    }

    #[test]
    fn the_store_passes_the_conformance_suite() {
        let root = TempRoot::new("state-conformance");
        let report = crate::storage_conformance::run(&open(&root));
        assert!(report.is_green(), "{report:#?}");
    }

    /// The vector `test_framing_golden_vector` pins for the Python provider,
    /// which the Swift and Kotlin providers share. A Rust store that wrote
    /// anything else would make a container unreadable across bindings.
    #[test]
    fn the_on_disk_format_matches_the_bindings_golden_vector() {
        let framed = records::frame("outbox", "m-1", b"\xaa\xbb").expect("frame");
        assert_eq!(framed, b"OPS1\x00\x06\x00\x03outboxm-1\xaa\xbb".to_vec());
        assert_eq!(
            type_directory_name("outbox"),
            "t_d5fac01c82279b8b061df80b3c312942e2ce27a41a48b1b7479ff07ad5a6198d"
        );
        assert_eq!(
            entry_name("outbox", "m-1"),
            "k_db5fcc2398ef2863d4269a61be6ea2de1f80d2889f34670c9a57c79cbe8058a1"
        );
        assert_eq!(
            records::parse_header(&framed).map(|h| h.value_offset),
            Some(17)
        );
    }

    /// A file laid down exactly as the Python provider writes it opens here.
    #[test]
    fn a_record_written_by_a_binding_opens() {
        let root = TempRoot::new("state-foreign");
        let store = open(&root);
        let directory = store.directory().join(type_directory_name("outbox"));
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(
            directory.join(entry_name("outbox", "m-1")),
            b"OPS1\x00\x06\x00\x03outboxm-1\xaa\xbb",
        )
        .expect("write");

        assert_eq!(
            store.load("outbox", "m-1").expect("load"),
            Some(vec![0xaa, 0xbb])
        );
        assert_eq!(store.list_keys("outbox").expect("list"), vec!["m-1"]);
    }

    #[test]
    fn records_survive_a_reopen() {
        let root = TempRoot::new("state-reopen");
        {
            let store = open(&root);
            store.store("outbox", "m-1", b"one").expect("store");
            store.store("pending", "bob", b"").expect("store");
        }
        let store = open(&root);
        assert_eq!(
            store.load("outbox", "m-1").expect("load"),
            Some(b"one".to_vec())
        );
        assert_eq!(
            store.load("pending", "bob").expect("load"),
            Some(Vec::new())
        );
        assert_eq!(store.list_keys("outbox").expect("list"), vec!["m-1"]);
    }

    #[test]
    fn an_oversized_file_is_dropped_without_being_read() {
        let root = TempRoot::new("state-oversized");
        let store = open(&root);
        store.store("outbox", "m-1", b"x").expect("store");
        let path = store.entry_path("outbox", "m-1");
        let handle = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open");
        handle
            .set_len((MAX_FRAME_BYTES + 1) as u64)
            .expect("sparse extend");
        drop(handle);

        assert!(matches!(
            store.load("outbox", "m-1"),
            Err(ProtocolStateError::Corrupted(_))
        ));
        assert!(!path.exists());
    }

    #[test]
    fn a_record_that_names_another_key_is_dropped_as_corrupt() {
        let root = TempRoot::new("state-misnamed");
        let store = open(&root);
        store.store("outbox", "m-1", b"one").expect("store");
        store.store("outbox", "m-2", b"two").expect("store");
        let first = store.entry_path("outbox", "m-1");
        std::fs::copy(store.entry_path("outbox", "m-2"), &first).expect("copy");

        assert!(matches!(
            store.load("outbox", "m-1"),
            Err(ProtocolStateError::Corrupted(_))
        ));
        assert!(!first.exists());
        // The planted copy never made m-2 appear twice, and m-2 still loads.
        assert_eq!(store.list_keys("outbox").expect("list"), vec!["m-2"]);
        assert_eq!(
            store.load("outbox", "m-2").expect("load"),
            Some(b"two".to_vec())
        );
    }

    #[test]
    fn garbage_framing_is_dropped_as_corrupt() {
        let root = TempRoot::new("state-garbage");
        let store = open(&root);
        store.store("outbox", "m-1", b"one").expect("store");
        let path = store.entry_path("outbox", "m-1");
        std::fs::write(&path, (0u8..9).collect::<Vec<_>>()).expect("write");
        assert!(matches!(
            store.load("outbox", "m-1"),
            Err(ProtocolStateError::Corrupted(_))
        ));
        assert!(!path.exists());
    }

    #[test]
    fn a_value_over_the_ceiling_is_refused_on_store() {
        let root = TempRoot::new("state-ceiling");
        let store = open(&root);
        let big = vec![0u8; MAX_VALUE_BYTES + 1];
        assert!(matches!(
            store.store("outbox", "m-1", &big),
            Err(ProtocolStateError::StoreFailed(_))
        ));
        assert_eq!(store.load("outbox", "m-1").expect("load"), None);
    }

    /// Case-insensitive volumes fold names; a digest name never depends on
    /// the key's case, so two ids differing only in case stay two records.
    #[test]
    fn ids_differing_only_in_case_are_distinct_records() {
        let root = TempRoot::new("state-case");
        let store = open(&root);
        store.store("groups", "Team", b"upper").expect("store");
        store.store("groups", "team", b"lower").expect("store");
        assert_eq!(
            store.load("groups", "Team").expect("load"),
            Some(b"upper".to_vec())
        );
        assert_eq!(
            store.load("groups", "team").expect("load"),
            Some(b"lower".to_vec())
        );
        assert_eq!(
            store.list_keys("groups").expect("list"),
            vec!["Team", "team"]
        );
    }

    #[test]
    fn a_second_store_over_one_directory_is_refused() {
        let root = TempRoot::new("state-in-use");
        let first = open(&root);
        assert!(matches!(
            FileProtocolStateStorage::open(root.path(), &namespace()),
            Err(FileStoreError::InUse(_))
        ));
        drop(first);
        open(&root);
    }

    #[cfg(unix)]
    #[test]
    fn directories_and_records_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let root = TempRoot::new("state-mode");
        let store = open(&root);
        store.store("outbox", "m-1", b"x").expect("store");
        let mode = |p: &Path| std::fs::metadata(p).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode(store.directory()), 0o700);
        assert_eq!(mode(&store.type_directory("outbox")), 0o700);
        assert_eq!(mode(&store.entry_path("outbox", "m-1")), 0o600);
    }
}
