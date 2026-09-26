//! A portable conformance suite for [`MlsStorage`] implementations.
//!
//! Every binding hands the SDK its own `MlsStorage`: the Keychain on iOS, an
//! encrypted preferences file on Android, the platform credential store in
//! Python, and whatever a Rust host wires in. Each is a promise that bytes
//! stored under `(key_type, key_id)` come back whole, that categories do not
//! bleed into each other, and that an overwrite replaces. Nothing checks the
//! promise: an implementation that returns `Ok` from every method can still
//! drop an overwrite, split a key id on the `:` a session id carries, or
//! truncate a ratchet tree, and the symptom arrives much later as a group
//! that cannot decrypt.
//!
//! Green here is the definition of "this backend is supported". The suite
//! writes only under its own probe key types and deletes everything it
//! wrote, so it is safe to run against a live store.
//!
//! The report type is shared with the protocol-state suite in the engine
//! crate, which re-exports it, so the two suites render one JSON shape:
//! `{"passed":[...],"failures":[{"check","detail"}]}`.

use serde::{Deserialize, Serialize};

use crate::storage::{MlsStorage, StorageError};
use crate::GroupId;

/// The key type every probe record is written under.
///
/// Deliberately not a real category: nothing else reads it, and a leftover
/// record from an interrupted run is inert.
pub const CONFORMANCE_KEY_TYPE: &str = "mls_storage_conformance_probe";

/// A second key type, used to prove categories do not bleed into each other.
pub const CONFORMANCE_KEY_TYPE_OTHER: &str = "mls_storage_conformance_probe_other";

/// A key type the suite never writes, used to prove listing and clearing an
/// untouched category is empty and not an error.
const CONFORMANCE_KEY_TYPE_UNUSED: &str = "mls_storage_conformance_probe_unused";

/// The largest value the suite writes.
///
/// The largest record the SDK stores through this trait is a group's ratchet
/// tree, which grows by about 1.1 KiB per member: 74 KiB at 65 members and
/// 270 KiB at the default `max_group_members` of 256 (measured on the
/// serialized OpenMLS tree). Half a MiB covers the default with headroom. A
/// deployment that raises the member cap scales the record linearly and
/// should probe its backend at its own ceiling.
pub const LARGE_RECORD_BYTES: usize = 512 * 1024;

/// Writers in the concurrency check.
const CONCURRENT_WRITERS: usize = 8;
/// Overwrites each writer performs.
const WRITES_PER_WRITER: usize = 16;
/// Size of each value under concurrent overwrite. Large enough that a
/// backend copying without a lock has a window to be caught in.
const CONCURRENT_VALUE_BYTES: usize = 4096;

/// One failed check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformanceFailure {
    /// The check that failed.
    pub check: String,
    /// What went wrong, in enough detail to fix the adapter.
    pub detail: String,
}

/// The result of running a suite against one backend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConformanceReport {
    /// Checks that passed, in run order.
    pub passed: Vec<String>,
    /// Checks that failed.
    pub failures: Vec<ConformanceFailure>,
}

impl ConformanceReport {
    /// An empty report.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the backend is supported.
    pub fn is_green(&self) -> bool {
        self.failures.is_empty()
    }

    /// A one-line summary suitable for a test assertion message.
    pub fn summary(&self) -> String {
        if self.is_green() {
            format!("{} checks passed", self.passed.len())
        } else {
            let names: Vec<&str> = self
                .failures
                .iter()
                .map(|failure| failure.check.as_str())
                .collect();
            format!(
                "{} of {} checks failed: {}",
                self.failures.len(),
                self.passed.len() + self.failures.len(),
                names.join(", ")
            )
        }
    }

    /// Records a passed check.
    pub fn pass(&mut self, check: &str) {
        self.passed.push(check.to_string());
    }

    /// Records a failed check.
    pub fn fail(&mut self, check: &str, detail: impl Into<String>) {
        self.failures.push(ConformanceFailure {
            check: check.to_string(),
            detail: detail.into(),
        });
    }

    /// Records one check's outcome.
    pub fn check(&mut self, name: &str, outcome: Result<(), String>) {
        match outcome {
            Ok(()) => self.pass(name),
            Err(detail) => self.fail(name, detail),
        }
    }
}

fn expect_load(
    storage: &dyn MlsStorage,
    key_type: &str,
    key_id: &str,
    expected: &[u8],
) -> Result<(), String> {
    match storage.load(key_type, key_id) {
        Ok(Some(actual)) if actual == expected => Ok(()),
        Ok(Some(actual)) => Err(format!(
            "loaded {} bytes, expected {} (contents differ)",
            actual.len(),
            expected.len()
        )),
        Ok(None) => Err("stored value loaded back as missing".to_string()),
        Err(err) => Err(format!("load failed: {err}")),
    }
}

fn store_and_read_back(
    storage: &dyn MlsStorage,
    key_id: &str,
    value: &[u8],
    what: &str,
) -> Result<(), String> {
    storage
        .store(CONFORMANCE_KEY_TYPE, key_id, value)
        .map_err(|err| format!("store of {what} failed: {err}"))?;
    expect_load(storage, CONFORMANCE_KEY_TYPE, key_id, value)
        .map_err(|detail| format!("{what}: {detail}"))
}

/// Run the suite.
///
/// Never panics on its own account: an adapter that misbehaves produces
/// failures in the report, because the whole point is to hand an adapter
/// author a list of what to fix rather than a stack trace.
pub fn run(storage: &dyn MlsStorage) -> ConformanceReport {
    let mut report = ConformanceReport::new();

    // Anything left from an interrupted earlier run would make the listing
    // checks report failures that are not the adapter's fault.
    cleanup(storage);

    // 1. The basic contract.
    report.check(
        "store_then_load",
        store_and_read_back(storage, "basic", b"hello", "a value"),
    );

    // 2. Values are bytes, not text. Every record here is either a key or
    // ciphertext; a backend that round-trips through a string type mangles
    // both, and the symptom is a group that will not open.
    let binary: Vec<u8> = (0u8..=255).collect();
    report.check(
        "binary_values_round_trip",
        store_and_read_back(storage, "binary", &binary, "a binary value"),
    );

    // 3. An empty value is a value, and must not read back as absent.
    report.check(
        "empty_value_is_not_missing",
        store_and_read_back(storage, "empty", b"", "an empty value"),
    );

    // 4. A second store replaces; it does not append or refuse. Every
    // epoch rewrites a group's secrets under the same id, so a backend
    // that keeps the first write serves last epoch's keys forever.
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, "overwrite", b"first")
        .and_then(|()| storage.store(CONFORMANCE_KEY_TYPE, "overwrite", b"second"))
        .map_err(|err| format!("store failed: {err}"))
        .and_then(|()| expect_load(storage, CONFORMANCE_KEY_TYPE, "overwrite", b"second"));
    report.check("store_overwrites", outcome);

    // 5. A key that was never written reads as missing, not as an error.
    // The manager probes for a session before creating one; an error here
    // is a failure to send, not "no session yet".
    let outcome = match storage.load(CONFORMANCE_KEY_TYPE, "never_written") {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err("a key that was never written returned a value".to_string()),
        Err(StorageError::KeyNotFound(_)) => {
            Err("absent keys must load as Ok(None), not KeyNotFound".to_string())
        }
        Err(err) => Err(format!("load of an absent key failed: {err}")),
    };
    report.check("absent_key_loads_as_none", outcome);

    // 6. Delete removes, and deleting again is not an error. A consumed key
    // package is deleted on the path that spent it; a second delete after a
    // crash must not turn a clean recovery into a failure.
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, "deletable", b"x")
        .and_then(|()| storage.delete(CONFORMANCE_KEY_TYPE, "deletable"))
        .map_err(|err| format!("store/delete failed: {err}"))
        .and_then(|()| match storage.load(CONFORMANCE_KEY_TYPE, "deletable") {
            Ok(None) => Ok(()),
            Ok(Some(_)) => Err("value survived a delete".to_string()),
            Err(err) => Err(format!("load after delete failed: {err}")),
        })
        .and_then(
            |()| match storage.delete(CONFORMANCE_KEY_TYPE, "deletable") {
                Ok(()) => Ok(()),
                Err(err) => Err(format!("second delete must succeed, got: {err}")),
            },
        );
    report.check("delete_removes_and_is_idempotent", outcome);

    // 7. Key types are separate namespaces. A group's id keys its state,
    // its metadata and its OpenMLS records in different categories; a
    // backend that merges them serves one where another was asked for.
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, "shared_id", b"mine")
        .and_then(|()| storage.store(CONFORMANCE_KEY_TYPE_OTHER, "shared_id", b"theirs"))
        .map_err(|err| format!("store failed: {err}"))
        .and_then(|()| expect_load(storage, CONFORMANCE_KEY_TYPE, "shared_id", b"mine"))
        .and_then(|()| {
            expect_load(storage, CONFORMANCE_KEY_TYPE_OTHER, "shared_id", b"theirs")
                .map_err(|_| "the same key id in two key types collided".to_string())
        });
    report.check("key_types_are_separate_namespaces", outcome);

    // 8. Listing names what is there, and only within its own key type.
    // Listing is how the manager finds every group on restart and every
    // key package to purge; an omitted id is a group that vanishes.
    let outcome = (|| -> Result<(), String> {
        let keys = storage
            .list_keys(CONFORMANCE_KEY_TYPE)
            .map_err(|err| format!("list_keys failed: {err}"))?;
        for expected in ["basic", "binary", "empty", "overwrite", "shared_id"] {
            if !keys.iter().any(|key| key == expected) {
                return Err(format!("list_keys omitted {expected:?}"));
            }
        }
        if keys.iter().any(|key| key == "deletable") {
            return Err("list_keys named a deleted record".to_string());
        }
        let other = storage
            .list_keys(CONFORMANCE_KEY_TYPE_OTHER)
            .map_err(|err| format!("list_keys failed: {err}"))?;
        if other.len() != 1 || other.first().map(String::as_str) != Some("shared_id") {
            return Err(format!("list_keys leaked across key types: {other:?}"));
        }
        Ok(())
    })();
    report.check("list_keys_is_accurate_and_scoped", outcome);

    // 9. Listing an untouched key type is empty, not an error. A fresh
    // install lists every category before it has written one.
    let outcome = match storage.list_keys(CONFORMANCE_KEY_TYPE_UNUSED) {
        Ok(keys) if keys.is_empty() => Ok(()),
        Ok(keys) => Err(format!("an unused key type listed {} keys", keys.len())),
        Err(err) => Err(format!("list_keys on an unused key type failed: {err}")),
    };
    report.check("unused_key_type_lists_empty", outcome);

    // 10. Key ids as the SDK actually writes them: a UUID for a key
    // package, a 64-character digest for an OpenMLS record, and a session
    // id, which is `session:<address>:<address>`. The colons are the one
    // that bites: a backend that composes `type:id` into one string and
    // splits it back on the first colon returns the wrong id from
    // `list_keys`, and every 1:1 session is lost on restart.
    let session_id = "session:off1qq6k7tx3adkz9gkvwt6c5c4v8lkh5g66ru6jm3s0dm\
                      :off1qq7wtkw2pmld2pcj3zn5fvhhj3kj5sgnlwq0mnh2yg7d";
    let ids = [
        ("a key package id", "8f2c4d3a-4b6e-4c1f-9d0a-1e2f3a4b5c6d"),
        (
            "an OpenMLS record id",
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        ),
        ("a session id", session_id),
    ];
    let outcome = (|| -> Result<(), String> {
        for (what, id) in ids {
            store_and_read_back(storage, id, what.as_bytes(), what)?;
        }
        let keys = storage
            .list_keys(CONFORMANCE_KEY_TYPE)
            .map_err(|err| format!("list_keys failed: {err}"))?;
        for (what, id) in ids {
            if !keys.iter().any(|key| key == id) {
                return Err(format!("list_keys did not return {what} verbatim"));
            }
        }
        Ok(())
    })();
    report.check("key_ids_as_the_sdk_writes_them", outcome);

    // 11. The longest key id the SDK can write: a group id at its cap,
    // which is the wire format's string-field cap, because group state is
    // keyed by the id itself. Probing anything shorter lets a backend with
    // a limit in between pass here and fail in production on a group a
    // peer created.
    let long_key = format!(
        "session:{}",
        "a".repeat(GroupId::MAX_LEN - "session:".len())
    );
    let outcome = (|| -> Result<(), String> {
        if GroupId::new(long_key.as_str()).is_err() {
            return Err("the probe id is not a valid group id; fix the suite".to_string());
        }
        store_and_read_back(storage, &long_key, b"long", "a maximum-length key id")
    })();
    report.check("long_key_ids_round_trip", outcome);

    // 12. A record at the size a large group's ratchet tree reaches. A
    // backend with a value limit in between (a credential store, a
    // preferences string) fails on exactly the record whose loss takes the
    // whole group with it.
    let large = vec![0xa5u8; LARGE_RECORD_BYTES];
    report.check(
        "large_records_round_trip",
        store_and_read_back(storage, "large", &large, "a large record"),
    );

    // 13. `exists` agrees with `load`. It has a default over `load`, so
    // only an override can disagree, and an override that lies makes the
    // manager mint a second identity over the first.
    let outcome = (|| -> Result<(), String> {
        let describe = |err: StorageError| format!("exists failed: {err}");
        if storage
            .exists(CONFORMANCE_KEY_TYPE, "exists_probe")
            .map_err(describe)?
        {
            return Err("exists reported a key that was never written".to_string());
        }
        storage
            .store(CONFORMANCE_KEY_TYPE, "exists_probe", b"x")
            .map_err(|err| format!("store failed: {err}"))?;
        if !storage
            .exists(CONFORMANCE_KEY_TYPE, "exists_probe")
            .map_err(describe)?
        {
            return Err("exists missed a key that was just stored".to_string());
        }
        if storage
            .exists(CONFORMANCE_KEY_TYPE_OTHER, "exists_probe")
            .map_err(describe)?
        {
            return Err("exists found a key under the wrong key type".to_string());
        }
        storage
            .delete(CONFORMANCE_KEY_TYPE, "exists_probe")
            .map_err(|err| format!("delete failed: {err}"))?;
        if storage
            .exists(CONFORMANCE_KEY_TYPE, "exists_probe")
            .map_err(describe)?
        {
            return Err("exists reported a deleted key".to_string());
        }
        Ok(())
    })();
    report.check("exists_agrees_with_load", outcome);

    // 14. `clear_type` empties one category and only that one. It is how
    // the manager drops every key package at once; a backend that clears
    // everything takes the identity with them.
    let outcome = (|| -> Result<(), String> {
        storage
            .store(CONFORMANCE_KEY_TYPE_OTHER, "survivor", b"kept")
            .map_err(|err| format!("store failed: {err}"))?;
        storage
            .clear_type(CONFORMANCE_KEY_TYPE)
            .map_err(|err| format!("clear_type failed: {err}"))?;
        let cleared = storage
            .list_keys(CONFORMANCE_KEY_TYPE)
            .map_err(|err| format!("list_keys failed: {err}"))?;
        if !cleared.is_empty() {
            return Err(format!(
                "{} records survived clear_type on their own key type",
                cleared.len()
            ));
        }
        expect_load(storage, CONFORMANCE_KEY_TYPE_OTHER, "survivor", b"kept")
            .map_err(|_| "clear_type reached into another key type".to_string())?;
        expect_load(storage, CONFORMANCE_KEY_TYPE_OTHER, "shared_id", b"theirs")
            .map_err(|_| "clear_type reached into another key type".to_string())?;
        storage
            .clear_type(CONFORMANCE_KEY_TYPE)
            .map_err(|err| format!("clear_type on an emptied key type failed: {err}"))?;
        storage
            .clear_type(CONFORMANCE_KEY_TYPE_UNUSED)
            .map_err(|err| format!("clear_type on an unused key type failed: {err}"))?;
        Ok(())
    })();
    report.check("clear_type_is_scoped_and_idempotent", outcome);

    // 15. Writes from several threads land whole. The trait requires
    // `Send + Sync` and a per-entry atomic commit; the engine takes it at
    // its word and stores from the event thread while the manager reads
    // from a caller's. A backend that copies without a lock hands back a
    // value that is half one epoch and half the next.
    report.check("concurrent_writes_land_whole", concurrent_writes(storage));

    // 16. Everything written can be found and removed. This is the logout
    // contract: wiping an account has to actually empty the backend.
    let outcome = (|| -> Result<(), String> {
        cleanup(storage);
        for key_type in [CONFORMANCE_KEY_TYPE, CONFORMANCE_KEY_TYPE_OTHER] {
            let remaining = storage
                .list_keys(key_type)
                .map_err(|err| format!("list_keys failed: {err}"))?;
            if !remaining.is_empty() {
                return Err(format!(
                    "{} records survived a listed delete in {key_type}",
                    remaining.len()
                ));
            }
        }
        Ok(())
    })();
    report.check("listed_records_can_all_be_deleted", outcome);

    report
}

/// The concurrency check: each writer overwrites its own key and one key
/// shared by all of them, reading the shared key back between writes.
///
/// Every value is one writer's fill byte repeated, so a torn read is any
/// value that is not exactly one writer's; nothing has to be timed.
fn concurrent_writes(storage: &dyn MlsStorage) -> Result<(), String> {
    const SHARED_KEY: &str = "concurrent_shared";
    let shared_values: Vec<Vec<u8>> = (0..CONCURRENT_WRITERS)
        .map(|writer| vec![writer as u8 + 1; CONCURRENT_VALUE_BYTES])
        .collect();

    let own_value = |writer: usize, round: usize| -> Vec<u8> {
        let mut value = vec![writer as u8 + 1; CONCURRENT_VALUE_BYTES];
        value[0] = round as u8;
        value
    };

    let outcomes: Vec<Result<(), String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..CONCURRENT_WRITERS)
            .map(|writer| {
                let shared_values = &shared_values;
                let own_value = &own_value;
                scope.spawn(move || -> Result<(), String> {
                    let own_key = format!("concurrent_writer_{writer}");
                    for round in 0..WRITES_PER_WRITER {
                        storage
                            .store(CONFORMANCE_KEY_TYPE, &own_key, &own_value(writer, round))
                            .map_err(|err| format!("writer {writer}: store failed: {err}"))?;
                        storage
                            .store(CONFORMANCE_KEY_TYPE, SHARED_KEY, &shared_values[writer])
                            .map_err(|err| format!("writer {writer}: store failed: {err}"))?;
                        match storage.load(CONFORMANCE_KEY_TYPE, SHARED_KEY) {
                            Ok(Some(seen)) if shared_values.contains(&seen) => {}
                            Ok(Some(seen)) => {
                                return Err(format!(
                                    "writer {writer} read a torn value ({} bytes) from a key \
                                     under concurrent overwrite",
                                    seen.len()
                                ))
                            }
                            Ok(None) => {
                                return Err(format!(
                                    "writer {writer} found a key it had just written missing \
                                     under concurrent overwrite"
                                ))
                            }
                            Err(err) => {
                                return Err(format!(
                                    "writer {writer}: load under concurrent writes failed: {err}"
                                ))
                            }
                        }
                    }
                    expect_load(
                        storage,
                        CONFORMANCE_KEY_TYPE,
                        &own_key,
                        &own_value(writer, WRITES_PER_WRITER - 1),
                    )
                    .map_err(|detail| format!("writer {writer}'s own key: {detail}"))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| {
                    Err("a writer thread panicked inside the adapter".to_string())
                })
            })
            .collect()
    });
    for outcome in outcomes {
        outcome?;
    }

    match storage.load(CONFORMANCE_KEY_TYPE, SHARED_KEY) {
        Ok(Some(seen)) if shared_values.contains(&seen) => Ok(()),
        Ok(Some(seen)) => Err(format!(
            "after all writers finished the shared key held a torn value ({} bytes)",
            seen.len()
        )),
        Ok(None) => Err("after all writers finished the shared key was missing".to_string()),
        Err(err) => Err(format!("load after concurrent writes failed: {err}")),
    }
}

/// Remove every record the suite writes, in both key types.
fn cleanup(storage: &dyn MlsStorage) {
    for key_type in [CONFORMANCE_KEY_TYPE, CONFORMANCE_KEY_TYPE_OTHER] {
        if let Ok(keys) = storage.list_keys(key_type) {
            for key in keys {
                let _ = storage.delete(key_type, &key);
            }
        }
    }
}

/// Run the suite and render the report as JSON.
///
/// The same shape the protocol-state suite renders, so one reader serves
/// both reports.
pub fn run_json(storage: &dyn MlsStorage) -> String {
    let report = run(storage);
    serde_json::to_string(&report).unwrap_or_else(|err| {
        format!(
            "{{\"passed\":[],\"failures\":[{{\"check\":\"report_serialization\",\"detail\":\"{}\"}}]}}",
            err.to_string().replace('"', "'")
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::storage::{InMemoryStorage, StorageResult};

    #[test]
    fn the_suite_is_green_against_the_in_memory_store() {
        let storage = InMemoryStorage::new();
        let report = run(&storage);
        assert!(report.is_green(), "{}", report.summary());
        assert_eq!(report.passed.len(), 16, "{:?}", report.passed);
        for key_type in [CONFORMANCE_KEY_TYPE, CONFORMANCE_KEY_TYPE_OTHER] {
            assert!(
                storage.list_keys(key_type).unwrap().is_empty(),
                "the suite left records behind in {key_type}"
            );
        }
    }

    #[test]
    fn the_suite_leaves_records_outside_its_probe_key_types_alone() {
        let storage = InMemoryStorage::new();
        storage.store("identity", "key_pair", b"secret").unwrap();
        let report = run(&storage);
        assert!(report.is_green(), "{}", report.summary());
        assert_eq!(
            storage.load("identity", "key_pair").unwrap(),
            Some(b"secret".to_vec())
        );
        assert_eq!(storage.list_keys("identity").unwrap(), vec!["key_pair"]);
    }

    #[test]
    fn the_report_renders_the_documented_json_shape() {
        let json = run_json(&InMemoryStorage::new());
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["passed"].as_array().is_some_and(|p| p.len() == 16));
        assert!(parsed["failures"].as_array().is_some_and(Vec::is_empty));

        let mut report = ConformanceReport::new();
        report.fail("x", "why");
        let parsed: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(parsed["failures"][0]["check"], "x");
        assert_eq!(parsed["failures"][0]["detail"], "why");
    }

    /// One defect at a time, each a class of bug an adapter can ship with
    /// while returning `Ok` from every method.
    #[derive(Debug, Clone, Copy)]
    enum Fault {
        None,
        KeepsFirstWrite,
        AbsentIsAnError,
        DeleteOfAbsentFails,
        IgnoresKeyType,
        SplitsKeyIdOnColon,
        CapsKeyIdAt255,
        TruncatesValuesAt64KiB,
        ExistsAlwaysFalse,
        ClearTypeClearsEverything,
        TearsOverwrites,
    }

    struct Faulty {
        fault: Fault,
        records: Mutex<HashMap<(String, String), Vec<u8>>>,
        previous: Mutex<HashMap<(String, String), Vec<u8>>>,
    }

    impl Faulty {
        fn new(fault: Fault) -> Self {
            Self {
                fault,
                records: Mutex::default(),
                previous: Mutex::default(),
            }
        }

        fn key(&self, key_type: &str, key_id: &str) -> (String, String) {
            let key_type = match self.fault {
                Fault::IgnoresKeyType => "",
                _ => key_type,
            };
            let key_id = match self.fault {
                Fault::SplitsKeyIdOnColon => key_id.split(':').next().unwrap_or(""),
                _ => key_id,
            };
            (key_type.to_string(), key_id.to_string())
        }
    }

    impl MlsStorage for Faulty {
        fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> StorageResult<()> {
            if matches!(self.fault, Fault::CapsKeyIdAt255) && key_id.len() > 255 {
                return Err(StorageError::StoreFailed("key too long".to_string()));
            }
            let mut data = data.to_vec();
            if matches!(self.fault, Fault::TruncatesValuesAt64KiB) {
                data.truncate(64 * 1024);
            }
            let key = self.key(key_type, key_id);
            let mut records = self.records.lock().unwrap();
            if matches!(self.fault, Fault::KeepsFirstWrite) {
                records.entry(key).or_insert(data);
                return Ok(());
            }
            if let Some(old) = records.insert(key.clone(), data) {
                self.previous.lock().unwrap().insert(key, old);
            }
            Ok(())
        }

        fn load(&self, key_type: &str, key_id: &str) -> StorageResult<Option<Vec<u8>>> {
            let key = self.key(key_type, key_id);
            let current = self.records.lock().unwrap().get(&key).cloned();
            match (self.fault, current) {
                (Fault::AbsentIsAnError, None) => {
                    Err(StorageError::KeyNotFound(key_id.to_string()))
                }
                (Fault::TearsOverwrites, Some(current)) => {
                    // Half the previous value and half the current one: what
                    // an unlocked copy hands a reader mid-overwrite.
                    let previous = self.previous.lock().unwrap().get(&key).cloned();
                    match previous {
                        Some(previous)
                            if previous != current && previous.len() == current.len() =>
                        {
                            let half = current.len() / 2;
                            let mut torn = previous[..half].to_vec();
                            torn.extend_from_slice(&current[half..]);
                            Ok(Some(torn))
                        }
                        _ => Ok(Some(current)),
                    }
                }
                (_, current) => Ok(current),
            }
        }

        fn delete(&self, key_type: &str, key_id: &str) -> StorageResult<()> {
            let key = self.key(key_type, key_id);
            let removed = self.records.lock().unwrap().remove(&key);
            if removed.is_none() && matches!(self.fault, Fault::DeleteOfAbsentFails) {
                return Err(StorageError::KeyNotFound(key_id.to_string()));
            }
            Ok(())
        }

        fn list_keys(&self, key_type: &str) -> StorageResult<Vec<String>> {
            let (key_type, _) = self.key(key_type, "");
            Ok(self
                .records
                .lock()
                .unwrap()
                .keys()
                .filter(|(kt, _)| *kt == key_type)
                .map(|(_, id)| id.clone())
                .collect())
        }

        fn exists(&self, key_type: &str, key_id: &str) -> StorageResult<bool> {
            if matches!(self.fault, Fault::ExistsAlwaysFalse) {
                return Ok(false);
            }
            Ok(self.load(key_type, key_id)?.is_some())
        }

        fn clear_type(&self, key_type: &str) -> StorageResult<()> {
            let mut records = self.records.lock().unwrap();
            if matches!(self.fault, Fault::ClearTypeClearsEverything) {
                records.clear();
                return Ok(());
            }
            let (key_type, _) = self.key(key_type, "");
            records.retain(|(kt, _), _| *kt != key_type);
            Ok(())
        }
    }

    #[test]
    fn the_fault_injecting_store_is_green_with_no_fault() {
        // Otherwise a failure below could be the harness's, not the fault's.
        let report = run(&Faulty::new(Fault::None));
        assert!(report.is_green(), "{}", report.summary());
    }

    #[test]
    fn the_suite_catches_each_defect_class() {
        // The negative controls. A suite that passes everything proves
        // nothing, so each check is shown failing the defect it exists for.
        let cases = [
            (Fault::KeepsFirstWrite, "store_overwrites"),
            (Fault::AbsentIsAnError, "absent_key_loads_as_none"),
            (
                Fault::DeleteOfAbsentFails,
                "delete_removes_and_is_idempotent",
            ),
            (Fault::IgnoresKeyType, "key_types_are_separate_namespaces"),
            (Fault::SplitsKeyIdOnColon, "key_ids_as_the_sdk_writes_them"),
            (Fault::CapsKeyIdAt255, "long_key_ids_round_trip"),
            (Fault::TruncatesValuesAt64KiB, "large_records_round_trip"),
            (Fault::ExistsAlwaysFalse, "exists_agrees_with_load"),
            (
                Fault::ClearTypeClearsEverything,
                "clear_type_is_scoped_and_idempotent",
            ),
            (Fault::TearsOverwrites, "concurrent_writes_land_whole"),
        ];
        for (fault, expected) in cases {
            let report = run(&Faulty::new(fault));
            assert!(!report.is_green(), "{fault:?} passed the suite");
            assert!(
                report.failures.iter().any(|f| f.check == expected),
                "{fault:?}: expected {expected} to fail, got: {}",
                report.summary()
            );
        }
    }

    #[test]
    fn the_absent_key_check_names_the_wrong_error_variant() {
        let report = run(&Faulty::new(Fault::AbsentIsAnError));
        let failure = report
            .failures
            .iter()
            .find(|f| f.check == "absent_key_loads_as_none")
            .expect("the check failed");
        assert!(failure.detail.contains("KeyNotFound"), "{}", failure.detail);
    }
}
