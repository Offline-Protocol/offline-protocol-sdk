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
//! The backend below is an in-memory `ProtocolStateStorage`, written out in
//! full because it is also the shape of the bring-your-own-storage seam: an
//! application swaps SQLite, files, or its own encrypted store in at exactly
//! this point, and sealing sits above it, so what `store` receives here is
//! already ciphertext.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use offline_protocol::{
    DataValue, OfflineProtocol, ProtocolConfig, ProtocolStateResult, ProtocolStateStorage,
};
use offline_protocol_mls::storage::InMemoryStorage;

/// A protocol-state backend that keeps records in a map.
///
/// Four methods over `(key_type, key_id, bytes)`. The rules a real adapter
/// has to honour are the ones the conformance suite checks: bytes are stored
/// verbatim, a second write to one key replaces the first, and a key that was
/// never written loads as `None` rather than as an error.
#[derive(Default)]
struct MemoryStateStorage {
    records: Mutex<HashMap<(String, String), Vec<u8>>>,
}

impl ProtocolStateStorage for MemoryStateStorage {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
        let mut records = self.records.lock().expect("storage mutex");
        records.insert((key_type.to_string(), key_id.to_string()), data.to_vec());
        Ok(())
    }

    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
        let records = self.records.lock().expect("storage mutex");
        Ok(records
            .get(&(key_type.to_string(), key_id.to_string()))
            .cloned())
    }

    fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
        let mut records = self.records.lock().expect("storage mutex");
        records.remove(&(key_type.to_string(), key_id.to_string()));
        Ok(())
    }

    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
        let records = self.records.lock().expect("storage mutex");
        Ok(records
            .keys()
            .filter(|(stored_type, _)| stored_type == key_type)
            .map(|(_, key_id)| key_id.clone())
            .collect())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Both stores outlive the engine below, which is what makes the reopen at
    // the end a reopen rather than a fresh start. The secure store matters as
    // much as the record store: documents are sealed with a key that lives in
    // it, so dropping only that half looks exactly like data loss.
    let secure = Arc::new(InMemoryStorage::default());
    let records = Arc::new(MemoryStateStorage::default());

    // `data.enabled` defaults to true, so nothing here switches the layer on.
    let config = ProtocolConfig::builder("com.example.notes", "default").build()?;
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

    // Reopen: a new engine over the same two stores. The records were sealed
    // by the first one and are read back by the second, which is the property
    // an application depends on across a restart.
    drop(protocol);
    let config = ProtocolConfig::builder("com.example.notes", "default").build()?;
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
