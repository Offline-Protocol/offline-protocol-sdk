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
    account_storage_namespace, FileProtocolStateStorage, FileStoreError, SealedFileMlsStorage,
    StaticStoreKey,
};
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
