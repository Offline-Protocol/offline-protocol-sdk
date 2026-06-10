//! File-backed [`MlsStorage`] for headless deployments.
//!
//! Each `(key_type, key_id)` entry is one file under
//! `<data_dir>/<key_type>/<hex(key_id)>`. Key ids are hex-encoded so
//! arbitrary ids (peer ids, group ids) can never escape the directory or
//! collide with each other. Writes go through a temp file + rename for
//! atomicity on POSIX filesystems.
//!
//! **Security note**: entries are stored as-is. On a phone the equivalent
//! storage is Keychain/Keystore; for a headless node, protect the data
//! directory with filesystem permissions (the node creates it `0700` on
//! Unix) and full-disk encryption. The directory holds MLS key material.

use offline_protocol_mls::storage::{MlsStorage, StorageError, StorageResult};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// File-system implementation of [`MlsStorage`].
pub struct FileStorage {
    root: PathBuf,
}

fn encode_id(key_id: &str) -> String {
    let mut out = String::with_capacity(key_id.len() * 2);
    for byte in key_id.as_bytes() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn decode_id(name: &str) -> Option<String> {
    if name.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(name.len() / 2);
    for chunk in name.as_bytes().chunks(2) {
        let hex = std::str::from_utf8(chunk).ok()?;
        bytes.push(u8::from_str_radix(hex, 16).ok()?);
    }
    String::from_utf8(bytes).ok()
}

fn sanitize_type(key_type: &str) -> StorageResult<String> {
    if key_type.is_empty()
        || !key_type
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(StorageError::StoreFailed(format!(
            "key_type must be non-empty [A-Za-z0-9_-]: {key_type:?}"
        )));
    }
    Ok(key_type.to_string())
}

impl FileStorage {
    /// Opens (creating if needed) a file-backed store rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> StorageResult<Self> {
        let root = root.into();
        fs::create_dir_all(&root)
            .map_err(|e| StorageError::Unavailable(format!("create {}: {e}", root.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&root, fs::Permissions::from_mode(0o700));
        }
        Ok(Self { root })
    }

    fn entry_path(&self, key_type: &str, key_id: &str) -> StorageResult<PathBuf> {
        let ty = sanitize_type(key_type)?;
        Ok(self.root.join(ty).join(encode_id(key_id)))
    }

    fn write_atomic(path: &Path, data: &[u8]) -> StorageResult<()> {
        let parent = path
            .parent()
            .ok_or_else(|| StorageError::Unavailable("entry path has no parent".into()))?;
        fs::create_dir_all(parent)
            .map_err(|e| StorageError::Unavailable(format!("create {}: {e}", parent.display())))?;
        let tmp = path.with_extension("tmp");
        {
            let mut file = fs::File::create(&tmp)
                .map_err(|e| StorageError::Unavailable(format!("create temp: {e}")))?;
            file.write_all(data)
                .map_err(|e| StorageError::Unavailable(format!("write: {e}")))?;
            file.sync_all()
                .map_err(|e| StorageError::Unavailable(format!("fsync: {e}")))?;
        }
        fs::rename(&tmp, path).map_err(|e| StorageError::Unavailable(format!("rename: {e}")))?;
        Ok(())
    }
}

impl MlsStorage for FileStorage {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> StorageResult<()> {
        let path = self.entry_path(key_type, key_id)?;
        Self::write_atomic(&path, data)
    }

    fn load(&self, key_type: &str, key_id: &str) -> StorageResult<Option<Vec<u8>>> {
        let path = self.entry_path(key_type, key_id)?;
        match fs::read(&path) {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StorageError::Unavailable(format!(
                "read {}: {e}",
                path.display()
            ))),
        }
    }

    fn delete(&self, key_type: &str, key_id: &str) -> StorageResult<()> {
        let path = self.entry_path(key_type, key_id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StorageError::Unavailable(format!(
                "delete {}: {e}",
                path.display()
            ))),
        }
    }

    fn list_keys(&self, key_type: &str) -> StorageResult<Vec<String>> {
        let dir = self.root.join(sanitize_type(key_type)?);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(StorageError::Unavailable(format!(
                    "list {}: {e}",
                    dir.display()
                )))
            }
        };
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| StorageError::Unavailable(format!("list entry: {e}")))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.ends_with(".tmp") {
                continue; // interrupted write — never surface as a key
            }
            if let Some(id) = decode_id(name) {
                keys.push(id);
            }
        }
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (FileStorage, PathBuf) {
        let dir = std::env::temp_dir()
            .join(format!("op-node-store-{}", std::process::id()))
            .join(format!(
                "{:x}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        (FileStorage::new(&dir).unwrap(), dir)
    }

    #[test]
    fn store_load_delete_roundtrip() {
        let (store, dir) = temp_store();
        assert_eq!(store.load("identity", "me").unwrap(), None);
        store.store("identity", "me", b"key-material").unwrap();
        assert_eq!(
            store.load("identity", "me").unwrap().as_deref(),
            Some(b"key-material".as_ref())
        );
        // Overwrite is atomic and replaces.
        store.store("identity", "me", b"v2").unwrap();
        assert_eq!(
            store.load("identity", "me").unwrap().as_deref(),
            Some(b"v2".as_ref())
        );
        store.delete("identity", "me").unwrap();
        assert_eq!(store.load("identity", "me").unwrap(), None);
        // Deleting a missing key is fine.
        store.delete("identity", "me").unwrap();
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn hostile_key_ids_cannot_escape_the_root() {
        let (store, dir) = temp_store();
        store.store("sessions", "../../etc/passwd", b"x").unwrap();
        // The entry lives under the root, hex-encoded; nothing outside it.
        let keys = store.list_keys("sessions").unwrap();
        assert_eq!(keys, vec!["../../etc/passwd".to_string()]);
        assert!(dir.join("sessions").exists());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn key_type_is_validated() {
        let (store, dir) = temp_store();
        assert!(store.store("../oops", "id", b"x").is_err());
        assert!(store.list_keys("").is_err());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn list_keys_roundtrips_arbitrary_ids() {
        let (store, dir) = temp_store();
        for id in ["alice", "peer:weird/$id", "群"] {
            store.store("tofu_keys", id, b"k").unwrap();
        }
        let mut keys = store.list_keys("tofu_keys").unwrap();
        keys.sort();
        assert_eq!(keys, vec!["alice", "peer:weird/$id", "群"]);
        fs::remove_dir_all(dir).ok();
    }
}
