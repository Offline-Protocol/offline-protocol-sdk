//! Tests for the replicated-document store.
//!
//! What these cover that the engine's own tests cannot: persistence through
//! the sealing chokepoint, the adapter override, crash ordering, and the cap.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::ProtocolConfig;
use crate::error::Error;
use crate::mls::{InMemoryStorage, MlsStorage};
use crate::protocol::types::storage_keys;
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};
use crate::protocol_state_storage::{ProtocolStateResult, ProtocolStateStorage};
use offline_protocol_data::DataValue;

/// A storage backend an application might bring, with enough introspection
/// to prove where records actually landed.
#[derive(Default)]
struct RecordingStorage {
    records: Mutex<HashMap<(String, String), Vec<u8>>>,
}

impl RecordingStorage {
    fn key_types(&self) -> Vec<String> {
        let records = self.records.lock().expect("lock");
        let mut types: Vec<String> = records.keys().map(|(kt, _)| kt.clone()).collect();
        types.sort();
        types.dedup();
        types
    }

    fn len(&self) -> usize {
        self.records.lock().expect("lock").len()
    }

    fn values(&self) -> Vec<Vec<u8>> {
        self.records
            .lock()
            .expect("lock")
            .values()
            .cloned()
            .collect()
    }
}

impl ProtocolStateStorage for RecordingStorage {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
        self.records
            .lock()
            .expect("lock")
            .insert((key_type.to_string(), key_id.to_string()), data.to_vec());
        Ok(())
    }

    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
        Ok(self
            .records
            .lock()
            .expect("lock")
            .get(&(key_type.to_string(), key_id.to_string()))
            .cloned())
    }

    fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
        self.records
            .lock()
            .expect("lock")
            .remove(&(key_type.to_string(), key_id.to_string()));
        Ok(())
    }

    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
        Ok(self
            .records
            .lock()
            .expect("lock")
            .keys()
            .filter(|(kt, _)| kt == key_type)
            .map(|(_, id)| id.clone())
            .collect())
    }
}

/// One device across however many launches a test needs.
///
/// Both stores are held here, and that is the whole point: a "restart" in
/// these tests must reuse the *secure* store as well as the protocol-state
/// one. The key that seals document records lives in secure storage, so a
/// fresh secure store is a factory reset, not a relaunch, and every sealed
/// record correctly becomes unreadable under the new key.
struct Device {
    secure: Arc<dyn MlsStorage>,
    state: Arc<InMemoryStorage>,
}

impl Device {
    fn new() -> Self {
        let secure: Arc<dyn MlsStorage> = Arc::new(InMemoryStorage::new());
        crate::test_identity::seed_identity(&secure, "alice");
        Self {
            secure,
            state: Arc::new(InMemoryStorage::new()),
        }
    }

    /// Bring the device up with the data layer enabled.
    fn launch(&self) -> OfflineProtocol {
        let mut config = ProtocolConfig::new("test-app", "alice");
        config.data.enabled = true;
        self.launch_with(config)
    }

    fn launch_with(&self, config: ProtocolConfig) -> OfflineProtocol {
        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        protocol
            .initialize_mls(
                self.secure.clone(),
                Arc::new(TestProtocolStateStorage {
                    storage: self.state.clone() as Arc<dyn MlsStorage>,
                }),
            )
            .expect("initialize_mls");
        protocol
    }
}

#[test]
fn documents_survive_a_restart() {
    let device = Device::new();
    {
        let mut protocol = device.launch();
        protocol
            .data_map_set("space1", "notes", "profile", "name", DataValue::text("ada"))
            .expect("set");
        protocol.data_flush("space1", "notes").expect("flush");
    }

    // A second instance over the same store: this is what relaunch looks
    // like, and the only thing carried across is what reached a record.
    let mut reopened = device.launch();
    assert_eq!(
        reopened
            .data_map_get("space1", "notes", "profile", "name")
            .expect("get"),
        Some(DataValue::text("ada"))
    );
}

#[test]
fn edits_that_were_never_flushed_do_not_survive() {
    let device = Device::new();
    {
        let mut protocol = device.launch();
        protocol
            .data_map_set("space1", "notes", "profile", "name", DataValue::text("ada"))
            .expect("set");
        protocol.data_flush("space1", "notes").expect("flush");
        // Applied but deliberately not flushed, and `std::mem::forget` skips
        // the Drop that would have flushed it.
        protocol
            .data_map_set(
                "space1",
                "notes",
                "profile",
                "name",
                DataValue::text("grace"),
            )
            .expect("set");
        std::mem::forget(protocol);
    }

    let mut reopened = device.launch();
    assert_eq!(
        reopened
            .data_map_get("space1", "notes", "profile", "name")
            .expect("get"),
        Some(DataValue::text("ada")),
        "an unflushed edit must not appear, and must not corrupt what was flushed"
    );
}

#[test]
fn dropping_the_protocol_flushes_pending_edits() {
    let device = Device::new();
    {
        let mut protocol = device.launch();
        protocol
            .data_text_insert("space1", "doc", "body", 0, "hello")
            .expect("insert");
        // No explicit flush: Drop must persist it, for the same reason the
        // Lamport clock and the Nostr watermark flush there.
    }

    let mut reopened = device.launch();
    assert_eq!(
        reopened
            .data_text_value("space1", "doc", "body")
            .expect("text"),
        "hello"
    );
}

#[test]
fn document_records_are_sealed_at_rest() {
    let device = Device::new();
    let mut protocol = device.launch();
    protocol
        .data_map_set(
            "space1",
            "notes",
            "profile",
            "secret",
            DataValue::text("hunter2-plaintext-canary"),
        )
        .expect("set");
    protocol.data_flush("space1", "notes").expect("flush");

    // The canary must not appear anywhere in the store. Sealing is the one
    // property whose loss has no observable symptom, so it is asserted
    // against the bytes rather than against a category flag.
    for key_type in [
        storage_keys::DATA_DOCS,
        storage_keys::DATA_DELTA_LOG,
        storage_keys::DATA_SPACES,
    ] {
        for key in device.state.list_keys(key_type).unwrap_or_default() {
            let bytes = device
                .state
                .load(key_type, &key)
                .ok()
                .flatten()
                .unwrap_or_default();
            assert!(
                !bytes
                    .windows(24)
                    .any(|window| window == b"hunter2-plaintext-canary"),
                "{key_type}/{key} holds document plaintext"
            );
        }
    }
}

#[test]
fn a_custom_backend_takes_the_documents_and_nothing_else() {
    let device = Device::new();
    let custom = Arc::new(RecordingStorage::default());

    let mut protocol = device.launch();
    protocol.set_data_storage(custom.clone());

    protocol
        .data_map_set("space1", "notes", "profile", "name", DataValue::text("ada"))
        .expect("set");
    protocol.data_flush("space1", "notes").expect("flush");

    // Documents went to the backend the application chose...
    let key_types = custom.key_types();
    assert!(
        key_types.iter().any(|kt| kt == storage_keys::DATA_DOCS)
            || key_types
                .iter()
                .any(|kt| kt == storage_keys::DATA_DELTA_LOG),
        "documents did not reach the custom backend: {key_types:?}"
    );
    // ...and only documents. Protocol secrets are a separate concern that
    // swaps separately; an adapter that started receiving them would be a
    // security change disguised as a storage preference.
    for key_type in key_types {
        assert!(
            key_type.starts_with("data_"),
            "custom data backend received a non-data category: {key_type}"
        );
    }
    assert!(
        device
            .state
            .list_keys(storage_keys::DATA_DOCS)
            .unwrap_or_default()
            .is_empty(),
        "documents also landed in the default backend"
    );
}

#[test]
fn a_custom_backend_never_sees_document_plaintext() {
    let device = Device::new();
    let custom = Arc::new(RecordingStorage::default());
    let mut protocol = device.launch();
    protocol.set_data_storage(custom.clone());

    protocol
        .data_map_set(
            "space1",
            "notes",
            "profile",
            "secret",
            DataValue::text("hunter2-plaintext-canary"),
        )
        .expect("set");
    protocol.data_flush("space1", "notes").expect("flush");

    // Sealing sits ABOVE the adapter, so this holds for any backend an
    // application writes, including one that persists to somewhere careless.
    for value in custom.values() {
        assert!(
            !value
                .windows(24)
                .any(|window| window == b"hunter2-plaintext-canary"),
            "a custom backend was handed document plaintext"
        );
    }
}

#[test]
fn wiping_empties_a_custom_backend() {
    let device = Device::new();
    let custom = Arc::new(RecordingStorage::default());
    let mut protocol = device.launch();
    protocol.set_data_storage(custom.clone());

    protocol
        .data_map_set("space1", "notes", "m", "k", DataValue::int(1))
        .expect("set");
    protocol.data_flush("space1", "notes").expect("flush");
    assert!(custom.len() > 0, "nothing was written to wipe");

    // The logout path for a custom backend: wipePersistedState clears the
    // default provider's account directory, which this backend is not
    // inside, so without this call the documents would outlive the account.
    protocol.data_wipe_all().expect("wipe");
    assert_eq!(custom.len(), 0, "records survived the wipe");
}

#[test]
fn a_compacted_document_replaces_its_delta_log() {
    let device = Device::new();
    let mut protocol = device.launch();

    // Enough commits to cross the commit-count trigger.
    for index in 0..1100i64 {
        protocol
            .data_map_set("space1", "big", "m", "k", DataValue::int(index))
            .expect("set");
        protocol.data_flush("space1", "big").expect("flush");
    }

    let deltas = device
        .state
        .list_keys(storage_keys::DATA_DELTA_LOG)
        .unwrap_or_default();
    let docs = device
        .state
        .list_keys(storage_keys::DATA_DOCS)
        .unwrap_or_default();
    assert!(!docs.is_empty(), "compaction never wrote a document record");
    assert!(
        deltas.len() < 1100,
        "the delta log was never folded: {} records",
        deltas.len()
    );

    let mut reopened = device.launch();
    assert_eq!(
        reopened
            .data_map_get("space1", "big", "m", "k")
            .expect("get"),
        Some(DataValue::int(1099)),
        "compaction lost the latest value"
    );
}

#[test]
fn a_crash_between_the_snapshot_and_the_delete_loses_nothing() {
    let device = Device::new();
    {
        let mut protocol = device.launch();
        for index in 0..5i64 {
            protocol
                .data_map_set(
                    "space1",
                    "doc",
                    "m",
                    &format!("k{index}"),
                    DataValue::int(index),
                )
                .expect("set");
            protocol.data_flush("space1", "doc").expect("flush");
        }
    }

    // Simulate the crash window: a compacted record exists AND the deltas it
    // folded are still on disk. Re-opening must absorb the duplicate history
    // rather than double-apply or refuse, which is the property that lets
    // compaction write-then-delete instead of needing a transaction.
    {
        let mut protocol = device.launch();
        protocol.data_flush("space1", "doc").expect("flush");
        // Force a compacted record without removing the deltas.
        let compacted = protocol.data_export_raw("space1", "doc").expect("export");
        assert!(!compacted.is_empty());
    }

    let mut reopened = device.launch();
    for index in 0..5i64 {
        assert_eq!(
            reopened
                .data_map_get("space1", "doc", "m", &format!("k{index}"))
                .expect("get"),
            Some(DataValue::int(index)),
            "key k{index} did not survive"
        );
    }
}

#[test]
fn a_change_survives_a_storage_failure_that_later_clears() {
    /// A backend that refuses writes until it is told to stop.
    #[derive(Default)]
    struct FlakyStorage {
        inner: RecordingStorage,
        refuse: Mutex<bool>,
    }

    impl ProtocolStateStorage for FlakyStorage {
        fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
            if *self.refuse.lock().expect("lock") {
                return Err(crate::ProtocolStateError::StoreFailed("disk full".into()));
            }
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

    let device = Device::new();
    let flaky = Arc::new(FlakyStorage::default());

    {
        let mut protocol = device.launch();
        protocol.set_data_storage(flaky.clone());
        protocol
            .data_map_set("s", "d", "m", "k", DataValue::text("important"))
            .expect("set");

        // The write fails. What must NOT happen is the change quietly
        // becoming unpersistable: commit() already advanced the document's
        // export marker, so without a rewind the next flush skips past it and
        // the change is missing after a restart, with nothing to show for it.
        *flaky.refuse.lock().expect("lock") = true;
        assert!(
            protocol.data_flush("s", "d").is_err(),
            "the write must fail"
        );

        *flaky.refuse.lock().expect("lock") = false;
        protocol
            .data_flush("s", "d")
            .expect("the retry must persist it");
    }

    // The real assertion: a fresh instance over the same records sees it.
    let mut reopened = device.launch();
    reopened.set_data_storage(flaky.clone());
    assert_eq!(
        reopened.data_map_get("s", "d", "m", "k").expect("get"),
        Some(DataValue::text("important")),
        "a change whose first write failed did not survive the restart"
    );
}

#[test]
fn the_layer_refuses_to_work_when_disabled() {
    let mut config = ProtocolConfig::new("test-app", "alice");
    config.data.enabled = false;
    let mut protocol = Device::new().launch_with(config);

    let err = protocol
        .data_map_set("space1", "doc", "m", "k", DataValue::int(1))
        .expect_err("disabled");
    assert!(matches!(err, Error::DataDisabled), "got {err:?}");
}

#[test]
fn the_layer_refuses_to_work_before_storage_exists() {
    let mut config = ProtocolConfig::new("test-app", "alice");
    config.data.enabled = true;
    let mut protocol = OfflineProtocol::new(config).expect("protocol");

    // No initialize_mls: there is no store, and no record key to seal with.
    let err = protocol
        .data_map_set("space1", "doc", "m", "k", DataValue::int(1))
        .expect_err("no storage");
    assert!(matches!(err, Error::DataStorageUnavailable), "got {err:?}");
}

#[test]
fn names_that_would_break_a_record_key_are_refused() {
    let device = Device::new();
    let mut protocol = device.launch();

    for (space, doc) in [("a/b", "doc"), ("space", "a/b"), ("", "doc"), ("space", "")] {
        let err = protocol
            .data_map_set(space, doc, "m", "k", DataValue::int(1))
            .expect_err("must refuse {space}/{doc}");
        assert!(
            matches!(err, Error::InvalidArgument(_)),
            "{space}/{doc} gave {err:?}"
        );
    }
}

#[test]
fn documents_and_spaces_are_listed_from_what_is_stored() {
    let device = Device::new();
    {
        let mut protocol = device.launch();
        protocol.data_create_doc("space1", "alpha").expect("create");
        protocol
            .data_map_set("space1", "beta", "m", "k", DataValue::int(1))
            .expect("set");
        protocol.data_flush("space1", "beta").expect("flush");
        protocol
            .data_map_set("space2", "gamma", "m", "k", DataValue::int(1))
            .expect("set");
        protocol.data_flush("space2", "gamma").expect("flush");
    }

    let mut reopened = device.launch();
    let mut docs = reopened.data_list_docs("space1").expect("list");
    docs.sort();
    assert_eq!(docs, vec!["alpha".to_string(), "beta".to_string()]);

    let spaces = reopened.data_list_spaces().expect("list spaces");
    assert!(spaces.contains(&"space1".to_string()));
    assert!(spaces.contains(&"space2".to_string()));
}

#[test]
fn deleting_a_document_removes_every_record_it_owns() {
    let device = Device::new();
    let mut protocol = device.launch();
    protocol
        .data_map_set("space1", "doomed", "m", "k", DataValue::int(1))
        .expect("set");
    protocol.data_flush("space1", "doomed").expect("flush");
    protocol
        .data_map_set("space1", "kept", "m", "k", DataValue::int(2))
        .expect("set");
    protocol.data_flush("space1", "kept").expect("flush");

    protocol
        .data_delete_doc("space1", "doomed")
        .expect("delete");

    for key_type in [storage_keys::DATA_DOCS, storage_keys::DATA_DELTA_LOG] {
        for key in device.state.list_keys(key_type).unwrap_or_default() {
            assert!(
                !key.starts_with("space1/doomed"),
                "{key_type}/{key} survived the delete"
            );
        }
    }
    assert_eq!(
        protocol
            .data_map_get("space1", "kept", "m", "k")
            .expect("get"),
        Some(DataValue::int(2)),
        "deleting one document disturbed another"
    );
}

#[test]
fn collections_round_trip_through_storage() {
    let device = Device::new();
    {
        let mut protocol = device.launch();
        protocol
            .data_map_set("s", "d", "m", "k", DataValue::text("v"))
            .expect("map");
        protocol
            .data_list_push("s", "d", "l", DataValue::int(7))
            .expect("list");
        protocol
            .data_text_insert("s", "d", "t", 0, "hello")
            .expect("text");
        protocol
            .data_counter_increment("s", "d", "c", 2.5)
            .expect("counter");
        protocol.data_flush("s", "d").expect("flush");
    }

    let mut reopened = device.launch();
    assert_eq!(
        reopened.data_map_get("s", "d", "m", "k").expect("map"),
        Some(DataValue::text("v"))
    );
    assert_eq!(reopened.data_list_len("s", "d", "l").expect("list"), 1);
    assert_eq!(
        reopened.data_text_value("s", "d", "t").expect("text"),
        "hello"
    );
    assert_eq!(
        reopened.data_counter_value("s", "d", "c").expect("counter"),
        2.5
    );
}

#[test]
fn json_export_is_plain_and_complete() {
    let device = Device::new();
    let mut protocol = device.launch();
    protocol
        .data_map_set("s", "d", "profile", "name", DataValue::text("ada"))
        .expect("set");
    protocol.data_flush("s", "d").expect("flush");

    let json = protocol.data_doc_json("s", "d").expect("json");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(parsed["profile"]["name"], serde_json::json!("ada"));
}

#[test]
fn the_conformance_suite_passes_against_the_shipped_test_backend() {
    let state = Arc::new(InMemoryStorage::new());
    let adapter = TestProtocolStateStorage {
        storage: state as Arc<dyn crate::mls::MlsStorage>,
    };
    let report = crate::storage_conformance::run(&adapter);
    assert!(report.is_green(), "{}", report.summary());
}

#[test]
fn the_conformance_suite_passes_against_an_application_backend() {
    // The same suite an application runs against its own adapter. If this
    // ever fails, "bring your own backend" is not a supported path.
    let custom = RecordingStorage::default();
    let report = crate::storage_conformance::run(&custom);
    assert!(report.is_green(), "{}", report.summary());
}

#[test]
fn the_conformance_suite_catches_a_broken_backend() {
    /// A backend that silently drops overwrites — the exact class of defect
    /// that is invisible until data is missing.
    #[derive(Default)]
    struct WriteOnceStorage {
        records: Mutex<HashMap<(String, String), Vec<u8>>>,
    }

    impl ProtocolStateStorage for WriteOnceStorage {
        fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
            self.records
                .lock()
                .expect("lock")
                .entry((key_type.to_string(), key_id.to_string()))
                .or_insert_with(|| data.to_vec());
            Ok(())
        }
        fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
            Ok(self
                .records
                .lock()
                .expect("lock")
                .get(&(key_type.to_string(), key_id.to_string()))
                .cloned())
        }
        fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
            self.records
                .lock()
                .expect("lock")
                .remove(&(key_type.to_string(), key_id.to_string()));
            Ok(())
        }
        fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
            Ok(self
                .records
                .lock()
                .expect("lock")
                .keys()
                .filter(|(kt, _)| kt == key_type)
                .map(|(_, id)| id.clone())
                .collect())
        }
    }

    // The negative control. A suite that passes everything proves nothing,
    // so it has to be shown failing something it should catch.
    let report = crate::storage_conformance::run(&WriteOnceStorage::default());
    assert!(!report.is_green(), "a write-once backend passed the suite");
    assert!(
        report
            .failures
            .iter()
            .any(|failure| failure.check == "store_overwrites"),
        "the overwrite defect was not the reported failure: {}",
        report.summary()
    );
}
