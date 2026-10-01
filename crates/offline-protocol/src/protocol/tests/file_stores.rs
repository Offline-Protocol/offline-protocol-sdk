//! The built-in file stores under a real engine: what a headless host gets
//! across a restart.
//!
//! The stores' own tests pin their contracts. What only an engine can show is
//! that the pair is enough on its own: the identity comes back (same address),
//! and a record sealed under the record key comes back, which needs the record
//! key from the MLS store and the ciphertext from the state store.

use std::sync::Arc;

use crate::config::ProtocolConfig;
use crate::file_store::test_support::{files_under, TempRoot};
use crate::file_store::{
    account_storage_namespace, FileProtocolStateStorage, FileStoreError, FileStorePair,
    SealedFileMlsStorage, SealedState, StaticStoreKey,
};
use crate::protocol::types::storage_keys;
use crate::protocol::OfflineProtocol;

const APP_ID: &str = "com.example.headless";
const PROFILE: &str = "node";

struct Roots {
    keys: TempRoot,
    state: TempRoot,
}

fn engine(roots: &Roots, key: u8) -> Result<OfflineProtocol, FileStoreError> {
    let namespace = account_storage_namespace(APP_ID, PROFILE);
    let secure = SealedFileMlsStorage::open(
        roots.keys.path(),
        &namespace,
        &StaticStoreKey::new([key; 32]),
    )?;
    let state = FileProtocolStateStorage::open(roots.state.path(), &namespace)?;
    let config = ProtocolConfig::builder(APP_ID, PROFILE)
        .build()
        .expect("config");
    let mut protocol = OfflineProtocol::new(config).expect("engine");
    protocol
        .initialize_mls(Arc::new(secure), Arc::new(state))
        .expect("initialize_mls");
    Ok(protocol)
}

#[test]
fn the_identity_survives_a_restart_over_the_file_stores() {
    let roots = Roots {
        keys: TempRoot::new("engine-keys"),
        state: TempRoot::new("engine-state"),
    };
    let first = engine(&roots, 3).expect("open");
    let address = first.local_address().expect("address").to_string();
    drop(first);

    let second = engine(&roots, 3).expect("reopen");
    assert_eq!(second.local_address(), Some(address.as_str()));
}

#[test]
fn a_wrong_store_key_cannot_start_an_engine_and_changes_nothing() {
    let roots = Roots {
        keys: TempRoot::new("engine-wrong-keys"),
        state: TempRoot::new("engine-wrong-state"),
    };
    drop(engine(&roots, 3).expect("open"));
    let before = files_under(roots.keys.path());

    // Refused at open, so the engine never sees a store in which its identity
    // is missing, which it would answer by minting a new one.
    assert!(matches!(
        engine(&roots, 4),
        Err(FileStoreError::WrongStoreKey(_))
    ));
    assert_eq!(files_under(roots.keys.path()), before);
    assert!(engine(&roots, 3).is_ok());
}

#[cfg(feature = "data")]
#[test]
fn a_sealed_document_survives_a_restart_over_the_file_stores() {
    use offline_protocol_data::DataValue;

    let roots = Roots {
        keys: TempRoot::new("engine-doc-keys"),
        state: TempRoot::new("engine-doc-state"),
    };
    let space = "demo-space";
    {
        let mut protocol = engine(&roots, 5).expect("open");
        protocol.data_create_doc(space, "notes").expect("create");
        protocol
            .data_map_set(
                space,
                "notes",
                "meta",
                "title",
                DataValue::Text {
                    value: "kept across a restart".to_string(),
                },
            )
            .expect("set");
        protocol.data_flush(space, "notes").expect("flush");
    }

    // The document is sealed under the record key before it reaches the
    // state store, so its text is nowhere on disk in the clear.
    for path in files_under(roots.state.path()) {
        let bytes = std::fs::read(&path).expect("read");
        assert!(
            !bytes
                .windows(b"kept across a restart".len())
                .any(|w| w == b"kept across a restart"),
            "{} holds document text in the clear",
            path.display()
        );
    }

    let mut protocol = engine(&roots, 5).expect("reopen");
    assert_eq!(
        protocol
            .data_map_get(space, "notes", "meta", "title")
            .expect("get"),
        Some(DataValue::Text {
            value: "kept across a restart".to_string(),
        })
    );
}

/// A damaged record key must not switch persistence off for good.
///
/// The sealed store never deletes on a read and reports the record as
/// `CorruptedData`. If the engine treated that like a failed load, nothing
/// would ever rewrite the record, and every launch after the damage would
/// refuse to persist sensitive state. The engine regenerates instead, as it
/// does for a key of the wrong length, and the next launch seals again.
#[cfg(feature = "data")]
#[test]
fn a_damaged_record_key_is_regenerated_and_persistence_resumes() {
    use offline_protocol_data::DataValue;

    let roots = Roots {
        keys: TempRoot::new("engine-rk-keys"),
        state: TempRoot::new("engine-rk-state"),
    };
    drop(engine(&roots, 6).expect("open"));

    let record_key = {
        let store = SealedFileMlsStorage::open(
            roots.keys.path(),
            &account_storage_namespace(APP_ID, PROFILE),
            &StaticStoreKey::new([6; 32]),
        )
        .expect("open sealed store");
        store.record_path("protocol_state_record_key", "current")
    };
    let mut bytes = std::fs::read(&record_key).expect("read record key");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&record_key, bytes).expect("damage record key");

    let space = "demo-space";
    {
        let mut protocol = engine(&roots, 6).expect("open over a damaged key");
        protocol.data_create_doc(space, "notes").expect("create");
        protocol
            .data_map_set(
                space,
                "notes",
                "meta",
                "title",
                DataValue::Text {
                    value: "sealed under the new key".to_string(),
                },
            )
            .expect("set");
        protocol.data_flush(space, "notes").expect("flush");
    }

    let mut protocol = engine(&roots, 6).expect("reopen");
    assert_eq!(
        protocol
            .data_map_get(space, "notes", "meta", "title")
            .expect("get"),
        Some(DataValue::Text {
            value: "sealed under the new key".to_string(),
        })
    );
}

/// Runs an engine over `roots`, parks `count` sealed records, and stops.
fn park(roots: &Roots, key: u8, count: usize) {
    let protocol = engine(roots, key).expect("open");
    let storage = protocol
        .protocol_state_storage
        .clone()
        .expect("a state store");
    for index in 0..count {
        protocol
            .write_state_record(
                storage.as_ref(),
                storage_keys::OUTBOX,
                &format!("message-{index}"),
                b"parked",
            )
            .expect("a sealed write");
    }
}

fn sealed_state(keys: &TempRoot, key: u8, state: &TempRoot) -> SealedState {
    let namespace = account_storage_namespace(APP_ID, PROFILE);
    let secure =
        SealedFileMlsStorage::open(keys.path(), &namespace, &StaticStoreKey::new([key; 32]))
            .expect("open the MLS store");
    FileProtocolStateStorage::open(state.path(), &namespace)
        .expect("open the state store")
        .sealed_state(&secure)
        .expect("a verdict")
}

/// The engine deletes a sealed record that does not open, so a state store
/// sealed under another identity's record key is emptied by the first
/// restore. The probe is how a host learns that before it hands the pair
/// over, and it has to tell the three cases apart on real engine output.
#[test]
fn the_probe_tells_a_store_that_belongs_from_one_that_does_not() {
    let ours = Roots {
        keys: TempRoot::new("probe-keys"),
        state: TempRoot::new("probe-state"),
    };
    let theirs = Roots {
        keys: TempRoot::new("probe-other-keys"),
        state: TempRoot::new("probe-other-state"),
    };
    let never_ran = TempRoot::new("probe-fresh-keys");

    // An engine that parked nothing left no sealed record.
    drop(engine(&ours, 8).expect("open"));
    assert_eq!(sealed_state(&ours.keys, 8, &ours.state), SealedState::Empty);

    park(&ours, 8, 3);
    park(&theirs, 8, 1);
    let before = files_under(ours.state.path());

    assert_eq!(sealed_state(&ours.keys, 8, &ours.state), SealedState::Opens);
    // Another identity, with a record key of its own.
    assert_eq!(
        sealed_state(&theirs.keys, 8, &ours.state),
        SealedState::Foreign
    );
    // A store that never ran an engine holds no record key at all.
    assert_eq!(
        sealed_state(&never_ran, 8, &ours.state),
        SealedState::Foreign
    );
    assert_eq!(
        files_under(ours.state.path()),
        before,
        "the probe changes nothing, whatever it answers"
    );

    // One record that opens settles it, whatever else is damaged.
    let damaged = &before
        .iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("k_"))
        })
        .collect::<Vec<_>>()[..2];
    for path in damaged {
        let mut bytes = std::fs::read(path).expect("read");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(path, bytes).expect("damage");
    }
    assert_eq!(sealed_state(&ours.keys, 8, &ours.state), SealedState::Opens);
}

/// A record key the store reports as damaged is an answer, not a failed
/// read, and not the answer a foreign store gets: nothing sealed under it
/// opens again, and the engine's regeneration is what settles the loss.
#[test]
fn a_damaged_record_key_is_reported_as_damaged() {
    let roots = Roots {
        keys: TempRoot::new("probe-rk-keys"),
        state: TempRoot::new("probe-rk-state"),
    };
    park(&roots, 9, 1);
    let namespace = account_storage_namespace(APP_ID, PROFILE);
    let record_key =
        SealedFileMlsStorage::open(roots.keys.path(), &namespace, &StaticStoreKey::new([9; 32]))
            .expect("open sealed store")
            .record_path(
                storage_keys::STATE_RECORD_KEY,
                storage_keys::STATE_RECORD_KEY_ID,
            );
    let mut bytes = std::fs::read(&record_key).expect("read record key");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&record_key, bytes).expect("damage record key");

    assert_eq!(
        sealed_state(&roots.keys, 9, &roots.state),
        SealedState::KeyDamaged
    );
}

/// An engine over a [`FileStorePair`], as the bindings open it.
fn paired_engine(roots: &Roots, key: u8) -> Result<OfflineProtocol, FileStoreError> {
    let pair = FileStorePair::open(
        roots.keys.path(),
        roots.state.path(),
        &account_storage_namespace(APP_ID, PROFILE),
        &StaticStoreKey::new([key; 32]),
    )?;
    let config = ProtocolConfig::builder(APP_ID, PROFILE)
        .build()
        .expect("config");
    let mut protocol = OfflineProtocol::new(config).expect("engine");
    protocol
        .initialize_mls(pair.secure_storage(), pair.state_storage())
        .expect("initialize_mls");
    Ok(protocol)
}

/// Damages the protocol-state record key in the MLS store under `roots`.
fn damage_record_key(roots: &Roots, key: u8) {
    let record_key = SealedFileMlsStorage::open(
        roots.keys.path(),
        &account_storage_namespace(APP_ID, PROFILE),
        &StaticStoreKey::new([key; 32]),
    )
    .expect("open sealed store")
    .record_path(
        storage_keys::STATE_RECORD_KEY,
        storage_keys::STATE_RECORD_KEY_ID,
    );
    let mut bytes = std::fs::read(&record_key).expect("read record key");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&record_key, bytes).expect("damage record key");
}

/// The launch after a record key was regenerated opens as the same identity.
///
/// The damaged-key launch is let through, and the engine regenerates the
/// key, but it cannot delete everything sealed under the lost one: each
/// restore walk stops at its delete allowance. The next launch then finds
/// sealed records that open under no key it holds, which the records alone
/// read as another identity's state. The pair was bound on its first open,
/// and that decides it. The pair here is an engine's first run over stores
/// opened one by one, so it was never bound: the damaged-key launch binds it.
#[test]
fn the_launch_after_a_regenerated_record_key_opens_as_the_same_identity() {
    let roots = Roots {
        keys: TempRoot::new("regen-keys"),
        state: TempRoot::new("regen-state"),
    };
    let parked = crate::protocol::storage::MAX_RESTORE_PRUNE_DELETES + 40;
    park(&roots, 5, parked);
    damage_record_key(&roots, 5);

    let address = {
        let protocol = paired_engine(&roots, 5).expect("the damaged-key launch opens");
        protocol.local_address().expect("address").to_string()
    };
    let namespace = account_storage_namespace(APP_ID, PROFILE);
    let leftovers = {
        use crate::protocol_state_storage::ProtocolStateStorage;
        FileProtocolStateStorage::open(roots.state.path(), &namespace)
            .expect("open")
            .list_keys(storage_keys::OUTBOX)
            .expect("list")
            .len()
    };
    assert!(
        leftovers > 0,
        "the fixture has to leave records under the lost key, or it proves nothing"
    );
    assert_eq!(
        sealed_state(&roots.keys, 5, &roots.state),
        SealedState::Foreign,
        "what the records alone say about the same identity's own state"
    );

    for launch in ["the next launch", "and the one after"] {
        let protocol = paired_engine(&roots, 5).unwrap_or_else(|err| panic!("{launch}: {err}"));
        assert_eq!(protocol.local_address(), Some(address.as_str()));
    }
}

/// The same, for a document: documents are read when they are opened, never
/// at launch, so the damaged-key launch deletes nothing of them. This pair is
/// bound from its first run.
#[cfg(feature = "data")]
#[test]
fn a_document_under_a_lost_record_key_does_not_turn_its_own_state_foreign() {
    use offline_protocol_data::DataValue;

    let roots = Roots {
        keys: TempRoot::new("regen-doc-keys"),
        state: TempRoot::new("regen-doc-state"),
    };
    let address = {
        let mut protocol = paired_engine(&roots, 6).expect("open");
        protocol
            .data_create_doc("demo-space", "notes")
            .expect("create");
        protocol
            .data_map_set(
                "demo-space",
                "notes",
                "meta",
                "title",
                DataValue::Text {
                    value: "sealed under the first key".to_string(),
                },
            )
            .expect("set");
        protocol.data_flush("demo-space", "notes").expect("flush");
        protocol.local_address().expect("address").to_string()
    };
    damage_record_key(&roots, 6);
    drop(paired_engine(&roots, 6).expect("the damaged-key launch opens"));
    assert_eq!(
        sealed_state(&roots.keys, 6, &roots.state),
        SealedState::Foreign,
        "the document is still there, under the lost key"
    );

    let protocol = paired_engine(&roots, 6).expect("the launch after it opens");
    assert_eq!(protocol.local_address(), Some(address.as_str()));
}

#[test]
fn a_second_engine_over_the_same_stores_is_refused() {
    let roots = Roots {
        keys: TempRoot::new("engine-in-use-keys"),
        state: TempRoot::new("engine-in-use-state"),
    };
    let first = engine(&roots, 7).expect("open");
    assert!(matches!(engine(&roots, 7), Err(FileStoreError::InUse(_))));
    drop(first);
    assert!(engine(&roots, 7).is_ok());
}
