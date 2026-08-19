//! A portable conformance suite for [`ProtocolStateStorage`] implementations.
//!
//! Swappable storage is only a property if it can be verified. Without a
//! suite, "bring your own backend" is a promise: an adapter that looks right
//! and returns `Ok` from every method can still lose records under an
//! overwrite, leak keys between categories, or truncate a value at some
//! internal limit, and the symptom arrives much later as data that quietly
//! is not there.
//!
//! Green here is the definition of "this backend is supported".
//!
//! The suite drives the trait directly rather than going through the
//! sealing chokepoint, because what is under test is the adapter's contract:
//! bytes in, the same bytes out, addressed by `(key_type, key_id)`. It
//! writes only under its own key type, and deletes everything it wrote.

use serde::{Deserialize, Serialize};

use crate::protocol_state_storage::{
    ProtocolStateError, ProtocolStateStorage, MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES,
};

/// The key type every probe record is written under.
///
/// Deliberately not a real category: nothing else reads it, and a leftover
/// record from an interrupted run is inert.
pub const CONFORMANCE_KEY_TYPE: &str = "storage_conformance_probe";

/// A second key type, used to prove categories do not bleed into each other.
pub const CONFORMANCE_KEY_TYPE_OTHER: &str = "storage_conformance_probe_other";

/// One failed check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformanceFailure {
    /// The check that failed.
    pub check: String,
    /// What went wrong, in enough detail to fix the adapter.
    pub detail: String,
}

/// The result of running the suite against one backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformanceReport {
    /// Checks that passed, in run order.
    pub passed: Vec<String>,
    /// Checks that failed.
    pub failures: Vec<ConformanceFailure>,
}

impl ConformanceReport {
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

    fn pass(&mut self, check: &str) {
        self.passed.push(check.to_string());
    }

    fn fail(&mut self, check: &str, detail: impl Into<String>) {
        self.failures.push(ConformanceFailure {
            check: check.to_string(),
            detail: detail.into(),
        });
    }

    fn check(&mut self, name: &str, outcome: std::result::Result<(), String>) {
        match outcome {
            Ok(()) => self.pass(name),
            Err(detail) => self.fail(name, detail),
        }
    }
}

fn expect_load(
    storage: &dyn ProtocolStateStorage,
    key_id: &str,
    expected: &[u8],
) -> std::result::Result<(), String> {
    match storage.load(CONFORMANCE_KEY_TYPE, key_id) {
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

/// Run the suite.
///
/// Never panics: an adapter that misbehaves produces failures in the report,
/// because the whole point is to hand an adapter author a list of what to
/// fix rather than a stack trace.
pub fn run(storage: &dyn ProtocolStateStorage) -> ConformanceReport {
    let mut report = ConformanceReport {
        passed: Vec::new(),
        failures: Vec::new(),
    };

    // Anything left from an interrupted earlier run would make the listing
    // checks report failures that are not the adapter's fault.
    cleanup(storage);

    // 1. The basic contract.
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, "basic", b"hello")
        .map_err(|err| format!("store failed: {err}"))
        .and_then(|()| expect_load(storage, "basic", b"hello"));
    report.check("store_then_load", outcome);

    // 2. Values are bytes, not text. A backend that round-trips through a
    // string type mangles these, and the failure is invisible until a
    // sealed record (which is ciphertext) comes back wrong.
    let binary: Vec<u8> = (0u8..=255).collect();
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, "binary", &binary)
        .map_err(|err| format!("store failed: {err}"))
        .and_then(|()| expect_load(storage, "binary", &binary));
    report.check("binary_values_round_trip", outcome);

    // 3. An empty value is a value, and must not read back as absent.
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, "empty", b"")
        .map_err(|err| format!("store failed: {err}"))
        .and_then(|()| expect_load(storage, "empty", b""));
    report.check("empty_value_is_not_missing", outcome);

    // 4. A second store replaces; it does not append or refuse.
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, "overwrite", b"first")
        .and_then(|()| storage.store(CONFORMANCE_KEY_TYPE, "overwrite", b"second"))
        .map_err(|err| format!("store failed: {err}"))
        .and_then(|()| expect_load(storage, "overwrite", b"second"));
    report.check("store_overwrites", outcome);

    // 5. A key that was never written reads as missing, not as an error.
    let outcome = match storage.load(CONFORMANCE_KEY_TYPE, "never_written") {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err("a key that was never written returned a value".to_string()),
        Err(ProtocolStateError::NotFound(_)) => {
            Err("absent keys must load as Ok(None), not NotFound".to_string())
        }
        Err(err) => Err(format!("load of an absent key failed: {err}")),
    };
    report.check("absent_key_loads_as_none", outcome);

    // 6. Delete removes, and deleting again is not an error. The data layer
    // deletes folded delta records after a crash may already have taken
    // them, so a delete that fails on an absent key turns a clean recovery
    // into a spurious failure.
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

    // 7. Key types are separate namespaces. The data layer keeps documents,
    // their delta logs and space indexes in three of them; a backend that
    // merges them would serve a delta where a document was asked for.
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, "shared_id", b"mine")
        .and_then(|()| storage.store(CONFORMANCE_KEY_TYPE_OTHER, "shared_id", b"theirs"))
        .map_err(|err| format!("store failed: {err}"))
        .and_then(|()| expect_load(storage, "shared_id", b"mine"))
        .and_then(
            |()| match storage.load(CONFORMANCE_KEY_TYPE_OTHER, "shared_id") {
                Ok(Some(value)) if value == b"theirs" => Ok(()),
                Ok(_) => Err("the same key id in two key types collided".to_string()),
                Err(err) => Err(format!("load failed: {err}")),
            },
        );
    report.check("key_types_are_separate_namespaces", outcome);

    // 8. Listing names what is there, and only within its own key type.
    let outcome = (|| -> std::result::Result<(), String> {
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

    // 9. Listing an untouched key type is empty, not an error.
    let outcome = match storage.list_keys("storage_conformance_probe_unused") {
        Ok(keys) if keys.is_empty() => Ok(()),
        Ok(keys) => Err(format!("an unused key type listed {} keys", keys.len())),
        Err(err) => Err(format!("list_keys on an unused key type failed: {err}")),
    };
    report.check("unused_key_type_lists_empty", outcome);

    // 10. Key ids as the SDK actually composes them. Document records are
    // keyed `{space}/{doc}` and delta records `{space}/{doc}/{seq}`, so a
    // backend that treats a key id as a filesystem path, or rejects the
    // separator, breaks the data layer specifically.
    let composed = "space-1/doc.name_2/000000000000002a";
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, composed, b"composed")
        .map_err(|err| format!("store with a composed key failed: {err}"))
        .and_then(|()| expect_load(storage, composed, b"composed"))
        .and_then(|()| {
            let keys = storage
                .list_keys(CONFORMANCE_KEY_TYPE)
                .map_err(|err| format!("list_keys failed: {err}"))?;
            if keys.iter().any(|key| key == composed) {
                Ok(())
            } else {
                Err("list_keys did not return a composed key verbatim".to_string())
            }
        });
    report.check("composed_key_ids_round_trip", outcome);

    // 11. A long key id. The SDK's are bounded, but a backend with a short
    // internal limit fails silently on the longest legitimate ones.
    let long_key = format!("{}/{}", "s".repeat(128), "d".repeat(128));
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, &long_key, b"long")
        .map_err(|err| format!("store with a long key failed: {err}"))
        .and_then(|()| expect_load(storage, &long_key, b"long"));
    report.check("long_key_ids_round_trip", outcome);

    // 12. A record at the size the SDK is allowed to write. A sealed record
    // may reach the record cap plus the seal's overhead, and the provider's
    // own ceiling is the documented superset of that.
    let large = vec![0xa5u8; MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES / 2];
    let outcome = storage
        .store(CONFORMANCE_KEY_TYPE, "large", &large)
        .map_err(|err| format!("store of a large record failed: {err}"))
        .and_then(|()| expect_load(storage, "large", &large));
    report.check("large_records_round_trip", outcome);

    // 13. Everything written can be found and removed. This is the logout
    // contract: an application pointing documents at a custom backend needs
    // wiping to actually empty it.
    let outcome = (|| -> std::result::Result<(), String> {
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

/// Remove every record the suite writes, in both key types.
fn cleanup(storage: &dyn ProtocolStateStorage) {
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
/// The shape the bindings use: one implementation of the checks, reachable
/// from every language, instead of four that drift.
pub fn run_json(storage: &dyn ProtocolStateStorage) -> String {
    let report = run(storage);
    serde_json::to_string(&report).unwrap_or_else(|err| {
        format!(
            "{{\"passed\":[],\"failures\":[{{\"check\":\"report_serialization\",\"detail\":\"{}\"}}]}}",
            err.to_string().replace('"', "'")
        )
    })
}
