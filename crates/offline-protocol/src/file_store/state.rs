//! [`FileProtocolStateStorage`]: the bindings' protocol-state file format,
//! written from Rust.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::records::{
    self, BoundedRead, DirectoryLock, StoreLock, SweepMemo, MAX_FRAME_BYTES, MAX_HEADER_BYTES,
};
use super::{account_directory, lock_directory, FileStoreError};
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
    _directory_lock: DirectoryLock,
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
    /// [`FileStoreError::InUse`] while this one lives.
    pub fn open(root: impl AsRef<Path>, namespace: &str) -> Result<Self, FileStoreError> {
        let directory = account_directory(root.as_ref(), namespace)?;
        let directory_lock = lock_directory(&directory, LOCK_FILE)?;
        Ok(Self {
            directory,
            lock: StoreLock::default(),
            swept: SweepMemo::default(),
            _directory_lock: directory_lock,
        })
    }

    /// The directory this store owns.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Whether `<root>/<namespace>/` holds protocol state, without opening,
    /// creating or locking anything.
    ///
    /// Anything but the lock file counts: a lock file is all an open that
    /// wrote nothing leaves, and every other entry is something a run (of
    /// this store or of a binding's provider, which write the same format)
    /// put there. A directory that cannot be read is an
    /// [`FileStoreError::Io`], never "no state".
    pub fn holds_records(root: impl AsRef<Path>, namespace: &str) -> Result<bool, FileStoreError> {
        super::account_holds(root.as_ref(), namespace, |name| !name.ends_with(".lock"))
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
        let _guard = self.lock.lock();
        let directory = self.type_directory(key_type);
        records::private_mkdir(&directory)
            .map_err(|err| ProtocolStateError::StoreFailed(err.to_string()))?;
        self.swept.sweep_once(&directory);
        records::write_atomic(&directory, &self.entry_path(key_type, key_id), &framed)
            .map_err(|err| ProtocolStateError::StoreFailed(err.to_string()))
    }

    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
        let _guard = self.lock.lock();
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
        let _guard = self.lock.lock();
        records::remove(&self.entry_path(key_type, key_id))
            .map(|_| ())
            .map_err(|err| ProtocolStateError::DeleteFailed(err.to_string()))
    }

    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
        let _guard = self.lock.lock();
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
mod tests {
    use super::*;
    use crate::file_store::account_storage_namespace;
    use crate::file_store::records::MAX_VALUE_BYTES;
    use crate::file_store::test_support::TempRoot;

    fn namespace() -> String {
        account_storage_namespace("com.example.state", "alice")
    }

    fn open(root: &TempRoot) -> FileProtocolStateStorage {
        FileProtocolStateStorage::open(root.path(), &namespace()).expect("open")
    }

    /// A lock file is all an open that wrote nothing leaves; anything else
    /// is state. The probe creates nothing.
    #[test]
    fn holds_records_ignores_only_the_lock_file() {
        let root = TempRoot::new("state-holds");
        let probe = || FileProtocolStateStorage::holds_records(root.path(), &namespace());
        assert!(!probe().expect("absent"));
        assert!(
            !root.path().join(namespace()).exists(),
            "the probe creates nothing"
        );
        let store = open(&root);
        assert!(!probe().expect("lock file only"));
        store
            .store("lamport_clock", "current", b"1")
            .expect("store");
        assert!(probe().expect("a record"));
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
