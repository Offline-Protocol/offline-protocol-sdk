//! Protocol state held in memory.
//!
//! The first browser store, and an honest one: it keeps nothing past the
//! worker, so a reload is a new device with a new address. It exists so the
//! engine can run before the durable store does, and it passes the
//! protocol-state conformance suite, which is what makes it a store at all.
//!
//! MLS material is held by the engine's own
//! [`offline_protocol::mls::InMemoryStorage`]. The two are separate instances:
//! one backend behind both handles is a test-fixture pattern the engine warns
//! against, because the categories of one can collide with the other's.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use offline_protocol::{ProtocolStateError, ProtocolStateResult, ProtocolStateStorage};

type Records = BTreeMap<(String, String), Vec<u8>>;

/// [`ProtocolStateStorage`] in a map. Keyed by `(key_type, key_id)` as a pair,
/// never as a joined string, so no separator can make two categories collide.
#[derive(Debug, Default)]
pub struct MemoryProtocolStateStorage {
    records: Mutex<Records>,
}

impl MemoryProtocolStateStorage {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn records(&self) -> ProtocolStateResult<std::sync::MutexGuard<'_, Records>> {
        // wasm32-unknown-unknown aborts on a panic, so a poisoned lock cannot
        // be observed there; on a host it means a test thread panicked.
        self.records
            .lock()
            .map_err(|_| ProtocolStateError::LoadFailed("memory store lock poisoned".to_string()))
    }
}

impl ProtocolStateStorage for MemoryProtocolStateStorage {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
        self.records()?
            .insert((key_type.to_string(), key_id.to_string()), data.to_vec());
        Ok(())
    }

    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
        Ok(self
            .records()?
            .get(&(key_type.to_string(), key_id.to_string()))
            .cloned())
    }

    fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
        self.records()?
            .remove(&(key_type.to_string(), key_id.to_string()));
        Ok(())
    }

    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
        let keys: BTreeSet<String> = self
            .records()?
            .keys()
            .filter(|(kind, _)| kind == key_type)
            .map(|(_, id)| id.clone())
            .collect();
        Ok(keys.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_store_passes_the_conformance_suite() {
        let store = MemoryProtocolStateStorage::new();
        let report = offline_protocol::storage_conformance::run(&store);
        assert!(report.is_green(), "{}", report.summary());
    }

    #[test]
    fn the_engine_memory_mls_store_passes_its_conformance_suite() {
        let store = offline_protocol::mls::InMemoryStorage::new();
        let report = offline_protocol::mls_storage_conformance::run(&store);
        assert!(report.is_green(), "{}", report.summary());
    }
}
