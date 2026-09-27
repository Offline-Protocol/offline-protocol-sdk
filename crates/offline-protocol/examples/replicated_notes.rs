//! Replicated documents on one device: open a store, edit, persist, reopen.
//!
//! Run it with:
//!
//! ```text
//! cargo run --package offline-protocol --example replicated_notes
//! ```
//!
//! What it shows, in order: that a document needs no storage setup, that the
//! four collection types behave the way an application expects, that a flush
//! is what makes a change durable, and that the same records reopen into the
//! same document after the engine is rebuilt.
//!
//! What it deliberately does not show is replication, which needs a second
//! device, a confirmed MLS session, and a transport. The merge semantics that
//! makes replication safe is a separate example that needs none of those:
//! `cargo run --package offline-protocol-data --example offline_merge`.
//!
//! The two stores are the built-in file stores a headless host uses: MLS
//! material sealed under a store key, and protocol state in the format every
//! binding writes. They live under a directory given as the first argument
//! (a fresh temporary directory otherwise). The store key comes from
//! `OFFLINE_PROTOCOL_STORE_KEY` (64 hex digits or base64) when it is set; the
//! example otherwise mints one for the run, which is enough for the reopen
//! below but not for a second run over the same directory.
//!
//! An application that brings its own storage (SQLite, its own encrypted
//! store) swaps it in at exactly the `initialize_mls` call, and sealing sits
//! above it, so what the protocol-state store receives is already ciphertext.

use std::path::PathBuf;
use std::sync::Arc;

use offline_protocol::{
    account_storage_namespace, DataValue, EnvStoreKey, FileProtocolStateStorage, OfflineProtocol,
    ProtocolConfig, SealedFileMlsStorage, StaticStoreKey, StoreKeyProvider,
};
use rand_core::{OsRng, RngCore};

const APP_ID: &str = "com.example.notes";
const PROFILE: &str = "default";

/// Opens the two stores for this account under `root`.
fn open_stores(
    root: &std::path::Path,
    key: &dyn StoreKeyProvider,
) -> Result<(Arc<SealedFileMlsStorage>, Arc<FileProtocolStateStorage>), Box<dyn std::error::Error>>
{
    let namespace = account_storage_namespace(APP_ID, PROFILE);
    // Two roots: MLS material may outlive an installation, protocol state
    // must not.
    let secure = SealedFileMlsStorage::open(root.join("keys"), &namespace, key)?;
    let records = FileProtocolStateStorage::open(root.join("state"), &namespace)?;
    Ok((Arc::new(secure), Arc::new(records)))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = match std::env::args_os().nth(1) {
        Some(root) => PathBuf::from(root),
        None => {
            let mut suffix = [0u8; 8];
            OsRng.fill_bytes(&mut suffix);
            std::env::temp_dir().join(format!("replicated-notes-{}", hex::encode(suffix)))
        }
    };
    let key: Box<dyn StoreKeyProvider> =
        if std::env::var_os(EnvStoreKey::DEFAULT_VARIABLE).is_some() {
            Box::new(EnvStoreKey::default())
        } else {
            let mut key = [0u8; 32];
            OsRng.fill_bytes(&mut key);
            Box::new(StaticStoreKey::new(key))
        };
    println!("stores:    {}", root.display());

    // Both stores are on disk, which is what makes the reopen at the end a
    // reopen rather than a fresh start. The secure store matters as much as
    // the record store: documents are sealed with a key that lives in it, so
    // losing only that half looks exactly like data loss.
    let (secure, records) = open_stores(&root, key.as_ref())?;

    // `data.enabled` defaults to true, so nothing here switches the layer on.
    let config = ProtocolConfig::builder(APP_ID, PROFILE).build()?;
    let mut protocol = OfflineProtocol::new(config)?;

    // Documents are sealed at rest and the record key is minted here, so this
    // call is the prerequisite for every method below.
    protocol.initialize_mls(secure.clone(), records.clone())?;

    // A space is an MLS scope: a peer's address for a 1:1 space, or the group
    // id for a group. Nothing replicates in this example, so the name only
    // has to be a valid one.
    let space = "demo-space";

    protocol.data_create_doc(space, "trip")?;

    // A map: last writer wins per key, and different keys never conflict.
    protocol.data_map_set(
        space,
        "trip",
        "meta",
        "title",
        DataValue::Text {
            value: "Coast road".to_string(),
        },
    )?;

    // A list: concurrent insertions both survive, in a deterministic order.
    for item in ["water", "map", "torch"] {
        protocol.data_list_push(
            space,
            "trip",
            "packing",
            DataValue::Text {
                value: item.to_string(),
            },
        )?;
    }

    // Text: character-level merge, so two people typing in one paragraph keep
    // both sets of words. Positions are character offsets, not byte offsets.
    protocol.data_text_insert(space, "trip", "notes", 0, "Meet at the bridge")?;

    // A counter: increments add up rather than overwriting each other.
    protocol.data_counter_increment(space, "trip", "opened", 1.0)?;

    // Edits batch before they reach storage. This is the call that makes them
    // durable, and `data_changed` fires after the same point, never before.
    protocol.data_flush(space, "trip")?;

    println!("spaces:    {:?}", protocol.data_list_spaces()?);
    println!("documents: {:?}", protocol.data_list_docs(space)?);
    println!(
        "compacted: {} bytes",
        protocol.data_doc_size(space, "trip")?
    );
    println!(
        "history:   {} bytes",
        protocol.data_export_raw(space, "trip")?.len()
    );
    println!("json:      {}", protocol.data_doc_json(space, "trip")?);

    // Both halves of the escape hatch are above: `data_doc_json` is the plain
    // JSON of the current state, `data_export_raw` the engine-native history.
    // An application can always take its data and leave.

    // Reopen: a new engine over the same two directories, opened afresh as a
    // restarted process would. The records were sealed by the first engine
    // and are read back by the second, which is the property an application
    // depends on across a restart.
    drop(protocol);
    drop((secure, records));
    let (secure, records) = open_stores(&root, key.as_ref())?;
    let config = ProtocolConfig::builder(APP_ID, PROFILE).build()?;
    let mut reopened = OfflineProtocol::new(config)?;
    reopened.initialize_mls(secure, records)?;

    println!("after reopen");
    println!(
        "  title:   {:?}",
        reopened.data_map_get(space, "trip", "meta", "title")?
    );
    println!(
        "  packing: {}",
        reopened.data_list_len(space, "trip", "packing")?
    );
    println!(
        "  notes:   {:?}",
        reopened.data_text_value(space, "trip", "notes")?
    );
    println!(
        "  opened:  {}",
        reopened.data_counter_value(space, "trip", "opened")?
    );

    Ok(())
}
