//! Storage trait definitions for MLS key material.
//!
//! Apps implement the [`MlsStorage`] trait to provide platform-native secure storage
//! (e.g., iOS Keychain, Android Keystore, EncryptedSharedPreferences).

use thiserror::Error;

/// Errors that can occur during storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    /// Failed to store data.
    #[error("Failed to store data: {0}")]
    StoreFailed(String),

    /// Failed to load data.
    #[error("Failed to load data: {0}")]
    LoadFailed(String),

    /// Failed to delete data.
    #[error("Failed to delete data: {0}")]
    DeleteFailed(String),

    /// Key not found in storage.
    #[error("Key not found: {0}")]
    KeyNotFound(String),

    /// Data is corrupted or invalid.
    #[error("Corrupted data: {0}")]
    CorruptedData(String),

    /// Storage is not available.
    #[error("Storage unavailable: {0}")]
    Unavailable(String),
}

/// Result type for storage operations.
pub type StorageResult<T> = std::result::Result<T, StorageError>;

/// Trait for MLS key material storage.
///
/// Apps must implement this trait to provide secure storage for MLS cryptographic
/// material. The implementation should use platform-native secure storage:
///
/// - **iOS**: Keychain Services
/// - **Android**: EncryptedSharedPreferences or Keystore
/// - **Desktop**: OS-specific secure storage or encrypted file storage
///
/// # Key Types
///
/// The storage uses a two-part key system: `(key_type, key_id)` where:
/// - `key_type`: Category of data (e.g., "identity", "key_package", "group_state")
/// - `key_id`: Unique identifier within that category
///
/// # Thread Safety
///
/// Implementations must be thread-safe (`Send + Sync`).
///
/// # Example
///
/// ```ignore
/// struct KeychainStorage;
///
/// impl MlsStorage for KeychainStorage {
///     fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> StorageResult<()> {
///         // Store in iOS Keychain
///         keychain_store(&format!("{}:{}", key_type, key_id), data)?;
///         Ok(())
///     }
///     // ... other methods
/// }
/// ```
pub trait MlsStorage: Send + Sync {
    /// Stores data with the given key type and ID.
    ///
    /// If data already exists for this key, it should be overwritten.
    ///
    /// # Arguments
    ///
    /// * `key_type` - Category of the data (e.g., "identity", "group_state")
    /// * `key_id` - Unique identifier within the category
    /// * `data` - Binary data to store
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> StorageResult<()>;

    /// Loads data for the given key type and ID.
    ///
    /// Returns `Ok(None)` if the key doesn't exist.
    /// Returns `Err` only for actual failures (corruption, access denied, etc.).
    ///
    /// # Arguments
    ///
    /// * `key_type` - Category of the data
    /// * `key_id` - Unique identifier within the category
    fn load(&self, key_type: &str, key_id: &str) -> StorageResult<Option<Vec<u8>>>;

    /// Deletes data for the given key type and ID.
    ///
    /// Should succeed even if the key doesn't exist.
    ///
    /// # Arguments
    ///
    /// * `key_type` - Category of the data
    /// * `key_id` - Unique identifier within the category
    fn delete(&self, key_type: &str, key_id: &str) -> StorageResult<()>;

    /// Lists all key IDs for a given key type.
    ///
    /// This is used for enumerating groups, key packages, etc.
    ///
    /// # Arguments
    ///
    /// * `key_type` - Category of data to list
    fn list_keys(&self, key_type: &str) -> StorageResult<Vec<String>>;

    /// Checks if a key exists.
    ///
    /// Default implementation uses `load`, but can be overridden for efficiency.
    fn exists(&self, key_type: &str, key_id: &str) -> StorageResult<bool> {
        Ok(self.load(key_type, key_id)?.is_some())
    }

    /// Clears all data of a given key type.
    ///
    /// Default implementation lists and deletes each key, but can be overridden
    /// for efficiency.
    fn clear_type(&self, key_type: &str) -> StorageResult<()> {
        for key_id in self.list_keys(key_type)? {
            self.delete(key_type, &key_id)?;
        }
        Ok(())
    }
}

/// In-memory storage implementation for testing.
///
/// **WARNING**: This implementation is NOT secure and should only be used for testing.
/// All data is lost when the process exits.
#[derive(Debug, Default)]
pub struct InMemoryStorage {
    data: std::sync::RwLock<std::collections::HashMap<String, Vec<u8>>>,
}

impl InMemoryStorage {
    /// Creates a new in-memory storage.
    pub fn new() -> Self {
        Self::default()
    }

    fn make_key(key_type: &str, key_id: &str) -> String {
        format!("{}:{}", key_type, key_id)
    }
}

impl MlsStorage for InMemoryStorage {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> StorageResult<()> {
        let mut storage = self
            .data
            .write()
            .map_err(|e| StorageError::StoreFailed(e.to_string()))?;
        storage.insert(Self::make_key(key_type, key_id), data.to_vec());
        Ok(())
    }

    fn load(&self, key_type: &str, key_id: &str) -> StorageResult<Option<Vec<u8>>> {
        let storage = self
            .data
            .read()
            .map_err(|e| StorageError::LoadFailed(e.to_string()))?;
        Ok(storage.get(&Self::make_key(key_type, key_id)).cloned())
    }

    fn delete(&self, key_type: &str, key_id: &str) -> StorageResult<()> {
        let mut storage = self
            .data
            .write()
            .map_err(|e| StorageError::DeleteFailed(e.to_string()))?;
        storage.remove(&Self::make_key(key_type, key_id));
        Ok(())
    }

    fn list_keys(&self, key_type: &str) -> StorageResult<Vec<String>> {
        let storage = self
            .data
            .read()
            .map_err(|e| StorageError::LoadFailed(e.to_string()))?;

        let prefix = format!("{}:", key_type);
        let keys: Vec<String> = storage
            .keys()
            .filter_map(|k| {
                if k.starts_with(&prefix) {
                    Some(k[prefix.len()..].to_string())
                } else {
                    None
                }
            })
            .collect();

        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_storage_basic_operations() {
        let storage = InMemoryStorage::new();

        // Store and load
        storage.store("test", "key1", b"value1").unwrap();
        let loaded = storage.load("test", "key1").unwrap();
        assert_eq!(loaded, Some(b"value1".to_vec()));

        // Load non-existent
        let loaded = storage.load("test", "key2").unwrap();
        assert_eq!(loaded, None);

        // Delete
        storage.delete("test", "key1").unwrap();
        let loaded = storage.load("test", "key1").unwrap();
        assert_eq!(loaded, None);
    }

    #[test]
    fn test_in_memory_storage_list_keys() {
        let storage = InMemoryStorage::new();

        storage.store("type_a", "key1", b"v1").unwrap();
        storage.store("type_a", "key2", b"v2").unwrap();
        storage.store("type_b", "key3", b"v3").unwrap();

        let mut keys_a = storage.list_keys("type_a").unwrap();
        keys_a.sort();
        assert_eq!(keys_a, vec!["key1", "key2"]);

        let keys_b = storage.list_keys("type_b").unwrap();
        assert_eq!(keys_b, vec!["key3"]);

        let keys_c = storage.list_keys("type_c").unwrap();
        assert!(keys_c.is_empty());
    }

    #[test]
    fn test_in_memory_storage_exists() {
        let storage = InMemoryStorage::new();

        assert!(!storage.exists("test", "key1").unwrap());

        storage.store("test", "key1", b"value").unwrap();
        assert!(storage.exists("test", "key1").unwrap());

        storage.delete("test", "key1").unwrap();
        assert!(!storage.exists("test", "key1").unwrap());
    }

    #[test]
    fn test_in_memory_storage_clear_type() {
        let storage = InMemoryStorage::new();

        storage.store("type_a", "key1", b"v1").unwrap();
        storage.store("type_a", "key2", b"v2").unwrap();
        storage.store("type_b", "key3", b"v3").unwrap();

        storage.clear_type("type_a").unwrap();

        assert!(storage.list_keys("type_a").unwrap().is_empty());
        assert_eq!(storage.list_keys("type_b").unwrap().len(), 1);
    }
}
