//! Built-in file-backed stores, for a host with a filesystem and no platform
//! keystore.
//!
//! The engine takes its two stores from the host
//! ([`crate::OfflineProtocol::initialize_mls`]). The mobile bindings hand in the
//! Keychain or Keystore and an app-container directory; a headless host on
//! Linux has neither a keystore the SDK can assume nor a binding that writes
//! files for it. These two stores are that host's defaults:
//!
//! - [`FileProtocolStateStorage`] keeps restartable delivery state. It writes
//!   the same on-disk format as the Python, Swift and Kotlin providers, byte
//!   for byte, so a state directory is portable across bindings. Its values
//!   arrive already sealed by the engine for every sensitive category, exactly
//!   as they do for those providers.
//! - [`SealedFileMlsStorage`] keeps identity keys, group state and the record
//!   key that seals the other store. It seals every record under a key derived
//!   from a *store key* the operator supplies through a [`StoreKeyProvider`],
//!   and names its files with a keyed digest.
//!
//! # Invariants
//!
//! - **A write the store acknowledged survives a crash.** Every store writes a
//!   temporary file, flushes it, renames it into place and flushes the
//!   directory. The engine writes state that depends on a returned `Ok`
//!   immediately (the record key is the sharp case), so an acknowledged write
//!   that is only staged is a data-loss bug, not a performance choice.
//! - **A load never allocates more than the ceiling.** The file size is
//!   checked before the read and the read itself is capped, so a planted
//!   multi-gigabyte file costs a `stat`.
//! - **File names are fixed-length digests, never an encoding of the key.**
//!   Case-insensitive volumes (APFS, NTFS by default) fold `group:Team` and
//!   `group:team` onto one file, and group ids are chosen by peers. A digest
//!   is lowercase hex whatever the key, so the key is compared byte for byte.
//! - **The sealed store never deletes on a read.** A record that does not
//!   authenticate is reported as corrupt, with its path, and left in place.
//!   The common cause is the operator supplying the wrong store key, and
//!   deleting on that would destroy the identity.
//!   [`SealedFileMlsStorage::open`] refuses a wrong key before any record is
//!   read, so a record that fails afterwards is damaged, and the error names
//!   the file so the operator can decide.
//! - **One store per directory.** Each store holds an exclusive lock on its
//!   account directory until it is closed or dropped, and a second store over
//!   the same directory, in this process or another, is refused with
//!   [`FileStoreError::InUse`]. Two stores over one MLS directory would each
//!   advance ratchet state the other never sees, and the groups would diverge
//!   without an error.
//! - **Releasing a directory never depends on who still holds the store.**
//!   `close` releases the lock at once and makes every later operation an
//!   error. A host whose runtime decides when an object is freed (a garbage
//!   collector, a reference cycle through a callback) would otherwise hold
//!   the directory for as long as anything, anywhere, kept a reference.
//!
//! # Layout
//!
//! Each store owns `<root>/<namespace>/`, where the namespace is
//! [`account_storage_namespace`] of the application id and profile, the same
//! derivation every binding uses. The two stores use different directory
//! prefixes and different lock files, so they do not collide if given one
//! root, but they should not share one: protocol state is scoped to an
//! installation and is meant to be removed with it, while MLS material may
//! outlive it.

mod key;
mod mls;
mod records;
mod state;

pub use key::{EnvStoreKey, StaticStoreKey, StoreKeyError, StoreKeyProvider, STORE_KEY_BYTES};
pub use mls::SealedFileMlsStorage;
pub use state::{FileProtocolStateStorage, SealedState};

use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Domain separator of the account namespace digest. Shared with the Python,
/// Swift and Kotlin `StorageNamespace` helpers; changing it strands every
/// existing store.
const NAMESPACE_DOMAIN: &[u8] = b"offline-protocol-storage-v1";

/// Prefix of every account namespace.
const NAMESPACE_PREFIX: &str = "account-";

/// Returns the opaque, filesystem-safe namespace for one protocol account.
///
/// `account-` followed by the lowercase hex SHA-256 of
/// `"offline-protocol-storage-v1" || 0x00 || app_id || 0x00 || profile`, the
/// derivation every binding uses, so a directory written by one binding opens
/// under another for the same account.
pub fn account_storage_namespace(app_id: &str, profile: &str) -> String {
    let mut sha = Sha256::new();
    sha.update(NAMESPACE_DOMAIN);
    sha.update([0u8]);
    sha.update(app_id.as_bytes());
    sha.update([0u8]);
    sha.update(profile.as_bytes());
    format!("{NAMESPACE_PREFIX}{}", hex::encode(sha.finalize()))
}

/// Whether `value` is a well-formed account namespace.
///
/// The namespace becomes a path component, so an arbitrary string could
/// escape its own account (`..`) or collide with another's. Only the exact
/// shape [`account_storage_namespace`] produces is accepted: `account-` and
/// 64 lowercase hex digits.
pub fn is_account_storage_namespace(value: &str) -> bool {
    value.strip_prefix(NAMESPACE_PREFIX).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Why a built-in file store could not be opened, or could not answer a
/// question about its directory.
///
/// Opening and the probes are the only steps with their own error type: a
/// record operation reports through the error type of the trait the store
/// implements.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum FileStoreError {
    /// The namespace is not the output of [`account_storage_namespace`].
    #[error("invalid storage namespace: expected `account-` and 64 lowercase hex digits")]
    InvalidNamespace,
    /// The store directory could not be created or read.
    #[error("store directory {path} is unusable: {source}")]
    Io {
        /// The directory the store tried to use.
        path: PathBuf,
        /// The underlying failure.
        source: std::io::Error,
    },
    /// The store key provider did not produce a key.
    #[error("store key unavailable: {0}")]
    StoreKey(#[from] StoreKeyError),
    /// The store key does not open this store's records.
    ///
    /// Nothing was read, changed or deleted. The usual cause is the operator
    /// supplying a different key than the one the store was created with.
    #[error("the store key does not open the store at {0}; nothing was changed")]
    WrongStoreKey(PathBuf),
    /// The key check file does not open, and the store holds no record that
    /// could prove the key either way.
    ///
    /// Nothing was read, changed or deleted. Either the key is wrong or the
    /// check file is damaged, and an empty store cannot tell the two apart.
    /// With nothing sealed to lose, removing the check file named here lets
    /// the next open start the store afresh under the key offered.
    #[error(
        "the store key check at {0} does not open and the store has no record to prove \
         the key; nothing was changed. If this is the right key, remove that file to \
         start the store afresh"
    )]
    KeyCheckUnverifiable(PathBuf),
    /// Another store already has this directory open, in this process or in
    /// another one.
    ///
    /// Nothing was read or changed. Two stores over one directory would
    /// diverge the MLS state, so the second is refused rather than guarded.
    #[error(
        "the store at {0} is already open elsewhere; two stores over one directory are refused"
    )]
    InUse(PathBuf),
    /// The store was closed, and holds its directory no longer.
    #[error("the store at {0} is closed")]
    Closed(PathBuf),
    /// The MLS store could not say whether it holds the key that seals
    /// protocol state.
    ///
    /// Nothing was changed. This is a read that failed, not a key that is
    /// missing or damaged: those are answers, and are reported as
    /// [`SealedState`].
    #[error("the protocol-state record key could not be read: {0}; nothing was changed")]
    RecordKeyUnreadable(String),
}

fn account_directory(root: &std::path::Path, namespace: &str) -> Result<PathBuf, FileStoreError> {
    if !is_account_storage_namespace(namespace) {
        return Err(FileStoreError::InvalidNamespace);
    }
    let directory = root.join(namespace);
    records::private_mkdir(&directory).map_err(|source| FileStoreError::Io {
        path: directory.clone(),
        source,
    })?;
    Ok(directory)
}

/// Whether the account directory `<root>/<namespace>/` holds an entry that
/// `counts` accepts, without creating, locking or changing anything.
///
/// A directory that is not there holds nothing. One that cannot be read is
/// an error, never "empty": a caller deciding whether it may start a new
/// identity over it must not take a permission error for a fresh install.
/// `counts` is handed the entry's path and name, and may look inside it; an
/// error it returns is reported against the path it was given.
fn account_holds(
    root: &std::path::Path,
    namespace: &str,
    counts: impl Fn(&std::path::Path, &str) -> std::io::Result<bool>,
) -> Result<bool, FileStoreError> {
    if !is_account_storage_namespace(namespace) {
        return Err(FileStoreError::InvalidNamespace);
    }
    let directory = root.join(namespace);
    let unusable = |path: &std::path::Path, source: std::io::Error| FileStoreError::Io {
        path: path.to_path_buf(),
        source,
    };
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(unusable(&directory, err)),
    };
    for entry in entries {
        let entry = entry.map_err(|err| unusable(&directory, err))?;
        let path = entry.path();
        if counts(&path, &entry.file_name().to_string_lossy())
            .map_err(|err| unusable(&path, err))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Takes the store's exclusive lock on `directory`.
fn lock_directory(
    directory: &std::path::Path,
    lock_name: &str,
) -> Result<records::DirectoryLock, FileStoreError> {
    records::DirectoryLock::acquire(directory, lock_name).map_err(|err| match err {
        records::LockError::Held => FileStoreError::InUse(directory.to_path_buf()),
        records::LockError::Io(source) => FileStoreError::Io {
            path: directory.join(lock_name),
            source,
        },
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    use rand_core::{OsRng, RngCore};

    /// A unique directory under the system temp dir, removed on drop.
    pub(crate) struct TempRoot(PathBuf);

    impl TempRoot {
        pub(crate) fn new(label: &str) -> Self {
            let mut suffix = [0u8; 8];
            OsRng.fill_bytes(&mut suffix);
            let path = std::env::temp_dir()
                .join(format!("offline-protocol-{label}-{}", hex::encode(suffix)));
            std::fs::create_dir_all(&path).expect("create temp root");
            Self(path)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Every regular file under `dir`, recursively.
    pub(crate) fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(next) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&next) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vector the Python binding pins in `test_storage_namespace.py`; the
    /// Swift and Kotlin helpers share the derivation. A Rust store that
    /// derived anything else would open an empty directory for an account a
    /// binding had already written.
    #[test]
    fn the_namespace_matches_the_bindings() {
        assert_eq!(
            account_storage_namespace("test-app", "test-user-1"),
            "account-814873e0cbdb2a1f25f14b31625e7f904cf9923e55b415b91ca4b29b210c12a1"
        );
        assert_ne!(
            account_storage_namespace("chat", "alice"),
            account_storage_namespace("chat", "bob")
        );
        assert_ne!(
            account_storage_namespace("chat", "alice"),
            account_storage_namespace("other-chat", "alice")
        );
    }

    #[test]
    fn only_the_derived_shape_is_a_namespace() {
        assert!(is_account_storage_namespace(&account_storage_namespace(
            "chat", "alice"
        )));
        let a64 = "a".repeat(64);
        for bad in [
            String::new(),
            "account-".to_string(),
            "../../other-account".to_string(),
            format!("account-{}", "a".repeat(63)),
            format!("account-{}", "a".repeat(65)),
            format!("account-{}", "A".repeat(64)),
            format!("account-{}", "g".repeat(64)),
            format!("account-{a64}/.."),
            format!("Account-{a64}"),
            format!("account-{}\u{0}", "a".repeat(63)),
        ] {
            assert!(!is_account_storage_namespace(&bad), "{bad:?} accepted");
        }
    }

    #[test]
    fn a_bad_namespace_is_refused_before_anything_is_created() {
        let root = test_support::TempRoot::new("namespace");
        let result = FileProtocolStateStorage::open(root.path(), "../escape");
        assert!(matches!(result, Err(FileStoreError::InvalidNamespace)));
        assert!(test_support::files_under(root.path()).is_empty());
        assert_eq!(
            std::fs::read_dir(root.path()).expect("read root").count(),
            0,
            "a refused namespace must not create a directory"
        );
    }
}
