//! [`SealedFileMlsStorage`]: MLS material in files, sealed under a store key.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use offline_protocol_mls::storage::{MlsStorage, StorageError, StorageResult};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use zeroize::Zeroizing;

use super::key::{StoreKeyError, StoreKeyProvider, STORE_KEY_BYTES};
use super::records::{self, BoundedRead, DirectoryLock, StoreLock, SweepMemo, MAX_FRAME_BYTES};
use super::{account_directory, lock_directory, FileStoreError};

/// Magic of a sealed record file. Distinct from the plain frame's `OPS1`, so
/// neither store mistakes the other's file for its own.
const SEALED_MAGIC: &[u8; 4] = b"OMS1";

/// ChaCha20-Poly1305 nonce length.
const NONCE_BYTES: usize = 12;

/// Poly1305 tag length.
const TAG_BYTES: usize = 16;

/// Bytes a seal adds to its plaintext.
const SEAL_OVERHEAD: usize = SEALED_MAGIC.len() + NONCE_BYTES + TAG_BYTES;

/// Largest sealed file a read accepts: the largest frame, sealed.
const MAX_SEALED_BYTES: usize = MAX_FRAME_BYTES + SEAL_OVERHEAD;

/// HKDF salt: names this construction, so a store key reused elsewhere (it
/// should not be) derives unrelated keys here.
const HKDF_SALT: &[u8] = b"offline-protocol sealed file mls store v1";

/// Prefix of a type directory: `c_` and the keyed digest of the key type.
const TYPE_PREFIX: &str = "c_";

/// Prefix of a record file: `e_` and the keyed digest of the type and id.
const ENTRY_PREFIX: &str = "e_";

/// The file that proves the store key before any record is read.
const KEY_CHECK_FILE: &str = "store-key-check";

/// What the key check seals.
const KEY_CHECK_PLAINTEXT: &[u8] = b"offline-protocol sealed file mls store key check v1";

/// Records the key check tries when the check file is missing or damaged.
const KEY_CHECK_PROBE_LIMIT: usize = 64;

/// The lock file this store holds in its account directory. Distinct from
/// the protocol-state store's, so the two can share a root.
const LOCK_FILE: &str = "mls-store.lock";

/// MLS storage in files, every record sealed under a key derived from the
/// operator's store key.
///
/// This is the headless host's stand-in for the platform keystore the mobile
/// bindings use. What it protects against is someone who can read the
/// directory but does not have the store key: a copied disk, a backup, another
/// local user. Against that reader:
///
/// - **Values are sealed.** ChaCha20-Poly1305 under a key derived with HKDF
///   from the store key and the account namespace, so a record copied into
///   another account's directory does not open.
/// - **Keys are sealed too.** The record's `(key_type, key_id)` travels inside
///   the ciphertext, and file names are an HMAC under a second derived key. A
///   listing reveals how many records of each kind exist, never which peers or
///   groups they belong to. (The protocol-state store's names are an unsalted
///   digest, a format shared with the bindings.)
/// - **A record is bound to its slot.** The associated data is the file name,
///   and the decrypted header must name the key asked for, so a record moved
///   onto another's name does not open.
///
/// What it cannot protect against is the store key itself. It arrives from a
/// [`StoreKeyProvider`], and on a host without a keystore that means in the
/// clear from the operator's configuration (residual risk R18).
///
/// A record that does not authenticate is reported as
/// [`StorageError::CorruptedData`], naming its file, and **left in place**.
/// The store never deletes on a read, because the common cause is a wrong key
/// and deleting on that would destroy the identity. [`Self::open`] checks the
/// key against a sealed check file first and refuses a wrong one before any
/// record is read, so a record that fails afterwards is damaged: the error
/// names the file, and what deleting it costs depends on what it held (the
/// engine regenerates a damaged protocol-state record key by itself).
pub struct SealedFileMlsStorage {
    directory: PathBuf,
    cipher: ChaCha20Poly1305,
    name_key: Zeroizing<[u8; 32]>,
    lock: StoreLock,
    swept: SweepMemo,
    _directory_lock: DirectoryLock,
}

impl std::fmt::Debug for SealedFileMlsStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedFileMlsStorage")
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

/// What the key check found when its file was missing or did not open.
enum Probe {
    NoRecords,
    Opened,
    NoneOpened,
}

impl SealedFileMlsStorage {
    /// Opens (creating if needed) the store at `<root>/<namespace>/`, sealed
    /// under the key `provider` supplies.
    ///
    /// The provider is asked once. The first open writes a sealed check file;
    /// every later open must open it, and a key that does not is refused with
    /// [`FileStoreError::WrongStoreKey`] before any record is read or changed.
    /// A check file that is missing or damaged is rebuilt only when an
    /// existing record opens under the key, which proves it.
    ///
    /// A second store over the same directory is refused with
    /// [`FileStoreError::InUse`] while this one lives.
    ///
    /// `root` should be a directory that survives an upgrade: this store holds
    /// the device's identity, and losing it gives the device a new address.
    pub fn open(
        root: impl AsRef<Path>,
        namespace: &str,
        provider: &dyn StoreKeyProvider,
    ) -> Result<Self, FileStoreError> {
        let directory = account_directory(root.as_ref(), namespace)?;
        let directory_lock = lock_directory(&directory, LOCK_FILE)?;
        let store_key = provider.store_key()?;
        let hkdf = Hkdf::<Sha256>::new(Some(HKDF_SALT), store_key.as_slice());
        let seal_key = derive(&hkdf, b"seal", namespace)?;
        let name_key = derive(&hkdf, b"names", namespace)?;
        let store = Self {
            directory,
            cipher: ChaCha20Poly1305::new(Key::from_slice(seal_key.as_slice())),
            name_key,
            lock: StoreLock::default(),
            swept: SweepMemo::default(),
            _directory_lock: directory_lock,
        };
        store.verify_store_key()?;
        Ok(store)
    }

    /// The directory this store owns.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn keyed_name(&self, prefix: &str, parts: &[&[u8]]) -> String {
        // Every part but the last is length-prefixed, so no two different
        // (type, id) pairs feed the MAC the same bytes.
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.name_key.as_slice())
            .unwrap_or_else(|_| unreachable!("HMAC accepts a key of any length"));
        if let Some((last, rest)) = parts.split_last() {
            for part in rest {
                mac.update(&(part.len() as u64).to_be_bytes());
                mac.update(part);
            }
            mac.update(last);
        }
        format!("{prefix}{}", hex::encode(mac.finalize().into_bytes()))
    }

    fn type_directory(&self, key_type: &str) -> PathBuf {
        self.directory
            .join(self.keyed_name(TYPE_PREFIX, &[b"type", key_type.as_bytes()]))
    }

    fn entry_name(&self, key_type: &str, key_id: &str) -> String {
        self.keyed_name(
            ENTRY_PREFIX,
            &[b"entry", key_type.as_bytes(), key_id.as_bytes()],
        )
    }

    fn entry_path(&self, key_type: &str, key_id: &str) -> PathBuf {
        self.type_directory(key_type)
            .join(self.entry_name(key_type, key_id))
    }

    /// The file a record lives in, for tests outside this module that damage
    /// one on purpose.
    #[cfg(test)]
    pub(crate) fn record_path(&self, key_type: &str, key_id: &str) -> PathBuf {
        self.entry_path(key_type, key_id)
    }

    /// Reports a record that is present but cannot be read, and leaves it.
    fn corrupt(path: &Path, reason: &str) -> StorageError {
        tracing::warn!(path = %path.display(), reason, "sealed MLS record is unreadable; left in place");
        StorageError::CorruptedData(format!("{reason}: {}; left in place", path.display()))
    }

    fn associated_data(name: &str) -> Vec<u8> {
        let mut aad = Vec::with_capacity(SEALED_MAGIC.len() + name.len());
        aad.extend_from_slice(SEALED_MAGIC);
        aad.extend_from_slice(name.as_bytes());
        aad
    }

    fn seal(&self, name: &str, plaintext: &[u8]) -> Option<Vec<u8>> {
        let mut nonce = [0u8; NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &Self::associated_data(name),
                },
            )
            .ok()?;
        let mut sealed = Vec::with_capacity(SEAL_OVERHEAD + plaintext.len());
        sealed.extend_from_slice(SEALED_MAGIC);
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&ciphertext);
        Some(sealed)
    }

    fn open_sealed(&self, name: &str, sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        let header = SEALED_MAGIC.len() + NONCE_BYTES;
        if sealed.len() < header + TAG_BYTES || &sealed[..SEALED_MAGIC.len()] != SEALED_MAGIC {
            return None;
        }
        self.cipher
            .decrypt(
                Nonce::from_slice(&sealed[SEALED_MAGIC.len()..header]),
                Payload {
                    msg: &sealed[header..],
                    aad: &Self::associated_data(name),
                },
            )
            .ok()
            .map(Zeroizing::new)
    }

    /// Opens one record file into its `(key_type, key_id)`, or `None` when it
    /// does not authenticate under this key and name. The value is not
    /// copied: the only plaintext is the zeroizing buffer, dropped here.
    fn open_record(&self, path: &Path, name: &str) -> Option<(String, String)> {
        let BoundedRead::Bytes(raw) = records::read_bounded(path, MAX_SEALED_BYTES).ok()? else {
            return None;
        };
        let plain = self.open_sealed(name, &raw)?;
        let header = records::parse_header(&plain)?;
        Some((header.key_type, header.key_id))
    }

    /// Proves the store key against the check file, minting the file on a
    /// fresh store and rebuilding it when a record proves the key.
    fn verify_store_key(&self) -> Result<(), FileStoreError> {
        let _guard = self.lock.lock();
        // The check is the one file written into the account directory, so
        // a crash mid-write orphans its temporary here, where no store write
        // would ever sweep.
        self.swept.sweep_once(&self.directory);
        let path = self.directory.join(KEY_CHECK_FILE);
        let io = |source| FileStoreError::Io {
            path: path.clone(),
            source,
        };
        let check_present =
            match records::read_bounded(&path, KEY_CHECK_PLAINTEXT.len() + SEAL_OVERHEAD)
                .map_err(io)?
            {
                BoundedRead::Bytes(raw) => match self.open_sealed(KEY_CHECK_FILE, &raw) {
                    Some(plain) if plain.as_slice() == KEY_CHECK_PLAINTEXT => return Ok(()),
                    _ => true,
                },
                BoundedRead::Oversized => true,
                BoundedRead::Absent => false,
            };
        // The check is missing, or present and not opening. Records that
        // open under this key prove it, whatever happened to the check: a
        // crash before its first write finished, an operator's cleanup, or
        // damage to the check alone. Records that exist and none of which
        // opens mean the key is wrong, and minting a check for it would lock
        // the right key out.
        //
        // Rebuilding a damaged check on a record's proof costs nothing: anyone
        // who can damage the check can delete it, which already reaches this
        // path. A check that does not open with no records to prove anything
        // stays a refusal; there is nothing sealed to lose, and the operator
        // can remove the check file to start the store afresh.
        match self.probe_existing_records().map_err(io)? {
            Probe::Opened => {
                if check_present {
                    tracing::warn!(
                        path = %path.display(),
                        "store key check does not open but a record does; rebuilding the check"
                    );
                }
            }
            Probe::NoneOpened => return Err(FileStoreError::WrongStoreKey(self.directory.clone())),
            Probe::NoRecords if check_present => {
                return Err(FileStoreError::WrongStoreKey(self.directory.clone()))
            }
            Probe::NoRecords => {}
        }
        let sealed = self
            .seal(KEY_CHECK_FILE, KEY_CHECK_PLAINTEXT)
            .ok_or_else(|| {
                FileStoreError::StoreKey(StoreKeyError::Unavailable(
                    "sealing the key check failed".to_string(),
                ))
            })?;
        records::write_atomic(&self.directory, &path, &sealed).map_err(io)
    }

    fn probe_existing_records(&self) -> std::io::Result<Probe> {
        let mut probe = Probe::NoRecords;
        let mut tried = 0usize;
        for type_entry in std::fs::read_dir(&self.directory)? {
            let type_entry = type_entry?;
            let is_type_dir = type_entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(TYPE_PREFIX))
                && type_entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if !is_type_dir {
                continue;
            }
            records::for_each_entry(
                &type_entry.path(),
                ENTRY_PREFIX,
                KEY_CHECK_PROBE_LIMIT,
                |path, name| {
                    if matches!(probe, Probe::Opened) || tried >= KEY_CHECK_PROBE_LIMIT {
                        return;
                    }
                    tried += 1;
                    probe = if self.open_record(path, name).is_some() {
                        Probe::Opened
                    } else {
                        Probe::NoneOpened
                    };
                },
            )?;
            if matches!(probe, Probe::Opened) || tried >= KEY_CHECK_PROBE_LIMIT {
                break;
            }
        }
        Ok(probe)
    }
}

fn derive(
    hkdf: &Hkdf<Sha256>,
    label: &[u8],
    namespace: &str,
) -> Result<Zeroizing<[u8; STORE_KEY_BYTES]>, FileStoreError> {
    let mut info = Vec::with_capacity(label.len() + 1 + namespace.len());
    info.extend_from_slice(label);
    info.push(0);
    info.extend_from_slice(namespace.as_bytes());
    let mut out = Zeroizing::new([0u8; STORE_KEY_BYTES]);
    hkdf.expand(&info, out.as_mut_slice()).map_err(|_| {
        FileStoreError::StoreKey(StoreKeyError::Unavailable(
            "key derivation failed".to_string(),
        ))
    })?;
    Ok(out)
}

impl MlsStorage for SealedFileMlsStorage {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> StorageResult<()> {
        let framed = Zeroizing::new(
            records::frame(key_type, key_id, data)
                .map_err(|err| StorageError::StoreFailed(err.to_string()))?,
        );
        let name = self.entry_name(key_type, key_id);
        let sealed = self
            .seal(&name, &framed)
            .ok_or_else(|| StorageError::StoreFailed("sealing the record failed".to_string()))?;
        let _guard = self.lock.lock();
        let directory = self.type_directory(key_type);
        records::private_mkdir(&directory)
            .map_err(|err| StorageError::StoreFailed(err.to_string()))?;
        self.swept.sweep_once(&directory);
        records::write_atomic(&directory, &directory.join(&name), &sealed)
            .map_err(|err| StorageError::StoreFailed(err.to_string()))
    }

    fn load(&self, key_type: &str, key_id: &str) -> StorageResult<Option<Vec<u8>>> {
        let _guard = self.lock.lock();
        let name = self.entry_name(key_type, key_id);
        let path = self.type_directory(key_type).join(&name);
        let raw = match records::read_bounded(&path, MAX_SEALED_BYTES)
            .map_err(|err| StorageError::LoadFailed(err.to_string()))?
        {
            BoundedRead::Absent => return Ok(None),
            BoundedRead::Oversized => {
                return Err(Self::corrupt(&path, "sealed record is over the ceiling"))
            }
            BoundedRead::Bytes(raw) => raw,
        };
        let plain = self.open_sealed(&name, &raw).ok_or_else(|| {
            Self::corrupt(
                &path,
                "sealed record does not authenticate under this store key",
            )
        })?;
        // The file name is in the associated data, so a record moved onto
        // another's name has already failed to open. This check is the second
        // lock: it holds even if two keys' names ever collided.
        match records::parse_header(&plain) {
            Some(header) if header.key_type == key_type && header.key_id == key_id => {
                Ok(Some(plain[header.value_offset..].to_vec()))
            }
            _ => Err(Self::corrupt(&path, "sealed record names another key")),
        }
    }

    fn delete(&self, key_type: &str, key_id: &str) -> StorageResult<()> {
        let _guard = self.lock.lock();
        records::remove(&self.entry_path(key_type, key_id))
            .map(|_| ())
            .map_err(|err| StorageError::DeleteFailed(err.to_string()))
    }

    fn list_keys(&self, key_type: &str) -> StorageResult<Vec<String>> {
        let _guard = self.lock.lock();
        let mut keys = BTreeSet::new();
        records::for_each_entry(
            &self.type_directory(key_type),
            ENTRY_PREFIX,
            records::MAX_LISTED_ENTRIES,
            |path, name| match self.open_record(path, name) {
                Some((record_type, record_id)) if record_type == key_type => {
                    keys.insert(record_id);
                }
                _ => tracing::debug!(name, "skipping a sealed record that does not open"),
            },
        )
        .map_err(|err| StorageError::LoadFailed(err.to_string()))?;
        Ok(keys.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_store::test_support::{files_under, TempRoot};
    use crate::file_store::{account_storage_namespace, StaticStoreKey};

    fn namespace() -> String {
        account_storage_namespace("com.example.mls", "alice")
    }

    fn key(byte: u8) -> StaticStoreKey {
        StaticStoreKey::new([byte; 32])
    }

    fn open(root: &TempRoot, byte: u8) -> SealedFileMlsStorage {
        SealedFileMlsStorage::open(root.path(), &namespace(), &key(byte)).expect("open")
    }

    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        files_under(dir)
            .into_iter()
            .map(|p| {
                let bytes = std::fs::read(&p).expect("read");
                (p, bytes)
            })
            .collect()
    }

    #[test]
    fn the_store_passes_the_conformance_suite() {
        let root = TempRoot::new("mls-conformance");
        let report = offline_protocol_mls::storage_conformance::run(&open(&root, 1));
        assert!(report.is_green(), "{report:#?}");
    }

    #[test]
    fn records_survive_a_reopen_under_the_same_key() {
        let root = TempRoot::new("mls-reopen");
        let large = vec![0x5au8; offline_protocol_mls::storage_conformance::LARGE_RECORD_BYTES];
        {
            let store = open(&root, 1);
            store
                .store("identity", "key_pair", b"identity")
                .expect("store");
            store.store("group_state", "team", &large).expect("store");
        }
        let store = open(&root, 1);
        assert_eq!(
            store.load("identity", "key_pair").expect("load"),
            Some(b"identity".to_vec())
        );
        assert_eq!(
            store.load("group_state", "team").expect("load"),
            Some(large)
        );
        assert_eq!(store.list_keys("group_state").expect("list"), vec!["team"]);
    }

    #[test]
    fn a_wrong_key_is_refused_at_open_and_nothing_changes() {
        let root = TempRoot::new("mls-wrong-key");
        {
            let store = open(&root, 1);
            store
                .store("identity", "key_pair", b"identity")
                .expect("store");
        }
        let before = snapshot(root.path());

        let result = SealedFileMlsStorage::open(root.path(), &namespace(), &key(2));
        assert!(matches!(result, Err(FileStoreError::WrongStoreKey(_))));
        assert_eq!(snapshot(root.path()), before);
    }

    #[test]
    fn a_lost_key_check_is_rebuilt_only_by_the_right_key() {
        let root = TempRoot::new("mls-lost-check");
        let check = {
            let store = open(&root, 1);
            store
                .store("identity", "key_pair", b"identity")
                .expect("store");
            store.directory().join(KEY_CHECK_FILE)
        };
        std::fs::remove_file(&check).expect("remove check");
        let before = snapshot(root.path());

        // A wrong key must not mint a check for itself over the records.
        assert!(matches!(
            SealedFileMlsStorage::open(root.path(), &namespace(), &key(2)),
            Err(FileStoreError::WrongStoreKey(_))
        ));
        assert_eq!(snapshot(root.path()), before);

        let store = open(&root, 1);
        assert!(check.exists(), "the right key rebuilds the check");
        assert_eq!(
            store.load("identity", "key_pair").expect("load"),
            Some(b"identity".to_vec())
        );
    }

    /// Damage to the check alone must not lock the right key out of its own
    /// identity: a record that opens proves the key, and the check is
    /// rebuilt. A wrong key still proves nothing and is still refused.
    #[test]
    fn a_damaged_key_check_is_rebuilt_only_by_the_right_key() {
        let root = TempRoot::new("mls-damaged-check");
        let check = {
            let store = open(&root, 1);
            store
                .store("identity", "key_pair", b"identity")
                .expect("store");
            store.directory().join(KEY_CHECK_FILE)
        };
        let mut bytes = std::fs::read(&check).expect("read");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&check, &bytes).expect("write");
        let before = snapshot(root.path());

        assert!(matches!(
            SealedFileMlsStorage::open(root.path(), &namespace(), &key(2)),
            Err(FileStoreError::WrongStoreKey(_))
        ));
        assert_eq!(snapshot(root.path()), before, "a wrong key changed nothing");

        let store = open(&root, 1);
        assert_ne!(std::fs::read(&check).expect("read"), bytes, "check rebuilt");
        assert_eq!(
            store.load("identity", "key_pair").expect("load"),
            Some(b"identity".to_vec())
        );
        drop(store);
        // And the rebuilt check is a real one: the next open takes the fast path.
        open(&root, 1);
    }

    /// With no record to prove anything, a check that does not open stays a
    /// refusal, whichever key is offered.
    #[test]
    fn a_damaged_key_check_over_an_empty_store_is_refused() {
        let root = TempRoot::new("mls-damaged-empty");
        let check = open(&root, 1).directory().join(KEY_CHECK_FILE);
        let mut bytes = std::fs::read(&check).expect("read");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&check, bytes).expect("write");
        assert!(matches!(
            SealedFileMlsStorage::open(root.path(), &namespace(), &key(1)),
            Err(FileStoreError::WrongStoreKey(_))
        ));
    }

    /// The probe keeps going past a record that does not open: one damaged
    /// record must not hide the good one that proves the key.
    #[test]
    fn a_lost_key_check_is_rebuilt_past_an_unreadable_record() {
        let root = TempRoot::new("mls-lost-check-damaged");
        let (check, damaged) = {
            let store = open(&root, 1);
            store.store("group_state", "a", b"one").expect("store");
            store.store("group_state", "b", b"two").expect("store");
            (
                store.directory().join(KEY_CHECK_FILE),
                store.entry_path("group_state", "a"),
            )
        };
        std::fs::remove_file(&check).expect("remove check");
        let mut bytes = std::fs::read(&damaged).expect("read");
        bytes[SEALED_MAGIC.len() + NONCE_BYTES] ^= 1;
        std::fs::write(&damaged, bytes).expect("write");

        let store = open(&root, 1);
        assert!(check.exists(), "the good record proved the key");
        assert_eq!(store.list_keys("group_state").expect("list"), vec!["b"]);
    }

    /// Nothing to write down in a record's name, so the error must carry the
    /// path: file names are keyed digests, and an operator cannot find the
    /// damaged file any other way.
    #[test]
    fn a_corrupt_record_error_names_its_file() {
        let root = TempRoot::new("mls-corrupt-path");
        let store = open(&root, 1);
        store.store("identity", "key_pair", b"id").expect("store");
        let path = store.entry_path("identity", "key_pair");
        let mut bytes = std::fs::read(&path).expect("read");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&path, bytes).expect("write");

        match store.load("identity", "key_pair") {
            Err(StorageError::CorruptedData(message)) => assert!(
                message.contains(&path.display().to_string()),
                "{message} does not name {}",
                path.display()
            ),
            other => panic!("expected CorruptedData, got {other:?}"),
        }
    }

    #[test]
    fn a_second_store_over_one_directory_is_refused() {
        let root = TempRoot::new("mls-in-use");
        let first = open(&root, 1);
        assert!(matches!(
            SealedFileMlsStorage::open(root.path(), &namespace(), &key(1)),
            Err(FileStoreError::InUse(_))
        ));
        drop(first);
        open(&root, 1);
    }

    /// The point of sealing names as well as values: a copied directory must
    /// not say which peers or groups the device knows.
    #[test]
    fn neither_values_nor_keys_reach_the_disk_in_the_clear() {
        let root = TempRoot::new("mls-opaque");
        let store = open(&root, 1);
        let secret_value = b"the-secret-value-marker";
        let secret_id = "group-named-secret-team";
        let secret_type = "contact_key_package";
        store
            .store(secret_type, secret_id, secret_value)
            .expect("store");

        for (path, bytes) in snapshot(root.path()) {
            let name = path.to_string_lossy();
            for needle in [
                &secret_value[..],
                secret_id.as_bytes(),
                secret_type.as_bytes(),
            ] {
                assert!(
                    !bytes.windows(needle.len()).any(|w| w == needle),
                    "{} holds {:?} in the clear",
                    name,
                    String::from_utf8_lossy(needle)
                );
                assert!(!name.contains(std::str::from_utf8(needle).expect("utf8")));
            }
        }
    }

    #[test]
    fn a_record_moved_onto_another_name_does_not_open_and_is_kept() {
        let root = TempRoot::new("mls-moved");
        let store = open(&root, 1);
        store.store("group_state", "a", b"one").expect("store");
        store.store("group_state", "b", b"two").expect("store");
        let a = store.entry_path("group_state", "a");
        std::fs::copy(store.entry_path("group_state", "b"), &a).expect("copy");

        assert!(matches!(
            store.load("group_state", "a"),
            Err(StorageError::CorruptedData(_))
        ));
        assert!(a.exists(), "the sealed store never deletes on a read");
        assert_eq!(store.list_keys("group_state").expect("list"), vec!["b"]);
        assert_eq!(
            store.load("group_state", "b").expect("load"),
            Some(b"two".to_vec())
        );
    }

    #[test]
    fn a_tampered_or_oversized_record_is_corrupt_and_kept() {
        let root = TempRoot::new("mls-tampered");
        let store = open(&root, 1);
        store
            .store("identity", "key_pair", b"identity")
            .expect("store");
        store.store("identity", "other", b"other").expect("store");
        let path = store.entry_path("identity", "key_pair");
        let mut bytes = std::fs::read(&path).expect("read");
        bytes[SEALED_MAGIC.len() + NONCE_BYTES] ^= 1;
        std::fs::write(&path, &bytes).expect("write");

        assert!(matches!(
            store.load("identity", "key_pair"),
            Err(StorageError::CorruptedData(_))
        ));
        assert!(path.exists());
        assert_eq!(store.list_keys("identity").expect("list"), vec!["other"]);

        let handle = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open");
        handle
            .set_len((MAX_SEALED_BYTES + 1) as u64)
            .expect("sparse extend");
        drop(handle);
        assert!(matches!(
            store.load("identity", "key_pair"),
            Err(StorageError::CorruptedData(_))
        ));
        assert!(path.exists());
    }

    /// The seal and name keys are derived per namespace, so one store key
    /// serving two accounts does not let one account's record open in the
    /// other's directory.
    #[test]
    fn a_record_copied_into_another_account_does_not_open() {
        let root = TempRoot::new("mls-accounts");
        let alice = open(&root, 1);
        let bob = SealedFileMlsStorage::open(
            root.path(),
            &account_storage_namespace("com.example.mls", "bob"),
            &key(1),
        )
        .expect("open");
        alice
            .store("identity", "key_pair", b"alice")
            .expect("store");
        bob.store("identity", "key_pair", b"bob").expect("store");

        std::fs::copy(
            alice.entry_path("identity", "key_pair"),
            bob.entry_path("identity", "key_pair"),
        )
        .expect("copy");
        assert!(matches!(
            bob.load("identity", "key_pair"),
            Err(StorageError::CorruptedData(_))
        ));
        assert_ne!(
            alice.entry_name("identity", "key_pair"),
            bob.entry_name("identity", "key_pair")
        );
    }

    #[test]
    fn ids_differing_only_in_case_are_distinct_records() {
        let root = TempRoot::new("mls-case");
        let store = open(&root, 1);
        store.store("group_state", "Team", b"upper").expect("store");
        store.store("group_state", "team", b"lower").expect("store");
        assert_eq!(
            store.load("group_state", "Team").expect("load"),
            Some(b"upper".to_vec())
        );
        assert_eq!(
            store.load("group_state", "team").expect("load"),
            Some(b"lower".to_vec())
        );
    }

    /// Length-prefixing keeps `("ab", "c")` and `("a", "bc")` apart.
    #[test]
    fn a_type_and_id_split_differently_name_different_files() {
        let root = TempRoot::new("mls-split");
        let store = open(&root, 1);
        assert_ne!(store.entry_name("ab", "c"), store.entry_name("a", "bc"));
    }
}
